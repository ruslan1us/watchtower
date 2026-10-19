//! Rendering: title, grouped session list (with the attention marker, the
//! age column and collapsed headers, doc/0006, doc/0007), the logs panel,
//! the filter's input line, status line, footer, and the overlays: help, account picker, confirmations, the
//! hand-off target picker and trust window (doc/0004), the orchestrator's
//! trust window (doc/0010), the new-session dialog, the resume window
//! of `R` ([`dialog`], doc/0019), the rename window of `e` and its trust
//! window (doc/0027) and the
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

mod dashboard;
mod detail;
mod dialog;
mod settings;
mod subtree;
mod theme;

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
    Picker, Popup, Purpose, QuitPrompt, Severity, TargetPicker, UntrustedHandoff,
    UntrustedOrchestrator, UntrustedRename,
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
    let area = frame.area();
    if app.settings().is_some() {
        // The Settings window takes the whole screen (doc/0024).
        let look = look(app, area);
        settings::draw(frame, app, look);
        draw_windows(frame, app);
        return;
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

    draw_title(frame, app, title);
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
            Look::Themed(Theme::new(app.theme, app.color))
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
/// `paused · line 120–160 of 900 · End follows`. When it does not fit,
/// the name goes first, then the note gets shorter.
pub(crate) fn logs_title(panel: &LogsPanel, width: u16) -> String {
    let room = usize::from(width.saturating_sub(2));
    let paused = panel
        .position()
        .map(|(first, last, len)| format!("paused · line {first}–{last} of {len} · End follows"));
    let partial = panel.partial();
    let build = |name: bool, note: Option<&str>| {
        let mut parts = vec!["logs"];
        if name && !panel.title.is_empty() {
            parts.push(&panel.title);
        }
        parts.extend(note);
        parts.extend(paused.as_deref());
        format!(" {} ", parts.join(" · "))
    };
    let mut tries = vec![build(true, partial.then_some(PARTIAL_NOTE))];
    if paused.is_some() || partial {
        tries.push(build(false, partial.then_some(PARTIAL_NOTE)));
    }
    if partial {
        tries.push(build(false, Some(PARTIAL_SHORT)));
    }
    let last = tries.len() - 1;
    tries
        .into_iter()
        .enumerate()
        .find(|(i, t)| t.width() <= room || *i == last)
        .map(|(_, t)| t)
        .unwrap_or_default()
}

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

    let columns: Vec<[String; 4]> = app
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
    let width = |i: usize, max: usize| {
        columns
            .iter()
            .map(|c| c[i].width())
            .max()
            .unwrap_or(0)
            .min(max)
    };
    let nw = columns
        .iter()
        .zip(&badges)
        .map(|(c, b)| subtree::name_width(c[0].width().min(NAME_MAX), b.as_deref()))
        .max()
        .unwrap_or(0);
    let (pw, dw) = (width(1, PROJECT_MAX), width(2, DETAIL_MAX));
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
    // Global, whatever the filter hides; it says so when it hides some
    // (doc/0006, doc/0007).
    let counter = attention_text(app.attention_count()).map(|text| {
        if app.attention_filtered() > 0 {
            format!("{text} (filtered)")
        } else {
            text
        }
    });
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
    // 15 list keys, a blank line, the dialog or logs title and 6 dialog
    // or logs keys: with the borders the help fits 24 rows (doc/0011).
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
                format!(
                    "The full conversation and project context will continue under {to} and \
                     its terms."
                ),
            ];
            if *interrupts {
                text.push(
                    "Stopping interrupts the current turn; the session then gets the hand-off \
                     prompt and picks up from there."
                        .to_owned(),
                );
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
    // The hand-off's `From:`, `To:` and `Plan:` are labels.
    let labels: &[&str] = if matches!(confirm, Confirm::Handoff { .. }) {
        &["From:", "To:", "Plan:"]
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
    let w = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let rect = centered(area, w.max(40), h);
    frame.render_widget(Clear, rect);
    let title = format!(" Continue {} under which account? ", picker.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
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
    let w = lines.iter().map(Line::width).max().unwrap_or(40) as u16 + 4;
    let rect = centered(area, w.max(40), h);
    frame.render_widget(Clear, rect);
    let title = format!(" Which account owns {}? ", picker.name);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
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
