//! Text fitted to its room: window titles shorten their name with `…`
//! and keep their fixed words, and the minimal list fits its columns to
//! the terminal's width, keeping the age and account columns
//! (doc/0011, doc/0020, doc/0028).

mod common;

use std::path::Path;
use std::time::Instant;

use common::{NoEnviron, ch, key, select_name};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Overlay};
use watchtower::config::{Config, UiStyle};
use watchtower::listing::parse_listing;
use watchtower::logview;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};

const NOW_MS: i64 = 1_800_000_000_000;
const HOME: &str = "/home/user";
const LONG_NAME: &str = "refactor-the-authentication-middleware-and-session-store";
const CYR_NAME: &str = "исправить-падающие-тесты-в-модуле-платежей";
const LONG_PROJECT: &str = "billing-platform-frontend-with-a-very-long-folder-name";
const CYR_PROJECT: &str = "проект-оплаты-и-счетов";
const DEEP: &str = "data/pipelines/reports/quarterly/2026/eu-west";

const CONFIG: &str = r#"
default_account = "main"
allow_cross_group_handoff = true

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
"#;

fn row(id: &str, name: &str, cwd: &str, state: &str, process: &str, ago_s: i64) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
            "cwd": "{HOME}/workspace/{cwd}", "kind": "background", "name": "{name}",
            "state": "{state}", "startedAt": {} }}"#,
        NOW_MS - ago_s * 1000
    )
}

fn listing() -> String {
    let rows = [
        row(
            "a1a1a1a1",
            LONG_NAME,
            LONG_PROJECT,
            "working",
            r#""pid": 4101, "status": "busy","#,
            300,
        ),
        row(
            "b2b2b2b2",
            CYR_NAME,
            CYR_PROJECT,
            "blocked",
            r#""pid": 4102, "status": "waiting", "waitingFor": "permission prompt","#,
            42,
        ),
        row(
            "c3c3c3c3",
            "print-date",
            "api",
            "done",
            r#""pid": 4103, "status": "idle","#,
            720,
        ),
        row("d4d4d4d4", "say-ok", "api", "done", "", 7200),
        row(
            "e5e5e5e5",
            "nightly-report",
            DEEP,
            "done",
            r#""pid": 4105, "status": "busy","#,
            1800,
        ),
        row(
            "f6f6f6f6",
            "pick-colour",
            "web",
            "blocked",
            r#""pid": 4106, "status": "waiting", "waitingFor": "input needed","#,
            90,
        ),
        format!(
            r#"{{ "pid": 4107, "cwd": "{HOME}/workspace/demo-app", "kind": "interactive",
                "startedAt": {}, "sessionId": "a7a7a7a7-0000-4000-8000-000000000000",
                "name": "demo-app-3f", "status": "idle" }}"#,
            NOW_MS - 60_000
        ),
    ];
    format!("[{}]", rows.join(",\n"))
}

/// Every row owned by `main`, listed by every account; the minimal style.
fn app() -> App {
    app_owned(true)
}

/// [`app`], with the owners unknown unless `owners`.
fn app_owned(owners: bool) -> App {
    let config = Config::parse(CONFIG, Path::new(HOME)).unwrap();
    let text = listing();
    let mut sc = Sidecar::default();
    if owners {
        for s in &parse_listing(&text).unwrap().sessions {
            if let Some(id) = &s.session_id {
                sc.record_owner(id, "main", AccountSource::Started);
            }
        }
    }
    let mut app = App::new(config, HOME.into(), sc, Box::new(NoEnviron));
    for account in ["main", "alt", "spare"] {
        app.apply_poll(PollUpdate {
            account: account.into(),
            cycle: 0,
            result: Ok(parse_listing(&text).unwrap()),
        });
    }
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
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
        .collect()
}

/// The title and list rows of the minimal screen (the status line and
/// footer left out), without blank rows.
fn list_rows(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let lines = render(app, w, h);
    lines[..lines.len() - 2]
        .iter()
        .filter(|l| !l.is_empty())
        .cloned()
        .collect()
}

/// The line holding `needle`.
fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

// ---- the minimal list -----------------------------------------------------

/// The minimal list where every column fits, as it was drawn before the
/// columns were fitted to the width (doc/0020): unchanged.
const WIDE: &[&str] = &[
    " tower  7 sessions  ·  accounts: main, alt, spare",
    "  ▾ Needs me (2)  ● 2",
    "▶ ● исправить-падающие-тесты-в-моду…  ~/workspace/проект-оплаты-и-счетов        permission prompt       42s  main",
    "  ● pick-colour                       ~/workspace/web                           input needed             1m  main",
    "  ▾ Ready for review (2)",
    "    print-date                        ~/workspace/api                           done                    12m  main",
    "    nightly-report                    ~/workspace/data/pipelines/reports/quar…  done · bg task          30m  main",
    "  ▾ Working (1)",
    "    refactor-the-authentication-mid…  ~/workspace/billing-platform-frontend-w…  working                  5m  main",
    "  ▸ Stopped & finished (1)",
    "  ▾ Interactive (1)",
    "    demo-app-3f                       ~/workspace/demo-app                      idle · not attachable    1m  main",
];

#[test]
fn the_minimal_list_is_unchanged_where_every_column_fits() {
    let mut app = app();
    for (w, h) in [(160, 45), (250, 60)] {
        assert_eq!(list_rows(&mut app, w, h), WIDE, "{w}x{h}");
    }
}

/// Every session row of `rows` fits `w` columns and ends with its age
/// and account.
fn assert_rows_keep_age_and_account(rows: &[String], w: usize) {
    let sessions: Vec<&String> = rows.iter().filter(|r| r.ends_with("  main")).collect();
    assert_eq!(
        sessions.len(),
        6,
        "{w}: every session row keeps its account\n{rows:#?}"
    );
    for r in sessions {
        assert!(r.chars().count() <= w, "{w}: {r}");
        let age = r.trim_end_matches("  main").rsplit(' ').next().unwrap();
        assert!(["42s", "1m", "12m", "30m", "5m"].contains(&age), "{w}: {r}");
    }
}

#[test]
fn at_104_columns_the_folders_and_states_get_short() {
    let mut app = app();
    let rows = list_rows(&mut app, 104, 32);
    assert_rows_keep_age_and_account(&rows, 104);
    assert_eq!(
        rows[2],
        "▶ ● исправить-падающие-тесты-в-моду…  …/проект-оплаты-и-счетов                  permission   42s  main"
    );
    assert_eq!(
        rows[6],
        "    nightly-report                    …/eu-west                                 done · bg    30m  main"
    );
    assert_eq!(
        rows[8],
        "    refactor-the-authentication-mid…  …/billing-platform-frontend-with-a-very…  working       5m  main"
    );
}

#[test]
fn at_90_columns_the_project_is_cut_too() {
    let mut app = app();
    let rows = list_rows(&mut app, 90, 28);
    assert_rows_keep_age_and_account(&rows, 90);
    assert_eq!(
        rows[8],
        "    refactor-the-authentication-mid…  …/billing-platform-frontend…  working       5m  main"
    );
    assert_eq!(
        rows[11],
        "    demo-app-3f                       …/demo-app                    idle          1m  main"
    );
}

#[test]
fn at_80_columns_every_row_keeps_its_age_and_account() {
    let mut app = app();
    let rows = list_rows(&mut app, 80, 24);
    assert_rows_keep_age_and_account(&rows, 80);
    assert_eq!(
        rows[2],
        "▶ ● исправить-падающие-тесты-в-моду…  …/проект-оплаты-и…  permission   42s  main"
    );
    assert_eq!(
        rows[3],
        "  ● pick-colour                       …/web               input         1m  main"
    );
}

#[test]
fn a_very_narrow_minimal_list_cuts_the_name_too() {
    let mut app = app();
    let rows = list_rows(&mut app, 60, 20);
    assert_rows_keep_age_and_account(&rows, 60);
    let row = line_with(&rows, "5m  main");
    assert!(row.contains("refactor-"), "{row}");
    assert!(!row.contains("refactor-the-authentication-mid…"), "{row}");
}

// ---- window titles --------------------------------------------------------

/// Whether `text` and a space end the title on `line`, at the border's
/// corner, as a fitted title does.
fn ends_at_corner(line: &str, text: &str) -> bool {
    ["╮", "┐", "┓"]
        .iter()
        .any(|corner| line.contains(&format!("{text} {corner}")))
}

#[test]
fn the_target_pickers_title_keeps_its_question() {
    let mut app = app();
    select_name(&mut app, LONG_NAME);
    app.handle_key(ch('a'));
    assert!(
        matches!(app.overlay(), Overlay::Target(_)),
        "{:?}",
        app.overlay()
    );
    for (w, h) in [(80, 24), (90, 28)] {
        let lines = render(&mut app, w, h);
        let title = line_with(&lines, "Continue refactor-");
        assert!(
            ends_at_corner(title, "… under which account?"),
            "{w}: {title}"
        );
    }
    // Wide enough: the window grows to the whole title.
    let lines = render(&mut app, 160, 45);
    let title = line_with(&lines, "Continue refactor-");
    assert!(
        title.contains(&format!(" Continue {LONG_NAME} under which account? ")),
        "{title}"
    );
}

#[test]
fn the_account_pickers_title_keeps_its_question() {
    let mut app = app_owned(false);
    select_name(&mut app, LONG_NAME);
    app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(app.overlay(), Overlay::Picker(_)),
        "{:?}",
        app.overlay()
    );
    let lines = render(&mut app, 80, 24);
    let title = line_with(&lines, "Which account owns refactor-");
    assert!(ends_at_corner(title, "…?"), "{title}");
    let lines = render(&mut app, 160, 45);
    let title = line_with(&lines, "Which account owns refactor-");
    assert!(
        title.contains(&format!(" Which account owns {LONG_NAME}? ")),
        "{title}"
    );
}

/// The logs panel open on the long name, with 300 lines in it.
fn logs_open() -> App {
    let mut app = app();
    select_name(&mut app, LONG_NAME);
    app.handle_key(ch(' '));
    let target = app.logs().unwrap().in_flight().cloned().unwrap();
    let lines: Vec<String> = (1..=300).map(|i| format!("line {i}")).collect();
    let screen = logview::render(format!("\x1b[2J\x1b[H{}", lines.join("\r\n")).as_bytes());
    app.logs_fetched(target, Ok(screen));
    app
}

#[test]
fn the_logs_title_shortens_the_name_and_keeps_the_account() {
    let mut app = logs_open();
    app.style = UiStyle::Dashboard;
    // Half of 140 columns: the name is cut, the account stays.
    let lines = render(&mut app, 140, 40);
    let title = line_with(&lines, "logs · refactor-");
    assert!(ends_at_corner(title, "… (main)"), "{title}");
    // Paused: the position and the account stay next to the cut name.
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Up));
    let lines = render(&mut app, 160, 45);
    let title = line_with(&lines, "logs · refactor-");
    assert!(title.contains("… (main) · paused · line "), "{title}");
    assert!(ends_at_corner(title, " · End follows"), "{title}");
    // Too narrow for a useful name: it goes, the position stays.
    app.style = UiStyle::Minimal;
    let lines = render(&mut app, 70, 24);
    let title = line_with(&lines, "logs · paused");
    assert!(!title.contains("(main)"), "{title}");
}
