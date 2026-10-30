//! The workspace's list bar (doc/0045): the list's groups, order, folding and
//! selection, with each session as a card, `[ui] card_size` tall. The outline
//! has the state's colour; the selected card's is heavy, and the whole card
//! sits on the selection's background. Cards show the account, not git; a
//! card's subagents are the `+N` badge on its title. `[ui] card_gap` keeps
//! cards and groups apart.
//!
//! Tells the app which entry each screen row shows, for clicks; a gap shows
//! none. The viewport moves by whole entries, and a card that does not fit is
//! not drawn.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::dashboard::{
    Level, RowText, account_chip, put, put_lr, row, row_text, scroll_lines, separator, spinner,
};
use super::theme::{Lb, Theme, rfit};
use super::{MARKER, age_column, fit, shorten};
use crate::app::{App, Entry, PaneFocus};
use crate::config::CardSize;
use crate::groups::{self, Group, Grouping};

/// One outline's glyphs.
struct Outline {
    top_left: &'static str,
    top_right: &'static str,
    bottom_left: &'static str,
    bottom_right: &'static str,
    across: &'static str,
    down: &'static str,
}

const ROUND: Outline = Outline {
    top_left: "╭",
    top_right: "╮",
    bottom_left: "╰",
    bottom_right: "╯",
    across: "─",
    down: "│",
};

const HEAVY: Outline = Outline {
    top_left: "┏",
    top_right: "┓",
    bottom_left: "┗",
    bottom_right: "┛",
    across: "━",
    down: "┃",
};

/// Interactive rows (doc/0006): they cannot be attached.
const DASHED: Outline = Outline {
    top_left: "╭",
    top_right: "╮",
    bottom_left: "╰",
    bottom_right: "╯",
    across: "┄",
    down: "┆",
};

/// The gap kept between two texts on a card's row.
const GAP: usize = 2;

/// A cut text keeps at least this many cells.
const CUT_MIN: usize = 5;

/// A name keeps at least this many cells before the `+N` badge goes.
const NAME_MIN: usize = 6;

/// The state and the folder on one row of `w` cells, with [`GAP`] between:
/// the folder first becomes `…/last`, then the state its short form, then the
/// folder and the state are cut with `…`. On a card too narrow for both, the
/// folder goes.
fn fit_state_and_folder(app: &App, i: usize, w: usize) -> (String, String) {
    let mut shortest = None;
    for level in [Level::Full, Level::ShortProject, Level::ShortState] {
        let t = row_text(app, i, level);
        if t.state.width() + GAP + t.project.width() <= w {
            return (t.state, t.project);
        }
        shortest = Some(t);
    }
    let t: RowText = shortest.expect("three levels");
    let (mut state, mut folder) = (t.state.width(), t.project.width());
    let mut over = (state + GAP + folder).saturating_sub(w);
    for column in [&mut folder, &mut state] {
        let cut = over.min(column.saturating_sub(CUT_MIN));
        *column -= cut;
        over -= cut;
    }
    if over > 0 {
        return (shorten(&t.state, w), String::new());
    }
    (shorten(&t.state, state), shorten(&t.project, folder))
}

/// Puts the selected card on the selection's background across its whole
/// width and height, in every look (doc/0045: an exception to doc/0021's
/// selected row). Reverse video where the theme selects that way. What already
/// has a background, an account chip, keeps it; every foreground stays, so the
/// outline keeps the state's colour.
fn fill_selected(buf: &mut Buffer, area: Rect, th: Theme) {
    let bar = th.selected();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &mut buf[(x, y)];
            if let Some(bg) = bar.bg
                && cell.bg == Color::Reset
            {
                cell.set_bg(bg);
            }
            cell.modifier.insert(bar.add_modifier);
        }
    }
}

/// One card, with what its rows share.
struct Card<'a> {
    app: &'a App,
    i: usize,
    width: usize,
    selected: bool,
    lit: bool,
    dim: bool,
    busy: bool,
    /// Medium and large; small has an edge instead.
    outlined: bool,
    color: Color,
    spin: &'static str,
    age: String,
    th: Theme,
}

impl Card<'_> {
    fn outline(&self) -> &'static Outline {
        if self.selected {
            &HEAVY
        } else if self.dim {
            &DASHED
        } else {
            &ROUND
        }
    }

    /// The outline's style: the state's colour, faint on an interactive row.
    fn edge(&self) -> Style {
        if self.dim {
            self.th.faint()
        } else {
            self.th.fg(self.color)
        }
    }

    /// Text on the selected card: its bar's own text colour, whatever the
    /// text is elsewhere, as a state or dim colour may not read on the bar.
    /// The outline still has the state's colour.
    fn on_card(&self) -> Style {
        self.th.text().patch(self.th.selected())
    }

    fn text(&self) -> Style {
        if self.selected {
            self.on_card()
        } else if self.dim {
            self.th.faint()
        } else {
            self.th.text()
        }
    }

    /// The folder: dim, so the name and the state stand out.
    fn soft(&self) -> Style {
        if self.selected {
            self.on_card()
        } else {
            self.th.faint()
        }
    }

    /// The badge and other small facts.
    fn fact(&self) -> Style {
        if self.selected {
            self.on_card()
        } else if self.dim {
            self.th.faint()
        } else {
            self.th.muted()
        }
    }

    fn state_style(&self) -> Style {
        let th = self.th;
        if self.selected {
            let style = self.on_card();
            if self.lit {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            }
        } else if self.dim {
            th.faint()
        } else if self.lit {
            th.attention().patch(th.hue(self.color))
        } else {
            th.hue(self.color)
        }
    }

    fn age_style(&self) -> Style {
        if !self.selected && !self.dim && self.age == "now" {
            self.th.text()
        } else {
            self.fact()
        }
    }

    /// What starts the title: the attention marker (doc/0007), then the
    /// spinner or a glyph, each with a space after it. An outline already
    /// has the state's colour, so only `small`'s glyph repeats it.
    fn lead(&self) -> Lb {
        let th = self.th;
        let mut l = Lb::new();
        if self.lit {
            l.push(format!("{MARKER} "), th.attention());
        }
        let (glyph, style) = if self.busy {
            (self.spin, th.bold(th.group_color(Group::Working)))
        } else if self.outlined && !self.selected {
            ("▪", th.faint())
        } else {
            ("▪", self.edge())
        };
        l.push(format!("{glyph} "), style);
        l
    }

    /// The name: bold in the normal text colour, so it leads the card;
    /// plain and dim on an interactive row.
    fn name_style(&self) -> Style {
        if self.dim && !self.selected {
            self.text()
        } else {
            self.text().add_modifier(Modifier::BOLD)
        }
    }

    /// The top edge: `╭─ ● ▪ name ───── +2 ─╮`. The name is cut with `…`; the
    /// badge goes before the name would keep fewer than [`NAME_MIN`] cells.
    fn top(&self) -> Line<'static> {
        let o = self.outline();
        let t = row_text(self.app, self.i, Level::Full);
        let edge = self.edge();
        let lead = self.lead();
        // The corner, a run, a space, the lead, a space, a run and the corner.
        let room = self.width.saturating_sub(lead.w + 6);
        let badge_w = t.badge.as_ref().map_or(0, |b| b.width() + 3);
        let (badge, room) = match &t.badge {
            Some(b) if room >= badge_w + NAME_MIN => (Some(b.as_str()), room - badge_w),
            _ => (None, room),
        };
        let mut l = Lb::new();
        l.push(o.top_left, edge);
        l.push(o.across, edge);
        l.push(" ", self.text());
        l.append(lead);
        l.push(shorten(&t.name, room.max(1)), self.name_style());
        l.push(" ", self.text());
        let mut right = Lb::new();
        if let Some(badge) = badge {
            right.push(format!(" {badge} "), self.fact());
            right.push(o.across, edge);
        }
        let run = self.width.saturating_sub(l.w + right.w + 1);
        l.push(o.across.repeat(run), edge);
        l.append(right);
        l.push(o.top_right, edge);
        l.line()
    }

    /// A row between the edges: `│ left    right │`.
    fn middle(&self, left: Lb, right: Lb) -> Line<'static> {
        let o = self.outline();
        let edge = self.edge();
        let mut l = Lb::new();
        l.push(o.down, edge);
        l.push(" ", self.text());
        let pad = self.width.saturating_sub(left.w + right.w + 4);
        l.append(left);
        l.push(" ".repeat(pad), self.text());
        l.append(right);
        l.push(" ", self.text());
        l.push(o.down, edge);
        l.line()
    }

    /// The account's chip in `room` cells at most, painted as the header
    /// paints it; faint on an interactive row.
    fn chip(&self, room: usize) -> (String, Style) {
        let account = row_text(self.app, self.i, Level::Full).account;
        let (_, style) = account_chip(self.app, &account, self.th);
        let style = if self.dim { self.th.faint() } else { style };
        let name = shorten(&account, room.saturating_sub(2).max(1));
        (format!(" {name} "), style)
    }

    /// The bottom edge: `╰─ main ──── 2m ─╯`, the age when `with_age`.
    fn bottom(&self, with_age: bool) -> Line<'static> {
        let o = self.outline();
        let edge = self.edge();
        let mut right = Lb::new();
        if with_age {
            right.push(format!(" {} ", self.age), self.age_style());
            right.push(o.across, edge);
        }
        // The corner, a run; a run and the corner.
        let (chip, chip_style) = self.chip(self.width.saturating_sub(right.w + 4));
        let mut l = Lb::new();
        l.push(o.bottom_left, edge);
        l.push(o.across, edge);
        l.push(chip, chip_style);
        let run = self.width.saturating_sub(l.w + right.w + 1);
        l.push(o.across.repeat(run), edge);
        l.append(right);
        l.push(o.bottom_right, edge);
        l.line()
    }

    /// `large`: name on the top edge; state and age; folder; the account on
    /// the bottom edge.
    fn large(&self) -> Vec<Line<'static>> {
        let inner = self.width.saturating_sub(4);
        let mut age = Lb::new();
        age.push(self.age.clone(), self.age_style());
        let state_room = inner.saturating_sub(age.w + GAP);
        let full = row_text(self.app, self.i, Level::Full);
        let state = if full.state.width() <= state_room {
            full.state.clone()
        } else {
            let short = row_text(self.app, self.i, Level::ShortState).state;
            shorten(&short, state_room.max(1))
        };
        let folder = if full.project.width() <= inner {
            full.project.clone()
        } else {
            let short = row_text(self.app, self.i, Level::ShortProject).project;
            shorten(&short, inner.max(1))
        };
        let mut left = Lb::new();
        left.push(state, self.state_style());
        let mut below = Lb::new();
        below.push(folder, self.soft());
        vec![
            self.top(),
            self.middle(left, age),
            self.middle(below, Lb::new()),
            self.bottom(false),
        ]
    }

    /// `medium`: name on the top edge; state and folder; the account and the
    /// age on the bottom edge.
    fn medium(&self) -> Vec<Line<'static>> {
        let inner = self.width.saturating_sub(4);
        let (state, folder) = fit_state_and_folder(self.app, self.i, inner);
        let mut left = Lb::new();
        left.push(state, self.state_style());
        let mut right = Lb::new();
        right.push(folder, self.soft());
        vec![self.top(), self.middle(left, right), self.bottom(true)]
    }

    /// `small`'s left edge. Where the selection is reverse video the selected
    /// card's is `▶`, as a selected row's, since a full block would not show.
    fn small_edge(&self) -> (&'static str, Style) {
        let reversed = !self.th.color || self.th.p.reverse;
        let glyph = match (self.selected, reversed, self.dim) {
            (true, false, _) => "█",
            (true, true, _) => "▶",
            (false, _, true) => "┆",
            (false, _, false) => "▌",
        };
        (glyph, self.edge())
    }

    /// `small`, without an outline: name and age; state, folder and account.
    /// The bar covers both rows.
    fn small(&self) -> Vec<Line<'static>> {
        let (glyph, edge) = self.small_edge();
        let t = row_text(self.app, self.i, Level::Full);
        let room = self.width.saturating_sub(2);

        let lead = self.lead();
        let mut top = Lb::new();
        top.push(glyph, edge);
        top.push(" ", self.text());
        let name_room = room.saturating_sub(lead.w + 1 + rfit(&self.age, 3).width());
        top.append(lead);
        let badge_w = t.badge.as_ref().map_or(0, |b| b.width() + 1);
        let (badge, name_room) = match &t.badge {
            Some(b) if name_room >= badge_w + NAME_MIN => (Some(b.as_str()), name_room - badge_w),
            _ => (None, name_room),
        };
        top.push(shorten(&t.name, name_room.max(1)), self.name_style());
        let mut right = Lb::new();
        if let Some(badge) = badge {
            right.push(format!("{badge} "), self.fact());
        }
        right.push(rfit(&self.age, 3), self.age_style());
        let pad = self.width.saturating_sub(top.w + right.w);
        top.push(" ".repeat(pad), self.text());
        top.append(right);

        let account_w = t.account.width().min(room / 3);
        let pair_room = room.saturating_sub(account_w + GAP);
        let (state, folder) = fit_state_and_folder(self.app, self.i, pair_room);
        let account = shorten(&t.account, account_w);
        let mut below = Lb::new();
        below.push(glyph, edge);
        below.push(" ", self.text());
        below.push(state, self.state_style());
        if !folder.is_empty() {
            below.push(" ".repeat(GAP), self.text());
            below.push(folder, self.soft());
        }
        let pad = self.width.saturating_sub(below.w + account.width());
        below.push(" ".repeat(pad), self.text());
        let account_style = match self.app.config().account(&t.account) {
            _ if self.dim => self.th.faint(),
            Some(_) => account_chip(self.app, &t.account, self.th).1,
            None => self.th.muted(),
        };
        below.push(account, account_style);
        vec![top.line(), below.line()]
    }

    fn lines(&self, size: CardSize) -> Vec<Line<'static>> {
        match size {
            CardSize::Large => self.large(),
            CardSize::Medium => self.medium(),
            CardSize::Small => self.small(),
        }
    }
}

/// The bar's top row: `▌◆ tower  by state    7 of 9 shown`. The `▌` says the
/// list has the keys, without colour. A click there goes Home (doc/0045).
fn draw_title(frame: &mut Frame, app: &App, area: Rect, th: Theme) {
    let w = usize::from(area.width);
    let shown = app
        .entries()
        .iter()
        .filter(|e| matches!(e, Entry::Row(_)))
        .count();
    let total = app.sessions().len();
    let (long, short) = if shown < total {
        (
            format!("{shown} of {total} shown"),
            format!("{shown} of {total}"),
        )
    } else if total == 1 {
        ("1 session".to_owned(), "1".to_owned())
    } else {
        (format!("{total} sessions"), total.to_string())
    };
    let mode = match app.grouping() {
        Grouping::State => "by state",
        Grouping::Project => "by project",
    };
    let mut left = Lb::new();
    if app.pane_focus() == PaneFocus::List {
        left.push("▌", th.fg(th.p.mark));
    } else {
        left.push(" ", th.text());
    }
    left.push("◆ tower", th.bold(th.p.logo));
    let room = w.saturating_sub(left.w);
    // The grouping goes first, then the words of the count.
    let (with_mode, count) = if mode.width() + GAP + long.width() + GAP < room {
        (true, long)
    } else if long.width() + GAP < room {
        (false, long)
    } else {
        (false, short)
    };
    if with_mode {
        left.push(format!("  {mode}"), th.muted());
    }
    let mut right = Lb::new();
    right.push(format!("{count} "), th.muted());
    put_lr(frame, area, left, right);
}

/// Draws the bar in `area`: the title row, then the list's whole entries from
/// the viewport's first.
pub(super) fn draw_bar(frame: &mut Frame, app: &mut App, area: Rect, now_ms: i64, th: Theme) {
    draw_title(frame, app, row(area, 0), th);
    let list = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let h = usize::from(list.height);
    let iw = usize::from(list.width);
    if h == 0 || iw == 0 {
        app.set_pane_bar(area, Vec::new());
        return;
    }
    if app.entries().is_empty() {
        app.page_size = 1;
        app.set_pane_bar(area, Vec::new());
        let text = if app.has_polled() {
            app.empty_list_text()
        } else {
            "Loading sessions…".to_owned()
        };
        put(
            frame,
            row(list, 0),
            Line::styled(fit(&format!(" {text}"), iw), th.muted()),
        );
        return;
    }
    let size = app.config().ui_card_size;
    let gap = usize::from(app.config().ui_card_gap);
    let selected = app.selected_entry();
    let entries = app.entries();
    let rows_of = |k: usize| match entries[k] {
        Entry::Header { .. } => 1,
        Entry::Row(_) => size.rows(),
    };
    // A blank row between cards and before each group header; a header stays
    // with its first card, and the list's top needs none.
    let gap_after = |k: usize| match entries.get(k + 1) {
        Some(Entry::Header { .. }) => gap,
        Some(Entry::Row(_)) if matches!(entries[k], Entry::Row(_)) => gap,
        _ => 0,
    };
    let heights: Vec<usize> = (0..entries.len())
        .map(|k| rows_of(k) + gap_after(k))
        .collect();
    let drawn: Vec<usize> = (0..entries.len()).map(rows_of).collect();
    // The selected entry's own gap may fall outside the view.
    let mut offset = scroll_lines(app.list_state.offset(), selected, h + gap, &heights);
    if let Some(sel) = selected {
        let height = |from: usize| heights[from..sel].iter().sum::<usize>() + drawn[sel];
        while offset < sel && height(offset) > h {
            offset += 1;
        }
    }

    // The entries that fit whole, the row each starts on, and which screen
    // row shows which entry.
    let mut tops: Vec<(usize, usize)> = Vec::new();
    let mut rows: Vec<(u16, usize)> = Vec::new();
    let mut y = 0;
    for k in offset..entries.len() {
        if y + drawn[k] > h {
            break;
        }
        tops.push((k, y));
        rows.extend((y..y + drawn[k]).map(|at| (list.y + at as u16, k)));
        y += heights[k];
    }
    *app.list_state.offset_mut() = offset;
    app.list_state.select(selected);
    // A page is what shows, so `PgDn` moves as far as the eye does.
    app.page_size = tops.len().max(1);
    app.set_pane_bar(area, rows);

    let app = &*app;
    let spin = spinner(now_ms);
    for (k, y) in tops {
        let is_sel = selected == Some(k);
        match &app.entries()[k] {
            Entry::Header {
                group,
                count,
                highlighted,
                collapsed,
            } => {
                let r = row(list, y as u16);
                let lead = match (is_sel, th.color) {
                    (true, true) => ("▌", th.fg(th.p.mark)),
                    (true, false) => ("▶", Style::new()),
                    (false, _) => ("─", th.line()),
                };
                put(
                    frame,
                    r,
                    separator(group, *count, *highlighted, *collapsed, lead, iw, th),
                );
                if is_sel {
                    frame.buffer_mut().set_style(r, th.selected());
                    if th.color && th.p.sel_fg.is_some() {
                        frame.buffer_mut()[(r.x, r.y)].set_fg(th.p.mark);
                    }
                }
            }
            Entry::Row(i) => {
                let m = &app.sessions()[*i];
                let dim = m.session.is_interactive();
                let card = Card {
                    app,
                    i: *i,
                    width: iw,
                    selected: is_sel,
                    lit: app.is_highlighted(m),
                    dim,
                    busy: !dim && groups::spins(&m.session),
                    outlined: size != CardSize::Small,
                    color: th.group_color(groups::classify(&m.session)),
                    spin,
                    age: age_column(app, m, now_ms),
                    th,
                };
                let lines = card.lines(size);
                let height = lines.len() as u16;
                for (dy, line) in lines.into_iter().enumerate() {
                    put(frame, row(list, y as u16 + dy as u16), line);
                }
                if is_sel {
                    let at = Rect {
                        y: list.y + y as u16,
                        height,
                        ..list
                    };
                    fill_selected(frame.buffer_mut(), at, th);
                }
            }
        }
    }
}
