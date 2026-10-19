//! The hand-off as a state machine (doc/0004, doc/0011, doc/0015): which
//! rows offer it, target selection with 1, 2 and 3 accounts, the
//! confirmation, progress in the status line, the sidecar update, copy
//! handling on the next poll, the trust retry, and that ownership
//! resolution agrees with the hand-off afterwards.

mod common;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{
    HOME, NoEnviron, app_single, app_two_with, ch, ctrl, key, poll, refresh_for_attach, status_text,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use watchtower::app::{App, Confirm, Effect, HandoffJob, Overlay, Severity};
use watchtower::config::Config;
use watchtower::handoff::{HandoffError, Outcome, Resumed, Step};
use watchtower::listing::parse_listing;
use watchtower::merge::{EnvironReader, Ownership};
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};

const SAY_OK: &str = "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81";
const TOUCH: &str = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";
const PRINT_DATE: &str = "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93";

/// A sidecar where `main` started every session of `after-stop-all.json`.
fn owned_by_main() -> Sidecar {
    let mut sc = Sidecar::default();
    for sid in [SAY_OK, TOUCH, PRINT_DATE] {
        sc.record_owner(sid, "main", AccountSource::Started);
    }
    sc
}

/// Two accounts with shared lists (both list everything) and an
/// unreadable environ, so owners come from the sidecar (rule 3).
fn app_two_owned() -> App {
    let mut app = app_two_with(owned_by_main());
    poll_both(&mut app, "after-stop-all.json");
    app
}

/// [`app_two_owned`] with `pid` → (`CLAUDE_CONFIG_DIR`, raw start ticks)
/// for some Claude Code processes.
fn app_two_environ(pids: &[(u32, &str, u64)]) -> App {
    let home = Path::new(HOME);
    let config = Config::parse(
        r#"
[[accounts]]
name = "main"
command = "claude"
[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#,
        home,
    )
    .unwrap();
    let environ = Environ(
        pids.iter()
            .map(|(p, d, start)| (*p, (OsString::from(d), *start)))
            .collect(),
    );
    let mut app = App::new(config, home.into(), owned_by_main(), Box::new(environ));
    poll_both(&mut app, "after-stop-all.json");
    app
}

fn poll_both(app: &mut App, fixture: &str) {
    app.apply_poll(poll("main", fixture));
    app.apply_poll(poll("alt", fixture));
}

const THREE: &str = r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"

[[accounts]]
name = "spare"
command = "claude-spare"
config_dir = "~/.claude-spare"
"#;

fn app_three(sidecar: Sidecar) -> App {
    let home = Path::new(HOME);
    let config = Config::parse(THREE, home).unwrap();
    let mut app = App::new(config, home.into(), sidecar, Box::new(NoEnviron));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(poll(a, "after-stop-all.json"));
    }
    app
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

fn handoff_of(effects: &[Effect]) -> Option<HandoffJob> {
    effects.iter().find_map(|e| match e {
        Effect::Handoff(job) => Some(job.clone()),
        _ => None,
    })
}

/// Presses `k`, which must open the hand-off confirmation without running
/// anything (every hand-off asks, doc/0004), then confirms with `y`.
fn confirmed(app: &mut App, k: KeyEvent) -> Option<HandoffJob> {
    let effects = app.handle_key(k);
    assert!(handoff_of(&effects).is_none(), "asks first: {effects:?}");
    assert!(
        matches!(app.overlay(), Overlay::Confirm(Confirm::Handoff { .. })),
        "{:?}",
        app.overlay()
    );
    handoff_of(&app.handle_key(ch('y')))
}

fn owner_of(app: &App, sid: &str) -> Ownership {
    app.sessions()
        .iter()
        .find(|m| m.session.session_id.as_deref() == Some(sid))
        .map(|m| m.owner.clone())
        .unwrap()
}

fn continued(id: &str) -> Resumed {
    Resumed {
        id: id.into(),
        name: None,
        outcome: Outcome::Continued,
        notes: vec![format!(
            "note: woke session {id} with its saved options (--name, --model)."
        )],
        idle: false,
    }
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- which rows, which targets ----------------------------------------------

#[test]
fn one_account_says_a_second_is_needed() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(
        status_text(&app).contains("only one account"),
        "{}",
        status_text(&app)
    );
    assert_eq!(app.overlay(), &Overlay::None);
}

#[test]
fn interactive_rows_and_rows_without_session_id_are_refused() {
    let mut app = app_two_owned();
    select(&mut app, "demo-app-3f");
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(
        status_text(&app).contains(
            "demo-app-3f runs in another terminal — press ← on an empty prompt there (or /bg)"
        ),
        "{}",
        status_text(&app)
    );

    let mut app = app_two_with(Sidecar::default());
    let rows = r#"[{"id": "1a2b3c4d", "cwd": "/home/user/workspace/demo-app",
        "kind": "background", "name": "no-sid", "state": "done"}]"#;
    for a in ["main", "alt"] {
        app.apply_poll(PollUpdate {
            account: a.into(),
            cycle: 0,
            result: Ok(parse_listing(rows).unwrap()),
        });
    }
    select(&mut app, "no-sid");
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(
        status_text(&app).contains("no sessionId"),
        "{}",
        status_text(&app)
    );
    assert_eq!(app.overlay(), &Overlay::None);
}

#[test]
fn two_accounts_target_the_other_one() {
    let mut app = app_two_owned();
    // A stopped row: asks too (every hand-off does), but no stop.
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).expect("y starts it");
    let r = &job.request;
    assert_eq!(r.target.name, "alt");
    assert_eq!(r.owner.as_ref().map(|a| a.name.as_str()), Some("main"));
    assert_eq!(r.id, "b2f8d064");
    assert_eq!(r.session_id, SAY_OK);
    assert_eq!(r.cwd, PathBuf::from("/home/user/workspace/demo-app"));
    assert_eq!(r.prompt, "continue");
    assert!(!r.live);
    assert!(r.stop_account().is_none(), "no live process: no stop");
    assert!(!job.after_trust);
    assert_eq!(
        app.handoff_progress(),
        Some("hand-off: say-ok — not running; continue under alt…")
    );
    assert!(status_text(&app).starts_with("hand-off: say-ok"));
}

#[test]
fn a_live_done_row_asks_without_the_interrupt_note() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    let Overlay::Confirm(Confirm::Handoff {
        detail, interrupts, ..
    }) = app.overlay().clone()
    else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(detail, "done");
    assert!(!interrupts, "a done row has no turn to interrupt");
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("From: main   To: alt"), "{screen}");
    assert!(
        screen.contains("full conversation and project context will continue under alt"),
        "{screen}"
    );
    assert!(!screen.contains("interrupts the current turn"), "{screen}");
    let job = handoff_of(&app.handle_key(ch('y'))).expect("y starts it");
    assert!(job.request.live);
    assert_eq!(
        job.request.stop_account().map(|a| a.name.as_str()),
        Some("main")
    );
}

#[test]
fn configured_handoff_prompt_is_used() {
    let home = Path::new(HOME);
    let config = Config::parse(
        r#"
handoff_prompt = "pick up where you stopped"
[[accounts]]
name = "main"
command = "claude"
[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#,
        home,
    )
    .unwrap();
    let mut app = App::new(config, home.into(), owned_by_main(), Box::new(NoEnviron));
    poll_both(&mut app, "after-stop-all.json");
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).unwrap();
    assert_eq!(job.request.prompt, "pick up where you stopped");
}

#[test]
fn working_or_blocked_rows_confirm_with_the_plan() {
    let mut app = app_two_owned();
    select(&mut app, "touch-config");
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    let Overlay::Confirm(Confirm::Handoff {
        job,
        detail,
        interrupts,
    }) = app.overlay().clone()
    else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(detail, "permission prompt");
    assert!(interrupts);
    assert_eq!(job.request.plan(), "stop under main, continue under alt");
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("interrupts the current turn"), "{screen}");
    assert!(
        screen.contains("full conversation and project context will continue under alt"),
        "{screen}"
    );
    assert!(
        screen.contains("Continue under another account?"),
        "{screen}"
    );
    assert!(
        screen.contains("stop under main, continue under alt"),
        "{screen}"
    );
    assert!(screen.contains("not shown again"), "{screen}");
    assert!(app.handoff_progress().is_none(), "nothing runs yet");

    // n, Esc and Enter cancel.
    for cancel in [ch('n'), key(KeyCode::Esc), key(KeyCode::Enter)] {
        assert!(app.handle_key(cancel).is_empty());
        assert_eq!(app.overlay(), &Overlay::None);
        assert!(status_text(&app).contains("hand-off cancelled"));
        app.handle_key(ch('a'));
    }
    let job = handoff_of(&app.handle_key(ch('y'))).expect("y starts it");
    assert!(job.request.live);
    assert_eq!(
        job.request.stop_account().map(|a| a.name.as_str()),
        Some("main")
    );
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(app.handoff_progress().unwrap().contains("stop under main"));
}

#[test]
fn three_accounts_pick_a_target_other_than_the_owner() {
    let mut app = app_three(owned_by_main());
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    let Overlay::Target(picker) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.targets, ["alt", "spare"]);
    assert_eq!(picker.owner.as_deref(), Some("main"));
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("Continue say-ok under which account?"),
        "{screen}"
    );
    assert!(screen.contains("▶ alt  (claude-alt)"), "{screen}");
    assert!(screen.contains("spare  (claude-spare)"), "{screen}");
    assert!(!screen.contains("main  (claude)"), "{screen}");

    // Esc cancels.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("hand-off cancelled"));

    app.handle_key(ch('a'));
    app.handle_key(ch('j'));
    app.handle_key(ch('j')); // clamps at the last entry
    let job = confirmed(&mut app, key(KeyCode::Enter)).unwrap();
    assert_eq!(job.request.target.name, "spare");
    assert_eq!(app.overlay(), &Overlay::None);
}

#[test]
fn three_accounts_and_a_working_row_pick_then_confirm() {
    let mut app = app_three(owned_by_main());
    select(&mut app, "touch-config");
    app.handle_key(ch('a'));
    assert!(matches!(app.overlay(), Overlay::Target(_)));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let Overlay::Confirm(Confirm::Handoff { job, .. }) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(job.request.target.name, "alt");
    assert!(handoff_of(&app.handle_key(ch('y'))).is_some());
}

#[test]
fn unknown_owner_picks_any_account_and_stops_with_the_target() {
    // Shared lists, no sidecar, unreadable environ: every owner is `?`.
    let mut app = app_two_with(Sidecar::default());
    poll_both(&mut app, "after-stop-all.json");
    select(&mut app, "print-date");
    assert_eq!(
        owner_of(&app, PRINT_DATE),
        Ownership::Unknown { config_dir: None }
    );
    app.handle_key(ch('a'));
    let Overlay::Target(picker) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.targets, ["main", "alt"]);
    assert_eq!(picker.owner, None);
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("Owner unknown"), "{screen}");
    app.handle_key(key(KeyCode::Down));
    let job = confirmed(&mut app, key(KeyCode::Enter)).unwrap();
    assert_eq!(job.request.owner, None);
    assert_eq!(job.request.target.name, "alt");
    assert_eq!(
        job.request.stop_account().map(|a| a.name.as_str()),
        Some("alt"),
        "stop works from any account (doc/0004)"
    );
    // Nothing is recorded until the hand-off succeeded.
    assert_eq!(app.sidecar.owner(PRINT_DATE), None);
}

#[test]
fn picker_closes_when_the_session_vanishes() {
    let mut app = app_three(owned_by_main());
    select(&mut app, "say-ok");
    app.handle_key(ch('a'));
    assert!(matches!(app.overlay(), Overlay::Target(_)));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(poll(a, "after-stop.json"));
    }
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("no longer listed"));
}

// ---- account groups (doc/0004) ----------------------------------------------

/// An app over `config` where every account lists `after-stop-all.json`
/// and owners come from `sidecar` (unreadable environ).
fn app_config(config: &str, sidecar: Sidecar) -> App {
    let home = Path::new(HOME);
    let config = Config::parse(config, home).unwrap();
    let names: Vec<String> = config.accounts.iter().map(|a| a.name.clone()).collect();
    let mut app = App::new(config, home.into(), sidecar, Box::new(NoEnviron));
    for a in &names {
        app.apply_poll(poll(a, "after-stop-all.json"));
    }
    app
}

/// `main` and `alt` in groups `main_group` / `alt_group` (`None`: no
/// `group` key), plus `extra` lines at the top level.
fn two_grouped(main_group: Option<&str>, alt_group: Option<&str>, extra: &str) -> String {
    let group = |g: Option<&str>| g.map(|g| format!("group = \"{g}\"\n")).unwrap_or_default();
    format!(
        "{extra}\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n{}\
         [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n\
         config_dir = \"~/.claude-alt\"\n{}",
        group(main_group),
        group(alt_group)
    )
}

fn assert_starts(text: &str, prefix: &str) {
    assert!(text.starts_with(prefix), "{text}");
}

const CROSS_REFUSAL: &str = "hand-off from main (work) to alt (personal) is disabled: different \
     account groups; set allow_cross_group_handoff = true to allow";

#[test]
fn same_group_is_allowed_with_confirmation() {
    let mut app = app_config(
        &two_grouped(Some("work"), Some("work"), ""),
        owned_by_main(),
    );
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("From: main (work)   To: alt (work)"),
        "{screen}"
    );
    assert!(
        screen.contains("project context will continue under alt (work)"),
        "{screen}"
    );
    assert!(screen.contains("its terms"), "{screen}");
    let job = handoff_of(&app.handle_key(ch('y'))).expect("y starts it");
    assert_eq!(job.request.target.name, "alt");
    assert_eq!(job.request.target.group, "work");
}

#[test]
fn different_groups_are_refused_without_any_effect() {
    let mut app = app_config(
        &two_grouped(Some("work"), Some("personal"), ""),
        owned_by_main(),
    );
    for row in ["say-ok", "print-date", "touch-config"] {
        select(&mut app, row);
        let effects = app.handle_key(ch('a'));
        assert!(effects.is_empty(), "{row}: no stop, no resume: {effects:?}");
        assert_eq!(app.overlay(), &Overlay::None, "{row}: nothing to confirm");
        assert!(
            status_text(&app).starts_with(CROSS_REFUSAL),
            "{row}: {}",
            status_text(&app)
        );
        assert!(app.handoff_progress().is_none());
        // A stray `y` does not start anything either.
        assert!(handoff_of(&app.handle_key(ch('y'))).is_none());
    }
}

#[test]
fn the_flag_allows_a_cross_group_handoff() {
    let mut app = app_config(
        &two_grouped(
            Some("work"),
            Some("personal"),
            "allow_cross_group_handoff = true",
        ),
        owned_by_main(),
    );
    select(&mut app, "print-date");
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("From: main (work)   To: alt (personal)"),
        "{screen}"
    );
    assert!(
        screen.contains("project context will continue under alt"),
        "{screen}"
    );
    assert!(screen.contains("(personal) and its terms"), "{screen}");
    let job = handoff_of(&app.handle_key(ch('y'))).expect("y starts it");
    assert_eq!(job.request.target.name, "alt");
    assert_eq!(
        job.request.stop_account().map(|a| a.name.as_str()),
        Some("main")
    );
}

const THREE_GROUPED: &str = r#"
[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal"

[[accounts]]
name = "spare"
command = "claude-spare"
config_dir = "~/.claude-spare"
group = "work"
"#;

#[test]
fn three_accounts_in_two_groups_offer_only_the_same_group() {
    // main (work): the only same-group target is spare, so no picker and
    // never alt.
    let mut app = app_config(THREE_GROUPED, owned_by_main());
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).unwrap();
    assert_eq!(job.request.target.name, "spare");

    // alt (personal) has no account in its group.
    let mut sc = Sidecar::default();
    sc.record_owner(SAY_OK, "alt", AccountSource::Started);
    let mut app = app_config(THREE_GROUPED, sc);
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert_starts(
        &status_text(&app),
        "hand-off from alt (personal) to main (work) or spare (work) is disabled: different \
         account groups; set allow_cross_group_handoff = true to allow",
    );

    // A fourth work account: the picker lists main's group only.
    let four = format!(
        "{THREE_GROUPED}\n[[accounts]]\nname = \"extra\"\ncommand = \"claude-extra\"\n\
         config_dir = \"~/.claude-extra\"\ngroup = \"work\"\n"
    );
    let mut app = app_config(&four, owned_by_main());
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    let Overlay::Target(picker) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.targets, ["spare", "extra"]);
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("▶ spare  (claude-spare)  group work"),
        "{screen}"
    );
    assert!(!screen.contains("claude-alt"), "{screen}");
    app.handle_key(ch('j'));
    let job = confirmed(&mut app, key(KeyCode::Enter)).unwrap();
    assert_eq!(job.request.target.name, "extra");

    // With the flag every other account is offered.
    let cross = format!("allow_cross_group_handoff = true\n{THREE_GROUPED}");
    let mut app = app_config(&cross, owned_by_main());
    select(&mut app, "say-ok");
    app.handle_key(ch('a'));
    let Overlay::Target(picker) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.targets, ["alt", "spare"]);
}

#[test]
fn unknown_owner_across_groups_is_refused() {
    let mut app = app_config(
        &two_grouped(Some("work"), Some("personal"), ""),
        Sidecar::default(),
    );
    select(&mut app, "print-date");
    assert_eq!(
        owner_of(&app, PRINT_DATE),
        Ownership::Unknown { config_dir: None }
    );
    let effects = app.handle_key(ch('a'));
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(app.overlay(), &Overlay::None, "no picker");
    let status = status_text(&app);
    assert!(status.contains("account is unknown"), "{status}");
    assert!(status.contains("attach first"), "{status}");
    assert!(status.contains("allow_cross_group_handoff"), "{status}");

    // With the flag the picker offers every account, as without groups.
    let mut app = app_config(
        &two_grouped(
            Some("work"),
            Some("personal"),
            "allow_cross_group_handoff = true",
        ),
        Sidecar::default(),
    );
    select(&mut app, "print-date");
    app.handle_key(ch('a'));
    let Overlay::Target(picker) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.targets, ["main", "alt"]);

    // One group for everyone: the unknown owner is no obstacle.
    let mut app = app_config(
        &two_grouped(Some("work"), Some("work"), ""),
        Sidecar::default(),
    );
    select(&mut app, "print-date");
    app.handle_key(ch('a'));
    assert!(matches!(app.overlay(), Overlay::Target(_)));
}

#[test]
fn ungrouped_accounts_behave_as_one_group() {
    // No group anywhere: allowed (also covered by the tests above).
    let mut app = app_config(&two_grouped(None, None, ""), owned_by_main());
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("From: main   To: alt"), "{screen}");
    assert!(handoff_of(&app.handle_key(ch('y'))).is_some());

    // An ungrouped account is not in a named group.
    let mut app = app_config(&two_grouped(None, Some("work"), ""), owned_by_main());
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('a')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert_starts(
        &status_text(&app),
        "hand-off from main (no group) to alt (work) is disabled: different account groups; \
         set allow_cross_group_handoff = true to allow",
    );
}

// ---- running, success, copies -----------------------------------------------

#[test]
fn progress_then_success_records_the_target() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();

    app.handoff_step(Step::Stopping {
        account: "main".into(),
    });
    assert!(status_text(&app).contains("hand-off: stopping print-date under main…"));
    app.handoff_step(Step::WaitingForStop);
    assert!(status_text(&app).contains("waiting for print-date to stop"));
    // The progress outlives the transient message TTL.
    let later = Instant::now() + Duration::from_secs(60);
    assert!(app.status_segments(later)[0].text.contains("waiting for"));
    app.handoff_step(Step::Resuming {
        account: "alt".into(),
    });
    assert!(status_text(&app).contains("continuing print-date under alt…"));

    // A second `a` while it runs is refused.
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(status_text(&app).contains("already running"));

    let effects = app.handoff_finished(job, Ok(continued("7b4d9f02")));
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
    assert!(effects.contains(&Effect::PollNow), "{effects:?}");
    assert_eq!(
        app.sidecar.owner(PRINT_DATE),
        Some(("alt", &AccountSource::Handoff))
    );
    assert_eq!(
        owner_of(&app, PRINT_DATE),
        Ownership::Known {
            account: "alt".into(),
            source: AccountSource::Handoff
        },
        "the row shows the new owner before the next poll"
    );
    assert!(app.handoff_progress().is_none());
    let status = status_text(&app);
    assert!(
        status.starts_with("continued print-date under alt"),
        "{status}"
    );
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Info);

    // The next poll (row stopped, shared lists) keeps alt: rule 3.
    poll_both(&mut app, "after-stop-all.json");
    assert_eq!(owner_of(&app, PRINT_DATE).account(), Some("alt"));
}

#[test]
fn an_idle_wake_is_a_warning() {
    let mut app = app_two_owned();
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let mut idle = continued("b2f8d064");
    idle.idle = true;
    app.handoff_finished(job, Ok(idle));
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Warning);
    assert!(seg.text.contains("woke idle"), "{}", seg.text);
}

#[test]
fn a_copy_warns_and_is_recorded_under_its_own_session_id_on_the_next_poll() {
    // The copy runs under alt (pid 5001); every other pid is unreadable.
    let mut app = app_two_environ(&[(5001, "/home/user/.claude-alt", 900_000)]);
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let copy = Resumed {
        id: "0a1b2c3d".into(),
        name: Some("say-ok".into()),
        outcome: Outcome::Copied {
            new_id: "0a1b2c3d".into(),
        },
        notes: vec!["note: made-up copy note".into()],
        idle: false,
    };
    let effects = app.handoff_finished(job, Ok(copy));
    assert!(effects.contains(&Effect::SaveSidecar));
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Warning);
    assert!(seg.text.contains("copy 0a1b2c3d"), "{}", seg.text);
    assert!(seg.text.contains("made-up copy note"), "{}", seg.text);
    // The original keeps its owner.
    assert_eq!(owner_of(&app, SAY_OK).account(), Some("main"));
    assert_eq!(
        app.sidecar.owner(SAY_OK),
        Some(("main", &AccountSource::Started))
    );

    // Next poll: the copy shows up with its sessionId.
    let copy_sid = "0a1b2c3d-1111-4222-8333-444455556666";
    let mut rows = common::listing("after-stop-all.json");
    rows.sessions.push(
        parse_listing(&format!(
            r#"[{{"pid": 5001, "id": "0a1b2c3d", "cwd": "/home/user/workspace/demo-app",
                "kind": "background", "startedAt": 1767232900000, "sessionId": "{copy_sid}",
                "name": "say-ok", "status": "busy", "state": "working"}}]"#
        ))
        .unwrap()
        .sessions
        .remove(0),
    );
    let mut effects = Vec::new();
    for a in ["main", "alt"] {
        effects.extend(app.apply_poll(PollUpdate {
            account: a.into(),
            cycle: 0,
            result: Ok(rows.clone()),
        }));
    }
    assert!(effects.contains(&Effect::SaveSidecar));
    // Moved from the short id to the copy's sessionId; the live process
    // then confirms alt (rule 1), so the record is a `proc` detection.
    assert_eq!(app.sidecar.owner("0a1b2c3d"), None);
    assert_eq!(app.sidecar.owner(copy_sid).map(|(a, _)| a), Some("alt"));
    assert_eq!(owner_of(&app, copy_sid).account(), Some("alt"));
    assert_eq!(
        app.sidecar.owner(SAY_OK),
        Some(("main", &AccountSource::Started))
    );
}

#[test]
fn failures_report_and_poll_without_touching_the_sidecar() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let before = app.sidecar.clone();
    let effects = app.handoff_finished(
        job.clone(),
        Err(HandoffError::Failed {
            code: Some(1),
            stderr: "No conversation found".into(),
        }),
    );
    assert_eq!(effects, [Effect::PollNow]);
    assert_eq!(app.sidecar, before);
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Error);
    assert!(
        seg.text.contains("cannot continue print-date under alt"),
        "{}",
        seg.text
    );
    assert!(seg.text.contains("No conversation found"), "{}", seg.text);
    assert!(app.handoff_progress().is_none());

    app.handoff_finished(
        job,
        Err(HandoffError::StillRunning {
            waited: Duration::from_secs(10),
            last_error: None,
        }),
    );
    assert!(status_text(&app).contains("hand-off of print-date to alt failed"));
    assert_eq!(owner_of(&app, PRINT_DATE).account(), Some("main"));
}

// ---- quitting while a hand-off runs ---------------------------------------------

fn quit_question(app: &App) -> Option<String> {
    app.quit_prompt().map(|q| q.text())
}

#[test]
fn quitting_during_a_handoff_asks_and_defaults_to_no() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();
    app.handoff_step(Step::WaitingForStop);

    // q asks; Enter, n and Esc keep tower running.
    for cancel in [key(KeyCode::Enter), ch('n'), key(KeyCode::Esc)] {
        assert!(app.handle_key(ch('q')).is_empty(), "q does not quit");
        assert_eq!(
            quit_question(&app).as_deref(),
            Some("a hand-off of print-date is running; quitting may leave it stopped — quit?")
        );
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("Quit tower?"), "{screen}");
        assert!(screen.contains("quitting may leave it stopped"), "{screen}");
        // Other keys, Ctrl+C included, leave the question open.
        assert!(app.handle_key(ch('j')).is_empty());
        assert!(app.handle_key(ctrl('c')).is_empty());
        assert!(app.quit_prompt().is_some());
        assert!(app.handle_key(cancel).is_empty());
        assert_eq!(app.quit_prompt(), None);
        assert!(
            status_text(&app).contains("not quitting"),
            "{}",
            status_text(&app)
        );
    }
    assert!(app.handoff_progress().is_some(), "still running");

    // Ctrl+C asks too, from the list and from other windows; y quits.
    assert!(app.handle_key(ctrl('c')).is_empty());
    assert!(app.quit_prompt().is_some());
    assert_eq!(app.handle_key(ch('y')), [Effect::Quit]);
    assert_eq!(app.quit_prompt(), None);

    app.handle_key(ch('?'));
    assert!(app.handle_key(ch('q')).is_empty());
    assert!(app.quit_prompt().is_some());
    app.handle_key(key(KeyCode::Esc));

    // The new-session dialog: straight to this question.
    app.handle_key(key(KeyCode::Esc)); // close the help
    app.open_dialog(Default::default());
    assert!(app.handle_key(ctrl('c')).is_empty());
    assert!(app.quit_prompt().is_some());
    assert_eq!(app.handle_key(ch('Y')), [Effect::Quit]);

    // Once the hand-off reports back, the question goes away and q quits.
    app.handle_key(ctrl('c'));
    assert!(app.quit_prompt().is_some());
    app.handoff_finished(job, Ok(continued("7b4d9f02")));
    assert_eq!(app.quit_prompt(), None);
}

#[test]
fn without_a_handoff_quitting_does_not_ask() {
    let mut app = app_two_owned();
    assert_eq!(app.handle_key(ch('q')), [Effect::Quit]);
    assert_eq!(app.handle_key(ctrl('c')), [Effect::Quit]);
    assert_eq!(app.quit_prompt(), None);
}

// ---- row actions while a hand-off runs -------------------------------------------

fn attach_account(effects: &[Effect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        Effect::Attach(r) => Some(r.account.name.clone()),
        _ => None,
    })
}

#[test]
fn row_actions_on_the_session_being_handed_off_are_refused() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();
    app.handoff_step(Step::WaitingForStop);

    // Enter would wake it under main, the old owner; s and d would race it.
    for k in [key(KeyCode::Enter), ch('s'), ch('d')] {
        assert!(app.handle_key(k).is_empty(), "{k:?}");
        assert_eq!(app.overlay(), &Overlay::None, "{k:?}");
        assert!(
            status_text(&app).contains("print-date is being handed off"),
            "{k:?}: {}",
            status_text(&app)
        );
    }
    // A second hand-off is refused.
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(status_text(&app).contains("already running (print-date)"));

    // Other rows are not affected.
    select(&mut app, "say-ok");
    let both = [
        ("main", "after-stop-all.json"),
        ("alt", "after-stop-all.json"),
    ];
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        attach_account(&refresh_for_attach(&mut app, effects, &both)).as_deref(),
        Some("main")
    );

    // Afterwards the row attaches with its new owner.
    app.handoff_finished(job, Ok(continued("7b4d9f02")));
    select(&mut app, "print-date");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        attach_account(&refresh_for_attach(&mut app, effects, &both)).as_deref(),
        Some("alt")
    );
}

#[test]
fn an_unknown_owner_row_being_handed_off_opens_no_picker() {
    let mut app = app_two_with(Sidecar::default());
    poll_both(&mut app, "after-stop-all.json");
    select(&mut app, "print-date");
    app.handle_key(ch('a'));
    assert!(confirmed(&mut app, key(KeyCode::Enter)).is_some());
    for k in [key(KeyCode::Enter), ch('s'), ch('d')] {
        assert!(app.handle_key(k).is_empty());
        assert_eq!(app.overlay(), &Overlay::None, "no account picker");
    }
    assert_eq!(app.sidecar.owner(PRINT_DATE), None, "nothing asked");
}

// ---- termination while a hand-off runs -------------------------------------------

#[test]
fn termination_waits_for_the_handoff_and_records_it() {
    use watchtower::jobs::JobResult;
    use watchtower::tui::finish_handoff_for_exit;

    // No hand-off: nothing to wait for.
    let mut app = app_two_owned();
    let mut asked = 0;
    assert!(!finish_handoff_for_exit(
        &mut app,
        Duration::from_secs(15),
        |_| {
            asked += 1;
            None
        }
    ));
    assert_eq!(asked, 0);

    // The step, then the result: applied and due for a save.
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let mut results = vec![
        JobResult::Handoff(Box::new(job.clone()), Ok(continued("7b4d9f02"))),
        JobResult::HandoffStep(Step::Resuming {
            account: "alt".into(),
        }),
    ];
    let mut waits = Vec::new();
    let save = finish_handoff_for_exit(&mut app, Duration::from_secs(15), |left| {
        waits.push(left);
        results.pop()
    });
    assert!(save);
    assert_eq!(waits.len(), 2, "stops once the hand-off reported back");
    assert!(waits.iter().all(|w| *w <= Duration::from_secs(15)));
    assert!(app.handoff_progress().is_none());
    assert_eq!(
        app.sidecar.owner(PRINT_DATE),
        Some(("alt", &AccountSource::Handoff))
    );

    // No result in time: gives up, nothing to save.
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    confirmed(&mut app, ch('a')).unwrap();
    let before = app.sidecar.clone();
    assert!(!finish_handoff_for_exit(
        &mut app,
        Duration::from_secs(15),
        |_| None
    ));
    assert!(app.handoff_progress().is_some());
    assert_eq!(app.sidecar, before);
    // A zero wait does not even ask.
    assert!(!finish_handoff_for_exit(&mut app, Duration::ZERO, |_| {
        panic!("no wait left")
    }));

    // A failed hand-off is applied too, without a save.
    let job = HandoffJob {
        request: job.request,
        after_trust: false,
        recorded: None,
    };
    let mut once = Some(JobResult::Handoff(
        Box::new(job),
        Err(HandoffError::StillRunning {
            waited: Duration::from_secs(10),
            last_error: None,
        }),
    ));
    assert!(!finish_handoff_for_exit(
        &mut app,
        Duration::from_secs(15),
        |_| once.take()
    ));
    assert!(app.handoff_progress().is_none());
}

// ---- trust ----------------------------------------------------------------------

#[test]
fn untrusted_offers_ctrl_t_and_retries_once_without_stopping_again() {
    let mut app = app_two_owned();
    select(&mut app, "print-date");
    let job = confirmed(&mut app, ch('a')).unwrap();
    assert!(job.request.live);
    let untrusted = || HandoffError::Untrusted {
        dir: PathBuf::from("/home/user/workspace/demo-app"),
        message: "Workspace not trusted. Run `claude` in /home/user/workspace/demo-app once \
                  and accept the trust prompt, then retry."
            .into(),
    };
    let effects = app.handoff_finished(job.clone(), Err(untrusted()));
    assert_eq!(effects, [Effect::PollNow]);
    let Overlay::Untrusted(u) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(u.job, job);
    assert!(status_text(&app).contains("Ctrl+T"));
    let screen = render(&mut app, 120, 30);
    assert!(screen.contains("Not trusted by alt"), "{screen}");
    assert!(
        screen.contains("Ctrl+T: run `claude-alt` in ~/workspace/demo-app"),
        "{screen}"
    );

    // Other keys do nothing; Ctrl+T runs the trust flow for the target.
    assert!(app.handle_key(ch('x')).is_empty());
    let effects = app.handle_key(ctrl('t'));
    assert_eq!(effects, [Effect::HandoffTrust(job.clone())]);
    assert_eq!(app.overlay(), &Overlay::None);

    // Back from claude-alt: retry once, without a second stop.
    let effects = app.handoff_trust_finished(job.clone(), Ok(Some(0)));
    let retry = handoff_of(&effects).expect("retries");
    assert!(retry.after_trust);
    assert!(!retry.request.live);
    assert!(retry.request.stop_account().is_none());
    assert_eq!(retry.request.target.name, "alt");

    // Still untrusted: offered again, saying so.
    app.handoff_finished(retry.clone(), Err(untrusted()));
    let Overlay::Untrusted(u) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert!(u.message.contains("still not trusted"), "{}", u.message);
    // Esc leaves it stopped.
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("stays stopped"));

    // Success on a retry records as usual.
    app.handoff_finished(retry, Ok(continued("7b4d9f02")));
    assert_eq!(
        app.sidecar.owner(PRINT_DATE),
        Some(("alt", &AccountSource::Handoff))
    );
}

#[test]
fn a_trust_flow_that_cannot_run_shows_why() {
    let mut app = app_two_owned();
    select(&mut app, "say-ok");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let effects = app.handoff_trust_finished(job, Err("cannot run `claude-alt`: not found".into()));
    assert!(effects.is_empty());
    let Overlay::Untrusted(u) = app.overlay().clone() else {
        panic!("{:?}", app.overlay());
    };
    assert!(u.message.contains("not found"));
}

// ---- ownership agrees after the wake ---------------------------------------

/// `pid` → `CLAUDE_CONFIG_DIR` and raw start ticks of a Claude Code
/// process; other pids are unreadable.
struct Environ(HashMap<u32, (OsString, u64)>);

fn unreadable() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::NotFound)
}

impl EnvironReader for Environ {
    fn config_dir_var(&self, pid: u32) -> std::io::Result<Option<OsString>> {
        self.0
            .get(&pid)
            .map(|(dir, _)| Some(dir.clone()))
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    fn cmdline(&self, pid: u32) -> std::io::Result<Vec<u8>> {
        self.0
            .get(&pid)
            .map(|_| b"claude\0--bg\0".to_vec())
            .ok_or_else(unreadable)
    }

    fn exe(&self, pid: u32) -> std::io::Result<std::path::PathBuf> {
        self.0
            .get(&pid)
            .map(|_| "/home/user/.local/share/claude/versions/1.0.0".into())
            .ok_or_else(unreadable)
    }

    fn start_ticks(&self, pid: u32) -> std::io::Result<u64> {
        self.0
            .get(&pid)
            .map(|(_, ticks)| *ticks)
            .ok_or_else(unreadable)
    }
}

#[test]
fn proc_detection_after_the_wake_agrees_with_the_handoff() {
    // The woken process (pid 5000) is Claude Code under alt's config dir.
    // The old process (4290) is gone: unreadable.
    let mut app = app_two_environ(&[(5000, "/home/user/.claude-alt", 815_195)]);
    select(&mut app, "print-date");
    assert_eq!(owner_of(&app, PRINT_DATE).account(), Some("main"));
    let job = confirmed(&mut app, ch('a')).unwrap();
    app.handoff_finished(job, Ok(continued("7b4d9f02")));

    // A late poll from before the stop (old pid 4290) does not move it back.
    app.apply_poll(poll("main", "after-stop-all.json"));
    assert_eq!(owner_of(&app, PRINT_DATE).account(), Some("alt"));

    // After the wake: live under pid 5000, /proc says alt.
    let woke = common::fixture("agents-json/after-stop-all.json")
        .replace("\"pid\": 4290,", "\"pid\": 5000,");
    let mut effects = Vec::new();
    for a in ["main", "alt"] {
        effects.extend(app.apply_poll(PollUpdate {
            account: a.into(),
            cycle: 0,
            result: Ok(parse_listing(&woke).unwrap()),
        }));
    }
    assert_eq!(
        owner_of(&app, PRINT_DATE),
        Ownership::Known {
            account: "alt".into(),
            source: AccountSource::Proc
        }
    );
    // The sidecar now holds the detection, still alt.
    assert_eq!(
        app.sidecar.owner(PRINT_DATE),
        Some(("alt", &AccountSource::Proc))
    );
}

// ---- help and footer ------------------------------------------------------------

#[test]
fn help_and_footer_offer_the_handoff() {
    let mut app = app_two_owned();
    app.handle_key(ch('?'));
    let screen = render(&mut app, 100, 24);
    assert!(
        screen.contains("continue under another account"),
        "{screen}"
    );
    assert!(!screen.contains("Not available yet"), "{screen}");
    assert!(screen.contains("Ctrl+Z"), "{screen}");
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 160, 20);
    let footer = screen.lines().last().unwrap();
    assert!(footer.contains("a hand-off"), "{footer}");
}

// ---- an empty hand-off prompt (doc/0033) ------------------------------------

const NO_PROMPT_CONFIG: &str = r#"
handoff_prompt = ""
[[accounts]]
name = "main"
command = "claude"
[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#;

#[test]
fn the_confirmation_names_the_prompt() {
    let mut app = app_two_owned();
    select(&mut app, "touch-config");
    app.handle_key(ch('a'));
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("Prompt: \"continue\", sent as the next turn under alt."),
        "{screen}"
    );
    assert!(
        screen.contains("the session then gets the hand-off"),
        "{screen}"
    );
}

#[test]
fn an_empty_prompt_says_the_session_will_wait_for_you() {
    let mut app = app_config(NO_PROMPT_CONFIG, owned_by_main());
    select(&mut app, "touch-config");
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    let screen = render(&mut app, 120, 30);
    assert!(
        screen.contains("Prompt: none. The session will wait for you under alt; no turn is"),
        "{screen}"
    );
    assert!(screen.contains("started."), "{screen}");
    // It was working (a permission prompt): the interrupt note says that it
    // will not pick up by itself.
    assert!(
        screen.contains("Stopping interrupts the current turn; without a prompt the session"),
        "{screen}"
    );
    assert!(screen.contains("working · idle"), "{screen}");
    assert!(
        !screen.contains("the session then gets the hand-off"),
        "{screen}"
    );
    let job = handoff_of(&app.handle_key(ch('y'))).expect("y starts it");
    assert_eq!(job.request.prompt_arg(), None);
}

/// One background row of `after-stop-all.json`'s project, live.
fn live_row(id: &str, sid: &str, name: &str, state: &str, status: &str) -> String {
    format!(
        r#"{{"pid": 5100, "id": "{id}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767233000000, "sessionId": "{sid}",
            "name": "{name}", "status": "{status}", "state": "{state}"}}"#
    )
}

#[test]
fn an_empty_prompt_hand_off_reports_waiting_and_raises_no_highlight() {
    let mut app = app_config(NO_PROMPT_CONFIG, owned_by_main());
    select(&mut app, "touch-config");
    let job = confirmed(&mut app, ch('a')).unwrap();
    let mut woke = continued("d4a1f7c8");
    woke.name = Some("touch-config".into());
    woke.idle = true;
    let effects = app.handoff_finished(job, Ok(woke));
    assert!(effects.contains(&Effect::PollNow), "{effects:?}");
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Info, "no prompt was the choice");
    assert_eq!(
        seg.text,
        "continued touch-config under alt · waiting for your prompt"
    );
    assert_eq!(
        app.sidecar.owner(TOUCH),
        Some(("alt", &AccountSource::Handoff))
    );

    // The next poll shows it as Claude Code reports it: stopped mid-turn, it
    // wakes `working` / `idle`; a `done` session woken the same way keeps
    // `done` / `idle`. Neither is highlighted (doc/0007).
    let rows = format!(
        "[{}, {}]",
        live_row("d4a1f7c8", TOUCH, "touch-config", "working", "idle"),
        live_row("7b4d9f02", PRINT_DATE, "print-date", "done", "idle"),
    );
    for a in ["main", "alt"] {
        app.apply_poll(PollUpdate {
            account: a.into(),
            cycle: 1,
            result: Ok(parse_listing(&rows).unwrap()),
        });
    }
    for (sid, group, detail) in [
        (TOUCH, watchtower::groups::Group::Working, "idle"),
        (
            PRINT_DATE,
            watchtower::groups::Group::ReadyForReview,
            "done",
        ),
    ] {
        let m = app
            .sessions()
            .iter()
            .find(|m| m.session.session_id.as_deref() == Some(sid))
            .unwrap();
        assert_eq!(watchtower::groups::classify(&m.session), group, "{sid}");
        assert_eq!(watchtower::groups::detail(&m.session), detail, "{sid}");
        assert!(!watchtower::groups::spins(&m.session), "{sid}");
        assert!(!app.is_highlighted(m), "{sid}");
    }
    assert_eq!(app.attention_count(), 0);
}

#[test]
fn a_prompted_hand_off_that_woke_idle_still_warns() {
    let mut app = app_two_owned();
    select(&mut app, "touch-config");
    let job = confirmed(&mut app, ch('a')).unwrap();
    assert_eq!(job.request.prompt_arg(), Some("continue"));
    let mut idle = continued("d4a1f7c8");
    idle.idle = true;
    app.handoff_finished(job, Ok(idle));
    let seg = &app.status_segments(Instant::now())[0];
    assert_eq!(seg.severity, Severity::Warning);
    assert!(
        !seg.text.contains("waiting for your prompt"),
        "{}",
        seg.text
    );
}
