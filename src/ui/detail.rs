//! The dashboard's detail panel (doc/0020): what tower knows about the
//! selected session, or a summary of the selected group. A side panel on
//! wide terminals, a three-line strip under the list on narrower ones.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::dashboard::{account_chip, panel, put, row, spinner};
use super::theme::Lb;
use super::theme::{BASE, BLUE, LINE, MUTED, TEXT, Theme, group_color, key_color};
use super::{MARKER, age_column};
use crate::app::{App, Entry};
use crate::config::Account;
use crate::groups::{self, Group, GroupKey};
use crate::handoff;
use crate::listing::Status;
use crate::merge::MergedSession;
use crate::paths;

/// Label column width of the key/value lines.
const KEY_W: usize = 10;

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
    let gc = group_color(groups::classify(&m.session));
    let mut l = Lb::new();
    if app.is_highlighted(m) {
        l.push(format!(" {MARKER} needs you: {detail}"), th.attention());
    } else {
        l.push(" ", Style::new());
        l.push(format!(" {detail} "), th.bold(BASE).patch(th.on(BASE, gc)));
    }
    l
}

/// `status`, with `waitingFor` when waiting; "no live process" for
/// stopped rows (doc/0006).
fn status_text(m: &MergedSession) -> String {
    let s = &m.session;
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

/// The keys that apply to the selected line.
fn keys(app: &App, subject: &Subject) -> Vec<(&'static str, &'static str)> {
    match subject {
        Subject::Session(m) if !m.session.is_attachable() => Vec::new(),
        Subject::Session(m) => {
            let mut k = vec![("Enter", "attach"), ("Space", "logs")];
            if app.is_highlighted(m) {
                k.push(("m", "seen"));
            }
            if matches!(
                handoff::targets(app.config(), owner(app, m), ""),
                Ok(t) if !t.is_empty()
            ) {
                k.push(("a", "hand-off"));
            }
            k.push(("s", "stop"));
            k
        }
        Subject::Group { collapsed, .. } => {
            vec![("Enter", if *collapsed { "unfold" } else { "fold" })]
        }
        Subject::Nothing => vec![("n", "new")],
    }
}

/// The sessions of a list group that the view lets through.
fn members<'a>(app: &'a App, key: &GroupKey) -> Vec<&'a MergedSession> {
    app.sessions()
        .iter()
        .filter(|m| match key {
            GroupKey::State(g) => groups::classify(&m.session) == *g,
            GroupKey::Project(p) => {
                let g = groups::classify(&m.session);
                g != Group::Orchestrator
                    && g != Group::Interactive
                    && paths::shorten_home(&m.session.cwd, app.home()) == *p
            }
        })
        .filter(|m| app.matches_filter(m))
        .collect()
}

/// A summary of a group: sessions, highlighted, accounts, states.
fn group_lines(app: &App, key: &GroupKey, count: usize, lit: usize, th: Theme) -> Vec<Lb> {
    let rows = members(app, key);
    let mut lines = vec![{
        let mut l = Lb::new();
        l.push(" ", Style::new());
        l.push(
            format!(" {} ", key.title()),
            th.bold(BASE).patch(th.on(BASE, key_color(key))),
        );
        l
    }];
    lines.push(Lb::new());
    let noun = if count == 1 { "session" } else { "sessions" };
    lines.push(kv("sessions", format!("{count} {noun}"), th.fg(TEXT), th));
    if lit > 0 {
        lines.push(kv(
            "attention",
            format!("{MARKER} {lit}"),
            th.attention(),
            th,
        ));
    }
    let mut accounts: Vec<(String, usize)> = Vec::new();
    for m in &rows {
        let name = m.owner.label().to_owned();
        match accounts.iter_mut().find(|(a, _)| *a == name) {
            Some((_, n)) => *n += 1,
            None => accounts.push((name, 1)),
        }
    }
    if !accounts.is_empty() {
        let text: Vec<String> = accounts.iter().map(|(a, n)| format!("{a} {n}")).collect();
        lines.push(kv("accounts", text.join(" · "), th.fg(TEXT), th));
    }
    if matches!(key, GroupKey::Project(_)) {
        let mut states: Vec<(Group, usize)> = Vec::new();
        for m in &rows {
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
        lines.push(kv("states", text.join(" · "), th.fg(TEXT), th));
    }
    lines
}

/// The side panel's lines for a session.
fn session_lines(app: &App, m: &MergedSession, now_ms: i64, th: Theme) -> Vec<Lb> {
    let s = &m.session;
    let group = groups::classify(s);
    let (id, kind) = id_text(m);
    let state = s.state.as_ref().map_or("-".to_owned(), ToString::to_string);
    let mut status = status_text(m);
    if s.status == Some(Status::Busy) && !s.is_interactive() {
        status = format!("{} {status}", spinner(now_ms));
    }
    let text = th.fg(TEXT);
    vec![
        headline(app, m, th),
        Lb::new(),
        kv("group", group.title(), th.bold(group_color(group)), th),
        kv("state", state, text, th),
        kv("status", status, text, th),
        kv("changed", changed_text(app, m, now_ms), text, th),
        Lb::new(),
        kv("folder", paths::shorten_home(&s.cwd, app.home()), text, th),
        account_line(app, m, "account", th),
        kv("id", id, text, th),
        kv("kind", kind, text, th),
        Lb::new(),
        kv("hand-off", handoff_text(app, m), th.muted(), th),
    ]
}

/// The panel's title and border colour.
fn frame_of(subject: &Subject) -> (String, ratatui::style::Color) {
    match subject {
        Subject::Session(m) => (
            format!(" {} ", m.session.display_name()),
            group_color(groups::classify(&m.session)),
        ),
        Subject::Group { key, .. } => (format!(" {} ", key.title()), key_color(key)),
        Subject::Nothing => (" details ".to_owned(), LINE),
    }
}

/// The detail panel right of the list.
pub fn draw_side(frame: &mut Frame, app: &App, area: Rect, now_ms: i64, th: Theme) {
    let subject = subject(app);
    let (title, border) = frame_of(&subject);
    let block = panel(th, border)
        .title(Line::styled(title, th.bold(TEXT)))
        .title_bottom(Line::styled(" selected ", th.muted()).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = match &subject {
        Subject::Session(m) => session_lines(app, m, now_ms, th),
        Subject::Group {
            key, count, lit, ..
        } => group_lines(app, key, *count, *lit, th),
        Subject::Nothing => {
            let mut l = Lb::new();
            l.push(" nothing selected", th.muted());
            vec![l]
        }
    };
    // The last two rows hold a rule and the keys of the selected line.
    let body_h = inner.height.saturating_sub(2);
    for (k, l) in lines.into_iter().enumerate() {
        if k as u16 >= body_h {
            break;
        }
        put(frame, row(inner, k as u16), l.line());
    }
    if inner.height >= 4 {
        let rule = "─".repeat(usize::from(inner.width));
        put(
            frame,
            row(inner, inner.height - 2),
            Line::styled(rule, th.line()),
        );
        let mut l = Lb::new();
        l.push(" ", th.muted());
        for (key, what) in keys(app, &subject) {
            let item = key.len() + what.len() + 3;
            if l.w + item > usize::from(inner.width) {
                break;
            }
            l.push(key, th.bold(BLUE))
                .push(format!(" {what}  "), th.muted());
        }
        if !matches!(subject, Subject::Nothing) && l.w == 1 {
            l.push("not attachable", th.muted());
        }
        put(frame, row(inner, inner.height - 1), l.line());
    }
}

/// The detail strip under the list: three lines in two halves.
pub fn draw_strip(frame: &mut Frame, app: &App, area: Rect, now_ms: i64, th: Theme) {
    let subject = subject(app);
    let (title, border) = frame_of(&subject);
    let block = panel(th, border).title(Line::styled(title, th.bold(TEXT)));
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
                    m.session
                        .status
                        .as_ref()
                        .map_or_else(|| status_text(m), ToString::to_string),
                    changed_text(app, m, now_ms)
                ),
                th.fg(TEXT),
            );
            let mut l3 = Lb::new();
            l3.push(format!(" id {id} · {kind}"), th.fg(MUTED));
            let folder = paths::shorten_home(&m.session.cwd, app.home());
            vec![
                (headline(app, m, th), kv("folder", folder, th.fg(TEXT), th)),
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
            let mut lines = group_lines(app, key, *count, *lit, th).into_iter();
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
            ]
        }
        Subject::Nothing => {
            let mut l = Lb::new();
            l.push(" nothing selected", th.muted());
            vec![(l, Lb::new())]
        }
    };
    for (k, (mut left, right)) in pairs.into_iter().enumerate() {
        if k as u16 >= inner.height {
            break;
        }
        // The left half is cut so the right half keeps its column.
        let mut cut = Lb::new();
        for span in left.spans.drain(..) {
            let room = half.saturating_sub(cut.w + 1);
            let text = span.content.into_owned();
            if text.width() <= room {
                cut.push(text, span.style);
            } else {
                cut.push(super::fit(&text, room), span.style);
                break;
            }
        }
        cut.pad_to(half, Style::new());
        cut.append(right);
        put(frame, row(inner, k as u16), cut.line());
    }
}
