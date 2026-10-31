//! The dashboard's detail panel (doc/0020): what tower knows about the
//! selected session, or a summary of the selected group. A side panel on
//! wide terminals, a four-line strip under the list on narrower ones; both
//! show the folder's `git` summary when tower has one (doc/0037). The side
//! panel wraps long values onto the rows it has free; the strip and the
//! titles cut them with `…` (doc/0041).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::dashboard::{account_chip, panel, put, row, spinner};
use super::theme::{Lb, Theme};
use super::{MARKER, age_column};
use crate::app::{App, Entry};
use crate::config::Account;
use crate::groups::{self, Group, GroupKey};
use crate::handoff;
use crate::merge::MergedSession;
use crate::paths;

/// Label column width of the key/value lines.
const KEY_W: usize = 10;

/// The most rows a wrapped value of the side panel takes (doc/0041).
const MAX_VALUE_ROWS: usize = 3;

/// The most rows the side panel's keys take (doc/0041).
const MAX_KEY_ROWS: usize = 2;

/// The most rows the side panel's `usage` value takes (doc/0043).
const MAX_USAGE_ROWS: usize = 8;

/// The most rows each value of [`ValueRows`] takes.
const MAX_ROWS: ValueRows = [
    MAX_VALUE_ROWS,
    MAX_VALUE_ROWS,
    MAX_VALUE_ROWS,
    MAX_VALUE_ROWS,
    MAX_USAGE_ROWS,
];

/// What the panel describes.
enum Subject<'a> {
    Session(&'a MergedSession),
    Group {
        key: GroupKey,
        count: usize,
        lit: usize,
        collapsed: bool,
    },
    Nothing,
}

fn subject(app: &App) -> Subject<'_> {
    let Some(k) = app.selected_entry() else {
        return Subject::Nothing;
    };
    match &app.entries()[k] {
        Entry::Row(i) => Subject::Session(&app.sessions()[*i]),
        Entry::Header {
            group,
            count,
            highlighted,
            collapsed,
        } => Subject::Group {
            key: group.clone(),
            count: *count,
            lit: *highlighted,
            collapsed: *collapsed,
        },
    }
}

/// A `label     value` line.
fn kv(label: &str, value: impl Into<String>, style: Style, th: Theme) -> Lb {
    let mut l = Lb::new();
    l.push(format!(" {label:<KEY_W$}"), th.faint());
    l.push(value, style);
    l
}

/// The owning account, when known and configured.
fn owner<'a>(app: &'a App, m: &MergedSession) -> Option<&'a Account> {
    m.owner
        .account()
        .and_then(|name| app.config().account(name))
}

/// The first line: the attention marker and what the row waits for, or
/// its state on a pill in the group's colour.
fn headline(app: &App, m: &MergedSession, th: Theme) -> Lb {
    let detail = groups::detail(&m.session);
    let gc = th.group_color(groups::classify(&m.session));
    let mut l = Lb::new();
    if app.is_highlighted(m) {
        l.push(format!(" {MARKER} needs you: {detail}"), th.attention());
    } else {
        l.push(" ", Style::new());
        l.push(format!(" {detail} "), th.pill(gc));
    }
    l
}

/// `status`, with `waitingFor` when waiting; "no live process" for
/// stopped rows (doc/0006); the background-task sentence when a task
/// outlived the turn (doc/0032).
fn status_text(m: &MergedSession) -> String {
    let s = &m.session;
    if groups::bg_task(s) {
        return groups::BG_TASK_STATUS.to_owned();
    }
    match (&s.status, &s.waiting_for) {
        (Some(status), Some(w)) => format!("{status} · {w}"),
        (Some(status), None) => status.to_string(),
        (None, _) if s.is_interactive() => "-".to_owned(),
        (None, _) => "no live process".to_owned(),
    }
}

/// When the row last changed: `42s ago`.
fn changed_text(app: &App, m: &MergedSession, now_ms: i64) -> String {
    match age_column(app, m, now_ms).as_str() {
        "-" => "unknown".to_owned(),
        "now" => "just now".to_owned(),
        age => format!("{age} ago"),
    }
}

/// The account chip with its group and command, or `?` when unknown.
fn account_line(app: &App, m: &MergedSession, label: &str, th: Theme) -> Lb {
    let mut l = Lb::new();
    l.push(format!(" {label:<KEY_W$}"), th.faint());
    match owner(app, m) {
        Some(a) => {
            let (chip, style) = account_chip(app, &a.name, th);
            l.push(chip, style);
            let group = if a.group.is_empty() {
                String::new()
            } else {
                format!("{} · ", a.group)
            };
            l.push(format!(" {group}{}", a.command), th.muted());
        }
        None => {
            l.push(format!("{} (unknown)", m.owner.label()), th.muted());
        }
    }
    l
}

/// Whether a hand-off target exists (doc/0004, doc/0018).
fn handoff_text(app: &App, m: &MergedSession) -> String {
    if !m.session.is_attachable() {
        return "not available here".to_owned();
    }
    let owner = owner(app, m);
    match handoff::targets(app.config(), owner, &m.session.display_name()) {
        Ok(t) if t.is_empty() => "no other account".to_owned(),
        Ok(t) => format!("to {}", t.join(", ")),
        Err(_) => match owner {
            Some(a) if !a.group.is_empty() => format!("no other {} account", a.group),
            Some(_) => "no account in its group".to_owned(),
            None => "owner unknown; attach first".to_owned(),
        },
    }
}

/// `git       feature/login · 3 changed · ↑2` for the session's folder in
/// a line `width` columns wide, when tower has a summary for it
/// (doc/0037): the strip's single row, the branch cut with `…` first so
/// the counts stay (doc/0041). Not inside a work tree, no git, a folder
/// that is gone or a `git status` that failed: no line.
fn git_line(app: &App, m: &MergedSession, width: usize, th: Theme) -> Option<Lb> {
    let summary = app.git_summary(&m.session.cwd)?;
    let room = width.saturating_sub(1 + KEY_W + 1);
    Some(kv("git", summary.fitted(room), th.text(), th))
}

/// The side panel's `git` lines: the whole summary wrapped onto at most
/// `rows` rows (doc/0041). The branch is not shortened while the summary
/// fits them; when it does not, the branch is cut with `…` first so the
/// counts stay.
fn git_lines(app: &App, m: &MergedSession, width: usize, rows: usize, th: Theme) -> Vec<Lb> {
    let Some(summary) = app.git_summary(&m.session.cwd) else {
        return Vec::new();
    };
    let room = value_room(width);
    let rows = rows.max(1);
    let mut parts = wrap_value(&summary.shown(), room);
    if parts.len() > rows {
        // The longest cut of the branch whose line still wraps onto
        // `rows` rows; one row is the strip's cut.
        parts = (room + 1..room * rows)
            .rev()
            .map(|w| wrap_value(&summary.fitted(w), room))
            .find(|p| p.len() <= rows)
            .unwrap_or_else(|| vec![summary.fitted(room)]);
    }
    labelled("git", parts, th.text(), th)
}

/// The short id and kind: `5e6f7a8b · background`.
fn id_text(m: &MergedSession) -> (String, &'static str) {
    let id = m.session.id.clone().unwrap_or_else(|| "-".to_owned());
    let kind = if m.session.is_interactive() {
        "interactive"
    } else {
        "background"
    };
    (id, kind)
}

/// The keys that apply to the selected line: the session's own actions
/// first, the hand-off to another account last (doc/0041).
fn keys(app: &App, subject: &Subject) -> Vec<(&'static str, &'static str)> {
    match subject {
        Subject::Session(m) if !m.session.is_attachable() => Vec::new(),
        Subject::Session(m) => {
            let mut k = vec![("Enter", "attach"), ("Space", "logs")];
            if app.is_highlighted(m) {
                k.push(("m", "seen"));
            }
            k.push(("s", "stop"));
            if !m.session.cwd.is_empty() {
                k.push(("!", "shell"));
            }
            if matches!(
                handoff::targets(app.config(), owner(app, m), ""),
                Ok(t) if !t.is_empty()
            ) {
                k.push(("a", "hand-off"));
            }
            k
        }
        Subject::Group { collapsed, .. } => {
            vec![("Enter", if *collapsed { "unfold" } else { "fold" })]
        }
        Subject::Nothing => vec![("n", "new")],
    }
}

/// The keys of the selected line on rows `width` columns wide, at most
/// `max_rows` of them: only the gaps between keys count, and a key that
/// does not fit the row goes on the next (doc/0041).
fn key_rows(app: &App, subject: &Subject, width: usize, max_rows: usize, th: Theme) -> Vec<Lb> {
    let start = || {
        let mut l = Lb::new();
        l.push(" ", th.muted());
        l
    };
    let fits = |l: &Lb, item: usize| l.w + if l.w > 1 { 2 } else { 0 } + item <= width;
    let mut rows = vec![start()];
    for (key, what) in keys(app, subject) {
        let item = key.width() + 1 + what.width();
        let last = rows.len() - 1;
        if !fits(&rows[last], item) && rows[last].w > 1 && rows.len() < max_rows.max(1) {
            rows.push(start());
        }
        let l = rows.last_mut().expect("one row at least");
        if !fits(l, item) {
            continue;
        }
        if l.w > 1 {
            l.push("  ", th.muted());
        }
        l.push(key, th.bold(th.p.key))
            .push(format!(" {what}"), th.muted());
    }
    if !matches!(subject, Subject::Nothing) && rows[0].w == 1 {
        rows[0].push("not attachable", th.muted());
    }
    rows
}

/// A summary of a group: its title on a pill (wrapped onto at most `rows`
/// rows of a line `width` columns wide), then sessions, highlighted,
/// accounts and states.
fn group_lines(
    app: &App,
    key: &GroupKey,
    count: usize,
    lit: usize,
    width: usize,
    rows: usize,
    th: Theme,
) -> Vec<Lb> {
    let members = app.group_members(key);
    let pill = th.pill(th.key_color(key));
    let mut lines: Vec<Lb> = wrap_capped(key.title(), width.saturating_sub(4).max(8), rows)
        .into_iter()
        .map(|part| {
            let mut l = Lb::new();
            l.push(" ", Style::new());
            l.push(format!(" {part} "), pill);
            l
        })
        .collect();
    lines.push(Lb::new());
    let noun = if count == 1 { "session" } else { "sessions" };
    lines.push(kv("sessions", format!("{count} {noun}"), th.text(), th));
    if lit > 0 {
        lines.push(kv(
            "attention",
            format!("{MARKER} {lit}"),
            th.attention(),
            th,
        ));
    }
    let mut accounts: Vec<(String, usize)> = Vec::new();
    for m in &members {
        let name = m.owner.label().to_owned();
        match accounts.iter_mut().find(|(a, _)| *a == name) {
            Some((_, n)) => *n += 1,
            None => accounts.push((name, 1)),
        }
    }
    if !accounts.is_empty() {
        let text: Vec<String> = accounts.iter().map(|(a, n)| format!("{a} {n}")).collect();
        lines.push(kv("accounts", text.join(" · "), th.text(), th));
    }
    if matches!(key, GroupKey::Project(_)) {
        let mut states: Vec<(Group, usize)> = Vec::new();
        for m in &members {
            let g = groups::classify(&m.session);
            match states.iter_mut().find(|(s, _)| *s == g) {
                Some((_, n)) => *n += 1,
                None => states.push((g, 1)),
            }
        }
        states.sort();
        let text: Vec<String> = states
            .iter()
            .map(|(g, n)| format!("{} {n}", g.title()))
            .collect();
        lines.push(kv("states", text.join(" · "), th.text(), th));
    }
    lines
}

/// The columns a value has in a line `width` columns wide: the label
/// column and a one-column margin on either side are taken.
fn value_room(width: usize) -> usize {
    width.saturating_sub(1 + KEY_W + 1).max(8)
}

/// `label     part 1`, then the other parts under the value column.
fn labelled(label: &str, parts: Vec<String>, style: Style, th: Theme) -> Vec<Lb> {
    parts
        .into_iter()
        .enumerate()
        .map(|(k, part)| kv(if k == 0 { label } else { "" }, part, style, th))
        .collect()
}

/// A `label     value` line, the value wrapped to `width` columns less a
/// one-column margin onto at most `rows` rows and continued under itself;
/// a value longer than that ends in `…` (doc/0041).
fn kv_wrapped(
    label: &str,
    value: &str,
    width: usize,
    rows: usize,
    style: Style,
    th: Theme,
) -> Vec<Lb> {
    labelled(
        label,
        wrap_capped(value, value_room(width), rows),
        style,
        th,
    )
}

/// Whether the space at `chars[j]` may end a row: not one next to a `·`,
/// so a separator never starts or ends a row.
fn breaks_at_space(chars: &[(usize, char)], j: usize) -> bool {
    let before = j.checked_sub(1).map(|i| chars[i].1);
    let after = chars.get(j + 1).map(|c| c.1);
    before != Some('·') && after != Some('·')
}

/// The rows of `text` wrapped to `width` columns, as byte ranges: a row
/// ends at a space (which is dropped), after a `/` or after a `-`, at the
/// last such place that fits; a part longer than a row is broken where
/// the row ends (doc/0041).
fn wrap_ranges(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |i: usize| chars.get(i).map_or(text.len(), |c| c.0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A row does not start with the space it broke at.
        if !out.is_empty() {
            while i < chars.len() && chars[i].1 == ' ' {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
        }
        let mut used = 0;
        let mut j = i;
        // Where the row may end, and where the next one starts.
        let mut brk: Option<(usize, usize)> = None;
        while j < chars.len() {
            let c = chars[j].1;
            if c == ' ' && j > i && breaks_at_space(&chars, j) {
                brk = Some((j, j + 1));
            }
            let w = c.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            used += w;
            if (c == '/' || c == '-') && j + 1 < chars.len() {
                brk = Some((j + 1, j + 1));
            }
            j += 1;
        }
        if j >= chars.len() {
            out.push((byte(i), text.len()));
            break;
        }
        let (end, next) = brk.unwrap_or((j.max(i + 1), j.max(i + 1)));
        let row = text[byte(i)..byte(end)].trim_end_matches(' ');
        out.push((byte(i), byte(i) + row.len()));
        i = next;
    }
    out
}

/// `text` wrapped to `width` columns at spaces, `/` and `-` (doc/0041).
fn wrap_value(text: &str, width: usize) -> Vec<String> {
    wrap_ranges(text, width)
        .into_iter()
        .map(|(a, b)| text[a..b].to_owned())
        .collect()
}

/// [`wrap_value`] onto at most `rows` rows: the last one ends in `…` when
/// the text goes on.
fn wrap_capped(text: &str, width: usize, rows: usize) -> Vec<String> {
    let rows = rows.max(1);
    let ranges = wrap_ranges(text, width);
    if ranges.len() <= rows {
        return ranges.iter().map(|&(a, b)| text[a..b].to_owned()).collect();
    }
    let mut out: Vec<String> = ranges[..rows - 1]
        .iter()
        .map(|&(a, b)| text[a..b].to_owned())
        .collect();
    out.push(ellipsize(&text[ranges[rows - 1].0..], width));
    out
}

/// `text` in at most `width` columns, ending in `…` when cut.
fn ellipsize(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// `l` in at most `width` columns: when it is wider, its text ends in `…`
/// where it is cut, in the style of the span that was cut.
fn fit_line(l: Lb, width: usize) -> Lb {
    if l.w <= width {
        return l;
    }
    let budget = width.saturating_sub(1);
    let mut cut = Lb::new();
    for span in l.spans {
        let text = span.content.into_owned();
        if cut.w + text.width() <= budget {
            cut.push(text, span.style);
            continue;
        }
        let mut part = String::new();
        let mut used = cut.w;
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > budget {
                break;
            }
            part.push(ch);
            used += w;
        }
        if width > 0 {
            part.push('…');
        }
        cut.push(part, span.style);
        break;
    }
    cut
}

/// How many rows each wrapped value of the side panel may take, in the
/// order the free rows go to them (doc/0041): folder, `git`, status, name,
/// and the owner's plan usage (doc/0043). A group's title uses the first.
type ValueRows = [usize; 5];

/// The side panel's `usage` lines for the owner of `m`, at most `rows` of
/// them (doc/0043): nothing while `[usage] show` is off or the owner is
/// unknown.
fn usage_lines(
    app: &App,
    m: &MergedSession,
    width: usize,
    rows: usize,
    now_ms: i64,
    th: Theme,
) -> Vec<Lb> {
    let Some(rows_of) =
        owner(app, m).and_then(|a| super::usage::detail_rows(app, &a.name, now_ms, th))
    else {
        return Vec::new();
    };
    let room = value_room(width);
    // The first rows first; a value that gets fewer rows than it needs
    // ends in `…`.
    let mut budget = rows.max(1);
    let mut parts: Vec<(String, Style)> = Vec::new();
    for r in rows_of {
        if budget == 0 {
            break;
        }
        let wrapped = wrap_capped(&r.text, room, r.rows.min(budget));
        budget -= wrapped.len();
        parts.extend(wrapped.into_iter().map(|part| (part, r.style)));
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(k, (part, style))| kv(if k == 0 { "usage" } else { "" }, part, style, th))
        .collect()
}

/// The side panel's lines for a session, `width` columns wide, its values
/// wrapped onto at most `rows` rows each. `name` is the full name when the
/// title had to shorten it.
fn session_lines(
    app: &App,
    m: &MergedSession,
    width: usize,
    rows: ValueRows,
    name: Option<&str>,
    now_ms: i64,
    th: Theme,
) -> Vec<Lb> {
    let s = &m.session;
    let group = groups::classify(s);
    let (id, kind) = id_text(m);
    let state = s.state.as_ref().map_or("-".to_owned(), ToString::to_string);
    let mut status = status_text(m);
    // Only a working turn spins (doc/0032).
    if groups::spins(s) {
        status = format!("{} {status}", spinner(now_ms));
    }
    // The background-task sentence is tower's own and is shown whole
    // while the panel has the rows (doc/0032).
    let [folder_rows, git_rows, status_rows, name_rows, usage_rows] = rows;
    let status_rows = if groups::bg_task(s) && status_rows >= MAX_VALUE_ROWS {
        usize::MAX
    } else {
        status_rows
    };
    let text = th.text();
    let mut lines = vec![headline(app, m, th), Lb::new()];
    if let Some(name) = name {
        lines.extend(kv_wrapped("name", name, width, name_rows, text, th));
    }
    // A pinned row keeps its state's group here (doc/0047).
    let group_text = if app.is_pinned(m) {
        format!("{} · pinned", group.title())
    } else {
        group.title().to_owned()
    };
    lines.extend([
        kv(
            "group",
            group_text,
            th.hue(th.group_color(group)).add_modifier(Modifier::BOLD),
            th,
        ),
        kv("state", state, text, th),
    ]);
    lines.extend(kv_wrapped("status", &status, width, status_rows, text, th));
    lines.extend([
        kv("changed", changed_text(app, m, now_ms), text, th),
        Lb::new(),
    ]);
    let folder = paths::shorten_home(&s.cwd, app.home());
    lines.extend(kv_wrapped("folder", &folder, width, folder_rows, text, th));
    lines.extend(git_lines(app, m, width, git_rows, th));
    lines.push(account_line(app, m, "account", th));
    lines.extend(usage_lines(app, m, width, usage_rows, now_ms, th));
    lines.extend([
        kv("id", id, text, th),
        kv("kind", kind, text, th),
        Lb::new(),
        kv("hand-off", handoff_text(app, m), th.muted(), th),
    ]);
    lines
}

/// The details of the selected line for the workspace's pane (doc/0045),
/// `width` columns wide: the side panel's lines, without its keys.
pub(super) fn pane_lines(app: &App, width: usize, now_ms: i64, th: Theme) -> Vec<Lb> {
    match subject(app) {
        Subject::Session(m) => session_lines(app, m, width, MAX_ROWS, None, now_ms, th),
        Subject::Group {
            key, count, lit, ..
        } => group_lines(app, &key, count, lit, width, MAX_VALUE_ROWS, th),
        Subject::Nothing => {
            let mut l = Lb::new();
            l.push(" nothing selected", th.muted());
            vec![l]
        }
    }
}

/// The panel's title (the name, not yet fitted) and border colour.
fn frame_of(subject: &Subject, th: Theme) -> (String, ratatui::style::Color) {
    match subject {
        Subject::Session(m) => (
            m.session.display_name(),
            th.detail_border(th.group_color(groups::classify(&m.session))),
        ),
        Subject::Group { key, .. } => (key.title().to_owned(), th.detail_border(th.key_color(key))),
        Subject::Nothing => ("details".to_owned(), th.p.line),
    }
}

/// The title on the top border of a panel `area_w` columns wide: the
/// name between a space on either side, cut with `…` when it does not
/// fit between the corners (doc/0041); and whether it was cut.
fn title_of(name: &str, area_w: u16) -> (String, bool) {
    let room = usize::from(area_w).saturating_sub(4);
    (format!(" {} ", ellipsize(name, room)), name.width() > room)
}

/// The detail panel right of the list.
pub fn draw_side(frame: &mut Frame, app: &App, area: Rect, now_ms: i64, th: Theme) {
    let subject = subject(app);
    let (name, border) = frame_of(&subject, th);
    // The whole name goes into the body when the title had to cut it.
    let (title, shortened) = title_of(&name, area.width);
    let block = panel(th, border)
        .title(Line::styled(title, th.bold(th.p.title)))
        .title_bottom(Line::styled(" selected ", th.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let width = usize::from(inner.width);
    // The last rows hold a rule and the keys of the selected line: one
    // row of keys, two when they need it and the panel has the rows.
    let max_keys = if inner.height > 4 { MAX_KEY_ROWS } else { 1 };
    let mut keys = if inner.height >= 4 {
        key_rows(app, &subject, width, max_keys, th)
    } else {
        Vec::new()
    };
    let body = |keys: usize| -> usize {
        let reserved = if keys == 0 { 2 } else { 1 + keys };
        usize::from(inner.height).saturating_sub(reserved)
    };
    let build = |rows: ValueRows| -> Vec<Lb> {
        match &subject {
            Subject::Session(m) => session_lines(
                app,
                m,
                width,
                rows,
                shortened.then_some(name.as_str()),
                now_ms,
                th,
            ),
            Subject::Group {
                key, count, lit, ..
            } => group_lines(app, key, *count, *lit, width, rows[0], th),
            Subject::Nothing => {
                let mut l = Lb::new();
                l.push(" nothing selected", th.muted());
                vec![l]
            }
        }
    };
    // The values take the free rows: each up to MAX_VALUE_ROWS when all
    // fit, else one row each and the rows left over to them in turn.
    let fitting = |body_h: usize| -> Option<Vec<Lb>> {
        let full = build(MAX_ROWS);
        if full.len() <= body_h {
            return Some(full);
        }
        let mut rows: ValueRows = [1; 5];
        let mut lines = build(rows);
        if lines.len() > body_h {
            return None;
        }
        for k in 0..rows.len() {
            while rows[k] < MAX_ROWS[k] {
                rows[k] += 1;
                let more = build(rows);
                if more.len() > body_h {
                    rows[k] -= 1;
                    break;
                }
                lines = more;
            }
        }
        Some(lines)
    };
    let lines = match fitting(body(keys.len())) {
        Some(lines) => lines,
        // A second row of keys gives its row back to the values first.
        None if keys.len() > 1 => {
            keys = key_rows(app, &subject, width, 1, th);
            fitting(body(1)).unwrap_or_else(|| build([1; 5]))
        }
        None => build([1; 5]),
    };
    let body_h = body(keys.len());
    for (k, l) in lines.into_iter().enumerate() {
        if k >= body_h {
            break;
        }
        put(frame, row(inner, k as u16), l.line());
    }
    if !keys.is_empty() {
        let first = inner.height - keys.len() as u16;
        let rule = "─".repeat(width);
        put(frame, row(inner, first - 1), Line::styled(rule, th.line()));
        for (k, l) in keys.into_iter().enumerate() {
            put(frame, row(inner, first + k as u16), l.line());
        }
    }
}

/// The detail strip under the list: three lines in two halves, and the
/// `git` line across both. Every value is cut with `…` to its half.
pub fn draw_strip(frame: &mut Frame, app: &App, area: Rect, now_ms: i64, th: Theme) {
    let subject = subject(app);
    let (name, border) = frame_of(&subject, th);
    let (title, _) = title_of(&name, area.width);
    let block = panel(th, border).title(Line::styled(title, th.bold(th.p.title)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let half = usize::from(inner.width) / 2;
    let pairs: Vec<(Lb, Lb)> = match &subject {
        Subject::Session(m) => {
            let (id, kind) = id_text(m);
            let state = m
                .session
                .state
                .as_ref()
                .map_or("-".to_owned(), ToString::to_string);
            let mut l2 = Lb::new();
            l2.push(
                format!(
                    " {state} · {} · changed {}",
                    // What it waits for is on the line above.
                    if groups::bg_task(&m.session) {
                        groups::BG_TASK_STATUS_SHORT.to_owned()
                    } else {
                        m.session
                            .status
                            .as_ref()
                            .map_or_else(|| status_text(m), ToString::to_string)
                    },
                    changed_text(app, m, now_ms)
                ),
                th.text(),
            );
            let mut l3 = Lb::new();
            l3.push(format!(" id {id} · {kind}"), th.muted());
            let folder = paths::shorten_home(&m.session.cwd, app.home());
            vec![
                (headline(app, m, th), kv("folder", folder, th.text(), th)),
                (l2, account_line(app, m, "account", th)),
                (l3, kv("hand-off", handoff_text(app, m), th.muted(), th)),
            ]
        }
        Subject::Group {
            key,
            count,
            lit,
            collapsed,
        } => {
            let mut lines = group_lines(app, key, *count, *lit, half, 1, th).into_iter();
            let head = lines.next().unwrap_or_default();
            let _blank = lines.next();
            let rest: Vec<Lb> = lines.collect();
            let mut fold = Lb::new();
            fold.push(
                if *collapsed {
                    " folded · Enter unfolds"
                } else {
                    " Enter folds"
                },
                th.muted(),
            );
            let mut rest = rest.into_iter();
            vec![
                (head, rest.next().unwrap_or_default()),
                (fold, rest.next().unwrap_or_default()),
                (Lb::new(), rest.next().unwrap_or_default()),
                (Lb::new(), rest.next().unwrap_or_default()),
            ]
        }
        Subject::Nothing => {
            let mut l = Lb::new();
            l.push(" nothing selected", th.muted());
            vec![(l, Lb::new())]
        }
    };
    // Each half keeps a one-column margin on its right.
    let right_w = usize::from(inner.width).saturating_sub(half + 1);
    for (k, (left, right)) in pairs.into_iter().enumerate() {
        if k as u16 >= inner.height {
            break;
        }
        // The left half is cut so the right half keeps its column.
        let mut l = fit_line(left, half.saturating_sub(1));
        l.pad_to(half, Style::new());
        l.append(fit_line(right, right_w));
        put(frame, row(inner, k as u16), l.line());
    }
    // The fourth line, across both halves: the folder's git summary
    // (doc/0037).
    if let Subject::Session(m) = &subject
        && inner.height >= 4
        && let Some(l) = git_line(app, m, usize::from(inner.width), th)
    {
        put(frame, row(inner, 3), l.line());
    }
}

#[cfg(test)]
mod tests {
    use super::{ellipsize, wrap_capped, wrap_value};

    #[test]
    fn wraps_after_a_slash_and_a_dash() {
        assert_eq!(
            wrap_value("~/workspace/billing-platform-frontend", 20),
            ["~/workspace/billing-", "platform-frontend"]
        );
        assert_eq!(
            wrap_value("feature/login-page", 10),
            ["feature/", "login-page"]
        );
    }

    #[test]
    fn wraps_at_spaces_and_drops_them() {
        assert_eq!(
            wrap_value("busy and still running now", 12),
            ["busy and", "still", "running now"]
        );
    }

    #[test]
    fn a_separator_stays_inside_a_row() {
        // Neither `retry ·` nor `· clean` alone: the break is in the branch.
        assert_eq!(
            wrap_value("feature/x-with-retry · clean", 20),
            ["feature/x-with-", "retry · clean"]
        );
    }

    #[test]
    fn a_part_longer_than_a_row_is_broken_where_the_row_ends() {
        assert_eq!(wrap_value("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_value("проектоплаты", 6), ["проект", "оплаты"]);
    }

    #[test]
    fn at_most_the_rows_given_the_last_ending_in_an_ellipsis() {
        let text = "aaaa-bbbb-cccc-dddd-eeee";
        assert_eq!(wrap_capped(text, 5, 3), ["aaaa-", "bbbb-", "cccc…"]);
        assert_eq!(wrap_capped(text, 5, 1), ["aaaa…"]);
        assert_eq!(wrap_capped("short", 5, 3), ["short"]);
    }

    #[test]
    fn ellipsize_cuts_only_what_does_not_fit() {
        assert_eq!(ellipsize("print-date", 10), "print-date");
        assert_eq!(ellipsize("print-date", 6), "print…");
        assert_eq!(ellipsize("print-date", 0), "");
    }
}
