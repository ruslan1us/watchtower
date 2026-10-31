//! The welcome window (doc/0048), drawn over the whole screen in the window
//! [`Look`] like the Settings window (doc/0024), whose row styles, swatches
//! and conflict question it shares: a header with the steps, the step's
//! panel, the status line and the footer. Below
//! [`crate::welcome::MIN_SIZE`] a plain notice takes its place.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::dialog::input_window;
use super::settings::{
    GAP, bar_line, choice_note, draw_question, kind_style, on_bar, put, put_lr, row, wrap_value,
};
use super::theme::{Lb, Look, palette};
use super::{fit, wrap};
use crate::app::App;
use crate::config::{UiStyle, UiTheme};
use crate::config_edit::{SettingKey, SettingValue};
use crate::groups::Group;
use crate::settings::{FolderInfo, Mode, NoticeKind, SettingsWindow, shown_value};
use crate::welcome::{ClaudeProbe, MIN_SIZE, OPTION_KEYS, Origin, Step, Welcome};

/// Where the guide for more accounts is.
const ACCOUNTS_GUIDE: &str = "the Configuration guide at github.com/ruslan1us/watchtower";

/// The first start's words on what tower is.
const INTRO: &str = "Tower keeps your Claude Code background sessions on one screen: what needs \
    you, what is working, what is done. It starts, opens and stops them with Claude Code's own \
    commands, and they keep running if tower closes.";

/// What `tower setup` says first.
const INTRO_SETUP: &str = "Change tower's look, project folders and options. Only what you change \
    is written; your comments and every other key stay as they are.";

/// Under a Claude Code that could not be run.
const NO_CLAUDE: &str = "Tower needs Claude Code 2.1.300 or newer for its sessions. You can \
    finish here and install it later.";

/// One switch of the Options step: its name and what it does.
fn option_text(key: SettingKey) -> (&'static str, &'static str) {
    match key {
        SettingKey::UsageShow => (
            "plan usage",
            "Show each account's usage as Claude Code's /usage reports it.",
        ),
        SettingKey::TrackSubagents => (
            "track subagents",
            "List the subagents of the sessions tower starts (observe-only hooks).",
        ),
        _ => (
            "tab title",
            "Show how many sessions need you in the terminal's tab title.",
        ),
    }
}

/// Draws the window when one is open.
pub fn draw(frame: &mut Frame, app: &App, look: Look) {
    let Some(win) = app.settings() else {
        return;
    };
    let Some(w) = win.welcome.as_ref() else {
        return;
    };
    let area = frame.area();
    frame.render_widget(Clear, area);
    if w.too_small {
        draw_too_small(frame, w, area, look);
        return;
    }
    let [header, body, status, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, w, header, look);
    let title = if w.failed.is_some() {
        " Not saved ".to_owned()
    } else {
        format!(" {} ", w.step.title())
    };
    let block = look.overlay_block(title);
    let inner = block.inner(body);
    frame.render_widget(block, body);
    let width = usize::from(inner.width);
    let mut cursor = None;
    let lines = match (&w.failed, w.step) {
        (Some(err), _) => failed_lines(w, err, width, look),
        (None, Step::Welcome) => welcome_lines(app, win, w, width, look),
        (None, Step::Look) => look_lines(app, win, w, width, look),
        (None, Step::Projects) => project_lines(win, w, inner, look, &mut cursor),
        (None, Step::Options) => option_lines(win, w, width, look),
        (None, Step::Done) => done_lines(win, w, width, look),
    };
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some(at) = cursor {
        frame.set_cursor_position(at);
    }
    draw_status(frame, win, w, status, look);
    draw_footer(frame, win, w, footer, look);
    if matches!(win.mode, Mode::Conflict(_)) {
        draw_question(frame, win, body, look);
    }
}

/// The plain notice below [`MIN_SIZE`].
fn draw_too_small(frame: &mut Frame, w: &Welcome, area: Rect, look: Look) {
    let esc = match w.origin {
        Origin::FirstRun => "Esc goes on with the built-in defaults, writing nothing.",
        Origin::Setup => "Esc goes on without changing anything.",
    };
    let text = format!(
        "This window needs a {}x{} terminal. Make it bigger to go on. {esc} Ctrl+C quits.",
        MIN_SIZE.0, MIN_SIZE.1
    );
    let lines: Vec<Line> = wrap(&text, usize::from(area.width))
        .into_iter()
        .map(|t| Line::styled(t, look.text()))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The header panel: the steps, with the current one marked.
fn draw_header(frame: &mut Frame, w: &Welcome, area: Rect, look: Look) {
    let name = match w.origin {
        Origin::FirstRun => " welcome ",
        Origin::Setup => " setup ",
    };
    let block = look
        .overlay_block(" ◆ tower ")
        .title_top(Line::styled(name, look.secondary()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut left = Lb::new();
    left.push(" ", look.text());
    let at = Step::ALL.iter().position(|s| *s == w.step).unwrap_or(0);
    for (i, step) in Step::ALL.iter().enumerate() {
        if i > 0 {
            left.push(" › ", look.hint());
        }
        let st = match i.cmp(&at) {
            std::cmp::Ordering::Less => look.secondary(),
            std::cmp::Ordering::Equal => look.strong(),
            std::cmp::Ordering::Greater => look.hint(),
        };
        left.push(step.title(), st);
    }
    let mut right = Lb::new();
    right.push(
        format!("step {} of {} ", at + 1, Step::ALL.len()),
        look.hint(),
    );
    put_lr(frame, row(inner, 0), left, right);
}

/// `text` wrapped under one padding column.
fn text_lines(text: &str, width: usize, st: Style) -> Vec<Line<'static>> {
    wrap(text, width.saturating_sub(2))
        .into_iter()
        .map(|t| Line::styled(format!(" {t}"), st))
        .collect()
}

/// A label and its value, the value wrapped under itself.
fn fact_lines(label: &str, value: &str, width: usize, st: Style, look: Look) -> Vec<Line<'static>> {
    // The label column, and one column clear of the border.
    let room = width.saturating_sub(17);
    wrap(value, room)
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let mut l = Lb::new();
            l.push(
                format!("   {}", fit(if i == 0 { label } else { "" }, 13)),
                look.hint(),
            )
            .push(part, st);
            l.line()
        })
        .collect()
}

/// One row: `main runs claude`, or every configured account, with the
/// pointer to the guide. Accounts are never edited here (doc/0022).
fn accounts_text(win: &SettingsWindow) -> String {
    let parts: Vec<String> = win
        .session
        .accounts()
        .iter()
        .map(|a| format!("{} runs {}", a.name, a.command))
        .collect();
    format!("{}. More accounts: {ACCOUNTS_GUIDE}", parts.join(", "))
}

/// The Welcome step.
fn welcome_lines(
    app: &App,
    win: &SettingsWindow,
    w: &Welcome,
    width: usize,
    look: Look,
) -> Vec<Line<'static>> {
    // `tower setup` with no file writes a whole new one, like a first start.
    let intro = if win.session.exists() {
        INTRO_SETUP
    } else {
        INTRO
    };
    let mut lines = text_lines(intro, width, look.text());
    lines.push(Line::raw(""));
    match &w.claude {
        ClaudeProbe::Checking => {
            lines.extend(fact_lines(
                "Claude Code",
                "checking…",
                width,
                look.secondary(),
                look,
            ));
        }
        ClaudeProbe::Found(version) => {
            lines.extend(fact_lines(
                "Claude Code",
                version,
                width,
                look.success(),
                look,
            ));
        }
        ClaudeProbe::Failed(why) => {
            lines.extend(fact_lines(
                "Claude Code",
                &format!("could not be run: {why}"),
                width,
                look.warning(),
                look,
            ));
            lines.extend(fact_lines("", NO_CLAUDE, width, look.secondary(), look));
        }
    }
    if !matches!(w.claude, ClaudeProbe::Failed(_)) {
        let sessions = if !app.has_polled() {
            "looking…".to_owned()
        } else {
            match app.sessions().len() {
                0 => "none yet".to_owned(),
                1 => "1 session".to_owned(),
                n => format!("{n} sessions"),
            }
        };
        lines.extend(fact_lines(
            "sessions",
            &sessions,
            width,
            look.secondary(),
            look,
        ));
    }
    lines.push(Line::raw(""));
    lines.extend(text_lines(&accounts_text(win), width, look.secondary()));
    lines.push(Line::raw(""));
    let next = match w.origin {
        Origin::FirstRun => format!(
            "Next: pick a look and your project folders. They are saved to {}, and nothing in \
             Claude Code changes.",
            win.shown_path
        ),
        Origin::Setup => format!(
            "Next: the look, your project folders and the options, saved to {}. Nothing in \
             Claude Code changes.",
            win.shown_path
        ),
    };
    lines.extend(text_lines(&next, width, look.text()));
    lines
}

/// The value column of the Look step: the longest theme with its `‹ ›`.
const VALUE_W: usize = 21;

/// The Look step: the style, the theme and, in the workspace style, the card
/// size; the selected one's help; the state colours the theme draws.
fn look_lines(
    app: &App,
    win: &SettingsWindow,
    w: &Welcome,
    width: usize,
    look: Look,
) -> Vec<Line<'static>> {
    let mut lines = text_lines(
        "How tower looks. Every change shows at once, in this window too.",
        width,
        look.secondary(),
    );
    lines.push(Line::raw(""));
    let rows = win.look_rows();
    for (i, key) in rows.iter().enumerate() {
        let selected = i == w.look_row;
        let bar = |st: Style| on_bar(look, selected, st);
        let plain = if selected {
            look.selected_item()
        } else {
            look.text()
        };
        let value = win.session.value(*key).display();
        let mut l = Lb::new();
        l.push(
            if selected { "▌" } else { " " },
            bar(look.selected_marker()),
        );
        l.push(" ", plain);
        l.push(
            fit(key.label(), 11),
            if selected {
                plain.add_modifier(Modifier::BOLD)
            } else {
                plain
            },
        );
        let shown = format!("‹ {value} ›");
        l.push(fit(&shown, VALUE_W), plain);
        if *key == SettingKey::UiTheme
            && let Ok(theme) = UiTheme::parse(&value)
            && let Look::Themed(th) = look
            && th.color
        {
            let p = palette(theme);
            for c in [p.red, p.amber, p.green, p.blue, p.teal, p.grey] {
                l.push("  ", Style::new().bg(c));
            }
            l.push(" ", plain);
        }
        let room = width.saturating_sub(l.w + 1);
        l.push(
            fit(choice_note(*key, &value), room).trim_end().to_owned(),
            bar(look.secondary()),
        );
        lines.push(bar_line(l, selected, width, look));
    }
    lines.push(Line::raw(""));
    if let Some(key) = rows.get(w.look_row) {
        lines.extend(text_lines(key.help(), width, look.secondary()));
        lines.push(Line::raw(""));
    }
    if app.style == UiStyle::Workspace {
        lines.extend(text_lines(
            "Below 100 columns tower draws the dashboard until the terminal grows.",
            width,
            look.hint(),
        ));
        lines.push(Line::raw(""));
    }
    if let Look::Themed(th) = look {
        let mut l = Lb::new();
        l.push(" ", look.text());
        for (name, group) in [
            ("● needs you", Group::NeedsMe),
            ("✗ failed", Group::Failed),
            ("✓ ready for review", Group::ReadyForReview),
            ("working", Group::Working),
        ] {
            l.push(name, th.fg(th.group_color(group)))
                .push(GAP, look.text());
        }
        lines.push(l.line());
    }
    lines
}

/// The Projects step: the folders with their ticks and what the scan found,
/// and the field for another one, kept in view.
fn project_lines(
    win: &SettingsWindow,
    w: &Welcome,
    inner: Rect,
    look: Look,
    cursor: &mut Option<(u16, u16)>,
) -> Vec<Line<'static>> {
    let width = usize::from(inner.width);
    let mut lines = text_lines(
        "The folders offered in the new-session dialog. Tick those that hold your projects.",
        width,
        look.secondary(),
    );
    lines.push(Line::raw(""));
    if w.projects.is_empty() {
        let text = if w.scanned {
            "None of the usual folders is here. Type yours below, or add them later in Settings \
             (c)."
        } else {
            "Looking in the usual folders under your home…"
        };
        lines.extend(text_lines(text, width, look.hint()));
        lines.push(Line::raw(""));
    }
    let n = w.projects.len();
    // The rows above and under the list: its text and the field's two.
    let rows = usize::from(inner.height)
        .saturating_sub(lines.len() + 3)
        .max(1);
    let offset = (w.project_row + 1).saturating_sub(rows).min(n);
    let folder_w = w
        .projects
        .iter()
        .map(|c| c.path.width())
        .max()
        .unwrap_or(0)
        .clamp(12, width.saturating_sub(30).max(12));
    for (i, choice) in w.projects.iter().enumerate().skip(offset).take(rows) {
        let selected = i == w.project_row;
        let bar = |st: Style| on_bar(look, selected, st);
        let plain = if selected {
            look.selected_item()
        } else {
            look.text()
        };
        let mut l = Lb::new();
        l.push(
            if selected { "▌" } else { " " },
            bar(look.selected_marker()),
        );
        l.push(" ", plain);
        l.push(
            if choice.ticked { "[x] " } else { "[ ] " },
            bar(if choice.ticked {
                look.warning()
            } else {
                look.hint()
            }),
        );
        l.push(format!("{}  ", fit(&choice.path, folder_w)), plain);
        let (note, kind) = win.folder_note(choice.list, &choice.path);
        l.push(
            fit(&note, width.saturating_sub(l.w + 1))
                .trim_end()
                .to_owned(),
            bar(kind_style(kind, look)),
        );
        lines.push(bar_line(l, selected, width, look));
    }
    if n > rows {
        let shown = format!(
            "   {} of {n} shown; ↑↓ moves",
            rows.min(n.saturating_sub(offset))
        );
        lines.push(Line::styled(shown, look.hint()));
    } else {
        lines.push(Line::raw(""));
    }
    lines.push(field_line(win, w, inner, lines.len() as u16, look, cursor));
    lines
}

/// The field for another folder, with what is known about the text in it.
fn field_line(
    win: &SettingsWindow,
    w: &Welcome,
    inner: Rect,
    y: u16,
    look: Look,
    cursor: &mut Option<(u16, u16)>,
) -> Line<'static> {
    let width = usize::from(inner.width);
    let focused = w.project_row == w.projects.len();
    let text = crate::settings::clean_folder(&w.field.text());
    let (note, kind) = if let Some(err) = &w.field_error {
        (format!("✗ {err}"), NoticeKind::Error)
    } else if text.is_empty() {
        (String::new(), NoticeKind::Info)
    } else {
        // What it would be added as (doc/0048); a scan's words do not apply.
        match win.folders.get(&text) {
            Some(FolderInfo::Missing) => ("not found".to_owned(), NoticeKind::Warning),
            Some(FolderInfo::NotFolder) => ("not a folder".to_owned(), NoticeKind::Warning),
            Some(FolderInfo::Folder { clones: 0, .. }) => {
                win.folder_note(SettingKey::ExtraProjects, &text)
            }
            _ => win.folder_note(SettingKey::ProjectRoots, &text),
        }
    };
    let border = if w.field_error.is_some() {
        look.error()
    } else if focused {
        look.key()
    } else {
        look.hint()
    };
    let mut l = Lb::new();
    l.push(if focused { "▌" } else { " " }, look.selected_marker());
    l.push(" + ", look.text());
    let note_w = note.width().min(30);
    let room = width.saturating_sub(l.w + 4 + note_w + 3).max(8);
    let field_w = (w.field.text().width() + 1).clamp(32.min(room), room);
    let (shown, col) = input_window(&w.field, field_w);
    l.push("[ ", border);
    let text_x = l.w;
    if shown.is_empty() && !focused {
        l.push(fit("another folder (~ allowed)", field_w), look.hint());
    } else if shown.is_empty() {
        l.push(fit("type a folder (~ allowed)", field_w), look.hint());
    } else {
        l.push(fit(&shown, field_w), look.strong());
    }
    l.push(" ]  ", border);
    l.push(
        fit(&note, width.saturating_sub(l.w + 1))
            .trim_end()
            .to_owned(),
        kind_style(kind, look),
    );
    if focused {
        *cursor = Some((inner.x + (text_x + col) as u16, inner.y + y));
    }
    l.line()
}

/// The Options step: each switch and a line on what it does.
fn option_lines(win: &SettingsWindow, w: &Welcome, width: usize, look: Look) -> Vec<Line<'static>> {
    let mut lines = text_lines(
        "Three switches. Each can change later in Settings (c).",
        width,
        look.secondary(),
    );
    lines.push(Line::raw(""));
    for (i, key) in OPTION_KEYS.iter().enumerate() {
        let selected = i == w.option_row;
        let bar = |st: Style| on_bar(look, selected, st);
        let plain = if selected {
            look.selected_item()
        } else {
            look.text()
        };
        let on = win.session.value(*key) == SettingValue::Bool(true);
        let (name, help) = option_text(*key);
        let mut l = Lb::new();
        l.push(
            if selected { "▌" } else { " " },
            bar(look.selected_marker()),
        );
        l.push(" ", plain);
        l.push(
            if on { "[x] " } else { "[ ] " },
            bar(if on { look.warning() } else { look.hint() }),
        );
        l.push(
            fit(name, 17),
            if selected {
                plain.add_modifier(Modifier::BOLD)
            } else {
                plain
            },
        );
        l.push(shown_value(*key, &SettingValue::Bool(on)), plain);
        lines.push(bar_line(l, selected, width, look));
        for t in wrap(help, width.saturating_sub(7)) {
            lines.push(Line::styled(format!("      {t}"), look.secondary()));
        }
        lines.push(Line::raw(""));
    }
    lines
}

/// The most rows the Done step gives to the list of folders.
const FOLDER_ROWS: usize = 3;

/// One row of the Done step's summary, `*` on a setting changed here.
fn summary_lines(
    win: &SettingsWindow,
    keys: &[SettingKey],
    label: &str,
    value: &str,
    width: usize,
    look: Look,
) -> Vec<Line<'static>> {
    let changed = win.session.exists() && keys.iter().any(|k| win.session.is_modified(*k));
    let mut parts = wrap_value(value, width.saturating_sub(21));
    if parts.len() > FOLDER_ROWS {
        parts.truncate(FOLDER_ROWS);
        parts[FOLDER_ROWS - 1].push_str(" …");
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let mut l = Lb::new();
            l.push(" ", look.text());
            l.push(if changed && i == 0 { "*" } else { " " }, look.modified());
            l.push(" ", look.text());
            l.push(fit(if i == 0 { label } else { "" }, 17), look.hint());
            l.push(part, look.text());
            l.line()
        })
        .collect()
}

/// The Done step: the summary and the file.
fn done_lines(win: &SettingsWindow, w: &Welcome, width: usize, look: Look) -> Vec<Line<'static>> {
    let n = win.change_count();
    let saving = if win.session.exists() {
        if n == 0 {
            "Nothing changed. Enter goes on to tower.".to_owned()
        } else {
            format!(
                "Enter saves {} to {} and goes on to tower.",
                if n == 1 {
                    "this 1 change".to_owned()
                } else {
                    format!("these {n} changes")
                },
                win.shown_path
            )
        }
    } else {
        format!("Enter saves to {} and goes on to tower.", win.shown_path)
    };
    let mut lines = Vec::new();
    let value = |key: SettingKey| shown_value(key, &win.session.value(key));
    let mut add = |keys: &[SettingKey], label: &str, value: String| {
        lines.extend(summary_lines(win, keys, label, &value, width, look));
    };
    add(&[SettingKey::UiStyle], "style", {
        let style = value(SettingKey::UiStyle);
        if style == "workspace" {
            format!("{style} (experimental)")
        } else {
            style
        }
    });
    add(&[SettingKey::UiTheme], "theme", value(SettingKey::UiTheme));
    if win.look_rows().contains(&SettingKey::UiCardSize) {
        add(
            &[SettingKey::UiCardSize],
            "card size",
            value(SettingKey::UiCardSize),
        );
    }
    let folders: Vec<&str> = w
        .projects
        .iter()
        .filter(|c| c.ticked)
        .map(|c| c.path.as_str())
        .collect();
    let folders = if folders.is_empty() {
        "none yet; add them in Settings (c)".to_owned()
    } else {
        folders.join(" · ")
    };
    add(
        &[SettingKey::ProjectRoots, SettingKey::ExtraProjects],
        "projects",
        folders,
    );
    for key in OPTION_KEYS {
        add(&[key], option_text(key).0, value(key));
    }
    lines.push(Line::raw(""));
    lines.extend(text_lines(&accounts_text(win), width, look.secondary()));
    lines.push(Line::raw(""));
    lines.extend(text_lines(&saving, width, look.strong()));
    lines
}

/// What the window says after a failed save.
fn failed_lines(w: &Welcome, err: &str, width: usize, look: Look) -> Vec<Line<'static>> {
    let mut lines = text_lines("The config could not be written:", width, look.text());
    lines.extend(text_lines(err, width, look.error()));
    lines.push(Line::raw(""));
    let next = match w.origin {
        Origin::FirstRun => {
            "Tower goes on with its built-in defaults for this run, and this window comes back \
             next time."
        }
        Origin::Setup => "The file stays as it was, and tower goes on with its current settings.",
    };
    lines.extend(text_lines(next, width, look.text()));
    lines.push(Line::raw(""));
    lines.extend(text_lines("Enter goes on to tower.", width, look.strong()));
    lines
}

/// The status line: the window's notice, else what a save does.
fn draw_status(frame: &mut Frame, win: &SettingsWindow, w: &Welcome, area: Rect, look: Look) {
    let width = usize::from(area.width).saturating_sub(1);
    let text = if let Some(notice) = &win.notice {
        Line::styled(
            format!(" {}", fit(&notice.text, width).trim_end()),
            kind_style(notice.kind, look),
        )
    } else {
        let text = match w.origin {
            Origin::FirstRun => "Nothing is written until you save. Ctrl+C quits without writing.",
            Origin::Setup => "Nothing is written until you save. Esc cancels.",
        };
        Line::styled(
            format!(" {}", fit(text, width).trim_end()),
            look.secondary(),
        )
    };
    put(frame, area, text);
}

/// The footer keys of the current step.
fn footer_keys(win: &SettingsWindow, w: &Welcome) -> Vec<(&'static str, &'static str)> {
    let esc = match w.origin {
        Origin::FirstRun => ("Esc", "skip, save"),
        Origin::Setup => ("Esc", "cancel"),
    };
    let quit = ("Ctrl+C", "quit");
    if matches!(win.mode, Mode::Conflict(_)) {
        return vec![("y", "overwrite"), ("n", "cancel"), quit];
    }
    if w.failed.is_some() {
        return vec![("Enter", "go on"), quit];
    }
    if win.saving {
        return vec![quit];
    }
    let back = ("Bksp", "back");
    match w.step {
        Step::Welcome => vec![("Enter", "next"), esc, quit],
        Step::Look => vec![
            ("Enter", "next"),
            back,
            ("↑↓", "move"),
            ("←→", "change"),
            esc,
            quit,
        ],
        Step::Projects if win.welcome_takes_text() && !w.field.text().is_empty() => vec![
            ("Enter", "add"),
            ("Tab", "complete"),
            ("Esc", "clear"),
            ("↑", "list"),
            quit,
        ],
        Step::Projects if win.welcome_takes_text() => {
            vec![
                ("Enter", "next"),
                ("Bksp", "back"),
                ("↑", "list"),
                esc,
                quit,
            ]
        }
        Step::Projects => vec![
            ("Space", "tick"),
            ("Enter", "next"),
            back,
            ("↑↓", "move"),
            esc,
            quit,
        ],
        Step::Options => vec![
            ("Space", "switch"),
            ("Enter", "next"),
            back,
            ("↑↓", "move"),
            esc,
            quit,
        ],
        Step::Done if win.session.exists() && !win.session.has_changes() => {
            vec![("Enter", "go on"), back, esc, quit]
        }
        // In a first start `Esc` here does what `Enter` does.
        Step::Done if w.origin == Origin::FirstRun => vec![("Enter", "save"), back, quit],
        Step::Done => vec![("Enter", "save"), back, esc, quit],
    }
}

/// The footer: the whole key hints that fit.
fn draw_footer(frame: &mut Frame, win: &SettingsWindow, w: &Welcome, area: Rect, look: Look) {
    let width = usize::from(area.width);
    let mut l = Lb::new();
    l.push(" ", look.key_label());
    for (i, (k, v)) in footer_keys(win, w).into_iter().enumerate() {
        let item = k.width() + 1 + v.width() + GAP.width();
        if l.w + item > width {
            break;
        }
        if i > 0 {
            l.push(GAP, look.key_label());
        }
        l.push(k, look.key())
            .push(format!(" {v}"), look.key_label());
    }
    put(frame, area, l.line());
}
