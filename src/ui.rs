//! Rendering of the milestone 1 screen: title, grouped session list, status
//! line, footer, and the help and account-picker overlays (doc/0006,
//! doc/0011). Pure drawing from [`App`] state; no I/O.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Entry, KEY_TABLE, LATER_KEYS, Overlay, Picker, Severity};
use crate::groups::{self, Group};
use crate::merge::MergedSession;
use crate::paths;

/// Footer hints; the detach hint is always shown (doc/0005, doc/0011).
pub const FOOTER: &str = "↑↓/jk move · Enter attach · r refresh · ? help · q quit   │   in a session: Ctrl+Z returns to tower";

const NAME_MAX: usize = 32;
const PROJECT_MAX: usize = 40;
const DETAIL_MAX: usize = 34;
const GAP: &str = "  ";

/// Draws the whole screen. Takes `&mut App` only to keep the list's scroll
/// offset and the page size current.
pub fn draw(frame: &mut Frame, app: &mut App, now: Instant) {
    let [title, list, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, app, title);
    draw_list(frame, app, list);
    draw_status(frame, app, status, now);
    frame.render_widget(
        Paragraph::new(FOOTER).style(Style::new().add_modifier(Modifier::DIM)),
        footer,
    );

    match app.overlay().clone() {
        Overlay::None => {}
        Overlay::Help => draw_help(frame, frame.area()),
        Overlay::Picker(p) => draw_picker(frame, app, &p, frame.area()),
    }
}

fn draw_title(frame: &mut Frame, app: &App, area: Rect) {
    let n = app.sessions().len();
    let accounts: Vec<&str> = app
        .config()
        .accounts
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    let line = Line::from(vec![
        Span::styled(" tower ", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            " {n} session{}  ·  accounts: {}",
            if n == 1 { "" } else { "s" },
            accounts.join(", ")
        )),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// The text columns of one row: name, project, detail, account.
pub fn row_columns(m: &MergedSession, home: &std::path::Path) -> [String; 4] {
    let s = &m.session;
    let mut detail = groups::detail(s);
    if s.is_interactive() {
        detail.push_str(" · not attachable");
    }
    [
        s.display_name(),
        paths::shorten_home(&s.cwd, home),
        detail,
        m.owner.label().to_owned(),
    ]
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    app.page_size = usize::from(area.height).max(1);
    if app.entries().is_empty() {
        let text = if app.has_polled() {
            "No sessions. Start one with `claude --bg \"<task>\"`; it will show up here."
        } else {
            "Loading sessions…"
        };
        frame.render_widget(Paragraph::new(format!("  {text}")), area);
        return;
    }

    let columns: Vec<[String; 4]> = app
        .sessions()
        .iter()
        .map(|m| row_columns(m, app.home()))
        .collect();
    let width = |i: usize, max: usize| {
        columns
            .iter()
            .map(|c| c[i].width())
            .max()
            .unwrap_or(0)
            .min(max)
    };
    let (nw, pw, dw) = (
        width(0, NAME_MAX),
        width(1, PROJECT_MAX),
        width(2, DETAIL_MAX),
    );

    let items: Vec<ListItem> = app
        .entries()
        .iter()
        .map(|entry| match *entry {
            Entry::Header { group, count } => {
                let mut style = Style::new().add_modifier(Modifier::BOLD);
                if group == Group::Interactive {
                    style = style.add_modifier(Modifier::DIM);
                }
                ListItem::new(Line::styled(format!("{} ({count})", group.title()), style))
            }
            Entry::Row(i) => {
                let c = &columns[i];
                let text = format!(
                    "{}{GAP}{}{GAP}{}{GAP}{}",
                    fit(&c[0], nw),
                    fit(&c[1], pw),
                    fit(&c[2], dw),
                    c[3]
                );
                let mut style = Style::new();
                if app.sessions()[i].session.is_interactive() {
                    style = style.add_modifier(Modifier::DIM);
                }
                ListItem::new(Line::styled(format!("  {text}"), style))
            }
        })
        .collect();

    app.list_state.select(app.selected_entry());
    let list = List::new(items)
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let mut spans = vec![Span::raw(" ")];
    for (i, seg) in app.status_segments(now).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                "  ·  ",
                Style::new().add_modifier(Modifier::DIM),
            ));
        }
        let style = match (seg.severity, app.color) {
            (Severity::Info, _) => Style::new(),
            (Severity::Warning, true) => Style::new().fg(Color::Yellow),
            (Severity::Error, true) => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            (_, false) => Style::new().add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(seg.text, style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A rectangle of at most `w` × `h`, centred in `area`.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = KEY_TABLE
        .iter()
        .map(|(key, what)| Line::from(format!(" {key:<16} {what}")))
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(format!(" Not available yet: {LATER_KEYS}")));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Esc or ? closes this help",
        Style::new().add_modifier(Modifier::DIM),
    ));
    let h = lines.len() as u16 + 2;
    let rect = centered(area, 62, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(" Keys ")),
        rect,
    );
}

fn draw_picker(frame: &mut Frame, app: &App, picker: &Picker, area: Rect) {
    let mut lines = vec![
        Line::from(" The account that owns this session is unknown."),
        Line::from(" Attaching with another account moves the session to it."),
    ];
    if let Some(dir) = &picker.config_dir {
        lines.push(Line::from(format!(
            " Its process uses CLAUDE_CONFIG_DIR={}, which matches no account.",
            dir.display()
        )));
    }
    lines.push(Line::raw(""));
    for (i, a) in app.config().accounts.iter().enumerate() {
        let text = format!(
            " {} {}  ({})",
            if i == picker.selected { "▶" } else { " " },
            a.name,
            a.command
        );
        let style = if i == picker.selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        lines.push(Line::styled(text, style));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Enter attach with this account · Esc cancel",
        Style::new().add_modifier(Modifier::DIM),
    ));
    let h = lines.len() as u16 + 2;
    let w = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let rect = centered(area, w.max(40), h);
    frame.render_widget(Clear, rect);
    let title = format!(" Which account owns {}? ", picker.name);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(title)),
        rect,
    );
}

/// Pads or truncates `s` to exactly `w` display columns, ending a truncated
/// value with `…`.
pub fn fit(s: &str, w: usize) -> String {
    let sw = s.width();
    if sw <= w {
        return format!("{s}{}", " ".repeat(w - sw));
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("日本語", 4), "日… ");
    }
}
