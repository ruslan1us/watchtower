//! A background task that outlives a finished turn (doc/0032): a live row
//! with `status: busy` outside `state: working` keeps its group and glyph,
//! its STATE column ends with ` · bg task`, and the dashboard's detail
//! panel explains the status. Only `working` + `busy` spins.

mod common;

use std::time::Instant;

use common::{HOME, NoEnviron, ch, key};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use serde_json::{Value, json};
use watchtower::app::App;
use watchtower::config::{Config, UiStyle, UiTheme};
use watchtower::groups::{self, Group};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;

const NOW_MS: i64 = 1_800_000_000_000;

/// The spinner frame at [`NOW_MS`].
const FRAME: &str = "⠋";

const BG_STATUS: &str = "busy — a background task (e.g. a shell) is still running; \
                         Claude Code may continue this session when it ends";

/// A live background row.
fn bg(id: &str, name: &str, cwd: &str, state: &str, status: &str, ago_s: i64) -> Value {
    json!({
        "id": id,
        "sessionId": format!("{id}-0000-4000-8000-000000000000"),
        "name": name,
        "cwd": format!("/home/user/workspace/{cwd}"),
        "kind": "background",
        "state": state,
        "status": status,
        "pid": 4_194_000,
        "startedAt": NOW_MS - ago_s * 1000,
    })
}

fn rows() -> Vec<Value> {
    vec![
        bg("1a1a1a1a", "shell-wait", "api", "done", "busy", 30),
        bg("2b2b2b2b", "print-date", "api", "done", "idle", 600),
        bg("3c3c3c3c", "run-test-suite", "web", "working", "busy", 120),
        bg("4d4d4d4d", "orchestrator", "tower", "done", "busy", 60),
    ]
}

fn app(style: UiStyle, theme: UiTheme) -> App {
    let home = std::path::Path::new(HOME);
    let mut app = App::new(
        Config::defaults(home),
        home.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    app.style = style;
    app.set_theme(theme);
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing(&serde_json::to_string(&rows()).unwrap()).unwrap()),
    });
    app
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
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

fn select(app: &mut App, name: &str) {
    app.handle_key(key(KeyCode::Home));
    for _ in 0..app.entries().len() {
        if app
            .selected_session()
            .is_some_and(|m| m.session.display_name() == name)
        {
            return;
        }
        app.handle_key(ch('j'));
    }
    panic!("cannot select {name}");
}

/// The side panel's status value, its wrapped lines joined: the cells
/// right of the `status` label up to the `changed` line, each without the
/// panel's right border.
fn side_status(l: &[String]) -> String {
    let y = index_of(l, "│ status ");
    let x = l[y][..l[y].find("│ status ").unwrap()].chars().count() + 1 + 10;
    let end = y + l[y..]
        .iter()
        .position(|r| r.contains("│ changed "))
        .unwrap();
    l[y..end]
        .iter()
        .map(|r| {
            let value: String = r.chars().skip(x).collect();
            value.trim_end_matches('│').trim().to_owned()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_helpers_tell_a_background_task_from_a_working_turn() {
    let listing = parse_listing(&serde_json::to_string(&rows()).unwrap()).unwrap();
    let by = |n: &str| {
        listing
            .sessions
            .iter()
            .find(|s| s.display_name() == n)
            .unwrap()
    };
    let shell = by("shell-wait");
    assert!(groups::bg_task(shell) && !groups::spins(shell));
    assert_eq!(groups::classify(shell), Group::ReadyForReview);
    assert_eq!(
        groups::detail(shell),
        "done",
        "the group detail is unchanged"
    );
    let working = by("run-test-suite");
    assert!(groups::spins(working) && !groups::bg_task(working));
    let idle = by("print-date");
    assert!(!groups::spins(idle) && !groups::bg_task(idle));
    let orch = by("orchestrator");
    assert!(groups::bg_task(orch));
    assert_eq!(groups::classify(orch), Group::Orchestrator);
    assert_eq!(groups::BG_TASK_STATUS, BG_STATUS);
}

#[test]
fn dashboard_rows_and_side_panel_in_two_themes() {
    for theme in [UiTheme::Neutral, UiTheme::Warm, UiTheme::Terminal] {
        let mut app = app(UiStyle::Dashboard, theme);
        select(&mut app, "shell-wait");
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);

        // Grouped by state: in Ready for review, with the state glyph and
        // the suffix, no spinner.
        let review = index_of(&l, "▾ Ready for review");
        let working = index_of(&l, "▾ Working");
        let y = index_of(&l, "shell-wait  ");
        assert!(review < y && y < working, "{theme:?}\n{}", l.join("\n"));
        let row = &l[y];
        assert!(row.contains("▪ shell-wait"), "{theme:?}: {row}");
        assert!(!row.contains(FRAME), "{theme:?}: {row}");
        assert!(row.contains("done · bg task"), "{theme:?}: {row}");
        // The glyph keeps the group's colour: the same as the state text.
        let gx = row.chars().position(|c| c == '▪').unwrap() as u16;
        let sx = row
            .find("done · bg task")
            .map(|b| row[..b].chars().count())
            .unwrap() as u16;
        assert_eq!(buf[(gx, y as u16)].fg, buf[(sx, y as u16)].fg, "{theme:?}");

        // `working` + `busy` still spins; `done` + `idle` is unchanged.
        let run = line_with(&l, "run-test-suite");
        assert!(run.contains(&format!("{FRAME} run-test-suite")), "{run}");
        assert!(run.contains("working") && !run.contains("bg task"), "{run}");
        let idle = line_with(&l, "print-date");
        assert!(idle.contains("▪ print-date"), "{idle}");
        assert!(idle.contains("done") && !idle.contains("bg task"), "{idle}");
        // The orchestrator: `ready · bg task`, no spinner.
        let orch = line_with(&l, "▪ orchestrator");
        assert!(orch.contains("ready · bg task"), "{orch}");

        // The side panel explains the status, wrapped to its width,
        // without a spinner.
        let status = side_status(&l);
        assert_eq!(status, BG_STATUS, "{theme:?}\n{}", l.join("\n"));
        let first = &l[index_of(&l, " status ")];
        assert!(!first.contains(FRAME), "{first}");
        assert!(line_with(&l, "│ state ").contains("done"));
    }
}

#[test]
fn a_working_busy_row_keeps_its_spinning_status() {
    let mut app = app(UiStyle::Dashboard, UiTheme::Neutral);
    select(&mut app, "run-test-suite");
    let l = lines(&render(&mut app, 160, 40));
    let status = line_with(&l, " status ");
    assert!(status.contains(&format!("{FRAME} busy")), "{status}");
    assert!(!l.iter().any(|x| x.contains("background task")));
}

#[test]
fn the_detail_strip_shows_the_compact_status() {
    let mut app = app(UiStyle::Dashboard, UiTheme::Warm);
    select(&mut app, "shell-wait");
    let l = lines(&render(&mut app, 100, 30));
    let strip = line_with(&l, "changed");
    assert!(strip.contains("done · busy (bg task) · changed"), "{strip}");
    assert!(!strip.contains(FRAME), "{strip}");
}

#[test]
fn a_narrow_dashboard_shortens_the_suffix() {
    // Long names and folders push the list to its shortest state details.
    let mut rows = rows();
    rows.push(bg(
        "5e5e5e5e",
        "a-rather-long-session-name-here",
        "a-rather-long-project-folder-name",
        "working",
        "idle",
        5,
    ));
    let mut app = app(UiStyle::Dashboard, UiTheme::Neutral);
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 1,
        result: Ok(parse_listing(&serde_json::to_string(&rows).unwrap()).unwrap()),
    });
    let l = lines(&render(&mut app, 80, 24));
    let row = line_with(&l, "shell-wait");
    assert!(row.contains("done · bg"), "{}", l.join("\n"));
    assert!(!row.contains("bg task"), "{row}");
    assert!(!row.contains(FRAME), "{row}");
    assert!(line_with(&l, "▪ orchestrator").contains("ready · bg"));
    let run = line_with(&l, "run-test-suite");
    assert!(run.contains(&format!("{FRAME} run-test-suite")), "{run}");
    // Nothing spills past the border.
    assert!(l.iter().all(|x| x.chars().count() <= 80));
}

#[test]
fn the_minimal_style_shows_the_suffix() {
    let mut app = app(UiStyle::Minimal, UiTheme::Neutral);
    let l = lines(&render(&mut app, 160, 40));
    assert!(line_with(&l, "shell-wait").contains("done · bg task"));
    assert!(line_with(&l, "orchestrator").contains("ready · bg task"));
    let run = line_with(&l, "run-test-suite");
    assert!(run.contains("working") && !run.contains("bg task"), "{run}");
    let idle = line_with(&l, "print-date");
    assert!(idle.contains("done") && !idle.contains("bg task"), "{idle}");
    // Grouped by state: the row stays in Ready for review.
    let review = index_of(&l, "Ready for review");
    let y = index_of(&l, "shell-wait");
    assert!(review < y && y < index_of(&l, "Working"));
    // A narrow terminal cuts the row at its edge, nothing more.
    let l = lines(&render(&mut app, 60, 20));
    assert!(line_with(&l, "shell-wait").chars().count() <= 60);
}

#[test]
#[ignore = "prints the screens for a look"]
fn dump() {
    let mut app = app(UiStyle::Dashboard, UiTheme::Neutral);
    select(&mut app, "shell-wait");
    for (w, h) in [(160, 40), (100, 30), (80, 24)] {
        println!("{w}x{h}\n{}", lines(&render(&mut app, w, h)).join("\n"));
    }
    app.style = UiStyle::Minimal;
    println!("minimal\n{}", lines(&render(&mut app, 100, 20)).join("\n"));
}
