//! The light themes, `ansi16` and the colour depth (doc/0038), rendered
//! with ratatui's `TestBackend`: every new theme draws the dashboard and
//! the main windows at a narrow and a wide size; `ansi16` draws no RGB or
//! indexed colour anywhere; a truecolor theme gives way to `ansi16` on a
//! 16-colour terminal and takes the nearest of 256 colours on a
//! 256-colour one; the Settings window cycles and saves the new values.

mod common;
mod settings_common;

use common::{app_two_with, fixture_path, poll, project_list, select_name, status_text};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use settings_common::{
    CONFIG, Env, app_for, ch, ctrl, env, file, key, lines, open, press, render, select,
};
use watchtower::app::{App, LogsTarget, Severity};
use watchtower::config::{ColorDepth, UiStyle, UiTheme};
use watchtower::config_edit::{SettingKey, SettingValue};
use watchtower::logview;
use watchtower::sidecar::{AccountSource, Sidecar};

const NEW_THEMES: [UiTheme; 3] = [UiTheme::NeutralLight, UiTheme::WarmLight, UiTheme::Ansi16];

/// Narrow (the dashboard's minimum, the strip under the list) and wide
/// (the side panel).
const SIZES: [(u16, u16); 2] = [(80, 24), (160, 40)];

/// Two accounts with the mixed-states listing, its background sessions
/// started by `main`, in `theme`.
fn dashboard(theme: UiTheme) -> App {
    let mut sidecar = Sidecar::default();
    for sid in [
        "3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60",
        "5a7c2e91-0d4b-4c8e-8f21-6b3a9d0e7c12",
        "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93",
        "9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20",
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
    ] {
        sidecar.record_owner(sid, "main", AccountSource::Started);
    }
    let mut app = app_two_with(sidecar);
    app.apply_poll(poll("main", "mixed-states.json"));
    app.style = UiStyle::Dashboard;
    app.set_theme(theme);
    app
}

/// The main windows over the dashboard, by name; the folder holds the
/// dialog's projects while they are drawn.
fn windows(theme: UiTheme) -> (Vec<(&'static str, App)>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut out = vec![("dashboard", dashboard(theme))];

    let mut app = dashboard(theme);
    app.handle_key(ch('?'));
    out.push(("help", app));

    let mut app = dashboard(theme);
    app.handle_key(ch('s'));
    out.push(("stop", app));

    let mut app = dashboard(theme);
    app.open_dialog(project_list(dir.path(), &["api", "web"]));
    out.push(("dialog", app));

    let mut app = dashboard(theme);
    app.handle_key(ch('/'));
    for c in "pi".chars() {
        app.handle_key(ch(c));
    }
    out.push(("filter", app));

    let mut app = dashboard(theme);
    select_name(&mut app, "pick-colour");
    open_logs(&mut app, "question");
    out.push(("logs", app));
    (out, dir)
}

/// Opens the logs panel on the selected row with a recorded screen, which
/// carries Claude Code's own truecolor colours.
fn open_logs(app: &mut App, fixture: &str) {
    app.handle_key(ch(' '));
    let target: LogsTarget = app.logs().unwrap().in_flight().cloned().unwrap();
    let bytes = std::fs::read(fixture_path(&format!("logs/{fixture}.ansi"))).unwrap();
    app.logs_fetched(target, Ok(logview::render(&bytes)));
}

/// The Settings window over a config in `theme`, opened.
fn settings(theme: UiTheme) -> (Env, App) {
    let env = env(Some(CONFIG));
    let mut app = app_for(&env);
    app.set_theme(theme);
    open(&mut app);
    (env, app)
}

/// Every cell whose foreground or background is RGB or indexed.
fn rich_cells(buf: &Buffer) -> Vec<(u16, u16, String, Color, Color)> {
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let rich = |c: Color| matches!(c, Color::Rgb(..) | Color::Indexed(_));
            if rich(c.fg) || rich(c.bg) {
                out.push((x, y, c.symbol().to_owned(), c.fg, c.bg));
            }
        }
    }
    out
}

#[test]
fn every_new_theme_draws_the_dashboard_and_the_windows_at_two_sizes() {
    for theme in NEW_THEMES {
        for (w, h) in SIZES {
            let (all, _dir) = windows(theme);
            for (name, mut app) in all {
                let l = lines(&render(&mut app, w, h));
                // The dashboard (its header shows, or a window over it),
                // with rounded panels and windows only.
                assert!(
                    l[0].starts_with("╭ ◆") || name == "help",
                    "{theme:?} {name} {w}x{h}\n{}",
                    l.join("\n")
                );
                assert!(
                    l.iter().all(|r| !r.contains('┌')),
                    "{theme:?} {name} {w}x{h}: rounded"
                );
            }
            let (_env, mut app) = settings(theme);
            let l = lines(&render(&mut app, w, h));
            assert!(
                l.iter().any(|r| r.contains("colour depth")),
                "{theme:?} settings {w}x{h}\n{}",
                l.join("\n")
            );
            // The theme's pick list, with its swatches.
            select(&mut app, "theme");
            press(&mut app, key(KeyCode::Enter));
            let l = lines(&render(&mut app, w, h));
            assert!(l.iter().any(|r| r.contains("ansi16")), "{theme:?} {w}x{h}");
        }
    }
}

#[test]
fn ansi16_draws_no_rgb_or_indexed_colour() {
    for (w, h) in SIZES {
        let (all, _dir) = windows(UiTheme::Ansi16);
        for (name, mut app) in all {
            if name == "logs" {
                // The replayed screen keeps the session's own colours at
                // truecolor; the next test reduces them at 16.
                continue;
            }
            let buf = render(&mut app, w, h);
            let rich = rich_cells(&buf);
            assert!(rich.is_empty(), "ansi16 {name} {w}x{h}: {:?}", &rich[..1]);
            // Colour is still there: pills and glyphs in named colours.
            assert!(
                buf.content()
                    .iter()
                    .any(|c| c.fg != Color::Reset || c.bg != Color::Reset),
                "ansi16 {name} {w}x{h}: no colour at all"
            );
        }
        let (_env, mut app) = settings(UiTheme::Ansi16);
        let buf = render(&mut app, w, h);
        assert!(rich_cells(&buf).is_empty(), "ansi16 settings {w}x{h}");
    }
    // Dim text and lines are the default colour dimmed: the borders.
    let mut app = dashboard(UiTheme::Ansi16);
    let buf = render(&mut app, 160, 40);
    assert_eq!(buf[(0, 0)].symbol(), "╭");
    assert_eq!(buf[(0, 0)].fg, Color::Reset);
    assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
}

#[test]
fn at_sixteen_colours_nothing_rich_reaches_the_terminal() {
    // `neutral` on a terminal that announces 16 colours: `ansi16`, and the
    // logs panel's replayed colours and the theme swatches are reduced to
    // the named ones too.
    for (w, h) in SIZES {
        let (all, _dir) = windows(UiTheme::Neutral);
        for (name, mut app) in all {
            app.detected_depth = ColorDepth::Ansi16;
            assert_eq!(app.drawn_theme(), UiTheme::Ansi16);
            let buf = render(&mut app, w, h);
            let rich = rich_cells(&buf);
            assert!(rich.is_empty(), "{name} {w}x{h}: {:?}", &rich[..1]);
        }
        let (_env, mut app) = settings(UiTheme::Warm);
        app.detected_depth = ColorDepth::Ansi16;
        select(&mut app, "theme");
        press(&mut app, key(KeyCode::Enter));
        let buf = render(&mut app, w, h);
        assert!(rich_cells(&buf).is_empty(), "settings pick list {w}x{h}");
    }
}

#[test]
fn the_fallback_follows_the_setting_and_the_environment() {
    let mut app = dashboard(UiTheme::Neutral);
    // Tests start at truecolor: the theme as designed.
    assert_eq!(app.color_depth, ColorDepth::Auto);
    assert_eq!(app.detected_depth, ColorDepth::Truecolor);
    assert_eq!(app.drawn_theme(), UiTheme::Neutral);
    let buf = render(&mut app, 160, 40);
    assert!(buf.content().iter().any(|c| matches!(c.fg, Color::Rgb(..))));
    assert!(
        !status_text(&app).contains("ansi16"),
        "no note at truecolor"
    );

    // `auto` on a 16-colour terminal: ansi16, said once in the status line.
    app.detected_depth = ColorDepth::Ansi16;
    let buf = render(&mut app, 160, 40);
    assert!(rich_cells(&buf).is_empty());
    let note = status_text(&app);
    assert!(
        note.contains("theme neutral") && note.contains("ansi16"),
        "{note}"
    );
    assert!(note.contains("COLORTERM and TERM"), "{note}");
    app.set_message("something else", Severity::Info);
    render(&mut app, 160, 40);
    assert!(!status_text(&app).contains("ansi16"), "said once per run");

    // A set depth wins over the environment.
    app.color_depth = ColorDepth::Truecolor;
    assert_eq!(app.drawn_theme(), UiTheme::Neutral);
    let buf = render(&mut app, 160, 40);
    assert!(buf.content().iter().any(|c| matches!(c.fg, Color::Rgb(..))));

    // 256 colours: the theme stays, its colours are palette indices 16 to
    // 255, never RGB and never the terminal's own 0 to 15.
    for (set, detected) in [
        (ColorDepth::Ansi256, ColorDepth::Truecolor),
        (ColorDepth::Ansi256, ColorDepth::Ansi16),
    ] {
        app.color_depth = set;
        app.detected_depth = detected;
        assert_eq!(app.drawn_theme(), UiTheme::Neutral);
        let buf = render(&mut app, 160, 40);
        let mut indexed = 0;
        for c in buf.content() {
            for colour in [c.fg, c.bg] {
                assert!(!matches!(colour, Color::Rgb(..)), "{set:?}: {colour:?}");
                if let Color::Indexed(i) = colour {
                    assert!(i >= 16, "{set:?}: index {i}");
                    indexed += 1;
                }
            }
        }
        assert!(indexed > 0, "{set:?}: the theme's colours as indices");
    }

    // `terminal` already uses named colours: kept at 16.
    let mut app = dashboard(UiTheme::Terminal);
    app.detected_depth = ColorDepth::Ansi16;
    assert_eq!(app.drawn_theme(), UiTheme::Terminal);
    render(&mut app, 160, 40);
    assert!(
        !status_text(&app).contains("ansi16"),
        "terminal does not give way"
    );

    // A set "16" names the setting in the note.
    let mut app = dashboard(UiTheme::Warm);
    app.color_depth = ColorDepth::Ansi16;
    render(&mut app, 160, 40);
    assert!(
        status_text(&app).contains("color_depth is \"16\""),
        "{}",
        status_text(&app)
    );
}

#[test]
fn no_color_keeps_todays_behaviour_at_every_depth() {
    for depth in [
        ColorDepth::Truecolor,
        ColorDepth::Ansi256,
        ColorDepth::Ansi16,
    ] {
        for theme in UiTheme::ALL {
            let mut app = dashboard(theme);
            app.color = false;
            app.detected_depth = depth;
            let buf = render(&mut app, 160, 40);
            assert!(
                buf.content()
                    .iter()
                    .all(|c| c.fg == Color::Reset && c.bg == Color::Reset),
                "{theme:?} at {depth:?}"
            );
            assert!(
                !status_text(&app).contains("ansi16"),
                "{theme:?}: no fallback note"
            );
        }
    }
}

#[test]
fn ansi16_keeps_hues_off_text_and_on_glyphs_and_pills() {
    let mut app = dashboard(UiTheme::Ansi16);
    let buf = render(&mut app, 160, 40);
    let l = lines(&buf);
    // A Working pill in the header: black or white text on a named hue,
    // not bold.
    let y = l.iter().position(|r| r.contains("◆ tower")).unwrap() + 1;
    let pills: Vec<_> = (0..buf.area.width)
        .map(|x| &buf[(x, y as u16)])
        .filter(|c| c.bg != Color::Reset)
        .collect();
    assert!(!pills.is_empty(), "{}", l[y]);
    for c in pills {
        assert!(
            matches!(c.fg, Color::Black | Color::White) || c.modifier.contains(Modifier::REVERSED),
            "{:?} on {:?}",
            c.fg,
            c.bg
        );
    }
    // A row's `▪` takes its state's hue; its state text does not.
    let (y, row) = l
        .iter()
        .enumerate()
        .find(|(_, r)| r.contains("▪ ") && !r.contains("▌") && !r.contains("●"))
        .unwrap();
    let gx = row.chars().position(|c| c == '▪').unwrap() as u16;
    let glyph = &buf[(gx, y as u16)];
    assert!(
        !matches!(glyph.fg, Color::Reset),
        "the glyph in a hue: {row}"
    );
    // Up to the list panel's border.
    let mut x = gx + 2;
    while buf[(x, y as u16)].symbol() != "│" {
        let c = &buf[(x, y as u16)];
        x += 1;
        assert!(
            matches!(c.fg, Color::Reset) || c.symbol() == " ",
            "plain text at {x}: {:?} {:?} in {row}",
            c.symbol(),
            c.fg
        );
    }
}

/// The Settings window offers the new themes and the colour depth,
/// previews them and saves them.
#[test]
fn settings_cycles_previews_and_saves_the_new_values() {
    let (env, mut app) = settings(UiTheme::Neutral);
    // The theme cycles through the light variants and ansi16 after the
    // original four.
    select(&mut app, "theme");
    let mut seen = Vec::new();
    for _ in 0..7 {
        press(&mut app, key(KeyCode::Right));
        seen.push(app.theme);
    }
    assert_eq!(
        seen,
        [
            UiTheme::Warm,
            UiTheme::Terminal,
            UiTheme::TokyoNight,
            UiTheme::NeutralLight,
            UiTheme::WarmLight,
            UiTheme::Ansi16,
            UiTheme::Neutral,
        ]
    );
    for _ in 0..4 {
        press(&mut app, key(KeyCode::Right));
    }
    assert_eq!(app.theme, UiTheme::NeutralLight, "previewed");

    // The colour depth: auto, truecolor, 256, 16, previewed at once.
    select(&mut app, "colour depth");
    let row = |app: &mut App| -> String {
        let l = lines(&render(app, 160, 40));
        l.iter()
            .find(|r| r.starts_with('│') && r.contains("colour depth  "))
            .unwrap()
            .clone()
    };
    assert!(row(&mut app).contains("auto"), "{}", row(&mut app));
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.color_depth, ColorDepth::Truecolor);
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.color_depth, ColorDepth::Ansi256);
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.color_depth, ColorDepth::Ansi16);
    assert_eq!(app.drawn_theme(), UiTheme::Ansi16, "previewed at 16");
    assert!(rich_cells(&render(&mut app, 160, 40)).is_empty());
    press(&mut app, key(KeyCode::Left));
    assert_eq!(app.color_depth, ColorDepth::Ansi256);

    // The pick list says what auto means here.
    press(&mut app, key(KeyCode::Enter));
    let l = lines(&render(&mut app, 160, 40));
    assert!(
        l.iter()
            .any(|r| r.contains("from COLORTERM and TERM: full colour here")),
        "{}",
        l.join("\n")
    );
    assert!(l.iter().any(|r| r.contains("colour depth 256")), "header");
    press(&mut app, key(KeyCode::Enter));

    // Saved with the window kept open, and applied.
    press(&mut app, ctrl('s'));
    let text = file(&env);
    assert!(text.contains("theme = \"neutral-light\""), "{text}");
    assert!(text.contains("color_depth = \"256\""), "{text}");
    assert!(text.contains("# Watchtower config for the Settings tests."));
    assert_eq!(app.config().ui_theme, UiTheme::NeutralLight);
    assert_eq!(app.config().ui_color_depth, ColorDepth::Ansi256);
    let window = app.settings().expect("still open");
    assert!(!window.session.is_modified(SettingKey::UiColorDepth));
    assert_eq!(
        window.session.saved(SettingKey::UiColorDepth),
        Some(SettingValue::Text("256".into()))
    );

    // Closing keeps the saved look; Del then a save removes the key.
    select(&mut app, "colour depth");
    press(&mut app, key(KeyCode::Delete));
    assert_eq!(app.color_depth, ColorDepth::Auto, "the default previewed");
    press(&mut app, ctrl('s'));
    assert!(!file(&env).contains("color_depth"), "{}", file(&env));
    press(&mut app, key(KeyCode::Esc));
    assert!(app.settings().is_none());
    assert_eq!(app.color_depth, ColorDepth::Auto);
    assert_eq!(app.theme, UiTheme::NeutralLight);
}

/// Closing without saving puts the running depth back (doc/0024's
/// preview rule).
#[test]
fn a_discarded_colour_depth_reverts() {
    let (env, mut app) = settings(UiTheme::Neutral);
    select(&mut app, "colour depth");
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.color_depth, ColorDepth::Ansi16);
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ch('n'));
    assert!(app.settings().is_none());
    assert_eq!(app.color_depth, ColorDepth::Auto);
    assert_eq!(app.drawn_theme(), UiTheme::Neutral);
    assert_eq!(file(&env), CONFIG);
}
