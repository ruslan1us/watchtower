//! Rendering of the slash-command menu (doc/0042), drawn over the window
//! that hosts it (the new-session dialog's task field, the quick reply
//! window) right below the cursor's row, or above it when there is no room
//! below. Pure drawing from a [`MenuView`]; its look follows the style and
//! theme like every window ([`Look`], doc/0020, doc/0021).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::dialog::hint_row;
use super::theme::Look;
use crate::commands::{LOADING_TEXT, MenuView, SlashCommand};

/// At most this many entries are listed at once.
pub const MENU_ROWS: usize = 8;
/// The name column (with the argument hint) is at most this wide.
const NAME_W: usize = 40;
/// The source tag column is at most this wide.
const TAG_W: usize = 24;
/// The description is left out when fewer columns than this remain for it.
const MIN_DESCRIPTION_W: usize = 8;

/// The menu's keys with their ranks, on its bottom border: whole hints
/// dropped from the lowest rank when it is too wide.
pub const MENU_KEYS: &[(u8, &str)] = &[(3, "↑↓ choose"), (1, "Tab insert"), (2, "Esc close")];

/// The rows the menu wants, its border included.
pub(crate) fn menu_height(view: &MenuView<'_>) -> u16 {
    let rows = match view {
        MenuView::Loading => 1,
        MenuView::List { items, .. } => items.len().clamp(1, MENU_ROWS),
    };
    rows as u16 + 2
}

/// Where the menu goes inside `area` (the host's text area, inside its
/// border and padding) for a cursor on row `cursor_y`: below that row when
/// it fits, else above it, else on the larger side with fewer rows. `None`
/// when not even one entry fits.
pub(crate) fn menu_rect(view: &MenuView<'_>, area: Rect, cursor_y: u16) -> Option<Rect> {
    let want = menu_height(view);
    let bottom = area.y + area.height;
    let below = bottom.saturating_sub(cursor_y + 1);
    let above = cursor_y.saturating_sub(area.y);
    let (y, h) = if below >= want {
        (cursor_y + 1, want)
    } else if above >= want {
        (cursor_y - want, want)
    } else if below >= above {
        (cursor_y + 1, below)
    } else {
        (area.y, above)
    };
    (h >= 3 && area.width >= 8).then_some(Rect {
        x: area.x,
        y,
        width: area.width,
        height: h,
    })
}

/// Draws the menu for a cursor on row `cursor_y` of `area` (the host's
/// text area): `loading commands…`, or the entries, each `/name [hint]`,
/// its description dimmed and its source tag, the chosen one on the
/// selection bar with `▶`.
pub(crate) fn draw_menu(
    frame: &mut Frame,
    view: &MenuView<'_>,
    area: Rect,
    cursor_y: u16,
    look: Look,
) {
    let Some(rect) = menu_rect(view, area, cursor_y) else {
        return;
    };
    frame.render_widget(Clear, rect);
    let keys_w = usize::from(rect.width.saturating_sub(4));
    let keys = hint_row(MENU_KEYS, keys_w.saturating_sub(2));
    let block = look
        .overlay_block(" Commands ")
        .title_bottom(look.hint_line(format!(" {keys} ")));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width < 3 || inner.height == 0 {
        return;
    }
    // One column of padding inside the border on each side.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width - 2,
        ..inner
    };
    let width = usize::from(inner.width);
    let rows = usize::from(inner.height);
    let lines: Vec<Line> = match view {
        MenuView::Loading => vec![Line::styled(super::fit(LOADING_TEXT, width), look.hint())],
        MenuView::List { items, selected } => {
            let sel = (*selected).min(items.len().saturating_sub(1));
            let start = if sel < rows { 0 } else { sel + 1 - rows };
            let shown: Vec<&SlashCommand> = items.iter().skip(start).take(rows).copied().collect();
            let cols = columns(&shown, width);
            shown
                .iter()
                .enumerate()
                .map(|(i, c)| entry_line(c, start + i == sel, &cols, look))
                .collect()
        }
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The column widths of the entries shown in `width` columns: the name
/// (with its hint), the description (0: left out) and the tag (0: left
/// out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    name: usize,
    description: usize,
    tag: usize,
}

fn name_width(c: &SlashCommand) -> usize {
    1 + c.name.width() + c.argument_hint.as_deref().map_or(0, |h| 1 + h.width())
}

fn columns(shown: &[&SlashCommand], width: usize) -> Columns {
    // `▶ ` or two spaces, then the columns with two spaces between them.
    let avail = width.saturating_sub(2);
    let longest = shown.iter().map(|c| name_width(c)).max().unwrap_or(1);
    let tag = shown
        .iter()
        .map(|c| c.origin.tag().width())
        .max()
        .unwrap_or(0)
        .min(TAG_W);
    let name = longest.min(NAME_W).min(avail).max(1);
    let rest = avail.saturating_sub(name + 2 + tag + 2);
    if rest >= MIN_DESCRIPTION_W {
        return Columns {
            name,
            description: rest,
            tag,
        };
    }
    if avail >= name + 2 + tag && tag > 0 {
        return Columns {
            name: avail - 2 - tag,
            description: 0,
            tag,
        };
    }
    Columns {
        name: avail.max(1),
        description: 0,
        tag: 0,
    }
}

/// One entry: marker, `/name` and its dimmed argument hint, the dimmed
/// description and the dimmed source tag, right-aligned.
fn entry_line(c: &SlashCommand, selected: bool, cols: &Columns, look: Look) -> Line<'static> {
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
    let name = format!("/{}", c.name);
    let nw = name.width();
    match c.argument_hint.as_deref() {
        Some(h) if nw + 1 < cols.name => {
            spans.push(Span::styled(name, item));
            spans.push(Span::styled(
                super::fit(&format!(" {h}"), cols.name - nw),
                hint,
            ));
        }
        _ => spans.push(Span::styled(super::fit(&name, cols.name), item)),
    }
    if cols.description > 0 {
        spans.push(Span::styled("  ", item));
        spans.push(Span::styled(
            super::fit(&c.description, cols.description),
            hint,
        ));
    }
    if cols.tag > 0 {
        let tag = super::shorten(&c.origin.tag(), cols.tag);
        spans.push(Span::styled("  ", item));
        let pad = " ".repeat(cols.tag.saturating_sub(tag.width()));
        spans.push(Span::styled(format!("{pad}{tag}"), hint));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Kind, Origin};

    fn cmd(name: &str, hint: Option<&str>, origin: Origin) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: "Fix a bug from a ticket".into(),
            argument_hint: hint.map(str::to_owned),
            origin,
            kind: Kind::Skill,
        }
    }

    #[test]
    fn the_columns_drop_the_description_then_the_tag() {
        let a = cmd("fix-bug", Some("[ticket]"), Origin::Project);
        let b = cmd(
            "review-kit:code-review",
            None,
            Origin::Plugin("review-kit".into()),
        );
        let shown = [&a, &b];
        let wide = columns(&shown, 90);
        assert_eq!(wide.name, "/review-kit:code-review".width());
        assert_eq!(wide.tag, "plugin review-kit".width());
        assert!(wide.description >= MIN_DESCRIPTION_W);
        let narrow = columns(&shown, 46);
        assert_eq!(narrow.description, 0);
        assert!(narrow.tag > 0);
        let tiny = columns(&shown, 20);
        assert_eq!((tiny.description, tiny.tag), (0, 0));
        assert_eq!(tiny.name, 18);
    }

    #[test]
    fn the_menu_goes_below_the_cursor_else_above() {
        let a = cmd("fix-bug", None, Origin::Personal);
        let items = vec![&a, &a, &a];
        let view = MenuView::List { items, selected: 0 };
        let area = Rect::new(2, 1, 60, 20);
        assert_eq!(menu_rect(&view, area, 3), Some(Rect::new(2, 4, 60, 5)));
        assert_eq!(menu_rect(&view, area, 18), Some(Rect::new(2, 13, 60, 5)));
        // Neither fits whole: the larger side, fewer rows.
        let small = Rect::new(0, 0, 60, 7);
        assert_eq!(menu_rect(&view, small, 2), Some(Rect::new(0, 3, 60, 4)));
        assert_eq!(menu_rect(&view, Rect::new(0, 0, 60, 3), 1), None);
    }
}
