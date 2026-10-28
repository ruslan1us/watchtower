//! Plan usage (doc/0043): the `/usage` probe's argv and folder, the
//! lenient parser, hiding the probe's own row, and the schedule (interval,
//! minimum, single flight, stale values, the rate-limit backoff and `r`).

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, fixture};
use fake_claude::{Fake, cat};
use watchtower::app::{App, Effect};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::listing::{Kind, parse_listing};
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;
use watchtower::usage::{
    self, Clock, Level, LimitKind, MAX_RUNNING, PROBE_ARGS, ProbeFolder, Probed, REFRESH_AFTER,
    Shown, UsageBoard, UsageJob, parse_iso8601, parse_probe, reset_text,
};

/// 2026-10-28T12:00:00Z.
const NOON_MS: i64 = 1_793_188_800_000;

fn probed(name: &str) -> Probed {
    parse_probe(&fixture(&format!("usage/{name}")), NOON_MS, Clock::UTC).unwrap()
}

// ---- the parser ---------------------------------------------------------------------

#[test]
fn the_time_constant_is_noon_utc() {
    assert_eq!(parse_iso8601("2026-10-28T12:00:00Z"), Some(NOON_MS));
}

#[test]
fn a_plan_reports_its_limits_in_order() {
    let p = probed("plan.jsonl");
    assert_eq!(p.version.as_deref(), Some("2.1.312"));
    assert!(!p.last_known);
    assert_eq!(p.rate_limited, None);
    let u = p.usage.expect("plan data");
    let kinds: Vec<&LimitKind> = u.limits.iter().map(|l| &l.kind).collect();
    assert_eq!(
        kinds,
        [
            &LimitKind::Session,
            &LimitKind::WeeklyAll,
            &LimitKind::WeeklyScoped
        ]
    );
    let s = u.session().unwrap();
    assert_eq!((s.window(), s.percent_text()), ("5h".into(), "24%".into()));
    assert_eq!(s.resets_at_ms, Some(NOON_MS + 6 * 3_600_000 + 177));
    assert_eq!(s.level(), Level::Normal);
    assert_eq!(s.active, Some(true));
    let w = u.weekly().unwrap();
    assert_eq!((w.window(), w.percent_text()), ("7d".into(), "12%".into()));
    let scoped = &u.limits[2];
    assert_eq!(scoped.scope_label().as_deref(), Some("Fable"));
    assert_eq!(scoped.window(), "7d");
    let extra = u.extra.unwrap();
    assert!(!extra.enabled);
}

#[test]
fn a_null_report_means_no_plan_data() {
    let p = probed("no-plan.jsonl");
    assert_eq!(p.usage, None);
    assert!(p.rate_limited.is_none());
}

#[test]
fn unknown_kinds_floats_and_odd_rows_are_read_leniently() {
    let p = probed("odd.jsonl");
    let u = p.usage.clone().expect("plan data");
    // The string `"not a limit"` is skipped; everything else is kept.
    assert_eq!(u.limits.len(), 5);
    let s = u.session().unwrap();
    assert_eq!(s.percent, Some(81.6));
    assert_eq!(s.percent_text(), "82%");
    assert_eq!(s.level(), Level::Warning);
    assert_eq!(u.weekly().unwrap().level(), Level::Critical);
    let scoped = &u.limits[2];
    assert_eq!(scoped.scope_label().as_deref(), Some("Opus"));
    assert_eq!(scoped.percent_text(), "40%");
    let other = &u.limits[3];
    assert_eq!(other.kind, LimitKind::Unknown("monthly_surface".into()));
    assert_eq!(other.window(), "monthly_surface");
    assert_eq!(other.percent, Some(7.0), "a number as a string");
    assert_eq!(other.scope_label().as_deref(), Some("web"));
    assert_eq!(other.level(), Level::Warning, "an unknown severity");
    assert_eq!(other.resets_at_ms, None);
    let bare = &u.limits[4];
    assert_eq!(bare.kind, LimitKind::Unknown(String::new()));
    assert_eq!(bare.severity, None);
    assert_eq!(bare.level(), Level::Normal);
    let extra = u.extra.unwrap();
    assert!(extra.enabled);
    assert_eq!(extra.text(), "12.50 of 50 USD");
    // The text's signals.
    assert!(p.last_known);
    let r = p.rate_limited.unwrap();
    assert_eq!(r.stated, "1:30pm");
    assert_eq!(r.until_ms, Some(NOON_MS + 90 * 60_000));
}

#[test]
fn the_stated_rate_limit_time_is_read_with_or_without_a_date() {
    let line = |text: &str| {
        format!(
            "{{\"type\":\"result\",\"is_error\":false,\"result\":{}}}",
            serde_json::to_string(text).unwrap()
        )
    };
    let until = |text: &str| {
        parse_probe(&line(text), NOON_MS, Clock::UTC)
            .unwrap()
            .rate_limited
            .and_then(|r| r.until_ms)
    };
    let h = 3_600_000;
    assert_eq!(
        until("rate limited until 13:15"),
        Some(NOON_MS + h + 15 * 60_000)
    );
    assert_eq!(until("Rate limited until 3pm."), Some(NOON_MS + 3 * h));
    // Earlier today: tomorrow.
    assert_eq!(until("rate limited until 9am"), Some(NOON_MS + 21 * h));
    assert_eq!(until("rate limited until 12am"), Some(NOON_MS + 12 * h));
    assert_eq!(
        until("rate limited until Oct 29, 9am (UTC)"),
        Some(NOON_MS + 21 * h)
    );
    // Unreadable: kept as stated, no time.
    let p = parse_probe(&line("rate limited until soon"), NOON_MS, Clock::UTC).unwrap();
    let r = p.rate_limited.unwrap();
    assert_eq!((r.stated.as_str(), r.until_ms), ("soon", None));
    assert!(until("all fine").is_none());
}

#[test]
fn an_error_result_or_no_output_is_an_error() {
    let err = parse_probe(
        r#"{"type":"result","is_error":true,"result":"Not logged in\nRun /login"}"#,
        NOON_MS,
        Clock::UTC,
    )
    .unwrap_err();
    assert_eq!(err, "/usage: Not logged in");
    assert_eq!(
        parse_probe("", NOON_MS, Clock::UTC).unwrap_err(),
        "no /usage output"
    );
    assert_eq!(
        parse_probe("garbage\n{}", NOON_MS, Clock::UTC).unwrap_err(),
        "no /usage output"
    );
}

#[test]
fn iso_times_are_read_in_their_offset() {
    let base = parse_iso8601("2026-10-28T12:00:00Z").unwrap();
    assert_eq!(parse_iso8601("2026-10-28T12:00:00.5Z"), Some(base + 500));
    assert_eq!(parse_iso8601("2026-10-28T14:00:00+02:00"), Some(base));
    assert_eq!(parse_iso8601("2026-10-28T07:30:00-0430"), Some(base));
    assert_eq!(parse_iso8601("2026-10-28T12:00"), Some(base));
    assert_eq!(parse_iso8601("2026-10-28 12:00:00"), Some(base));
    for bad in [
        "",
        "2026-10-28",
        "2026-13-01T00:00:00Z",
        "noon",
        "2026-10-28T25:00Z",
    ] {
        assert_eq!(parse_iso8601(bad), None, "{bad}");
    }
}

#[test]
fn reset_times_show_the_weekday_when_not_today() {
    let h = 3_600_000;
    let at = |ms| reset_text(ms, NOON_MS, false, Clock::UTC).unwrap();
    assert_eq!(at(NOON_MS + 6 * h), "18:00");
    // 2026-10-29 is a Thursday.
    assert_eq!(at(NOON_MS + 13 * h), "Thu 01:00");
    assert_eq!(
        reset_text(NOON_MS + 6 * h, NOON_MS, true, Clock::UTC).unwrap(),
        "Wed 18:00"
    );
    assert_eq!(at(NOON_MS - 1), "now");
}

#[test]
fn ages_are_short() {
    assert_eq!(usage::age_text(42_000), "42s");
    assert_eq!(usage::age_text(12 * 60_000), "12m");
    assert_eq!(usage::age_text(3 * 3_600_000), "3h");
    assert_eq!(usage::age_text(2 * 86_400_000), "2d");
}

// ---- the probe ------------------------------------------------------------------------

#[test]
fn the_probe_runs_slash_usage_in_its_own_folder() {
    let _guard = fake_claude::lock();
    let fake = Fake::new("claude", &cat("usage/plan.jsonl"));
    let home = tempfile::tempdir().unwrap();
    let dir = usage::dir(&home.path().join(".config/watchtower"));
    assert!(!dir.exists());
    let (folder, result) = usage::run_probe(&CliRunner::new(), &fake.account(), &dir, Clock::UTC);
    let p = result.unwrap();
    assert!(p.usage.is_some());
    assert_eq!(
        fake.argv(),
        PROBE_ARGS
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fake.argv(),
        [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--safe-mode",
            "/usage"
        ]
    );
    let canonical = std::fs::canonicalize(&dir).unwrap();
    assert_eq!(fake.cwd(), canonical);
    assert_eq!(folder, Some(canonical));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    // An inherited CLAUDE_CONFIG_DIR never reaches it (doc/0017).
    assert_eq!(fake.env_var_of(1, "CLAUDE_CONFIG_DIR"), None);
}

#[test]
fn a_failed_probe_says_why() {
    let _guard = fake_claude::lock();
    let fake = Fake::new("claude", "echo 'error: something broke' >&2; exit 3");
    let home = tempfile::tempdir().unwrap();
    let dir = usage::dir(home.path());
    let (_, result) = usage::run_probe(&CliRunner::new(), &fake.account(), &dir, Clock::UTC);
    assert_eq!(
        result.unwrap_err(),
        "/usage failed (exit 3): error: something broke"
    );
    let fake = Fake::new("claude", "exit 0");
    let (_, result) = usage::run_probe(&CliRunner::new(), &fake.account(), &dir, Clock::UTC);
    assert_eq!(result.unwrap_err(), "no /usage output");
}

// ---- the probe's own row ----------------------------------------------------------

fn listing_with(cwd: &str) -> String {
    format!(
        r#"[
  {{ "pid": 4201, "cwd": "{cwd}", "kind": "interactive", "startedAt": 1793188800000,
     "sessionId": "1a2b3c4d-0000-4000-8000-0000000000aa", "name": "usage-8b", "status": "idle" }},
  {{ "pid": 4202, "cwd": "{HOME}/workspace/api", "kind": "interactive", "startedAt": 1793188800000,
     "sessionId": "1a2b3c4d-0000-4000-8000-0000000000bb", "name": "api-1c", "status": "idle" }},
  {{ "pid": 4203, "id": "5e6f7a8b", "cwd": "{cwd}", "kind": "background", "startedAt": 1793188800000,
     "sessionId": "5e6f7a8b-0000-4000-8000-000000000000", "name": "odd-one", "state": "working",
     "status": "busy" }}
]"#
    )
}

#[test]
fn the_probe_row_is_hidden_by_its_folder() {
    let folder = ProbeFolder {
        path: "/home/user/.config/watchtower/usage".into(),
        canonical: Some("/home/user/dotfiles/watchtower/usage".into()),
    };
    for cwd in [
        "/home/user/.config/watchtower/usage",
        "/home/user/dotfiles/watchtower/usage",
    ] {
        let mut listing = parse_listing(&listing_with(cwd)).unwrap();
        folder.hide(&mut listing);
        let names: Vec<&str> = listing
            .sessions
            .iter()
            .map(|s| s.name.as_deref().unwrap())
            .collect();
        // Only the interactive row in the folder: a background session
        // there is a session.
        assert_eq!(names, ["api-1c", "odd-one"], "{cwd}");
        assert!(listing.sessions.iter().any(|s| s.kind == Kind::Background));
    }
}

fn app_with(config: &str) -> App {
    let config = Config::parse(config, Path::new(HOME)).unwrap();
    App::new(config, HOME.into(), Sidecar::default(), Box::new(NoEnviron))
}

const TWO: &str = r#"
[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#;

#[test]
fn the_app_hides_the_probe_row_from_its_list() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_with(TWO);
    app.set_config_dir(tmp.path());
    let cwd = usage::dir(tmp.path()).display().to_string();
    for account in ["main", "alt"] {
        app.apply_poll(PollUpdate {
            account: account.into(),
            cycle: 1,
            result: Ok(parse_listing(&listing_with(&cwd)).unwrap()),
        });
    }
    let names: Vec<String> = app
        .sessions()
        .iter()
        .map(|m| m.session.display_name())
        .collect();
    assert!(!names.iter().any(|n| n == "usage-8b"), "{names:?}");
    assert!(names.iter().any(|n| n == "api-1c"), "{names:?}");
    assert_eq!(app.usage_folder().path, usage::dir(tmp.path()));
}

// ---- the schedule ---------------------------------------------------------------------

const MIN: Duration = Duration::from_secs(60);
const FIVE: Duration = Duration::from_secs(300);

fn ok() -> Result<Probed, String> {
    Ok(probed("plan.jsonl"))
}

#[test]
fn every_account_is_probed_at_once_one_at_a_time() {
    assert_eq!(MAX_RUNNING, 1);
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    let names = ["main", "alt"];
    assert_eq!(b.due(&names, t0, FIVE), ["main"]);
    // Single flight: nothing more while it runs.
    assert!(b.due(&names, t0, FIVE).is_empty());
    assert!(b.get("main").unwrap().running());
    b.finished("main", ok(), t0, NOON_MS, FIVE);
    assert_eq!(b.due(&names, t0, FIVE), ["alt"]);
    b.finished("alt", ok(), t0, NOON_MS, FIVE);
    assert!(b.due(&names, t0 + FIVE - MIN, FIVE).is_empty());
    // One interval after each start.
    assert_eq!(b.due(&names, t0 + FIVE, FIVE), ["main"]);
}

#[test]
fn refresh_asks_again_only_after_a_minute() {
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    let names = ["main"];
    b.due(&names, t0, FIVE);
    b.finished("main", ok(), t0, NOON_MS, FIVE);
    b.refresh(&names, t0 + Duration::from_secs(30));
    assert!(b.due(&names, t0 + Duration::from_secs(30), FIVE).is_empty());
    let later = t0 + REFRESH_AFTER + Duration::from_secs(1);
    b.refresh(&names, later);
    assert_eq!(b.due(&names, later, FIVE), ["main"]);
    // A refresh while it runs is no second probe.
    b.refresh(&names, later + REFRESH_AFTER * 2);
    assert!(b.due(&names, later + REFRESH_AFTER * 2, FIVE).is_empty());
}

#[test]
fn values_go_stale_after_two_intervals_or_a_failed_probe() {
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    b.due(&["main"], t0, FIVE);
    b.finished("main", ok(), t0, NOON_MS, FIVE);
    let shown = |b: &UsageBoard, now_ms| match b.get("main").unwrap().shown(now_ms, FIVE) {
        Shown::Plan { stale, age_ms, .. } => (stale, age_ms),
        other => panic!("{other:?}"),
    };
    assert_eq!(shown(&b, NOON_MS + 600_000), (false, 600_000));
    assert_eq!(shown(&b, NOON_MS + 600_001), (true, 600_001));
    // A failed probe keeps the value, stale, with its error.
    b.due(&["main"], t0 + FIVE, FIVE);
    b.finished(
        "main",
        Err("/usage timed out after 20s".into()),
        t0 + FIVE,
        NOON_MS + 300_000,
        FIVE,
    );
    match b.get("main").unwrap().shown(NOON_MS + 300_000, FIVE) {
        Shown::Plan {
            stale,
            error,
            age_ms,
            ..
        } => {
            assert!(stale);
            assert_eq!(error, Some("/usage timed out after 20s"));
            assert_eq!(age_ms, 300_000);
        }
        other => panic!("{other:?}"),
    }
    // Without an earlier value a failure is shown as such.
    let mut c = UsageBoard::default();
    c.due(&["alt"], t0, FIVE);
    c.finished("alt", Err("boom".into()), t0, NOON_MS, FIVE);
    assert_eq!(
        c.get("alt").unwrap().shown(NOON_MS, FIVE),
        Shown::Failed("boom")
    );
    // No plan data.
    c.due(&["alt"], t0 + FIVE, FIVE);
    c.finished("alt", Ok(probed("no-plan.jsonl")), t0 + FIVE, NOON_MS, FIVE);
    assert_eq!(c.get("alt").unwrap().shown(NOON_MS, FIVE), Shown::NoPlan);
}

#[test]
fn last_known_numbers_are_stale_at_once() {
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    b.due(&["main"], t0, FIVE);
    b.finished("main", Ok(probed("odd.jsonl")), t0, NOON_MS, FIVE);
    match b.get("main").unwrap().shown(NOON_MS, FIVE) {
        Shown::Plan {
            stale,
            snapshot,
            rate_limited,
            ..
        } => {
            assert!(stale);
            assert!(snapshot.last_known);
            assert_eq!(rate_limited.unwrap().stated, "1:30pm");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_rate_limit_holds_the_next_probe_until_its_time() {
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    let names = ["main"];
    b.due(&names, t0, MIN);
    // "rate limited until 1:30pm", 90 minutes after noon.
    b.finished("main", Ok(probed("odd.jsonl")), t0, NOON_MS, MIN);
    assert!(b.due(&names, t0 + MIN, MIN).is_empty());
    b.refresh(&names, t0 + Duration::from_secs(3600));
    assert!(
        b.due(&names, t0 + Duration::from_secs(3600), MIN)
            .is_empty()
    );
    assert_eq!(
        b.due(&names, t0 + Duration::from_secs(90 * 60), MIN),
        ["main"]
    );
}

#[test]
fn reset_forgets_values_but_keeps_a_running_probe() {
    let t0 = Instant::now();
    let mut b = UsageBoard::default();
    let names = ["main", "alt"];
    b.due(&names, t0, FIVE);
    b.finished("main", ok(), t0, NOON_MS, FIVE);
    b.due(&names, t0, FIVE);
    assert!(b.get("alt").unwrap().running());
    b.reset();
    assert!(b.get("main").is_none());
    assert!(b.get("alt").unwrap().running());
    assert!(b.get("alt").unwrap().latest.is_none());
    // Nothing new beside the running one; main probes once alt is done.
    assert!(b.due(&names, t0, FIVE).is_empty());
    b.finished("alt", ok(), t0, NOON_MS, FIVE);
    assert_eq!(b.due(&names, t0, FIVE), ["main"]);
}

// ---- the app ------------------------------------------------------------------------------

fn probes(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::ProbeUsage(job) => Some(job.account.name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn nothing_is_probed_while_usage_is_off() {
    let mut app = app_with(TWO);
    assert!(!app.config().usage_show, "off by default");
    let now = Instant::now();
    assert!(probes(&app.tick(now)).is_empty());
    assert!(probes(&app.tick(now + Duration::from_secs(3600))).is_empty());
    assert_eq!(app.usage_shown("main", NOON_MS), None);
}

#[test]
fn the_app_probes_on_its_tick_while_usage_is_on() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_with(&format!("[usage]\nshow = true\nrefresh_secs = 60\n{TWO}"));
    app.set_config_dir(tmp.path());
    assert_eq!(app.usage_shown("main", NOON_MS), Some(Shown::Waiting));
    let now = Instant::now();
    let effects = app.tick(now);
    let job = effects
        .iter()
        .find_map(|e| match e {
            Effect::ProbeUsage(job) => Some(job.clone()),
            _ => None,
        })
        .expect("a probe");
    assert_eq!(
        job,
        UsageJob {
            account: app.config().account("main").unwrap().clone(),
            dir: usage::dir(tmp.path()),
        }
    );
    app.usage_received("main", ok(), now, NOON_MS);
    assert_eq!(probes(&app.tick(now)), ["alt"]);
    app.usage_received("alt", ok(), now, NOON_MS);
    assert!(probes(&app.tick(now + Duration::from_secs(59))).is_empty());
    assert_eq!(probes(&app.tick(now + MIN)), ["main"]);
    assert!(matches!(
        app.usage_shown("main", NOON_MS),
        Some(Shown::Plan { stale: false, .. })
    ));
}

#[test]
fn r_refreshes_usage_older_than_a_minute() {
    let mut app = app_with(&format!("[usage]\nshow = true\n{TWO}"));
    let start = Instant::now() - Duration::from_secs(120);
    for name in ["main", "alt"] {
        let effects = app.tick(start);
        assert_eq!(probes(&effects), [name]);
        app.usage_received(name, ok(), start, NOON_MS);
    }
    assert!(probes(&app.tick(Instant::now())).is_empty());
    // `r` still asks for a poll, as before.
    assert_eq!(app.handle_key(ch('r')), [Effect::PollNow]);
    assert_eq!(probes(&app.tick(Instant::now())), ["main"]);
}

#[test]
fn turning_usage_off_forgets_it_and_on_again_probes_at_once() {
    let mut app = app_with(&format!("[usage]\nshow = true\n{TWO}"));
    let now = Instant::now();
    app.tick(now);
    app.usage_received("main", ok(), now, NOON_MS);
    let off = Config::parse(TWO, Path::new(HOME)).unwrap();
    app.apply_config(off.clone());
    assert_eq!(app.usage_shown("main", NOON_MS), None);
    assert!(probes(&app.tick(now)).is_empty());
    let mut on = off;
    on.usage_show = true;
    app.apply_config(on);
    assert_eq!(app.usage_shown("main", NOON_MS), Some(Shown::Waiting));
    assert_eq!(probes(&app.tick(now)), ["main"]);
}
