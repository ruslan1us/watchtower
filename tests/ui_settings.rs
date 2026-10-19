//! The Settings window rendered with ratatui's `TestBackend` (doc/0024):
//! the owner-approved mock's screens at 160×40 and 100×30, in the
//! dashboard style's themes (`neutral`, `warm`) and the minimal style, and
//! without colour under `NO_COLOR`.

mod common;
mod settings_common;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;
use settings_common::*;
use watchtower::app::App;
use watchtower::config::{UiStyle, UiTheme};

fn opened_with(theme: UiTheme, style: UiStyle) -> (Env, App) {
    let env = env(Some(CONFIG));
    let mut app = app_for(&env);
    app.style = style;
    app.set_theme(theme);
    open(&mut app);
    (env, app)
}

/// Every needle appears on some line.
fn has_all(lines: &[String], needles: &[&str]) {
    for needle in needles {
        line_with(lines, needle);
    }
}

/// Screen 1 of the mock: the overview with one unsaved change.
fn overview(app: &mut App) {
    select(app, "default effort");
    for _ in 0..3 {
        press(app, key(KeyCode::Right));
    }
    select(app, "style");
}

#[test]
fn overview_at_160x40_matches_the_mock() {
    for theme in [UiTheme::Neutral, UiTheme::Warm] {
        let (_env, mut app) = opened_with(theme, UiStyle::Dashboard);
        overview(&mut app);
        let s = screen(&mut app, 160, 40);
        assert!(s[0].starts_with("╭ ◆ tower ─"), "{}", s[0]);
        assert!(s[0].ends_with("─ settings ╮"), "{}", s[0]);
        has_all(
            &s,
            &[
                "│ Settings   ~/.config/watchtower/config.toml  (found)",
                "* 1 unsaved change │",
                "╭ Settings ─",
                "─ 12 settings · 2 accounts ╮╭ style ─",
                "├─ Appearance ─",
                "│▌   style                        ‹ dashboard ›  default",
                "Enter pick   ←→ change │",
                "│    theme                        neutral  default",
                "├─ New sessions ─",
                "│    default account              main  (work)  default",
                "│ *  default effort               high",
                "│    default permission mode      auto  default",
                "│    project roots                ~/workspace, ~/old-projects  2 folders ▸",
                "│    extra projects               ~/notes  1 folder ▸",
                "│    hand-off prompt              \"continue\"  default",
                "│    allow cross-group hand-off   [ ] off  default",
                "│    poll interval                2 s  default",
                "│    track subagents              [x] on  default",
                "├─ Accounts  read-only ─",
                "│    main    claude       ~/.claude       work        default account",
                "│    alt     claude-alt   ~/.claude-alt   personal",
                "a wrong command or config dir can wake sessions",
                "key       [ui] style",
                "values    dashboard · minimal",
                "default   \"dashboard\"",
                "in file   \"dashboard\"",
                "applies   at once",
                "Enter pick   ←→ change   Del default",
            ],
        );
        assert_eq!(
            s[38],
            " Ctrl+S saves to ~/.config/watchtower/config.toml · only changed keys are \
             rewritten; comments and formatting are kept"
        );
        assert!(s[39].starts_with(" ↑↓ move   Enter edit   ←→ change   Ctrl+S save   Esc close"));
        assert!(s[39].ends_with("? help"), "{}", s[39]);
    }
}

#[test]
fn overview_at_100x30_has_the_help_strip() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    overview(&mut app);
    let s = screen(&mut app, 100, 30);
    has_all(
        &s,
        &[
            "│ Settings   ~/.config/watchtower/config.toml  (found)",
            "* 1 unsaved change │",
            "─ 12 settings · 2 accounts ╮",
            "│▌   style                       ‹ dashboard ›  default",
            "│ *  default effort              high",
            "│    main    claude       ~/.claude       work        default account",
            "│ style  The look of the session list",
        ],
    );
    assert!(
        !s.iter().any(|l| l.contains("key       [ui] style")),
        "no detail panel"
    );
    assert_eq!(
        s[28],
        " Ctrl+S saves to ~/.config/watchtower/config.toml · comments and formatting are kept"
    );
    assert!(s[29].ends_with("? help"));
}

#[test]
fn the_theme_pick_previews_the_whole_window() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    select(&mut app, "theme");
    let neutral = render(&mut app, 160, 40);
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, key(KeyCode::Down));
    let buf = render(&mut app, 160, 40);
    let s = lines(&buf);
    has_all(
        &s,
        &[
            "previewing theme warm   no unsaved changes │",
            "│▌*  theme                        ‹ warm ›  preview",
            "╭ theme ─",
            "neutral                    greys; colour only for states         saved│",
            "│▌ warm                       warm greys and a clay accent",
            "terminal                   your terminal's 16 colours",
            "tokyo-night                tower 0.2's blue-grey palette",
            "╰ ↑↓ preview   Enter keep   Esc revert to neutral ─",
        ],
    );
    assert!(s[39].starts_with(" ↑↓ preview   Enter keep   Esc revert"));
    // Drawn in warm already: the borders take its line colour.
    assert_ne!(
        neutral[(0, 0)].fg,
        buf[(0, 0)].fg,
        "the border colour follows the preview"
    );
}

#[test]
fn the_list_editor_checks_each_folder() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ch('a'));
    type_in(&mut app, "~/workspace/api");
    for (w, h) in [(160, 40), (100, 30)] {
        let s = screen(&mut app, w, h);
        has_all(
            &s,
            &[
                "╭ project roots ─",
                "─ 2 folders + 1 being added ╮",
                "Folders scanned one level deep for git clones in the new-session dialog.",
                "    ~/workspace             ✓ 3 git clones",
                "    ~/old-projects          not found; skipped when scanning",
                "│▌* + [ ~/workspace/api",
                "]  no git clones in it",
                "~ is kept as typed and expanded when scanning. Duplicates are refused.",
                "Changes stay unsaved until Ctrl+S in the Settings window.",
                "╰ a add   Enter edit   d remove   Esc back to settings ─",
            ],
        );
        assert!(
            s[usize::from(h) - 1]
                .starts_with(" Enter keep   Tab complete folder   Ctrl+U clear   Esc cancel input")
        );
    }
}

#[test]
fn an_invalid_number_turns_the_field_red() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    select(&mut app, "poll interval");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "0");
    let buf = render(&mut app, 160, 40);
    let s = lines(&buf);
    let row = line_with(&s, "│▌   poll interval                [ 0");
    assert!(row.contains("]  ✗ must be 1 to 60 seconds"), "{row}");
    assert!(s[39].starts_with(" Enter keep   Esc undo edit   ←→ cursor   Ctrl+U clear"));
    // The field's bracket and the message are red.
    let y = s.iter().position(|l| l.contains("[ 0")).unwrap() as u16;
    let x = s[usize::from(y)].chars().position(|c| c == '✗').unwrap() as u16;
    assert!(
        matches!(buf[(x, y)].fg, Color::Rgb(r, _, _) if r > 200),
        "{:?}",
        buf[(x, y)].fg
    );
}

/// Screens 5 and 6: three changes, `Esc` asks; then `Ctrl+S` saves.
#[test]
fn the_unsaved_question_and_the_saved_line() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    overview(&mut app);
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ch('a'));
    type_in(&mut app, "~/notes");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, key(KeyCode::Esc));
    let s = screen(&mut app, 160, 40);
    has_all(
        &s,
        &[
            "* 3 unsaved changes │",
            "╭ Unsaved changes ─",
            "│ 3 settings changed and not saved:",
            "│   * theme             neutral  →  warm",
            "│   * default effort    default  →  high",
            "│   * project roots     + ~/notes",
            "│ Save them to ~/.config/watchtower/config.toml?",
            "│ y save and close   n discard   Esc/Enter keep editing",
        ],
    );
    assert!(s[39].starts_with(" y save and close   n discard and close   Esc keep editing"));
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ctrl('s'));
    let s = screen(&mut app, 160, 40);
    line_with(&s, "✓ saved │");
    assert!(
        !s.iter().any(|l| l.contains("│ *  ")),
        "no change markers left"
    );
    assert!(
        s[38].starts_with(
            " ✓ Saved 3 changes to ~/.config/watchtower/config.toml · comments and formatting \
             kept · backup config.toml.bak · applied now"
        ),
        "{}",
        s[38]
    );
}

/// Screen 7: no config file.
#[test]
fn a_missing_file_says_ctrl_s_creates_it() {
    let env = env(None);
    let mut app = app_for(&env);
    open(&mut app);
    let s = screen(&mut app, 160, 40);
    has_all(
        &s,
        &[
            "│ Settings   ~/.config/watchtower/config.toml  (not found — using defaults)",
            "no unsaved changes │",
            "─ 12 settings · 1 account ╮",
            "│    default account              main  default",
            "│    default effort               default",
            "│    project roots                none  0 folders ▸  default",
            "│    main    claude       ~/.claude       (no group)  default account",
            "No config file: one built-in account.",
            "in file   no file yet",
        ],
    );
    assert_eq!(
        s[38],
        " ~/.config/watchtower/config.toml does not exist yet · Ctrl+S creates it with the \
         settings you change; everything else stays default"
    );
}

#[test]
fn the_minimal_style_draws_the_same_window_plainly() {
    let (_env, mut app) = opened_with(UiTheme::Warm, UiStyle::Minimal);
    overview(&mut app);
    for (w, h) in [(160, 40), (100, 30)] {
        let buf = render(&mut app, w, h);
        let s = lines(&buf);
        assert!(s[0].starts_with("┌ ◆ tower ─"), "square borders: {}", s[0]);
        has_all(
            &s,
            &[
                "┌ Settings ─",
                "├─ Appearance ─",
                "‹ dashboard ›  default",
                "* 1 unsaved change",
                "│    main    claude       ~/.claude       work        default account",
            ],
        );
        // The minimal look paints no theme colours.
        assert!(
            (0..buf.area.width).all(|x| !matches!(buf[(x, 0)].fg, Color::Rgb(..))),
            "no truecolor in the minimal look"
        );
    }
}

#[test]
fn a_small_terminal_falls_back_to_the_minimal_look() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    let s = screen(&mut app, 70, 20);
    assert!(s[0].starts_with("┌ ◆ tower"), "{}", s[0]);
    line_with(&s, "style");
}

fn colourless(buf: &Buffer) -> bool {
    let area = buf.area;
    (0..area.height).all(|y| {
        (0..area.width).all(|x| {
            let cell = &buf[(x, y)];
            cell.fg == Color::Reset && cell.bg == Color::Reset
        })
    })
}

#[test]
fn no_color_draws_no_colour() {
    for (theme, style) in [
        (UiTheme::Neutral, UiStyle::Dashboard),
        (UiTheme::Warm, UiStyle::Dashboard),
        (UiTheme::Neutral, UiStyle::Minimal),
    ] {
        let (_env, mut app) = opened_with(theme, style);
        app.color = false;
        overview(&mut app);
        assert!(
            colourless(&render(&mut app, 160, 40)),
            "{theme:?} {style:?}"
        );
        select(&mut app, "theme");
        press(&mut app, key(KeyCode::Enter));
        assert!(
            colourless(&render(&mut app, 160, 40)),
            "pick list {theme:?}"
        );
        press(&mut app, key(KeyCode::Esc));
        press(&mut app, key(KeyCode::Esc));
        assert!(colourless(&render(&mut app, 100, 30)), "question {theme:?}");
    }
}

#[test]
fn the_questions_render() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    select(&mut app, "allow cross-group hand-off");
    press(&mut app, key(KeyCode::Enter));
    let s = screen(&mut app, 100, 30);
    has_all(
        &s,
        &[
            "Allow cross-group hand-off?",
            "Terms follow the account",
            "y turn on   n/Esc keep it off",
        ],
    );
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ch('?'));
    let s = screen(&mut app, 100, 30);
    has_all(
        &s,
        &["Settings keys", "Ctrl+S", "save; the window stays open"],
    );
}
