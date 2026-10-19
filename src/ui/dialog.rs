//! Rendering of the new-session dialog (doc/0008) and of the resume window
//! of `R` (doc/0019). Pure drawing from the [`Dialog`] and [`ResumeWindow`]
//! state.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::dialog::{
    Choice, Dialog, DialogPrompt, DialogStatus, Field, LineInput, ResumeField, ResumeWindow,
};
use crate::dispatch::MODEL_CHOICES;

/// Width of the label column (marker + label + gap).
const LABEL_W: u16 = 10;
/// At most this many project rows are listed.
const PROJECT_ROWS: usize = 6;
/// At most this many task rows are shown.
const TASK_ROWS: usize = 10;

/// Wraps `text` into rows of at most `width` display columns, breaking
/// anywhere: the task text, where every character must stay where the
/// cursor arithmetic expects it.
fn wrap_chars(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for ch in line.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > width && !row.is_empty() {
                out.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(ch);
            used += cw;
        }
        out.push(row);
    }
    out
}

/// Wraps `text` into rows of at most `width` display columns at spaces,
/// breaking a word only when it is longer than a row. Line breaks in `text`
/// are kept. For messages, hints and the refusal text.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for word in line.split(' ') {
            let ww = word.width();
            let gap = usize::from(!row.is_empty());
            if used + gap + ww <= width {
                if gap == 1 {
                    row.push(' ');
                }
                row.push_str(word);
                used += gap + ww;
                continue;
            }
            if !row.is_empty() {
                out.push(std::mem::take(&mut row));
            }
            if ww <= width {
                row.push_str(word);
                used = ww;
                continue;
            }
            // A word longer than a row: hard breaks.
            let mut pieces = wrap_chars(word, width);
            let last = pieces.pop().unwrap_or_default();
            out.extend(pieces);
            used = last.width();
            row = last;
        }
        out.push(row);
    }
    out
}

/// The visible part of a line input `width` columns wide, scrolled so the
/// cursor stays visible, and the cursor's column within it.
fn input_window(input: &LineInput, width: usize) -> (String, usize) {
    let chars: Vec<char> = input.text().chars().collect();
    let cursor = input.cursor().min(chars.len());
    let width = width.max(1);
    let col = |range: &[char]| range.iter().map(|c| c.width().unwrap_or(0)).sum::<usize>();
    // Start as far left as possible while the cursor still fits.
    let mut start = 0;
    while start < cursor && col(&chars[start..cursor]) >= width {
        start += 1;
    }
    let mut shown = String::new();
    let mut used = 0;
    for ch in &chars[start..] {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width {
            break;
        }
        shown.push(*ch);
        used += cw;
    }
    (shown, col(&chars[start..cursor]))
}

/// One logical line wrapped like [`wrap`], plus the visual (row, column) of
/// the cursor placed before char `cursor` (or at the end). A cursor after a
/// full last row gets an extra empty row.
fn wrap_with_cursor(line: &str, width: usize, cursor: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    let mut pos = None;
    for (k, ch) in line.chars().enumerate() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        if k == cursor {
            pos = Some((rows.len(), used));
        }
        row.push(ch);
        used += cw;
    }
    rows.push(row);
    let pos = pos.unwrap_or_else(|| {
        if used >= width {
            rows.push(String::new());
            (rows.len() - 1, 0)
        } else {
            (rows.len() - 1, used)
        }
    });
    (rows, pos)
}

/// Visual rows of the task wrapped to `width`, and the cursor's visual
/// (row, column).
fn task_rows(dialog: &Dialog, width: usize) -> (Vec<String>, (usize, usize)) {
    let (crow, ccol) = dialog.task.cursor();
    let mut rows = Vec::new();
    let mut cursor = (0, 0);
    for (i, line) in dialog.task.lines().iter().enumerate() {
        if i == crow {
            let (wrapped, (r, c)) = wrap_with_cursor(line, width, ccol);
            cursor = (rows.len() + r, c);
            rows.extend(wrapped);
        } else {
            rows.extend(wrap_chars(line, width));
        }
    }
    (rows, cursor)
}

struct Painter<'a, 'b> {
    frame: &'a mut Frame<'b>,
    area: Rect,
    y: u16,
}

impl Painter<'_, '_> {
    fn bottom(&self) -> u16 {
        self.area.y + self.area.height
    }

    fn has_room(&self) -> bool {
        self.y < self.bottom()
    }

    /// Draws one line at the current row and moves down.
    fn line(&mut self, line: Line<'_>) {
        if self.has_room() {
            let rect = Rect {
                x: self.area.x,
                y: self.y,
                width: self.area.width,
                height: 1,
            };
            self.frame.render_widget(Paragraph::new(line), rect);
        }
        self.y += 1;
    }
}

fn label(field: Field, focused: Field, color: bool) -> Span<'static> {
    let on = field == focused;
    let text = format!(
        "{}{:<w$}",
        if on { "▶ " } else { "  " },
        field.label(),
        w = usize::from(LABEL_W) - 2
    );
    let mut style = Style::new();
    if on {
        style = style.add_modifier(Modifier::BOLD);
        if color {
            style = style.fg(Color::Cyan);
        }
    }
    Span::styled(text, style)
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn choice_spans(choice: &Choice, on: bool, extra: &str) -> Vec<Span<'static>> {
    let mut style = Style::new().add_modifier(Modifier::BOLD);
    if on {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let mut spans = vec![Span::styled(format!("‹ {} ›", choice.value()), style)];
    if choice.items().len() > 1 {
        spans.push(Span::styled(
            format!("  {}/{}", choice.index() + 1, choice.items().len()),
            dim(),
        ));
    }
    if !extra.is_empty() {
        spans.push(Span::styled(format!("  {extra}"), dim()));
    }
    spans
}

/// The `Ctrl+Enter` variant that works in this terminal (doc/0011).
pub fn attach_key(app: &App) -> &'static str {
    if app.keyboard_enhanced {
        "Ctrl+Enter (or Alt+Enter)"
    } else {
        "Alt+Enter"
    }
}

/// The project list's note when nothing is listed: still scanning, no
/// project at all (naming the config file and whether it was loaded), or
/// none matching the filter.
fn empty_project_text(app: &App, scanning: bool, total: usize) -> String {
    if scanning {
        "scanning projects…".to_owned()
    } else if total == 0 {
        app.no_projects_text()
    } else {
        "no project matches the filter".to_owned()
    }
}

/// Draws the dialog centred in `area`.
pub fn draw(frame: &mut Frame, app: &App, dialog: &Dialog, area: Rect) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
    let color = app.color;
    let width = usize::from(w.saturating_sub(2)).max(1);
    let value_w = width.saturating_sub(usize::from(LABEL_W)).max(1);
    let focused = dialog.field;
    let editing = dialog.prompt.is_none() && !dialog.is_starting();

    // Messages below the fields, wrapped to the dialog width.
    let mut messages: Vec<Line> = Vec::new();
    let warn = if color {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let err = if color {
        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    if let Some(first) = dialog.project_warnings.first() {
        let more = dialog.project_warnings.len() - 1;
        let text = if more > 0 {
            format!("{first} (+{more} more)")
        } else {
            first.clone()
        };
        for row in wrap(&text, width) {
            messages.push(Line::styled(row, dim()));
        }
    }
    if let Some(text) = dialog.haiku_warning() {
        for row in wrap(&format!("⚠ {text}"), width) {
            messages.push(Line::styled(row, warn));
        }
    }
    if let Some(text) = &dialog.error {
        for row in wrap(text.trim(), width) {
            messages.push(Line::styled(row, err));
        }
    }
    match &dialog.status {
        DialogStatus::Editing => {}
        DialogStatus::Starting { .. } => messages.push(Line::styled(
            "starting… (a supervisor may have to start first; this can take a few seconds)",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        DialogStatus::Untrusted { dir, .. } => {
            let account = app
                .config()
                .accounts
                .get(dialog.account.index())
                .map(|a| a.command.as_str())
                .unwrap_or("claude");
            let dir = crate::paths::shorten_home(&dir.to_string_lossy(), app.home());
            let text = format!(
                "Ctrl+T: run `{account}` in {dir} to accept Claude Code's trust prompt, then \
                 /exit; tower retries the dispatch afterwards"
            );
            for row in wrap(&text, width) {
                messages.push(Line::styled(row, Style::new().add_modifier(Modifier::BOLD)));
            }
        }
    }
    match dialog.prompt {
        None => {}
        Some(DialogPrompt::Discard) => messages.push(Line::styled(
            "Discard this task?  y discard · n keep editing",
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )),
        Some(DialogPrompt::Quit) => messages.push(Line::styled(
            "Quit tower? The dialog's contents are lost.  y quit · n stay",
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )),
    }

    let enter_hint = if focused == Field::Task {
        "Enter new line"
    } else {
        "Enter next field"
    };
    let footer_text = format!(
        "Tab/Shift+Tab field · {enter_hint} · Ctrl+S start · {} start & attach · Ctrl+G editor",
        attach_key(app)
    );
    let footer: Vec<String> = wrap(&footer_text, width);

    // Fixed rows: project label, blank, task label, blank, five fields, blank.
    let fixed = 10 + messages.len() + footer.len();
    let filtered = dialog.filtered();
    // Without a project to list, a note (wrapped) takes the list's rows.
    let empty_rows: Vec<String> = if filtered.is_empty() {
        wrap(
            &empty_project_text(app, dialog.scanning, dialog.projects.len()),
            value_w,
        )
    } else {
        Vec::new()
    };
    let list_len = filtered.len().max(empty_rows.len());
    let (task_wanted, _) = task_rows(dialog, value_w);
    let wanted = fixed + list_len.clamp(1, PROJECT_ROWS) + task_wanted.len().clamp(3, TASK_ROWS);
    let h = (wanted as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" New session ")
        .title_bottom(Line::styled(" Esc cancel ", dim()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let available = usize::from(inner.height).saturating_sub(fixed);
    let project_rows = list_len.clamp(1, PROJECT_ROWS).min((available / 2).max(1));
    let task_h = available.saturating_sub(project_rows).clamp(1, TASK_ROWS);

    let mut p = Painter {
        frame,
        area: inner,
        y: inner.y,
    };
    let mut cursor: Option<Position> = None;
    let value_x = inner.x + LABEL_W;

    // Project: filter line, then the matching projects.
    let (shown, cx) = input_window(&dialog.filter, value_w.saturating_sub(16));
    let mut spans = vec![label(Field::Project, focused, color)];
    if dialog.filter.is_empty() && focused == Field::Project {
        spans.push(Span::styled("type to filter", dim()));
    } else {
        spans.push(Span::raw(shown));
    }
    spans.push(Span::styled(
        format!("   {} of {}", filtered.len(), dialog.projects.len()),
        dim(),
    ));
    if focused == Field::Project && editing && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(spans));
    if filtered.is_empty() {
        for row in empty_rows.iter().take(project_rows) {
            p.line(Line::styled(
                format!("{:w$}{row}", "", w = usize::from(LABEL_W)),
                dim(),
            ));
        }
    } else {
        let sel = dialog.project_index.min(filtered.len() - 1);
        let start = if sel < project_rows {
            0
        } else {
            sel + 1 - project_rows
        };
        for (i, project) in filtered.iter().enumerate().skip(start).take(project_rows) {
            let selected = i == sel;
            let mut style = Style::new();
            if selected {
                style = style.add_modifier(if focused == Field::Project {
                    Modifier::REVERSED
                } else {
                    Modifier::BOLD
                });
            }
            let text = super::fit(
                &format!("{} {}", if selected { "▶" } else { " " }, project.display),
                value_w,
            );
            p.line(Line::from(vec![
                Span::raw(" ".repeat(usize::from(LABEL_W))),
                Span::styled(text, style),
            ]));
        }
    }
    p.line(Line::raw(""));

    // Task: label row, then the wrapped text.
    p.line(Line::from(vec![
        label(Field::Task, focused, color),
        Span::styled(
            if dialog.task.is_blank() {
                "describe the task · Enter new line · Ctrl+G opens $VISUAL / $EDITOR"
            } else {
                ""
            },
            dim(),
        ),
    ]));
    let (rows, (crow, ccol)) = task_rows(dialog, value_w);
    let start = if crow < task_h { 0 } else { crow + 1 - task_h };
    for i in start..start + task_h {
        let text = rows.get(i).cloned().unwrap_or_default();
        if i == crow && focused == Field::Task && editing && p.has_room() {
            cursor = Some(Position::new(value_x + ccol as u16, p.y));
        }
        p.line(Line::from(vec![
            Span::styled(format!("{:>w$}", "┊ ", w = usize::from(LABEL_W)), dim()),
            Span::raw(text),
        ]));
    }
    p.line(Line::raw(""));

    // Name.
    let (shown, cx) = input_window(&dialog.name, value_w);
    let mut spans = vec![label(Field::Name, focused, color)];
    if dialog.name.is_empty() {
        spans.push(Span::styled("optional", dim()));
    } else {
        spans.push(Span::raw(shown));
    }
    if focused == Field::Name && editing && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(spans));

    // Account.
    let command = app
        .config()
        .accounts
        .get(dialog.account.index())
        .map(|a| a.command.clone())
        .unwrap_or_default();
    let mut spans = vec![label(Field::Account, focused, color)];
    spans.extend(choice_spans(
        &dialog.account,
        focused == Field::Account,
        &command,
    ));
    p.line(Line::from(spans));

    // Model: free text with presets.
    let hint = format!("  ↑↓ {} · or type a model name", MODEL_CHOICES.join(" · "));
    let field_w = value_w.saturating_sub(hint.width()).clamp(8, 40);
    let (shown, cx) = input_window(&dialog.model, field_w);
    let mut model_style = Style::new().add_modifier(Modifier::BOLD);
    if focused == Field::Model {
        model_style = model_style.add_modifier(Modifier::UNDERLINED);
    }
    if focused == Field::Model && editing && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(vec![
        label(Field::Model, focused, color),
        Span::styled(super::fit(&shown, field_w), model_style),
        Span::styled(hint, dim()),
    ]));

    // Effort and permission mode.
    let mut spans = vec![label(Field::Effort, focused, color)];
    spans.extend(choice_spans(&dialog.effort, focused == Field::Effort, ""));
    p.line(Line::from(spans));
    let mut spans = vec![label(Field::Mode, focused, color)];
    spans.extend(choice_spans(
        &dialog.mode,
        focused == Field::Mode,
        "permission mode",
    ));
    p.line(Line::from(spans));
    p.line(Line::raw(""));

    for line in messages {
        p.line(line);
    }
    for row in footer {
        p.line(Line::styled(row, dim()));
    }

    if let Some(pos) = cursor
        && pos.x < inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
}

/// At most this many project rows are listed in the resume window.
const RESUME_PROJECT_ROWS: usize = 8;

/// The note of the resume window: what `Enter` runs (`command` is the
/// chosen account's command) and how to come back with the conversation in
/// the background (doc/0019).
pub fn resume_note(command: &str) -> String {
    format!(
        "Enter runs `{command} --resume` in the folder: Claude Code's own picker of past \
         conversations (Ctrl+A there shows all projects). Then press ← on an empty prompt and \
         Ctrl+C twice to come back to tower (Esc there reopens the conversation); it keeps \
         running in the background as a copy and appears in tower."
    )
}

/// Draws the resume window of `R` centred in `area` (doc/0019): the
/// project list with its filter, the account, a note and the keys.
pub fn draw_resume(frame: &mut Frame, app: &App, window: &ResumeWindow, area: Rect) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
    let color = app.color;
    let width = usize::from(w.saturating_sub(2)).max(1);
    let value_w = width.saturating_sub(usize::from(LABEL_W)).max(1);
    // The resume window reuses the dialog's labels for its two fields.
    let focused = match window.field {
        ResumeField::Project => Field::Project,
        ResumeField::Account => Field::Account,
    };

    let filtered = window.filtered();
    let empty_rows: Vec<String> = if filtered.is_empty() {
        wrap(
            &empty_project_text(app, window.scanning, window.projects.len()),
            value_w,
        )
    } else {
        Vec::new()
    };
    let list_len = filtered.len().max(empty_rows.len());
    let mut messages: Vec<Line> = Vec::new();
    if let Some(first) = window.project_warnings.first() {
        for row in wrap(first, width) {
            messages.push(Line::styled(row, dim()));
        }
    }
    let command = window
        .selected_account(&app.config().accounts)
        .map(|a| a.command.clone())
        .unwrap_or_default();
    for row in wrap(&resume_note(&command), width) {
        messages.push(Line::styled(row, dim()));
    }
    if let Some(text) = &window.error {
        let err = if color {
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        for row in wrap(text, width) {
            messages.push(Line::styled(row, err));
        }
    }
    let footer = wrap(
        "Tab/Shift+Tab field · ↑↓ choose · type to filter · Enter open the picker · Esc cancel",
        width,
    );

    // Fixed rows: project label, blank, account, blank.
    let fixed = 4 + messages.len() + footer.len();
    let wanted = fixed + list_len.clamp(1, RESUME_PROJECT_ROWS);
    let h = (wanted as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" Resume a past conversation ")
        .title_bottom(Line::styled(" Esc cancel ", dim()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let available = usize::from(inner.height).saturating_sub(fixed);
    let project_rows = list_len.clamp(1, RESUME_PROJECT_ROWS).min(available.max(1));

    let mut p = Painter {
        frame,
        area: inner,
        y: inner.y,
    };
    let mut cursor: Option<Position> = None;
    let value_x = inner.x + LABEL_W;

    let (shown, cx) = input_window(&window.filter, value_w.saturating_sub(16));
    let mut spans = vec![label(Field::Project, focused, color)];
    if window.filter.is_empty() && focused == Field::Project {
        spans.push(Span::styled("type to filter", dim()));
    } else {
        spans.push(Span::raw(shown));
    }
    spans.push(Span::styled(
        format!("   {} of {}", filtered.len(), window.projects.len()),
        dim(),
    ));
    if focused == Field::Project && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(spans));
    if filtered.is_empty() {
        for row in empty_rows.iter().take(project_rows) {
            p.line(Line::styled(
                format!("{:w$}{row}", "", w = usize::from(LABEL_W)),
                dim(),
            ));
        }
    } else {
        let sel = window.project_index.min(filtered.len() - 1);
        let start = if sel < project_rows {
            0
        } else {
            sel + 1 - project_rows
        };
        for (i, project) in filtered.iter().enumerate().skip(start).take(project_rows) {
            let selected = i == sel;
            let mut style = Style::new();
            if selected {
                style = style.add_modifier(if focused == Field::Project {
                    Modifier::REVERSED
                } else {
                    Modifier::BOLD
                });
            }
            let text = super::fit(
                &format!("{} {}", if selected { "▶" } else { " " }, project.display),
                value_w,
            );
            p.line(Line::from(vec![
                Span::raw(" ".repeat(usize::from(LABEL_W))),
                Span::styled(text, style),
            ]));
        }
    }
    p.line(Line::raw(""));

    let mut spans = vec![label(Field::Account, focused, color)];
    spans.extend(choice_spans(
        &window.account,
        focused == Field::Account,
        &command,
    ));
    p.line(Line::from(spans));
    p.line(Line::raw(""));

    for line in messages {
        p.line(line);
    }
    for row in footer {
        p.line(Line::styled(row, dim()));
    }

    if let Some(pos) = cursor
        && pos.x < inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
}
