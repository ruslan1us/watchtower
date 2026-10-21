//! The dashboard's details panel with long values (doc/0041): the side
//! panel wraps the name, the folder and the status at spaces, `/` and
//! `-` onto at most three rows and uses only the rows it has free; the
//! strip and the titles cut with `…`; the keys row counts only the gaps
//! between keys and may take a second row.

mod common;

use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use watchtower::app::App;
use watchtower::config::UiStyle;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;

const NOW_MS: i64 = 1_800_000_000_000;
const LONG_NAME: &str = "refactor-the-authentication-middleware-and-session-store";
const LONG_FOLDER: &str = "billing-platform-frontend-with-a-very-long-folder-name";

/// A dashboard app with one row named `name` in `~/workspace/<folder>`,
/// `state` and `status` as given (`waiting_for` when not empty), selected.
fn app(name: &str, folder: &str, state: &str, status: &str, waiting_for: &str) -> App {
    let mut app = common::app_single();
    app.style = UiStyle::Dashboard;
    let waiting = if waiting_for.is_empty() {
        String::new()
    } else {
        format!(r#""waitingFor": "{waiting_for}","#)
    };
    let rows = format!(
        r#"[
        {{"id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000001",
         "cwd": "/home/user/workspace/{folder}", "kind": "background", "name": "{name}",
         "state": "{state}", "pid": 4242, "status": "{status}", {waiting}
         "startedAt": {}}}
    ]"#,
        NOW_MS - 60_000
    );
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 1,
        result: Ok(parse_listing(&rows).unwrap()),
    });
    common::select_name(&mut app, name);
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
        .collect()
}

/// The side panel's part of each row: from its left border on.
fn side(lines: &[String]) -> Vec<String> {
    let at = lines
        .iter()
        .find_map(|l| l.find("╮╭").map(|b| l[..b].chars().count() + 1))
        .unwrap_or_else(|| panic!("no side panel:\n{}", lines.join("\n")));
    lines.iter().map(|l| l.chars().skip(at).collect()).collect()
}

/// The index of the row holding `needle`.
fn row_of(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in\n{}", lines.join("\n")))
}

#[test]
fn a_long_folder_wraps_after_slashes_and_dashes() {
    let mut a = app("fix-login", LONG_FOLDER, "done", "idle", "");
    let s = side(&render(&mut a, 160, 40));
    let y = row_of(&s, "│ folder    ~/workspace/billing-platform-  ");
    assert!(
        s[y + 1].starts_with("│           frontend-with-a-very-long-folder-  "),
        "{}",
        s[y + 1]
    );
    assert!(s[y + 2].starts_with("│           name  "), "{}", s[y + 2]);
    // Nothing is cut: no `…` in the panel's body.
    assert!(!s[1..s.len() - 1].iter().any(|l| l.contains('…')), "{s:#?}");
}

#[test]
fn a_long_status_wraps_at_spaces_onto_three_rows_at_most() {
    let question = "which of the three deployment regions should the new cache cluster \
                    use for the billing service and its nightly jobs";
    let mut a = app("ask", "api", "blocked", "waiting", question);
    let s = side(&render(&mut a, 160, 40));
    let y = row_of(&s, "│ status    waiting · which of the three");
    assert!(
        s[y + 1].starts_with("│           deployment"),
        "{}",
        s[y + 1]
    );
    assert!(s[y + 2].starts_with("│           "), "{}", s[y + 2]);
    assert!(s[y + 2].trim_end().ends_with("… │"), "{}", s[y + 2]);
    assert!(s[y + 3].starts_with("│ changed   "), "{}", s[y + 3]);
}

#[test]
fn a_long_name_is_cut_in_the_title_and_shown_whole_in_the_body() {
    let mut a = app(LONG_NAME, "api", "done", "idle", "");
    let s = side(&render(&mut a, 160, 40));
    // The title fits between the corners and ends in `…`.
    let t = row_of(&s, "╭ refactor-the-authentication-");
    assert_eq!(
        s[t].trim_end(),
        "╭ refactor-the-authentication-middleware-and-… ╮"
    );
    let y = row_of(&s, "│ name      refactor-the-authentication-  ");
    assert!(
        s[y + 1].starts_with("│           middleware-and-session-store  "),
        "{}",
        s[y + 1]
    );
}

#[test]
fn a_short_name_has_no_name_line() {
    let mut a = app("fix-login", "api", "done", "idle", "");
    let s = side(&render(&mut a, 160, 40)).join("\n");
    assert!(s.contains("╭ fix-login ─"), "{s}");
    assert!(!s.contains(" name  "), "{s}");
}

#[test]
fn a_short_panel_keeps_every_line_and_gives_its_free_rows_in_turn() {
    let mut a = app(LONG_NAME, LONG_FOLDER, "done", "idle", "");
    let lines = render(&mut a, 140, 24);
    let s = side(&lines);
    let text = s.join("\n");
    // Every line is still there.
    for label in ["name", "group", "folder", "account", "hand-off"] {
        assert!(text.contains(&format!("│ {label:<10}")), "{label}:\n{text}");
    }
    // The one free row goes to the folder, whose second row ends in `…`;
    // the name keeps one row.
    let y = row_of(&s, "│ folder    ~/workspace/billing-platform-  ");
    assert!(
        s[y + 1].starts_with("│           frontend-"),
        "{}",
        s[y + 1]
    );
    assert!(s[y + 1].trim_end().ends_with("… │"), "{}", s[y + 1]);
    assert!(s[y + 2].starts_with("│ account"), "{}", s[y + 2]);
    let n = row_of(&s, "│ name      refactor-the-");
    assert!(s[n].trim_end().ends_with("… │"), "{}", s[n]);
    assert!(s[n + 1].starts_with("│ group"), "{}", s[n + 1]);
    // No free row is left above the rule.
    let rule = row_of(&s, "│───");
    assert!(s[rule - 1].starts_with("│ hand-off"), "{}", s[rule - 1]);
}

#[test]
fn the_strip_cuts_its_right_half_with_an_ellipsis() {
    let mut a = app("fix-login", LONG_FOLDER, "done", "idle", "");
    let lines = render(&mut a, 90, 28);
    let y = row_of(&lines, "folder    ~/workspace/billing-platform-");
    let row = &lines[y];
    assert!(row.ends_with("… │"), "{row}");
    assert_eq!(unicode_width::UnicodeWidthStr::width(row.as_str()), 90);
}

#[test]
fn the_strip_title_is_cut_with_an_ellipsis() {
    let name = "a-session-name-that-is-much-longer-than-the-eighty-columns-of-this-small-terminal";
    let mut a = app(name, "api", "done", "idle", "");
    let lines = render(&mut a, 80, 26);
    let y = row_of(&lines, "╭ a-session-name-that-is-much-longer");
    assert!(lines[y].ends_with("… ╮"), "{}", lines[y]);
    assert_eq!(unicode_width::UnicodeWidthStr::width(lines[y].as_str()), 80);
}

#[test]
fn the_keys_take_a_second_row_when_the_first_is_full() {
    // A highlighted row offers Enter, Space, m, s and !: the first four
    // fit 46 columns with the gaps between them, `! shell` goes on below.
    let mut a = app("ask", "api", "blocked", "waiting", "permission prompt");
    let s = side(&render(&mut a, 160, 40));
    let y = row_of(&s, "│ Enter attach  Space logs  m seen  s stop  ");
    assert!(s[y - 1].starts_with("│───"), "the rule above: {}", s[y - 1]);
    assert!(s[y + 1].starts_with("│ ! shell  "), "{}", s[y + 1]);
    assert!(s[y + 2].starts_with("╰"), "{}", s[y + 2]);
}

#[test]
fn keys_that_fit_one_row_take_one() {
    let mut a = app("fix-login", "api", "done", "idle", "");
    let s = side(&render(&mut a, 160, 40));
    let y = row_of(&s, "│ Enter attach  Space logs  s stop  ! shell  ");
    assert!(s[y - 1].starts_with("│───"), "{}", s[y - 1]);
    assert!(s[y + 1].starts_with("╰"), "{}", s[y + 1]);
}
