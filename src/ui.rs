//! Rendering: title, grouped session list (with the attention marker and
//! the age column, doc/0006, doc/0007), the logs panel, status line,
//! footer, and the overlays: help, account picker, confirmations, the
//! hand-off target picker and trust window (doc/0004), the orchestrator's
//! trust window (doc/0010), the new-session dialog ([`dialog`]) and the
//! popup window (doc/0006, doc/0008, doc/0009, doc/0011). Pure drawing from [`App`] state; no I/O.
//!
//! The logs panel sits to the right of the list when the terminal is at
//! least [`SIDE_PANEL_MIN_WIDTH`] columns wide, otherwise below it, taking
//! half the space either way. Its lines are never wrapped: a narrow panel
//! clips the right-hand side (doc/0009).

mod dialog;

pub use dialog::wrap;

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::activity;
use crate::app::{
    App, Confirm, DIALOG_KEY_TABLE, Entry, KEY_TABLE, LATER_KEYS, LogsPanel, LogsView, Overlay,
    Picker, Popup, Purpose, QuitPrompt, Severity, TargetPicker, UntrustedHandoff,
    UntrustedOrchestrator,
};
use crate::groups::{self, Group};
use crate::merge::MergedSession;
use crate::paths;
use crate::sidecar;

/// Footer key hints of the session list (doc/0011).
pub const FOOTER_KEYS: &str = "↑↓ move · Enter attach · n new · a hand-off · s stop · d remove · \
     Space logs · m seen · o orchestrator · ? help · q quit";

/// The detach hint, always shown at the right of the footer (doc/0005,
/// doc/0011); the key hints are cut first when the terminal is narrow.
pub const DETACH_HINT: &str = "in a session: Ctrl+Z returns to tower";

/// From this width on, the logs panel is a side panel; below it, a bottom
/// panel.
pub const SIDE_PANEL_MIN_WIDTH: u16 = 150;

/// The attention marker in a highlighted row's leading column, and before
/// the status bar's counter (doc/0007).
pub const MARKER: &str = "●";

/// Width of the "since last change" column (`now`, `42s`, `59m`, `999d`).
const AGE_W: usize = 4;

const NAME_MAX: usize = 32;
const PROJECT_MAX: usize = 40;
const DETAIL_MAX: usize = 34;
const GAP: &str = "  ";

/// Draws the whole screen. Takes `&mut App` only to keep the list's scroll
/// offset and the page size current.
pub fn draw(frame: &mut Frame, app: &mut App, now: Instant) {
    draw_at(frame, app, now, sidecar::now_ms());
}

/// [`draw`] with the wall-clock time `now_ms` (Unix ms) the age column
/// counts from (doc/0006).
pub fn draw_at(frame: &mut Frame, app: &mut App, now: Instant, now_ms: i64) {
    let [title, list, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, app, title);
    match app.logs().cloned() {
        None => draw_list(frame, app, list, now_ms),
        Some(panel) => {
            let [list_area, panel_area] = if list.width >= SIDE_PANEL_MIN_WIDTH {
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(list)
            } else {
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(list)
            };
            draw_list(frame, app, list_area, now_ms);
            draw_logs(frame, app, &panel, panel_area);
        }
    }
    draw_status(frame, app, status, now);
    draw_footer(frame, footer);

    let area = frame.area();
    match app.overlay().clone() {
        Overlay::None => {}
        Overlay::Help => draw_help(frame, app, area),
        Overlay::Picker(p) => draw_picker(frame, app, &p, area),
        Overlay::Confirm(c) => draw_confirm(frame, app, &c, area),
        Overlay::Target(p) => draw_target(frame, app, &p, area),
        Overlay::Untrusted(u) => draw_untrusted(frame, app, &u, area),
        Overlay::OrchestratorUntrusted(u) => draw_orchestrator_untrusted(frame, app, &u, area),
    }
    if let Some(d) = app.dialog() {
        dialog::draw(frame, app, d, area);
    }
    if let Some(popup) = app.popup_mut() {
        draw_popup(frame, popup, area);
    }
    if let Some(prompt) = app.quit_prompt() {
        draw_quit_prompt(frame, app, prompt, area);
    }
}

/// The quit question while a hand-off runs (doc/0004, doc/0011), above
/// every other window.
fn draw_quit_prompt(frame: &mut Frame, app: &App, prompt: &QuitPrompt, area: Rect) {
    let w = area.width.saturating_sub(4).min(76);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let mut lines: Vec<Line> = wrap(&prompt.text(), inner_w)
        .into_iter()
        .map(|r| Line::from(format!(" {r}")))
        .collect();
    lines.push(Line::from(""));
    let key_style = if app.color {
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(vec![
        Span::styled(" y", key_style),
        Span::raw(" quit   "),
        Span::styled("n", key_style),
        Span::raw("/"),
        Span::styled("Esc", key_style),
        Span::raw(" keep running (default)"),
    ]));
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(" Quit tower? ")),
        rect,
    );
}

/// The footer: key hints on the left, the detach hint on the right.
fn draw_footer(frame: &mut Frame, area: Rect) {
    let w = usize::from(area.width);
    let hint = DETACH_HINT.width();
    let sep = "  │  ";
    let text = if w >= hint + sep.width() + 8 {
        // Key hints padded or cut to the space left of the detach hint.
        let keys_w = w - hint - sep.width();
        format!("{}{sep}{DETACH_HINT}", fit(FOOTER_KEYS, keys_w))
    } else {
        fit(DETACH_HINT, w)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::new().add_modifier(Modifier::DIM)),
        area,
    );
}

/// The logs panel: the replayed screen's last rows, unwrapped (doc/0009).
fn draw_logs(frame: &mut Frame, app: &App, panel: &LogsPanel, area: Rect) {
    let title = if panel.title.is_empty() {
        " logs ".to_owned()
    } else {
        format!(" logs · {} ", panel.title)
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::styled(
            " Space/Esc close ",
            Style::new().add_modifier(Modifier::DIM),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let note = |text: &str| -> Vec<Line<'static>> {
        wrap(text, usize::from(inner.width).saturating_sub(1))
            .into_iter()
            .map(|r| Line::styled(format!(" {r}"), Style::new().add_modifier(Modifier::DIM)))
            .collect()
    };
    let lines: Vec<Line> = match &panel.view {
        LogsView::Note(text) => note(text),
        LogsView::Loading => note("loading…"),
        LogsView::Error(text) => {
            let style = if app.color {
                Style::new().fg(Color::Red)
            } else {
                Style::new().add_modifier(Modifier::BOLD)
            };
            wrap(text, usize::from(inner.width).saturating_sub(1))
                .into_iter()
                .map(|r| Line::styled(format!(" {r}"), style))
                .collect()
        }
        LogsView::Screen(screen) if screen.is_empty() => note("(no output yet)"),
        LogsView::Screen(screen) => {
            let mut lines = screen.tail(usize::from(inner.height));
            if !app.color {
                for line in &mut lines {
                    for span in &mut line.spans {
                        span.style.fg = None;
                        span.style.bg = None;
                    }
                }
            }
            lines
        }
    };
    // No wrapping: a narrow panel clips the right-hand side.
    frame.render_widget(Paragraph::new(lines), inner);
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

/// The style of the attention marker and a highlighted row's state
/// column: bold, plus an accent colour unless `NO_COLOR` (doc/0007).
fn attention_style(color: bool) -> Style {
    let style = Style::new().add_modifier(Modifier::BOLD);
    if color {
        style.fg(Color::LightRed)
    } else {
        style
    }
}

/// The age column of a row: time since its last change (doc/0006).
pub fn age_column(app: &App, m: &MergedSession, now_ms: i64) -> String {
    match app.last_change_at(m, now_ms) {
        Some(at) => activity::age_label(now_ms.saturating_sub(at)),
        None => "-".to_owned(),
    }
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, now_ms: i64) {
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
                let mut spans = vec![Span::styled(format!("{} ({count})", group.title()), style)];
                let lit = app.highlighted_in(group);
                if lit > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("{MARKER} {lit}"),
                        attention_style(app.color),
                    ));
                }
                ListItem::new(Line::from(spans))
            }
            Entry::Row(i) => {
                let c = &columns[i];
                let m = &app.sessions()[i];
                let lit = app.is_highlighted(m);
                let mut base = Style::new();
                if m.session.is_interactive() {
                    base = base.add_modifier(Modifier::DIM);
                }
                if lit {
                    base = base.add_modifier(Modifier::BOLD);
                }
                let accent = if lit {
                    attention_style(app.color)
                } else {
                    base
                };
                let age = age_column(app, m, now_ms);
                let line = Line::from(vec![
                    Span::styled(
                        if lit {
                            format!("{MARKER} ")
                        } else {
                            "  ".to_owned()
                        },
                        accent,
                    ),
                    Span::styled(
                        format!("{}{GAP}{}{GAP}", fit(&c[0], nw), fit(&c[1], pw)),
                        base,
                    ),
                    Span::styled(fit(&c[2], dw), accent),
                    Span::styled(format!("{GAP}{age:>AGE_W$}{GAP}{}", c[3]), base),
                ]);
                ListItem::new(line)
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

/// The status bar's attention counter (doc/0007): `1 needs you`,
/// `2 need you`; `None` when nothing is highlighted.
pub fn attention_text(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 needs you".to_owned()),
        n => Some(format!("{n} need you")),
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let mut spans = vec![Span::raw(" ")];
    let counter = attention_text(app.attention_count());
    if let Some(text) = &counter {
        spans.push(Span::styled(
            format!("{MARKER} {text}"),
            attention_style(app.color),
        ));
    }
    for (i, seg) in app.status_segments(now).into_iter().enumerate() {
        if i > 0 || counter.is_some() {
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
pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = KEY_TABLE
        .iter()
        .map(|(key, what)| Line::from(format!(" {key:<16} {what}")))
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " New-session dialog",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    for (key, what) in DIALOG_KEY_TABLE {
        let key = if key.starts_with("Ctrl+Enter") && !app.keyboard_enhanced {
            "Alt+Enter"
        } else {
            key
        };
        lines.push(Line::from(format!(" {key:<22} {what}")));
    }
    // The keys still to come share the bottom border with the closing
    // hint, so the help fits 24 rows.
    let h = lines.len() as u16 + 2;
    let rect = centered(area, 72, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::ALL)
                .title(" Keys ")
                .title_bottom(Line::styled(
                    format!(" Not available yet: {LATER_KEYS} · Esc or ? closes this help "),
                    Style::new().add_modifier(Modifier::DIM),
                )),
        ),
        rect,
    );
}

fn draw_confirm(frame: &mut Frame, app: &App, confirm: &Confirm, area: Rect) {
    let (title, mut text) = match confirm {
        Confirm::Stop { action, detail } => (
            " Stop session? ",
            vec![
                format!(
                    "Stop {} ({}, account {})? It is {detail}.",
                    action.name, action.id, action.account.name
                ),
                "The conversation is kept; attaching wakes the session again.".to_owned(),
            ],
        ),
        Confirm::Remove { action } => (
            " Remove session? ",
            vec![
                format!(
                    "Remove {} ({}, account {}) from the list?",
                    action.name, action.id, action.account.name
                ),
                "The transcript stays on disk and can still be resumed with claude --resume."
                    .to_owned(),
                "`claude rm` may also delete the session's worktree when that is safe; it \
                 refuses if there is uncommitted or unpushed work."
                    .to_owned(),
            ],
        ),
        Confirm::Handoff { job, detail } => {
            let r = &job.request;
            let mut text = vec![
                format!(
                    "Continue {} ({}) under {}? It is {detail}.",
                    r.name, r.id, r.target.name
                ),
                format!("Tower will {}.", r.plan()),
                "Stopping interrupts the current turn; the session then gets the hand-off \
                 prompt and picks up from there."
                    .to_owned(),
            ];
            if detail == "permission prompt" {
                text.push(
                    "The pending permission prompt is not shown again after the wake.".to_owned(),
                );
            }
            (" Continue under another account? ", text)
        }
    };
    text.push(String::new());
    let w = area.width.saturating_sub(4).min(76);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let mut lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| Line::from(format!(" {r}")))
        .collect();
    let key_style = if app.color {
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(vec![
        Span::styled(" y", key_style),
        Span::raw(" yes   "),
        Span::styled("n", key_style),
        Span::raw("/"),
        Span::styled("Esc", key_style),
        Span::raw(" no (default)"),
    ]));
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(title)),
        rect,
    );
}

/// The popup window: wrapped text, scrolled; the scroll offset is clamped
/// here because only the renderer knows the visible height.
fn draw_popup(frame: &mut Frame, popup: &mut Popup, area: Rect) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let rows: Vec<String> = popup.lines.iter().flat_map(|l| wrap(l, inner_w)).collect();
    let max_h = area.height.saturating_sub(2).max(3);
    let h = (rows.len() as u16 + 2).min(max_h);
    let rect = centered(area, w, h);
    let visible = usize::from(h.saturating_sub(2)).max(1);
    popup.scroll = popup.scroll.min(rows.len().saturating_sub(visible));
    let more = rows.len() > visible;
    let lines: Vec<Line> = rows
        .iter()
        .skip(popup.scroll)
        .take(visible)
        .map(|r| Line::from(format!(" {r}")))
        .collect();
    let hint = if more {
        " ↑↓/PgUp/PgDn scroll · Esc close "
    } else {
        " Esc close "
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::ALL)
                .title(format!(" {} ", popup.title))
                .title_bottom(Line::styled(hint, Style::new().add_modifier(Modifier::DIM))),
        ),
        rect,
    );
}

/// The hand-off target picker (doc/0004): every account except the owner.
fn draw_target(frame: &mut Frame, app: &App, picker: &TargetPicker, area: Rect) {
    let mut lines = vec![match &picker.owner {
        Some(owner) => Line::from(format!(
            " Owner: {owner}. Stop it there and continue under:"
        )),
        None => {
            Line::from(" Owner unknown (?). Stop it with the target's command and continue under:")
        }
    }];
    lines.push(Line::raw(""));
    for (i, name) in picker.targets.iter().enumerate() {
        let command = app
            .config()
            .account(name)
            .map(|a| a.command.as_str())
            .unwrap_or("?");
        let text = format!(
            " {} {name}  ({command})",
            if i == picker.selected { "▶" } else { " " }
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
        " Enter continue under this account · Esc cancel",
        Style::new().add_modifier(Modifier::DIM),
    ));
    let h = lines.len() as u16 + 2;
    let w = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let rect = centered(area, w.max(40), h);
    frame.render_widget(Clear, rect);
    let title = format!(" Continue {} under which account? ", picker.name);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(title)),
        rect,
    );
}

/// The window after a hand-off failed with `Workspace not trusted`
/// (doc/0004, doc/0008).
fn draw_untrusted(frame: &mut Frame, app: &App, untrusted: &UntrustedHandoff, area: Rect) {
    let r = &untrusted.job.request;
    let dir = paths::shorten_home(&r.cwd.to_string_lossy(), app.home());
    let mut text: Vec<String> = untrusted.message.lines().map(str::to_owned).collect();
    text.extend([
        String::new(),
        format!(
            "{} is stopped and was not continued under {}.",
            r.name, r.target.name
        ),
        format!(
            "Ctrl+T: run `{}` in {dir} to accept Claude Code's trust prompt, then /exit; tower \
             then retries the hand-off once.",
            r.target.command
        ),
        "Esc: leave it stopped.".to_owned(),
    ]);
    let w = area.width.saturating_sub(4).min(80);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| Line::from(format!(" {r}")))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.target.name);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(title)),
        rect,
    );
}

/// The window after the orchestrator's start failed with `Workspace not
/// trusted` (doc/0008, doc/0010).
fn draw_orchestrator_untrusted(
    frame: &mut Frame,
    app: &App,
    untrusted: &UntrustedOrchestrator,
    area: Rect,
) {
    let r = &untrusted.job.request;
    let dir = paths::shorten_home(&r.project.to_string_lossy(), app.home());
    let mut text: Vec<String> = untrusted.message.lines().map(str::to_owned).collect();
    text.extend([
        String::new(),
        format!(
            "The orchestrator was not started: {} has not trusted its folder.",
            r.account.name
        ),
        format!(
            "Ctrl+T: run `{}` in {dir} to accept Claude Code's trust prompt, then /exit; tower \
             then retries the start once.",
            r.account.command
        ),
        "Esc: do not start it.".to_owned(),
    ]);
    let w = area.width.saturating_sub(4).min(80);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| Line::from(format!(" {r}")))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.account.name);
    frame.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(title)),
        rect,
    );
}

fn draw_picker(frame: &mut Frame, app: &App, picker: &Picker, area: Rect) {
    let mut lines = vec![Line::from(
        " The account that owns this session is unknown.",
    )];
    if picker.purpose == Purpose::Attach {
        lines.push(Line::from(
            " Attaching with another account moves the session to it.",
        ));
    }
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
        format!(
            " Enter {} with this account · Esc cancel",
            picker.purpose.verb()
        ),
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
