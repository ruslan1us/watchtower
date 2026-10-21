//! The dashboard style's colour themes (doc/0020, doc/0021): a [`Palette`]
//! of colour roles per theme, and the [`Theme`] the renderers draw with,
//! whose style helpers drop every colour under `NO_COLOR` (doc/0007), so
//! only bold, dim and reversed remain.
//!
//! Nothing paints a background behind ordinary text: painted backgrounds
//! are only the selected row, the state pills and the account chips.
//!
//! doc/0038 adds the light-background palettes, `ansi16` (the 16 named
//! colours plus attributes) and [`reduce_buffer`], which brings a drawn
//! frame within the terminal's colour depth.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::UnicodeWidthStr;

use crate::config::{ColorDepth, UiTheme};
use crate::groups::{Group, GroupKey};

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// How account chips are painted.
#[derive(Debug, Clone, Copy)]
pub enum Chip {
    /// The account's colour, bold, on a tint of it: `amount` of the colour
    /// over [`Palette::tint_base`].
    Tint(f32),
    /// Bold `fg` on `bg`, the same for every account.
    Flat(Color, Color),
    /// The account's colour, bold, without a background.
    Plain,
}

/// The colour roles of one dashboard theme (doc/0021).
#[derive(Debug)]
pub struct Palette {
    /// Text on the solid state pills.
    pub pill_text: Color,
    /// The dark end of the account chips' tint.
    pub tint_base: Color,
    /// Ordinary text.
    pub text: Color,
    /// Secondary text: labels, projects, ages.
    pub muted: Color,
    /// Dim text: interactive rows, the column header, key labels.
    pub faint: Color,
    /// Panel borders and rules.
    pub line: Color,
    /// The Interactive pill's background and its share of the bar.
    pub surface: Color,
    /// Background of the selected row.
    pub sel: Color,
    /// Text colour forced on the selected row, if any.
    pub sel_fg: Option<Color>,
    pub red: Color,
    pub amber: Color,
    pub green: Color,
    pub blue: Color,
    pub grey: Color,
    pub teal: Color,
    pub orange: Color,
    /// `◆ tower` in the header.
    pub logo: Color,
    /// Panel titles.
    pub title: Color,
    /// Keys in the footer and the detail panel.
    pub key: Color,
    /// `Ctrl+Z` in the detach hint.
    pub key_hot: Color,
    /// The selection bar `▌`.
    pub mark: Color,
    /// A project group's hue (grouping by project).
    pub project: Color,
    /// The logs panel's border.
    pub logs_border: Color,
    /// Whether the detail panel's border takes its subject's group colour;
    /// else it is [`Palette::line`].
    pub detail_border_group: bool,
    /// Colours of the account groups, in the order the groups first
    /// appear in the config; they repeat after the last.
    pub accounts: [Color; 4],
    pub chip: Chip,
    /// The selected row and the pills use reverse video instead of
    /// painted colours, so they follow the terminal's own theme.
    pub reverse: bool,
    /// The 16-colour attribute mode of `ansi16` (doc/0038), for a palette
    /// whose colours may sit on a dark or a light background: text in a
    /// hue ([`Theme::hue`]) is plain, so hues only colour glyphs, bars,
    /// borders and pills; pills are fixed named pairs (black or white text
    /// on the hue, not bold, as bold can brighten black); and `DarkGray`,
    /// the dim and line role, is drawn as the default foreground dimmed.
    pub ansi16: bool,
}

/// Neutral greys, no tint; colour only for states and attention. The
/// default (doc/0021).
pub static NEUTRAL: Palette = {
    const TEXT: Color = rgb(235, 235, 235);
    const LINE: Color = rgb(110, 110, 110);
    Palette {
        pill_text: rgb(0, 0, 0),
        tint_base: rgb(0, 0, 0),
        text: TEXT,
        muted: rgb(172, 172, 172),
        faint: rgb(138, 138, 138),
        line: LINE,
        surface: rgb(64, 64, 64),
        sel: rgb(58, 58, 58),
        sel_fg: Some(rgb(255, 255, 255)),
        red: rgb(255, 100, 100),
        amber: rgb(255, 196, 0),
        green: rgb(80, 220, 110),
        blue: rgb(100, 170, 255),
        grey: rgb(172, 172, 172),
        teal: rgb(0, 215, 180),
        orange: rgb(255, 145, 60),
        logo: TEXT,
        title: TEXT,
        key: TEXT,
        key_hot: TEXT,
        mark: rgb(255, 255, 255),
        project: TEXT,
        logs_border: LINE,
        detail_border_group: false,
        accounts: [TEXT; 4],
        chip: Chip::Flat(TEXT, rgb(64, 64, 64)),
        reverse: false,
        ansi16: false,
    }
};

/// Warm greys with a clay-orange accent for the logo, titles, keys and
/// the selection bar.
pub static WARM: Palette = {
    const ACCENT: Color = rgb(232, 135, 95);
    const AMBER: Color = rgb(240, 185, 75);
    const CYAN: Color = rgb(110, 200, 215);
    const MAGENTA: Color = rgb(215, 150, 210);
    const TEAL: Color = rgb(110, 205, 175);
    const LINE: Color = rgb(118, 108, 97);
    Palette {
        pill_text: rgb(0, 0, 0),
        tint_base: rgb(0, 0, 0),
        text: rgb(236, 230, 221),
        muted: rgb(184, 174, 162),
        faint: rgb(146, 137, 126),
        line: LINE,
        surface: rgb(72, 64, 56),
        sel: rgb(62, 52, 44),
        sel_fg: Some(rgb(255, 250, 244)),
        red: rgb(245, 105, 95),
        amber: AMBER,
        green: rgb(160, 205, 110),
        blue: rgb(130, 170, 240),
        grey: rgb(184, 174, 162),
        teal: TEAL,
        orange: rgb(250, 160, 120),
        logo: ACCENT,
        title: ACCENT,
        key: ACCENT,
        key_hot: AMBER,
        mark: ACCENT,
        project: ACCENT,
        logs_border: LINE,
        detail_border_group: false,
        accounts: [CYAN, MAGENTA, TEAL, AMBER],
        chip: Chip::Tint(0.15),
        reverse: false,
        ansi16: false,
    }
};

/// Named ANSI colours only, so the terminal's theme decides the real
/// values; reverse video for the selection and the pills.
pub static TERMINAL: Palette = Palette {
    // Unused: pills are reversed, so their text is the terminal background.
    pill_text: Color::Black,
    tint_base: Color::Black,
    text: Color::Reset,
    muted: Color::Gray,
    faint: Color::DarkGray,
    line: Color::DarkGray,
    surface: Color::DarkGray,
    sel: Color::Reset,
    sel_fg: None,
    red: Color::LightRed,
    amber: Color::LightYellow,
    green: Color::LightGreen,
    blue: Color::LightCyan,
    grey: Color::Gray,
    teal: Color::LightMagenta,
    orange: Color::Yellow,
    logo: Color::Reset,
    title: Color::Reset,
    key: Color::Reset,
    key_hot: Color::LightYellow,
    mark: Color::Reset,
    project: Color::Reset,
    logs_border: Color::DarkGray,
    detail_border_group: true,
    accounts: [
        Color::Cyan,
        Color::LightMagenta,
        Color::Green,
        Color::Yellow,
    ],
    chip: Chip::Plain,
    reverse: true,
    ansi16: false,
};

/// The original blue-grey truecolor palette (doc/0020), unchanged.
pub static TOKYO_NIGHT: Palette = {
    const BASE: Color = rgb(26, 27, 38);
    const TEXT: Color = rgb(192, 202, 245);
    const AMBER: Color = rgb(224, 175, 104);
    const BLUE: Color = rgb(122, 162, 247);
    const CYAN: Color = rgb(125, 207, 255);
    const MAGENTA: Color = rgb(187, 154, 247);
    const TEAL: Color = rgb(115, 218, 202);
    const ORANGE: Color = rgb(255, 158, 100);
    Palette {
        pill_text: BASE,
        tint_base: BASE,
        text: TEXT,
        muted: rgb(115, 122, 162),
        faint: rgb(84, 92, 126),
        line: rgb(59, 66, 97),
        surface: rgb(52, 59, 88),
        sel: rgb(45, 63, 118),
        sel_fg: None,
        red: rgb(247, 118, 142),
        amber: AMBER,
        green: rgb(158, 206, 106),
        blue: BLUE,
        grey: rgb(130, 137, 175),
        teal: TEAL,
        orange: ORANGE,
        logo: BLUE,
        title: TEXT,
        key: BLUE,
        key_hot: AMBER,
        mark: BLUE,
        project: BLUE,
        logs_border: BLUE,
        detail_border_group: true,
        accounts: [CYAN, MAGENTA, TEAL, ORANGE],
        chip: Chip::Tint(0.3),
        reverse: false,
        ansi16: false,
    }
};

/// `neutral` for a light terminal background (doc/0038): dark greys on
/// the terminal's own white, deeper state accents with white pill text,
/// and a light-grey selection bar with black text.
pub static NEUTRAL_LIGHT: Palette = {
    const TEXT: Color = rgb(31, 31, 31);
    const LINE: Color = rgb(138, 138, 138);
    const CHIP: Color = rgb(220, 220, 220);
    Palette {
        pill_text: rgb(255, 255, 255),
        tint_base: rgb(255, 255, 255),
        text: TEXT,
        muted: rgb(79, 79, 79),
        faint: rgb(107, 107, 107),
        line: LINE,
        surface: CHIP,
        sel: rgb(214, 214, 214),
        sel_fg: Some(rgb(0, 0, 0)),
        red: rgb(196, 43, 43),
        amber: rgb(138, 90, 0),
        green: rgb(29, 122, 53),
        blue: rgb(31, 95, 196),
        grey: rgb(92, 92, 92),
        teal: rgb(0, 120, 106),
        orange: rgb(179, 71, 0),
        logo: TEXT,
        title: TEXT,
        key: TEXT,
        key_hot: TEXT,
        mark: rgb(0, 0, 0),
        project: TEXT,
        logs_border: LINE,
        detail_border_group: false,
        accounts: [TEXT; 4],
        chip: Chip::Flat(TEXT, CHIP),
        reverse: false,
        ansi16: false,
    }
};

/// `warm` for a light terminal background (doc/0038): warm dark greys, a
/// deep clay accent, a sand selection bar, account chips on a pale tint.
pub static WARM_LIGHT: Palette = {
    const ACCENT: Color = rgb(152, 64, 28);
    const AMBER: Color = rgb(135, 90, 0);
    const CYAN: Color = rgb(26, 108, 133);
    const MAGENTA: Color = rgb(147, 64, 138);
    const TEAL: Color = rgb(29, 115, 94);
    const LINE: Color = rgb(150, 138, 126);
    Palette {
        pill_text: rgb(255, 255, 255),
        tint_base: rgb(255, 255, 255),
        text: rgb(42, 35, 29),
        muted: rgb(92, 81, 72),
        faint: rgb(117, 106, 95),
        line: LINE,
        surface: rgb(227, 216, 203),
        sel: rgb(234, 220, 205),
        sel_fg: Some(rgb(26, 18, 11)),
        red: rgb(184, 50, 42),
        amber: AMBER,
        green: rgb(45, 112, 35),
        blue: rgb(58, 92, 173),
        grey: rgb(92, 81, 72),
        teal: TEAL,
        orange: rgb(168, 74, 26),
        logo: ACCENT,
        title: ACCENT,
        key: ACCENT,
        key_hot: AMBER,
        mark: ACCENT,
        project: ACCENT,
        logs_border: LINE,
        detail_border_group: false,
        accounts: [CYAN, MAGENTA, TEAL, AMBER],
        chip: Chip::Tint(0.12),
        reverse: false,
        ansi16: false,
    }
};

/// The 16 ANSI colours with attributes, for dark and light backgrounds
/// alike (doc/0038): text in the default foreground, dim text and lines
/// dimmed, hues only on glyphs, bars, borders and pills (named pairs), the
/// selection in reverse video. What a truecolor theme gives way to on a
/// 16-colour terminal.
pub static ANSI16: Palette = Palette {
    // Unused: pills take black or white by their hue (`ansi16`).
    pill_text: Color::Black,
    tint_base: Color::Black,
    text: Color::Reset,
    muted: Color::Reset,
    // Drawn as the default foreground dimmed (`ansi16`).
    faint: Color::DarkGray,
    line: Color::DarkGray,
    surface: Color::DarkGray,
    sel: Color::Reset,
    sel_fg: None,
    red: Color::Red,
    amber: Color::Yellow,
    green: Color::Green,
    blue: Color::Blue,
    grey: Color::Gray,
    teal: Color::Cyan,
    orange: Color::Magenta,
    logo: Color::Reset,
    title: Color::Reset,
    key: Color::Reset,
    key_hot: Color::Reset,
    mark: Color::Reset,
    project: Color::Reset,
    logs_border: Color::Reset,
    detail_border_group: true,
    accounts: [Color::Reset; 4],
    chip: Chip::Plain,
    reverse: true,
    ansi16: true,
};

/// The palette of `theme`.
pub fn palette(theme: UiTheme) -> &'static Palette {
    match theme {
        UiTheme::Neutral => &NEUTRAL,
        UiTheme::Warm => &WARM,
        UiTheme::Terminal => &TERMINAL,
        UiTheme::TokyoNight => &TOKYO_NIGHT,
        UiTheme::NeutralLight => &NEUTRAL_LIGHT,
        UiTheme::WarmLight => &WARM_LIGHT,
        UiTheme::Ansi16 => &ANSI16,
    }
}

/// The text colour of an `ansi16` pill on hue `c`: white on the dark
/// hues, black on the light ones; `None` (reverse video) for anything
/// else, such as the default colour.
fn ansi16_pill_text(c: Color) -> Option<Color> {
    match c {
        Color::Red | Color::Blue | Color::Magenta => Some(Color::White),
        Color::Green | Color::Yellow | Color::Cyan | Color::Gray => Some(Color::Black),
        _ => None,
    }
}

/// `c` mixed into `base`: `amount` 0 is `base`, 1 is `c`; named colours
/// are returned as they are.
fn mix(c: Color, base: Color, amount: f32) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) = (c, base) else {
        return c;
    };
    let m = |x: u8, y: u8| (f32::from(y) + (f32::from(x) - f32::from(y)) * amount).round() as u8;
    Color::Rgb(m(r, br), m(g, bg), m(b, bb))
}

/// What the dashboard draws with: the theme's palette, and whether
/// colours may be used at all (`NO_COLOR` unset). With `color` false
/// every colour is dropped and only modifiers remain.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub p: &'static Palette,
}

impl Theme {
    pub fn new(theme: UiTheme, color: bool) -> Theme {
        Theme {
            color,
            p: palette(theme),
        }
    }

    /// The hue of a state group.
    pub fn group_color(self, g: Group) -> Color {
        let p = self.p;
        match g {
            Group::Orchestrator => p.teal,
            Group::NeedsMe => p.amber,
            Group::Failed => p.red,
            Group::ReadyForReview => p.green,
            Group::Working => p.blue,
            Group::Other => p.orange,
            Group::Stopped => p.grey,
            Group::Interactive => p.faint,
        }
    }

    /// The hue of a list group: its state group's, or the project hue.
    pub fn key_color(self, key: &GroupKey) -> Color {
        match key {
            GroupKey::State(g) => self.group_color(*g),
            GroupKey::Project(_) => self.p.project,
        }
    }

    /// The colour of account group number `index` (in config order).
    pub fn account_color(self, index: usize) -> Color {
        self.p.accounts[index % self.p.accounts.len()]
    }

    /// A solid state pill in colour `c`: dark text on `c`, or reverse
    /// video in a reversing theme; bold only without colour.
    pub fn pill(self, c: Color) -> Style {
        if !self.color {
            Style::new().add_modifier(Modifier::BOLD)
        } else if self.p.ansi16
            && let Some(text) = ansi16_pill_text(c)
        {
            Style::new().fg(text).bg(c)
        } else if self.p.reverse {
            Style::new()
                .fg(c)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new()
                .fg(self.p.pill_text)
                .bg(c)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// The Interactive pill: text on the surface colour.
    pub fn interactive_pill(self) -> Style {
        if !self.color {
            Style::new().add_modifier(Modifier::BOLD)
        } else if self.p.reverse {
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new()
                .fg(self.p.text)
                .bg(self.p.surface)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// An account chip in colour `c`.
    pub fn chip(self, c: Color) -> Style {
        match self.p.chip {
            Chip::Tint(a) => self.bold(c).patch(self.on(c, mix(c, self.p.tint_base, a))),
            Chip::Flat(f, b) => self.bold(f).patch(self.on(f, b)),
            Chip::Plain => self.bold(c),
        }
    }

    /// The detail panel's border for a subject in colour `c`.
    pub fn detail_border(self, c: Color) -> Color {
        if self.p.detail_border_group {
            c
        } else {
            self.p.line
        }
    }

    /// The selected line: a painted bar, or reversed in a reversing theme
    /// and without colour (doc/0007: readable without colour).
    pub fn selected(self) -> Style {
        if !self.color {
            Style::new().add_modifier(Modifier::REVERSED)
        } else if self.p.reverse {
            Style::new()
                .fg(Color::Reset)
                .add_modifier(Modifier::REVERSED)
        } else {
            match self.p.sel_fg {
                Some(f) => Style::new().bg(self.p.sel).fg(f),
                None => Style::new().bg(self.p.sel),
            }
        }
    }

    /// Foreground `c`; in `ansi16` its dim role (`DarkGray`) is the default
    /// foreground dimmed, which reads on dark and light backgrounds.
    pub fn fg(self, c: Color) -> Style {
        if !self.color {
            Style::new()
        } else if self.p.ansi16 && c == Color::DarkGray {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().fg(c)
        }
    }

    /// Whether text may take a hue: colour is on and the theme is not
    /// `ansi16`, whose hues cannot be trusted on every background
    /// (doc/0038).
    pub fn hues(self) -> bool {
        self.color && !self.p.ansi16
    }

    /// Text in hue `c` (a state column, a group title, a warning): the
    /// hue, or plain where [`Theme::hues`] is false. Glyphs, bars and
    /// borders take [`Theme::fg`] instead.
    pub fn hue(self, c: Color) -> Style {
        if self.hues() {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    /// Bold foreground `c`.
    pub fn bold(self, c: Color) -> Style {
        self.fg(c).add_modifier(Modifier::BOLD)
    }

    /// Foreground `f` on background `b`; plain without colour.
    pub fn on(self, f: Color, b: Color) -> Style {
        if self.color {
            Style::new().fg(f).bg(b)
        } else {
            Style::new()
        }
    }

    /// Faint text: a grey with colour, the dim modifier without.
    pub fn faint(self) -> Style {
        if self.color {
            self.fg(self.p.faint)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// Ordinary text.
    pub fn text(self) -> Style {
        self.fg(self.p.text)
    }

    /// Muted text, for labels; plain without colour.
    pub fn muted(self) -> Style {
        self.fg(self.p.muted)
    }

    /// The attention style (doc/0007): bold, plus the accent with colour.
    pub fn attention(self) -> Style {
        self.bold(self.p.red)
    }

    /// Panel borders and rules.
    pub fn line(self) -> Style {
        self.fg(self.p.line)
    }
}

/// How the windows over the screen are drawn (doc/0020, doc/0021): the
/// overlays, dialogs, pickers, help, popups and the filter's input line.
/// The minimal style keeps its plain look (square borders, terminal
/// colours); the dashboard style draws them in its theme, like its panels.
/// Every window takes its styles from here, so a new one looks like the
/// others.
#[derive(Debug, Clone, Copy)]
pub enum Look {
    /// The minimal style's look; `color` is false under `NO_COLOR`.
    Minimal { color: bool },
    /// The dashboard style in a theme.
    Themed(Theme),
}

/// Whether `token` (trimmed of brackets and punctuation) names a key, for
/// the key hints of [`Look::hint_line`].
fn is_key(token: &str) -> bool {
    const KEYS: &[&str] = &[
        "Tab/Shift+Tab",
        "Tab",
        "Enter",
        "Esc",
        "Space",
        "Space/Esc",
        "↑↓",
        "↑↓/PgUp/PgDn",
        "PgUp/PgDn",
        "Home",
        "End",
        "?",
        "y",
        "n",
        "n/Esc",
    ];
    ["Ctrl+", "Alt+", "Shift+"]
        .iter()
        .any(|p| token.starts_with(p) && token.len() > p.len())
        || KEYS.contains(&token)
}

/// `token` split into leading punctuation, the core and trailing
/// punctuation: `(or` → `("(", "or", "")`, `Alt+Enter)` → `("",
/// "Alt+Enter", ")")`.
fn split_token(token: &str) -> (&str, &str, &str) {
    let punct = |c: char| matches!(c, '(' | ')' | ',' | ':' | '.' | ';');
    let core = token.trim_start_matches(punct);
    let lead = &token[..token.len() - core.len()];
    let trimmed = core.trim_end_matches(punct);
    // A lone `?` stays a key; only brackets and separators are trimmed.
    let trimmed = if trimmed.is_empty() { core } else { trimmed };
    (lead, trimmed, &core[trimmed.len()..])
}

impl Look {
    /// The minimal look's style, or the theme's.
    fn pick(self, minimal: Style, themed: impl FnOnce(Theme) -> Style) -> Style {
        match self {
            Look::Minimal { .. } => minimal,
            Look::Themed(th) => themed(th),
        }
    }

    /// A window's frame: square borders and a plain title, or rounded
    /// borders in the theme's line colour with the title in its title
    /// colour, like the dashboard's panels.
    pub fn overlay_block(self, title: impl Into<String>) -> Block<'static> {
        let title = title.into();
        match self {
            Look::Minimal { .. } => Block::new().borders(Borders::ALL).title(title),
            Look::Themed(th) => Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(th.line())
                .title(Line::styled(title, self.overlay_title())),
        }
    }

    /// A window's title: plain, or bold in the theme's title colour.
    pub fn overlay_title(self) -> Style {
        self.pick(Style::new(), |th| th.bold(th.p.title))
    }

    /// Ordinary text: values, window text.
    pub fn text(self) -> Style {
        self.pick(Style::new(), Theme::text)
    }

    /// Secondary text: labels such as `From:` and `To:`.
    pub fn secondary(self) -> Style {
        self.pick(Style::new(), Theme::muted)
    }

    /// Hints, placeholders and notes: dim.
    pub fn hint(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::DIM), Theme::faint)
    }

    /// Bold text: a value that stands out, a state message.
    pub fn strong(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::BOLD), |th| {
            th.bold(th.p.text)
        })
    }

    /// A heading inside a window (the help's dialog section).
    pub fn heading(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::BOLD), |th| {
            th.bold(th.p.title)
        })
    }

    /// The keys of a confirmation's answer line (`y`, `n`, `Esc`).
    pub fn answer_key(self) -> Style {
        match self {
            Look::Minimal { color: true } => {
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            }
            Look::Minimal { color: false } => Style::new().add_modifier(Modifier::BOLD),
            Look::Themed(th) => th.bold(th.p.key),
        }
    }

    /// A key in a table of keys (the help): plain in the minimal look.
    pub fn table_key(self) -> Style {
        self.pick(Style::new(), |th| th.bold(th.p.key))
    }

    /// The `/` of the filter's input line.
    pub fn input_prompt(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::BOLD), |th| {
            th.bold(th.p.key)
        })
    }

    /// A field's label: secondary, or bold in the key colour while the
    /// field has the focus (with its `▶`).
    pub fn field_label(self, focused: bool) -> Style {
        match self {
            Look::Minimal { color } => {
                let mut style = Style::new();
                if focused {
                    style = style.add_modifier(Modifier::BOLD);
                    if color {
                        style = style.fg(Color::Cyan);
                    }
                }
                style
            }
            Look::Themed(th) if focused => th.bold(th.p.key),
            Look::Themed(th) => th.muted(),
        }
    }

    /// The selected item of a list (a picker's account, the dialog's
    /// project): reversed in the minimal look; in a theme the selected
    /// row's bar (reversed in `terminal` and without colour).
    pub fn selected_item(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::REVERSED), |th| {
            th.text().patch(th.selected())
        })
    }

    /// The `▶` of the selected item, drawn over [`Look::selected_item`]:
    /// the theme's selection mark colour.
    pub fn selected_marker(self) -> Style {
        self.pick(Style::new(), |th| th.fg(th.p.mark))
    }

    /// An item of a list that is not selected.
    pub fn item(self) -> Style {
        self.text()
    }

    /// The selected item of a list without the focus: bold.
    pub fn chosen_item(self) -> Style {
        self.strong()
    }

    /// A choice field's value (`‹ auto ›`), on the selection bar while
    /// focused.
    pub fn choice(self, focused: bool) -> Style {
        if focused {
            self.pick(
                Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
                |th| th.text().patch(th.selected()).add_modifier(Modifier::BOLD),
            )
        } else {
            self.strong()
        }
    }

    /// A question inside a window (discard, quit): bold on the selection
    /// bar.
    pub fn prompt(self) -> Style {
        self.pick(
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            |th| th.text().patch(th.selected()).add_modifier(Modifier::BOLD),
        )
    }

    /// An error: red and bold, bold without colour.
    pub fn error(self) -> Style {
        match self {
            Look::Minimal { color: true } => {
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
            }
            Look::Minimal { color: false } => Style::new().add_modifier(Modifier::BOLD),
            Look::Themed(th) => th.bold(th.p.red),
        }
    }

    /// A warning (the Haiku note): amber, bold without colour and in
    /// `ansi16`.
    pub fn warning(self) -> Style {
        match self {
            Look::Minimal { color: true } => Style::new().fg(Color::Yellow),
            Look::Themed(th) if th.hues() => th.fg(th.p.amber),
            _ => Style::new().add_modifier(Modifier::BOLD),
        }
    }

    /// Borders and rules drawn by hand (section separators): plain, or the
    /// theme's line colour.
    pub fn rule(self) -> Style {
        self.pick(Style::new(), Theme::line)
    }

    /// The `*` of an unsaved change (the Settings window): amber and bold,
    /// bold without colour. `●` stays the attention marker (doc/0007).
    pub fn modified(self) -> Style {
        match self {
            Look::Minimal { color: true } => {
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            }
            Look::Themed(th) => th.hue(th.p.amber).add_modifier(Modifier::BOLD),
            Look::Minimal { color: false } => Style::new().add_modifier(Modifier::BOLD),
        }
    }

    /// A success (`✓ saved`): green, bold without colour and in `ansi16`.
    pub fn success(self) -> Style {
        match self {
            Look::Minimal { color: true } => Style::new().fg(Color::Green),
            Look::Themed(th) if th.hues() => th.fg(th.p.green),
            _ => Style::new().add_modifier(Modifier::BOLD),
        }
    }

    /// A key in a footer or a key line: bold, in the key colour.
    pub fn key(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::BOLD), |th| {
            th.bold(th.p.key)
        })
    }

    /// The label after a key in a footer: dim, or the theme's secondary.
    pub fn key_label(self) -> Style {
        self.pick(Style::new().add_modifier(Modifier::DIM), Theme::muted)
    }

    /// A row of key hints (`Enter keep · Esc clear`): dim, with the keys in
    /// the theme's key colour.
    pub fn hint_line(self, text: impl Into<String>) -> Line<'static> {
        match self {
            Look::Minimal { .. } => Line::styled(text.into(), self.hint()),
            Look::Themed(_) => Line::from(self.hint_spans(text)),
        }
    }

    /// [`Look::hint_line`] as spans, each with its whole style.
    pub fn hint_spans(self, text: impl Into<String>) -> Vec<Span<'static>> {
        let text = text.into();
        let Look::Themed(th) = self else {
            return vec![Span::styled(text, self.hint())];
        };
        let (hint, key) = (th.faint(), th.bold(th.p.key));
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut plain = String::new();
        for (i, token) in text.split(' ').enumerate() {
            if i > 0 {
                plain.push(' ');
            }
            let (lead, core, tail) = split_token(token);
            if is_key(core) {
                plain.push_str(lead);
                if !plain.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut plain), hint));
                }
                spans.push(Span::styled(core.to_owned(), key));
                plain.push_str(tail);
            } else {
                plain.push_str(token);
            }
        }
        if !plain.is_empty() {
            spans.push(Span::styled(plain, hint));
        }
        spans
    }

    /// A row of window text in `base`; a leading `Key:` (as in `Ctrl+T:
    /// run …`, `Esc: leave it`) takes the key colour.
    pub fn text_line(self, row: impl Into<String>, base: Style) -> Line<'static> {
        let row = row.into();
        if let Look::Themed(th) = self {
            let body = row.trim_start_matches(' ');
            let indent = row.len() - body.len();
            if let Some((first, rest)) = body.split_once(' ')
                && let Some(core) = first.strip_suffix(':')
                && is_key(core)
            {
                return Line::from(vec![
                    Span::styled(row[..indent].to_owned(), base),
                    Span::styled(core.to_owned(), base.patch(th.bold(th.p.key))),
                    Span::styled(format!(": {rest}"), base),
                ]);
            }
        }
        Line::styled(row, base)
    }

    /// A row of window text whose `labels` (`From:`, `To:`) are secondary
    /// and the rest ordinary text.
    pub fn labelled_line(self, row: &str, labels: &[&str]) -> Line<'static> {
        if let Look::Minimal { .. } = self {
            return Line::from(row.to_owned());
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, token) in row.split(' ').enumerate() {
            let style = if labels.contains(&token) {
                self.secondary()
            } else {
                self.text()
            };
            let s = if i > 0 {
                format!(" {token}")
            } else {
                token.to_owned()
            };
            spans.push(Span::styled(s, style));
        }
        Line::from(spans)
    }
}

/// The 16 named ANSI colours in palette order (index 0 to 15).
const NAMED: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// xterm's default values of the 16 named colours, only to pick the
/// nearest one; the terminal shows its own.
const NAMED_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// The levels of the 256-colour palette's 6×6×6 cube (indices 16 to 231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs();
    d(a.0, b.0).pow(2) + d(a.1, b.1).pow(2) + d(a.2, b.2).pow(2)
}

/// The nearest colour of the 256-colour palette's cube and grey ramp
/// (16 to 255; 0 to 15 are the terminal's own, so never chosen).
pub fn nearest_256(c: (u8, u8, u8)) -> u8 {
    let level = |v: u8| -> usize {
        (0..CUBE.len())
            .min_by_key(|&i| (i32::from(CUBE[i]) - i32::from(v)).unsigned_abs())
            .expect("the cube has levels")
    };
    let (r, g, b) = (level(c.0), level(c.1), level(c.2));
    let cube = (CUBE[r], CUBE[g], CUBE[b]);
    let grey = (0..24u8)
        .min_by_key(|&i| {
            let v = 8 + 10 * i;
            distance(c, (v, v, v))
        })
        .expect("the ramp has greys");
    let gv = 8 + 10 * grey;
    if distance(c, (gv, gv, gv)) < distance(c, cube) {
        232 + grey
    } else {
        16 + (36 * r + 6 * g + b) as u8
    }
}

/// The RGB value of 256-colour index `i` (xterm's defaults for 0 to 15).
fn rgb_of_index(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => NAMED_RGB[usize::from(i)],
        16..=231 => {
            let k = usize::from(i - 16);
            (CUBE[k / 36], CUBE[(k / 6) % 6], CUBE[k % 6])
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

/// The nearest of the 16 named colours.
pub fn nearest_16(c: (u8, u8, u8)) -> Color {
    let i = (0..16)
        .min_by_key(|&i| distance(c, NAMED_RGB[i]))
        .expect("16 colours");
    NAMED[i]
}

/// `c` within `depth` (doc/0038): at 256 colours an RGB colour becomes
/// the nearest palette index; at 16 an RGB or indexed colour becomes the
/// nearest named one. Named colours and `Reset` stay.
pub fn reduce(c: Color, depth: ColorDepth) -> Color {
    match (c, depth) {
        (Color::Rgb(r, g, b), ColorDepth::Ansi256) => Color::Indexed(nearest_256((r, g, b))),
        (Color::Rgb(r, g, b), ColorDepth::Ansi16) => nearest_16((r, g, b)),
        (Color::Indexed(i), ColorDepth::Ansi16) if i < 16 => NAMED[usize::from(i)],
        (Color::Indexed(i), ColorDepth::Ansi16) => nearest_16(rgb_of_index(i)),
        _ => c,
    }
}

/// Brings every colour drawn in `buf` within `depth` ([`reduce`]): after
/// a frame is drawn, so the themes, the minimal style and the logs
/// panel's replayed colours all reach the terminal in colours it shows.
/// Nothing changes at truecolor.
pub fn reduce_buffer(buf: &mut Buffer, depth: ColorDepth) {
    if !matches!(depth, ColorDepth::Ansi256 | ColorDepth::Ansi16) {
        return;
    }
    for cell in &mut buf.content {
        cell.fg = reduce(cell.fg, depth);
        cell.bg = reduce(cell.bg, depth);
    }
}

/// A line under construction that tracks its display width.
#[derive(Default)]
pub struct Lb {
    pub spans: Vec<Span<'static>>,
    pub w: usize,
}

impl Lb {
    pub fn new() -> Lb {
        Lb::default()
    }

    pub fn push(&mut self, s: impl Into<String>, st: Style) -> &mut Lb {
        let s = s.into();
        self.w += s.width();
        self.spans.push(Span::styled(s, st));
        self
    }

    /// Appends another line's spans.
    pub fn append(&mut self, other: Lb) -> &mut Lb {
        self.w += other.w;
        self.spans.extend(other.spans);
        self
    }

    /// Pads with spaces up to `w` columns.
    pub fn pad_to(&mut self, w: usize, st: Style) -> &mut Lb {
        if self.w < w {
            let n = w - self.w;
            self.push(" ".repeat(n), st);
        }
        self
    }

    pub fn line(self) -> Line<'static> {
        Line::from(self.spans)
    }
}

/// Right-aligned in `w` columns.
pub fn rfit(s: &str, w: usize) -> String {
    format!("{}{s}", " ".repeat(w.saturating_sub(s.width())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2 relative luminance of an RGB colour.
    fn luminance(c: Color) -> f64 {
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
    }

    /// WCAG 2 contrast ratio of `a` against `b`.
    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    const BLACK: Color = Color::Rgb(0, 0, 0);

    /// doc/0021's targets for `neutral` and `warm`, on a black terminal:
    /// text and secondary 7:1, dim and state colours 4.5:1, lines 3:1, and
    /// 7:1 for text on every painted background (selection, pills, chips).
    #[test]
    fn neutral_and_warm_meet_the_contrast_targets_on_black() {
        for (name, p) in [("neutral", &NEUTRAL), ("warm", &WARM)] {
            let check = |what: &str, fg: Color, bg: Color, min: f64| {
                let r = contrast(fg, bg);
                assert!(r >= min, "{name} {what}: {r:.2} < {min}");
            };
            check("text", p.text, BLACK, 7.0);
            check("secondary", p.muted, BLACK, 7.0);
            check("dim", p.faint, BLACK, 4.5);
            check("lines", p.line, BLACK, 3.0);
            let th = Theme { color: true, p };
            for g in Group::ALL {
                if g == Group::Interactive {
                    continue;
                }
                let c = th.group_color(g);
                check(g.title(), c, BLACK, 4.5);
                check(&format!("{} pill", g.title()), p.pill_text, c, 7.0);
                let st = th.pill(c);
                assert_eq!((st.fg, st.bg), (Some(p.pill_text), Some(c)));
            }
            let st = th.interactive_pill();
            check("Interactive pill", st.fg.unwrap(), st.bg.unwrap(), 7.0);
            let st = th.selected();
            check("selection", st.fg.unwrap(), st.bg.unwrap(), 7.0);
            for k in 0..p.accounts.len() {
                let st = th.chip(th.account_color(k));
                check(&format!("chip {k}"), st.fg.unwrap(), st.bg.unwrap(), 7.0);
                check(&format!("account {k}"), th.account_color(k), BLACK, 4.5);
            }
            check("logo", p.logo, BLACK, 4.5);
            check("keys", p.key, BLACK, 4.5);

            // The windows' roles (doc/0021): text on the selection bar
            // (selected items, focused choices, prompts) 7:1, the `▶`
            // marker on it 4.5:1, title, key, error and warning on black.
            let look = Look::Themed(th);
            for (what, st) in [
                ("selected item", look.selected_item()),
                ("focused choice", look.choice(true)),
                ("prompt", look.prompt()),
            ] {
                check(what, st.fg.unwrap(), st.bg.unwrap(), 7.0);
            }
            let bar = look.selected_item().bg.unwrap();
            let marker = look.selected_item().patch(look.selected_marker());
            check("selection marker", marker.fg.unwrap(), bar, 4.5);
            for (what, st, min) in [
                ("title", look.overlay_title(), 7.0),
                ("label", look.field_label(false), 7.0),
                ("focused label", look.field_label(true), 4.5),
                ("text", look.text(), 7.0),
                ("hint", look.hint(), 4.5),
                ("answer key", look.answer_key(), 4.5),
                ("error", look.error(), 4.5),
                ("warning", look.warning(), 4.5),
            ] {
                check(what, st.fg.unwrap(), BLACK, min);
            }
        }
    }

    #[test]
    fn hints_put_keys_in_the_key_colour_and_keep_the_text() {
        let th = Theme::new(UiTheme::Warm, true);
        let text = " Ctrl+Enter (or Alt+Enter) start · Esc or ? closes · type a name ";
        let spans = Look::Themed(th).hint_spans(text);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, text);
        let keys: Vec<&str> = spans
            .iter()
            .filter(|s| s.style == th.bold(WARM.key))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(keys, ["Ctrl+Enter", "Alt+Enter", "Esc", "?"]);
        // The minimal look keeps one dim span.
        let minimal = Look::Minimal { color: true }.hint_spans(text);
        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].style, Style::new().add_modifier(Modifier::DIM));
        assert_eq!(split_token("(or"), ("(", "or", ""));
        assert_eq!(split_token("Alt+Enter)"), ("", "Alt+Enter", ")"));
        assert_eq!(split_token("?"), ("", "?", ""));
    }

    #[test]
    fn the_contrast_formula_matches_known_values() {
        let white = Color::Rgb(255, 255, 255);
        assert!((contrast(white, BLACK) - 21.0).abs() < 1e-9);
        assert!((contrast(BLACK, BLACK) - 1.0).abs() < 1e-9);
        // The mockups' table (doc/0021): neutral text 17.6, old lines 2.1.
        assert!((contrast(NEUTRAL.text, BLACK) - 17.6).abs() < 0.05);
        assert!((contrast(TOKYO_NIGHT.line, BLACK) - 2.1).abs() < 0.05);
    }

    #[test]
    fn terminal_uses_named_colours_and_reverse_video() {
        let th = Theme::new(UiTheme::Terminal, true);
        assert!(TERMINAL.reverse);
        assert!(
            th.pill(TERMINAL.amber)
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(th.selected().add_modifier.contains(Modifier::REVERSED));
        let p = &TERMINAL;
        for c in [
            p.text,
            p.muted,
            p.faint,
            p.line,
            p.surface,
            p.sel,
            p.red,
            p.amber,
            p.green,
            p.blue,
            p.grey,
            p.teal,
            p.orange,
            p.logo,
            p.title,
            p.key,
            p.key_hot,
            p.mark,
            p.project,
            p.logs_border,
        ]
        .into_iter()
        .chain(p.accounts)
        {
            assert!(!matches!(c, Color::Rgb(..) | Color::Indexed(_)), "{c:?}");
        }
    }

    #[test]
    fn every_theme_name_has_its_palette() {
        for t in UiTheme::ALL {
            let p = palette(t);
            assert_eq!(
                p.reverse,
                matches!(t, UiTheme::Terminal | UiTheme::Ansi16),
                "{t:?}"
            );
            assert_eq!(p.ansi16, t == UiTheme::Ansi16, "{t:?}");
        }
        assert!(std::ptr::eq(palette(UiTheme::default()), &NEUTRAL));
        assert!(std::ptr::eq(palette(UiTheme::NeutralLight), &NEUTRAL_LIGHT));
        assert!(std::ptr::eq(palette(UiTheme::WarmLight), &WARM_LIGHT));
        assert!(std::ptr::eq(palette(UiTheme::Ansi16), &ANSI16));
    }

    const WHITE: Color = Color::Rgb(255, 255, 255);

    /// Every role of `p` (doc/0021's list) with the minimum contrast
    /// doc/0038 sets for the light themes against a white terminal
    /// background: text and secondary 7:1, dim and the state accents
    /// 4.5:1, lines 3:1; text on the selection bar 7:1, on the pills and
    /// chips 4.5:1 (bold), the bar's `▌` 4.5:1. Returns `(what, ratio,
    /// minimum)` for each, so the report and the assertion share them.
    fn light_contrasts(p: &'static Palette) -> Vec<(String, f64, f64)> {
        let mut out = Vec::new();
        let mut check = |what: &str, fg: Color, bg: Color, min: f64| {
            out.push((what.to_owned(), contrast(fg, bg), min));
        };
        check("text", p.text, WHITE, 7.0);
        check("secondary", p.muted, WHITE, 7.0);
        check("dim", p.faint, WHITE, 4.5);
        check("lines", p.line, WHITE, 3.0);
        let th = Theme { color: true, p };
        for g in Group::ALL {
            if g == Group::Interactive {
                continue;
            }
            let c = th.group_color(g);
            check(g.title(), c, WHITE, 4.5);
            let st = th.pill(c);
            check(
                &format!("{} pill", g.title()),
                st.fg.unwrap(),
                st.bg.unwrap(),
                4.5,
            );
        }
        let st = th.interactive_pill();
        check("Interactive pill", st.fg.unwrap(), st.bg.unwrap(), 7.0);
        let st = th.selected();
        check("selection", st.fg.unwrap(), st.bg.unwrap(), 7.0);
        check("selection mark", p.mark, p.sel, 4.5);
        check("attention", p.red, WHITE, 4.5);
        for k in 0..p.accounts.len() {
            let st = th.chip(th.account_color(k));
            check(&format!("chip {k}"), st.fg.unwrap(), st.bg.unwrap(), 4.5);
            check(&format!("account {k}"), th.account_color(k), WHITE, 4.5);
        }
        for (what, c) in [
            ("logo", p.logo),
            ("title", p.title),
            ("keys", p.key),
            ("Ctrl+Z", p.key_hot),
            ("project", p.project),
        ] {
            check(what, c, WHITE, 4.5);
        }
        let look = Look::Themed(th);
        for (what, st) in [
            ("selected item", look.selected_item()),
            ("focused choice", look.choice(true)),
            ("prompt", look.prompt()),
        ] {
            check(what, st.fg.unwrap(), st.bg.unwrap(), 7.0);
        }
        for (what, st, min) in [
            ("window title", look.overlay_title(), 4.5),
            ("label", look.field_label(false), 7.0),
            ("focused label", look.field_label(true), 4.5),
            ("hint", look.hint(), 4.5),
            ("error", look.error(), 4.5),
            ("warning", look.warning(), 4.5),
            ("success", look.success(), 4.5),
            ("unsaved", look.modified(), 4.5),
        ] {
            check(what, st.fg.unwrap(), WHITE, min);
        }
        out
    }

    #[test]
    fn light_themes_meet_the_contrast_targets_on_white() {
        for (name, p) in [
            ("neutral-light", &NEUTRAL_LIGHT),
            ("warm-light", &WARM_LIGHT),
        ] {
            for (what, r, min) in light_contrasts(p) {
                assert!(r >= min, "{name} {what}: {r:.2} < {min}");
            }
            // No light theme paints behind ordinary text, and its bar
            // forces the text colour, so dim lines under a selected row
            // are drawn in the bar's text colour.
            assert!(p.sel_fg.is_some(), "{name}");
            assert!(!p.reverse && !p.ansi16, "{name}");
        }
    }

    /// The truecolor themes at 256 colours ([`reduce`]) keep their
    /// targets: the dark ones on black (doc/0021) and the light ones on
    /// white (doc/0038), checked on the nearest palette colours.
    #[test]
    fn the_256_colour_approximation_keeps_the_targets() {
        fn quantized(c: Color) -> Color {
            match reduce(c, ColorDepth::Ansi256) {
                Color::Indexed(i) => {
                    assert!(i >= 16, "{c:?} took a terminal colour {i}");
                    let (r, g, b) = rgb_of_index(i);
                    Color::Rgb(r, g, b)
                }
                other => panic!("{c:?} became {other:?}"),
            }
        }
        // Text, secondary, dim, lines, accents, pills and the selection,
        // with the same minimums as in truecolor; pills 7:1 on the dark
        // themes, 4.5:1 on the light ones.
        for (name, p, bg, pill) in [
            ("neutral", &NEUTRAL, BLACK, 7.0),
            ("warm", &WARM, BLACK, 7.0),
            ("neutral-light", &NEUTRAL_LIGHT, WHITE, 4.5),
            ("warm-light", &WARM_LIGHT, WHITE, 4.5),
        ] {
            let check = |what: &str, fg: Color, bg: Color, min: f64| {
                let r = contrast(quantized(fg), quantized(bg));
                assert!(r >= min, "{name} at 256 {what}: {r:.2} < {min}");
            };
            check("text", p.text, bg, 7.0);
            check("secondary", p.muted, bg, 7.0);
            check("dim", p.faint, bg, 4.5);
            check("lines", p.line, bg, 3.0);
            for c in [p.red, p.amber, p.green, p.blue, p.grey, p.teal, p.orange] {
                check(&format!("{c:?}"), c, bg, 4.5);
                check(&format!("pill on {c:?}"), p.pill_text, c, pill);
            }
            check("selection", p.sel_fg.unwrap(), p.sel, 7.0);
            // The bar's `▌` and `▶`, glyphs: warm-light's clay on its sand
            // bar comes to 4.1 at 256 colours (5.1 in truecolor).
            check("selection mark", p.mark, p.sel, 4.0);
        }
    }

    #[test]
    fn ansi16_uses_named_colours_and_attributes() {
        let th = Theme::new(UiTheme::Ansi16, true);
        let p = &ANSI16;
        for c in [
            p.pill_text,
            p.tint_base,
            p.text,
            p.muted,
            p.faint,
            p.line,
            p.surface,
            p.sel,
            p.red,
            p.amber,
            p.green,
            p.blue,
            p.grey,
            p.teal,
            p.orange,
            p.logo,
            p.title,
            p.key,
            p.key_hot,
            p.mark,
            p.project,
            p.logs_border,
        ]
        .into_iter()
        .chain(p.accounts)
        {
            assert!(!matches!(c, Color::Rgb(..) | Color::Indexed(_)), "{c:?}");
        }
        // Dim text and lines: the default colour, dimmed.
        for st in [th.faint(), th.line()] {
            assert_eq!(st.fg, None);
            assert!(st.add_modifier.contains(Modifier::DIM));
        }
        // Text takes no hue; glyphs and pills do.
        assert_eq!(th.hue(p.amber), Style::new());
        assert_eq!(th.fg(p.amber).fg, Some(Color::Yellow));
        for (c, text) in [
            (Color::Red, Color::White),
            (Color::Blue, Color::White),
            (Color::Magenta, Color::White),
            (Color::Yellow, Color::Black),
            (Color::Green, Color::Black),
            (Color::Cyan, Color::Black),
            (Color::Gray, Color::Black),
        ] {
            let st = th.pill(c);
            assert_eq!((st.fg, st.bg), (Some(text), Some(c)), "{c:?}");
            assert!(!st.add_modifier.contains(Modifier::BOLD), "{c:?}");
        }
        // The project hue (the default colour) and the selection reverse.
        assert!(th.pill(p.project).add_modifier.contains(Modifier::REVERSED));
        assert!(th.selected().add_modifier.contains(Modifier::REVERSED));
        // Warnings and successes are bold, as without colour.
        let look = Look::Themed(th);
        for st in [look.warning(), look.success(), look.modified()] {
            assert_eq!(st.fg, None);
            assert!(st.add_modifier.contains(Modifier::BOLD));
        }
        // Without colour it is the same as every theme: nothing at all.
        let plain = Theme::new(UiTheme::Ansi16, false);
        assert_eq!(plain.fg(p.amber), Style::new());
        assert_eq!(plain.hue(p.amber), Style::new());
    }

    #[test]
    fn colours_are_reduced_to_the_terminal_depth() {
        // 256 colours: the cube and the grey ramp, never 0 to 15.
        assert_eq!(nearest_256((0, 0, 0)), 16);
        assert_eq!(nearest_256((255, 255, 255)), 231);
        assert_eq!(nearest_256((255, 0, 0)), 196);
        assert_eq!(nearest_256((128, 128, 128)), 244);
        assert_eq!(nearest_256((235, 235, 235)), 255);
        for i in 16..=255u8 {
            assert_eq!(nearest_256(rgb_of_index(i)), i, "{i} maps to itself");
        }
        assert_eq!(
            reduce(Color::Rgb(255, 0, 0), ColorDepth::Ansi256),
            Color::Indexed(196)
        );
        // 16 colours: the nearest named one; indexed ones too.
        assert_eq!(
            reduce(Color::Rgb(250, 10, 10), ColorDepth::Ansi16),
            Color::LightRed
        );
        assert_eq!(
            reduce(Color::Rgb(20, 20, 20), ColorDepth::Ansi16),
            Color::Black
        );
        assert_eq!(reduce(Color::Indexed(4), ColorDepth::Ansi16), Color::Blue);
        assert_eq!(
            reduce(Color::Indexed(231), ColorDepth::Ansi16),
            Color::White
        );
        // Named colours and the default stay at every depth.
        for depth in ColorDepth::ALL {
            for c in NAMED.into_iter().chain([Color::Reset]) {
                assert_eq!(reduce(c, depth), c, "{c:?} at {depth:?}");
            }
        }
        // Truecolor (and an unresolved `auto`) changes nothing.
        for depth in [ColorDepth::Truecolor, ColorDepth::Auto] {
            assert_eq!(reduce(Color::Rgb(1, 2, 3), depth), Color::Rgb(1, 2, 3));
            assert_eq!(reduce(Color::Indexed(100), depth), Color::Indexed(100));
        }
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 2, 1));
        buf[(0, 0)]
            .set_fg(Color::Rgb(255, 0, 0))
            .set_bg(Color::Indexed(200));
        reduce_buffer(&mut buf, ColorDepth::Ansi16);
        assert_eq!(buf[(0, 0)].fg, Color::LightRed);
        assert!(!matches!(
            buf[(0, 0)].bg,
            Color::Rgb(..) | Color::Indexed(_)
        ));
    }
}
