//! The `git` line of the dashboard's details panel (doc/0037): in the side
//! panel and the strip, in two themes, cut on a narrow terminal, absent
//! without a summary and in the minimal style. The summaries are fed in as
//! a finished job would; no git runs here.

mod common;

use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use watchtower::app::App;
use watchtower::config::{UiStyle, UiTheme};
use watchtower::gitinfo::{GitJob, GitState, GitSummary, Head, Upstream};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;

const API: &str = "/home/user/workspace/api";
const NOW_MS: i64 = 1_800_000_000_000;

/// A dashboard app with two rows; `fix-login` (in `API`) is selected.
fn app(theme: UiTheme) -> App {
    let mut app = common::app_single();
    app.style = UiStyle::Dashboard;
    app.set_theme(theme);
    let rows = format!(
        r#"[
        {{"id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000001",
         "cwd": "{API}", "kind": "background", "name": "fix-login",
         "state": "done", "pid": 4242, "status": "idle"}},
        {{"id": "5e6f7a8b", "sessionId": "5e6f7a8b-0000-4000-8000-000000000002",
         "cwd": "/home/user/workspace/web", "kind": "background", "name": "pick-colour",
         "state": "done", "pid": 4243, "status": "idle"}}
    ]"#
    );
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 1,
        result: Ok(parse_listing(&rows).unwrap()),
    });
    common::select_name(&mut app, "fix-login");
    app
}

fn feed(app: &mut App, cwd: &str, branch: &str) {
    app.git_finished(
        GitJob { cwd: cwd.into() },
        GitState::Summary(GitSummary {
            head: Head::Branch(branch.into()),
            changed: 3,
            upstream: Upstream::Tracking {
                ahead: 2,
                behind: 0,
            },
        }),
    );
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

/// The row and column where `needle` starts.
fn find(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let l = lines(buf);
    l.iter().enumerate().find_map(|(y, line)| {
        line.find(needle).map(|byte| {
            let x = unicode_width::UnicodeWidthStr::width(&line[..byte]);
            (y as u16, x as u16)
        })
    })
}

const LINE: &str = "git       feature/login · 3 changed · ↑2";

#[test]
fn the_side_panel_shows_the_git_line_under_the_folder() {
    let mut app = app(UiTheme::Neutral);
    feed(&mut app, API, "feature/login");
    let buf = render(&mut app, 160, 40);
    let (gy, gx) = find(&buf, LINE).unwrap_or_else(|| panic!("{}", lines(&buf).join("\n")));
    let (fy, fx) = find(&buf, "folder    ~/workspace/api").unwrap();
    assert_eq!((gy, gx), (fy + 1, fx), "right under the folder");
    // The label is faint, the value is text: truecolor in neutral.
    let value = &buf[(gx + 10, gy)];
    assert_eq!(value.symbol(), "f");
    assert!(matches!(value.fg, Color::Rgb(..)), "{:?}", value.fg);
    assert_ne!(buf[(gx, gy)].fg, value.fg, "label and value differ");
}

#[test]
fn the_terminal_theme_uses_named_colours() {
    let mut app = app(UiTheme::Terminal);
    feed(&mut app, API, "feature/login");
    let buf = render(&mut app, 160, 40);
    let (gy, gx) = find(&buf, LINE).unwrap_or_else(|| panic!("{}", lines(&buf).join("\n")));
    let value = &buf[(gx + 10, gy)];
    assert!(!matches!(value.fg, Color::Rgb(..)), "{:?}", value.fg);
    assert!(!matches!(buf[(gx, gy)].fg, Color::Rgb(..)));
}

#[test]
fn the_strip_shows_it_as_its_fourth_line() {
    let mut app = app(UiTheme::Warm);
    feed(&mut app, API, "feature/login");
    let buf = render(&mut app, 100, 30);
    let l = lines(&buf);
    let strip = l
        .iter()
        .position(|x| x.starts_with("╭ fix-login"))
        .unwrap_or_else(|| panic!("{}", l.join("\n")));
    assert!(
        l[strip + 1].contains("folder    ~/workspace/api"),
        "{}",
        l[strip + 1]
    );
    assert!(l[strip + 3].contains("hand-off"), "{}", l[strip + 3]);
    assert!(l[strip + 4].contains(LINE), "{}", l[strip + 4]);
    assert!(l[strip + 5].starts_with("╰"), "{}", l[strip + 5]);
}

#[test]
fn a_narrow_strip_cuts_the_branch_and_keeps_the_counts() {
    let mut app = app(UiTheme::Neutral);
    feed(
        &mut app,
        API,
        "feature/an-extremely-long-branch-name-that-does-not-fit-in-eighty-columns",
    );
    let buf = render(&mut app, 80, 26);
    let l = lines(&buf);
    let row = l
        .iter()
        .find(|x| x.contains("git       feature/an-"))
        .unwrap_or_else(|| panic!("{}", l.join("\n")));
    assert!(row.contains("… · 3 changed · ↑2 │"), "{row}");
    assert_eq!(unicode_width::UnicodeWidthStr::width(row.as_str()), 80);
}

#[test]
fn a_long_branch_in_the_side_panel_is_cut_too() {
    let mut app = app(UiTheme::Neutral);
    feed(
        &mut app,
        API,
        "feature/a-branch-name-longer-than-the-side-panel",
    );
    let l = lines(&render(&mut app, 160, 40));
    let row = l
        .iter()
        .find(|x| x.contains("git       feature/a-"))
        .unwrap();
    assert!(row.contains("… · 3 changed · ↑2"), "{row}");
}

#[test]
fn no_line_without_a_summary() {
    let mut app = app(UiTheme::Neutral);
    // Another folder's summary, and none for the selected one.
    feed(&mut app, "/home/user/workspace/web", "main");
    let side = lines(&render(&mut app, 160, 40)).join("\n");
    assert!(side.contains("folder    ~/workspace/api"), "{side}");
    assert!(!side.contains("git  "), "{side}");
    // Not a repository: no line either.
    app.git_finished(GitJob { cwd: API.into() }, GitState::NotRepo);
    let strip = lines(&render(&mut app, 100, 30)).join("\n");
    assert!(!strip.contains("git  "), "{strip}");
}

#[test]
fn the_minimal_style_shows_no_git_line() {
    let mut app = app(UiTheme::Neutral);
    app.style = UiStyle::Minimal;
    feed(&mut app, API, "feature/login");
    let screen = lines(&render(&mut app, 160, 40)).join("\n");
    assert!(!screen.contains("feature/login"), "{screen}");
}
