//! The windows over the screen (doc/0020, doc/0021) rendered with
//! ratatui's `TestBackend`: in the dashboard style every overlay follows
//! the theme (rounded borders, title colour, selection, errors, warnings),
//! `NO_COLOR` drops every colour, and every theme draws the same text.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{HOME, app_single, app_two, app_two_with, ch, ctrl, key, poll, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use watchtower::actions::RemoveError;
use watchtower::app::{App, Effect, SessionAction};
use watchtower::config::{UiStyle, UiTheme};
use watchtower::dispatch::DispatchError;
use watchtower::handoff::{HandoffError, Step};
use watchtower::listing::parse_listing;
use watchtower::orchestrator::StartError;
use watchtower::poller::PollUpdate;
use watchtower::projects::{Project, ProjectList};
use watchtower::sidecar::{AccountSource, Sidecar};

/// The wall clock the screens are drawn at (Unix ms), so the spinner and
/// the ages do not move between renders.
const NOW_MS: i64 = 1_800_000_000_000;
const W: u16 = 120;
const H: u16 = 40;

/// Projects shown as `~/workspace/<name>`, in folders under `root`.
fn projects(root: &Path, names: &[&str]) -> ProjectList {
    ProjectList {
        projects: names
            .iter()
            .map(|n| {
                let path = root.join(n);
                std::fs::create_dir_all(&path).unwrap();
                Project {
                    canonical: path.clone(),
                    display: format!("~/workspace/{n}"),
                    path,
                    last_used_ms: None,
                    extra: false,
                }
            })
            .collect(),
        warnings: Vec::new(),
    }
}

fn empty_poll(account: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing("[]").unwrap()),
    }
}

/// Two accounts that list the sessions of `after-stop-all.json`, all
/// started by `main`.
fn app_two_owned() -> App {
    let mut sc = Sidecar::default();
    for sid in [
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42",
        "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93",
    ] {
        sc.record_owner(sid, "main", AccountSource::Started);
    }
    let mut app = app_two_with(sc);
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    app
}

/// The hand-off of `print-date` to `alt`, confirmed.
fn handoff_running() -> (App, watchtower::app::HandoffJob) {
    let mut app = app_two_owned();
    common::select_name(&mut app, "print-date");
    app.handle_key(ch('a'));
    let job = app
        .handle_key(ch('y'))
        .into_iter()
        .find_map(|e| match e {
            Effect::Handoff(job) => Some(job),
            _ => None,
        })
        .expect("the hand-off starts");
    (app, job)
}

/// The new-session dialog over three projects in folders under `root`.
fn dialog_app(root: &Path) -> App {
    let mut app = app_two();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.open_dialog(projects(root, &["api", "demo-app", "web"]));
    app
}

/// Every window, by name, in the minimal style with colour; the folder
/// holds the projects while they are drawn.
fn scenarios() -> (Vec<(&'static str, App)>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut out = Vec::new();

    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(ch('?'));
    out.push(("help", app));

    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Down));
    out.push(("picker", app));

    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(ch('s'));
    out.push(("stop", app));

    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(ch('d'));
    out.push(("remove", app));

    let mut app = app_two_owned();
    common::select_name(&mut app, "print-date");
    app.handle_key(ch('a'));
    out.push(("handoff", app));

    let (mut app, job) = handoff_running();
    app.handoff_finished(
        job,
        Err(HandoffError::Untrusted {
            dir: PathBuf::from(format!("{HOME}/workspace/demo-app")),
            message: "Workspace not trusted. Run `claude` there once and accept the trust \
                      prompt, then retry."
                .into(),
        }),
    );
    out.push(("handoff-untrusted", app));

    let (mut app, _job) = handoff_running();
    app.handoff_step(Step::WaitingForStop);
    app.handle_key(ch('q'));
    out.push(("quit", app));

    let mut sc = Sidecar::default();
    sc.record_owner(
        "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93",
        "main",
        AccountSource::Started,
    );
    let config = watchtower::config::Config::parse(
        r#"
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
"#,
        Path::new(HOME),
    )
    .unwrap();
    let mut app = App::new(config, HOME.into(), sc, Box::new(common::NoEnviron));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(poll(a, "after-stop-all.json"));
    }
    common::select_name(&mut app, "print-date");
    app.handle_key(ch('a'));
    out.push(("target", app));

    let mut app = app_two();
    app.apply_poll(empty_poll("main"));
    app.apply_poll(empty_poll("alt"));
    let effects = app.handle_key(ch('o'));
    let [Effect::StartOrchestrator(job)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    app.orchestrator_finished(
        job.clone(),
        Err(StartError::Dispatch(DispatchError::Untrusted {
            dir: "/x".into(),
            message: "Workspace not trusted. Run claude there first.".into(),
        })),
    );
    out.push(("orchestrator-untrusted", app));

    let mut app = app_single();
    let action = SessionAction {
        account: app.config().accounts[0].clone(),
        id: "b2f8d064".into(),
        name: "say-ok".into(),
    };
    app.remove_finished(
        action,
        Err(RemoveError::Refused {
            code: Some(1),
            message: "refusal line 1\nrefusal line 2".into(),
        }),
    );
    out.push(("popup", app));

    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.handle_key(ch('/'));
    type_text(&mut app, "pi");
    out.push(("filter", app));

    out.push(("dialog", dialog_app(root)));

    let mut app = dialog_app(root);
    type_text(&mut app, "dem");
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "first line");
    out.push(("dialog-task", app));

    let mut app = dialog_app(root);
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Tab));
    }
    while app.dialog().unwrap().model.text() != "haiku" {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(ctrl('s'));
    out.push(("dialog-haiku-error", app));

    let mut app = dialog_app(root);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "do the thing");
    app.handle_key(key(KeyCode::Esc));
    out.push(("dialog-discard", app));

    let mut app = dialog_app(root);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "do the thing");
    let effects = app.handle_key(ctrl('s'));
    let job = effects.iter().find_map(|e| match e {
        Effect::Dispatch(job) => Some(job.clone()),
        _ => None,
    });
    out.push(("dialog-starting", app));
    if let Some(job) = job {
        let mut app = dialog_app(root);
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "do the thing");
        app.handle_key(ctrl('s'));
        app.dispatch_finished(
            job,
            Err(DispatchError::Untrusted {
                dir: PathBuf::from(format!("{HOME}/workspace/api")),
                message: "Workspace not trusted. Run `claude` there once.".into(),
            }),
        );
        out.push(("dialog-untrusted", app));
    }

    let mut app = app_two();
    app.apply_poll(poll("main", "mixed-states.json"));
    let effects = app.handle_key(ch('R'));
    let [Effect::ScanProjects(scan)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    app.projects_found(scan.id, projects(root, &["web", "api", "demo-app"]));
    out.push(("resume", app));

    // The rename window of `e` (doc/0027): a running row, a stopped row
    // with an error, and the trust window of a stopped row's rename.
    let mut app = app_two_owned();
    common::select_name(&mut app, "print-date");
    app.handle_key(ch('e'));
    type_text(&mut app, "-v2");
    out.push(("rename", app));

    let mut app = app_two_owned();
    common::select_name(&mut app, "say-ok");
    app.handle_key(ch('e'));
    app.handle_key(ctrl('u'));
    app.handle_key(key(KeyCode::Enter));
    out.push(("rename-stopped", app));

    let mut app = app_two_owned();
    common::select_name(&mut app, "say-ok");
    app.handle_key(ch('e'));
    type_text(&mut app, "-v2");
    let effects = app.handle_key(key(KeyCode::Enter));
    let [Effect::Rename(job)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    app.rename_finished(
        job.clone(),
        Err(watchtower::rename::RenameError::Untrusted {
            dir: PathBuf::from(format!("{HOME}/workspace/demo-app")),
            message: "Workspace not trusted. Run `claude` there once.".into(),
        }),
    );
    out.push(("rename-untrusted", app));

    (out, dir)
}

fn render(app: &mut App, w: u16, h: u16) -> (Buffer, (u16, u16)) {
    render_at(app, w, h, Instant::now())
}

/// As [`render`], drawn as at `at`.
fn render_at(app: &mut App, w: u16, h: u16, at: Instant) -> (Buffer, (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, at, NOW_MS))
        .unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    (terminal.backend().buffer().clone(), (pos.x, pos.y))
}

/// The text of every row.
fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

/// The cell where `needle` starts on the last row containing it (windows
/// are drawn last, over the screen).
fn cell_at(buf: &Buffer, needle: &str) -> (u16, u16) {
    let l = lines(buf);
    let y = l
        .iter()
        .rposition(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in\n{}", l.join("\n")));
    let x = unicode_width::UnicodeWidthStr::width(&l[y][..l[y].rfind(needle).unwrap()]);
    (x as u16, y as u16)
}

/// The scenario `name`, in `style` and `theme`, with or without colour.
fn window(name: &str, style: UiStyle, theme: UiTheme, color: bool) -> (Buffer, tempfile::TempDir) {
    let (all, dir) = scenarios();
    let (_, mut app) = all
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    app.style = style;
    app.set_theme(theme);
    app.color = color;
    (render(&mut app, W, H).0, dir)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// What a theme's windows are drawn with (doc/0021).
struct Expect {
    theme: UiTheme,
    title: Color,
    line: Color,
    /// The selection bar's text and background; reversed in `terminal`.
    sel: (Color, Color),
    mark: Color,
    red: Color,
    amber: Color,
    key: Color,
}

const EXPECT: [Expect; 4] = [
    Expect {
        theme: UiTheme::Neutral,
        title: rgb(235, 235, 235),
        line: rgb(110, 110, 110),
        sel: (rgb(255, 255, 255), rgb(58, 58, 58)),
        mark: rgb(255, 255, 255),
        red: rgb(255, 100, 100),
        amber: rgb(255, 196, 0),
        key: rgb(235, 235, 235),
    },
    Expect {
        theme: UiTheme::Warm,
        title: rgb(232, 135, 95),
        line: rgb(118, 108, 97),
        sel: (rgb(255, 250, 244), rgb(62, 52, 44)),
        mark: rgb(232, 135, 95),
        red: rgb(245, 105, 95),
        amber: rgb(240, 185, 75),
        key: rgb(232, 135, 95),
    },
    Expect {
        theme: UiTheme::Terminal,
        title: Color::Reset,
        line: Color::DarkGray,
        sel: (Color::Reset, Color::Reset),
        mark: Color::Reset,
        red: Color::LightRed,
        amber: Color::LightYellow,
        key: Color::Reset,
    },
    Expect {
        theme: UiTheme::TokyoNight,
        title: rgb(192, 202, 245),
        line: rgb(59, 66, 97),
        sel: (rgb(192, 202, 245), rgb(45, 63, 118)),
        mark: rgb(122, 162, 247),
        red: rgb(247, 118, 142),
        amber: rgb(224, 175, 104),
        key: rgb(122, 162, 247),
    },
];

/// Windows and their titles.
const TITLED: [(&str, &str); 14] = [
    ("dialog", "New session"),
    ("resume", "Resume a past conversation"),
    ("stop", "Stop session?"),
    ("remove", "Remove session?"),
    ("handoff", "Continue under another account?"),
    ("picker", "Which account owns"),
    ("target", "Continue print-date under which account?"),
    ("help", "Keys"),
    ("popup", "say-ok"),
    ("quit", "Quit tower?"),
    ("orchestrator-untrusted", "Not trusted by"),
    ("rename", "Rename session"),
    ("rename-stopped", "Rename session"),
    ("rename-untrusted", "Not trusted by"),
];

#[test]
fn every_window_has_rounded_borders_and_a_title_in_the_theme() {
    for e in &EXPECT {
        let theme = e.theme;
        for (name, title) in TITLED {
            let (buf, _dir) = window(name, UiStyle::Dashboard, theme, true);
            let (x, y) = cell_at(&buf, &format!("╭ {title}"));
            let corner = &buf[(x, y)];
            assert_eq!(corner.fg, e.line, "{theme:?} {name} border");
            let t = &buf[(x + 2, y)];
            assert_eq!(t.fg, e.title, "{theme:?} {name} title");
            assert!(t.modifier.contains(Modifier::BOLD), "{theme:?} {name}");
            assert!(
                lines(&buf).iter().all(|l| !l.contains('┌')),
                "{theme:?} {name}: no square corner"
            );
        }
    }
}

/// The selection bar: the theme's colours, reversed only in `terminal`.
fn assert_selected(buf: &Buffer, needle: &str, e: &Expect, what: &str) {
    let theme = e.theme;
    let (x, y) = cell_at(buf, needle);
    let marker = &buf[(x, y)];
    assert_eq!(marker.symbol(), "▶", "{theme:?} {what}");
    let item = &buf[(x + 2, y)];
    let reversed = theme == UiTheme::Terminal;
    assert_eq!(
        item.modifier.contains(Modifier::REVERSED),
        reversed,
        "{theme:?} {what}"
    );
    assert_eq!((item.fg, item.bg), e.sel, "{theme:?} {what} selection");
    assert_eq!(marker.bg, e.sel.1, "{theme:?} {what} marker on the bar");
    assert_eq!(marker.fg, e.mark, "{theme:?} {what} marker");
    if !reversed {
        // Never the default white on reverse.
        assert_ne!(item.bg, Color::Reset, "{theme:?} {what}");
    }
}

#[test]
fn selected_items_use_the_themes_selection_bar() {
    for e in &EXPECT {
        let (buf, _dir) = window("dialog", UiStyle::Dashboard, e.theme, true);
        assert_selected(&buf, "▶ ~/workspace/api", e, "project");
        let (buf, _dir) = window("resume", UiStyle::Dashboard, e.theme, true);
        assert_selected(&buf, "▶ ~/workspace/web", e, "resume project");
        let (buf, _dir) = window("picker", UiStyle::Dashboard, e.theme, true);
        assert_selected(&buf, "▶ alt", e, "account");
        let (buf, _dir) = window("target", UiStyle::Dashboard, e.theme, true);
        assert_selected(&buf, "▶ alt", e, "target");
    }
}

#[test]
fn errors_are_red_and_warnings_amber() {
    for e in &EXPECT {
        let theme = e.theme;
        let (buf, _dir) = window("dialog-haiku-error", UiStyle::Dashboard, theme, true);
        let (x, y) = cell_at(&buf, "the task must be");
        assert_eq!(buf[(x, y)].fg, e.red, "{theme:?} error");
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
        let (x, y) = cell_at(&buf, "⚠ auto mode");
        assert_eq!(buf[(x, y)].fg, e.amber, "{theme:?} warning");
        let (buf, _dir) = window("rename-stopped", UiStyle::Dashboard, theme, true);
        let (x, y) = cell_at(&buf, "the name must not be empty");
        assert_eq!(buf[(x, y)].fg, e.red, "{theme:?} rename error");
    }
}

#[test]
fn labels_hints_and_keys_take_their_roles() {
    for e in &EXPECT {
        let theme = e.theme;
        let (buf, _dir) = window("dialog", UiStyle::Dashboard, theme, true);
        // The focused label in the key colour, the others secondary.
        let (x, y) = cell_at(&buf, "▶ Project");
        assert_eq!(buf[(x + 2, y)].fg, e.key, "{theme:?} focused label");
        let (x, y) = cell_at(&buf, "  Name ");
        assert_ne!(buf[(x + 2, y)].fg, e.key, "{theme:?} label");
        // Keys of the footer in the key colour, the rest dim.
        let (x, y) = cell_at(&buf, "Ctrl+S start");
        assert_eq!(buf[(x, y)].fg, e.key, "{theme:?} key");
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
        assert_ne!(buf[(x + 7, y)].fg, e.key, "{theme:?} hint");

        // The hand-off's From/To labels are secondary, the values text.
        let (buf, _dir) = window("handoff", UiStyle::Dashboard, theme, true);
        let (x, y) = cell_at(&buf, "From: main");
        let (label, value) = (&buf[(x, y)], &buf[(x + 6, y)]);
        if theme != UiTheme::Terminal {
            assert_ne!(label.fg, value.fg, "{theme:?} From:");
        }
        let (x, y) = cell_at(&buf, "n/Esc no");
        assert_eq!(buf[(x, y)].fg, e.key, "{theme:?} answer key");

        // The trust window's `Ctrl+T:` is a key.
        let (buf, _dir) = window("orchestrator-untrusted", UiStyle::Dashboard, theme, true);
        let (x, y) = cell_at(&buf, "Ctrl+T: run");
        assert_eq!(buf[(x, y)].fg, e.key, "{theme:?} Ctrl+T");
    }
}

#[test]
fn the_filter_line_and_the_cursor_in_the_dashboard() {
    for e in &EXPECT {
        let mut app = scenarios()
            .0
            .into_iter()
            .find(|(n, _)| *n == "filter")
            .unwrap()
            .1;
        app.style = UiStyle::Dashboard;
        app.set_theme(e.theme);
        let (buf, cursor) = render(&mut app, W, H);
        let (x, y) = cell_at(&buf, " /pi");
        assert_eq!(buf[(x + 1, y)].fg, e.key, "{:?} /", e.theme);
        assert_eq!(cursor, (x + 4, y), "{:?} cursor after the text", e.theme);
        let (x, y) = cell_at(&buf, "Enter keep");
        assert_eq!(buf[(x, y)].fg, e.key, "{:?}", e.theme);
    }
    let (all, _dir) = scenarios();
    for (name, mut app) in all {
        if !name.starts_with("dialog") || name.contains("discard") || name.contains("starting") {
            continue;
        }
        let (_, minimal) = render(&mut app, W, H);
        app.style = UiStyle::Dashboard;
        let (_, themed) = render(&mut app, W, H);
        assert_eq!(themed, minimal, "{name}: the cursor stays where it was");
    }
}

#[test]
fn without_colour_no_window_has_colour() {
    for theme in UiTheme::ALL {
        let (all, _dir) = scenarios();
        for (name, mut app) in all {
            app.style = UiStyle::Dashboard;
            app.set_theme(theme);
            app.color = false;
            let buf = render(&mut app, W, H).0;
            for cell in buf.content() {
                assert!(
                    cell.fg == Color::Reset && cell.bg == Color::Reset,
                    "{theme:?} {name}: colour at {:?}",
                    cell.symbol()
                );
            }
        }
        // The selection stays visible: `▶` plus reversed.
        for (name, needle) in [
            ("dialog", "▶ ~/workspace/api"),
            ("picker", "▶ alt"),
            ("target", "▶ alt"),
        ] {
            let (buf, _dir) = window(name, UiStyle::Dashboard, theme, false);
            let (x, y) = cell_at(&buf, needle);
            assert!(
                buf[(x + 2, y)].modifier.contains(Modifier::REVERSED),
                "{theme:?} {name}"
            );
        }
        let (buf, _dir) = window("dialog-haiku-error", UiStyle::Dashboard, theme, false);
        let (x, y) = cell_at(&buf, "the task must be");
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "{theme:?}");
    }
}

#[test]
fn every_theme_draws_the_same_windows() {
    // Every scenario is drawn as at one instant taken before any of them is
    // built, so the times the screen shows ("refreshed 0s ago", messages
    // still on) are the same for every theme however slow the machine is.
    let at = Instant::now();
    let text = |theme: UiTheme| -> Vec<Vec<String>> {
        let (all, _dir) = scenarios();
        all.into_iter()
            .map(|(_, mut app)| {
                app.style = UiStyle::Dashboard;
                app.set_theme(theme);
                lines(&render_at(&mut app, W, H, at).0)
            })
            .collect()
    };
    let base = text(UiTheme::Neutral);
    for theme in UiTheme::ALL {
        assert_eq!(text(theme), base, "{theme:?}");
    }
}

#[test]
fn neutral_and_warm_paint_only_their_selection_behind_window_text() {
    for theme in [UiTheme::Neutral, UiTheme::Warm] {
        let e = EXPECT.iter().find(|e| e.theme == theme).unwrap();
        for (name, title) in [
            ("dialog", "New session"),
            ("dialog-discard", "New session"),
            ("picker", "Which account owns"),
            ("target", "Continue print-date"),
            ("resume", "Resume a past"),
        ] {
            let (buf, _dir) = window(name, UiStyle::Dashboard, theme, true);
            // Inside the window: from its top-left corner to the bottom
            // one below it.
            let (x0, y0) = cell_at(&buf, &format!("╭ {title}"));
            let y1 = (y0..buf.area.height)
                .find(|&y| buf[(x0, y)].symbol() == "╰")
                .unwrap();
            let x1 = (x0 + 1..buf.area.width)
                .find(|&x| buf[(x, y0)].symbol() == "╮")
                .unwrap();
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let cell = &buf[(x, y)];
                    if cell.bg != Color::Reset {
                        assert_eq!(
                            cell.bg,
                            e.sel.1,
                            "{theme:?} {name}: background at {x},{y} {:?}",
                            cell.symbol()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn the_minimal_style_keeps_its_windows() {
    let (all, _dir) = scenarios();
    for (name, mut app) in all {
        let buf = render(&mut app, W, H).0;
        let l = lines(&buf);
        assert!(l.iter().all(|r| !r.contains('╭')), "{name}");
        for cell in buf.content() {
            for c in [cell.fg, cell.bg] {
                assert!(!matches!(c, Color::Rgb(..)), "{name}: {c:?}");
            }
        }
    }
    let (buf, _dir) = window("dialog", UiStyle::Minimal, UiTheme::Warm, true);
    let (x, y) = cell_at(&buf, "▶ ~/workspace/api");
    assert!(buf[(x + 2, y)].modifier.contains(Modifier::REVERSED));
    assert_eq!(buf[(x + 2, y)].bg, Color::Reset);
}

#[test]
fn below_the_dashboards_size_windows_keep_the_minimal_look() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app.style = UiStyle::Dashboard;
    app.set_theme(UiTheme::Warm);
    app.handle_key(ch('?'));
    let l = lines(&render(&mut app, 79, 24).0);
    assert!(l.iter().any(|r| r.contains("┌ Keys ")), "{}", l.join("\n"));
    let l = lines(&render(&mut app, 80, 24).0);
    assert!(l.iter().any(|r| r.contains("╭ Keys ")), "{}", l.join("\n"));
}
