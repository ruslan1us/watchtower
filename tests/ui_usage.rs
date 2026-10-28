//! Drawing plan usage (doc/0043): the header after each account's chip,
//! fitted to the width; the details panel of the selected row's owner;
//! the minimal title; nothing while `[usage] show` is off; and never in
//! the hand-off windows or status texts. Local time is UTC here.

mod common;

use std::path::Path;
use std::time::Instant;

use common::{HOME, NoEnviron, ch, key, listing, status_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Modifier;
use watchtower::app::{App, Overlay};
use watchtower::config::{Config, UiStyle, UiTheme};
use watchtower::handoff::Step;
use watchtower::jobs::JobResult;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::usage::{Clock, Limit, LimitKind, PlanUsage, Probed};

/// 2026-10-28T12:00:00Z, a Wednesday.
const NOW_MS: i64 = 1_793_188_800_000;

fn config(show: bool, cross: bool) -> Config {
    Config::parse(
        &format!(
            r#"
allow_cross_group_handoff = {cross}

[usage]
show = {show}

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "work"

[[accounts]]
name = "spare"
command = "claude-spare"
config_dir = "~/.claude-spare"
group = "personal"
"#
        ),
        Path::new(HOME),
    )
    .unwrap()
}

fn limit(kind: &str, percent: f64, hours: i64, severity: &str, model: Option<&str>) -> Limit {
    Limit {
        kind: LimitKind::from(kind),
        group: None,
        percent: Some(percent),
        resets_at_ms: Some(NOW_MS + hours * 3_600_000),
        model: model.map(str::to_owned),
        surface: None,
        severity: Some(severity.to_owned()),
        active: None,
    }
}

fn plan(session: f64, weekly: f64, severity: &str) -> Probed {
    Probed {
        usage: Some(PlanUsage {
            limits: vec![
                limit("session", session, 6, "normal", None),
                limit("weekly_all", weekly, 6 * 24 - 4, severity, None),
                limit("weekly_scoped", 3.0, 6 * 24 - 4, "normal", Some("Fable")),
            ],
            extra: None,
        }),
        ..Probed::default()
    }
}

/// Every fixture row owned by `main`; `main` fresh, `alt` 20 minutes old,
/// `spare` without plan data.
fn app_with(show: bool, cross: bool) -> App {
    let rows = listing("mixed-states.json");
    let mut sc = Sidecar::default();
    for s in &rows.sessions {
        if let Some(id) = &s.session_id {
            sc.record_owner(id, "main", AccountSource::Started);
        }
    }
    let mut app = App::new(config(show, cross), HOME.into(), sc, Box::new(NoEnviron));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(PollUpdate {
            account: a.into(),
            cycle: 1,
            result: Ok(rows.clone()),
        });
    }
    app.clock = Clock::UTC;
    let now = Instant::now();
    app.usage_received("main", Ok(plan(24.0, 12.0, "normal")), now, NOW_MS - 30_000);
    app.usage_received(
        "alt",
        Ok(plan(5.0, 97.0, "critical")),
        now,
        NOW_MS - 1_200_000,
    );
    app.usage_received("spare", Ok(Probed::default()), now, NOW_MS);
    select(&mut app, "print-date");
    app
}

fn select(app: &mut App, name: &str) {
    app.handle_key(key(KeyCode::Home));
    for _ in 0..30 {
        if app
            .selected_session()
            .is_some_and(|m| m.session.display_name() == name)
        {
            return;
        }
        app.handle_key(key(KeyCode::Down));
    }
    panic!("no row {name}");
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    t.backend().buffer().clone()
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

fn looks() -> [(UiStyle, UiTheme); 2] {
    [
        (UiStyle::Dashboard, UiTheme::Neutral),
        (UiStyle::Dashboard, UiTheme::NeutralLight),
    ]
}

fn styled(app: &mut App, style: UiStyle, theme: UiTheme) {
    app.style = style;
    app.set_theme(theme);
}

/// The cell where `needle` starts on row `y`.
fn at(buf: &Buffer, y: u16, needle: &str) -> (u16, u16) {
    let row = &rows(buf)[usize::from(y)];
    let chars: Vec<char> = row.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    let x = (0..chars.len())
        .find(|&x| chars[x..].starts_with(&n))
        .unwrap_or_else(|| panic!("no {needle:?} in {row}"));
    (x as u16, y)
}

#[test]
fn the_header_shows_each_accounts_numbers_after_its_chip() {
    for (style, theme) in looks() {
        let mut app = app_with(true, false);
        styled(&mut app, style, theme);
        let buf = render(&mut app, 160, 45);
        let header = &rows(&buf)[1];
        assert!(header.contains(" main  5h 24% · 7d 12%   "), "{header}");
        assert!(
            header.contains(" alt  5h 5% · 7d 97% · 20m ago   "),
            "stale with its age: {header}"
        );
        assert!(header.contains(" spare  —  "), "{header}");
        // Severity colours: the critical week differs from a normal number,
        // and a stale value is drawn faint.
        let normal = buf[at(&buf, 1, "24%")].fg;
        let critical = buf[at(&buf, 1, "97%")].fg;
        assert_ne!(normal, critical);
        let mut fresh = app_with(true, false);
        styled(&mut fresh, style, theme);
        fresh.usage_received(
            "alt",
            Ok(plan(5.0, 97.0, "critical")),
            Instant::now(),
            NOW_MS,
        );
        let buf2 = render(&mut fresh, 160, 45);
        assert_ne!(
            buf2[at(&buf2, 1, "97%")].fg,
            critical,
            "fresh and stale differ"
        );
    }
}

#[test]
fn the_header_drops_the_week_then_the_numbers_on_narrow_screens() {
    let mut app = app_with(true, false);
    styled(&mut app, UiStyle::Dashboard, UiTheme::Neutral);
    let header = |app: &mut App, w, h| rows(&render(app, w, h))[1].clone();
    let wide = header(&mut app, 250, 60);
    assert!(wide.contains("main  5h 24% · 7d 12%  work · "), "{wide}");
    let mid = header(&mut app, 120, 36);
    assert!(
        mid.contains("main  5h 24%   alt  5h 5%   spare  —"),
        "{mid}"
    );
    assert!(!mid.contains("7d"), "{mid}");
    let narrow = header(&mut app, 100, 30);
    assert!(!narrow.contains("5h"), "{narrow}");
    assert!(narrow.contains(" main "), "{narrow}");
    for h in [&wide, &mid, &narrow] {
        assert!(!h.contains("5h 2…") && !h.contains("· …"), "{h}");
    }
}

#[test]
fn the_details_panel_shows_the_owners_limits_and_resets() {
    for (style, theme) in looks() {
        let mut app = app_with(true, false);
        styled(&mut app, style, theme);
        let s = rows(&render(&mut app, 160, 45));
        let panel: Vec<String> = s
            .iter()
            .map(|r| r.chars().skip(112).collect::<String>())
            .collect();
        let text = panel.join("\n");
        for line in [
            "│ account    main  work · claude",
            "│ usage     5h 24% · resets 18:00",
            "│           7d 12% · resets Tue 08:00",
            "│           Fable 7d 3% · resets Tue 08:00",
            "│ id        7b4d9f02",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        assert!(!text.contains("as of"), "fresh: no age\n{text}");
    }
}

#[test]
fn a_stale_owner_shows_its_age_and_no_plan_says_so() {
    let mut app = app_with(true, false);
    styled(&mut app, UiStyle::Dashboard, UiTheme::Neutral);
    app.usage_received(
        "main",
        Ok(plan(24.0, 12.0, "normal")),
        Instant::now(),
        NOW_MS - 1_200_000,
    );
    let text = rows(&render(&mut app, 160, 45)).join("\n");
    assert!(text.contains("│           as of 20m ago"), "{text}");
    let buf = render(&mut app, 160, 45);
    let s = rows(&buf);
    let y = s
        .iter()
        .position(|r| r.contains("usage     5h 24%"))
        .unwrap();
    let (x, _) = at(&buf, y as u16, "5h 24%");
    let mut fresh = app_with(true, false);
    styled(&mut fresh, UiStyle::Dashboard, UiTheme::Neutral);
    let fbuf = render(&mut fresh, 160, 45);
    assert_ne!(buf[(x, y as u16)].fg, fbuf[(x, y as u16)].fg, "dimmed");

    app.usage_received("main", Ok(Probed::default()), Instant::now(), NOW_MS);
    let text = rows(&render(&mut app, 160, 45)).join("\n");
    assert!(
        text.contains("│ usage     — no plan data (logged out, API"),
        "{text}"
    );
}

#[test]
fn without_colour_a_stale_value_is_dim() {
    let mut app = app_with(true, false);
    styled(&mut app, UiStyle::Dashboard, UiTheme::Neutral);
    app.color = false;
    let buf = render(&mut app, 160, 45);
    let cell = &buf[at(&buf, 1, "5h 5%")];
    assert!(cell.modifier.contains(Modifier::DIM));
    let cell = &buf[at(&buf, 1, "5h 24%")];
    assert!(!cell.modifier.contains(Modifier::DIM));
}

#[test]
fn the_minimal_title_has_a_compact_line_when_it_fits() {
    let mut app = app_with(true, false);
    styled(&mut app, UiStyle::Minimal, UiTheme::Neutral);
    let title = rows(&render(&mut app, 120, 36))[0].clone();
    assert!(
        title.contains("accounts: main, alt, spare  ·  usage: main 24%/12%, alt 5%/97%, spare —"),
        "{title}"
    );
    let title = rows(&render(&mut app, 80, 24))[0].clone();
    assert!(!title.contains("usage"), "{title}");
    assert!(title.contains("accounts: main, alt, spare"), "{title}");
}

#[test]
fn nothing_is_drawn_while_usage_is_off() {
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = app_with(false, false);
        styled(&mut app, style, UiTheme::Neutral);
        let text = rows(&render(&mut app, 250, 60)).join("\n");
        for needle in ["5h", "7d", "usage", "24%", "—"] {
            assert!(!text.contains(needle), "{needle:?} with usage off:\n{text}");
        }
    }
}

/// The rows and columns inside the one window over a minimal screen.
fn window_text(buf: &Buffer) -> String {
    let s = rows(buf);
    let top = s
        .iter()
        .position(|r| r.contains('┌'))
        .expect("a window is open");
    let bottom = (top + 1..s.len()).find(|&y| s[y].contains('└')).unwrap();
    let left = s[top].chars().position(|c| c == '┌').unwrap();
    let right = s[top].chars().position(|c| c == '┐').unwrap();
    s[top..=bottom]
        .iter()
        .map(|r| {
            r.chars()
                .skip(left)
                .take(right - left + 1)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn no_usage(text: &str) {
    for needle in ["%", "5h", "7d", "usage", "limit", "reset"] {
        assert!(!text.contains(needle), "{needle:?} in\n{text}");
    }
}

#[test]
fn the_hand_off_windows_and_status_texts_never_mention_usage() {
    // Two targets: the picker, then the confirmation.
    let mut app = app_with(true, true);
    styled(&mut app, UiStyle::Minimal, UiTheme::Neutral);
    let title = rows(&render(&mut app, 160, 45))[0].clone();
    assert!(title.contains("usage: main 24%/12%"), "the data is there");
    app.handle_key(ch('a'));
    assert!(
        matches!(app.overlay(), Overlay::Target(_)),
        "{:?}",
        app.overlay()
    );
    let picker = window_text(&render(&mut app, 160, 45));
    assert!(
        picker.contains("alt") && picker.contains("spare"),
        "{picker}"
    );
    no_usage(&picker);
    if let Overlay::Target(t) = app.overlay() {
        no_usage(&format!("{t:?}"));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(app.overlay(), Overlay::Confirm(_)),
        "{:?}",
        app.overlay()
    );
    let confirm = window_text(&render(&mut app, 160, 45));
    no_usage(&confirm);
    no_usage(&format!("{:?}", app.overlay()));
    // The steps and the status line while it runs.
    let effects = app.handle_key(ch('y'));
    assert!(!effects.is_empty());
    for step in [
        Step::Stopping {
            account: "main".into(),
        },
        Step::WaitingForStop,
        Step::Resuming {
            account: "alt".into(),
        },
    ] {
        JobResult::HandoffStep(step).apply(&mut app);
        no_usage(&status_text(&app));
        let s = rows(&render(&mut app, 160, 45));
        no_usage(&s[s.len() - 2]);
    }
}
