//! The workspace style (doc/0045): the session list in a bar at the left,
//! Claude's real terminal in a pane filling the rest where the dashboard has
//! its details panel, and one footer row. Windows draw over it as over any
//! style. Tower never reads the pane's screen for status; it only copies it.

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::BorderType;
use unicode_width::UnicodeWidthStr;

use super::cards::draw_bar;
use super::dashboard::{
    account_chip, footer_hints_of, group_counts, hint_spans, panel, put, put_lr, row, status_spans,
};
use super::theme::{Lb, Look, Theme};
use super::usage::detail_rows;
use super::{
    FOOTER_KEYS_PANE, FOOTER_KEYS_PANE_HEADER, FOOTER_KEYS_PANE_HOME, FOOTER_KEYS_PANE_SHELL,
    FOOTER_KEYS_PANE_STOPPED, MARKER, detail, shorten, wrap,
};
use crate::app::{App, PaneFocus, interactive_hint};
use crate::embed::PaneScreen;
use crate::groups::{self, Group};
use crate::logview;
use crate::merge::MergedSession;

/// The smallest terminal the workspace draws on, columns × rows; below it
/// tower draws the dashboard (doc/0045).
pub const MIN_SIZE: (u16, u16) = (100, 24);

/// The smallest terminal zoom draws on: it needs no bar, so it goes on below
/// [`MIN_SIZE`] (doc/0045).
pub const ZOOM_MIN_SIZE: (u16, u16) = (20, 4);

/// The list bar's share of the width and its limits, without `[ui]
/// list_width`.
const BAR_PERCENT: u32 = 18;
const BAR_MIN: u16 = 28;
const BAR_MAX: u16 = 40;

/// The Home panel's key list (doc/0045): key, what it does.
const HOME_KEYS: &[(&str, &str)] = &[
    ("↑↓ or click", "select a session"),
    ("Enter", "type in its terminal"),
    ("Ctrl+Z, F12", "back to the list"),
    ("n", "new session"),
    ("c", "settings"),
    ("?", "help"),
];

/// Width of [`HOME_KEYS`]'s key column.
const HOME_KEY_W: usize = 13;

/// Whether the workspace fits in `area` and no client failed to start.
pub fn fits(app: &App, area: Rect) -> bool {
    let min = if app.pane_wants_zoom() {
        ZOOM_MIN_SIZE
    } else {
        MIN_SIZE
    };
    area.width >= min.0 && area.height >= min.1 && !app.pane_failed()
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
    if app.pane_wants_zoom() {
        // No bar and no frame: the terminal has it all but the footer.
        app.set_pane_rect(Some(body));
        app.clear_pane_bar();
        draw_zoomed(frame, app, body, th);
        draw_footer(frame, app, footer, now, th);
        return;
    }
    let [bar, pane] = Layout::horizontal([
        Constraint::Length(list_width(app, area.width)),
        Constraint::Min(1),
    ])
    .areas(body);
    // Set first: the bar asks whether the pane shows Home.
    app.set_pane_rect(Some(panel(th, th.p.line).inner(pane)));
    draw_bar(frame, app, bar, now_ms, th);
    // The title bar shows the `git` line (doc/0037); Home has no row.
    let home = app.pane_home();
    app.set_git_shown(!home);
    if home {
        draw_home(frame, app, pane, now_ms, th);
    } else {
        draw_pane(frame, app, pane, now_ms, th);
    }
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
    // The listed name first: a rename in the pane shows at once.
    let name = m
        .map(|m| m.session.display_name())
        .or_else(|| app.pane_client_name().map(str::to_owned))
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

/// The pane's title while it shows the shell: `shell  ~/workspace/api`, and who
/// has the keys.
fn shell_title(app: &App, folder: &str, room: usize, th: Theme) -> Lb {
    let focus = match app.pane_focus() {
        PaneFocus::Terminal => " · keyboard here ",
        PaneFocus::List => " · view only ",
    };
    let marker = scrolled_marker(app);
    let mut l = Lb::new();
    l.push(" shell  ", th.bold(th.p.title));
    let folder_room = room
        .saturating_sub(l.w + focus.width() + marker.width())
        .max(4);
    l.push(shorten(folder, folder_room), th.bold(th.p.title));
    l.push(focus, th.muted());
    l.push(marker, th.faint());
    l
}

/// `↑ 120 lines ` while the pane's view is up in its history, else nothing.
fn scrolled_marker(app: &App) -> String {
    match app.pane_scrolled() {
        0 => String::new(),
        1 => "↑ 1 line ".to_owned(),
        n => format!("↑ {n} lines "),
    }
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
    let app = &*app;
    let row_shown = title_row(app);
    let room = usize::from(area.width).saturating_sub(4);
    let head = match app.pane_shell_folder() {
        Some(folder) => shell_title(app, folder, room, th),
        None => title(app, row_shown, room, th),
    };
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
        (_, Some(_), false) if app.pane_shell_showing() => " Enter: type here · !: hide the shell ",
        (Some(_), ..) => " Enter attaches again ",
        (None, Some(_), true) => " Ctrl+Z or F12: back to the list · F2: zoom ",
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
    if draw_terminal(frame, app, inner, th) {
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

/// The terminal copied into `inner`, with tower's selection and the cursor.
/// Whether there is one to draw.
fn draw_terminal(frame: &mut Frame, app: &App, inner: Rect, th: Theme) -> bool {
    let Some(screen) = app.pane_screen() else {
        return false;
    };
    paint(frame.buffer_mut(), screen, inner, th.color);
    let selected = th.selected();
    for (r, first, last) in app.pane_selection_spans() {
        for c in first..=last {
            if let Some(cell) = (r < inner.height && c < inner.width)
                .then(|| frame.buffer_mut().cell_mut((inner.x + c, inner.y + r)))
                .flatten()
            {
                cell.set_style(selected);
            }
        }
    }
    // The child's cursor, only while its terminal has the keys: a window shows
    // its own or none (doc/0045). Up in the history it has no place.
    if app.pane_has_keys()
        && app.pane_scrolled() == 0
        && let Some((y, x)) = screen.with(|s| (!s.hide_cursor()).then(|| s.cursor_position()))
        && x < inner.width
        && y < inner.height
    {
        frame.set_cursor_position((inner.x + x, inner.y + y));
    }
    true
}

/// The terminal alone, zoomed: the footer says what the title would. Until a
/// client is there, what the pane says instead of it.
fn draw_zoomed(frame: &mut Frame, app: &App, area: Rect, th: Theme) {
    if draw_terminal(frame, app, area, th) {
        return;
    }
    for (y, line) in note_lines(app, usize::from(area.width), th)
        .into_iter()
        .take(usize::from(area.height))
        .enumerate()
    {
        put(frame, row(area, y as u16), line.line());
    }
}

/// The Home panel (doc/0045), where the pane is until a row is chosen. It only
/// reads what tower already has: it attaches, wakes and asks nothing.
fn draw_home(frame: &mut Frame, app: &App, area: Rect, now_ms: i64, th: Theme) {
    let block = panel(th, th.p.line).title(Line::styled(" Home ", th.bold(th.p.title)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = home_lines(
        app,
        usize::from(inner.width),
        usize::from(inner.height),
        now_ms,
        th,
    );
    for (y, line) in lines
        .into_iter()
        .take(usize::from(inner.height))
        .enumerate()
    {
        put(frame, row(inner, y as u16), line.line());
    }
}

/// `2 need you`, `1 ready`: a group's count as Home says it.
fn count_label(group: Group, n: usize) -> String {
    match group {
        Group::Orchestrator if n > 1 => format!("{n} orchestrators"),
        Group::Orchestrator => "1 orchestrator".to_owned(),
        Group::Pinned => format!("{n} pinned"),
        Group::NeedsMe if n > 1 => format!("{n} need you"),
        Group::NeedsMe => "1 needs you".to_owned(),
        Group::Failed => format!("{n} failed"),
        Group::ReadyForReview => format!("{n} ready"),
        Group::Working => format!("{n} working"),
        Group::Other => format!("{n} other"),
        Group::Stopped => format!("{n} stopped"),
        Group::Interactive => format!("{n} interactive"),
    }
}

/// The counts per group in the theme's state colours, on as many rows as they
/// need, or what stands for them with no sessions.
fn home_counts(app: &App, width: usize, th: Theme) -> Vec<Lb> {
    let counts = group_counts(app);
    if counts.is_empty() {
        let text = if app.has_polled() {
            "No sessions yet: n starts one."
        } else {
            "Loading sessions…"
        };
        let mut l = Lb::new();
        l.push(format!(" {text}"), th.muted());
        return vec![l];
    }
    let mut out = Vec::new();
    let mut l = Lb::new();
    l.push(" ", th.text());
    for (k, (group, n)) in counts.into_iter().enumerate() {
        let label = count_label(group, n);
        if k > 0 && l.w + 3 + label.width() > width {
            out.push(std::mem::replace(&mut l, Lb::new()));
            l.push(" ", th.text());
        } else if k > 0 {
            l.push(" · ", th.faint());
        }
        l.push(label, th.hue(th.group_color(group)));
    }
    out.push(l);
    out
}

/// The sessions that need attention, at most `room` rows: `● name  state
/// account`, as the list has them (doc/0007), hidden rows too.
fn home_attention(app: &App, width: usize, room: usize, th: Theme) -> Vec<Lb> {
    let lit: Vec<_> = app
        .sessions()
        .iter()
        .filter(|m| app.is_highlighted(m))
        .collect();
    if lit.is_empty() {
        let mut l = Lb::new();
        l.push(" Nothing needs you.", th.muted());
        return vec![l];
    }
    let mut head = Lb::new();
    head.push(" Needs you", th.bold(th.p.title));
    let mut out = vec![head];
    let room = room.saturating_sub(1).max(1);
    let listed = if lit.len() > room {
        room.saturating_sub(1)
    } else {
        lit.len()
    };
    let name_w = lit
        .iter()
        .take(listed)
        .map(|m| m.session.display_name().width())
        .max()
        .unwrap_or(0)
        .min(width / 2);
    for m in lit.iter().take(listed) {
        let mut l = Lb::new();
        l.push(format!(" {MARKER} "), th.attention());
        let name = shorten(&m.session.display_name(), name_w);
        let pad = name_w.saturating_sub(name.width());
        l.push(name, th.text().add_modifier(Modifier::BOLD));
        l.push(" ".repeat(pad + 2), th.text());
        let (chip, chip_style) = match m.owner.account() {
            Some(account) => account_chip(app, account, th),
            None => (format!(" {} ", m.owner.label()), th.muted()),
        };
        let state_room = width.saturating_sub(l.w + chip.width() + 2);
        let group = groups::classify(&m.session);
        l.push(
            shorten(&groups::detail(&m.session), state_room.max(1)),
            th.hue(th.group_color(group)),
        );
        l.push("  ", th.text());
        l.push(chip, chip_style);
        out.push(l);
    }
    if listed < lit.len() {
        let mut l = Lb::new();
        l.push(format!("   … {} more", lit.len() - listed), th.muted());
        out.push(l);
    }
    out
}

/// Each account's plan usage while `[usage] show` is on (doc/0043): this style
/// has no header to show it in.
fn home_usage(app: &App, width: usize, now_ms: i64, th: Theme) -> Vec<Lb> {
    if !app.config().usage_show {
        return Vec::new();
    }
    let accounts = &app.config().accounts;
    let name_w = accounts.iter().map(|a| a.name.width()).max().unwrap_or(0);
    let mut head = Lb::new();
    head.push(" Plan usage", th.bold(th.p.title));
    let mut out = vec![head];
    for account in accounts {
        let mut l = Lb::new();
        l.push(" ", th.text());
        let (chip, style) = account_chip(app, &account.name, th);
        l.push(chip, style);
        l.push(
            " ".repeat(name_w.saturating_sub(account.name.width())),
            th.text(),
        );
        // The limits with their reset times, as many as fit whole; the bar's
        // block has the short form (doc/0043).
        for (k, part) in detail_rows(app, &account.name, now_ms, th)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let gap = if k == 0 { " " } else { " · " };
            if l.w + gap.width() + part.text.width() > width {
                if k == 0 {
                    l.push(gap, th.faint());
                    let room = width.saturating_sub(l.w).max(1);
                    l.push(shorten(&part.text, room), part.style);
                }
                break;
            }
            l.push(gap, th.faint());
            l.push(part.text, part.style);
        }
        out.push(l);
    }
    out
}

/// The Home panel's rows in `width` × `height`: what tower is, the counts, the
/// attention list, the usage and the keys. A long attention list gives way
/// to the rest.
fn home_lines(app: &App, width: usize, height: usize, now_ms: i64, th: Theme) -> Vec<Lb> {
    let mut title = Lb::new();
    title.push(
        format!(" ◆ tower {}", env!("CARGO_PKG_VERSION")),
        th.bold(th.p.logo),
    );
    let counts = home_counts(app, width, th);
    let usage = home_usage(app, width, now_ms, th);
    let keys: Vec<Lb> = HOME_KEYS
        .iter()
        .map(|(key, what)| {
            let mut l = Lb::new();
            l.push(
                format!(" {}", super::fit(key, HOME_KEY_W)),
                th.bold(th.p.key),
            );
            l.push(*what, th.muted());
            l
        })
        .collect();
    // The title, the counts, the usage and the keys, each with a blank row
    // after it but the last.
    let gaps = if usage.is_empty() { 3 } else { 4 };
    let taken = 1 + counts.len() + usage.len() + keys.len() + gaps;
    let attention = home_attention(app, width, height.saturating_sub(taken), th);

    let mut out = vec![title, Lb::new()];
    out.extend(counts);
    out.push(Lb::new());
    out.extend(attention);
    out.push(Lb::new());
    if !usage.is_empty() {
        out.extend(usage);
        out.push(Lb::new());
    }
    out.extend(keys);
    out
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
    let zoomed = app.pane_wants_zoom();
    let terminal = zoomed || app.pane_focus() == PaneFocus::Terminal;
    let mut left = Lb::new();
    if zoomed {
        left.push(" ZOOM ", th.pill(th.p.mark));
    } else if terminal {
        left.push(" TERMINAL ", th.pill(th.p.mark));
    } else {
        left.push(" LIST ", th.pill(th.p.key));
    }
    left.push(" ", th.text());
    let mut right = Lb::new();
    if zoomed {
        // The title is gone, so its marker is here; the keys as they fit.
        let marker = scrolled_marker(app);
        if !marker.is_empty() {
            right.push(format!("{marker} "), th.faint());
        }
        let room = w.saturating_sub(left.w + right.w + 2);
        let hints: &[&str] = match room {
            0..=8 => &[],
            9..=24 => &["F2 back"],
            _ => &["F2 back", "Ctrl+Z list"],
        };
        right.append(hint_spans(hints, th));
    } else if terminal {
        right.append(hint_spans(&["F2 zoom", "Ctrl+Z list", "F12 list"], th));
    }
    let hint_min = if terminal { right.w + 2 } else { 8 };
    let status_room = w.saturating_sub(left.w + hint_min);
    status_spans(&mut left, app, now, status_room, th);
    if !terminal {
        let live = app
            .selected_session()
            .is_none_or(|m| m.session.has_live_process());
        let keys = if app.pane_home() {
            FOOTER_KEYS_PANE_HOME
        } else if app.pane_shell_showing() {
            FOOTER_KEYS_PANE_SHELL
        } else if app.selected_header().is_some() {
            FOOTER_KEYS_PANE_HEADER
        } else if live {
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
