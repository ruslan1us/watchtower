//! Rendering of the new-session dialog (doc/0008), with its template
//! picker over it (doc/0036), the resume window of `R` (doc/0019), the
//! rename window of `e` (doc/0027) and the quick reply window of `t` with
//! its template picker (doc/0040); both text fields host the slash-command
//! menu (doc/0042). Pure drawing from the [`Dialog`],
//! [`ResumeWindow`], [`RenameWindow`] and [`ReplyWindow`] state.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Look;
use crate::app::App;
use crate::dialog::{
    Choice, Dialog, DialogPrompt, DialogStatus, Field, LineInput, RenameWindow, ReplyWindow,
    ResumeField, ResumeWindow, TextArea,
};
use crate::dispatch::MODEL_CHOICES;
use crate::templates::TemplatePicker;

/// Width of the label column (marker + label + gap).
const LABEL_W: u16 = 10;
/// At most this many project rows are listed.
const PROJECT_ROWS: usize = 6;
/// At most this many task rows are shown.
const TASK_ROWS: usize = 10;

/// Wraps `text` into rows of at most `width` display columns, breaking
/// anywhere: the pieces of a word longer than a row.
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
///
/// A ` · ` between two items goes with the item after it, so a row never
/// ends with `·`; where the rows break between them it is left out (the
/// break separates them), so no row starts with `·` either.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        let words: Vec<&str> = line.split(' ').collect();
        let mut i = 0;
        while i < words.len() {
            // ` · ` and the next word, inside a row; a lone word otherwise.
            let sep = words[i] == "·" && i + 1 < words.len() && !row.is_empty();
            let word = words[i + usize::from(sep)];
            i += 1 + usize::from(sep);
            let ww = word.width();
            let gap = match (row.is_empty(), sep) {
                (true, _) => "",
                (false, true) => " · ",
                (false, false) => " ",
            };
            if used + gap.width() + ww <= width {
                row.push_str(gap);
                row.push_str(word);
                used += gap.width() + ww;
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

/// One row of key hints, `width` columns at most (doc/0008): `hints` in
/// reading order, each with its rank (1 is kept longest), joined by ` · `.
/// While the row is too wide, the hint with the lowest rank (the last of
/// equal ranks) is dropped whole; the row never wraps and never starts or
/// ends with `·`. Only a single hint wider than the row is cut, with `…`.
pub(super) fn hint_row(hints: &[(u8, &str)], width: usize) -> String {
    let mut kept: Vec<(u8, &str)> = hints.to_vec();
    loop {
        let line = kept.iter().map(|(_, h)| *h).collect::<Vec<_>>().join(" · ");
        if line.width() <= width {
            return line;
        }
        // The lowest rank, the last of equal ranks.
        let Some((drop, _)) = kept
            .iter()
            .enumerate()
            .max_by_key(|(i, (rank, _))| (*rank, *i))
            .filter(|_| kept.len() > 1)
        else {
            return super::fit(&line, width).trim_end().to_owned();
        };
        kept.remove(drop);
    }
}

/// The longest of `variants` (longest first) that fits `width` columns;
/// the last one cut with `…` when none fits.
fn longest_fit(variants: &[&str], width: usize) -> String {
    match variants.iter().find(|v| v.width() <= width) {
        Some(v) => (*v).to_owned(),
        None => super::fit(variants.last().copied().unwrap_or_default(), width)
            .trim_end()
            .to_owned(),
    }
}

/// The new-session dialog's key hints with their ranks (doc/0008): `Ctrl+S`
/// and `Ctrl+P` (doc/0036) stay longest, then the attach key, `Tab`,
/// `Ctrl+G` and, first to go, `Enter` (the task placeholder names it too).
fn dialog_hints(app: &App, focused: Field) -> Vec<(u8, String)> {
    let enter = if focused == Field::Task {
        "Enter new line"
    } else {
        "Enter next field"
    };
    vec![
        (4, "Tab/Shift+Tab field".to_owned()),
        (6, enter.to_owned()),
        (1, "Ctrl+S start".to_owned()),
        (3, format!("{} start & attach", attach_key(app))),
        (5, "Ctrl+G editor".to_owned()),
        (2, "Ctrl+P template".to_owned()),
    ]
}

/// The resume window's key hints with their ranks (doc/0019): `Enter` and
/// `Esc` stay longest; `type to filter` goes first (the project field's
/// placeholder says it too).
const RESUME_HINTS: &[(u8, &str)] = &[
    (3, "Tab/Shift+Tab field"),
    (4, "↑↓ choose"),
    (5, "type to filter"),
    (1, "Enter open the picker"),
    (2, "Esc cancel"),
];

/// The task field's placeholder, longest first (doc/0008).
const TASK_PLACEHOLDERS: &[&str] = &[
    "describe the task · Enter new line · Ctrl+G opens $VISUAL / $EDITOR · Ctrl+P inserts a \
     template",
    "describe the task · Enter new line · Ctrl+G editor · Ctrl+P template",
    "describe the task · Ctrl+G editor · Ctrl+P template",
    "describe the task · Ctrl+P template",
    "describe the task",
];

/// The model field's hint, longest first; it shows only whole (doc/0008).
fn model_hints() -> [String; 2] {
    [
        format!("  ↑↓ {} · or type a model name", MODEL_CHOICES.join(" · ")),
        "  ↑↓ presets · or type a model name".to_owned(),
    ]
}

/// The model input's width and the hint after it, in `value_w` columns:
/// the input keeps at least `min(max(text + 1, 8), value_w / 2)` columns
/// (and at most 40 while a hint follows), and the longest hint that fits
/// beside it shows whole; with none, the input takes the whole row.
fn model_layout(text: &str, value_w: usize) -> (usize, Option<String>) {
    let keep = (text.width() + 1).max(8).min(value_w / 2).max(1);
    for hint in model_hints() {
        let room = value_w.saturating_sub(hint.width());
        if room >= keep {
            return (room.min(40).max(keep), Some(hint));
        }
    }
    (value_w.max(1), None)
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

/// Where the visual rows of one text-area line start (char indices), for
/// rows `width` columns wide: the rows break at spaces, and the spaces
/// after a row's last word stay with that row even past the width (drawn
/// as nothing); a word breaks inside only when it is longer than a row.
fn row_starts(chars: &[char], width: usize) -> Vec<usize> {
    let cw = |c: &char| c.width().unwrap_or(0);
    let mut starts = vec![0];
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() {
        let mut j = i;
        while j < chars.len() && chars[j] != ' ' {
            j += 1;
        }
        let ww: usize = chars[i..j].iter().map(cw).sum();
        let row_start = starts.last().copied().unwrap_or(0);
        if used + ww > width && i > row_start {
            starts.push(i);
            used = 0;
        }
        if used + ww > width {
            // Longer than a row: break inside the word.
            for (k, c) in chars[i..j].iter().enumerate() {
                let w = cw(c);
                let row_start = starts.last().copied().unwrap_or(0);
                if used + w > width && i + k > row_start {
                    starts.push(i + k);
                    used = 0;
                }
                used += w;
            }
        } else {
            used += ww;
        }
        let mut k = j;
        while k < chars.len() && chars[k] == ' ' {
            k += 1;
        }
        used += k - j;
        i = k;
    }
    starts
}

/// One logical line of a text area wrapped at word boundaries
/// ([`row_starts`]), plus the visual (row, column) of the cursor placed
/// before char `cursor` (or at the end), when given. A cursor on the
/// spaces past a row's end shows at the row's last column plus one; a
/// cursor at the end of a full last row gets an extra empty row.
fn wrap_text_line(
    line: &str,
    width: usize,
    cursor: Option<usize>,
) -> (Vec<String>, Option<(usize, usize)>) {
    let width = width.max(1);
    let chars: Vec<char> = line.chars().collect();
    let starts = row_starts(&chars, width);
    let col_of = |range: &[char]| range.iter().map(|c| c.width().unwrap_or(0)).sum::<usize>();
    let mut rows: Vec<String> = Vec::with_capacity(starts.len());
    for (r, &s) in starts.iter().enumerate() {
        let e = starts.get(r + 1).copied().unwrap_or(chars.len());
        // Only the spaces after the last word can run past the width (and
        // a wide char on a row narrower than it).
        let mut row = String::new();
        let mut used = 0;
        for c in &chars[s..e] {
            let w = c.width().unwrap_or(0);
            if used + w > width && !row.is_empty() {
                break;
            }
            row.push(*c);
            used += w;
        }
        rows.push(row);
    }
    let pos = cursor.map(|c| {
        let c = c.min(chars.len());
        let r = starts.iter().rposition(|&s| s <= c).unwrap_or(0);
        let col = col_of(&chars[starts[r]..c]);
        if c == chars.len() && col >= width {
            rows.push(String::new());
            (rows.len() - 1, 0)
        } else {
            (r, col.min(width))
        }
    });
    (rows, pos)
}

/// Visual rows of the task wrapped to `width`, and the cursor's visual
/// (row, column).
fn task_rows(dialog: &Dialog, width: usize) -> (Vec<String>, (usize, usize)) {
    text_rows(&dialog.task, width)
}

/// Visual rows of a text area wrapped to `width` at word boundaries, and
/// the cursor's visual (row, column).
fn text_rows(text: &TextArea, width: usize) -> (Vec<String>, (usize, usize)) {
    let (crow, ccol) = text.cursor();
    let mut rows = Vec::new();
    let mut cursor = (0, 0);
    for (i, line) in text.lines().iter().enumerate() {
        let (wrapped, pos) = wrap_text_line(line, width, (i == crow).then_some(ccol));
        if let Some((r, c)) = pos {
            cursor = (rows.len() + r, c);
        }
        rows.extend(wrapped);
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

/// The text area of a window: `inner` (inside the border) less one column
/// of padding on each side, so no text touches the border. A cursor after
/// a full row may sit in the right padding column.
fn padded(inner: Rect) -> Rect {
    if inner.width < 2 {
        return Rect { width: 0, ..inner };
    }
    Rect {
        x: inner.x + 1,
        width: inner.width - 2,
        ..inner
    }
}

/// The height a window `w` columns wide needs to hold the template picker
/// open over it inside its border (doc/0036); 0 without a picker.
fn picker_room(picker: Option<&TemplatePicker>, w: u16) -> u16 {
    picker.map_or(0, |p| super::templates::picker_size(p, w).1 + 2)
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
    // Inside the border, one column of padding on each side.
    let width = usize::from(w.saturating_sub(4)).max(1);
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
        DialogStatus::Starting { .. } => {
            for row in wrap(
                "starting… (a supervisor may have to start first; this can take a few seconds)",
                width,
            ) {
                messages.push(Line::styled(row, look.strong()));
            }
        }
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
    let question = match dialog.prompt {
        None => None,
        Some(DialogPrompt::Discard) => Some("Discard this task?  y discard · n keep editing"),
        Some(DialogPrompt::Quit) => {
            Some("Quit tower? The dialog's contents are lost.  y quit · n stay")
        }
    };
    for row in question.map(|q| wrap(q, width)).unwrap_or_default() {
        messages.push(Line::styled(row, look.prompt()));
    }

    let hints = dialog_hints(app, focused);
    let hints: Vec<(u8, &str)> = hints.iter().map(|(r, h)| (*r, h.as_str())).collect();
    let footer = [hint_row(&hints, width)];

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
        .max(picker_room(dialog.templates.as_ref(), w))
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = look
        .overlay_block(" New session ")
        .title_bottom(look.hint_line(" Esc cancel "));
    let inner = padded(block.inner(rect));
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

    // Task: label row (the placeholder, or an accepted slash command's
    // argument hint, doc/0042), then the wrapped text.
    let mut spans = vec![label(Field::Task, focused, look)];
    if dialog.task.is_blank() {
        spans.extend(look.hint_spans(longest_fit(TASK_PLACEHOLDERS, value_w)));
    } else if let Some((name, hint)) = dialog.slash.argument_hint(&dialog.task) {
        spans.push(Span::styled(
            super::shorten(&format!("{name} {hint}"), value_w),
            look.hint(),
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
    let (field_w, hint) = model_layout(&dialog.model.text(), value_w);
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
    if let Some(hint) = hint {
        spans.extend(look.hint_spans(hint));
    }
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
        && pos.x <= inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
        // The slash-command menu under the task's cursor row (doc/0042).
        if focused == Field::Task
            && editing
            && let Some(view) = dialog.slash.view(&dialog.task)
        {
            super::commands::draw_menu(p.frame, &view, inner, pos.y, look);
        }
    }
    if let Some(picker) = &dialog.templates {
        super::templates::draw_picker(p.frame, picker, rect, look);
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
    // Inside the border, one column of padding on each side.
    let width = usize::from(w.saturating_sub(4)).max(1);
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
    let footer = [hint_row(RESUME_HINTS, width)];

    // Fixed rows: project label, blank, account, blank.
    let fixed = 4 + messages.len() + footer.len();
    let wanted = fixed + list_len.clamp(1, RESUME_PROJECT_ROWS);
    let h = (wanted as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = look.overlay_block(" Resume a past conversation ");
    let inner = padded(block.inner(rect));
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
        && pos.x <= inner.x + inner.width
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
    let footer = [hint_row(
        &[(1, enter), (3, "Ctrl+U clear"), (2, "Esc cancel")],
        width,
    )];

    // Name, blank, body, blank, footer.
    let wanted = 2 + body.len() + 1 + footer.len();
    let h = (wanted as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = look.overlay_block(" Rename session ");
    let inner = padded(block.inner(rect));
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
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
        && pos.x <= inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
    }
}

/// The reply window's line saying what `Ctrl+S` does (doc/0040): a live
/// session is stopped first, a stopped one only woken.
pub fn reply_plan(live: bool, account: &str) -> String {
    if live {
        format!(
            "Stops the idle session and wakes it under {account} with your text as the next \
             prompt."
        )
    } else {
        format!("Wakes the session under {account} with your text as the next prompt.")
    }
}

/// The reply window's keys with their ranks (doc/0040): one row, whole
/// hints dropped from the lowest rank when it is too wide ([`hint_row`]).
pub const REPLY_KEYS: &[(u8, &str)] = &[
    (4, "Enter new line"),
    (1, "Ctrl+S send"),
    (3, "Ctrl+P template"),
    (2, "Esc cancel"),
];

/// Draws the quick reply window of `t` centred in `area` (doc/0040): the
/// text, what `Ctrl+S` does, an error or question, the keys, and the
/// template picker over it while it is open.
pub fn draw_reply(frame: &mut Frame, _app: &App, window: &ReplyWindow, area: Rect, look: Look) {
    let w = if area.width > 104 {
        100
    } else {
        area.width.saturating_sub(2).max(1)
    };
    let width = usize::from(w.saturating_sub(4)).max(1);
    let editing = window.prompt.is_none() && window.templates.is_none();
    let mut below: Vec<Line> = Vec::new();
    for row in wrap(&reply_plan(window.live, &window.account), width) {
        below.push(Line::styled(row, look.text()));
    }
    if let Some(err) = &window.error {
        for row in wrap(err, width) {
            below.push(Line::styled(row, look.error()));
        }
    }
    match window.prompt {
        None => {}
        Some(DialogPrompt::Discard) => below.push(Line::styled(
            "Discard this text?  y discard · n keep editing",
            look.prompt(),
        )),
        Some(DialogPrompt::Quit) => below.push(Line::styled(
            "Quit tower? The text is lost.  y quit · n stay",
            look.prompt(),
        )),
    }
    let footer = [hint_row(REPLY_KEYS, width)];
    let text_w = width.saturating_sub(2).max(1);
    let (rows, (crow, ccol)) = text_rows(&window.text, text_w);
    // Text, blank (or an accepted slash command's argument hint, doc/0042),
    // plan and messages, blank, footer.
    let fixed = 1 + below.len() + 1 + footer.len();
    let mut wanted = fixed + rows.len().clamp(3, TASK_ROWS);
    // The window grows so the slash-command menu fits below the cursor
    // over the text rows and the blank row after them; the plan and the
    // keys stay visible.
    let menu = if editing {
        window.slash.view(&window.text)
    } else {
        None
    };
    if let Some(view) = &menu {
        let menu_h = usize::from(super::commands::menu_height(view));
        wanted = wanted.max(fixed + (crow.min(TASK_ROWS - 1) + menu_h).min(TASK_ROWS));
    }
    let h = (wanted as u16 + 2)
        .max(picker_room(window.templates.as_ref(), w))
        .min(area.height.saturating_sub(2))
        .max(1);
    let rect = super::centered(area, w, h);
    frame.render_widget(Clear, rect);
    let title = format!(" Reply to {} ", window.name);
    let block = look.overlay_block(title);
    let inner = padded(block.inner(rect));
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let text_h = usize::from(inner.height)
        .saturating_sub(fixed)
        .clamp(1, TASK_ROWS);
    let mut p = Painter {
        frame,
        area: inner,
        y: inner.y,
    };
    let mut cursor = None;
    if window.text.is_blank() && crow == 0 && ccol == 0 {
        p.line(Line::from(vec![
            Span::styled("┊ ", look.hint()),
            Span::styled(
                "your next prompt for the session · Ctrl+P inserts a template",
                look.hint(),
            ),
        ]));
        if editing {
            cursor = Some(Position::new(inner.x + 2, inner.y));
        }
        for _ in 1..text_h {
            p.line(Line::styled("┊ ", look.hint()));
        }
    } else {
        let start = if crow < text_h { 0 } else { crow + 1 - text_h };
        for i in start..start + text_h {
            let text = rows.get(i).cloned().unwrap_or_default();
            if i == crow && editing && p.has_room() {
                cursor = Some(Position::new(inner.x + 2 + ccol as u16, p.y));
            }
            p.line(Line::from(vec![
                Span::styled("┊ ", look.hint()),
                Span::styled(text, look.text()),
            ]));
        }
    }
    match window.slash.argument_hint(&window.text) {
        Some((name, hint)) => p.line(Line::styled(
            super::shorten(&format!("{name} {hint}"), width),
            look.hint(),
        )),
        None => p.line(Line::raw("")),
    }
    for line in below {
        p.line(line);
    }
    p.line(Line::raw(""));
    for row in footer {
        p.line(look.hint_line(row));
    }
    if let Some(pos) = cursor
        && pos.x <= inner.x + inner.width
        && pos.y < inner.y + inner.height
    {
        p.frame.set_cursor_position(pos);
        if let Some(view) = &menu {
            super::commands::draw_menu(p.frame, view, inner, pos.y, look);
        }
    }
    if let Some(picker) = &window.templates {
        super::templates::draw_picker(p.frame, picker, rect, look);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HINTS: &[(u8, &str)] = &[(3, "Tab field"), (1, "Ctrl+S start"), (2, "Esc cancel")];

    #[test]
    fn a_hint_row_drops_the_lowest_rank_first_and_keeps_its_order() {
        let full = "Tab field · Ctrl+S start · Esc cancel";
        assert_eq!(hint_row(HINTS, 80), full);
        assert_eq!(hint_row(HINTS, full.width()), full);
        assert_eq!(
            hint_row(HINTS, full.width() - 1),
            "Ctrl+S start · Esc cancel"
        );
        assert_eq!(hint_row(HINTS, 24), "Ctrl+S start");
        // Only a single hint is cut, as a last resort.
        assert_eq!(hint_row(HINTS, 8), "Ctrl+S …");
        // Equal ranks: the last goes first.
        assert_eq!(hint_row(&[(1, "a b"), (1, "c d")], 4), "a b");
    }

    #[test]
    fn the_longest_variant_that_fits_wins() {
        assert_eq!(longest_fit(TASK_PLACEHOLDERS, 200), TASK_PLACEHOLDERS[0]);
        assert_eq!(
            longest_fit(TASK_PLACEHOLDERS, 66),
            "describe the task · Ctrl+G editor · Ctrl+P template"
        );
        assert_eq!(
            longest_fit(TASK_PLACEHOLDERS, 40),
            "describe the task · Ctrl+P template"
        );
        assert_eq!(longest_fit(TASK_PLACEHOLDERS, 20), "describe the task");
        assert_eq!(longest_fit(TASK_PLACEHOLDERS, 10), "describe …");
    }

    #[test]
    fn a_wrapped_list_never_ends_or_starts_a_row_with_its_separator() {
        let list = "default · fable · opus · sonnet · haiku · a full model name";
        // 33 columns once ended the first row with `sonnet ·`.
        assert_eq!(
            wrap(list, 33),
            [
                "default · fable · opus · sonnet",
                "haiku · a full model name"
            ]
        );
        let words: Vec<&str> = list.split(' ').filter(|w| *w != "·").collect();
        for width in 1..70 {
            let rows = wrap(list, width);
            for row in &rows {
                assert!(!row.trim_end().ends_with('·'), "{width}: {rows:?}");
                assert!(!row.trim_start().starts_with('·'), "{width}: {rows:?}");
                assert!(row.width() <= width, "{width}: {rows:?}");
            }
            // Only separators at a break are left out; every item stays.
            if width >= "default".len() {
                let kept: Vec<String> = rows
                    .iter()
                    .flat_map(|r| r.split(' ').map(str::to_owned).collect::<Vec<_>>())
                    .filter(|w| w != "·")
                    .collect();
                assert_eq!(kept, words, "{width}: {rows:?}");
            }
        }
        // A `·` inside a word or a lone one at the start is text.
        assert_eq!(wrap("a·b c", 3), ["a·b", "c"]);
        assert_eq!(wrap("· x", 10), ["· x"]);
    }

    /// The rows of `line` and the cursor before each char index.
    fn cursors(line: &str, width: usize) -> (Vec<String>, Vec<(usize, usize)>) {
        let n = line.chars().count();
        let rows = wrap_text_line(line, width, None).0;
        let pos = (0..=n)
            .map(|c| wrap_text_line(line, width, Some(c)).1.unwrap())
            .collect();
        (rows, pos)
    }

    #[test]
    fn text_areas_wrap_at_words_and_keep_the_cursor_in_place() {
        // ASCII: the space after a full row stays on it, drawn as nothing.
        let (rows, pos) = cursors("hello world foo", 11);
        assert_eq!(rows, ["hello world", "foo"]);
        assert_eq!(pos[5], (0, 5));
        assert_eq!(pos[11], (0, 11));
        assert_eq!(pos[12], (1, 0));
        assert_eq!(pos[15], (1, 3));
        let (rows, pos) = cursors("hello world foo", 8);
        assert_eq!(rows, ["hello ", "world ", "foo"]);
        assert_eq!(pos[6], (1, 0));
        assert_eq!(pos[12], (2, 0));

        // Cyrillic: whole words move to the next row.
        let (rows, pos) = cursors("после обновления библиотеки", 18);
        assert_eq!(rows, ["после обновления ", "библиотеки"]);
        assert_eq!(pos[6], (0, 6));
        assert_eq!(pos[17], (1, 0));
        assert_eq!(pos[20], (1, 3));
        assert_eq!(pos[27], (1, 10));

        // A word longer than the row breaks inside, on a row of its own.
        let (rows, pos) = cursors("a abcdefghijkl", 5);
        assert_eq!(rows, ["a ", "abcde", "fghij", "kl"]);
        assert_eq!(pos[2], (1, 0));
        assert_eq!(pos[7], (2, 0));
        assert_eq!(pos[14], (3, 2));
        // At the end of a full last row the cursor gets a row of its own.
        let (rows, pos) = cursors("abcdefghij", 5);
        assert_eq!(rows, ["abcde", "fghij"]);
        assert_eq!(pos[10], (2, 0));

        // Wide characters count two columns and never split.
        let (rows, pos) = cursors("日本語 テキスト", 6);
        assert_eq!(rows, ["日本語", "テキス", "ト"]);
        assert_eq!(pos[2], (0, 4));
        assert_eq!(pos[3], (0, 6));
        assert_eq!(pos[4], (1, 0));
        assert_eq!(pos[7], (2, 0));
        assert_eq!(pos[8], (2, 2));
        assert_eq!(cursors("ab日本", 3).0, ["ab", "日", "本"]);
    }

    #[test]
    fn the_wrapped_cursor_moves_forward_one_cell_at_a_time() {
        let texts = [
            "Найди, почему падают интеграционные тесты платежей после обновления",
            "then open a pull request with a short summary",
            "  indented   with   runs of spaces  ",
            "日本語のテキスト and words",
            "averyveryverylongwordwithoutanyspaces then more",
        ];
        for text in texts {
            let chars: Vec<char> = text.chars().collect();
            // From 2 columns, where a wide char fits a row.
            for width in 2..40 {
                let (rows, pos) = cursors(text, width);
                for row in &rows {
                    assert!(row.width() <= width, "{width} {text}: {rows:?}");
                }
                for c in 0..chars.len() {
                    let (a, b) = (pos[c], pos[c + 1]);
                    assert!(a.1 <= width && b.1 <= width, "{width} {text}: {c}");
                    // The next position is further on: the same row further
                    // right (by the char's width, or not at all past the
                    // row's end), or the start of a later row.
                    let cw = chars[c].width().unwrap_or(0);
                    let same_row = b.0 == a.0 && (b.1 == a.1 + cw || b.1 == width);
                    let next_row = b.0 == a.0 + 1 && b.1 == 0;
                    assert!(same_row || next_row, "{width} {text}: {c} {a:?} {b:?}");
                    // A visible char is drawn where the cursor before it is.
                    if chars[c] != ' ' {
                        let row: Vec<char> = rows[a.0].chars().collect();
                        let col: usize = row
                            .iter()
                            .scan(0, |w, ch| {
                                let at = *w;
                                *w += ch.width().unwrap_or(0);
                                Some((at, *ch))
                            })
                            .find(|(at, _)| *at == a.1)
                            .map(|(_, ch)| ch.width().unwrap_or(0))
                            .unwrap_or(usize::MAX);
                        assert_eq!(col, cw, "{width} {text}: {c} {a:?} {rows:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn text_rows_count_every_line_and_find_the_cursor() {
        let text = TextArea::new("first line here\nвторая строка текста");
        let (rows, cursor) = text_rows(&text, 12);
        assert_eq!(
            rows,
            ["first line ", "here", "вторая ", "строка ", "текста"]
        );
        assert_eq!(cursor, (4, 6));
    }

    #[test]
    fn the_model_hint_shows_whole_beside_room_for_the_text() {
        let [full, short] = model_hints();
        // 100 columns: the input keeps 8 columns or more for `default`.
        assert_eq!(model_layout("default", 86), (19, Some(full.clone())));
        // 80 columns: only the short hint fits.
        assert_eq!(model_layout("default", 66), (31, Some(short.clone())));
        // A long name keeps half the row, or the whole row without a hint.
        let long = "claude-opus-4-1-20250805-experimental-long-context";
        assert_eq!(model_layout(long, 66), (66, None));
        assert_eq!(model_layout(long, 86), (43, Some(short)));
        for w in 1..120 {
            let (field, hint) = model_layout(long, w);
            let hint_w = hint.as_deref().map_or(0, UnicodeWidthStr::width);
            assert!(field + hint_w <= w.max(1), "{w}");
            assert!(field >= (long.width() + 1).min(w / 2), "{w}");
        }
    }
}
