//! Rendering: title, grouped session list (with the attention marker, the
//! age column and collapsed headers, doc/0006, doc/0007), the logs panel,
//! the filter's input line, status line, footer, and the overlays: help, account picker, confirmations, the
//! hand-off target picker and trust window (doc/0004), the orchestrator's
//! trust window (doc/0010), the new-session dialog, the resume window
//! of `R` ([`dialog`], doc/0019), the rename window of `e` and its trust
//! window (doc/0027), the quick reply window of `t` and its trust window
//! (doc/0040) and the
//! popup window (doc/0006, doc/0008, doc/0009, doc/0011). Pure drawing from [`App`] state; no I/O.
//!
//! Two styles draw the screen under the same windows (doc/0020): the
//! minimal one here, and the dashboard ([`dashboard`], the default
//! `[ui] style`), which falls back to the minimal layout on terminals
//! smaller than [`DASHBOARD_MIN_SIZE`]. [`App::style`] picks one.
//!
//! Both styles draw the running subagents of a row as a tree under it
//! ([`subtree`], doc/0029); those lines are drawn, never selected.
//!
//! In the minimal style the logs panel sits to the right of the list when
//! the terminal is at least [`SIDE_PANEL_MIN_WIDTH`] columns wide, otherwise
//! below it, taking half the space either way. Its lines are never wrapped:
//! a narrow panel clips the right-hand side (doc/0009).

mod commands;
mod dashboard;
mod detail;
mod dialog;
mod settings;
mod subtree;
mod templates;
mod theme;
mod usage;

use theme::{Look, Theme};

pub use dashboard::{MIN_SIZE as DASHBOARD_MIN_SIZE, SIDE_MIN_WIDTH as DASHBOARD_SIDE_MIN_WIDTH};
pub use dialog::{resume_note, wrap};

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::activity;
use crate::app::{
    App, Confirm, DIALOG_KEY_TABLE, Entry, KEY_TABLE, LOGS_KEY_TABLE, LogsPanel, LogsView, Overlay,
    Picker, Popup, Purpose, QuitPrompt, Severity, StatusSegment, TargetPicker, UntrustedHandoff,
    UntrustedOrchestrator, UntrustedRename, UntrustedReply,
};
use crate::config::UiStyle;
use crate::groups;
use crate::merge::MergedSession;
use crate::paths;
use crate::sidecar;

/// Footer key hints of the session list (doc/0011), most important first.
/// A narrow footer drops whole hints from the end; [`FOOTER_HELP`] always
/// closes the list while it fits.
pub const FOOTER_KEYS: &[&str] = &[
    "Enter attach",
    "n new",
    "Space logs",
    "m seen",
    "/ filter",
    "a hand-off",
    "s stop",
    "d remove",
    "o orchestrator",
    "Tab fold",
    "p by project",
    "i interactive",
    "R resume",
    "e rename",
    "t reply",
    "! shell",
    "c settings",
    "↑↓ move",
    "q quit",
];

/// [`FOOTER_KEYS`] while the logs panel is open with the list focused:
/// `→` gives the panel the keys (doc/0028).
pub const FOOTER_KEYS_LOGS_OPEN: &[&str] = &[
    "Enter attach",
    "Space logs",
    "→ scroll logs",
    "n new",
    "m seen",
    "/ filter",
    "a hand-off",
    "s stop",
    "d remove",
    "o orchestrator",
    "Tab fold",
    "p by project",
    "i interactive",
    "R resume",
    "e rename",
    "t reply",
    "! shell",
    "c settings",
    "↑↓ move",
    "q quit",
];

/// Footer key hints while the logs panel has the focus (doc/0028).
pub const FOOTER_LOGS_KEYS: &[&str] = &[
    "↑↓ scroll",
    "PgUp/PgDn page",
    "g/G top/follow",
    "f full screen",
    "← list",
    "Space close",
    "q quit",
];

/// The last footer hint, kept while anything fits.
pub const FOOTER_HELP: &str = "? help";

/// Separator between footer hints.
const HINT_SEP: &str = " · ";

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
    draw_screen_and_windows(frame, app, now, now_ms);
    // Every colour within what the terminal shows (doc/0038).
    theme::reduce_buffer(frame.buffer_mut(), app.depth());
}

/// [`draw_at`] before the colours are brought within the terminal's
/// depth.
fn draw_screen_and_windows(frame: &mut Frame, app: &mut App, now: Instant, now_ms: i64) {
    let area = frame.area();
    // The dashboard sets it again when it draws a details panel
    // (doc/0037).
    app.set_git_shown(false);
    if app.settings().is_some() {
        // The Settings window takes the whole screen (doc/0024).
        let look = look(app, area);
        settings::draw(frame, app, look);
        draw_windows(frame, app);
        return;
    }
    if app.color
        && app.style == UiStyle::Dashboard
        && dashboard::fits(area)
        && app.drawn_theme() != app.theme
    {
        // The theme needs more colours than the terminal shows: `ansi16`,
        // said once (doc/0038).
        app.note_theme_fallback();
    }
    match app.style {
        UiStyle::Dashboard if dashboard::fits(area) => {
            dashboard::draw_screen(frame, app, now, now_ms);
        }
        UiStyle::Dashboard => {
            // Too small for the panels: the minimal layout, said once
            // (doc/0020).
            app.note_style_fallback(dashboard::MIN_SIZE);
            draw_minimal(frame, app, now, now_ms);
        }
        UiStyle::Minimal => draw_minimal(frame, app, now, now_ms),
    }
    draw_windows(frame, app);
}

/// The minimal style's screen: title, list (and logs panel), the filter's
/// input line, status line and footer.
fn draw_minimal(frame: &mut Frame, app: &mut App, now: Instant, now_ms: i64) {
    // The filter's input line sits above the status bar while it is
    // edited (doc/0006).
    let input_h = u16::from(app.filter_editing());
    let [title, list, input, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, app, title, now_ms);
    let full = app.logs().is_some_and(LogsPanel::full_screen);
    match app.logs().is_some() {
        false => draw_list(frame, app, list, now_ms),
        // Full screen (doc/0028): the panel takes the list's place.
        true if full => draw_logs(frame, app, list),
        true => {
            let [list_area, panel_area] = if list.width >= SIDE_PANEL_MIN_WIDTH {
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(list)
            } else {
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(list)
            };
            draw_list(frame, app, list_area, now_ms);
            draw_logs(frame, app, panel_area);
        }
    }
    if app.filter_editing() {
        draw_filter_input(frame, app, input, Look::Minimal { color: app.color });
    }
    draw_status(frame, app, status, now);
    draw_footer(frame, footer, footer_hint_list(app));
}

/// The footer's key hints for the keys' owner (doc/0011, doc/0028): the
/// logs panel's keys while it has the focus, else the list's, with
/// `→ scroll logs` while the panel is open.
pub fn footer_hint_list(app: &App) -> &'static [&'static str] {
    match app.logs() {
        Some(panel) if panel.focused() => FOOTER_LOGS_KEYS,
        Some(_) => FOOTER_KEYS_LOGS_OPEN,
        None => FOOTER_KEYS,
    }
}

/// How the windows are drawn over a screen of `area`: in the dashboard's
/// theme while the dashboard is drawn, else in the minimal look (doc/0020,
/// doc/0021).
pub(crate) fn look(app: &App, area: Rect) -> Look {
    match app.style {
        UiStyle::Dashboard if dashboard::fits(area) => {
            Look::Themed(Theme::new(app.drawn_theme(), app.color))
        }
        _ => Look::Minimal { color: app.color },
    }
}

/// The windows above either style's screen: overlays, the dialogs, the
/// popup and the quit question. The same windows in both styles, drawn in
/// the style's [`Look`].
fn draw_windows(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let look = look(app, area);
    match app.overlay().clone() {
        Overlay::None => {}
        Overlay::Help => draw_help(frame, app, area, look),
        Overlay::Picker(p) => draw_picker(frame, app, &p, area, look),
        Overlay::Confirm(c) => draw_confirm(frame, &c, area, look),
        Overlay::Target(p) => draw_target(frame, app, &p, area, look),
        Overlay::Untrusted(u) => draw_untrusted(frame, app, &u, area, look),
        Overlay::OrchestratorUntrusted(u) => {
            draw_orchestrator_untrusted(frame, app, &u, area, look);
        }
        Overlay::RenameUntrusted(u) => draw_rename_untrusted(frame, app, &u, area, look),
        Overlay::ReplyUntrusted(u) => draw_reply_untrusted(frame, app, &u, area, look),
    }
    if let Some(d) = app.dialog() {
        dialog::draw(frame, app, d, area, look);
    }
    if let Some(r) = app.resume_window() {
        dialog::draw_resume(frame, app, r, area, look);
    }
    if let Some(r) = app.rename_window() {
        dialog::draw_rename(frame, app, r, area, look);
    }
    if let Some(r) = app.reply_window() {
        dialog::draw_reply(frame, app, r, area, look);
    }
    if let Some(popup) = app.popup_mut() {
        draw_popup(frame, popup, area, look);
    }
    if let Some(prompt) = app.quit_prompt() {
        draw_quit_prompt(frame, prompt, area, look);
    }
}

/// A confirmation's answer line: ` y <yes>   n/Esc <no>`.
fn answer_line(yes: &str, no: &str, look: Look) -> Line<'static> {
    let key = look.answer_key();
    let text = look.text();
    Line::from(vec![
        Span::styled(" y", key),
        Span::styled(format!(" {yes}   "), text),
        Span::styled("n", key),
        Span::styled("/", text),
        Span::styled("Esc", key),
        Span::styled(format!(" {no}"), text),
    ])
}

/// A picker's item: `▶` and the selection bar when selected.
fn picker_item(text: String, selected: bool, look: Look) -> Line<'static> {
    if !selected {
        return Line::styled(text, look.item());
    }
    // ` ▶ rest`: the marker in the selection's mark colour.
    let Some((pre, rest)) = text.split_once('▶') else {
        return Line::styled(text, look.selected_item());
    };
    Line::from(vec![
        Span::raw(pre.to_owned()),
        Span::styled("▶", look.selected_marker()),
        Span::raw(rest.to_owned()),
    ])
    .style(look.selected_item())
}

/// The quit question while a hand-off runs (doc/0004, doc/0011), above
/// every other window.
fn draw_quit_prompt(frame: &mut Frame, prompt: &QuitPrompt, area: Rect, look: Look) {
    let w = area.width.saturating_sub(4).min(76);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let mut lines: Vec<Line> = wrap(&prompt.text(), inner_w)
        .into_iter()
        .map(|r| Line::styled(format!(" {r}"), look.text()))
        .collect();
    lines.push(Line::from(""));
    lines.push(answer_line("quit", "keep running (default)", look));
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(look.overlay_block(" Quit tower? ")),
        rect,
    );
}

/// The filter's input line (doc/0006): `/` and the text with the cursor,
/// then the match count and the keys.
pub(crate) fn draw_filter_input(frame: &mut Frame, app: &App, area: Rect, look: Look) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = app.filter_text();
    let matches = match app.filter_match_count() {
        _ if !app.filter_active() => "type to filter by name, project, account".to_owned(),
        0 => "no matches".to_owned(),
        1 => "1 match".to_owned(),
        n => format!("{n} matches"),
    };
    let hint = format!("  {matches} · Enter keep · Esc clear");
    let w = usize::from(area.width);
    // Long text keeps its end, where the cursor is, in view.
    let room = w.saturating_sub(3).max(1);
    let mut shown = text.to_owned();
    while shown.width() > room {
        shown.remove(0);
    }
    let used = 2 + shown.width();
    let mut spans = vec![
        Span::styled(" /", look.input_prompt()),
        Span::styled(shown, look.text()),
    ];
    spans.extend(look.hint_spans(fit(&hint, w.saturating_sub(used + 1))));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    let x = area.x + (used as u16).min(area.width.saturating_sub(1));
    frame.set_cursor_position((x, area.y));
}

/// The footer's key hints in `w` columns: whole hints in order of
/// importance, ending with `? help`; a hint that does not fit is dropped
/// with the rest, never cut.
pub fn footer_keys(w: usize) -> String {
    footer_keys_of(FOOTER_KEYS, w)
}

/// [`footer_keys`] for the hints `keys`.
pub fn footer_keys_of(keys: &[&str], w: usize) -> String {
    let help = FOOTER_HELP.width();
    let mut out = String::new();
    for hint in keys {
        let next = out.width() + hint.width() + HINT_SEP.width();
        if next + help > w {
            break;
        }
        out.push_str(hint);
        out.push_str(HINT_SEP);
    }
    if out.width() + help <= w {
        out.push_str(FOOTER_HELP);
    } else {
        out.truncate(out.trim_end_matches(HINT_SEP).len());
    }
    fit(&out, w)
}

/// The footer: key hints on the left, the detach hint on the right.
fn draw_footer(frame: &mut Frame, area: Rect, keys: &[&str]) {
    let w = usize::from(area.width);
    let hint = DETACH_HINT.width();
    let sep = "  │  ";
    let text = if w >= hint + sep.width() + 8 {
        // Whole key hints in the space left of the detach hint.
        let keys_w = w - hint - sep.width();
        format!("{}{sep}{DETACH_HINT}", footer_keys_of(keys, keys_w))
    } else {
        fit(DETACH_HINT, w)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::new().add_modifier(Modifier::DIM)),
        area,
    );
}

/// The logs panel: the replayed screen's last rows, or the scrolled-to
/// page, unwrapped (doc/0009, doc/0028). With the focus its border is
/// thick and its title bold.
fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    app.set_logs_height(logs_text_height(area));
    let app: &App = app;
    let Some(panel) = app.logs() else {
        return;
    };
    let (border, title) = if panel.focused() {
        (BorderType::Thick, Style::new().add_modifier(Modifier::BOLD))
    } else {
        (BorderType::Plain, Style::new())
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(border)
        .title(Line::styled(logs_title(panel, area.width), title))
        .title_bottom(Line::styled(
            logs_hint(panel, area.width),
            Style::new().add_modifier(Modifier::DIM),
        ));
    draw_logs_in(frame, app, panel, area, block);
}

/// Text rows inside a bordered logs panel of `area`.
pub(crate) fn logs_text_height(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(2))
}

/// The note for a screen rebuilt without a full repaint (doc/0030).
pub const PARTIAL_NOTE: &str = "partial output · attach once for a full screen";
/// [`PARTIAL_NOTE`] for narrow panels.
const PARTIAL_SHORT: &str = "partial output";

/// The logs panel's title in a panel `width` columns wide:
/// `logs · <name> (<account>)`, then [`PARTIAL_NOTE`] when the screen is
/// partial (doc/0030), and while paused (doc/0028) where it is, e.g.
/// `paused · line 120–160 of 900 · End follows`. It is fitted to the
/// width less 4: when it does not fit the name is shortened with `…`
/// (the account kept); next to a note or the position only down to
/// [`LOGS_NAME_MIN`] columns, after which the name goes, then the note
/// gets shorter.
pub(crate) fn logs_title(panel: &LogsPanel, width: u16) -> String {
    let room = usize::from(width.saturating_sub(4));
    let paused = panel
        .position()
        .map(|(first, last, len)| format!("paused · line {first}–{last} of {len} · End follows"));
    let partial = panel.partial();
    // `<name> (<account>)`: the name is the part that gets shorter.
    let (name, account) = match panel.title.rsplit_once(" (") {
        Some((name, account)) if !name.is_empty() && account.ends_with(')') => {
            (name, format!(" ({account}"))
        }
        _ => (panel.title.as_str(), String::new()),
    };
    let build = |name: Option<&str>, note: Option<&str>| {
        let mut parts = vec!["logs".to_owned()];
        if let Some(name) = name.filter(|_| !panel.title.is_empty()) {
            parts.push(format!("{name}{account}"));
        }
        parts.extend(note.map(str::to_owned));
        parts.extend(paused.clone());
        parts.join(" · ")
    };
    let pad = |t: String| format!(" {t} ");
    let note = partial.then_some(PARTIAL_NOTE);
    let full = build(Some(name), note);
    if full.width() <= room {
        return pad(full);
    }
    let notes = paused.is_some() || partial;
    let budget = room.saturating_sub(build(Some(""), note).width());
    if !panel.title.is_empty() && (!notes || budget >= LOGS_NAME_MIN) {
        return pad(build(Some(&shorten(name, budget.max(1))), note));
    }
    let tries = [
        Some(build(None, note)),
        partial.then(|| build(None, Some(PARTIAL_SHORT))),
    ];
    let tries: Vec<String> = tries.into_iter().flatten().collect();
    let last = tries.len() - 1;
    tries
        .into_iter()
        .enumerate()
        .find(|(i, t)| t.width() <= room || *i == last)
        .map(|(_, t)| pad(t))
        .unwrap_or_default()
}

/// The fewest columns of a shortened name in the logs panel's title next
/// to a note or the paused position; with less room the name goes
/// (doc/0028).
const LOGS_NAME_MIN: usize = 12;

/// The hint on the logs panel's bottom border of a panel `width` columns
/// wide: how to scroll it from the list, or the panel's own keys while it
/// has the focus (doc/0028); led by [`PARTIAL_NOTE`] for a partial screen
/// when that fits (doc/0030).
pub(crate) fn logs_hint(panel: &LogsPanel, width: u16) -> String {
    let keys = match (panel.focused(), panel.full_screen()) {
        (false, _) => " → scroll · Space/Esc close ",
        (true, false) => " ↑↓ PgUp/PgDn g/G · f full screen · ← list · Space close ",
        (true, true) => " ↑↓ PgUp/PgDn g/G · f/← back · Space close ",
    };
    if panel.partial() {
        let with_note = format!(" {PARTIAL_NOTE} ·{keys}");
        if with_note.width() <= usize::from(width.saturating_sub(2)) {
            return with_note;
        }
    }
    keys.to_owned()
}

/// The logs panel's content inside `block`, which either style draws in
/// its own look (doc/0009, doc/0020).
pub(crate) fn draw_logs_in(
    frame: &mut Frame,
    app: &App,
    panel: &LogsPanel,
    area: Rect,
    block: Block,
) {
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
            let mut lines = screen.page(usize::from(inner.height), panel.scroll());
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

fn draw_title(frame: &mut Frame, app: &App, area: Rect, now_ms: i64) {
    let n = app.sessions().len();
    let accounts: Vec<&str> = app
        .config()
        .accounts
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    let mut spans = vec![
        Span::styled(" tower ", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            " {n} session{}  ·  accounts: {}",
            if n == 1 { "" } else { "s" },
            accounts.join(", ")
        )),
    ];
    // Each account's plan usage, `main 24%/12%`, when all of it fits
    // (doc/0043).
    let used: Vec<(String, Style)> = accounts
        .iter()
        .filter_map(|a| usage::compact(app, a, now_ms))
        .collect();
    if !used.is_empty() {
        let mut extra = vec![Span::raw("  ·  usage: ")];
        for (k, (text, style)) in used.into_iter().enumerate() {
            if k > 0 {
                extra.push(Span::raw(", "));
            }
            extra.push(Span::styled(text, style));
        }
        let width = |s: &[Span]| s.iter().map(|s| s.content.width()).sum::<usize>();
        if width(&spans) + width(&extra) <= usize::from(area.width) {
            spans.extend(extra);
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The text columns of one row: name, project, detail, account. The
/// detail ends with `· not attachable` on interactive rows and
/// [`groups::BG_TASK_SUFFIX`] when a background task outlived the turn
/// (doc/0032).
pub fn row_columns(m: &MergedSession, home: &std::path::Path) -> [String; 4] {
    let s = &m.session;
    let mut detail = groups::detail(s);
    if s.is_interactive() {
        detail.push_str(" · not attachable");
    } else if groups::bg_task(s) {
        detail.push_str(groups::BG_TASK_SUFFIX);
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
            app.empty_list_text()
        } else {
            "Loading sessions…".to_owned()
        };
        frame.render_widget(Paragraph::new(format!("  {text}")), area);
        return;
    }

    let full: Vec<[String; 4]> = app
        .sessions()
        .iter()
        .map(|m| row_columns(m, app.home()))
        .collect();
    // The subagent count badges (doc/0029), at the end of the name column.
    let badges: Vec<Option<String>> = app
        .sessions()
        .iter()
        .map(|m| subtree::badge(app, m))
        .collect();
    let MinimalWidths {
        columns,
        name: nw,
        project: pw,
        detail: dw,
    } = minimal_widths(full, &badges, usize::from(area.width));
    // Child lines start after the highlight symbol's two cells.
    let gap = GAP.len();
    let tree = subtree::Columns {
        name_at: 2,
        age_at: 2 + nw + gap + pw + gap + dw + gap,
        width: usize::from(area.width).saturating_sub(2),
        type_w: subtree::list_type_width(app),
    };
    let with_children = tree.fit_children();

    let items: Vec<ListItem> = app
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Header {
                group,
                count,
                highlighted,
                collapsed,
            } => {
                let mut style = Style::new().add_modifier(Modifier::BOLD);
                if group.is_interactive() {
                    style = style.add_modifier(Modifier::DIM);
                }
                let fold = if *collapsed { "▸" } else { "▾" };
                let mut spans = vec![Span::styled(
                    format!("{fold} {} ({count})", group.title()),
                    style,
                )];
                let lit = *highlighted;
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
                let c = &columns[*i];
                let m = &app.sessions()[*i];
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
                let name = subtree::name_cell(&c[0], badges[*i].as_deref(), nw);
                let line = Line::from(vec![
                    Span::styled(
                        if lit {
                            format!("{MARKER} ")
                        } else {
                            "  ".to_owned()
                        },
                        accent,
                    ),
                    Span::styled(format!("{name}{GAP}{}{GAP}", fit(&c[1], pw)), base),
                    Span::styled(fit(&c[2], dw), accent),
                    Span::styled(format!("{GAP}{age:>AGE_W$}{GAP}{}", c[3]), base),
                ]);
                if !with_children || badges[*i].is_none() {
                    return ListItem::new(line);
                }
                // The subagents under the row, dimmed (doc/0029); the
                // highlight covers them with the row.
                let mut lines = vec![line];
                let dim = Style::new().add_modifier(Modifier::DIM);
                for child in subtree::children(app.subagents_of(&m.session)) {
                    let parts = subtree::child_line(&child, &tree, now_ms);
                    lines.push(Line::from(
                        parts
                            .into_iter()
                            .map(|(text, _)| Span::styled(text, dim))
                            .collect::<Vec<_>>(),
                    ));
                }
                ListItem::new(lines)
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

/// How far the minimal list's columns are shortened to fit the width
/// (doc/0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    ShortFolder,
    ShortState,
}

/// The minimal list's columns fitted to its width: each row's texts at
/// the chosen [`Level`], and the widths of the name (with its badge),
/// project and state columns.
struct MinimalWidths {
    columns: Vec<[String; 4]>,
    name: usize,
    project: usize,
    detail: usize,
}

/// The selection bar, the marker, the gaps after name, project, state and
/// age, and the age of a minimal row.
const MINIMAL_FIXED_W: usize = 2 + 2 + 4 * GAP.len() + AGE_W;

/// The narrowest the minimal list cuts its project, name and state
/// columns to.
const MINIMAL_CUT_MIN: usize = 8;

/// The widest layout of the minimal list that fits `w` columns (doc/0020),
/// from every session so the columns do not jump when groups fold: the
/// columns as they are (each capped), else the folders as `…/last`, else
/// the states in their short form as well; if even that does not fit, the
/// project, then the name, then the state are cut with `…`. The age and
/// the account always stay.
fn minimal_widths(full: Vec<[String; 4]>, badges: &[Option<String>], w: usize) -> MinimalWidths {
    let shorter = |level: Level| -> Vec<[String; 4]> {
        full.iter()
            .map(|[name, project, detail, account]| {
                let detail = if level >= Level::ShortState {
                    short_state(detail)
                } else {
                    detail.clone()
                };
                [name.clone(), short_folder(project), detail, account.clone()]
            })
            .collect()
    };
    let measure = |columns: Vec<[String; 4]>| {
        let width = |i: usize, max: usize| {
            columns
                .iter()
                .map(|c| c[i].width())
                .max()
                .unwrap_or(0)
                .min(max)
        };
        let name = columns
            .iter()
            .zip(badges)
            .map(|(c, b)| subtree::name_width(c[0].width().min(NAME_MAX), b.as_deref()))
            .max()
            .unwrap_or(0);
        let (project, detail, account) = (
            width(1, PROJECT_MAX),
            width(2, DETAIL_MAX),
            width(3, usize::MAX),
        );
        let used = MINIMAL_FIXED_W + name + project + detail + account;
        let ws = MinimalWidths {
            columns,
            name,
            project,
            detail,
        };
        (ws, used)
    };
    let (mut ws, mut used) = measure(full.clone());
    for level in [Level::ShortFolder, Level::ShortState] {
        if used <= w {
            break;
        }
        (ws, used) = measure(shorter(level));
    }
    let mut over = used.saturating_sub(w);
    for col in [&mut ws.project, &mut ws.name, &mut ws.detail] {
        let cut = over.min(col.saturating_sub(MINIMAL_CUT_MIN));
        *col -= cut;
        over -= cut;
    }
    ws
}

/// `~/workspace/api` → `…/api`: a folder for a narrow list (doc/0020).
fn short_folder(project: &str) -> String {
    match project.rsplit_once('/') {
        Some((_, last)) if !last.is_empty() => format!("…/{last}"),
        _ => project.to_owned(),
    }
}

/// A shorter state detail for a narrow list (doc/0020): `permission
/// prompt` → `permission`, `waiting for your reply` → `reply`; the
/// interactive rows' `· not attachable` goes, and `· bg task` becomes
/// `· bg` (doc/0032).
fn short_state(detail: &str) -> String {
    if let Some(head) = detail.strip_suffix(groups::BG_TASK_SUFFIX) {
        return format!("{} · bg", short_state(head));
    }
    let detail = detail.split(" · ").next().unwrap_or(detail);
    if detail == groups::WAITING_FOR_REPLY {
        return "reply".to_owned();
    }
    detail.split(' ').next().unwrap_or(detail).to_owned()
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

/// The separator between status-line segments.
pub(crate) const STATUS_GAP: &str = "  ·  ";

/// The fewest columns a cut status segment keeps; with less room it is
/// left out.
const STATUS_CUT_MIN: usize = 12;

/// Words of a cut status segment longer than this are shortened before
/// the segment's end is cut.
const STATUS_WORD_MIN: usize = 20;

/// One piece of the fitted status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusPiece {
    /// The attention counter with its marker, `● 2 need you`.
    Counter(String),
    /// [`STATUS_GAP`], only ever between two other pieces.
    Gap,
    Segment(String, Severity),
}

/// The status line of `app` fitted to `width` columns (the leading space
/// excluded), in both styles (doc/0007, doc/0020).
pub(crate) fn status_pieces(app: &App, now: Instant, width: usize) -> Vec<StatusPiece> {
    // Global, whatever the filter hides; it says so when it hides some
    // (doc/0006, doc/0007).
    let counter = attention_text(app.attention_count()).map(|text| {
        if app.attention_filtered() > 0 {
            format!("{MARKER} {text} (filtered)")
        } else {
            format!("{MARKER} {text}")
        }
    });
    fit_status(counter, app.status_segments(now), width)
}

/// Whether `text` is the status line's refresh segment (`refreshed 3s
/// ago`, `no successful refresh yet`, `loading…`; see
/// [`App::status_segments`]), which gives way to every other segment.
/// The status line's pieces in `width` columns: the counter, then whole
/// segments by priority (errors, then the view's state, progress and
/// messages, then warnings, each in [`App::status_segments`] order, and
/// the refresh segment last). The first
/// segment that does not fit is the last: its long words are shortened
/// (folders to `…/last`, then any word to [`STATUS_WORD_MIN`]), then its
/// end is cut with `…`, or it is left out with less than
/// [`STATUS_CUT_MIN`] columns. A gap never starts or ends the line.
pub(crate) fn fit_status(
    counter: Option<String>,
    segments: Vec<StatusSegment>,
    width: usize,
) -> Vec<StatusPiece> {
    let rank = |s: &StatusSegment| match s.severity {
        _ if s.refresh => 3,
        Severity::Error => 0,
        Severity::Info => 1,
        Severity::Warning => 2,
    };
    let mut segments = segments;
    segments.sort_by_key(rank);
    let mut out = Vec::new();
    let mut used = 0;
    if let Some(text) = counter {
        if text.width() > width {
            out.push(StatusPiece::Counter(shorten(&text, width)));
            return out;
        }
        used = text.width();
        out.push(StatusPiece::Counter(text));
    }
    for seg in segments {
        let gap = if out.is_empty() {
            0
        } else {
            STATUS_GAP.width()
        };
        let need = gap + seg.text.width();
        if used + need <= width {
            if gap > 0 {
                out.push(StatusPiece::Gap);
            }
            used += need;
            out.push(StatusPiece::Segment(seg.text, seg.severity));
            continue;
        }
        let room = width.saturating_sub(used + gap);
        if room >= STATUS_CUT_MIN || (out.is_empty() && room > 0) {
            if gap > 0 {
                out.push(StatusPiece::Gap);
            }
            out.push(StatusPiece::Segment(
                shorten_segment(&seg.text, room),
                seg.severity,
            ));
        }
        break;
    }
    out
}

/// A status segment in `room` columns: folders become `…/last` and long
/// words are shortened, longest first and only as far as needed, then
/// the end is cut with `…`.
fn shorten_segment(text: &str, room: usize) -> String {
    let mut words: Vec<String> = text.split(' ').map(str::to_owned).collect();
    let total = |words: &[String]| words.iter().map(|w| w.width()).sum::<usize>() + words.len() - 1;
    // Folders first, the longest first.
    while total(&words) > room {
        let Some(k) = (0..words.len())
            .filter(|&k| words[k].contains('/') && short_folder(&words[k]) != words[k])
            .max_by_key(|&k| words[k].width())
        else {
            break;
        };
        words[k] = short_folder(&words[k]);
    }
    // Then any long word, longest first.
    while total(&words) > room {
        let Some(k) = (0..words.len())
            .filter(|&k| words[k].width() > STATUS_WORD_MIN)
            .max_by_key(|&k| words[k].width())
        else {
            break;
        };
        let over = total(&words) - room;
        let w = words[k].width();
        words[k] = shorten(&words[k], w.saturating_sub(over).max(STATUS_WORD_MIN));
    }
    shorten(&words.join(" "), room)
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, now: Instant) {
    let mut spans = vec![Span::raw(" ")];
    let width = usize::from(area.width).saturating_sub(1);
    for piece in status_pieces(app, now, width) {
        spans.push(match piece {
            StatusPiece::Counter(text) => Span::styled(text, attention_style(app.color)),
            StatusPiece::Gap => Span::styled(STATUS_GAP, Style::new().add_modifier(Modifier::DIM)),
            StatusPiece::Segment(text, severity) => {
                let style = match (severity, app.color) {
                    (Severity::Info, _) => Style::new(),
                    (Severity::Warning, true) => Style::new().fg(Color::Yellow),
                    (Severity::Error, true) => {
                        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                    (_, false) => Style::new().add_modifier(Modifier::BOLD),
                };
                Span::styled(text, style)
            }
        });
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

/// A row of the help's key table: the key padded to `w` columns.
fn help_row(key: &str, what: &str, w: usize, look: Look) -> Line<'static> {
    Line::from(vec![
        Span::styled(" ", look.text()),
        Span::styled(format!("{key:<w$}"), look.table_key()),
        Span::styled(format!(" {what}"), look.text()),
    ])
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect, look: Look) {
    let mut lines: Vec<Line> = KEY_TABLE
        .iter()
        .map(|(key, what)| help_row(key, what, 16, look))
        .collect();
    lines.push(Line::raw(""));
    if app.logs_focused() {
        // From the logs panel its own keys take the dialog's place
        // (doc/0028).
        lines.push(Line::styled(" Logs panel (after →)", look.heading()));
        for (key, what) in LOGS_KEY_TABLE {
            lines.push(help_row(key, what, 16, look));
        }
    } else {
        lines.push(Line::styled(" New-session dialog", look.heading()));
        for (key, what) in DIALOG_KEY_TABLE {
            let key = if key.starts_with("Ctrl+Enter") && !app.keyboard_enhanced {
                "Alt+Enter"
            } else {
                key
            };
            lines.push(help_row(key, what, 22, look));
        }
    }
    // 14 list keys, a blank line, the dialog or logs title and 6 dialog
    // or logs keys: 22 rows, so with the borders the help fits 24 rows
    // (doc/0011).
    let h = lines.len() as u16 + 2;
    let rect = centered(area, 72, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            look.overlay_block(" Keys ")
                .title_bottom(look.hint_line(" Esc or ? closes this help ")),
        ),
        rect,
    );
}

fn draw_confirm(frame: &mut Frame, confirm: &Confirm, area: Rect, look: Look) {
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
        Confirm::Handoff {
            job,
            detail,
            interrupts,
        } => {
            let r = &job.request;
            let from = r
                .owner
                .as_ref()
                .map_or_else(|| "? (account unknown)".to_owned(), |a| a.label());
            let to = r.target.label();
            let mut text = vec![
                format!(
                    "Continue {} ({}) under {}? It is {detail}.",
                    r.name, r.id, r.target.name
                ),
                format!("From: {from}   To: {to}"),
                format!("Plan: {}.", r.plan()),
                match r.prompt_arg() {
                    Some(prompt) => format!(
                        "Prompt: \"{prompt}\", sent as the next turn under {}.",
                        r.target.name
                    ),
                    // An empty `handoff_prompt` (doc/0033).
                    None => format!(
                        "Prompt: none. The session will wait for you under {}; no turn is \
                         started.",
                        r.target.name
                    ),
                },
                format!(
                    "The full conversation and project context will continue under {to} and \
                     its terms."
                ),
            ];
            if *interrupts {
                text.push(if r.prompt_arg().is_some() {
                    "Stopping interrupts the current turn; the session then gets the hand-off \
                     prompt and picks up from there."
                        .to_owned()
                } else {
                    format!(
                        "Stopping interrupts the current turn; without a prompt the session \
                         does not pick up by itself and may show as working · idle under {} \
                         until you send one.",
                        r.target.name
                    )
                });
                if detail == "permission prompt" {
                    text.push(
                        "The pending permission prompt is not shown again after the wake."
                            .to_owned(),
                    );
                }
            }
            (" Continue under another account? ", text)
        }
    };
    text.push(String::new());
    let w = area.width.saturating_sub(4).min(76);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    // The hand-off's `From:`, `To:`, `Plan:` and `Prompt:` are labels.
    let labels: &[&str] = if matches!(confirm, Confirm::Handoff { .. }) {
        &["From:", "To:", "Plan:", "Prompt:"]
    } else {
        &[]
    };
    let mut lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| look.labelled_line(&format!(" {r}"), labels))
        .collect();
    lines.push(answer_line("yes", "no (default)", look));
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

/// The popup window: wrapped text, scrolled; the scroll offset is clamped
/// here because only the renderer knows the visible height.
fn draw_popup(frame: &mut Frame, popup: &mut Popup, area: Rect, look: Look) {
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
        .map(|r| Line::styled(format!(" {r}"), look.text()))
        .collect();
    let hint = if more {
        " ↑↓/PgUp/PgDn scroll · Esc close "
    } else {
        " Esc close "
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            look.overlay_block(format!(" {} ", popup.title))
                .title_bottom(look.hint_line(hint)),
        ),
        rect,
    );
}

/// The hand-off target picker (doc/0004): the accounts of the owner's
/// group except the owner, or every other account with
/// `allow_cross_group_handoff` ([`crate::handoff::targets`]).
fn draw_target(frame: &mut Frame, app: &App, picker: &TargetPicker, area: Rect, look: Look) {
    let mut lines = vec![match &picker.owner {
        Some(owner) => look.labelled_line(
            &format!(" Owner: {owner}. Stop it there and continue under:"),
            &["Owner:"],
        ),
        None => Line::styled(
            " Owner unknown (?). Stop it with the target's command and continue under:",
            look.text(),
        ),
    }];
    lines.push(Line::raw(""));
    for (i, name) in picker.targets.iter().enumerate() {
        let account = app.config().account(name);
        let command = account.map(|a| a.command.as_str()).unwrap_or("?");
        let group = match account.map(|a| a.group.as_str()) {
            Some(g) if !g.is_empty() => format!("  group {g}"),
            _ => String::new(),
        };
        let text = format!(
            " {} {name}  ({command}){group}",
            if i == picker.selected { "▶" } else { " " }
        );
        lines.push(picker_item(text, i == picker.selected, look));
    }
    lines.push(Line::raw(""));
    lines.push(look.hint_line(" Enter continue under this account · Esc cancel"));
    let h = lines.len() as u16 + 2;
    let body = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let make = |name: &str| format!("Continue {name} under which account?");
    let w = window_width(area, body.max(40), make(&picker.name).width());
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = fit_title(make, &picker.name, usize::from(rect.width));
    frame.render_widget(
        Paragraph::new(lines).block(look.overlay_block(format!(" {title} "))),
        rect,
    );
}

/// The window after a hand-off failed with `Workspace not trusted`
/// (doc/0004, doc/0008).
fn draw_untrusted(
    frame: &mut Frame,
    app: &App,
    untrusted: &UntrustedHandoff,
    area: Rect,
    look: Look,
) {
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
        .map(|r| look.text_line(format!(" {r}"), look.text()))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.target.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

/// The window after the orchestrator's start failed with `Workspace not
/// trusted` (doc/0008, doc/0010).
fn draw_orchestrator_untrusted(
    frame: &mut Frame,
    app: &App,
    untrusted: &UntrustedOrchestrator,
    area: Rect,
    look: Look,
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
        .map(|r| look.text_line(format!(" {r}"), look.text()))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.account.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

/// The window after a rename failed with `Workspace not trusted`
/// (doc/0008, doc/0027).
fn draw_rename_untrusted(
    frame: &mut Frame,
    app: &App,
    untrusted: &UntrustedRename,
    area: Rect,
    look: Look,
) {
    let r = &untrusted.job.request;
    let dir = paths::shorten_home(&r.cwd.to_string_lossy(), app.home());
    let mut text: Vec<String> = untrusted.message.lines().map(str::to_owned).collect();
    text.extend([
        String::new(),
        format!(
            "{} was not renamed: {} has not trusted its folder, so it was not woken and stays \
             stopped.",
            r.old_name, r.owner.name
        ),
        format!(
            "Ctrl+T: run `{}` in {dir} to accept Claude Code's trust prompt, then /exit; tower \
             then retries the rename once.",
            r.owner.command
        ),
        "Esc: leave it as it was.".to_owned(),
    ]);
    let w = area.width.saturating_sub(4).min(80);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| look.text_line(format!(" {r}"), look.text()))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.owner.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

/// The window after a quick reply failed with `Workspace not trusted`
/// (doc/0008, doc/0040).
fn draw_reply_untrusted(
    frame: &mut Frame,
    app: &App,
    untrusted: &UntrustedReply,
    area: Rect,
    look: Look,
) {
    let r = &untrusted.job.request;
    let dir = paths::shorten_home(&r.cwd.to_string_lossy(), app.home());
    let mut text: Vec<String> = untrusted.message.lines().map(str::to_owned).collect();
    text.extend([
        String::new(),
        format!(
            "Your text was not sent to {}: {} has not trusted its folder, so it was not woken \
             and stays stopped.",
            r.name, r.owner.name
        ),
        format!(
            "Ctrl+T: run `{}` in {dir} to accept Claude Code's trust prompt, then /exit; tower \
             then sends the text once more.",
            r.owner.command
        ),
        "Esc: leave it stopped; the text is not sent.".to_owned(),
    ]);
    let w = area.width.saturating_sub(4).min(80);
    let inner_w = usize::from(w.saturating_sub(4)).max(1);
    let lines: Vec<Line> = text
        .iter()
        .flat_map(|t| wrap(t, inner_w))
        .map(|r| look.text_line(format!(" {r}"), look.text()))
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Not trusted by {} ", r.owner.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

fn draw_picker(frame: &mut Frame, app: &App, picker: &Picker, area: Rect, look: Look) {
    let mut lines = vec![Line::styled(
        " The account that owns this session is unknown.",
        look.text(),
    )];
    match picker.purpose {
        Purpose::Attach => lines.push(Line::styled(
            " Attaching with another account moves the session to it.",
            look.text(),
        )),
        Purpose::Rename => lines.push(Line::styled(
            " Tower wakes it with that account to rename it, which moves the session to it.",
            look.text(),
        )),
        Purpose::Reply => lines.push(Line::styled(
            " Tower wakes it with that account to send your text, which moves the session to it.",
            look.text(),
        )),
        Purpose::Stop | Purpose::Remove => {}
    }
    if let Some(dir) = &picker.config_dir {
        lines.push(Line::styled(
            format!(
                " Its process uses CLAUDE_CONFIG_DIR={}, which matches no account.",
                dir.display()
            ),
            look.text(),
        ));
    }
    lines.push(Line::raw(""));
    for (i, a) in app.config().accounts.iter().enumerate() {
        let text = format!(
            " {} {}  ({})",
            if i == picker.selected { "▶" } else { " " },
            a.name,
            a.command
        );
        lines.push(picker_item(text, i == picker.selected, look));
    }
    lines.push(Line::raw(""));
    lines.push(look.hint_line(format!(
        " Enter {} with this account · Esc cancel",
        picker.purpose.verb()
    )));
    let h = lines.len() as u16 + 2;
    let body = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let make = |name: &str| format!("Which account owns {name}?");
    let w = window_width(area, body.max(40), make(&picker.name).width());
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = fit_title(make, &picker.name, usize::from(rect.width));
    frame.render_widget(
        Paragraph::new(lines).block(look.overlay_block(format!(" {title} "))),
        rect,
    );
}

/// The widest a window grows to show its whole title (doc/0011), as the
/// popup window.
const TITLE_WINDOW_MAX: u16 = 100;

/// The width of a window over `area` whose body needs `body` columns
/// (borders included) and whose title is `title` columns (without its
/// padding spaces): it grows to show the whole title, up to
/// [`TITLE_WINDOW_MAX`] and the screen less 4 columns, and never below
/// the body's width.
pub(crate) fn window_width(area: Rect, body: u16, title: usize) -> u16 {
    let max = TITLE_WINDOW_MAX.min(area.width.saturating_sub(4));
    let want = u16::try_from(title.saturating_add(4)).unwrap_or(u16::MAX);
    body.max(want.min(max))
}

/// The title `make(name)` of a box `width` columns wide, fitted to the
/// width less 4 (a corner and a space each side): when it does not fit
/// the name is shortened with `…` and the fixed words around it are kept
/// (doc/0011).
pub(crate) fn fit_title(make: impl Fn(&str) -> String, name: &str, width: usize) -> String {
    let room = width.saturating_sub(4);
    let full = make(name);
    if full.width() <= room {
        return full;
    }
    let fixed = make("").width();
    make(&shorten(name, room.saturating_sub(fixed).max(1)))
}

/// `s` in at most `w` columns: unchanged when it fits, else cut and ended
/// with `…`, with no padding.
pub(crate) fn shorten(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_owned();
    }
    fit(s, w).trim_end_matches(' ').to_owned()
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
    use super::{StatusPiece, fit, fit_status};
    use crate::app::{Severity, StatusSegment};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("日本語", 4), "日… ");
    }

    fn seg(text: &str, severity: Severity) -> StatusSegment {
        StatusSegment {
            text: text.to_owned(),
            severity,
            refresh: text.starts_with("refreshed "),
        }
    }

    /// The line as drawn, without the leading space.
    fn text(pieces: &[StatusPiece]) -> String {
        pieces
            .iter()
            .map(|p| match p {
                StatusPiece::Counter(t) | StatusPiece::Segment(t, _) => t.as_str(),
                StatusPiece::Gap => super::STATUS_GAP,
            })
            .collect()
    }

    fn many() -> Vec<StatusSegment> {
        vec![
            seg("filter \"auth\": 2 matches", Severity::Info),
            seg(
                "started refactor-the-authentication-middleware-and-session-store under main \
                 in ~/workspace/billing-platform-frontend-with-a-very-long-folder-name",
                Severity::Info,
            ),
            seg("refreshed 3s ago", Severity::Info),
            seg(
                "alt: claude-alt agents --json failed (exit 1): could not reach the \
                 background service",
                Severity::Error,
            ),
            seg(
                "project root ~/old-projects does not exist",
                Severity::Warning,
            ),
        ]
    }

    #[test]
    fn the_status_line_keeps_whole_segments_by_priority() {
        let counter = || Some("● 2 need you".to_owned());
        // Wide enough for everything: errors, info, warnings, refresh.
        let all = text(&fit_status(counter(), many(), 400));
        assert_eq!(
            all,
            "● 2 need you  ·  alt: claude-alt agents --json failed (exit 1): could not reach \
             the background service  ·  filter \"auth\": 2 matches  ·  started \
             refactor-the-authentication-middleware-and-session-store under main in \
             ~/workspace/billing-platform-frontend-with-a-very-long-folder-name  ·  project \
             root ~/old-projects does not exist  ·  refreshed 3s ago"
        );
        for w in [79, 119, 249] {
            let pieces = fit_status(counter(), many(), w);
            let line = text(&pieces);
            assert!(line.width() <= w, "{w}: {line}");
            assert!(
                line.starts_with("● 2 need you  ·  alt: claude-alt"),
                "{w}: {line}"
            );
            assert!(!line.trim_end().ends_with('·'), "{w}: {line}");
            assert!(!matches!(pieces.last(), Some(StatusPiece::Gap)), "{w}");
            // Only the last piece is cut.
            let cut: Vec<usize> = pieces
                .iter()
                .enumerate()
                .filter(|(_, p)| matches!(p, StatusPiece::Segment(t, _) if t.ends_with('…')))
                .map(|(k, _)| k)
                .collect();
            assert!(cut.iter().all(|&k| k + 1 == pieces.len()), "{w}: {line}");
        }
        // 80 columns: the error is cut at its end.
        let line = text(&fit_status(counter(), many(), 79));
        assert_eq!(
            line,
            "● 2 need you  ·  alt: claude-alt agents --json failed (exit 1): could not reac…"
        );
        // 120: the error whole, the filter cut, the rest gives way.
        let line = text(&fit_status(counter(), many(), 119));
        assert!(
            line.ends_with("background service  ·  filter \"aut…"),
            "{line}"
        );
        assert!(!line.contains("started"), "{line}");
        // 250: the message's folder becomes `…/last` and is shortened
        // before anything else; the warning and `refreshed` give way.
        let line = text(&fit_status(counter(), many(), 249));
        assert!(
            line.ends_with(
                "·  started refactor-the-authentication-middleware-and-session-store under \
                 main in …/billing-platform-frontend-with-…"
            ),
            "{line}"
        );
        assert!(!line.contains("project root"), "{line}");
        assert!(!line.contains("refreshed"), "{line}");
    }

    #[test]
    fn a_cut_segment_shortens_its_names_first() {
        let pieces = fit_status(None, many()[1..2].to_vec(), 60);
        assert_eq!(
            text(&pieces),
            "started refactor-the-authen… under main in …/billing-platfo…"
        );
        // A gap never starts the line, and nothing is cut too short.
        let pieces = fit_status(Some("● 1 needs you".into()), many(), 20);
        assert_eq!(text(&pieces), "● 1 needs you");
        assert_eq!(
            text(&fit_status(Some("● 1 needs you".into()), many(), 5)),
            "● 1 …"
        );
        assert!(fit_status(None, Vec::new(), 80).is_empty());
    }
}
