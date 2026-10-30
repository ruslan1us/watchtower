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
                "─ 17 settings · 2 accounts ╮╭ style ─",
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
                "│    follow logs in background    10 s  default",
                "│    track subagents              [x] on  default",
                "├─ Accounts  read-only ─",
                "│    main    claude       ~/.claude       work        default account",
                "│    alt     claude-alt   ~/.claude-alt   personal",
                "a wrong command or config dir can wake sessions",
                "key       [ui] style",
                "values    dashboard · minimal · workspace",
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
            "─ 17 settings · 2 accounts ╮",
            "│▌   style                       ‹ dashboard ›  default",
            "│ *  default effort              high",
            // The rest of General and the accounts are below the fold at 30
            // rows.
            "│    show plan usage             [ ] off  default",
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
            "neutral                     greys; colour only for states       saved │",
            "│▌ warm                        warm greys and a clay accent",
            "terminal                    your terminal's 16 colours",
            "tokyo-night                 tower 0.2's blue-grey palette",
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
            "─ 17 settings · 1 account ╮",
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
                "show plan usage",
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

/// A config with long values: a long model name and prompt, a long
/// account and long project roots.
const LONG_CONFIG: &str = r#"default_account = "main"
default_model = "claude-opus-4-1-20250805-experimental-long-context"
handoff_prompt = "Continue where you left off: re-run the failing tests and fix them"
project_roots = ["~/workspace", "~/old-projects-from-the-previous-laptop-backup", "~/clients/very-long-client-name-holdings/repositories"]

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alternative-account"
command = "claude-alternative-wrapper"
config_dir = "~/.config/claude-code-alternative-profile"
group = "personal-sandbox"
"#;

const LONG_MODEL: &str = "claude-opus-4-1-20250805-experimental-long-context";
const LONG_PROMPT: &str = "\"Continue where you left off: re-run the failing tests and fix them\"";

fn opened_long() -> (Env, App) {
    let env = env(Some(LONG_CONFIG));
    let mut app = app_for(&env);
    app.set_theme(UiTheme::Neutral);
    open(&mut app);
    (env, app)
}

/// The text of the list panel's row holding `needle`, between its borders.
fn list_row(lines: &[String], needle: &str) -> String {
    let line = line_with(lines, needle);
    line.split('│').nth(1).unwrap_or("").to_owned()
}

/// No `…` glued to the next value, and one free column before the border.
fn spaced(row: &str, at: (u16, u16)) {
    let cs: Vec<char> = row.chars().collect();
    assert!(
        !cs.windows(2).any(|p| p[0] == '…' && p[1] != ' '),
        "{at:?}: a cut value runs into the next: {row:?}"
    );
    assert!(row.ends_with(' '), "{at:?}: touches the border: {row:?}");
}

#[test]
fn a_long_selected_choice_is_fitted_with_its_arrows() {
    let (_env, mut app) = opened_long();
    select(&mut app, "default model");
    for (w, h) in [(80, 24), (100, 30), (160, 45)] {
        let s = screen(&mut app, w, h);
        let row = list_row(&s, "default model  ");
        spaced(&row, (w, h));
        if w == 80 {
            assert!(
                row.ends_with("… › ") && row.contains("‹ claude-opus-4-1-2025"),
                "{w}x{h}: {row:?}"
            );
        } else {
            assert!(row.contains(&format!("‹ {LONG_MODEL} ›")), "{row:?}");
        }
    }
}

#[test]
fn unselected_values_use_the_whole_row() {
    let (_env, mut app) = opened_long();
    select(&mut app, "style");
    for (w, h) in [(80, 24), (100, 30), (160, 45)] {
        let s = screen(&mut app, w, h);
        let model = list_row(&s, "default model  ");
        let prompt = list_row(&s, "hand-off prompt  ");
        let roots = list_row(&s, "project roots  ");
        for row in [&model, &prompt, &roots] {
            spaced(row, (w, h));
            if row.contains('…') {
                // Cut only at the row's end: one padding column after it.
                let after = row.split('…').nth(1).unwrap();
                assert!(
                    after == " " || after == "  3 folders ▸ ",
                    "{w}x{h}: room left after the cut: {row:?}"
                );
            }
        }
        assert!(roots.contains("  3 folders ▸"), "{roots:?}");
        if w == 80 {
            assert!(
                prompt.contains("\"Continue where you left off: re-run the fai… "),
                "{prompt:?}"
            );
        } else if w == 100 {
            assert!(
                prompt.contains("re-run the failing tests and fix t… "),
                "{prompt:?}"
            );
        } else {
            assert!(prompt.contains(LONG_PROMPT), "{prompt:?}");
        }
        if w == 80 {
            assert!(
                model.ends_with(" claude-opus-4-1-20250805-experimental-long-c… "),
                "{model:?}"
            );
        } else {
            assert!(model.contains(LONG_MODEL), "{w}x{h}: {model:?}");
        }
        if w == 160 {
            assert!(
                roots.ends_with(
                    "~/workspace, ~/old-projects-from-the-previous-laptop-backup,…  3 folders ▸ "
                ),
                "{roots:?}"
            );
        }
    }
}

#[test]
fn account_columns_follow_their_values() {
    let (_env, mut app) = opened_long();
    press(&mut app, key(KeyCode::End));
    for (w, h) in [(80, 24), (100, 30), (160, 45)] {
        let s = screen(&mut app, w, h);
        let main = list_row(&s, "│    main ");
        let alt = list_row(&s, "altern");
        spaced(&main, (w, h));
        spaced(&alt, (w, h));
        assert!(main.contains(" default account "), "{w}x{h}: {main:?}");
        // The columns line up: each value starts where the other row's does.
        let starts = |row: &str| -> Vec<usize> {
            let cs: Vec<char> = row.chars().collect();
            (4..cs.len())
                .filter(|i| cs[*i] != ' ' && cs[*i - 1] == ' ')
                .take(4)
                .collect()
        };
        assert_eq!(starts(&main), starts(&alt), "{w}x{h}:\n{main}\n{alt}");
        let expected = match w {
            80 => "alternative-… claude-alter… ~/.config/clau… personal-san…",
            // The config dir gives way first, then the widest column.
            100 => "alternative-account claude-alternative-wrap… ~/.config/clau… personal-sandbox",
            _ => {
                "alternative-account claude-alternative-wrapper ~/.config/claude-code-… \
                 personal-sandbox"
            }
        };
        assert!(alt.contains(expected), "{w}x{h}: {alt:?}");
    }
}

#[test]
fn the_folder_list_sizes_its_columns_from_the_folders() {
    let (_env, mut app) = opened_long();
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    for (w, h) in [(80, 24), (100, 30), (160, 45)] {
        let s = screen(&mut app, w, h);
        let rows: Vec<String> = s
            .iter()
            .filter(|l| l.contains("✓ 3 git clones") || l.contains("skipped when"))
            .map(|l| l.split('│').find(|p| p.contains("~/")).unwrap().to_owned())
            .collect();
        assert_eq!(rows.len(), 3, "{w}x{h}:\n{}", s.join("\n"));
        for row in &rows {
            spaced(row, (w, h));
        }
        for row in &rows[1..] {
            assert!(
                row.contains(" not found; skipped when scanning "),
                "{w}x{h}: {row:?}"
            );
        }
        let clients = &rows[2];
        if w == 80 {
            assert!(clients.contains("… not found"), "{w}x{h}: {clients:?}");
        } else {
            assert!(
                clients.contains("~/clients/very-long-client-name-holdings/repositories "),
                "{w}x{h}: the window grows for the folders: {clients:?}"
            );
        }
    }
}

/// The detail panel's rows between its borders: the last 50 columns.
fn detail_rows(lines: &[String], w: u16) -> Vec<String> {
    let from = usize::from(w) - 50;
    lines
        .iter()
        .map(|l| l.chars().skip(from).collect::<String>())
        .filter(|r| r.starts_with('│') && r.ends_with('│'))
        .map(|r| {
            let cs: Vec<char> = r.chars().collect();
            cs[1..cs.len() - 1].iter().collect()
        })
        .collect()
}

/// Long values in the detail panel wrap and keep one padding column
/// before its border; a list of values breaks between its items
/// (doc/0024). Below 130 columns the help strip is cut with `…`.
#[test]
fn long_values_in_the_detail_panel_wrap_inside_its_padding() {
    let (_env, mut app) = opened_long();
    select(&mut app, "default model");
    let s = screen(&mut app, 160, 45);
    let rows = detail_rows(&s, 160);
    assert!(rows.len() > 10, "{}", s.join("\n"));
    for row in &rows {
        assert!(row.ends_with(' '), "touches the border: {row:?}");
        assert!(!row.trim_end().ends_with('·'), "{row:?}");
        assert!(!row.trim_start().starts_with('·'), "{row:?}");
    }
    let at = rows
        .iter()
        .position(|r| r.starts_with(" in file   "))
        .unwrap();
    assert_eq!(
        rows[at].trim_end(),
        " in file   \"claude-opus-4-1-20250805-"
    );
    assert_eq!(rows[at + 1].trim(), "experimental-long-context\"");
    let values = rows
        .iter()
        .position(|r| r.starts_with(" values    "))
        .unwrap();
    assert_eq!(
        rows[values].trim_end(),
        " values    default · fable · opus · sonnet"
    );
    assert_eq!(rows[values + 1].trim(), "haiku · a full model name");
    // A short value keeps its row.
    select(&mut app, "style");
    let s = screen(&mut app, 160, 45);
    let rows = detail_rows(&s, 160);
    assert!(
        rows.iter()
            .any(|r| r.trim_end() == " values    dashboard · minimal · workspace"),
        "{rows:?}"
    );

    // 80 columns: the one-line help strip, cut before its padding.
    select(&mut app, "default model");
    let s = screen(&mut app, 80, 24);
    let strip = list_row(&s, "default model  The new-session");
    assert!(strip.ends_with("… "), "{strip:?}");
    assert!(
        !s.iter().any(|l| l.contains("in file   ")),
        "no detail panel"
    );
}

/// The width of the window whose top border, on the row holding `row`,
/// starts with `title`.
fn window_width(lines: &[String], row: &str, title: &str) -> usize {
    let line = line_with(lines, row);
    let from = line.find(title).unwrap();
    let rest = &line[from..];
    rest.chars().take_while(|c| *c != '╮').count() + 1
}

/// The mock's screens are unchanged on a large terminal: columns sized from
/// short values keep their widths.
#[test]
fn short_values_keep_the_mock_layout_at_160x45() {
    let (_env, mut app) = opened_with(UiTheme::Neutral, UiStyle::Dashboard);
    overview(&mut app);
    let s = screen(&mut app, 160, 45);
    has_all(
        &s,
        &[
            "│▌   style                        ‹ dashboard ›  default",
            "│    theme                        neutral  default",
            "│    default account              main  (work)  default",
            "│ *  default effort               high",
            "│    project roots                ~/workspace, ~/old-projects  2 folders ▸",
            "│    hand-off prompt              \"continue\"  default",
            "│    allow cross-group hand-off   [ ] off  default",
            "│    main    claude       ~/.claude       work        default account",
            "│    alt     claude-alt   ~/.claude-alt   personal",
        ],
    );
    let style = line_with(&s, "‹ dashboard ›");
    assert!(style.contains("Enter pick   ←→ change │"), "{style}");
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    let s = screen(&mut app, 160, 45);
    has_all(
        &s,
        &[
            "│▌   ~/workspace             ✓ 3 git clones",
            "│    ~/old-projects          not found; skipped when scanning",
        ],
    );
    assert_eq!(window_width(&s, "─ 2 folders ╮", "╭ project roots ─"), 78);
}
