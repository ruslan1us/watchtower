//! Rendering of the template picker (doc/0036), drawn over the window that
//! hosts it (the new-session dialog, doc/0036, or the quick reply window,
//! doc/0040). Pure drawing from a
//! [`TemplatePicker`]; its look follows the style and theme like every
//! window ([`Look`], doc/0020, doc/0021).

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::dialog::{hint_row, input_window, wrap};
use super::theme::Look;
use crate::templates::TemplatePicker;

/// The picker is at most this wide.
const MAX_W: u16 = 80;
/// At most this many templates are listed at once.
const ROWS: usize = 10;
/// The name column is at most this wide.
const NAME_W: usize = 28;
/// The preview is left out when fewer columns than this remain for it.
const MIN_PREVIEW_W: usize = 8;

/// The picker's key hints with their ranks: one row, whole hints dropped
/// from the lowest rank when it is too wide (`type to filter` first: the
/// filter line says it too).
pub const PICKER_KEYS: &[(u8, &str)] = &[
    (3, "↑↓ choose"),
    (4, "type to filter"),
    (1, "Enter insert"),
    (2, "Esc close"),
];

/// The picker's width over a host window `host_w` columns wide, inside
/// the host's border and padding column on each side (so the two borders
/// never touch), and the rows it wants, its border included. A host makes
/// itself at least two rows taller than that while the picker is open.
pub(crate) fn picker_size(picker: &TemplatePicker, host_w: u16) -> (u16, u16) {
    let w = host_w.saturating_sub(4).clamp(1, MAX_W);
    let (_, _, footer, list_len) = layout(picker, w);
    // Filter, blank, list, blank, footer.
    let wanted = 2 + list_len + 1 + footer.len();
    (w, wanted as u16 + 2)
}

/// The text width inside a picker `w` columns wide (one column of padding
/// inside the border on each side), its note, its keys and how many list
/// rows it wants.
fn layout(picker: &TemplatePicker, w: u16) -> (usize, Vec<String>, Vec<String>, usize) {
    let width = usize::from(w.saturating_sub(4)).max(1);
    let note: Vec<String> = picker
        .empty_text()
        .map(|t| wrap(&t, width))
        .unwrap_or_default();
    let footer = vec![hint_row(PICKER_KEYS, width)];
    let list_len = picker.filtered().len().max(note.len()).clamp(1, ROWS);
    (width, note, footer, list_len)
}

/// Draws `picker` inside its host window at `host`: the filter line, the
/// templates (name and a dimmed first line) or a note, and the keys
/// ([`picker_size`]).
pub(crate) fn draw_picker(frame: &mut Frame, picker: &TemplatePicker, host: Rect, look: Look) {
    let (w, wanted) = picker_size(picker, host.width);
    let (width, note, footer, list_len) = layout(picker, w);
    let filtered = picker.filtered();
    // Inside the host, below its top border and above its bottom one.
    let h = wanted.min(host.height.saturating_sub(2)).max(1);
    let rect = super::centered(host, w, h);
    frame.render_widget(Clear, rect);
    // `Esc close` is on the keys row, as in every window.
    let block = look.overlay_block(" Templates ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width < 3 || inner.height == 0 {
        return;
    }
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width - 2,
        ..inner
    };
    let fixed = 3 + footer.len();
    let rows = usize::from(inner.height)
        .saturating_sub(fixed)
        .clamp(1, list_len);

    let mut lines: Vec<Line> = Vec::new();
    // The filter line: `› text`, or a placeholder, and the match count.
    let count = format!("   {} of {}", filtered.len(), picker.templates().len());
    let field_w = width.saturating_sub(2 + count.width()).max(1);
    let (shown, cx) = input_window(picker.filter(), field_w);
    let mut spans = vec![Span::styled("› ", look.input_prompt())];
    if picker.filter().is_empty() {
        spans.push(Span::styled(
            super::fit("type to filter", field_w),
            look.hint(),
        ));
    } else {
        spans.push(Span::styled(super::fit(&shown, field_w), look.text()));
    }
    if !picker.is_loading() {
        spans.push(Span::styled(count, look.hint()));
    }
    lines.push(Line::from(spans));
    lines.push(Line::raw(""));

    if filtered.is_empty() {
        let style = match picker.list() {
            Some(list) if list.error.is_some() => look.error(),
            _ => look.hint(),
        };
        for row in note.iter().take(rows) {
            lines.push(Line::styled(row.clone(), style));
        }
    } else {
        let sel = picker.selected_index().min(filtered.len() - 1);
        let start = if sel < rows { 0 } else { sel + 1 - rows };
        let longest = filtered.iter().map(|t| t.name.width()).max().unwrap_or(0);
        // `▶ ` or two spaces, then the name column.
        let name_w = longest.min(NAME_W).min(width.saturating_sub(2)).max(1);
        let preview_w = width.saturating_sub(2 + name_w + 2);
        for (i, t) in filtered.iter().enumerate().skip(start).take(rows) {
            let selected = i == sel;
            let (item, hint) = if selected {
                let bar = look.selected_item();
                (bar, bar.patch(look.hint()))
            } else {
                (look.item(), look.hint())
            };
            let mut spans = Vec::new();
            if selected {
                spans.push(Span::styled("▶", item.patch(look.selected_marker())));
                spans.push(Span::styled(" ", item));
            } else {
                spans.push(Span::styled("  ", item));
            }
            if preview_w >= MIN_PREVIEW_W {
                spans.push(Span::styled(super::fit(&t.name, name_w), item));
                spans.push(Span::styled("  ", item));
                spans.push(Span::styled(super::fit(t.preview(), preview_w), hint));
            } else {
                let rest = width.saturating_sub(2);
                spans.push(Span::styled(super::fit(&t.name, rest), item));
            }
            lines.push(Line::from(spans));
        }
    }
    lines.push(Line::raw(""));
    for row in footer {
        lines.push(look.hint_line(row));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let pos = Position::new(inner.x + 2 + cx as u16, inner.y);
    if pos.x < inner.x + inner.width && pos.y < inner.y + inner.height {
        frame.set_cursor_position(pos);
    }
}
