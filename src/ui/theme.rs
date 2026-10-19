//! The dashboard style's palette (doc/0020): a dark truecolor palette, and
//! style helpers that drop every colour under `NO_COLOR` (doc/0007), so
//! only bold, dim and reversed remain.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::groups::{Group, GroupKey};

/// Dark text on solid pills.
pub const BASE: Color = Color::Rgb(26, 27, 38);
pub const TEXT: Color = Color::Rgb(192, 202, 245);
pub const MUTED: Color = Color::Rgb(115, 122, 162);
pub const FAINT: Color = Color::Rgb(84, 92, 126);
pub const LINE: Color = Color::Rgb(59, 66, 97);
pub const SURF3: Color = Color::Rgb(52, 59, 88);
/// Background of the selected row.
pub const SEL: Color = Color::Rgb(45, 63, 118);

pub const RED: Color = Color::Rgb(247, 118, 142);
pub const AMBER: Color = Color::Rgb(224, 175, 104);
pub const GREEN: Color = Color::Rgb(158, 206, 106);
pub const BLUE: Color = Color::Rgb(122, 162, 247);
pub const CYAN: Color = Color::Rgb(125, 207, 255);
pub const MAGENTA: Color = Color::Rgb(187, 154, 247);
pub const GREY: Color = Color::Rgb(130, 137, 175);
pub const TEAL: Color = Color::Rgb(115, 218, 202);
pub const ORANGE: Color = Color::Rgb(255, 158, 100);

/// Colours of the account groups, in the order the groups first appear in
/// the config; they repeat after the last.
const ACCOUNT_COLORS: [Color; 4] = [CYAN, MAGENTA, TEAL, ORANGE];

/// The hue of a state group.
pub fn group_color(g: Group) -> Color {
    match g {
        Group::Orchestrator => TEAL,
        Group::NeedsMe => AMBER,
        Group::Failed => RED,
        Group::ReadyForReview => GREEN,
        Group::Working => BLUE,
        Group::Other => ORANGE,
        Group::Stopped => GREY,
        Group::Interactive => FAINT,
    }
}

/// The hue of a list group: its state group's, or blue for a project.
pub fn key_color(key: &GroupKey) -> Color {
    match key {
        GroupKey::State(g) => group_color(*g),
        GroupKey::Project(_) => BLUE,
    }
}

/// The colour of account group number `index` (in config order).
pub fn account_color(index: usize) -> Color {
    ACCOUNT_COLORS[index % ACCOUNT_COLORS.len()]
}

/// `c` mixed into the dark base: `amount` 0 is the base, 1 is `c`.
pub fn tint(c: Color, amount: f32) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) = (c, BASE) else {
        return c;
    };
    let m = |x: u8, y: u8| (f32::from(y) + (f32::from(x) - f32::from(y)) * amount).round() as u8;
    Color::Rgb(m(r, br), m(g, bg), m(b, bb))
}

/// Styles that honour `NO_COLOR`: with `color` false every colour is
/// dropped and only modifiers remain.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
}

impl Theme {
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
            Style::new().fg(FAINT)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// Muted text, for labels; plain without colour.
    pub fn muted(self) -> Style {
        self.fg(MUTED)
    }

    /// The attention style (doc/0007): bold, plus the accent with colour.
    pub fn attention(self) -> Style {
        self.bold(RED)
    }

    /// Panel borders and rules.
    pub fn line(self) -> Style {
        self.fg(LINE)
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
