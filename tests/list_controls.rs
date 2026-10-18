//! List controls of milestone 5 (doc/0006, doc/0011): the `/` text filter,
//! grouping by state or by project (`p`), collapsing groups (`Tab`, `Enter`
//! on a header), showing and hiding interactive rows (`i`), and where the
//! selection goes meanwhile. State machine first, then `TestBackend`
//! renders.

mod common;

use std::time::Instant;

use common::{app_two, ch, ctrl, headers, key, status_text, type_text, visible_rows};
use ratatui::Terminal;
use ratatui::crossterm::event::KeyCode;
use ratatui::{backend::TestBackend, buffer::Buffer};
use serde_json::{Value, json};
use watchtower::app::{App, Effect};
use watchtower::groups::{self, Group, GroupKey, Grouping};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;

const API: &str = "/home/user/workspace/api";
const WEB: &str = "/home/user/workspace/web";
const NOTES: &str = "/home/user/notes";

/// A background row; `live: false` gives a row without `pid` and
/// `status` (Stopped & finished). `startedAt` is the first change time.
fn bg(id: &str, name: &str, cwd: &str, state: &str, status: Option<&str>, started: i64) -> Value {
    let mut row = json!({
        "kind": "background",
        "id": id,
        "sessionId": format!("{id}-0000-4000-8000-000000000000"),
        "name": name,
        "cwd": cwd,
        "state": state,
        "startedAt": started,
    });
    if let Some(status) = status {
        row["status"] = json!(status);
        row["pid"] = json!(started / 100 + 1);
    }
    row
}

fn main_rows() -> Vec<Value> {
    let mut fix = bg(
        "aaaa0001",
        "api-fix",
        API,
        "blocked",
        Some("waiting"),
        1_000,
    );
    fix["waitingFor"] = json!("permission prompt");
    vec![
        fix,
        bg("aaaa0002", "api-tests", API, "working", Some("busy"), 5_000),
        bg("aaaa0003", "web-build", WEB, "done", Some("idle"), 9_000),
        bg("aaaa0004", "web-old", WEB, "done", None, 2_000),
    ]
}

fn alt_rows() -> Vec<Value> {
    vec![
        bg(
            "bbbb0001",
            "notes-sum",
            NOTES,
            "failed",
            Some("idle"),
            3_000,
        ),
        json!({
            "kind": "interactive",
            "pid": 77,
            "sessionId": "cccc0001-0000-4000-8000-000000000000",
            "name": "shell-web",
            "cwd": WEB,
            "status": "busy",
            "startedAt": 4_000,
        }),
    ]
}

fn update(account: &str, cycle: u64, rows: &[Value]) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle,
        result: Ok(parse_listing(&serde_json::to_string(rows).unwrap()).unwrap()),
    }
}

/// Accounts `main` and `alt` with separate lists, so each row's owner is
/// known (rule 2): the api and web rows run under main, notes-sum and the
/// interactive shell-web under alt.
fn app() -> App {
    let mut app = app_two();
    poll_both(&mut app, 0, main_rows());
    app
}

fn poll_both(app: &mut App, cycle: u64, main: Vec<Value>) {
    app.apply_poll(update("main", cycle, &main));
    app.apply_poll(update("alt", cycle, &alt_rows()));
}

fn selected(app: &App) -> String {
    match app.selected_header() {
        Some(group) => format!("[{}]", group.title()),
        None => app
            .selected_session()
            .map(|m| m.session.display_name())
            .unwrap_or_default(),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn titles(app: &App) -> Vec<String> {
    headers(app).into_iter().map(|h| h.0).collect()
}

// ---- the filter -------------------------------------------------------------

#[test]
fn filter_types_live_keeps_on_enter_and_clears_on_esc() {
    let mut app = app();
    assert_eq!(selected(&app), "api-fix");
    assert!(app.handle_key(ch('/')).is_empty());
    assert!(app.filter_editing());

    // Letters that are list keys elsewhere are just text here.
    type_text(&mut app, "qjpi?");
    assert_eq!(app.filter_text(), "qjpi?");
    for _ in 0..5 {
        assert!(app.handle_key(key(KeyCode::Backspace)).is_empty());
    }
    type_text(&mut app, "API");
    assert_eq!(app.filter_text(), "API");
    assert_eq!(visible_rows(&app), names(&["api-fix", "api-tests"]));
    // Groups without a match are hidden; counts are the matches.
    assert_eq!(
        headers(&app),
        [
            ("Needs me".to_owned(), 1, false),
            ("Working".to_owned(), 1, false)
        ]
    );
    assert!(
        status_text(&app).contains("filter \"API\": 2 matches"),
        "{}",
        status_text(&app)
    );

    // The selection follows the first match.
    type_text(&mut app, "-t");
    assert_eq!(visible_rows(&app), names(&["api-tests"]));
    assert_eq!(selected(&app), "api-tests");
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    assert_eq!(app.filter_text(), "API");
    assert_eq!(selected(&app), "api-fix");

    // Enter keeps the filter and gives the keys back to the list.
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(!app.filter_editing());
    assert_eq!(app.filter_text(), "API");
    app.handle_key(ch('j'));
    app.handle_key(ch('j'));
    assert_eq!(selected(&app), "api-tests");
    assert_eq!(app.handle_key(ch('q')), [Effect::Quit], "q quits again");

    // `/` edits the active filter: all words must match.
    app.handle_key(ch('/'));
    assert_eq!(app.filter_text(), "API");
    type_text(&mut app, " fix");
    assert_eq!(visible_rows(&app), names(&["api-fix"]));
    app.handle_key(key(KeyCode::Enter));

    // Esc in the list clears it; the selection stays on the session.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.filter_text(), "");
    assert!(!app.filter_active());
    assert_eq!(selected(&app), "api-fix");
    assert_eq!(titles(&app).len(), 6, "every group is back");
    assert!(!status_text(&app).contains("filter \""));

    // Esc while typing clears it too.
    app.handle_key(ch('/'));
    type_text(&mut app, "zzz");
    assert!(app.entries().is_empty(), "nothing matches");
    assert!(status_text(&app).contains("no matches"));
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.filter_editing());
    assert!(!app.filter_active());
    assert_eq!(titles(&app).len(), 6);
    assert_eq!(selected(&app), "api-fix");

    // An empty filter kept with Enter is no filter.
    app.handle_key(ch('/'));
    type_text(&mut app, "  ");
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.filter_active());
    assert_eq!(app.filter_text(), "");
}

#[test]
fn filter_input_keeps_the_reserved_keys() {
    let mut app = app();
    app.handle_key(ch('/'));
    type_text(&mut app, "web");
    // ← is never bound; Ctrl+Z stays a no-op with its hint; Ctrl+C quits.
    assert!(app.handle_key(key(KeyCode::Left)).is_empty());
    assert_eq!(app.filter_text(), "web");
    assert!(app.handle_key(ctrl('z')).is_empty());
    assert!(status_text(&app).contains("Ctrl+Z"));
    assert!(app.filter_editing());
    // ↑ / ↓ move the selection while typing.
    let first = selected(&app);
    app.handle_key(key(KeyCode::Down));
    assert_ne!(selected(&app), first);
    // Ctrl+U empties the line.
    app.handle_key(ctrl('u'));
    assert_eq!(app.filter_text(), "");
    assert!(app.filter_editing());
    assert_eq!(app.handle_key(ctrl('c')), [Effect::Quit]);
}

#[test]
fn filter_matches_name_project_and_account_ignoring_case() {
    let mut app = app();
    let mut matches = |text: &str| {
        app.handle_key(ch('/'));
        type_text(&mut app, text);
        let rows = visible_rows(&app);
        app.handle_key(key(KeyCode::Esc));
        rows
    };
    // Project (cwd), case-insensitive; the interactive row counts too.
    assert_eq!(
        matches("WEB"),
        names(&["web-build", "web-old", "shell-web"])
    );
    // The `~` form and the listed path both match.
    assert_eq!(matches("~/notes"), names(&["notes-sum"]));
    assert_eq!(matches("/home/user/notes"), names(&["notes-sum"]));
    // Account.
    assert_eq!(matches("alt"), names(&["notes-sum", "shell-web"]));
    // Every word must match, each in any field.
    assert_eq!(matches("main old"), names(&["web-old"]));
    assert_eq!(matches("alt api"), Vec::<String>::new());

    assert!(groups::filter_matches("", &["x"]));
    assert!(groups::filter_matches("Ab  cD", &["xxABxx", "cd"]));
    assert!(!groups::filter_matches("ab zz", &["ab"]));
}

#[test]
fn a_filter_opens_collapsed_groups_and_folding_waits() {
    let mut app = app();
    // Stopped & finished starts collapsed (doc/0006).
    assert!(app.is_collapsed(&GroupKey::State(Group::Stopped)));
    assert!(!visible_rows(&app).contains(&"web-old".to_owned()));

    app.handle_key(ch('/'));
    type_text(&mut app, "web-");
    app.handle_key(key(KeyCode::Enter));
    // While the filter is active the collapsed group shows its matches.
    assert_eq!(visible_rows(&app), names(&["web-build", "web-old"]));
    assert_eq!(
        headers(&app),
        [
            ("Ready for review".to_owned(), 1, false),
            ("Stopped & finished".to_owned(), 1, false)
        ]
    );
    // Folding is refused meanwhile, and says why.
    let before = app.entries().to_vec();
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.entries(), before.as_slice());
    assert!(status_text(&app).contains("groups stay open while a filter is active"));

    // Without the filter it is collapsed again.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.is_collapsed(&GroupKey::State(Group::Stopped)));
    assert!(!visible_rows(&app).contains(&"web-old".to_owned()));
}

#[test]
fn the_attention_counter_stays_global_under_a_filter() {
    let mut app = app();
    // api-fix (permission prompt) and notes-sum (failed) are highlighted.
    assert_eq!(app.attention_count(), 2);
    assert_eq!(app.attention_filtered(), 0);
    app.handle_key(ch('/'));
    type_text(&mut app, "api");
    assert_eq!(app.attention_count(), 2, "not filtered");
    assert_eq!(app.attention_filtered(), 1, "notes-sum is hidden");
    let status = render_lines(&mut app, 120, 12)[10].clone();
    assert!(status.contains("● 2 need you (filtered)"), "{status}");
    app.handle_key(key(KeyCode::Esc));
    let status = render_lines(&mut app, 120, 12)[10].clone();
    assert!(status.contains("● 2 need you"), "{status}");
    assert!(!status.contains("(filtered)"), "{status}");
}

// ---- grouping by project ----------------------------------------------------

#[test]
fn p_groups_by_project_newest_first_and_back() {
    let mut app = app();
    assert_eq!(app.grouping(), Grouping::State);
    groups_select(&mut app, "api-tests");
    assert!(app.handle_key(ch('p')).is_empty());
    assert_eq!(app.grouping(), Grouping::Project);
    assert!(status_text(&app).contains("grouped by project"));
    // Projects by their most recent change (web 9000, api 5000, notes
    // 3000); the interactive row keeps its own group, last.
    assert_eq!(
        headers(&app),
        [
            ("~/workspace/web".to_owned(), 2, false),
            ("~/workspace/api".to_owned(), 2, false),
            ("~/notes".to_owned(), 1, false),
            ("Interactive".to_owned(), 1, false),
        ]
    );
    // Inside a project rows follow the state order: Ready before Stopped,
    // Needs me before Working.
    assert_eq!(
        visible_rows(&app),
        names(&[
            "web-build",
            "web-old",
            "api-fix",
            "api-tests",
            "notes-sum",
            "shell-web"
        ])
    );
    // Same session selected; header highlight counts per project.
    assert_eq!(selected(&app), "api-tests");
    let lit: Vec<usize> = app
        .entries()
        .iter()
        .filter_map(|e| match e {
            watchtower::app::Entry::Header { highlighted, .. } => Some(*highlighted),
            _ => None,
        })
        .collect();
    assert_eq!(lit, [0, 1, 1, 0]);

    app.handle_key(ch('p'));
    assert_eq!(app.grouping(), Grouping::State);
    assert_eq!(selected(&app), "api-tests");
    assert_eq!(titles(&app)[0], "Needs me");
}

#[test]
fn project_grouping_orders_groups_and_rows() {
    // The same rule on plain sessions: no time sorts last, ties by name.
    let rows = parse_listing(
        &serde_json::to_string(&[
            bg("dddd0001", "b-done", API, "done", Some("idle"), 10),
            bg("dddd0002", "b-need", API, "blocked", Some("waiting"), 5),
            bg("dddd0003", "a-new", WEB, "working", Some("busy"), 50),
            json!({"kind": "background", "id": "dddd0004", "name": "z", "cwd": "", "state": "done", "status": "idle", "pid": 9}),
        ])
        .unwrap(),
    )
    .unwrap()
    .sessions;
    let home = std::path::Path::new(common::HOME);
    let grouped = groups::group_by_project_with(&rows, home, |s| s.started_at);
    let shape: Vec<(String, Vec<String>)> = grouped
        .iter()
        .map(|g| {
            (
                g.key.title().to_owned(),
                g.rows.iter().map(|s| s.display_name()).collect(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("~/workspace/web".to_owned(), names(&["a-new"])),
            ("~/workspace/api".to_owned(), names(&["b-need", "b-done"])),
            (groups::NO_PROJECT.to_owned(), names(&["z"])),
        ]
    );
}

// ---- collapsing ----------------------------------------------------------

#[test]
fn tab_and_enter_on_a_header_fold_groups() {
    let mut app = app();
    assert_eq!(selected(&app), "api-fix");
    // Tab on a row collapses its group; the selection moves to the header.
    assert!(app.handle_key(key(KeyCode::Tab)).is_empty());
    assert_eq!(selected(&app), "[Needs me]");
    // The collapsed header keeps its count and highlight count.
    let header = app.entries()[app.selected_entry().unwrap()].clone();
    assert_eq!(
        header,
        watchtower::app::Entry::Header {
            group: GroupKey::State(Group::NeedsMe),
            count: 1,
            highlighted: 1,
            collapsed: true,
        }
    );
    assert!(!visible_rows(&app).contains(&"api-fix".to_owned()));
    // Enter on a header expands it instead of attaching.
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(visible_rows(&app).contains(&"api-fix".to_owned()));
    assert_eq!(selected(&app), "[Needs me]");
    // Tab on a header toggles that group.
    app.handle_key(key(KeyCode::Tab));
    assert!(app.is_collapsed(&GroupKey::State(Group::NeedsMe)));

    // The default-collapsed Stopped group opens with Enter on its header.
    app.handle_key(key(KeyCode::End));
    while selected(&app) != "[Stopped & finished]" {
        app.handle_key(ch('k'));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(visible_rows(&app).contains(&"web-old".to_owned()));

    // Remembered for the session: across polls and grouping toggles.
    poll_both(&mut app, 1, main_rows());
    assert!(app.is_collapsed(&GroupKey::State(Group::NeedsMe)));
    assert!(!app.is_collapsed(&GroupKey::State(Group::Stopped)));
    app.handle_key(ch('p'));
    // Projects start expanded and fold on their own.
    assert!(headers(&app).iter().all(|h| !h.2));
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Tab));
    assert!(app.is_collapsed(&GroupKey::Project("~/workspace/web".into())));
    app.handle_key(ch('p'));
    assert!(app.is_collapsed(&GroupKey::State(Group::NeedsMe)));
    assert!(!app.is_collapsed(&GroupKey::State(Group::Stopped)));
    app.handle_key(ch('p'));
    assert!(app.is_collapsed(&GroupKey::Project("~/workspace/web".into())));
}

// ---- interactive rows ------------------------------------------------------

#[test]
fn i_hides_and_shows_the_interactive_group() {
    let mut app = app();
    assert!(app.shows_interactive(), "shown by default (doc/0006)");
    common::select_name(&mut app, "shell-web");
    assert!(app.handle_key(ch('i')).is_empty());
    assert!(!app.shows_interactive());
    assert!(!titles(&app).contains(&"Interactive".to_owned()));
    assert!(!visible_rows(&app).contains(&"shell-web".to_owned()));
    assert!(status_text(&app).contains("1 interactive hidden"));
    // The selection is still on a line of the list.
    assert!(app.selected_entry().unwrap() < app.entries().len());
    // Hidden in project mode too, and not counted by the filter.
    app.handle_key(ch('p'));
    assert!(!titles(&app).contains(&"Interactive".to_owned()));
    app.handle_key(ch('/'));
    type_text(&mut app, "web");
    assert_eq!(app.filter_match_count(), 2);
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('i'));
    assert!(titles(&app).contains(&"Interactive".to_owned()));
    assert!(!status_text(&app).contains("interactive hidden"));
}

// ---- selection across polls ---------------------------------------------------

#[test]
fn selection_stays_on_the_session_when_polls_add_and_remove_rows() {
    let mut app = app();
    common::select_name(&mut app, "web-build");
    // A new row at the top (Needs me) does not move the selection.
    let mut rows = main_rows();
    let mut new = bg("aaaa0009", "a-new", API, "blocked", Some("waiting"), 20_000);
    new["waitingFor"] = json!("input needed");
    rows.push(new);
    poll_both(&mut app, 1, rows.clone());
    assert_eq!(selected(&app), "web-build");
    // Under a filter, new matches do not move it either.
    app.handle_key(ch('/'));
    type_text(&mut app, "web");
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ch('j'));
    let at = selected(&app);
    poll_both(&mut app, 2, rows.clone());
    assert_eq!(selected(&app), at);
    app.handle_key(key(KeyCode::Esc));
    // The selected row leaves the listing: the selection stays valid.
    common::select_name(&mut app, "web-build");
    rows.retain(|r| r["name"] != "web-build");
    poll_both(&mut app, 3, rows);
    let e = app.selected_entry().expect("still a selection");
    assert!(e < app.entries().len());
    // An empty listing leaves nothing to select; rows coming back give the
    // first row again.
    poll_both(&mut app, 4, Vec::new());
    poll_both(&mut app, 5, main_rows());
    assert!(app.selected_entry().is_some());
}

/// Selects a row by name (expanding groups on the way).
fn groups_select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

// ---- rendering ---------------------------------------------------------------

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), 20_000))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render_lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let buf = render(app, w, h);
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

#[test]
fn project_mode_renders_project_headers_and_row_details() {
    let mut app = app();
    app.handle_key(ch('p'));
    let lines = render_lines(&mut app, 120, 16);
    let at = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
    assert!(at("▾ ~/workspace/web (2)") < at("▾ ~/workspace/api (2)  ● 1"));
    assert!(at("▾ ~/workspace/api (2)") < at("▾ ~/notes (1)  ● 1"));
    assert!(at("~/notes (1)") < at("Interactive (1)"));
    // Rows keep their state detail and the highlight.
    let fix = line_with(&lines, "api-fix");
    assert!(fix.contains("● api-fix"), "{fix}");
    assert!(fix.contains("permission prompt"), "{fix}");
    assert!(line_with(&lines, "web-old").contains("stopped"));
    assert!(line_with(&lines, "shell-web").contains("not attachable"));
}

#[test]
fn a_collapsed_header_renders_its_count_and_highlights() {
    let mut app = app();
    app.handle_key(key(KeyCode::Tab));
    let lines = render_lines(&mut app, 120, 16);
    let header = line_with(&lines, "Needs me");
    assert!(header.starts_with("▶ ▸ Needs me (1)  ● 1"), "{header}");
    assert!(line_with(&lines, "Stopped & finished").contains("▸ Stopped & finished (1)"));
    assert!(line_with(&lines, "Working").contains("▾ Working (1)"));
    assert!(!lines.iter().any(|l| l.contains("api-fix")));
    // The footer names the new keys where they fit.
    let footer = render_lines(&mut app, 200, 12).pop().unwrap();
    for hint in [
        "/ filter",
        "Tab fold",
        "p by project",
        "i interactive",
        "? help",
    ] {
        assert!(footer.contains(hint), "{hint}: {footer}");
    }
    assert!(footer.ends_with("in a session: Ctrl+Z returns to tower"));
    // A narrow footer drops whole hints, never half of one.
    let footer = render_lines(&mut app, 80, 12).pop().unwrap();
    assert!(!footer.contains('…'), "{footer}");
    assert!(footer.contains("? help"), "{footer}");
}

#[test]
fn the_filter_line_and_the_status_bar_show_the_filter() {
    let mut app = app();
    app.handle_key(ch('/'));
    type_text(&mut app, "api");
    // title, list…, input line, status, footer.
    let lines = render_lines(&mut app, 120, 14);
    let input = &lines[11];
    assert!(input.starts_with(" /api"), "{input}");
    assert!(
        input.contains("2 matches · Enter keep · Esc clear"),
        "{input}"
    );
    assert!(
        lines[12].contains("filter \"api\": 2 matches"),
        "{}",
        lines[12]
    );
    assert!(!lines.iter().any(|l| l.contains("web-build")));
    // After Enter the input line goes; the status bar keeps the filter.
    app.handle_key(key(KeyCode::Enter));
    let lines = render_lines(&mut app, 120, 14);
    assert!(!lines.iter().any(|l| l.starts_with(" /api")));
    assert!(
        lines[12].contains("filter \"api\": 2 matches"),
        "{}",
        lines[12]
    );
}

#[test]
fn help_names_every_list_key_and_fits_24_rows() {
    let mut app = app();
    app.handle_key(ch('?'));
    let lines = render_lines(&mut app, 80, 24);
    assert!(lines[0].contains("Keys"), "{}", lines[0]);
    assert!(
        lines[23].contains("Esc or ? closes this help"),
        "{}",
        lines[23]
    );
    let all = lines.join("\n");
    for needle in [
        "collapse / expand the group of the selected line",
        "filter by name, project, account; Esc clears it",
        "group by state / project; show / hide interactive",
        "on a group header: collapse / expand",
        "inside a session: return to tower",
        "Ctrl+S",
    ] {
        assert!(all.contains(needle), "{needle} missing in\n{all}");
    }
    assert!(!all.contains("Not available yet"));
}
