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
//!
//! doc/0046 adds palettes published by other projects, with each source and
//! licence. Colour values only; a value that had to move to reach a contrast
//! target is listed there, next to the one published.

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

/// A colour written as its published hex value: `hex(0x1e1e2e)`.
const fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
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

/// A published palette (doc/0046) in tower's roles: its own colours mapped by
/// meaning, state pills in the palette's background colour, the detail panel's
/// border in the subject's state colour.
struct Published {
    /// The background it was made for, which tower does not paint: the text
    /// on the state pills.
    bg: Color,
    tint_base: Color,
    tint: f32,
    text: Color,
    muted: Color,
    faint: Color,
    line: Color,
    surface: Color,
    sel: Color,
    sel_fg: Color,
    red: Color,
    amber: Color,
    green: Color,
    blue: Color,
    grey: Color,
    teal: Color,
    orange: Color,
    logo: Color,
    title: Color,
    key: Color,
    key_hot: Color,
    mark: Color,
    project: Color,
    logs_border: Color,
    accounts: [Color; 4],
}

const fn published(p: Published) -> Palette {
    Palette {
        pill_text: p.bg,
        tint_base: p.tint_base,
        text: p.text,
        muted: p.muted,
        faint: p.faint,
        line: p.line,
        surface: p.surface,
        sel: p.sel,
        sel_fg: Some(p.sel_fg),
        red: p.red,
        amber: p.amber,
        green: p.green,
        blue: p.blue,
        grey: p.grey,
        teal: p.teal,
        orange: p.orange,
        logo: p.logo,
        title: p.title,
        key: p.key,
        key_hot: p.key_hot,
        mark: p.mark,
        project: p.project,
        logs_border: p.logs_border,
        detail_border_group: true,
        accounts: p.accounts,
        chip: Chip::Tint(p.tint),
        reverse: false,
        ansi16: false,
    }
}

/// Catppuccin Mocha: every value as published.
pub static CATPPUCCIN_MOCHA: Palette = {
    const BASE: Color = hex(0x1e1e2e);
    const SURFACE0: Color = hex(0x313244);
    const OVERLAY2: Color = hex(0x9399b2);
    const YELLOW: Color = hex(0xf9e2af);
    const MAUVE: Color = hex(0xcba6f7);
    const PEACH: Color = hex(0xfab387);
    const LAVENDER: Color = hex(0xb4befe);
    const TEXT: Color = hex(0xcdd6f4);
    published(Published {
        bg: BASE,
        tint_base: BASE,
        tint: 0.2,
        text: TEXT,
        muted: hex(0xbac2de),
        faint: OVERLAY2,
        line: hex(0x6c7086),
        surface: SURFACE0,
        sel: SURFACE0,
        sel_fg: TEXT,
        red: hex(0xf38ba8),
        amber: YELLOW,
        green: hex(0xa6e3a1),
        blue: hex(0x89b4fa),
        grey: OVERLAY2,
        teal: MAUVE,
        orange: PEACH,
        logo: MAUVE,
        title: LAVENDER,
        key: LAVENDER,
        key_hot: YELLOW,
        mark: LAVENDER,
        project: LAVENDER,
        logs_border: LAVENDER,
        accounts: [hex(0x74c7ec), hex(0xf5c2e7), hex(0x94e2d5), PEACH],
    })
};

/// Dracula: no blue, so Working is its cyan; its comment colour is lines, and
/// the other greys are its foreground over that comment colour.
pub static DRACULA: Palette = {
    const BG: Color = hex(0x282a36);
    const FG: Color = hex(0xf8f8f2);
    const GREY: Color = hex(0x8491b6);
    const CYAN: Color = hex(0x8be9fd);
    const GREEN: Color = hex(0x50fa7b);
    const ORANGE: Color = hex(0xffb86c);
    const PINK: Color = hex(0xff79c6);
    const PURPLE: Color = hex(0xbd93f9);
    const YELLOW: Color = hex(0xf1fa8c);
    published(Published {
        bg: BG,
        tint_base: BG,
        tint: 0.1,
        text: FG,
        muted: hex(0xb0b8cd),
        faint: GREY,
        line: hex(0x6272a4),
        surface: hex(0x44475a),
        // Its selection colour a little towards the background, for the red
        // and the grey outline on a selected card.
        sel: hex(0x404355),
        sel_fg: FG,
        red: hex(0xff5555),
        amber: YELLOW,
        green: GREEN,
        blue: CYAN,
        grey: GREY,
        teal: PURPLE,
        orange: ORANGE,
        logo: PINK,
        title: PURPLE,
        key: PINK,
        key_hot: YELLOW,
        mark: YELLOW,
        project: PURPLE,
        logs_border: PURPLE,
        accounts: [CYAN, PINK, GREEN, ORANGE],
    })
};

/// Gruvbox dark, medium contrast: its blue sits next to its grey, so Working
/// is its purple. Its red moves a little to reach 4.5:1.
pub static GRUVBOX_DARK: Palette = {
    const BG1: Color = hex(0x3c3836);
    const FG4: Color = hex(0xa89984);
    const YELLOW: Color = hex(0xfabd2f);
    const BLUE: Color = hex(0x83a598);
    const PURPLE: Color = hex(0xd3869b);
    const AQUA: Color = hex(0x8ec07c);
    const ORANGE: Color = hex(0xfe8019);
    published(Published {
        bg: hex(0x282828),
        tint_base: hex(0x1d2021),
        tint: 0.15,
        text: hex(0xebdbb2),
        muted: hex(0xd5c4a1),
        faint: FG4,
        line: hex(0x7c6f64),
        surface: BG1,
        sel: BG1,
        sel_fg: hex(0xfbf1c7),
        red: hex(0xfe513c),
        amber: YELLOW,
        green: hex(0xb8bb26),
        blue: PURPLE,
        grey: FG4,
        teal: AQUA,
        orange: ORANGE,
        logo: ORANGE,
        title: YELLOW,
        key: ORANGE,
        key_hot: YELLOW,
        mark: ORANGE,
        project: AQUA,
        logs_border: BLUE,
        accounts: [BLUE, PURPLE, AQUA, YELLOW],
    })
};

/// Kanagawa, the wave variant: red is its peach red, as its error reds are too
/// dark on its background; the grey is its fuji grey lifted to 4.5:1.
pub static KANAGAWA: Palette = {
    const FUJI_WHITE: Color = hex(0xdcd7ba);
    const CARP: Color = hex(0xe6c384);
    const CRYSTAL: Color = hex(0x7e9cd8);
    const SPRING_BLUE: Color = hex(0x7fb4ca);
    const AQUA: Color = hex(0x7aa89f);
    const SURIMI: Color = hex(0xffa066);
    published(Published {
        bg: hex(0x1f1f28),
        tint_base: hex(0x16161d),
        tint: 0.15,
        text: FUJI_WHITE,
        muted: hex(0xc8c093),
        faint: hex(0x938aa9),
        line: hex(0x727169),
        surface: hex(0x363646),
        sel: hex(0x223249),
        sel_fg: FUJI_WHITE,
        red: hex(0xff5d62),
        amber: CARP,
        green: hex(0x98bb6c),
        blue: CRYSTAL,
        grey: hex(0x88877f),
        teal: AQUA,
        orange: SURIMI,
        logo: CRYSTAL,
        title: CRYSTAL,
        key: SPRING_BLUE,
        key_hot: CARP,
        mark: CARP,
        project: SPRING_BLUE,
        logs_border: CRYSTAL,
        accounts: [SPRING_BLUE, hex(0xd27e99), AQUA, SURIMI],
    })
};

/// Nord: the frost and snow colours as published; its red, orange and purple
/// are lifted, and the greys are its snow over its polar night, as nord3 is
/// 1.7:1 on nord0. The look stays, but its red reads as salmon.
pub static NORD: Palette = {
    const NORD0: Color = hex(0x2e3440);
    const GREY: Color = hex(0x979da9);
    const FROST: Color = hex(0x88c0d0);
    const FROST_BLUE: Color = hex(0x81a1c1);
    const PURPLE: Color = hex(0xc19bba);
    const YELLOW: Color = hex(0xebcb8b);
    const SNOW: Color = hex(0xeceff4);
    published(Published {
        bg: NORD0,
        tint_base: NORD0,
        tint: 0.05,
        text: hex(0xe5e9f0),
        muted: hex(0xd8dee9),
        faint: GREY,
        line: hex(0x797f8a),
        surface: hex(0x3b4252),
        sel: hex(0x434c5e),
        sel_fg: SNOW,
        red: hex(0xe28088),
        amber: YELLOW,
        green: hex(0xa3be8c),
        blue: FROST,
        grey: GREY,
        teal: PURPLE,
        orange: hex(0xe79c84),
        logo: FROST,
        title: FROST,
        key: FROST,
        key_hot: YELLOW,
        mark: SNOW,
        project: FROST_BLUE,
        logs_border: FROST_BLUE,
        accounts: [FROST, PURPLE, hex(0x8fbcbb), YELLOW],
    })
};

/// One Dark: its text moves to reach 7:1 for secondary text and a step above it
/// for text, its red a little to reach 4.5:1, its two comment greys more; the
/// selected row's text is lighter, for the bar.
pub static ONE_DARK: Palette = {
    const BG: Color = hex(0x282c34);
    const SECONDARY: Color = hex(0xb1b8c5);
    const GREY: Color = hex(0x8c93a1);
    const VISUAL: Color = hex(0x3e4452);
    const YELLOW: Color = hex(0xe5c07b);
    const GREEN: Color = hex(0x98c379);
    const BLUE: Color = hex(0x61afef);
    const PURPLE: Color = hex(0xc678dd);
    const DARK_YELLOW: Color = hex(0xd19a66);
    published(Published {
        bg: BG,
        tint_base: BG,
        tint: 0.1,
        text: hex(0xb9c0cd),
        muted: SECONDARY,
        faint: GREY,
        line: hex(0x6e7583),
        surface: VISUAL,
        sel: VISUAL,
        sel_fg: hex(0xe6e9ef),
        red: hex(0xe36f78),
        amber: YELLOW,
        green: GREEN,
        blue: BLUE,
        grey: GREY,
        teal: PURPLE,
        orange: DARK_YELLOW,
        logo: BLUE,
        title: BLUE,
        key: BLUE,
        key_hot: YELLOW,
        mark: YELLOW,
        project: PURPLE,
        logs_border: BLUE,
        accounts: [hex(0x56b6c2), BLUE, GREEN, DARK_YELLOW],
    })
};

/// Rosé Pine: no green, so Ready for review is its foam and Working its iris;
/// secondary text is its text over its base.
pub static ROSE_PINE: Palette = {
    const BASE: Color = hex(0x191724);
    const TEXT: Color = hex(0xe0def4);
    const SUBTLE: Color = hex(0x908caa);
    const GOLD: Color = hex(0xf6c177);
    const ROSE: Color = hex(0xebbcba);
    const FOAM: Color = hex(0x9ccfd8);
    const IRIS: Color = hex(0xc4a7e7);
    published(Published {
        bg: BASE,
        tint_base: BASE,
        tint: 0.2,
        text: TEXT,
        muted: hex(0xa4a2b6),
        faint: SUBTLE,
        line: hex(0x6e6a86),
        surface: hex(0x26233a),
        sel: hex(0x403d52),
        sel_fg: TEXT,
        red: hex(0xeb6f92),
        amber: GOLD,
        green: FOAM,
        blue: IRIS,
        grey: SUBTLE,
        teal: hex(0x95b1ac),
        orange: ROSE,
        logo: ROSE,
        title: IRIS,
        key: IRIS,
        key_hot: GOLD,
        mark: ROSE,
        project: FOAM,
        logs_border: IRIS,
        accounts: [FOAM, IRIS, ROSE, GOLD],
    })
};

/// Catppuccin Latte: red, mauve and the greys as published, its text secondary
/// with a darker step above it; yellow, green and peach are much deeper, as no
/// light background keeps them at 4.5:1. At the limit of what doc/0046 lets a
/// palette move.
pub static CATPPUCCIN_LATTE: Palette = {
    const BASE: Color = hex(0xeff1f5);
    const TEXT: Color = hex(0x4c4f69);
    const SUBTEXT1: Color = hex(0x5c5f77);
    const YELLOW: Color = hex(0x9b5f02);
    const GREEN: Color = hex(0x217f02);
    const BLUE: Color = hex(0x1b63f1);
    const MAUVE: Color = hex(0x8839ef);
    const PEACH: Color = hex(0xbd4700);
    published(Published {
        bg: BASE,
        tint_base: hex(0xffffff),
        tint: 0.05,
        text: hex(0x3f425c),
        muted: TEXT,
        faint: SUBTEXT1,
        line: hex(0x7c7f93),
        surface: hex(0xdce0e8),
        sel: hex(0xccd0da),
        sel_fg: hex(0x2a2c3e),
        red: hex(0xd20f39),
        amber: YELLOW,
        green: GREEN,
        blue: BLUE,
        grey: SUBTEXT1,
        teal: MAUVE,
        orange: PEACH,
        logo: MAUVE,
        title: MAUVE,
        key: BLUE,
        key_hot: YELLOW,
        mark: TEXT,
        project: BLUE,
        logs_border: BLUE,
        accounts: [hex(0x037a81), MAUVE, GREEN, PEACH],
    })
};

/// Gruvbox light, medium contrast, in its faded accents: Ready for review is
/// its aqua, as its green is next to its yellow; the yellow is deeper.
pub static GRUVBOX_LIGHT: Palette = {
    const BG1: Color = hex(0xebdbb2);
    const FG3: Color = hex(0x665c54);
    const YELLOW: Color = hex(0x996102);
    const BLUE: Color = hex(0x076678);
    const PURPLE: Color = hex(0x8f3f71);
    const AQUA: Color = hex(0x407956);
    const ORANGE: Color = hex(0xaf3a03);
    published(Published {
        bg: hex(0xfbf1c7),
        tint_base: hex(0xffffff),
        tint: 0.08,
        text: hex(0x3c3836),
        muted: hex(0x504945),
        faint: FG3,
        line: hex(0x928374),
        surface: BG1,
        sel: BG1,
        sel_fg: hex(0x282828),
        red: hex(0x9d0006),
        amber: YELLOW,
        green: AQUA,
        blue: BLUE,
        grey: FG3,
        teal: PURPLE,
        orange: ORANGE,
        logo: ORANGE,
        title: ORANGE,
        key: BLUE,
        key_hot: YELLOW,
        mark: PURPLE,
        project: AQUA,
        logs_border: BLUE,
        accounts: [BLUE, PURPLE, AQUA, YELLOW],
    })
};

/// One Light: its accents move to reach 4.5:1 and its red a little further, so
/// that it stays apart from the green in deuteranopia.
pub static ONE_LIGHT: Palette = {
    const TEXT: Color = hex(0x383a42);
    const MONO2: Color = hex(0x696c77);
    const YELLOW: Color = hex(0xa86201);
    const GREEN: Color = hex(0x318332);
    const BLUE: Color = hex(0x356ce5);
    const PURPLE: Color = hex(0xa626a4);
    published(Published {
        bg: hex(0xfafafa),
        tint_base: hex(0xffffff),
        tint: 0.04,
        text: TEXT,
        muted: hex(0x4c4e57),
        faint: MONO2,
        line: hex(0x909096),
        surface: hex(0xe8e8e9),
        sel: hex(0xe5e5e6),
        sel_fg: TEXT,
        red: hex(0xb30f3c),
        amber: YELLOW,
        green: GREEN,
        blue: BLUE,
        grey: MONO2,
        teal: PURPLE,
        orange: hex(0xce4136),
        logo: BLUE,
        title: BLUE,
        key: BLUE,
        key_hot: YELLOW,
        mark: PURPLE,
        project: PURPLE,
        logs_border: BLUE,
        accounts: [hex(0x037aae), PURPLE, GREEN, YELLOW],
    })
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
        UiTheme::CatppuccinMocha => &CATPPUCCIN_MOCHA,
        UiTheme::Dracula => &DRACULA,
        UiTheme::GruvboxDark => &GRUVBOX_DARK,
        UiTheme::Kanagawa => &KANAGAWA,
        UiTheme::Nord => &NORD,
        UiTheme::OneDark => &ONE_DARK,
        UiTheme::RosePine => &ROSE_PINE,
        UiTheme::CatppuccinLatte => &CATPPUCCIN_LATTE,
        UiTheme::GruvboxLight => &GRUVBOX_LIGHT,
        UiTheme::OneLight => &ONE_LIGHT,
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
            assert_eq!(
                t.shown_at(ColorDepth::Ansi16),
                if p.reverse { t } else { UiTheme::Ansi16 },
                "{t:?} at 16 colours"
            );
        }
        // No two names share a palette.
        for (i, a) in UiTheme::ALL.iter().enumerate() {
            for b in &UiTheme::ALL[i + 1..] {
                assert!(!std::ptr::eq(palette(*a), palette(*b)), "{a:?} {b:?}");
            }
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

    /// The background each published palette (doc/0046) was made for, which
    /// tower does not paint, and the plain one a terminal is likelier to have.
    static PUBLISHED: [(&str, &Palette, Color, Color); 10] = [
        ("catppuccin-mocha", &CATPPUCCIN_MOCHA, hex(0x1e1e2e), BLACK),
        ("dracula", &DRACULA, hex(0x282a36), BLACK),
        ("gruvbox-dark", &GRUVBOX_DARK, hex(0x282828), BLACK),
        ("kanagawa", &KANAGAWA, hex(0x1f1f28), BLACK),
        ("nord", &NORD, hex(0x2e3440), BLACK),
        ("one-dark", &ONE_DARK, hex(0x282c34), BLACK),
        ("rose-pine", &ROSE_PINE, hex(0x191724), BLACK),
        ("catppuccin-latte", &CATPPUCCIN_LATTE, hex(0xeff1f5), WHITE),
        ("gruvbox-light", &GRUVBOX_LIGHT, hex(0xfbf1c7), WHITE),
        ("one-light", &ONE_LIGHT, hex(0xfafafa), WHITE),
    ];

    /// What a role of a published palette is drawn on: the terminal's own
    /// background, or one tower paints.
    #[derive(Clone, Copy)]
    enum On {
        Terminal,
        Painted(Color),
    }

    /// Every role of `p` with doc/0046's target: text and secondary 7:1, dim
    /// and accents 4.5:1, lines 3:1, text on the bar 7:1, pills and chips 4.5:1,
    /// the bar's `▌` / `▶` 4.5:1. The state colours also stand on the bar, as a
    /// selected card's outline and its `●`: 3:1, for graphics.
    fn published_roles(p: &'static Palette) -> Vec<(String, Color, On, f64)> {
        let th = Theme { color: true, p };
        let sel_fg = p.sel_fg.expect("a published palette sets the bar's text");
        let mut out: Vec<(String, Color, On, f64)> = vec![
            ("text".to_owned(), p.text, On::Terminal, 7.0),
            ("secondary".to_owned(), p.muted, On::Terminal, 7.0),
            ("dim".to_owned(), p.faint, On::Terminal, 4.5),
            ("lines".to_owned(), p.line, On::Terminal, 3.0),
            ("logs border".to_owned(), p.logs_border, On::Terminal, 3.0),
            ("selection".to_owned(), sel_fg, On::Painted(p.sel), 7.0),
            ("mark".to_owned(), p.mark, On::Painted(p.sel), 4.5),
            (
                "Interactive pill".to_owned(),
                p.text,
                On::Painted(p.surface),
                4.5,
            ),
        ];
        for g in Group::ALL {
            if g == Group::Interactive {
                continue;
            }
            let c = th.group_color(g);
            out.push((g.title().to_owned(), c, On::Terminal, 4.5));
            let pill = format!("{} pill", g.title());
            out.push((pill, p.pill_text, On::Painted(c), 4.5));
            let bar = format!("{} on the bar", g.title());
            out.push((bar, c, On::Painted(p.sel), 3.0));
        }
        for k in 0..p.accounts.len() {
            let c = th.account_color(k);
            let st = th.chip(c);
            out.push((format!("account {k}"), c, On::Terminal, 4.5));
            let bg = st.bg.expect("a published palette tints its chips");
            out.push((format!("chip {k}"), c, On::Painted(bg), 4.5));
        }
        for (what, c) in [
            ("logo", p.logo),
            ("title", p.title),
            ("keys", p.key),
            ("Ctrl+Z", p.key_hot),
            ("project", p.project),
        ] {
            out.push((what.to_owned(), c, On::Terminal, 4.5));
        }
        out
    }

    /// doc/0046's targets on each palette's own background and on plain black
    /// or white, which only adds contrast, and in the windows' styles.
    #[test]
    fn published_themes_meet_the_targets_on_their_background_and_a_plain_one() {
        for (name, p, own, plain) in PUBLISHED {
            for (what, c, on, min) in published_roles(p) {
                let (r, r_plain) = match on {
                    On::Terminal => (contrast(c, own), contrast(c, plain)),
                    On::Painted(b) => (contrast(c, b), contrast(c, b)),
                };
                assert!(r >= min, "{name} {what}: {r:.2} < {min}");
                assert!(r_plain >= r, "{name} {what}: {r_plain:.2} on plain, {r:.2}");
            }
            let look = Look::Themed(Theme { color: true, p });
            for (what, st) in [
                ("selected item", look.selected_item()),
                ("focused choice", look.choice(true)),
                ("prompt", look.prompt()),
            ] {
                let r = contrast(st.fg.unwrap(), st.bg.unwrap());
                assert!(r >= 7.0, "{name} {what}: {r:.2}");
            }
            for (what, st, min) in [
                ("label", look.field_label(false), 7.0),
                ("focused label", look.field_label(true), 4.5),
                ("hint", look.hint(), 4.5),
                ("error", look.error(), 4.5),
                ("warning", look.warning(), 4.5),
                ("success", look.success(), 4.5),
                ("unsaved", look.modified(), 4.5),
            ] {
                let r = contrast(st.fg.unwrap(), own);
                assert!(r >= min, "{name} {what}: {r:.2} < {min}");
            }
            assert!(!p.reverse && !p.ansi16, "{name}");
        }
    }

    /// Prints doc/0046's tables: `cargo test contrast_table -- --nocapture`.
    /// The weakest role of each column, in truecolor (asserted above) and at
    /// 256 colours (asserted below).
    #[test]
    fn contrast_table() {
        for (depth, q) in [
            ("truecolor", (|c| c) as fn(Color) -> Color),
            ("256", at_256 as fn(Color) -> Color),
        ] {
            println!("{depth}:");
            for (name, p, own, plain) in PUBLISHED {
                let roles = published_roles(p);
                let ratio = |c: Color, on: On| match on {
                    On::Terminal => contrast(q(c), own),
                    On::Painted(b) => contrast(q(c), q(b)),
                };
                let weakest = |column: &[&str]| {
                    roles
                        .iter()
                        .filter(|(what, ..)| column.iter().any(|c| what.contains(c)))
                        .map(|(_, c, on, _)| ratio(*c, *on))
                        .fold(f64::MAX, f64::min)
                };
                let accent = [p.amber, p.blue, p.green, p.grey, p.teal, p.orange, p.red]
                    .into_iter()
                    .map(|c| ratio(c, On::Terminal))
                    .fold(f64::MAX, f64::min);
                let five = [p.amber, p.blue, p.green, p.grey, p.red];
                let mut apart_min = f64::MAX;
                let mut protan_min = f64::MAX;
                let mut deutan_min = f64::MAX;
                for (i, a) in five.iter().enumerate() {
                    for b in &five[i + 1..] {
                        apart_min = apart_min.min(apart(q(*a), q(*b), None));
                        protan_min = protan_min.min(apart(q(*a), q(*b), Some(&PROTAN)));
                        deutan_min = deutan_min.min(apart(q(*a), q(*b), Some(&DEUTAN)));
                    }
                }
                println!(
                    "| {name} | {:.2} | {:.2} | {:.2} | {:.2} | {accent:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {apart_min:.1} | {protan_min:.1} | {deutan_min:.1} |",
                    ratio(p.text, On::Terminal),
                    ratio(p.muted, On::Terminal),
                    ratio(p.faint, On::Terminal),
                    ratio(p.line, On::Terminal),
                    ratio(p.red, On::Terminal),
                    weakest(&["selection"]),
                    weakest(&["mark"]),
                    weakest(&[" pill"]),
                    weakest(&["chip"]),
                    weakest(&["on the bar"]),
                    contrast(q(p.text), plain),
                );
            }
        }
    }

    /// `c` in OKLab, scaled so that 1 is about one just noticeable step.
    fn oklab(c: [f64; 3]) -> [f64; 3] {
        let l = (0.412_221_47 * c[0] + 0.536_332_54 * c[1] + 0.051_445_99 * c[2]).cbrt();
        let m = (0.211_903_5 * c[0] + 0.680_699_55 * c[1] + 0.107_396_96 * c[2]).cbrt();
        let s = (0.088_302_46 * c[0] + 0.281_718_84 * c[1] + 0.629_978_7 * c[2]).cbrt();
        [
            100.0 * (0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_05 * s),
            100.0 * (1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s),
            100.0 * (0.025_904_04 * l + 0.782_771_77 * m - 0.808_675_77 * s),
        ]
    }

    /// How far apart two colours look, through a colour-vision matrix or none.
    fn apart(a: Color, b: Color, vision: Option<&[[f64; 3]; 3]>) -> f64 {
        let see = |c: Color| {
            let Color::Rgb(r, g, b) = c else {
                panic!("{c:?} is not RGB");
            };
            let l = [r, g, b].map(|v| {
                let s = f64::from(v) / 255.0;
                if s <= 0.040_45 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            });
            oklab(match vision {
                None => l,
                Some(m) => [0, 1, 2]
                    .map(|i| (m[i][0] * l[0] + m[i][1] * l[1] + m[i][2] * l[2]).clamp(0.0, 1.0)),
            })
        };
        let (x, y) = (see(a), see(b));
        (0..3).map(|i| (x[i] - y[i]).powi(2)).sum::<f64>().sqrt()
    }

    /// Protanopia and deuteranopia (Machado, Oliveira and Fernandes 2009,
    /// severity 1).
    const PROTAN: [[f64; 3]; 3] = [
        [0.152_286, 1.052_583, -0.204_868],
        [0.114_503, 0.786_281, 0.099_216],
        [-0.003_882, -0.048_116, 1.051_998],
    ];
    const DEUTAN: [[f64; 3]; 3] = [
        [0.367_322, 0.860_646, -0.227_968],
        [0.280_085, 0.672_501, 0.047_413],
        [-0.011_820, 0.042_940, 0.968_881],
    ];

    /// `c` as the nearest of the 256 colours.
    fn at_256(c: Color) -> Color {
        match reduce(c, ColorDepth::Ansi256) {
            Color::Indexed(i) => {
                assert!(i >= 16, "{c:?} took a terminal colour {i}");
                let (r, g, b) = rgb_of_index(i);
                Color::Rgb(r, g, b)
            }
            other => panic!("{c:?} became {other:?}"),
        }
    }

    /// The five states a user tells apart on a card's outline (doc/0046): at
    /// least 9 apart, 6 at 256 colours. Under protanopia and deuteranopia no
    /// pair collapses (2) and Failed stays off Ready for review (3); words and
    /// glyphs carry the state either way. The two other groups stay off the
    /// five (5).
    #[test]
    fn published_states_stay_distinct() {
        for (name, p, ..) in PUBLISHED {
            let five = [
                ("Needs me", p.amber),
                ("Working", p.blue),
                ("Ready", p.green),
                ("Failed", p.red),
                ("Stopped", p.grey),
            ];
            for (i, (an, a)) in five.iter().enumerate() {
                for (bn, b) in &five[i + 1..] {
                    let d = apart(*a, *b, None);
                    assert!(d >= 9.0, "{name} {an}/{bn}: {d:.1}");
                    let d = apart(at_256(*a), at_256(*b), None);
                    assert!(d >= 6.0, "{name} at 256 {an}/{bn}: {d:.1}");
                    for (vision, m) in [("protan", &PROTAN), ("deutan", &DEUTAN)] {
                        let d = apart(*a, *b, Some(m));
                        assert!(d >= 2.0, "{name} {an}/{bn} {vision}: {d:.1}");
                    }
                }
                for (bn, b) in [("Orchestrator", p.teal), ("Other", p.orange)] {
                    let d = apart(*a, b, None);
                    assert!(d >= 5.0, "{name} {an}/{bn}: {d:.1}");
                }
            }
            for (vision, m) in [("protan", &PROTAN), ("deutan", &DEUTAN)] {
                let d = apart(p.red, p.green, Some(m));
                assert!(d >= 3.0, "{name} Failed/Ready {vision}: {d:.1}");
            }
        }
    }

    /// doc/0046's floors at 256 colours, which are opt-in: the cube has few
    /// values near these mid-tones, and truecolor keeps the published values
    /// rather than bending them towards it. Only what tower paints is reduced;
    /// the terminal's background stays what it is.
    #[test]
    fn the_256_colour_approximation_of_the_published_themes() {
        for (name, p, own, _) in PUBLISHED {
            for (what, c, on, min) in published_roles(p) {
                let r = match on {
                    On::Terminal => contrast(at_256(c), own),
                    On::Painted(b) => contrast(at_256(c), at_256(b)),
                };
                let floor = if min >= 7.0 {
                    6.0
                } else if min >= 4.5 {
                    3.5
                } else {
                    2.5
                };
                assert!(r >= floor, "{name} at 256 {what}: {r:.2} < {floor}");
            }
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
