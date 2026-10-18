//! Rendering with ratatui's `TestBackend` (doc/0015). Assertions are on key
//! lines and text rather than whole-screen dumps.

mod common;

use std::time::Instant;

use common::{app_single, app_two, poll, poll_err};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::App;

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
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
        .collect()
}

fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

fn index_of(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?}"))
}

#[test]
fn list_renders_groups_rows_and_columns() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    let lines = render(&mut app, 120, 24);

    assert!(lines[0].contains("tower"));
    assert!(lines[0].contains("7 sessions"));
    // Groups in doc/0006 order with counts.
    let order = [
        "Needs me (2)",
        "Failed (1)",
        "Ready for review (2)",
        "Stopped & finished (1)",
        "Interactive (1)",
    ];
    let pos: Vec<usize> = order.iter().map(|h| index_of(&lines, h)).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");
    assert!(
        !lines.iter().any(|l| l.contains("Working (")),
        "empty group hidden"
    );

    // Row: name, project with ~, detail, account.
    let row = line_with(&lines, "touch-config");
    assert!(row.contains("~/workspace/demo-app"), "{row}");
    assert!(row.contains("permission prompt"), "{row}");
    assert!(row.trim_end().ends_with("main"), "{row}");
    assert!(row.starts_with("▶"), "first row selected: {row}");
    assert!(line_with(&lines, "pick-colour").contains("input needed"));
    assert!(line_with(&lines, "bad-model").contains("failed"));
    // Stopped & finished starts collapsed: its header, no rows.
    assert!(line_with(&lines, "▸ Stopped & finished (1)").contains('▸'));
    assert!(line_with(&lines, "▾ Needs me (2)").contains("● 2"));
    assert!(!lines.iter().any(|l| l.contains("say-ok")));
    assert!(line_with(&lines, "demo-app-3f").contains("not attachable"));
    assert!(!lines.iter().any(|l| l.contains("/home/user")));

    // Status and footer.
    assert!(lines[22].contains("refreshed"), "{}", lines[22]);
    assert!(
        lines[23].contains("Ctrl+Z returns to tower"),
        "{}",
        lines[23]
    );
    assert!(lines[23].contains("Enter attach"));
}

#[test]
fn unknown_account_shows_question_mark() {
    let mut app = app_two();
    app.apply_poll(poll("main", "permission-prompt.json"));
    app.apply_poll(poll("alt", "permission-prompt.json"));
    let lines = render(&mut app, 120, 24);
    let row = line_with(&lines, "touch-config");
    assert!(row.trim_end().ends_with('?'), "{row}");
    assert!(lines[0].contains("accounts: main, alt"));
}

#[test]
fn selection_moves_highlight() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = render(&mut app, 100, 20);
    assert!(line_with(&lines, "Failed (1)").starts_with("▶"), "a header");
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = render(&mut app, 100, 20);
    assert!(line_with(&lines, "bad-model").starts_with("▶"));
    assert!(!line_with(&lines, "pick-colour").starts_with("▶"));
    assert!(line_with(&lines, "run-test-suite").contains("working"));
}

#[test]
fn empty_and_loading_states() {
    let mut app = app_single();
    let lines = render(&mut app, 100, 10);
    assert!(lines.iter().any(|l| l.contains("Loading sessions")));
    assert!(lines[8].contains("loading"));

    app.apply_poll(watchtower::poller::PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(watchtower::listing::parse_listing("[]").unwrap()),
    });
    let lines = render(&mut app, 100, 10);
    assert!(
        lines.iter().any(|l| l.contains("No sessions")),
        "{lines:#?}"
    );
    assert!(lines[0].contains("0 sessions"));
}

#[test]
fn poll_error_in_status_line() {
    let mut app = app_single();
    app.apply_poll(poll_err("main", "cannot run `claude`: not found"));
    let lines = render(&mut app, 120, 10);
    assert!(
        lines[8].contains("main: cannot run `claude`: not found"),
        "{}",
        lines[8]
    );
}

#[test]
fn sidecar_warning_in_status_line() {
    let mut app = app_single();
    app.add_warning("state file is corrupt; starting with empty state");
    let lines = render(&mut app, 140, 10);
    assert!(lines[8].contains("state file is corrupt"), "{}", lines[8]);
}

#[test]
fn help_overlay_lists_active_keys() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let lines = render(&mut app, 100, 24);
    let text = lines.join("\n");
    for needle in [
        "Keys",
        "move selection",
        "PgUp/PgDn",
        "attach; on a group header: collapse / expand",
        "refresh now",
        "quit",
        "filter by name, project, account",
        "group by state / project; show / hide interactive",
        "Esc or ? closes this help",
        "Ctrl+Z",
    ] {
        assert!(text.contains(needle), "{needle} missing in\n{text}");
    }
}

#[test]
fn picker_overlay_lists_accounts() {
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = render(&mut app, 100, 24);
    let text = lines.join("\n");
    assert!(text.contains("Which account owns touch-config?"), "{text}");
    assert!(line_with(&lines, "main  (claude)").contains("main"));
    assert!(line_with(&lines, "alt  (claude-alt)").contains("▶ alt"));
    assert!(text.contains("Esc cancel"));
}

#[test]
fn small_terminal_does_not_panic() {
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for (w, h) in [(1, 1), (10, 3), (20, 5), (40, 8)] {
        render(&mut app, w, h);
    }
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    for (w, h) in [(1, 1), (10, 3), (30, 6)] {
        render(&mut app, w, h);
    }
}

#[test]
fn long_list_scrolls_to_selection() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    // 6 lines: title, 3 list lines, status, footer.
    let lines = render(&mut app, 100, 6);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("▶") && l.contains("demo-app-3f")),
        "{lines:#?}"
    );
    assert_eq!(app.page_size, 3);
}
