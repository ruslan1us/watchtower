//! The Settings window (doc/0024), drawn over the whole screen in the
//! window [`Look`]: the dashboard theme's look while the dashboard style is
//! drawn, else the minimal one (doc/0020, doc/0021). A header panel, the
//! list of settings with section separators joined to its border, a detail
//! panel on the right from [`SIDE_MIN`] columns (a one-line help strip under
//! the list below that), the status line and the footer; the pick list,
//! list editor, questions and help on top.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::dialog::input_window;
use super::theme::{Lb, Look, palette};
use super::{centered, fit, wrap};
use crate::app::App;
use crate::config::{ColorDepth, UiTheme};
use crate::config_edit::{Section, SettingKey, SettingKind, SettingValue};
use crate::settings::{
    ListEditor, Mode, NoticeKind, OTHER, Pick, Row, SettingsWindow, change_text, shown_value,
    toml_text,
};

/// From this width the detail panel sits right of the list.
pub const SIDE_MIN: u16 = 130;

/// Width of the detail panel.
const DETAIL_W: u16 = 50;

/// Gap between footer hints.
const GAP: &str = "   ";

/// What a line of the settings list is.
#[derive(Debug, Clone)]
enum Item {
    Section(Section),
    /// A row the selection can be on, by index into the window's rows.
    Row(usize),
    /// A line of the accounts note.
    Note(String),
    Blank,
}

/// The style of a span on a row that may be selected: the selection bar
/// with the span's colour on it when the bar is painted; on a reversed bar
/// (the minimal look, `terminal`, `NO_COLOR`) only its modifiers.
fn on_bar(look: Look, selected: bool, st: Style) -> Style {
    if !selected {
        return st;
    }
    let bar = look.selected_item();
    match look {
        Look::Themed(th) if th.color && !th.p.reverse => bar.patch(st),
        _ => bar.add_modifier(st.add_modifier),
    }
}

/// Draws the window when it is open.
pub fn draw(frame: &mut Frame, app: &App, look: Look) {
    let Some(win) = app.settings() else {
        return;
    };
    let area = frame.area();
    frame.render_widget(Clear, area);
    if area.width < 30 || area.height < 8 {
        frame.render_widget(
            Paragraph::new(fit(
                "Settings: the terminal is too small",
                area.width.into(),
            )),
            area,
        );
        return;
    }
    let [header, body, status, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, win, header, look);
    let narrow = area.width < SIDE_MIN;
    let list_area = if narrow {
        body
    } else {
        let [list, detail] =
            Layout::horizontal([Constraint::Min(60), Constraint::Length(DETAIL_W)]).areas(body);
        draw_detail(frame, win, detail, look);
        list
    };
    let anchor = draw_list(frame, win, list_area, narrow, look);
    draw_status(frame, win, status, look);
    draw_footer(frame, win, footer, look);
    match &win.mode {
        Mode::Pick(pick) => draw_pick(frame, win, pick, anchor, area, look, app.detected_depth),
        Mode::List(list) => draw_list_editor(frame, win, list, list_area, look),
        Mode::Unsaved
        | Mode::EditPrompt
        | Mode::Conflict(_)
        | Mode::CrossGroup
        | Mode::QuitUnsaved => {
            draw_question(frame, win, list_area, look);
        }
        Mode::Help => draw_help(frame, area, look),
        Mode::Browse | Mode::Input(_) => {}
    }
}

/// One row of `area` at offset `y`.
fn row(area: Rect, y: u16) -> Rect {
    Rect {
        x: area.x,
        y: area.y + y,
        width: area.width,
        height: 1,
    }
}

fn put(frame: &mut Frame, area: Rect, line: Line<'static>) {
    if area.width > 0 && area.height > 0 {
        frame.render_widget(Paragraph::new(line), area);
    }
}

/// `left` and a right-aligned `right` on one row; `right` wins.
fn put_lr(frame: &mut Frame, area: Rect, left: Lb, right: Lb) {
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

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The header panel: the file, whether it exists, and the unsaved count.
fn draw_header(frame: &mut Frame, win: &SettingsWindow, area: Rect, look: Look) {
    let block = look
        .overlay_block(" ◆ tower ")
        .title_top(Line::styled(" settings ", look.secondary()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut left = Lb::new();
    left.push(" Settings", look.strong())
        .push("   ", look.text())
        .push(win.shown_path.clone(), look.secondary());
    if win.session.exists() {
        left.push("  (found)", look.hint());
    } else {
        left.push("  (not found — using defaults)", look.warning());
    }
    let mut right = Lb::new();
    if let Some(preview) = win.previewing() {
        right.push(format!("previewing {preview}   "), look.warning());
    }
    let n = win.change_count();
    if win.saving {
        right.push("saving… ", look.text());
    } else if n > 0 {
        right.push("* ", look.modified()).push(
            format!("{} ", plural(n, "unsaved change", "unsaved changes")),
            look.warning(),
        );
    } else if win.just_saved {
        right.push("✓ saved ", look.success());
    } else {
        right.push("no unsaved changes ", look.hint());
    }
    if left.w + right.w + 2 > usize::from(inner.width) {
        let mut short = Lb::new();
        short.push(" Settings", look.strong());
        put_lr(frame, row(inner, 0), short, right);
    } else {
        put_lr(frame, row(inner, 0), left, right);
    }
}

/// The lines of the list, `w` columns wide, with blank lines between the
/// sections when everything fits in `h` rows.
fn items(win: &SettingsWindow, w: usize, h: usize) -> Vec<Item> {
    let note = if win.session.exists() {
        "Edit accounts in config.toml (e) and restart tower: a wrong command or config dir can \
         wake sessions under the wrong account."
    } else {
        "No config file: one built-in account. Add accounts in config.toml (e) and restart tower."
    };
    let mut out = Vec::new();
    for section in Section::ALL {
        let rows: Vec<usize> = (0..win.rows.len())
            .filter(|i| win.rows[*i].section() == section)
            .collect();
        if rows.is_empty() && section != Section::Accounts {
            continue;
        }
        if !out.is_empty() {
            out.push(Item::Blank);
        }
        out.push(Item::Section(section));
        out.extend(rows.into_iter().map(Item::Row));
        if section == Section::Accounts {
            out.extend(wrap(note, w.saturating_sub(5)).into_iter().map(Item::Note));
        }
    }
    if out.len() > h {
        out.retain(|i| !matches!(i, Item::Blank));
    }
    out
}

/// The label column's width.
fn label_w(narrow: bool) -> usize {
    if narrow { 28 } else { 29 }
}

/// Draws the list; returns where the selected row's value starts, for the
/// pick list.
fn draw_list(
    frame: &mut Frame,
    win: &SettingsWindow,
    area: Rect,
    narrow: bool,
    look: Look,
) -> Rect {
    let n_set = SettingKey::ALL.len();
    let n_acc = win.session.accounts().len();
    let block = look.overlay_block(" Settings ").title_top(
        Line::styled(
            format!(
                " {n_set} settings · {} ",
                plural(n_acc, "account", "accounts")
            ),
            look.secondary(),
        )
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let strip = narrow && inner.height >= 6;
    let list_h = if strip {
        inner.height - 2
    } else {
        inner.height
    };
    let w = usize::from(inner.width);
    let lines = items(win, w, usize::from(list_h));
    let sel_line = lines
        .iter()
        .position(|i| matches!(i, Item::Row(r) if *r == win.selected))
        .unwrap_or(0);
    let h = usize::from(list_h);
    // Keep the selection in view, with its section title when it fits.
    let mut offset = 0;
    if sel_line >= h {
        offset = sel_line + 1 - h;
    }
    let mut anchor = Rect::default();
    for (y, item) in lines.iter().skip(offset).take(h).enumerate() {
        let ra = row(inner, y as u16);
        match item {
            Item::Section(section) => {
                let full = Rect {
                    x: area.x,
                    width: area.width,
                    ..ra
                };
                put(
                    frame,
                    full,
                    separator(*section, usize::from(area.width), look),
                );
            }
            Item::Row(i) => {
                let selected = *i == win.selected;
                match win.rows[*i] {
                    Row::Setting(key) => {
                        let x = draw_setting(frame, win, key, ra, selected, label_w(narrow), look);
                        if selected {
                            anchor = Rect { x: ra.x + x, ..ra };
                        }
                    }
                    Row::Account(a) => draw_account(frame, win, a, ra, selected, look),
                }
            }
            Item::Note(text) => {
                let mut l = Lb::new();
                l.push("    ", look.hint()).push(text.clone(), look.hint());
                put(frame, ra, l.line());
            }
            Item::Blank => {}
        }
    }
    if strip {
        let rule_y = inner.y + inner.height - 2;
        let mut rule = Lb::new();
        rule.push("├", look.rule())
            .push(
                "─".repeat(usize::from(area.width).saturating_sub(2)),
                look.rule(),
            )
            .push("┤", look.rule());
        put(
            frame,
            Rect {
                x: area.x,
                y: rule_y,
                width: area.width,
                height: 1,
            },
            rule.line(),
        );
        let (label, help) = match win.selected_row() {
            Row::Setting(key) => (key.label(), key.help()),
            Row::Account(_) => (
                "accounts",
                "Read-only here: edit config.toml (e) and restart tower.",
            ),
        };
        let mut l = Lb::new();
        l.push(" ", look.text())
            .push(label, look.strong())
            .push("  ", look.text());
        let room = w.saturating_sub(l.w);
        l.push(fit(help, room), look.secondary());
        put(frame, row(inner, inner.height - 1), l.line());
    }
    anchor
}

/// A section's separator, joined to the panel's border:
/// `├─ Appearance ────┤`.
fn separator(section: Section, width: usize, look: Look) -> Line<'static> {
    let mut l = Lb::new();
    l.push("├─ ", look.rule())
        .push(section.title(), look.strong());
    if section == Section::Accounts {
        l.push("  read-only", look.hint());
    }
    l.push(" ", look.rule());
    let rest = width.saturating_sub(l.w + 1);
    l.push("─".repeat(rest), look.rule()).push("┤", look.rule());
    l.line()
}

/// The style of a [`NoticeKind`].
fn kind_style(kind: NoticeKind, look: Look) -> Style {
    match kind {
        NoticeKind::Info => look.text(),
        NoticeKind::Success => look.success(),
        NoticeKind::Warning => look.warning(),
        NoticeKind::Error => look.error(),
    }
}

/// The key hint at the right of the selected row.
fn row_hint(kind: &SettingKind) -> &'static str {
    match kind {
        SettingKind::Choice { .. } | SettingKind::Steps { .. } => "Enter pick   ←→ change ",
        SettingKind::Bool => "Enter toggle ",
        SettingKind::Int { .. } | SettingKind::Text => "Enter edit ",
        SettingKind::Paths => "Enter open list ",
    }
}

/// A setting's row: the selection mark, the `*` of an unsaved change, the
/// label and the value (or the inline input). Returns the column where the
/// value starts.
fn draw_setting(
    frame: &mut Frame,
    win: &SettingsWindow,
    key: SettingKey,
    ra: Rect,
    selected: bool,
    lw: usize,
    look: Look,
) -> u16 {
    let w = usize::from(ra.width);
    let mut setting = win.session.setting(key);
    // A style or theme under the pick list's cursor is shown, as it is
    // previewed.
    if let Mode::Pick(pick) = &win.mode
        && pick.key == key
        && key.previewed()
        && let Some(v) = pick.value()
        && setting.value.display() != v
    {
        setting.value = SettingValue::Text(v.to_owned());
        setting.modified = setting.saved.as_ref() != Some(&setting.value);
    }
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
    l.push(
        if setting.modified { "*" } else { " " },
        bar(look.modified()),
    );
    l.push("  ", plain);
    let label_st = if selected {
        plain.add_modifier(Modifier::BOLD)
    } else {
        plain
    };
    l.push(fit(setting.label, lw), label_st);
    let value_x = l.w as u16;
    let editing = match &win.mode {
        Mode::Input(input) if input.key == key => Some(input),
        _ => None,
    };
    if let Some(input) = editing {
        let border = if input.error.is_some() {
            look.error()
        } else {
            look.key()
        };
        let err_w = input.error.as_ref().map_or(0, |e| e.width() + 4);
        let room = w.saturating_sub(l.w + 4 + err_w).max(4);
        let field_w = (input.input.text().width() + 1).clamp(12.min(room), room);
        let (shown, col) = input_window(&input.input, field_w);
        l.push("[ ", bar(border));
        let text_x = l.w;
        l.push(shown.clone(), bar(look.strong()));
        l.push(" ".repeat(field_w.saturating_sub(shown.width())), plain);
        l.push(" ]", bar(border));
        if let Some(e) = &input.error {
            l.push("  ✗ ", bar(look.error()))
                .push(e.clone(), bar(look.error()));
        }
        l.pad_to(w, plain);
        put(frame, ra, l.line());
        let x = ra.x + (text_x + col).min(w.saturating_sub(1)) as u16;
        frame.set_cursor_position((x, ra.y));
        return value_x;
    }
    let value = &setting.value;
    let is_default = setting.is_default();
    let value_st = if is_default && !setting.modified {
        if selected { plain } else { look.secondary() }
    } else {
        plain
    };
    let room = w.saturating_sub(l.w + 14);
    match (&setting.kind, value) {
        (SettingKind::Choice { .. } | SettingKind::Steps { .. }, _) if selected => {
            l.push("‹ ", bar(look.hint()))
                .push(shown_value(key, value), plain.add_modifier(Modifier::BOLD))
                .push(" ›", bar(look.hint()));
        }
        (SettingKind::Paths, SettingValue::Paths(paths)) => {
            let shown = fit(&value.display(), room.clamp(8, 40));
            l.push(shown.trim_end().to_owned(), value_st);
            l.push(
                format!("  {} ▸", plural(paths.len(), "folder", "folders")),
                bar(look.hint()),
            );
        }
        (SettingKind::Bool, SettingValue::Bool(on)) => {
            l.push(
                if *on { "[x] " } else { "[ ] " },
                bar(if *on { look.warning() } else { look.hint() }),
            );
            l.push(shown_value(key, value), value_st);
        }
        _ => {
            let shown = fit(&shown_value(key, value), room.max(8));
            l.push(shown.trim_end().to_owned(), value_st);
        }
    }
    if key == SettingKey::DefaultAccount
        && let SettingValue::Text(name) = value
        && let Some(account) = win.session.accounts().iter().find(|a| a.name == *name)
        && !account.group.is_empty()
    {
        l.push(format!("  ({})", account.group), bar(look.hint()));
    }
    let picking = matches!(&win.mode, Mode::Pick(p) if p.key == key);
    if picking && key.previewed() {
        l.push("  preview", bar(look.warning()));
    } else if is_default && value.display() != "default" {
        l.push("  default", bar(look.hint()));
    }
    if selected {
        let hint = row_hint(&setting.kind);
        if l.w + hint.width() + 2 <= w {
            l.pad_to(w - hint.width(), plain);
            l.push(hint, bar(look.secondary()));
        }
        l.pad_to(w, plain);
    }
    put(frame, ra, l.line());
    value_x
}

/// An account's row: name, command, config dir, group; read-only.
fn draw_account(
    frame: &mut Frame,
    win: &SettingsWindow,
    index: usize,
    ra: Rect,
    selected: bool,
    look: Look,
) {
    let Some(account) = win.session.accounts().get(index) else {
        return;
    };
    let w = usize::from(ra.width);
    let bar = |st: Style| on_bar(look, selected, st);
    let plain = if selected {
        look.selected_item()
    } else {
        look.secondary()
    };
    let home = win.home_dir();
    let dir = crate::paths::shorten_home(&account.config_dir.to_string_lossy(), &home);
    let mut l = Lb::new();
    l.push(
        if selected { "▌" } else { " " },
        bar(look.selected_marker()),
    );
    l.push("   ", plain);
    l.push(
        format!("{} ", fit(&account.name, 7)),
        if selected {
            plain.add_modifier(Modifier::BOLD)
        } else {
            look.text()
        },
    );
    l.push(format!("{} ", fit(&account.command, 12)), plain);
    l.push(format!("{} ", fit(&dir, 15)), plain);
    let group = if account.group.is_empty() {
        "(no group)"
    } else {
        account.group.as_str()
    };
    l.push(format!("{} ", fit(group, 11)), bar(look.hint()));
    if win.session.value(SettingKey::DefaultAccount) == SettingValue::Text(account.name.clone()) {
        l.push("default account", bar(look.hint()));
    }
    if selected {
        l.pad_to(w, plain);
    }
    put(frame, ra, l.line());
}

/// The detail panel: the selected setting's help, its key, values,
/// default, file value, unsaved value and when it applies.
fn draw_detail(frame: &mut Frame, win: &SettingsWindow, area: Rect, look: Look) {
    let inner_w = usize::from(area.width.saturating_sub(3));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let text_lines = |lines: &mut Vec<Line<'static>>, text: &str, st: Style| {
        for t in wrap(text, inner_w) {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled(t, st)]));
        }
    };
    let kv = |lines: &mut Vec<Line<'static>>, k: &str, v: &str, st: Style| {
        for (i, part) in wrap(v, inner_w.saturating_sub(10)).into_iter().enumerate() {
            let mut l = Lb::new();
            l.push(" ", look.text())
                .push(fit(if i == 0 { k } else { "" }, 10), look.hint())
                .push(part, st);
            lines.push(l.line());
        }
    };
    let title = match win.selected_row() {
        Row::Setting(key) => {
            let setting = win.session.setting(key);
            text_lines(&mut lines, key.help(), look.text());
            lines.push(Line::raw(""));
            kv(&mut lines, "key", &key.toml_key(), look.text());
            kv(
                &mut lines,
                "values",
                &values_text(&setting.kind),
                look.secondary(),
            );
            kv(
                &mut lines,
                "default",
                &toml_text(&setting.default),
                look.secondary(),
            );
            let in_file = if !win.session.exists() {
                "no file yet".to_owned()
            } else {
                setting
                    .saved
                    .as_ref()
                    .map_or_else(|| "not set (the default applies)".to_owned(), toml_text)
            };
            kv(&mut lines, "in file", &in_file, look.secondary());
            if setting.modified {
                let reset = win
                    .session
                    .changes()
                    .iter()
                    .any(|c| c.key == key && c.reset);
                let new = if reset {
                    "removed (the default applies)".to_owned()
                } else {
                    toml_text(&setting.value)
                };
                kv(&mut lines, "new", &new, look.warning());
            }
            kv(&mut lines, "applies", key.applies(), look.secondary());
            lines.push(Line::raw(""));
            let keys: &[(&str, &str)] = match setting.kind {
                SettingKind::Choice { .. } | SettingKind::Steps { .. } => {
                    &[("Enter", "pick"), ("←→", "change"), ("Del", "default")]
                }
                SettingKind::Bool => &[("Enter", "toggle"), ("Del", "default")],
                SettingKind::Int { .. } | SettingKind::Text => {
                    &[("Enter", "edit"), ("Del", "default")]
                }
                SettingKind::Paths => &[("Enter", "open list"), ("Del", "default")],
            };
            lines.push(key_line(keys, look));
            key.label().to_owned()
        }
        Row::Account(_) => {
            text_lines(
                &mut lines,
                "Accounts are read-only in this window.",
                look.text(),
            );
            lines.push(Line::raw(""));
            text_lines(
                &mut lines,
                "Edit them in config.toml (e) and restart tower. A wrong command or config dir \
                 can wake sessions under the wrong account, and a changed group changes where \
                 hand-offs may go.",
                look.secondary(),
            );
            lines.push(Line::raw(""));
            lines.push(key_line(&[("e", "edit config.toml")], look));
            "accounts".to_owned()
        }
    };
    frame.render_widget(
        Paragraph::new(lines).block(look.overlay_block(format!(" {title} "))),
        area,
    );
}

/// The values a setting takes, for the detail panel.
fn values_text(kind: &SettingKind) -> String {
    match kind {
        SettingKind::Choice { values, free_text } => {
            let mut text = values.join(" · ");
            if *free_text {
                text.push_str(" · a full model name");
            }
            text
        }
        SettingKind::Bool => "on · off".to_owned(),
        SettingKind::Int { min, max } => format!("{min} to {max} seconds"),
        SettingKind::Steps { values, min, max } => {
            let steps: Vec<String> = values
                .iter()
                .map(|v| match v {
                    0 => "off".to_owned(),
                    v => format!("{v} s"),
                })
                .collect();
            format!("{} (file: 0, or {min} to {max})", steps.join(" · "))
        }
        SettingKind::Text => "any text, not empty".to_owned(),
        SettingKind::Paths => "folders; ~ is expanded".to_owned(),
    }
}

/// ` Enter pick   ←→ change   Del default`.
fn key_line(keys: &[(&str, &str)], look: Look) -> Line<'static> {
    let mut l = Lb::new();
    l.push(" ", look.text());
    for (i, (k, v)) in keys.iter().enumerate() {
        if i > 0 {
            l.push(GAP, look.key_label());
        }
        l.push(*k, look.key())
            .push(format!(" {v}"), look.key_label());
    }
    l.line()
}

/// The status line: the window's notice, else what `Ctrl+S` does.
fn draw_status(frame: &mut Frame, win: &SettingsWindow, area: Rect, look: Look) {
    let w = usize::from(area.width);
    let wide = w >= 120;
    let mut l = Lb::new();
    l.push(" ", look.text());
    if let Some(notice) = &win.notice {
        l.push(notice.text.clone(), kind_style(notice.kind, look));
    } else if win.session.exists() {
        l.push("Ctrl+S", look.key())
            .push(" saves to ", look.secondary())
            .push(win.shown_path.clone(), look.text());
        l.push(
            if wide {
                " · only changed keys are rewritten; comments and formatting are kept"
            } else {
                " · comments and formatting are kept"
            },
            look.secondary(),
        );
    } else {
        l.push(win.shown_path.clone(), look.text())
            .push(" does not exist yet · ", look.warning())
            .push("Ctrl+S", look.key());
        l.push(
            if wide {
                " creates it with the settings you change; everything else stays default"
            } else {
                " creates it"
            },
            look.secondary(),
        );
    }
    let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
    if text.width() > w {
        // Too long: one plain line, cut.
        let st = l.spans.get(1).map_or(look.text(), |s| s.style);
        put(frame, area, Line::styled(fit(&text, w), st));
    } else {
        put(frame, area, l.line());
    }
}

/// The footer keys of the window's current mode.
fn footer_keys(win: &SettingsWindow) -> Vec<(&'static str, &'static str)> {
    match &win.mode {
        Mode::Pick(pick) if pick.key.previewed() => {
            vec![("↑↓", "preview"), ("Enter", "keep"), ("Esc", "revert")]
        }
        Mode::Pick(_) => vec![("↑↓", "move"), ("Enter", "keep"), ("Esc", "cancel")],
        Mode::Input(_) => vec![
            ("Enter", "keep"),
            ("Esc", "undo edit"),
            ("←→", "cursor"),
            ("Ctrl+U", "clear"),
        ],
        Mode::List(list) if list.input.is_some() => vec![
            ("Enter", "keep"),
            ("Tab", "complete folder"),
            ("Ctrl+U", "clear"),
            ("Esc", "cancel input"),
        ],
        Mode::List(_) => vec![
            ("a", "add"),
            ("Enter", "edit"),
            ("d", "remove"),
            ("Esc", "back to settings"),
        ],
        Mode::Unsaved => vec![
            ("y", "save and close"),
            ("n", "discard and close"),
            ("Esc", "keep editing"),
        ],
        Mode::EditPrompt => vec![
            ("y", "save and edit"),
            ("n", "discard and edit"),
            ("Esc", "cancel"),
        ],
        Mode::Conflict(_) => vec![("y", "overwrite"), ("n", "cancel")],
        Mode::CrossGroup => vec![("y", "turn on"), ("n", "keep it off")],
        Mode::QuitUnsaved => vec![("y", "quit"), ("n", "keep editing")],
        Mode::Help => vec![("Esc", "close help")],
        Mode::Browse => vec![
            ("↑↓", "move"),
            ("Enter", "edit"),
            ("←→", "change"),
            ("Ctrl+S", "save"),
            ("Esc", "close"),
            ("Tab", "next section"),
            ("Del", "default"),
            ("e", "editor"),
        ],
    }
}

/// The footer: whole key hints that fit, then `? help` on the right.
fn draw_footer(frame: &mut Frame, win: &SettingsWindow, area: Rect, look: Look) {
    let w = usize::from(area.width);
    let mut right = Lb::new();
    right.push("?", look.key()).push(" help ", look.key_label());
    let mut left = Lb::new();
    left.push(" ", look.key_label());
    for (i, (k, v)) in footer_keys(win).into_iter().enumerate() {
        let item = k.width() + 1 + v.width() + GAP.width();
        if left.w + item + right.w + 3 > w {
            break;
        }
        if i > 0 {
            left.push(GAP, look.key_label());
        }
        left.push(k, look.key())
            .push(format!(" {v}"), look.key_label());
    }
    put_lr(frame, area, left, right);
}

/// One line of a pick list or the list editor: the selection bar, a
/// `▌` in the mark colour.
fn bar_line(mut l: Lb, selected: bool, w: usize, look: Look) -> Line<'static> {
    if selected {
        l.pad_to(w, look.selected_item());
    }
    l.line()
}

/// A short description of a style or theme in the pick list.
fn choice_note(key: SettingKey, value: &str) -> &'static str {
    match (key, value) {
        (SettingKey::UiStyle, "dashboard") => "rounded panels, pills and a detail panel",
        (SettingKey::UiStyle, "minimal") => "the plain single-column list",
        (SettingKey::UiTheme, "neutral") => "greys; colour only for states",
        (SettingKey::UiTheme, "warm") => "warm greys and a clay accent",
        (SettingKey::UiTheme, "terminal") => "your terminal's 16 colours",
        (SettingKey::UiTheme, "tokyo-night") => "tower 0.2's blue-grey palette",
        (SettingKey::UiTheme, "neutral-light") => "neutral for a light background",
        (SettingKey::UiTheme, "warm-light") => "warm for a light background",
        (SettingKey::UiTheme, "ansi16") => "16 colours, dark or light background",
        (SettingKey::UiColorDepth, "truecolor") => "24-bit colour: themes as designed",
        (SettingKey::UiColorDepth, "256") => "nearest of 256 colours",
        (SettingKey::UiColorDepth, "16") => "truecolor themes give way to ansi16",
        (SettingKey::DefaultModel, OTHER) => "type a full model name",
        (SettingKey::LogsFollowSecs, "off") => "no background fetches",
        _ => "",
    }
}

/// The pick list of a choice, under its row (above it when there is no
/// room below); a theme's entry shows a swatch of its state colours.
fn draw_pick(
    frame: &mut Frame,
    win: &SettingsWindow,
    pick: &Pick,
    anchor: Rect,
    screen: Rect,
    look: Look,
    detected: ColorDepth,
) {
    let wide_list = pick.key.previewed();
    let w: u16 = if wide_list { 74 } else { 52 };
    let w = w.min(screen.width.saturating_sub(4));
    let h = (pick.items.len() as u16 + 4).min(screen.height.saturating_sub(2));
    let x = anchor
        .x
        .saturating_sub(4)
        .min(screen.width.saturating_sub(w + 1));
    let below = anchor.y + 1;
    let y = if below + h <= screen.height.saturating_sub(2) {
        below
    } else {
        anchor.y.saturating_sub(h)
    };
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let before = win.session.value(pick.key).display();
    let bottom = if wide_list {
        format!(" ↑↓ preview   Enter keep   Esc revert to {before} ")
    } else {
        " ↑↓ move   Enter keep   Esc cancel ".to_owned()
    };
    let block = look
        .overlay_block(format!(" {} ", pick.key.label()))
        .title_bottom(look.hint_line(bottom));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let iw = usize::from(inner.width);
    let saved = win
        .session
        .saved(pick.key)
        .map(|v| shown_value(pick.key, &v));
    let default = shown_value(pick.key, &win.session.default_value(pick.key));
    let rows = usize::from(inner.height.saturating_sub(2));
    let offset = pick.cursor.saturating_sub(rows.saturating_sub(1));
    for (i, value) in pick.items.iter().enumerate().skip(offset).take(rows) {
        let selected = i == pick.cursor;
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
            fit(value, 13),
            if selected {
                plain.add_modifier(Modifier::BOLD)
            } else {
                plain
            },
        );
        if pick.key == SettingKey::UiTheme
            && let Ok(theme) = UiTheme::parse(value)
            && let Look::Themed(th) = look
            && th.color
        {
            let p = palette(theme);
            for c in [p.red, p.amber, p.green, p.blue, p.teal, p.grey] {
                l.push("  ", Style::new().bg(c));
            }
            l.push("  ", plain);
        }
        let tag = if saved.as_deref() == Some(value.as_str()) {
            "  saved"
        } else if saved.is_none() && *value == default {
            "  default"
        } else {
            ""
        };
        let room = iw.saturating_sub(l.w + tag.width());
        let note = if pick.key == SettingKey::UiColorDepth && value == "auto" {
            match detected {
                ColorDepth::Ansi16 => "from COLORTERM and TERM: 16 here",
                _ => "from COLORTERM and TERM: full colour here",
            }
            .to_owned()
        } else {
            choice_note(pick.key, value).to_owned()
        };
        l.push(fit(&note, room), bar(look.secondary()));
        l.push(tag, bar(look.hint()));
        put(
            frame,
            row(inner, 1 + (i - offset) as u16),
            bar_line(l, selected, iw, look),
        );
    }
}

/// The list editor of a folder list, centred over the settings list.
fn draw_list_editor(
    frame: &mut Frame,
    win: &SettingsWindow,
    list: &ListEditor,
    body: Rect,
    look: Look,
) {
    let w = 78.min(body.width.saturating_sub(4));
    let iw = usize::from(w.saturating_sub(2));
    let help = wrap(list.key.help(), iw.saturating_sub(2));
    let notes = [
        "~ is kept as typed and expanded when scanning. Duplicates are refused.",
        "Changes stay unsaved until Ctrl+S in the Settings window.",
    ];
    let note_lines: Vec<String> = notes
        .iter()
        .flat_map(|n| wrap(n, iw.saturating_sub(2)))
        .collect();
    let adding = list.input.as_ref().is_some_and(|i| i.index.is_none());
    let entries = list.items.len() + usize::from(adding);
    let fixed = help.len() + 1 + 1 + note_lines.len() + 2;
    let max_h = usize::from(body.height.saturating_sub(2));
    let room = max_h.saturating_sub(fixed).max(1);
    let shown = entries.max(1).min(room);
    let h = (fixed + shown).min(max_h.max(1)) as u16;
    let rect = centered(body, w, h);
    frame.render_widget(Clear, rect);
    let mut count = plural(list.items.len(), "folder", "folders");
    if adding {
        count.push_str(" + 1 being added");
    }
    let block = look
        .overlay_block(format!(" {} ", list.key.label()))
        .title_top(Line::styled(format!(" {count} "), look.secondary()).right_aligned())
        .title_bottom(look.hint_line(" a add   Enter edit   d remove   Esc back to settings "));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut y = 0u16;
    for t in help {
        put(
            frame,
            row(inner, y),
            Line::from(vec![Span::raw(" "), Span::styled(t, look.secondary())]),
        );
        y += 1;
    }
    y += 1;
    let current = if adding {
        list.items.len()
    } else {
        list.selected
    };
    let offset = current.saturating_sub(shown.saturating_sub(1));
    if entries == 0 {
        put(
            frame,
            row(inner, y),
            Line::styled("    no folders yet; a adds one", look.hint()),
        );
    }
    for i in (0..entries).skip(offset).take(shown) {
        let ra = row(inner, y + (i - offset) as u16);
        let selected = i == current;
        let editing = list
            .input
            .as_ref()
            .filter(|input| input.index.unwrap_or(list.items.len()) == i);
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
        let saved_items = match win.session.saved(list.key) {
            Some(SettingValue::Paths(p)) => p,
            _ => Vec::new(),
        };
        if let Some(input) = editing {
            l.push("* ", bar(look.modified()));
            l.push(if input.index.is_none() { "+ " } else { "  " }, plain);
            let border = if input.error.is_some() {
                look.error()
            } else {
                look.key()
            };
            let text = crate::settings::clean_folder(&input.input.text());
            let (note, kind) = match &input.error {
                Some(e) => (format!("✗ {e}"), NoticeKind::Error),
                None if text.is_empty() => (String::new(), NoticeKind::Info),
                None => win.folder_note(list.key, &text),
            };
            let room = iw.saturating_sub(l.w + 6 + note.width().min(36));
            let field_w = (input.input.text().width() + 1).clamp(20.min(room), room.max(4));
            let (shown_text, col) = input_window(&input.input, field_w);
            l.push("[ ", bar(border));
            let text_x = l.w;
            l.push(shown_text.clone(), bar(look.strong()));
            l.push(
                " ".repeat(field_w.saturating_sub(shown_text.width())),
                plain,
            );
            l.push(" ]  ", bar(border));
            l.push(
                fit(&note, iw.saturating_sub(l.w)).trim_end().to_owned(),
                bar(kind_style(kind, look)),
            );
            put(frame, ra, bar_line(l, selected, iw, look));
            let x = ra.x + (text_x + col).min(iw.saturating_sub(1)) as u16;
            frame.set_cursor_position((x, ra.y));
            continue;
        }
        let item = &list.items[i];
        let changed = !saved_items.contains(item);
        l.push(if changed { "* " } else { "  " }, bar(look.modified()));
        l.push(" ", plain);
        l.push(fit(item, 24), plain);
        let (note, kind) = win.folder_note(list.key, item);
        l.push(
            fit(&note, iw.saturating_sub(l.w)).trim_end().to_owned(),
            bar(kind_style(kind, look)),
        );
        put(frame, ra, bar_line(l, selected, iw, look));
    }
    let first = y + shown as u16 + 1;
    for (y, t) in (first..inner.height).zip(note_lines) {
        put(
            frame,
            row(inner, y),
            Line::from(vec![Span::raw(" "), Span::styled(t, look.hint())]),
        );
    }
}

/// The questions: unsaved changes on `Esc` or `e`, a conflict on save,
/// the cross-group hand-off, quitting with unsaved changes.
fn draw_question(frame: &mut Frame, win: &SettingsWindow, body: Rect, look: Look) {
    let w = 76.min(body.width.saturating_sub(4));
    let iw = usize::from(w.saturating_sub(4)).max(10);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let text = |lines: &mut Vec<Line<'static>>, t: &str, st: Style| {
        for r in wrap(t, iw) {
            lines.push(Line::from(vec![Span::raw(" "), Span::styled(r, st)]));
        }
    };
    let changes = |lines: &mut Vec<Line<'static>>| {
        for c in win.session.changes() {
            let mut l = Lb::new();
            l.push("   ", look.text())
                .push("* ", look.modified())
                .push(fit(c.key.label(), 18), look.text())
                .push(
                    fit(&change_text(&c), iw.saturating_sub(21))
                        .trim_end()
                        .to_owned(),
                    look.secondary(),
                );
            lines.push(l.line());
        }
    };
    let n = win.change_count();
    let changed = format!(
        "{} changed and not saved:",
        plural(n, "setting", "settings")
    );
    let (title, answers): (&str, Vec<(&str, &str)>) = match &win.mode {
        Mode::Unsaved => {
            text(&mut lines, &changed, look.text());
            lines.push(Line::raw(""));
            changes(&mut lines);
            lines.push(Line::raw(""));
            text(
                &mut lines,
                &format!("Save them to {}?", win.shown_path),
                look.strong(),
            );
            (
                " Unsaved changes ",
                vec![
                    ("y", "save and close"),
                    ("n", "discard"),
                    ("Esc/Enter", "keep editing"),
                ],
            )
        }
        Mode::EditPrompt => {
            text(&mut lines, &changed, look.text());
            lines.push(Line::raw(""));
            changes(&mut lines);
            lines.push(Line::raw(""));
            text(
                &mut lines,
                &format!("Save them before {} opens in your editor?", win.shown_path),
                look.strong(),
            );
            (
                " Unsaved changes ",
                vec![
                    ("y", "save and edit"),
                    ("n", "discard and edit"),
                    ("Esc/Enter", "cancel"),
                ],
            )
        }
        Mode::Conflict(conflicts) => {
            text(
                &mut lines,
                "Changed in the file since this window opened (another tower or a hand edit):",
                look.text(),
            );
            lines.push(Line::raw(""));
            let shown = |v: &Option<SettingValue>| {
                v.as_ref().map_or_else(|| "not set".to_owned(), toml_text)
            };
            for c in conflicts {
                let mut l = Lb::new();
                l.push("   ", look.text())
                    .push("* ", look.modified())
                    .push(c.key.toml_key(), look.strong());
                lines.push(l.line());
                let mut l = Lb::new();
                l.push("       file  ", look.hint()).push(
                    fit(&shown(&c.on_disk), iw.saturating_sub(14))
                        .trim_end()
                        .to_owned(),
                    look.text(),
                );
                lines.push(l.line());
                let mine = c
                    .mine
                    .as_ref()
                    .map_or_else(|| "removed (the default)".to_owned(), toml_text);
                let mut l = Lb::new();
                l.push("       mine  ", look.hint()).push(
                    fit(&mine, iw.saturating_sub(14)).trim_end().to_owned(),
                    look.warning(),
                );
                lines.push(l.line());
            }
            lines.push(Line::raw(""));
            text(
                &mut lines,
                "Overwrite them with your values?",
                look.strong(),
            );
            (
                " Changed elsewhere ",
                vec![
                    ("y", "overwrite with mine"),
                    ("n/Esc", "cancel; nothing is saved"),
                ],
            )
        }
        Mode::CrossGroup => {
            text(&mut lines, &cross_group_text(), look.text());
            lines.push(Line::raw(""));
            text(&mut lines, CROSS_GROUP_TERMS, look.secondary());
            lines.push(Line::raw(""));
            text(&mut lines, "Turn on cross-group hand-offs?", look.strong());
            (
                " Allow cross-group hand-off? ",
                vec![("y", "turn on"), ("n/Esc", "keep it off")],
            )
        }
        Mode::QuitUnsaved => {
            text(
                &mut lines,
                &format!(
                    "{} not saved; quitting discards {}.",
                    plural(n, "change is", "changes are"),
                    if n == 1 { "it" } else { "them" }
                ),
                look.text(),
            );
            lines.push(Line::raw(""));
            changes(&mut lines);
            (
                " Quit tower? ",
                vec![("y", "quit"), ("n/Esc", "keep editing")],
            )
        }
        _ => return,
    };
    lines.push(Line::raw(""));
    let mut l = Lb::new();
    for (i, (k, v)) in answers.iter().enumerate() {
        l.push(if i == 0 { " " } else { GAP }, look.text());
        l.push(*k, look.answer_key())
            .push(format!(" {v}"), look.text());
    }
    lines.push(l.line());
    let h = (lines.len() as u16 + 2).min(body.height);
    let rect = centered(body, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(look.overlay_block(title)), rect);
}

/// The terms note of the cross-group question (doc/0018).
pub const CROSS_GROUP_TERMS: &str = "Terms follow the account: Claude Code is under the \
    Commercial Terms for Team, Enterprise and API accounts and the Consumer Terms for Free, Pro \
    and Max accounts, and their training and retention rules differ. Moving an employer's \
    session to a personal account moves its code and conversation under other terms.";

/// The first line of the cross-group question.
fn cross_group_text() -> String {
    "A hand-off moves the whole conversation and project context to the target account, which \
     continues it under that account's terms. With this on, `a` also offers accounts of other \
     groups; every hand-off still asks first."
        .to_owned()
}

/// The window's keys.
pub const SETTINGS_KEY_TABLE: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move; PgUp/PgDn, Home/End"),
    ("Tab/Shift+Tab", "next / previous section"),
    ("Enter", "edit: pick, toggle, type, or the folder list"),
    ("←/→", "previous / next value"),
    ("Del", "back to the default (removed from the file)"),
    ("Ctrl+S", "save; the window stays open"),
    ("e", "open config.toml in $VISUAL / $EDITOR"),
    ("Esc", "close (asks when something is unsaved)"),
    ("a, Enter, d", "folder list: add, edit, remove"),
    ("Tab", "folder input: complete the folder name"),
];

fn draw_help(frame: &mut Frame, area: Rect, look: Look) {
    let lines: Vec<Line> = SETTINGS_KEY_TABLE
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(" ", look.text()),
                Span::styled(format!("{k:<14}"), look.table_key()),
                Span::styled(format!(" {v}"), look.text()),
            ])
        })
        .collect();
    let h = lines.len() as u16 + 2;
    let rect = centered(area, 66, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            look.overlay_block(" Settings keys ")
                .title_bottom(look.hint_line(" Esc or ? closes this help ")),
        ),
        rect,
    );
}
