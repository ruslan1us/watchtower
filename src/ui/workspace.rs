//! The workspace style (doc/0045): the session list in a bar at the left,
//! Claude's real terminal in a pane filling the rest where the dashboard has
//! its details panel, and one footer row. Windows draw over it as over any
//! style. Tower never reads the pane's screen for status; it only copies it.

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::BorderType;

use super::dashboard::{
    account_chip, draw_bar, footer_hints_of, hint_spans, panel, put, put_lr, row, status_spans,
};
use super::theme::{Lb, Look, Theme};
use super::{FOOTER_KEYS_PANE, FOOTER_KEYS_PANE_STOPPED, detail, shorten, wrap};
use crate::app::{App, PaneFocus, interactive_hint};
use crate::embed::PaneScreen;
use crate::groups;
use crate::logview;
use crate::merge::MergedSession;

/// The smallest terminal the workspace draws on, columns × rows; below it
/// tower draws the dashboard (doc/0045).
pub const MIN_SIZE: (u16, u16) = (100, 24);

/// The list bar's share of the width and its limits, without `[ui]
/// list_width`.
const BAR_PERCENT: u32 = 18;
const BAR_MIN: u16 = 28;
const BAR_MAX: u16 = 40;

/// Whether the workspace fits in `area` and no client failed to start.
pub fn fits(app: &App, area: Rect) -> bool {
    area.width >= MIN_SIZE.0 && area.height >= MIN_SIZE.1 && !app.pane_failed()
}

/// The list bar's width on a terminal `total` columns wide (doc/0045). The
/// pane keeps at least half of a narrow terminal.
pub fn list_width(app: &App, total: u16) -> u16 {
    let width = app.config().ui_list_width.unwrap_or_else(|| {
        let share = u32::from(total) * BAR_PERCENT / 100;
        u16::try_from(share)
            .unwrap_or(BAR_MAX)
            .clamp(BAR_MIN, BAR_MAX)
    });
    width.min(total / 2)
}

/// Draws the bar, the pane and the footer.
pub fn draw_screen(frame: &mut Frame, app: &mut App, now: Instant, now_ms: i64) {
    let area = frame.area();
    let th = Theme::new(app.drawn_theme(), app.color);
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let [bar, pane] = Layout::horizontal([
        Constraint::Length(list_width(app, area.width)),
        Constraint::Min(1),
    ])
    .areas(body);
    draw_bar(frame, app, bar, now_ms, th);
    // The title bar shows the `git` line (doc/0037).
    app.set_git_shown(true);
    draw_pane(frame, app, pane, now_ms, th);
    // The filter's input line (doc/0006) takes the footer's row while it is
    // edited, so the pane keeps its size.
    if app.filter_editing() {
        super::draw_filter_input(frame, app, footer, Look::Themed(th));
    } else {
        draw_footer(frame, app, footer, now, th);
    }
}

/// The row the pane's title describes: the client's, else the selection's.
fn title_row(app: &App) -> Option<&MergedSession> {
    match app.pane_client_key() {
        Some(key) => app.sessions().iter().find(|m| m.session.key() == Some(key)),
        None => app.selected_session(),
    }
}

/// The pane's title: name, state, account, and who has the keys.
fn title(app: &App, m: Option<&MergedSession>, room: usize, th: Theme) -> Lb {
    let name = app
        .pane_client_name()
        .map(str::to_owned)
        .or_else(|| m.map(|m| m.session.display_name()))
        .or_else(|| app.selected_header().map(|group| group.title().to_owned()))
        .unwrap_or_else(|| "details".to_owned());
    let focus = match (app.pane_screen().is_some(), app.pane_focus()) {
        (false, _) => "",
        (true, PaneFocus::Terminal) => " · keyboard here",
        (true, PaneFocus::List) => " · view only",
    };
    let mut rest = Lb::new();
    if let Some(m) = m {
        let group = groups::classify(&m.session);
        rest.push(
            format!("  {}  ", groups::detail(&m.session)),
            th.hue(th.group_color(group)),
        );
        if let Some(account) = m.owner.account() {
            let (chip, style) = account_chip(app, account, th);
            rest.push(chip, style);
        } else {
            rest.push(format!(" {} ", m.owner.label()), th.muted());
        }
    }
    rest.push(format!("{focus} "), th.muted());
    let mut l = Lb::new();
    let name_room = room.saturating_sub(rest.w + 1).max(4);
    l.push(
        format!(" {}", shorten(&name, name_room)),
        th.bold(th.p.title),
    );
    l.append(rest);
    l
}

/// The pane: its frame and title, and the terminal or what stands for it.
fn draw_pane(frame: &mut Frame, app: &mut App, area: Rect, now_ms: i64, th: Theme) {
    let focused = app.pane_focus() == PaneFocus::Terminal;
    let block = if focused {
        panel(th, th.p.mark).border_type(BorderType::Thick)
    } else {
        panel(th, th.p.line)
    };
    let inner = block.inner(area);
    app.set_pane_rect(Some(inner));
    let app = &*app;
    let row_shown = title_row(app);
    let room = usize::from(area.width).saturating_sub(4);
    let head = title(app, row_shown, room, th);
    let git_room = room.saturating_sub(head.w + 6);
    let mut block = block.title(head.line());
    if let Some(summary) = row_shown.and_then(|m| app.git_summary(&m.session.cwd))
        && git_room >= 12
    {
        block = block.title_top(
            Line::styled(format!(" git {} ", summary.fitted(git_room)), th.muted()).right_aligned(),
        );
    }
    let hint = match (app.pane_ended_code(), app.pane_screen(), focused) {
        (Some(_), ..) => " Enter attaches again ",
        (None, Some(_), true) => " Ctrl+Z or F12: back to the list ",
        (None, Some(_), false) => " Enter: type here ",
        (None, None, _) => "",
    };
    if !hint.is_empty() {
        block = block.title_bottom(Line::styled(hint, th.muted()).right_aligned());
    }
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if let Some(screen) = app.pane_screen() {
        paint(frame.buffer_mut(), screen, inner, th.color);
        // Claude's cursor, only while its terminal has the keys: a window
        // shows its own or none (doc/0045).
        if app.pane_has_keys()
            && let Some((y, x)) = screen.with(|s| (!s.hide_cursor()).then(|| s.cursor_position()))
            && x < inner.width
            && y < inner.height
        {
            frame.set_cursor_position((inner.x + x, inner.y + y));
        }
        return;
    }
    let width = usize::from(inner.width);
    let mut lines = note_lines(app, width, th);
    if !lines.is_empty() {
        lines.push(Lb::new());
    }
    if app.selected_entry().is_some() {
        lines.extend(detail::pane_lines(app, width, now_ms, th));
    }
    for (y, line) in lines
        .into_iter()
        .take(usize::from(inner.height))
        .enumerate()
    {
        put(frame, row(inner, y as u16), line.line());
    }
}

/// What the pane says instead of a terminal.
fn note_lines(app: &App, width: usize, th: Theme) -> Vec<Lb> {
    let Some(m) = app.selected_session() else {
        return Vec::new();
    };
    let name = m.session.display_name();
    let s = &m.session;
    let (text, key) = if !s.is_attachable() {
        (interactive_hint(&name), None)
    } else if app.pane_row_busy(m) {
        (
            format!("tower is working on {name}; the terminal returns when it finishes."),
            None,
        )
    } else if !s.has_live_process() {
        let owner = m.owner.account().map_or_else(
            || "tower asks which account first".to_owned(),
            |account| format!("under {account}, its owner"),
        );
        (
            "This session is not running; selecting it started nothing.".to_owned(),
            Some(format!("Enter wakes it here ({owner})")),
        )
    } else if let Some(why) = app.pane_held() {
        (
            format!("Not attached: {why}."),
            Some("Enter attaches it here".to_owned()),
        )
    } else if m.owner.account().is_none() {
        (
            "Tower does not know this session's account yet.".to_owned(),
            Some("Enter asks which one, then attaches it here".to_owned()),
        )
    } else {
        ("attaching…".to_owned(), None)
    };
    let mut lines: Vec<Lb> = wrap(&text, width.saturating_sub(2).max(1))
        .into_iter()
        .map(|part| {
            let mut l = Lb::new();
            l.push(format!(" {part}"), th.text());
            l
        })
        .collect();
    if let Some(key) = key {
        let mut l = Lb::new();
        l.push(format!(" {key}"), th.bold(th.p.key));
        lines.push(l);
    }
    lines
}

/// Copies the child's screen into `area` of the frame. With `NO_COLOR` the
/// child's attributes stay and its colours go.
fn paint(buf: &mut Buffer, screen: &PaneScreen, area: Rect, color: bool) {
    screen.with(|s| {
        let (rows, cols) = s.size();
        for r in 0..area.height.min(rows) {
            for c in 0..area.width.min(cols) {
                let Some(cell) = s.cell(r, c) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style: Style = logview::cell_style(cell);
                if !color {
                    style.fg = Some(Color::Reset);
                    style.bg = Some(Color::Reset);
                }
                let text = cell.contents();
                // A wide character needs both cells inside the pane.
                let fits = !cell.is_wide() || c + 1 < area.width;
                let symbol = if text.is_empty() || !fits { " " } else { text };
                if let Some(out) = buf.cell_mut((area.x + c, area.y + r)) {
                    out.set_symbol(symbol).set_style(style);
                }
            }
        }
    });
}

/// The footer: the `LIST` or `TERMINAL` chip, the status line, and the key
/// hints in what is left.
fn draw_footer(frame: &mut Frame, app: &App, area: Rect, now: Instant, th: Theme) {
    let w = usize::from(area.width);
    let terminal = app.pane_focus() == PaneFocus::Terminal;
    let mut left = Lb::new();
    if terminal {
        left.push(" TERMINAL ", th.pill(th.p.mark));
    } else {
        left.push(" LIST ", th.pill(th.p.key));
    }
    left.push(" ", th.text());
    let mut right = Lb::new();
    if terminal {
        right.append(hint_spans(&["Ctrl+Z list", "F12 list"], th));
    }
    let hint_min = if terminal { right.w + 2 } else { 8 };
    let status_room = w.saturating_sub(left.w + hint_min);
    status_spans(&mut left, app, now, status_room, th);
    if !terminal {
        let live = app
            .selected_session()
            .is_none_or(|m| m.session.has_live_process());
        let keys = if live {
            FOOTER_KEYS_PANE
        } else {
            FOOTER_KEYS_PANE_STOPPED
        };
        let hints = footer_hints_of(keys, w.saturating_sub(left.w + 3));
        right.append(hint_spans(&hints, th));
    }
    right.push(" ", th.text());
    put_lr(frame, area, left, right);
}
