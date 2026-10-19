//! The dashboard style (doc/0020): a header panel with per-state count
//! pills, the accounts and a share bar; the sessions panel with section
//! separators joined to its border; a detail panel for the selected line
//! ([`super::detail`]), which the logs panel replaces while it is open; and
//! the status line and footer in the same palette.
//!
//! Presentation only: the lines, their order, the selection, collapse,
//! filter and attention all come from [`App`] exactly as in the minimal
//! style (doc/0006, doc/0007, doc/0011).

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::theme::{Lb, Theme, rfit};
use super::{DETACH_HINT, FOOTER_HELP, FOOTER_KEYS, MARKER, age_column, fit, row_columns};
use crate::app::{App, Entry, LogsPanel, Severity};
use crate::groups::{self, Group, GroupKey, Grouping};
use crate::listing::Status;

/// The smallest terminal the dashboard draws on, columns × rows; below it
/// tower uses the minimal layout (doc/0020).
pub const MIN_SIZE: (u16, u16) = (80, 24);

/// From this width the detail (or logs) panel sits right of the list;
/// below it, under the list.
pub const SIDE_MIN_WIDTH: u16 = 130;

/// Width of the detail panel on the side.
const DETAIL_W: u16 = 48;

/// From this height the header has the share bar, the list its column
/// header, and a narrow terminal the detail strip under the list.
const TALL: u16 = 26;

/// Height of the detail strip under the list, borders included.
const STRIP_H: u16 = 5;

/// Braille spinner of busy rows; one frame per redraw tick.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner's frame time: the event loop's redraw tick, so the spinner
/// costs no extra wake-ups.
const SPINNER_STEP_MS: i64 = 200;

/// Separator between footer hints.
const HINT_GAP: &str = "   ";

const NAME_MAX: usize = 28;
const PROJECT_MAX: usize = 40;
const DETAIL_MAX: usize = 34;

/// Whether the dashboard fits in `area` ([`MIN_SIZE`]).
pub fn fits(area: Rect) -> bool {
    area.width >= MIN_SIZE.0 && area.height >= MIN_SIZE.1
}

/// A rounded panel with a border in `border`.
pub(super) fn panel(th: Theme, border: ratatui::style::Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(th.fg(border))
}

/// The spinner frame at wall-clock time `now_ms`.
pub(super) fn spinner(now_ms: i64) -> &'static str {
    SPINNER[now_ms
        .div_euclid(SPINNER_STEP_MS)
        .rem_euclid(SPINNER.len() as i64) as usize]
}

/// Draws the dashboard: header, list with the detail or logs panel, the
/// filter's input line, status line and footer.
pub fn draw_screen(frame: &mut Frame, app: &mut App, now: Instant, now_ms: i64) {
    let area = frame.area();
    let th = Theme::new(app.theme, app.color);
    let tall = area.height >= TALL;
    let input_h = u16::from(app.filter_editing());
    let [header, body, input, status, footer] = Layout::vertical([
        Constraint::Length(if tall { 4 } else { 3 }),
        Constraint::Min(4),
        Constraint::Length(input_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header, tall, th);
    let wide = area.width >= SIDE_MIN_WIDTH;
    match app.logs().cloned() {
        // The logs panel takes the detail panel's place (doc/0009); it
        // gets half the space, since the replayed screen is wide.
        Some(logs) => {
            let [list, side] = if wide {
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(body)
            } else {
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(body)
            };
            draw_list(frame, app, list, tall, now_ms, th);
            draw_logs(frame, app, &logs, side, th);
        }
        None if wide => {
            let [list, side] =
                Layout::horizontal([Constraint::Min(60), Constraint::Length(DETAIL_W)]).areas(body);
            draw_list(frame, app, list, tall, now_ms, th);
            super::detail::draw_side(frame, app, side, now_ms, th);
        }
        None if tall => {
            let [list, strip] =
                Layout::vertical([Constraint::Min(4), Constraint::Length(STRIP_H)]).areas(body);
            draw_list(frame, app, list, tall, now_ms, th);
            super::detail::draw_strip(frame, app, strip, now_ms, th);
        }
        None => draw_list(frame, app, body, tall, now_ms, th),
    }
    if app.filter_editing() {
        super::draw_filter_input(frame, app, input);
    }
    draw_status(frame, app, status, now, th);
    draw_footer(frame, footer, th);
}

/// The logs panel in the dashboard's look (doc/0009).
fn draw_logs(frame: &mut Frame, app: &App, logs: &LogsPanel, area: Rect, th: Theme) {
    let block = panel(th, th.p.logs_border)
        .title(Line::styled(super::logs_title(logs), th.bold(th.p.title)))
        .title_bottom(Line::styled(" Space/Esc close ", th.muted()).right_aligned());
    super::draw_logs_in(frame, app, logs, area, block);
}

/// One row of `area` at offset `y`.
pub(super) fn row(area: Rect, y: u16) -> Rect {
    Rect {
        x: area.x,
        y: area.y + y,
        width: area.width,
        height: 1,
    }
}

/// Draws `line` into a one-row `area`.
pub(super) fn put(frame: &mut Frame, area: Rect, line: Line<'static>) {
    if area.width > 0 && area.height > 0 {
        frame.render_widget(Paragraph::new(line), area);
    }
}

/// Splits `left` and `right` over one row: `right` is right-aligned and
/// wins where they overlap.
pub(super) fn put_lr(frame: &mut Frame, area: Rect, left: Lb, right: Lb) {
    put(frame, area, left.line());
    let rw = right.w as u16;
    if rw > 0 && rw <= area.width {
        let r = Rect {
            x: area.x + area.width - rw,
            width: rw,
            ..area
        };
        frame.render_widget(Paragraph::new(right.line()), r);
    }
}

/// A shorter pill label for narrow headers.
fn short_title(g: Group) -> &'static str {
    match g {
        Group::Orchestrator => "Orch.",
        Group::ReadyForReview => "Review",
        Group::Stopped => "Stopped",
        Group::Interactive => "Interact.",
        other => other.title(),
    }
}

/// Number of sessions per state group, over every listed session.
fn group_counts(app: &App) -> Vec<(Group, usize)> {
    Group::ALL
        .into_iter()
        .map(|g| {
            let n = app
                .sessions()
                .iter()
                .filter(|m| groups::classify(&m.session) == g)
                .count();
            (g, n)
        })
        .filter(|(_, n)| *n > 0)
        .collect()
}

/// The index of `group` among the config's account groups, in config
/// order: picks its colour.
pub(super) fn account_group_index(app: &App, group: &str) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for a in &app.config().accounts {
        if !seen.contains(&a.group.as_str()) {
            seen.push(&a.group);
        }
    }
    seen.iter().position(|g| *g == group).unwrap_or(0)
}

/// An account chip: the name in its group's colour, painted as the theme
/// paints chips.
pub(super) fn account_chip(app: &App, name: &str, th: Theme) -> (String, Style) {
    match app.config().account(name) {
        Some(a) => {
            let c = th.account_color(account_group_index(app, &a.group));
            (format!(" {name} "), th.chip(c))
        }
        None => (format!(" {name} "), th.muted()),
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect, tall: bool, th: Theme) {
    let mode = match app.grouping() {
        Grouping::State => " by state ",
        Grouping::Project => " by project ",
    };
    let block = panel(th, th.p.line)
        .title(Line::styled(" ◆ tower ", th.bold(th.p.logo)))
        .title_top(Line::styled(mode, th.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let iw = usize::from(inner.width);
    let counts = group_counts(app);

    // One pill per non-empty state group; `● n` after it counts its
    // highlighted rows (doc/0007).
    let pills = |short: bool, all: bool| {
        let mut l = Lb::new();
        l.push(" ", th.text());
        for &(g, n) in &counts {
            if !all && matches!(g, Group::Stopped | Group::Interactive) {
                continue;
            }
            let label = if short { short_title(g) } else { g.title() };
            let style = if g == Group::Interactive {
                th.interactive_pill()
            } else {
                th.pill(th.group_color(g))
            };
            l.push(format!(" {label} {n} "), style);
            let lit = app.highlighted_in(g);
            if lit > 0 {
                l.push(format!(" {MARKER} {lit}"), th.attention());
            }
            l.push(" ", th.text());
        }
        if counts.is_empty() {
            l.push("no sessions", th.muted());
        }
        l
    };
    let accounts = |with_groups: bool| {
        let mut l = Lb::new();
        for a in &app.config().accounts {
            let (chip, style) = account_chip(app, &a.name, th);
            l.push(chip, style);
            if with_groups {
                let n = app
                    .sessions()
                    .iter()
                    .filter(|m| m.owner.account() == Some(a.name.as_str()))
                    .count();
                let group = if a.group.is_empty() {
                    String::new()
                } else {
                    format!("{} · ", a.group)
                };
                l.push(format!(" {group}{n}  "), th.muted());
            } else {
                l.push(" ", th.muted());
            }
        }
        l
    };
    // Narrower: short pill labels, then no account groups, then no
    // accounts, then no Stopped and Interactive pills.
    let mut chosen = None;
    for (short, acc, with_groups, all) in [
        (false, true, true, true),
        (true, true, true, true),
        (true, true, false, true),
        (true, false, false, true),
        (true, false, false, false),
    ] {
        let p = pills(short, all);
        let a = if acc {
            accounts(with_groups)
        } else {
            Lb::new()
        };
        let fits = p.w + a.w + 2 <= iw;
        chosen = Some((p, a));
        if fits {
            break;
        }
    }
    if let Some((p, a)) = chosen {
        put_lr(frame, row(inner, 0), p, a);
    }

    // The share of each state group, btop-style; colour carries it, so it
    // is left out without colour.
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    if tall && inner.height >= 2 && th.color && total > 0 {
        let bw = iw.saturating_sub(2);
        let mut l = Lb::new();
        l.push(" ", th.text());
        let mut used = 0;
        for (k, &(g, n)) in counts.iter().enumerate() {
            let cells = if k + 1 == counts.len() {
                bw - used
            } else {
                bw * n / total
            };
            used += cells;
            let c = if g == Group::Interactive {
                th.p.surface
            } else {
                th.group_color(g)
            };
            l.push("━".repeat(cells), th.fg(c));
        }
        put(frame, row(inner, 1), l.line());
    }
}

/// How much the row columns are shortened to fit the list's width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Full,
    ShortProject,
    ShortState,
}

/// `~/workspace/api` → `…/api`.
fn short_project(project: &str) -> String {
    match project.rsplit_once('/') {
        Some((_, last)) if !last.is_empty() => format!("…/{last}"),
        _ => project.to_owned(),
    }
}

/// A shorter state detail for narrow lists: `permission prompt` →
/// `permission`, `waiting for your reply` → `reply`; the interactive
/// rows' `· not attachable` goes.
fn short_state(detail: &str) -> String {
    let detail = detail.split(" · ").next().unwrap_or(detail);
    if detail == groups::WAITING_FOR_REPLY {
        return "reply".to_owned();
    }
    detail.split(' ').next().unwrap_or(detail).to_owned()
}

/// The texts of one row's columns: name, project, state, account.
struct RowText {
    name: String,
    project: String,
    state: String,
    account: String,
}

fn row_text(app: &App, i: usize, level: Level) -> RowText {
    let [name, project, state, account] = row_columns(&app.sessions()[i], app.home());
    RowText {
        name,
        project: if level >= Level::ShortProject {
            short_project(&project)
        } else {
            project
        },
        state: if level >= Level::ShortState {
            short_state(&state)
        } else {
            state
        },
        account,
    }
}

/// Column widths: name, project, state, account.
struct Widths {
    level: Level,
    name: usize,
    project: usize,
    state: usize,
    account: usize,
}

/// Selection bar, marker, glyph; the gaps after name, project, state and
/// age; the age.
const FIXED_W: usize = 1 + 2 + 2 + 2 + 2 + 2 + 2 + 4;

/// The widest layout that fits `w` columns, from every session so the
/// columns do not jump when groups fold; if even the shortest does not
/// fit, the project and then the name are cut.
fn widths(app: &App, w: usize) -> Widths {
    let mut best = None;
    for level in [Level::Full, Level::ShortProject, Level::ShortState] {
        let texts: Vec<RowText> = (0..app.sessions().len())
            .map(|i| row_text(app, i, level))
            .collect();
        let max = |f: &dyn Fn(&RowText) -> &str, cap: usize| {
            texts
                .iter()
                .map(|t| f(t).width())
                .max()
                .unwrap_or(0)
                .min(cap)
        };
        let ws = Widths {
            level,
            name: max(&|t| &t.name, NAME_MAX).max(4),
            project: max(&|t| &t.project, PROJECT_MAX).max(7),
            state: max(&|t| &t.state, DETAIL_MAX).max(5),
            account: max(&|t| &t.account, 16).max(4),
        };
        let fits = FIXED_W + ws.name + ws.project + ws.state + ws.account <= w;
        best = Some(ws);
        if fits {
            break;
        }
    }
    let mut ws = best.expect("three levels");
    let mut over = (FIXED_W + ws.name + ws.project + ws.state + ws.account).saturating_sub(w);
    for (col, min) in [(&mut ws.project, 5), (&mut ws.name, 8), (&mut ws.state, 5)] {
        let cut = over.min(col.saturating_sub(min));
        *col -= cut;
        over -= cut;
    }
    ws
}

/// The scroll offset that keeps `selected` in view of `h` lines.
fn scroll(offset: usize, selected: Option<usize>, h: usize, len: usize) -> usize {
    let mut offset = offset.min(len.saturating_sub(h));
    if let Some(sel) = selected {
        if sel < offset {
            offset = sel;
        } else if sel >= offset + h {
            offset = sel + 1 - h;
        }
    }
    offset
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, tall: bool, now_ms: i64, th: Theme) {
    let shown = app
        .entries()
        .iter()
        .filter(|e| matches!(e, Entry::Row(_)))
        .count();
    let total = app.sessions().len();
    // Folded groups, the filter and `i` hide rows (doc/0006).
    let count = if shown < total {
        format!(" {shown} of {total} shown ")
    } else if total == 1 {
        " 1 session ".to_owned()
    } else {
        format!(" {total} sessions ")
    };
    let block = panel(th, th.p.line)
        .title(Line::styled(" Sessions ", th.bold(th.p.title)))
        .title_top(Line::styled(count, th.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if app.entries().is_empty() {
        app.page_size = usize::from(inner.height).max(1);
        let text = if app.has_polled() {
            app.empty_list_text()
        } else {
            "Loading sessions…".to_owned()
        };
        put(
            frame,
            row(inner, 0),
            Line::styled(format!("  {text}"), th.muted()),
        );
        return;
    }
    let iw = usize::from(inner.width);
    let ws = widths(app, iw);
    let head = u16::from(tall && inner.height > 2);
    if head == 1 {
        let st = th.faint().add_modifier(Modifier::BOLD);
        let text = format!(
            "     {}  {}  {}  {}  {}",
            fit("NAME", ws.name),
            fit("PROJECT", ws.project),
            fit("STATE", ws.state),
            rfit("AGE", 4),
            "ACCT"
        );
        put(frame, row(inner, 0), Line::styled(text, st));
    }

    // Scrolling keeps the selection in view; the offset lives in the
    // app's list state between frames, as in the minimal list.
    let h = usize::from(inner.height - head).max(1);
    app.page_size = h;
    let selected = app.selected_entry();
    let offset = scroll(app.list_state.offset(), selected, h, app.entries().len());
    *app.list_state.offset_mut() = offset;
    app.list_state.select(selected);

    let app = &*app;
    let spin = spinner(now_ms);
    for (k, entry) in app.entries().iter().enumerate().skip(offset).take(h) {
        let y = head + (k - offset) as u16;
        let r = row(inner, y);
        let is_sel = selected == Some(k);
        match entry {
            Entry::Header {
                group,
                count,
                highlighted,
                collapsed,
            } => {
                let connect = y > 0;
                // The selection bar of rows, else the rule joining the border.
                let lead = match (is_sel, th.color) {
                    (true, true) => ("▌", th.fg(th.p.mark)),
                    (true, false) => ("▶", Style::new()),
                    (false, _) if connect => ("─", th.line()),
                    (false, _) => (" ", th.line()),
                };
                let line = separator(group, *count, *highlighted, *collapsed, lead, iw, th);
                put(frame, r, line);
                if connect {
                    let buf = frame.buffer_mut();
                    buf[(area.x, r.y)].set_symbol("├");
                    buf[(area.x + area.width - 1, r.y)].set_symbol("┤");
                }
            }
            Entry::Row(i) => {
                let line = session_row(app, *i, &ws, is_sel, spin, now_ms, th);
                put(frame, r, line);
            }
        }
        if is_sel {
            frame.buffer_mut().set_style(r, th.selected());
            // A theme that forces the text colour on the selection bar
            // keeps the bar's mark and the attention marker in theirs.
            if th.color && th.p.sel_fg.is_some() {
                let buf = frame.buffer_mut();
                buf[(r.x, r.y)].set_fg(th.p.mark);
                if let Entry::Row(i) = entry
                    && app.is_highlighted(&app.sessions()[*i])
                    && r.width > 1
                {
                    buf[(r.x + 1, r.y)].set_fg(th.p.red);
                }
            }
        }
    }
}

/// A group's section separator: `─▾ Needs me 2  ● 2 ────`, joined to the
/// panel border by the caller; `lead` is its first cell.
fn separator(
    group: &GroupKey,
    count: usize,
    lit: usize,
    collapsed: bool,
    lead: (&'static str, Style),
    iw: usize,
    th: Theme,
) -> Line<'static> {
    let title_st = if group.is_interactive() {
        th.faint().add_modifier(Modifier::BOLD)
    } else {
        th.bold(th.key_color(group))
    };
    let mut l = Lb::new();
    l.push(lead.0, lead.1)
        .push(if collapsed { "▸ " } else { "▾ " }, th.muted())
        .push(group.title().to_owned(), title_st)
        .push(format!(" {count}"), th.muted());
    if lit > 0 {
        l.push("  ", th.muted())
            .push(format!("{MARKER} {lit}"), th.attention());
    }
    l.push(" ", th.muted());
    let n = iw.saturating_sub(l.w);
    l.push("─".repeat(n), th.line());
    l.line()
}

/// One session row: selection bar, attention marker, spinner or state
/// glyph, name, project, state, age, account (doc/0006, doc/0007).
fn session_row(
    app: &App,
    i: usize,
    ws: &Widths,
    selected: bool,
    spin: &str,
    now_ms: i64,
    th: Theme,
) -> Line<'static> {
    let m = &app.sessions()[i];
    let t = row_text(app, i, ws.level);
    let lit = app.is_highlighted(m);
    let dim = m.session.is_interactive();
    let group = groups::classify(&m.session);
    let gc = th.group_color(group);
    let busy = !dim && m.session.status == Some(Status::Busy);
    let text = if dim { th.faint() } else { th.text() };
    let soft = if dim { th.faint() } else { th.muted() };

    let mut l = Lb::new();
    match (selected, th.color) {
        (true, true) => l.push("▌", th.fg(th.p.mark)),
        (true, false) => l.push("▶", Style::new()),
        (false, _) => l.push(" ", text),
    };
    l.push(
        if lit {
            format!("{MARKER} ")
        } else {
            "  ".into()
        },
        th.attention(),
    );
    if busy {
        l.push(format!("{spin} "), th.bold(th.group_color(Group::Working)));
    } else {
        l.push("▪ ", if dim { th.faint() } else { th.fg(gc) });
    }
    let name_st = if lit {
        text.add_modifier(Modifier::BOLD)
    } else {
        text
    };
    l.push(format!("{}  ", fit(&t.name, ws.name)), name_st);
    l.push(format!("{}  ", fit(&t.project, ws.project)), soft);
    let mut state_st = if dim { th.faint() } else { th.fg(gc) };
    if lit {
        state_st = th.attention().patch(th.fg(gc));
    }
    l.push(fit(&t.state, ws.state), state_st).push("  ", text);
    let age = age_column(app, m, now_ms);
    let age_st = if dim {
        th.faint()
    } else if age == "now" {
        th.text()
    } else {
        th.muted()
    };
    l.push(rfit(&age, 4), age_st).push("  ", text);
    let account_st = match app.config().account(&t.account) {
        _ if dim => th.faint(),
        Some(a) => th.bold(th.account_color(account_group_index(app, &a.group))),
        None => th.muted(),
    };
    l.push(fit(&t.account, ws.account), account_st);
    l.line()
}

/// The status line: the attention counter, then the same segments as the
/// minimal style (doc/0007, doc/0011), in the dashboard's palette.
fn draw_status(frame: &mut Frame, app: &App, area: Rect, now: Instant, th: Theme) {
    let mut l = Lb::new();
    l.push(" ", Style::new());
    let counter = super::attention_text(app.attention_count()).map(|text| {
        if app.attention_filtered() > 0 {
            format!("{text} (filtered)")
        } else {
            text
        }
    });
    if let Some(text) = &counter {
        l.push(format!("{MARKER} {text}"), th.attention());
    }
    for (i, seg) in app.status_segments(now).into_iter().enumerate() {
        if i > 0 || counter.is_some() {
            l.push("  ·  ", th.faint());
        }
        let style = match (seg.severity, th.color) {
            (Severity::Info, _) => th.text(),
            (Severity::Warning, true) => th.fg(th.p.amber),
            (Severity::Error, true) => th.attention(),
            (_, false) => Style::new().add_modifier(Modifier::BOLD),
        };
        l.push(seg.text, style);
    }
    put(frame, area, l.line());
}

/// The footer (doc/0011): whole key hints, least used dropped first, then
/// `? help`, and the detach hint on the right, which is kept longest.
fn draw_footer(frame: &mut Frame, area: Rect, th: Theme) {
    let w = usize::from(area.width);
    let key_st = th.bold(th.p.key);
    let label_st = th.muted();
    let mut right = Lb::new();
    let (before, after) = DETACH_HINT
        .split_once("Ctrl+Z")
        .unwrap_or((DETACH_HINT, ""));
    right
        .push(before, label_st)
        .push("Ctrl+Z", th.bold(th.p.key_hot))
        .push(format!("{after} "), label_st);
    if w < right.w + 1 {
        put(frame, area, Line::styled(fit(DETACH_HINT, w), label_st));
        return;
    }
    // Room for the keys: everything left of the detach hint, with a gap.
    let room = w.saturating_sub(right.w + 3);
    let hints = footer_hints(room.saturating_sub(1));
    let mut left = Lb::new();
    left.push(" ", label_st);
    for (k, hint) in hints.iter().enumerate() {
        if k > 0 {
            left.push(HINT_GAP, label_st);
        }
        let (key, label) = hint.split_once(' ').unwrap_or((hint, ""));
        left.push(key, key_st).push(format!(" {label}"), label_st);
    }
    put_lr(frame, area, left, right);
}

/// The footer hints that fit in `w` columns: whole hints in order of
/// importance, then `? help` while it fits.
pub fn footer_hints(w: usize) -> Vec<&'static str> {
    let help = FOOTER_HELP.width();
    let mut out = Vec::new();
    let mut used = 0;
    for hint in FOOTER_KEYS {
        let next = used + hint.width() + HINT_GAP.width();
        if next + help > w {
            break;
        }
        used = next;
        out.push(*hint);
    }
    if used + help <= w {
        out.push(FOOTER_HELP);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_texts() {
        assert_eq!(short_project("~/workspace/api"), "…/api");
        assert_eq!(short_project("~"), "~");
        assert_eq!(short_state("permission prompt"), "permission");
        assert_eq!(short_state("waiting for your reply"), "reply");
        assert_eq!(short_state("idle · not attachable"), "idle");
        assert_eq!(short_state("done"), "done");
    }

    #[test]
    fn scroll_keeps_the_selection_in_view() {
        assert_eq!(scroll(0, Some(3), 5, 20), 0);
        assert_eq!(scroll(0, Some(7), 5, 20), 3);
        assert_eq!(scroll(6, Some(2), 5, 20), 2);
        assert_eq!(scroll(18, None, 5, 20), 15);
    }

    #[test]
    fn footer_keeps_help_and_drops_whole_hints() {
        assert_eq!(footer_hints(6), ["? help"]);
        assert!(footer_hints(3).is_empty());
        let some = footer_hints(40);
        assert_eq!(some.last(), Some(&"? help"));
        assert_eq!(some[0], "Enter attach");
        let all = footer_hints(1000);
        assert_eq!(all.len(), FOOTER_KEYS.len() + 1);
    }

    #[test]
    fn spinner_advances_per_tick() {
        assert_eq!(spinner(0), "⠋");
        assert_eq!(spinner(199), "⠋");
        assert_eq!(spinner(200), "⠙");
        assert_eq!(spinner(2_000), "⠋");
    }
}
