//! The dashboard style's colour themes (doc/0020, doc/0021): a [`Palette`]
//! of colour roles per theme, and the [`Theme`] the renderers draw with,
//! whose style helpers drop every colour under `NO_COLOR` (doc/0007), so
//! only bold, dim and reversed remain.
//!
//! Nothing paints a background behind ordinary text: painted backgrounds
//! are only the selected row, the state pills and the account chips.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::config::UiTheme;
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
    }
};

/// The palette of `theme`.
pub fn palette(theme: UiTheme) -> &'static Palette {
    match theme {
        UiTheme::Neutral => &NEUTRAL,
        UiTheme::Warm => &WARM,
        UiTheme::Terminal => &TERMINAL,
        UiTheme::TokyoNight => &TOKYO_NIGHT,
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

    /// Foreground `c`.
    pub fn fg(self, c: Color) -> Style {
        if self.color {
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
            Style::new().fg(self.p.faint)
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
        }
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
            assert_eq!(p.reverse, t == UiTheme::Terminal, "{t:?}");
        }
        assert!(std::ptr::eq(palette(UiTheme::default()), &NEUTRAL));
    }
}
