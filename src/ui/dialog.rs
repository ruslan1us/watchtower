//! Rendering of the new-session dialog (doc/0008), with its template
//! picker over it (doc/0036), the resume window of `R` (doc/0019) and the
//! rename window of `e` (doc/0027). Pure drawing from the [`Dialog`],
//! [`ResumeWindow`] and [`RenameWindow`] state.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Look;
use crate::app::App;
use crate::dialog::{
    Choice, Dialog, DialogPrompt, DialogStatus, Field, LineInput, RenameWindow, ResumeField,
    ResumeWindow,
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
pub(super) fn input_window(input: &LineInput, width: usize) -> (String, usize) {
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

fn label(field: Field, focused: Field, look: Look) -> Span<'static> {
    let on = field == focused;
    let text = format!(
        "{}{:<w$}",
        if on { "▶ " } else { "  " },
        field.label(),
        w = usize::from(LABEL_W) - 2
    );
    Span::styled(text, look.field_label(on))
}

fn choice_spans(choice: &Choice, on: bool, extra: &str, look: Look) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("‹ {} ›", choice.value()),
        look.choice(on),
    )];
    if choice.items().len() > 1 {
        spans.push(Span::styled(
            format!("  {}/{}", choice.index() + 1, choice.items().len()),
            look.hint(),
        ));
    }
    if !extra.is_empty() {
        spans.push(Span::styled(format!("  {extra}"), look.hint()));
    }
    spans
}

/// One row of the project list: `▶` and the selection bar on the selected
/// project (bold while the list is not focused), `value_w` columns wide.
fn project_line(
    display: &str,
    selected: bool,
    focused: bool,
    value_w: usize,
    look: Look,
) -> Line<'static> {
    let text = super::fit(
        &format!("{} {display}", if selected { "▶" } else { " " }),
        value_w,
    );
    let style = match (selected, focused) {
        (false, _) => look.item(),
        (true, true) => look.selected_item(),
        (true, false) => look.chosen_item(),
    };
    let mut spans = vec![Span::raw(" ".repeat(usize::from(LABEL_W)))];
    if selected {
        let rest = text.strip_prefix('▶').unwrap_or(&text).to_owned();
        spans.push(Span::styled("▶", style.patch(look.selected_marker())));
        spans.push(Span::styled(rest, style));
    } else {
        spans.push(Span::styled(text, style));
    }
    Line::from(spans)
}

/// The project field's first row: the label, the filter (or its
/// placeholder) and the match count.
fn filter_spans(
    input: &LineInput,
    focused: Field,
    shown: String,
    count: String,
    look: Look,
) -> Vec<Span<'static>> {
    let mut spans = vec![label(Field::Project, focused, look)];
    if input.is_empty() && focused == Field::Project {
        spans.push(Span::styled("type to filter", look.hint()));
    } else {
        spans.push(Span::styled(shown, look.text()));
    }
    spans.push(Span::styled(count, look.hint()));
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
pub fn draw(frame: &mut Frame, app: &App, dialog: &Dialog, area: Rect, look: Look) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
    let width = usize::from(w.saturating_sub(2)).max(1);
    let value_w = width.saturating_sub(usize::from(LABEL_W)).max(1);
    let focused = dialog.field;
    // The template picker over the dialog has the keys and the cursor.
    let editing = dialog.prompt.is_none() && !dialog.is_starting() && dialog.templates.is_none();

    // Messages below the fields, wrapped to the dialog width.
    let mut messages: Vec<Line> = Vec::new();
    let warn = look.warning();
    let err = look.error();
    if let Some(first) = dialog.project_warnings.first() {
        let more = dialog.project_warnings.len() - 1;
        let text = if more > 0 {
            format!("{first} (+{more} more)")
        } else {
            first.clone()
        };
        for row in wrap(&text, width) {
            messages.push(Line::styled(row, look.hint()));
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
            look.strong(),
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
                messages.push(look.text_line(row, look.strong()));
            }
        }
    }
    match dialog.prompt {
        None => {}
        Some(DialogPrompt::Discard) => messages.push(Line::styled(
            "Discard this task?  y discard · n keep editing",
            look.prompt(),
        )),
        Some(DialogPrompt::Quit) => messages.push(Line::styled(
            "Quit tower? The dialog's contents are lost.  y quit · n stay",
            look.prompt(),
        )),
    }

    let enter_hint = if focused == Field::Task {
        "Enter new line"
    } else {
        "Enter next field"
    };
    let footer_text = format!(
        "Tab/Shift+Tab field · {enter_hint} · Ctrl+S start · {} start & attach · Ctrl+G editor \
         · Ctrl+P template",
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
    let block = look
        .overlay_block(" New session ")
        .title_bottom(look.hint_line(" Esc cancel "));
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
    let spans = filter_spans(
        &dialog.filter,
        focused,
        shown,
        format!("   {} of {}", filtered.len(), dialog.projects.len()),
        look,
    );
    if focused == Field::Project && editing && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(spans));
    if filtered.is_empty() {
        for row in empty_rows.iter().take(project_rows) {
            p.line(Line::styled(
                format!("{:w$}{row}", "", w = usize::from(LABEL_W)),
                look.hint(),
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
            p.line(project_line(
                &project.display,
                i == sel,
                focused == Field::Project,
                value_w,
                look,
            ));
        }
    }
    p.line(Line::raw(""));

    // Task: label row, then the wrapped text.
    let mut spans = vec![label(Field::Task, focused, look)];
    if dialog.task.is_blank() {
        spans.extend(look.hint_spans(
            "describe the task · Enter new line · Ctrl+G opens $VISUAL / $EDITOR · Ctrl+P \
                 inserts a template",
        ));
    } else {
        spans.push(Span::styled("", look.hint()));
    }
    p.line(Line::from(spans));
    let (rows, (crow, ccol)) = task_rows(dialog, value_w);
    let start = if crow < task_h { 0 } else { crow + 1 - task_h };
    for i in start..start + task_h {
        let text = rows.get(i).cloned().unwrap_or_default();
        if i == crow && focused == Field::Task && editing && p.has_room() {
            cursor = Some(Position::new(value_x + ccol as u16, p.y));
        }
        p.line(Line::from(vec![
            Span::styled(
                format!("{:>w$}", "┊ ", w = usize::from(LABEL_W)),
                look.hint(),
            ),
            Span::styled(text, look.text()),
        ]));
    }
    p.line(Line::raw(""));

    // Name.
    let (shown, cx) = input_window(&dialog.name, value_w);
    let mut spans = vec![label(Field::Name, focused, look)];
    if dialog.name.is_empty() {
        spans.push(Span::styled("optional", look.hint()));
    } else {
        spans.push(Span::styled(shown, look.text()));
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
    let mut spans = vec![label(Field::Account, focused, look)];
    spans.extend(choice_spans(
        &dialog.account,
        focused == Field::Account,
        &command,
        look,
    ));
    p.line(Line::from(spans));

    // Model: free text with presets.
    let hint = format!("  ↑↓ {} · or type a model name", MODEL_CHOICES.join(" · "));
    let field_w = value_w.saturating_sub(hint.width()).clamp(8, 40);
    let (shown, cx) = input_window(&dialog.model, field_w);
    let mut model_style = look.strong();
    if focused == Field::Model {
        model_style = model_style.add_modifier(Modifier::UNDERLINED);
    }
    if focused == Field::Model && editing && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    let mut spans = vec![
        label(Field::Model, focused, look),
        Span::styled(super::fit(&shown, field_w), model_style),
    ];
    spans.extend(look.hint_spans(hint));
    p.line(Line::from(spans));

    // Effort and permission mode.
    let mut spans = vec![label(Field::Effort, focused, look)];
    spans.extend(choice_spans(
        &dialog.effort,
        focused == Field::Effort,
        "",
        look,
    ));
    p.line(Line::from(spans));
    let mut spans = vec![label(Field::Mode, focused, look)];
    spans.extend(choice_spans(
        &dialog.mode,
        focused == Field::Mode,
        "permission mode",
        look,
    ));
    p.line(Line::from(spans));
    p.line(Line::raw(""));

    for line in messages {
        p.line(line);
    }
    for row in footer {
        p.line(look.hint_line(row));
    }

    if let Some(pos) = cursor
        && pos.x < inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
    if let Some(picker) = &dialog.templates {
        super::templates::draw_picker(p.frame, picker, area, look);
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
pub fn draw_resume(frame: &mut Frame, app: &App, window: &ResumeWindow, area: Rect, look: Look) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
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
            messages.push(Line::styled(row, look.hint()));
        }
    }
    let command = window
        .selected_account(&app.config().accounts)
        .map(|a| a.command.clone())
        .unwrap_or_default();
    for row in wrap(&resume_note(&command), width) {
        messages.push(Line::styled(row, look.hint()));
    }
    if let Some(text) = &window.error {
        for row in wrap(text, width) {
            messages.push(Line::styled(row, look.error()));
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
    let block = look
        .overlay_block(" Resume a past conversation ")
        .title_bottom(look.hint_line(" Esc cancel "));
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
    let spans = filter_spans(
        &window.filter,
        focused,
        shown,
        format!("   {} of {}", filtered.len(), window.projects.len()),
        look,
    );
    if focused == Field::Project && p.has_room() {
        cursor = Some(Position::new(value_x + cx as u16, p.y));
    }
    p.line(Line::from(spans));
    if filtered.is_empty() {
        for row in empty_rows.iter().take(project_rows) {
            p.line(Line::styled(
                format!("{:w$}{row}", "", w = usize::from(LABEL_W)),
                look.hint(),
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
            p.line(project_line(
                &project.display,
                i == sel,
                focused == Field::Project,
                value_w,
                look,
            ));
        }
    }
    p.line(Line::raw(""));

    let mut spans = vec![label(Field::Account, focused, look)];
    spans.extend(choice_spans(
        &window.account,
        focused == Field::Account,
        &command,
        look,
    ));
    p.line(Line::from(spans));
    p.line(Line::raw(""));

    for line in messages {
        p.line(line);
    }
    for row in footer {
        p.line(look.hint_line(row));
    }

    if let Some(pos) = cursor
        && pos.x < inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
}

/// The rename window's explanation of what `Enter` does (doc/0027), for a
/// stopped session: tower wakes it with `/rename` and stops it again.
pub fn rename_stopped_note(old: &str, owner: Option<&str>) -> String {
    let who = match owner {
        Some(owner) => format!("under {owner}"),
        None => "with the account you pick (its owner is unknown)".to_owned(),
    };
    format!(
        "{old} is stopped. Enter wakes it {who} in the background without attaching, runs \
         /rename there (a local command, no model turn), waits until the list shows the new \
         name and stops it again."
    )
}

/// Draws the rename window of `e` centred in `area` (doc/0027): the name
/// field, what `Enter` does, an error and the keys.
pub fn draw_rename(frame: &mut Frame, _app: &App, window: &RenameWindow, area: Rect, look: Look) {
    let w = area.width.saturating_sub(4).clamp(1, 80);
    let width = usize::from(w.saturating_sub(4)).max(1);
    let value_w = width.saturating_sub(usize::from(LABEL_W)).max(1);
    let typed = window.name.text();
    let mut body: Vec<Line> = Vec::new();
    if window.running {
        let intro = format!(
            "Claude Code renames a running session from inside it. Enter attaches to {} (as \
             Enter on its row); type there:",
            window.old_name
        );
        for row in wrap(&intro, width) {
            body.push(Line::styled(row, look.text()));
        }
        let command = crate::rename::rename_prompt(&typed);
        for row in wrap(&command, width.saturating_sub(4).max(1)) {
            body.push(Line::styled(format!("    {row}"), look.strong()));
        }
        for row in wrap(
            "and press Enter, then Ctrl+Z to come back to tower; tower then checks the name.",
            width,
        ) {
            body.push(Line::styled(row, look.text()));
        }
    } else {
        let note = rename_stopped_note(&window.old_name, window.owner.as_deref());
        for row in wrap(&note, width) {
            body.push(Line::styled(row, look.text()));
        }
    }
    if let Some(err) = &window.error {
        body.push(Line::raw(""));
        for row in wrap(err, width) {
            body.push(Line::styled(row, look.error()));
        }
    }
    let enter = if window.running {
        "Enter attach"
    } else {
        "Enter rename"
    };
    let footer = wrap(&format!("{enter} · Ctrl+U clear · Esc cancel"), width);

    // Name, blank, body, blank, footer.
    let wanted = 2 + body.len() + 1 + footer.len();
    let h = (wanted as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = look
        .overlay_block(" Rename session ")
        .title_bottom(look.hint_line(" Esc cancel "));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // One column of padding inside the border, as the other windows.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    let mut p = Painter {
        frame,
        area: inner,
        y: inner.y,
    };
    let (shown, cx) = input_window(&window.name, value_w);
    let cursor = p
        .has_room()
        .then(|| Position::new(inner.x + LABEL_W + cx as u16, p.y));
    p.line(Line::from(vec![
        label(Field::Name, Field::Name, look),
        Span::styled(shown, look.text()),
    ]));
    p.line(Line::raw(""));
    for line in body {
        p.line(line);
    }
    p.line(Line::raw(""));
    for row in footer {
        p.line(look.hint_line(row));
    }
    if let Some(pos) = cursor
        && pos.x < inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
}
