//! The light themes, `ansi16` and the colour depth (doc/0038) and the
//! published palettes (doc/0046), rendered with ratatui's `TestBackend`: every
//! new theme draws the dashboard and the main windows at a narrow and a wide
//! size; `ansi16` draws no RGB or indexed colour anywhere; a truecolor theme
//! gives way to `ansi16` on a 16-colour terminal and takes the nearest of 256
//! colours on a 256-colour one; the Settings window cycles and saves the new
//! values, and its pick list fits and scrolls.

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

const PUBLISHED: [UiTheme; 10] = [
    UiTheme::CatppuccinMocha,
    UiTheme::Dracula,
    UiTheme::GruvboxDark,
    UiTheme::Kanagawa,
    UiTheme::Nord,
    UiTheme::OneDark,
    UiTheme::RosePine,
    UiTheme::CatppuccinLatte,
    UiTheme::GruvboxLight,
    UiTheme::OneLight,
];

const NEW_THEMES: [UiTheme; 13] = [
    UiTheme::NeutralLight,
    UiTheme::WarmLight,
    UiTheme::Ansi16,
    UiTheme::CatppuccinMocha,
    UiTheme::Dracula,
    UiTheme::GruvboxDark,
    UiTheme::Kanagawa,
    UiTheme::Nord,
    UiTheme::OneDark,
    UiTheme::RosePine,
    UiTheme::CatppuccinLatte,
    UiTheme::GruvboxLight,
    UiTheme::OneLight,
];

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
    // The theme cycles through every theme and comes back: the light variants
    // and ansi16 after the original four, then the published palettes.
    select(&mut app, "theme");
    let mut seen = Vec::new();
    for _ in 0..UiTheme::ALL.len() {
        press(&mut app, key(KeyCode::Right));
        seen.push(app.theme);
    }
    let mut want: Vec<UiTheme> = UiTheme::ALL[1..].to_vec();
    want.push(UiTheme::Neutral);
    assert_eq!(seen, want);
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

/// WCAG 2 contrast of two drawn colours.
fn contrast(a: Color, b: Color) -> f64 {
    let lum = |c: Color| {
        let Color::Rgb(r, g, b) = c else {
            panic!("{c:?} is not RGB");
        };
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The workspace style fills the whole selected card with the selection's
/// background (doc/0045): in every published palette what is written on it
/// stays readable, at 7:1, or 4.5:1 on an account chip, which keeps its own
/// background; the outline, in the state's colour, at 3:1 (doc/0046).
#[test]
fn a_selected_card_stays_readable_in_every_published_palette() {
    for theme in PUBLISHED {
        let name = "pick-colour";
        let mut app = dashboard(theme);
        app.style = UiStyle::Workspace;
        select_name(&mut app, name);
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        // The card's top edge, in its heavy outline: the pane's title has the
        // name too.
        let y = l
            .iter()
            .position(|r| r.contains(name) && r.contains('━'))
            .unwrap_or_else(|| panic!("{theme:?}: no {name}\n{}", l.join("\n")))
            as u16;
        let bar = (0..buf.area.width)
            .map(|x| buf[(x, y)].bg)
            .find(|c| *c != Color::Reset)
            .unwrap_or_else(|| panic!("{theme:?}: the card is not filled"));
        let mut written = 0;
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            if c.bg == Color::Reset || c.symbol() == " " {
                continue;
            }
            let min = if c.symbol().chars().all(char::is_alphanumeric) {
                if c.bg == bar { 7.0 } else { 4.5 }
            } else {
                3.0
            };
            let r = contrast(c.fg, c.bg);
            assert!(r >= min, "{theme:?} {:?}: {r:.2} < {min}", c.symbol());
            written += 1;
        }
        assert!(written > 0, "{theme:?}");
    }
}

/// `theme` under its pick list at `w` x `h`: the rows, and the cursor's.
fn pick_list(app: &mut App, w: u16, h: u16) -> (Buffer, Vec<String>, usize) {
    let buf = render(app, w, h);
    let l = lines(&buf);
    let cursor = l
        .iter()
        .position(|r| r.contains(&format!("▌ {} ", app.theme.as_str())))
        .unwrap_or_else(|| panic!("{:?}: no cursor\n{}", app.theme, l.join("\n")));
    (buf, l, cursor)
}

/// Every theme is in the pick list at 80x24, its swatch whole; on a shorter
/// screen the list stays on the screen, says what is out of sight, and the
/// cursor can reach every theme in both directions.
#[test]
fn the_theme_pick_list_fits_and_scrolls_with_its_swatches() {
    let (_env, mut app) = settings(UiTheme::Neutral);
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Enter));

    let (buf, l, _) = pick_list(&mut app, 80, 24);
    for theme in UiTheme::ALL {
        let y = l
            .iter()
            .position(|r| r.contains(&format!(" {} ", theme.as_str())))
            .unwrap_or_else(|| panic!("{theme:?} at 80x24\n{}", l.join("\n")));
        let swatch: Vec<Color> = (0..buf.area.width)
            .map(|x| buf[(x, y as u16)].bg)
            .filter(|c| !matches!(c, Color::Reset))
            .collect();
        // Six colours, two columns wide.
        assert!(swatch.len() >= 12, "{theme:?}: {swatch:?}");
    }
    assert!(!l.iter().any(|r| r.contains(" more")), "{}", l.join("\n"));

    for h in [20, 16, 12] {
        // Below 80x24 the window has the minimal look, without swatches
        // (doc/0024). Down through the list, wrapping at the end: the cursor's
        // row is always on the screen ([`pick_list`] finds it).
        for _ in UiTheme::ALL {
            press(&mut app, key(KeyCode::Down));
            pick_list(&mut app, 80, h);
        }
        press(&mut app, key(KeyCode::End));
        let (_, l, _) = pick_list(&mut app, 80, h);
        assert_eq!(app.theme, UiTheme::OneLight);
        assert!(
            l.iter().any(|r| r.contains('↑') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        assert!(
            !l.iter().any(|r| r.contains('↓') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        press(&mut app, key(KeyCode::Home));
        let (_, l, _) = pick_list(&mut app, 80, h);
        assert_eq!(app.theme, UiTheme::Neutral);
        assert!(
            !l.iter().any(|r| r.contains('↑') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        assert!(
            l.iter().any(|r| r.contains('↓') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        // In the middle the list goes on both ways, and one step up moves the
        // cursor's row by at most one: no jumps.
        for _ in 0..8 {
            press(&mut app, key(KeyCode::Down));
        }
        let (_, l, before) = pick_list(&mut app, 80, h);
        assert!(
            l.iter().any(|r| r.contains('↑') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        assert!(
            l.iter().any(|r| r.contains('↓') && r.contains(" more")),
            "{}",
            l.join("\n")
        );
        press(&mut app, key(KeyCode::Up));
        let (_, _, after) = pick_list(&mut app, 80, h);
        assert!(before.abs_diff(after) <= 1, "{before} {after}");
        press(&mut app, key(KeyCode::Home));
    }
}

/// A published palette picked from the list is previewed and saved, and at 16
/// colours gives way to `ansi16` like the others; at 256 it is drawn in the cube.
#[test]
fn a_published_palette_is_picked_saved_and_reduced() {
    for theme in PUBLISHED {
        let (env, mut app) = settings(UiTheme::Neutral);
        select(&mut app, "theme");
        press(&mut app, key(KeyCode::Enter));
        for _ in 0..UiTheme::ALL.len() {
            if app.theme == theme {
                break;
            }
            press(&mut app, key(KeyCode::Down));
        }
        assert_eq!(app.theme, theme, "previewed");
        press(&mut app, key(KeyCode::Enter));
        press(&mut app, ctrl('s'));
        let text = file(&env);
        assert!(
            text.contains(&format!("theme = \"{}\"", theme.as_str())),
            "{text}"
        );
        assert_eq!(app.config().ui_theme, theme);
        press(&mut app, key(KeyCode::Esc));
        assert_eq!(app.theme, theme);
    }

    for theme in PUBLISHED {
        let mut app = dashboard(theme);
        app.detected_depth = ColorDepth::Ansi16;
        assert_eq!(app.drawn_theme(), UiTheme::Ansi16, "{theme:?}");
        let buf = render(&mut app, 80, 24);
        assert!(rich_cells(&buf).is_empty(), "{theme:?} at 16");
        let note = status_text(&app);
        assert!(note.contains(theme.as_str()), "{note}");
        assert!(note.contains("ansi16"), "{note}");

        let mut app = dashboard(theme);
        app.color_depth = ColorDepth::Ansi256;
        assert_eq!(app.drawn_theme(), theme, "{theme:?} stays at 256");
        let buf = render(&mut app, 80, 24);
        let mut indexed = 0;
        for c in buf.content() {
            for colour in [c.fg, c.bg] {
                assert!(!matches!(colour, Color::Rgb(..)), "{theme:?}: {colour:?}");
                if let Color::Indexed(i) = colour {
                    assert!(i >= 16, "{theme:?}: index {i}");
                    indexed += 1;
                }
            }
        }
        assert!(indexed > 0, "{theme:?}");
    }
}

/// Every published palette has its own look, the same text as `neutral`
/// (doc/0021: layout and text are the same in every theme).
#[test]
fn published_palettes_change_colours_only() {
    let text = |theme: UiTheme| lines(&render(&mut dashboard(theme), 160, 40));
    let colours = |theme: UiTheme| {
        render(&mut dashboard(theme), 160, 40)
            .content()
            .iter()
            .map(|c| (c.fg, c.bg))
            .collect::<Vec<_>>()
    };
    let neutral = text(UiTheme::Neutral);
    let mut seen = vec![colours(UiTheme::Neutral)];
    for theme in PUBLISHED {
        assert_eq!(text(theme), neutral, "{theme:?}");
        let drawn = colours(theme);
        assert!(!seen.contains(&drawn), "{theme:?} looks like another");
        seen.push(drawn);
    }
}
