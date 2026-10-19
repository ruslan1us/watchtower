//! The dashboard style (doc/0020) rendered with ratatui's `TestBackend`:
//! header pills, the sessions panel with its separators, the detail panel
//! and its strip, the logs panel in its place, the fallback to the minimal
//! layout on small terminals, `NO_COLOR`, and the colour themes
//! (doc/0021). The layout tests draw with `tokyo-night`, the palette they
//! were written against.

mod common;

use std::path::Path;
use std::time::Instant;

use common::{HOME, NoEnviron, ch, fixture_path, key};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use watchtower::app::{App, LogsTarget};
use watchtower::config::{Config, UiStyle, UiTheme};
use watchtower::listing::parse_listing;
use watchtower::logview;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;

const NOW_MS: i64 = 1_800_000_000_000;

/// A background row; live (with a pid) when it has a `status`.
fn bg(
    id: &str,
    name: &str,
    cwd: &str,
    state: &str,
    status: Option<&str>,
    waiting: Option<&str>,
    ago_s: i64,
) -> Value {
    let mut row = json!({
        "id": id,
        "sessionId": format!("{id}-0000-4000-8000-000000000000"),
        "name": name,
        "cwd": format!("/home/user/workspace/{cwd}"),
        "kind": "background",
        "state": state,
        "startedAt": NOW_MS - ago_s * 1000,
    });
    if let Some(status) = status {
        row["status"] = json!(status);
        row["pid"] = json!(4_194_000);
    }
    if let Some(w) = waiting {
        row["waitingFor"] = json!(w);
    }
    row
}

fn main_rows() -> Vec<Value> {
    vec![
        bg(
            "d4a1f7c8",
            "touch-config",
            "api",
            "blocked",
            Some("waiting"),
            Some("permission prompt"),
            5,
        ),
        bg(
            "9c6e1a35",
            "pick-colour",
            "web",
            "blocked",
            Some("waiting"),
            Some("input needed"),
            42,
        ),
        bg(
            "3e9a1c07",
            "bad-model",
            "infra",
            "failed",
            Some("idle"),
            None,
            300,
        ),
        bg(
            "7b4d9f02",
            "print-date",
            "api",
            "done",
            Some("idle"),
            None,
            720,
        ),
        bg(
            "5a7c2e91",
            "run-test-suite",
            "api",
            "working",
            Some("busy"),
            None,
            120,
        ),
        bg(
            "0f1e2d3c",
            "orchestrator",
            "tower",
            "blocked",
            Some("idle"),
            None,
            60,
        ),
        bg("b2f8d064", "say-ok", "api", "done", None, None, 7200),
        json!({
            "kind": "interactive",
            "name": "demo-app-3f",
            "cwd": "/home/user/workspace/infra",
            "pid": 1201,
            "status": "idle",
            "startedAt": NOW_MS - 1_500_000,
        }),
    ]
}

fn alt_rows() -> Vec<Value> {
    vec![bg(
        "6a5b4c3d",
        "docs-cleanup",
        "web",
        "working",
        Some("idle"),
        None,
        1080,
    )]
}

fn update(account: &str, rows: &[Value]) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(&serde_json::to_string(rows).unwrap()).unwrap()),
    }
}

/// Accounts `main` (group work) and `alt` (group personal), the dashboard
/// style in the `tokyo-night` theme, and one poll of each.
fn app() -> App {
    themed(UiTheme::TokyoNight)
}

/// As [`app`], in `theme`.
fn themed(theme: UiTheme) -> App {
    let config = Config::parse(
        r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal"
"#,
        Path::new(HOME),
    )
    .unwrap();
    assert_eq!(config.ui_style, UiStyle::Dashboard);
    assert_eq!(config.ui_theme, UiTheme::Neutral);
    let mut app = App::new(
        config.clone(),
        HOME.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    app.style = config.ui_style;
    assert_eq!(app.theme, UiTheme::Neutral, "from the config");
    app.set_theme(theme);
    app.apply_poll(update("main", &main_rows()));
    app.apply_poll(update("alt", &alt_rows()));
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

fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    lines(&render(app, w, h))
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

/// Moves the selection down to the row named `name`.
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

/// The cell where `needle` starts on the first line containing it.
fn cell_at(lines: &[String], needle: &str) -> (u16, u16) {
    let y = index_of(lines, needle);
    let prefix = &lines[y][..lines[y].find(needle).unwrap()];
    let x = unicode_width::UnicodeWidthStr::width(prefix);
    (x as u16, y as u16)
}

#[test]
fn wide_dashboard_has_header_list_and_detail_side_by_side() {
    let mut app = app();
    select(&mut app, "pick-colour");
    let buf = render(&mut app, 160, 40);
    let l = lines(&buf);

    // Header panel: title, grouping, a pill per non-empty state group with
    // `● n` for highlighted rows, the Orchestrator included, and the
    // accounts with their groups.
    assert!(l[0].starts_with("╭ ◆ tower"), "{}", l[0]);
    assert!(l[0].contains("by state"), "{}", l[0]);
    let pills = &l[1];
    for pill in [
        "Orch. 1",
        "Needs me 2  ● 2",
        "Failed 1  ● 1",
        "Review 1",
        "Working 2",
        "Stopped 1",
        "Interact. 1",
    ] {
        assert!(pills.contains(pill), "{pill}: {pills}");
    }
    assert!(pills.contains("main  work · 8"), "{pills}");
    assert!(pills.contains("alt  personal · 1"), "{pills}");
    // The share bar.
    assert!(l[2].contains("━━━━"), "{}", l[2]);

    // Sessions panel: title with the shown count (Stopped & finished
    // starts folded), the column header, separators joined to the border.
    let top = line_with(&l, "╭ Sessions");
    assert!(top.contains("8 of 9 shown"), "{top}");
    let head = line_with(&l, "NAME");
    for col in ["PROJECT", "STATE", "AGE", "ACCT"] {
        assert!(head.contains(col), "{head}");
    }
    assert!(line_with(&l, "├─▾ Orchestrator 1").contains('┤'));
    assert!(line_with(&l, "├─▾ Needs me 2  ● 2").contains('┤'));
    assert!(l.iter().any(|x| x.contains("├─▸ Stopped & finished 1")));
    assert!(
        !l.iter().any(|x| x.contains("say-ok")),
        "folded rows hidden"
    );
    let order = [
        "▾ Orchestrator",
        "▾ Needs me",
        "▾ Failed",
        "▾ Ready for review",
        "▾ Working",
    ];
    let pos: Vec<usize> = order.iter().map(|h| index_of(&l, h)).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");

    // Rows: marker, glyph or spinner, name, project, state, age, account.
    let row = line_with(&l, "touch-config");
    assert!(row.starts_with("│ ● ▪ touch-config"), "{row}");
    assert!(row.contains("~/workspace/api") && row.contains("permission prompt"));
    assert!(row.contains(" now  main"), "{row}");
    let sel = line_with(&l, "pick-colour  ");
    assert!(sel.starts_with("│▌● ▪ pick-colour"), "{sel}");
    assert!(sel.contains("42s  main"), "{sel}");
    // The orchestrator is never highlighted (doc/0007).
    let orch = line_with(&l, "▪ orchestrator");
    assert!(orch.starts_with("│   ▪ orchestrator"), "{orch}");
    assert!(orch.contains("ready"), "{orch}");
    // A busy row spins; the frame follows the clock.
    assert!(line_with(&l, "run-test-suite").contains("⠋ run-test-suite"));
    assert!(line_with(&l, "docs-cleanup").contains("18m  alt"));
    assert!(line_with(&l, "demo-app-3f").contains("idle · not attachable"));

    // Detail panel on the right, for the selected row.
    let (x, _) = cell_at(&l, "╭ pick-colour");
    assert!(x >= 100, "side panel at column {x}");
    for text in [
        "● needs you: input needed",
        "group     Needs me",
        "state     blocked",
        "status    waiting · input needed",
        "changed   42s ago",
        "folder    ~/workspace/web",
        "account    main  work · claude",
        "id        9c6e1a35",
        "kind      background",
        "hand-off  no other work account",
        "Enter attach  Space logs  m seen",
    ] {
        assert!(
            l.iter().any(|x| x.contains(text)),
            "{text}\n{}",
            l.join("\n")
        );
    }

    // Status line and footer as in the minimal style (doc/0011).
    assert!(l[38].starts_with(" ● 3 need you"), "{}", l[38]);
    assert!(l[38].contains("refreshed"), "{}", l[38]);
    let footer = &l[39];
    assert!(
        footer.contains("Enter attach") && footer.contains("? help"),
        "{footer}"
    );
    assert!(
        footer.ends_with("in a session: Ctrl+Z returns to tower"),
        "{footer}"
    );

    // Colours: the marker and a highlighted state in the accent, bold.
    let (mx, my) = cell_at(&l, "● ▪ touch-config");
    let marker = &buf[(mx, my)];
    assert_eq!(marker.symbol(), "●");
    assert!(marker.modifier.contains(Modifier::BOLD));
    assert!(matches!(marker.fg, Color::Rgb(..)), "{:?}", marker.fg);
}

#[test]
fn medium_dashboard_puts_the_detail_strip_under_the_list() {
    let mut app = app();
    select(&mut app, "pick-colour");
    let l = screen(&mut app, 100, 30);

    assert!(l[0].contains("◆ tower") && l[0].contains("by state"));
    assert!(l[1].contains("Needs me 2  ● 2"), "{}", l[1]);
    assert!(line_with(&l, "╭ Sessions").contains("8 of 9 shown"));
    assert!(line_with(&l, "NAME").contains("ACCT"));
    // The strip sits below the list's bottom border.
    let list_end = l
        .iter()
        .rposition(|x| x.starts_with("╰"))
        .and_then(|last| l[..last].iter().rposition(|x| x.starts_with("╰")))
        .unwrap();
    let strip = index_of(&l, "╭ pick-colour");
    assert!(strip > list_end, "{strip} > {list_end}\n{}", l.join("\n"));
    let first = &l[strip + 1];
    assert!(first.contains("● needs you: input needed"), "{first}");
    assert!(first.contains("folder    ~/workspace/web"), "{first}");
    let second = &l[strip + 2];
    assert!(
        second.contains("blocked · waiting · changed 42s ago"),
        "{second}"
    );
    assert!(
        second.contains("account    main  work · claude"),
        "{second}"
    );
    let third = &l[strip + 3];
    assert!(third.contains("id 9c6e1a35 · background"), "{third}");
    assert!(third.contains("hand-off  no other work account"), "{third}");
    let footer = &l[29];
    assert!(footer.contains("? help"), "{footer}");
    assert!(
        footer.ends_with("in a session: Ctrl+Z returns to tower"),
        "{footer}"
    );
}

#[test]
fn smallest_dashboard_keeps_the_list_and_drops_the_extras() {
    let mut app = app();
    select(&mut app, "pick-colour");
    let l = screen(&mut app, 80, 24);

    assert!(l[0].starts_with("╭ ◆ tower"), "{}", l[0]);
    assert!(l[1].contains("Needs me 2  ● 2"), "{}", l[1]);
    // No share bar, column header or detail strip below 26 rows.
    assert!(l[2].starts_with("╰"), "{}", l[2]);
    assert!(!l.iter().any(|x| x.contains("NAME")));
    assert!(!l.iter().any(|x| x.contains("╭ pick-colour")));
    assert!(line_with(&l, "╭ Sessions").contains("8 of 9 shown"));
    assert!(line_with(&l, "pick-colour").starts_with("│▌● ▪ pick-colour"));
    assert!(l.iter().any(|x| x.contains("▾ Orchestrator 1")));
    assert!(l[22].starts_with(" ● 3 need you"), "{}", l[22]);
    let footer = &l[23];
    assert!(footer.contains("? help"), "{footer}");
    assert!(
        footer.ends_with("in a session: Ctrl+Z returns to tower"),
        "{footer}"
    );
}

#[test]
fn a_selected_group_header_shows_a_group_summary() {
    let mut app = app();
    select(&mut app, "touch-config");
    app.handle_key(ch('k'));
    assert!(app.selected_header().is_some());
    let l = screen(&mut app, 160, 40);
    assert!(
        l.iter().any(|x| x.contains("╭ Needs me")),
        "{}",
        l.join("\n")
    );
    // The selected header carries the selection bar.
    assert!(l.iter().any(|x| x.starts_with("├▌▾ Needs me 2  ● 2")));
    for text in [
        "sessions  2 sessions",
        "attention ● 2",
        "accounts  main 2",
        "Enter fold",
    ] {
        assert!(
            l.iter().any(|x| x.contains(text)),
            "{text}\n{}",
            l.join("\n")
        );
    }
    // Folded with Enter: the summary says how to unfold.
    app.handle_key(key(KeyCode::Enter));
    let l = screen(&mut app, 160, 40);
    assert!(l.iter().any(|x| x.starts_with("├▌▸ Needs me 2  ● 2")));
    assert!(l.iter().any(|x| x.contains("Enter unfold")));
    assert!(line_with(&l, "╭ Sessions").contains("6 of 9 shown"));
}

/// Opens the logs panel on the selected row and feeds it a fixture.
fn open_logs(app: &mut App, fixture: &str) {
    app.handle_key(ch(' '));
    let target: LogsTarget = app.logs().unwrap().in_flight().cloned().unwrap();
    let bytes = std::fs::read(fixture_path(&format!("logs/{fixture}.ansi"))).unwrap();
    app.logs_fetched(target, Ok(logview::render(&bytes)));
}

#[test]
fn the_logs_panel_takes_the_detail_panels_place() {
    let mut app = app();
    select(&mut app, "pick-colour");
    open_logs(&mut app, "question");

    let buf = render(&mut app, 160, 40);
    let l = lines(&buf);
    let (x, y) = cell_at(&l, "logs · pick-colour (main)");
    assert!(x >= 80, "side logs at column {x}");
    assert!(y < 10, "{y}");
    assert!(
        !l.iter().any(|x| x.contains("● needs you:")),
        "detail replaced"
    );
    assert!(l.iter().any(|x| x.contains("Space/Esc close")));

    // Narrower: under the list, in place of the strip.
    let l = screen(&mut app, 100, 30);
    let logs = index_of(&l, "logs · pick-colour (main)");
    assert!(logs > index_of(&l, "╭ Sessions"), "{}", l.join("\n"));
    assert!(l[logs].starts_with("╭"), "{}", l[logs]);
    assert!(!l.iter().any(|x| x.contains("╭ pick-colour")));

    // Closing it brings the detail panel back.
    app.handle_key(ch(' '));
    let l = screen(&mut app, 160, 40);
    assert!(l.iter().any(|x| x.contains("● needs you: input needed")));
}

#[test]
fn the_filter_shows_n_of_m_and_its_input_line() {
    let mut app = app();
    app.handle_key(ch('/'));
    for c in "web".chars() {
        app.handle_key(ch(c));
    }
    let l = screen(&mut app, 100, 30);
    assert!(line_with(&l, "╭ Sessions").contains("2 of 9 shown"));
    assert!(l[27].starts_with(" /web"), "{}", l[27]);
    app.handle_key(key(KeyCode::Enter));
    let l = screen(&mut app, 100, 30);
    assert!(line_with(&l, "╭ Sessions").contains("2 of 9 shown"));
    assert!(l[28].contains("filter \"web\": 2 matches"), "{}", l[28]);
    // The attention counter still counts every highlighted row.
    assert!(l[28].starts_with(" ● 3 need you (filtered)"), "{}", l[28]);
    assert!(!l.iter().any(|x| x.contains("touch-config")));
}

#[test]
fn grouping_by_project_names_the_mode_and_the_projects() {
    let mut app = app();
    app.handle_key(ch('p'));
    let l = screen(&mut app, 160, 40);
    assert!(l[0].contains("by project"), "{}", l[0]);
    assert!(l.iter().any(|x| x.contains("├─▾ Orchestrator 1")));
    assert!(l.iter().any(|x| x.contains("├─▾ ~/workspace/api")));
    assert!(l.iter().any(|x| x.contains("├─▾ Interactive 1")));
}

#[test]
fn without_colour_attention_is_the_marker_and_bold() {
    let mut app = app();
    app.color = false;
    select(&mut app, "pick-colour");
    let buf = render(&mut app, 160, 40);
    let l = lines(&buf);
    for cell in buf.content() {
        assert!(
            cell.fg == Color::Reset && cell.bg == Color::Reset,
            "colour at {:?} {:?}: {:?}",
            cell.symbol(),
            cell.fg,
            cell.bg
        );
    }
    // No share bar without colour.
    assert!(l[2].trim_matches(['│', ' ']).is_empty(), "{}", l[2]);
    let (x, y) = cell_at(&l, "● ▪ touch-config");
    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    let (x, y) = cell_at(&l, "touch-config");
    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "name bold");
    // The selection: `▶` and reversed.
    let sel = line_with(&l, "pick-colour  ");
    assert!(sel.starts_with("│▶● ▪ pick-colour"), "{sel}");
    let (x, y) = cell_at(&l, "▶● ▪ pick-colour");
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
    let (x, y) = cell_at(&l, " ● 3 need you");
    assert!(buf[(x + 1, y)].modifier.contains(Modifier::BOLD));
    // Interactive rows are dimmed.
    let (x, y) = cell_at(&l, "demo-app-3f");
    assert!(buf[(x, y)].modifier.contains(Modifier::DIM));
}

#[test]
fn below_the_minimum_size_the_minimal_layout_is_used_and_said_once() {
    let mut app = app();
    let (w, h) = watchtower::ui::DASHBOARD_MIN_SIZE;
    assert_eq!((w, h), (80, 24));

    let l = screen(&mut app, 80, 24);
    assert!(l[0].starts_with("╭ ◆ tower"), "{}", l[0]);
    assert!(!l.iter().any(|x| x.contains("the dashboard needs")));

    for (w, h) in [(79, 24), (80, 23), (60, 20)] {
        let l = screen(&mut app, w, h);
        assert!(l[0].starts_with(" tower "), "{w}x{h}: {}", l[0]);
        assert!(l.iter().any(|x| x.contains("▾ Needs me (2)  ● 2")));
        assert!(!l.iter().any(|x| x.contains('╭')), "{w}x{h}");
    }
    let l = screen(&mut app, 120, 23);
    let status = &l[21];
    assert!(
        status.contains("the dashboard needs 80x24; minimal layout until the terminal is larger"),
        "{status}"
    );
    // Back to the dashboard when the terminal grows.
    let l = screen(&mut app, 100, 30);
    assert!(l[0].starts_with("╭ ◆ tower"), "{}", l[0]);
    // Only once: a later message is not replaced by the hint.
    select(&mut app, "print-date");
    app.handle_key(ch('m'));
    let l = screen(&mut app, 120, 23);
    assert!(l[21].contains("print-date is not highlighted"), "{}", l[21]);
    assert!(!l[21].contains("the dashboard needs"), "{}", l[21]);
}

#[test]
fn the_minimal_style_is_unchanged_and_never_falls_back() {
    let mut app = app();
    app.style = UiStyle::Minimal;
    let l = screen(&mut app, 160, 40);
    assert!(l[0].starts_with(" tower "), "{}", l[0]);
    assert!(l[0].contains("9 sessions"), "{}", l[0]);
    assert!(!l.iter().any(|x| x.contains('╭')));
    let l = screen(&mut app, 60, 20);
    assert!(!l.iter().any(|x| x.contains("the dashboard needs")));
    // The library's App starts with the minimal renderer; the runtime
    // applies the config's style.
    let fresh = App::new(
        Config::defaults(Path::new(HOME)),
        HOME.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    assert_eq!(fresh.style, UiStyle::Minimal);
}

#[test]
fn a_long_list_scrolls_to_keep_the_selection_in_view() {
    let mut rows = main_rows();
    for k in 0..30 {
        rows.push(bg(
            &format!("{k:08x}"),
            &format!("job-{k:02}"),
            "api",
            "working",
            Some("idle"),
            None,
            600 + k,
        ));
    }
    let mut app = app();
    app.apply_poll(update("main", &rows));
    app.handle_key(key(KeyCode::End));
    let l = screen(&mut app, 100, 30);
    let last = app.selected_session().unwrap().session.display_name();
    assert_eq!(last, "demo-app-3f");
    assert!(
        line_with(&l, "demo-app-3f").starts_with("│▌"),
        "{}",
        l.join("\n")
    );
    app.handle_key(key(KeyCode::Home));
    let l = screen(&mut app, 100, 30);
    assert!(
        l.iter()
            .any(|x| x.contains("├─▾ Orchestrator 1") || x.contains("▾ Orchestrator 1"))
    );
}

#[test]
fn windows_draw_over_the_dashboard() {
    let mut app = app();
    app.handle_key(ch('?'));
    // The help fits 24 rows (doc/0011), over the panels.
    let l = screen(&mut app, 80, 24);
    assert!(line_with(&l, " Keys ").contains('┌'), "{}", l.join("\n"));
    assert!(l.iter().any(|x| x.contains("Esc or ? closes this help")));
    let l = screen(&mut app, 100, 30);
    assert!(l[0].starts_with("╭ ◆ tower"), "{}", l[0]);
    assert!(line_with(&l, " Keys ").contains('┌'));
}

/// What a theme paints: ordinary text, the Needs-me pill and the selected
/// row (doc/0021).
struct Expect {
    theme: UiTheme,
    text: Color,
    pill: (Color, Color),
    sel: (Color, Color),
    /// Pills and the selection in reverse video.
    reversed: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

const EXPECT: [Expect; 4] = [
    Expect {
        theme: UiTheme::Neutral,
        text: rgb(235, 235, 235),
        pill: (rgb(0, 0, 0), rgb(255, 196, 0)),
        sel: (rgb(255, 255, 255), rgb(58, 58, 58)),
        reversed: false,
    },
    Expect {
        theme: UiTheme::Warm,
        text: rgb(236, 230, 221),
        pill: (rgb(0, 0, 0), rgb(240, 185, 75)),
        sel: (rgb(255, 250, 244), rgb(62, 52, 44)),
        reversed: false,
    },
    Expect {
        theme: UiTheme::Terminal,
        text: Color::Reset,
        pill: (Color::LightYellow, Color::Reset),
        sel: (Color::Reset, Color::Reset),
        reversed: true,
    },
    // Today's palette, unchanged (doc/0020).
    Expect {
        theme: UiTheme::TokyoNight,
        text: rgb(192, 202, 245),
        pill: (rgb(26, 27, 38), rgb(224, 175, 104)),
        sel: (rgb(192, 202, 245), rgb(45, 63, 118)),
        reversed: false,
    },
];

#[test]
fn each_theme_paints_text_pills_and_the_selection_in_its_colours() {
    // One app, switched live between the themes as a settings window
    // would (doc/0021).
    let mut app = themed(UiTheme::Neutral);
    select(&mut app, "pick-colour");
    for e in &EXPECT {
        app.set_theme(e.theme);
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        let theme = e.theme.as_str();

        let (x, y) = cell_at(&l, "print-date");
        let name = &buf[(x, y)];
        assert_eq!((name.fg, name.bg), (e.text, Color::Reset), "{theme} text");

        let (x, y) = cell_at(&l, "Needs me 2");
        assert_eq!(y, 1, "{theme}: the header pill");
        let pill = &buf[(x, y)];
        assert_eq!((pill.fg, pill.bg), e.pill, "{theme} pill");
        assert!(pill.modifier.contains(Modifier::BOLD), "{theme}");
        assert_eq!(
            pill.modifier.contains(Modifier::REVERSED),
            e.reversed,
            "{theme} pill"
        );

        let (x, y) = cell_at(&l, "▪ pick-colour");
        let sel = &buf[(x + 2, y)];
        assert_eq!(sel.symbol(), "p");
        assert_eq!((sel.fg, sel.bg), e.sel, "{theme} selection");
        assert_eq!(
            sel.modifier.contains(Modifier::REVERSED),
            e.reversed,
            "{theme} selection"
        );
    }
}

#[test]
fn the_app_starts_with_the_configured_theme() {
    let config = Config::parse("[ui]\ntheme = \"warm\"\n", Path::new(HOME)).unwrap();
    let mut app = App::new(config, HOME.into(), Sidecar::default(), Box::new(NoEnviron));
    assert_eq!(app.theme, UiTheme::Warm);
    app.set_theme(UiTheme::Terminal);
    assert_eq!(app.theme, UiTheme::Terminal);
}

#[test]
fn a_forced_selection_colour_keeps_the_bar_and_the_attention_marker() {
    for (theme, mark, red) in [
        (UiTheme::Neutral, rgb(255, 255, 255), rgb(255, 100, 100)),
        (UiTheme::Warm, rgb(232, 135, 95), rgb(245, 105, 95)),
    ] {
        let mut app = themed(theme);
        select(&mut app, "pick-colour");
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        let (x, y) = cell_at(&l, "▌● ▪ pick-colour");
        assert_eq!(buf[(x, y)].fg, mark, "{theme:?}");
        assert_eq!(buf[(x + 1, y)].symbol(), "●");
        assert_eq!(buf[(x + 1, y)].fg, red, "{theme:?}");
        assert!(buf[(x + 1, y)].modifier.contains(Modifier::BOLD));
    }
}

#[test]
fn the_terminal_theme_uses_only_named_colours() {
    let mut app = themed(UiTheme::Terminal);
    select(&mut app, "pick-colour");
    for (w, h) in [(160, 40), (100, 30), (80, 24)] {
        let buf = render(&mut app, w, h);
        for cell in buf.content() {
            for c in [cell.fg, cell.bg] {
                assert!(
                    !matches!(c, Color::Rgb(..) | Color::Indexed(_)),
                    "{w}x{h}: {:?} in {c:?}",
                    cell.symbol()
                );
            }
        }
    }
    // A group summary and grouping by project too. (The logs panel replays
    // the session's own output, colours included; that is not the theme.)
    select(&mut app, "touch-config");
    app.handle_key(ch('k'));
    app.handle_key(ch('p'));
    let buf = render(&mut app, 160, 40);
    assert!(
        buf.content()
            .iter()
            .all(|c| !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..)))
    );
}

#[test]
fn no_colour_overrides_every_theme() {
    for theme in UiTheme::ALL {
        let mut app = themed(theme);
        app.color = false;
        select(&mut app, "pick-colour");
        for (w, h) in [(160, 40), (100, 30)] {
            let buf = render(&mut app, w, h);
            for cell in buf.content() {
                assert!(
                    cell.fg == Color::Reset && cell.bg == Color::Reset,
                    "{theme:?} {w}x{h}: colour at {:?}",
                    cell.symbol()
                );
            }
            // Attention stays the marker plus bold; the selection `▶`
            // plus reversed (doc/0007).
            let l = lines(&buf);
            let (x, y) = cell_at(&l, "● ▪ touch-config");
            assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
            let (x, y) = cell_at(&l, "▶● ▪ pick-colour");
            assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
        }
    }
}

#[test]
fn only_the_selection_pills_and_chips_have_a_background() {
    for theme in UiTheme::ALL {
        let mut app = themed(theme);
        select(&mut app, "print-date");
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        let sel = index_of(&l, "▪ print-date") as u16;
        // The detail panel's headline pill and account chip.
        let pill = index_of(&l, "╭ print-date") as u16 + 1;
        assert!(
            l[usize::from(pill)].contains("  done "),
            "{}",
            l[usize::from(pill)]
        );
        let account = index_of(&l, "account    main") as u16;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                if cell.bg != Color::Reset {
                    assert!(
                        [1, sel, pill, account].contains(&y),
                        "{theme:?}: background {:?} at {x},{y} {:?}\n{}",
                        cell.bg,
                        cell.symbol(),
                        l[usize::from(y)]
                    );
                }
            }
        }
    }
}

#[test]
fn every_theme_draws_the_same_text() {
    let base = {
        let mut app = themed(UiTheme::TokyoNight);
        select(&mut app, "pick-colour");
        [(160, 40), (100, 30), (80, 24)].map(|(w, h)| screen(&mut app, w, h))
    };
    for theme in UiTheme::ALL {
        let mut app = themed(theme);
        select(&mut app, "pick-colour");
        let screens = [(160, 40), (100, 30), (80, 24)].map(|(w, h)| screen(&mut app, w, h));
        assert_eq!(screens, base, "{theme:?}");
    }
}

#[test]
#[ignore = "prints the screens for a look"]
fn dump() {
    let mut app = app();
    select(&mut app, "pick-colour");
    for (w, h) in [(160, 40), (100, 30), (80, 24), (79, 24)] {
        println!("{w}x{h}\n{}", screen(&mut app, w, h).join("\n"));
    }
}
