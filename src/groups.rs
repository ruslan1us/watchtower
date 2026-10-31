//! Overview groups and per-row detail labels (doc/0006).

use std::path::Path;

use crate::listing::{Session, State, Status};
use crate::orchestrator;
use crate::paths;

/// The overview groups, in display order (doc/0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// Background rows named exactly `orchestrator` (doc/0010), whatever
    /// their state: pinned at the top and never highlighted (doc/0007).
    Orchestrator,
    /// Sessions the user pinned (doc/0047), whatever their state. Only a list
    /// grouped by state has it; [`classify`] never returns it, so a pinned row
    /// keeps its state's detail, colour and attention.
    Pinned,
    /// `state: blocked`, or `status: waiting` in any state.
    NeedsMe,
    /// `state: failed`.
    Failed,
    /// `state: done`: turn finished, waiting for the next prompt.
    ReadyForReview,
    /// `state: working`.
    Working,
    /// A `state` value tower does not know (or none on a background row).
    Other,
    /// Background rows with no live process, whatever their `state`, and
    /// `state: stopped`.
    Stopped,
    /// `kind: interactive`: not attachable.
    Interactive,
}

impl Group {
    /// All groups in display order.
    pub const ALL: [Group; 9] = [
        Group::Orchestrator,
        Group::Pinned,
        Group::NeedsMe,
        Group::Failed,
        Group::ReadyForReview,
        Group::Working,
        Group::Other,
        Group::Stopped,
        Group::Interactive,
    ];

    /// Header title.
    pub fn title(self) -> &'static str {
        match self {
            Group::Orchestrator => "Orchestrator",
            Group::Pinned => "Pinned",
            Group::NeedsMe => "Needs me",
            Group::Failed => "Failed",
            Group::ReadyForReview => "Ready for review",
            Group::Working => "Working",
            Group::Other => "Other",
            Group::Stopped => "Stopped & finished",
            Group::Interactive => "Interactive",
        }
    }

    /// Whether the group starts expanded. Stopped & finished starts collapsed;
    /// Interactive is shown and toggled separately (doc/0006).
    pub fn default_expanded(self) -> bool {
        self != Group::Stopped
    }
}

/// Maps a row to its group (doc/0006). The orchestrator rows
/// ([`orchestrator::is_orchestrator`]) go to Orchestrator whatever their
/// state; every other row to its [`state_group`].
pub fn classify(s: &Session) -> Group {
    if orchestrator::is_orchestrator(s) {
        return Group::Orchestrator;
    }
    state_group(s)
}

/// The group a row's state alone gives it, never Orchestrator (doc/0006).
/// Interactive rows go to Interactive. For other rows the no-live-process
/// check comes first: no `pid` and no `status` means Stopped & finished
/// even when `state` is `done` or `failed`.
pub fn state_group(s: &Session) -> Group {
    if s.is_interactive() {
        return Group::Interactive;
    }
    if !s.has_live_process() {
        return Group::Stopped;
    }
    if s.state == Some(State::Blocked) || s.status == Some(Status::Waiting) {
        return Group::NeedsMe;
    }
    match &s.state {
        Some(State::Failed) => Group::Failed,
        Some(State::Done) => Group::ReadyForReview,
        Some(State::Working) => Group::Working,
        Some(State::Stopped) => Group::Stopped,
        Some(State::Blocked) => Group::NeedsMe,
        Some(State::Unknown(_)) | None => Group::Other,
    }
}

/// The detail of a Needs me row without `waitingFor`: what Claude Code
/// reports (`state: blocked`, `status: idle`) when a turn ended with a
/// question to the user (doc/0006).
pub const WAITING_FOR_REPLY: &str = "waiting for your reply";

/// The orchestrator's detail while it waits for the user: its turn ended,
/// with a question or without (doc/0010).
pub const READY: &str = "ready";

/// The state detail shown in a row (doc/0006): `waitingFor` text or
/// [`WAITING_FOR_REPLY`] for Needs me, "failed", "done", "working" or
/// "idle" for Working, the raw `state` for Other, "stopped", and the `status`
/// for interactive rows. Unknown values are shown as-is. An orchestrator
/// shows the detail of its [`state_group`], except [`READY`] where it waits
/// for the user's next message.
pub fn detail(s: &Session) -> String {
    match classify(s) {
        Group::Orchestrator => match state_group(s) {
            Group::NeedsMe if s.waiting_for.is_none() => READY.to_owned(),
            Group::ReadyForReview => READY.to_owned(),
            group => state_detail(s, group),
        },
        group => state_detail(s, group),
    }
}

/// What the STATE column appends to the detail of a [`bg_task`] row
/// (doc/0032): `done · bg task`, the orchestrator's `ready · bg task`.
pub const BG_TASK_SUFFIX: &str = " · bg task";

/// The dashboard detail panel's status of a [`bg_task`] row (doc/0032).
pub const BG_TASK_STATUS: &str = "busy — a background task (e.g. a shell) is still running; \
     Claude Code may continue this session when it ends";

/// The compact form of [`BG_TASK_STATUS`], for the detail strip
/// (doc/0032).
pub const BG_TASK_STATUS_SHORT: &str = "busy (bg task)";

/// Whether the row's busy spinner is drawn (doc/0032): a live background
/// row with `state: working` and `status: busy`.
pub fn spins(s: &Session) -> bool {
    !s.is_interactive()
        && s.has_live_process()
        && s.state == Some(State::Working)
        && s.status == Some(Status::Busy)
}

/// Whether a background task (e.g. a shell) outlived the session's turn
/// (doc/0032): a live background row with `status: busy` in any `state`
/// but `working`, e.g. `done` + `busy`. It keeps its state's group and
/// glyph; the STATE column appends [`BG_TASK_SUFFIX`].
pub fn bg_task(s: &Session) -> bool {
    !s.is_interactive()
        && s.has_live_process()
        && s.state != Some(State::Working)
        && s.status == Some(Status::Busy)
}

/// The detail of a row in the state group `group`.
fn state_detail(s: &Session, group: Group) -> String {
    match group {
        Group::NeedsMe => match &s.waiting_for {
            Some(w) => w.to_string(),
            None => WAITING_FOR_REPLY.to_owned(),
        },
        Group::Failed => "failed".to_owned(),
        Group::ReadyForReview => "done".to_owned(),
        Group::Working => match &s.status {
            Some(Status::Busy) | None => "working".to_owned(),
            Some(other) => other.to_string(),
        },
        Group::Other => match &s.state {
            Some(state) => state.to_string(),
            None => "unknown".to_owned(),
        },
        Group::Stopped => "stopped".to_owned(),
        Group::Interactive => match &s.status {
            Some(status) => status.to_string(),
            None => "interactive".to_owned(),
        },
        // Not state groups; `state_group` never returns them.
        Group::Orchestrator => "orchestrator".to_owned(),
        Group::Pinned => "pinned".to_owned(),
    }
}

/// Rows of one non-empty group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRows<'a, T> {
    pub group: Group,
    pub rows: Vec<&'a T>,
}

/// Groups rows by state for the overview: non-empty groups only, in display
/// order (doc/0006), each sorted by `startedAt` as the change time. Tower's
/// own list uses [`group_by_state_with`] and the tracked times.
pub fn group_by_state<T: AsRef<Session>>(items: &[T]) -> Vec<GroupRows<'_, T>> {
    group_by_state_with(items, |t| t.as_ref().started_at)
}

/// Groups rows by state for the overview: non-empty groups only, in display
/// order (doc/0006). Within a group rows are sorted by last change, most
/// recent first (a row without a time last), then by display name; the
/// Orchestrator group instead in the order `o` picks from
/// ([`orchestrator::rank`]). `last_change` gives a row's change time, Unix ms (see
/// [`crate::activity::last_change_at`]).
pub fn group_by_state_with<T, F>(items: &[T], last_change: F) -> Vec<GroupRows<'_, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
{
    group_by_state_pinned(items, last_change, |_| false)
}

/// [`group_by_state_with`] with the rows `pinned` names in the Pinned group,
/// right after Orchestrator (doc/0047). The orchestrator and interactive rows
/// keep their own groups: neither can be pinned.
pub fn group_by_state_pinned<T, F, P>(
    items: &[T],
    last_change: F,
    pinned: P,
) -> Vec<GroupRows<'_, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
    P: Fn(&T) -> bool,
{
    let listed_in = |t: &T| match classify(t.as_ref()) {
        Group::Orchestrator => Group::Orchestrator,
        Group::Interactive => Group::Interactive,
        _ if pinned(t) => Group::Pinned,
        group => group,
    };
    let mut out: Vec<GroupRows<'_, T>> = Vec::new();
    for group in Group::ALL {
        let mut rows: Vec<(Option<i64>, String, &T)> = items
            .iter()
            .filter(|t| listed_in(t) == group)
            .map(|t| (last_change(t), t.as_ref().display_name(), t))
            .collect();
        if rows.is_empty() {
            continue;
        }
        if group == Group::Orchestrator {
            rows.sort_by_key(|r| std::cmp::Reverse(orchestrator::rank(r.2.as_ref())));
        } else {
            rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        }
        out.push(GroupRows {
            group,
            rows: rows.into_iter().map(|(_, _, t)| t).collect(),
        });
    }
    out
}

/// How the overview groups its rows (doc/0006): by state, the default, or
/// by project (`p`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// The state groups of the table in doc/0006.
    #[default]
    State,
    /// One group per project folder; interactive rows keep their own group.
    Project,
}

/// The identity of one group in the overview: a state group, or a project
/// in project mode, named by its `cwd` with the home directory shortened
/// to `~`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GroupKey {
    /// A state group; in project mode only [`Group::Orchestrator`] and
    /// [`Group::Interactive`].
    State(Group),
    /// A project folder in project mode, e.g. `~/workspace/api`.
    Project(String),
}

/// Header title of a project group whose rows have no `cwd`.
pub const NO_PROJECT: &str = "(no folder)";

impl GroupKey {
    /// Header title.
    pub fn title(&self) -> &str {
        match self {
            GroupKey::State(g) => g.title(),
            GroupKey::Project(p) if p.is_empty() => NO_PROJECT,
            GroupKey::Project(p) => p,
        }
    }

    /// Whether the group starts expanded: Stopped & finished starts
    /// collapsed, every other group (and every project) expanded (doc/0006).
    pub fn default_expanded(&self) -> bool {
        match self {
            GroupKey::State(g) => g.default_expanded(),
            GroupKey::Project(_) => true,
        }
    }

    /// Whether this is the Interactive group (dimmed, toggled with `i`).
    pub fn is_interactive(&self) -> bool {
        *self == GroupKey::State(Group::Interactive)
    }
}

/// Rows of one non-empty group of either grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedRows<'a, T> {
    pub key: GroupKey,
    pub rows: Vec<&'a T>,
}

/// Groups rows for the overview in the given mode (doc/0006); see
/// [`group_by_state_with`] and [`group_by_project_with`].
pub fn group_with<'a, T, F>(
    mode: Grouping,
    items: &'a [T],
    home: &Path,
    last_change: F,
) -> Vec<KeyedRows<'a, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
{
    group_pinned(mode, items, home, last_change, |_| false)
}

/// [`group_with`] with the rows `pinned` names first: in their own group by
/// state, first within their project by project (doc/0047).
pub fn group_pinned<'a, T, F, P>(
    mode: Grouping,
    items: &'a [T],
    home: &Path,
    last_change: F,
    pinned: P,
) -> Vec<KeyedRows<'a, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
    P: Fn(&T) -> bool,
{
    match mode {
        Grouping::State => group_by_state_pinned(items, last_change, pinned)
            .into_iter()
            .map(|g| KeyedRows {
                key: GroupKey::State(g.group),
                rows: g.rows,
            })
            .collect(),
        Grouping::Project => group_by_project_pinned(items, home, last_change, pinned),
    }
}

/// Groups rows by project (doc/0006): one group per `cwd`, named with the
/// home directory shortened to `~`. Groups are ordered by their most
/// recent change, newest first (a group without any time last), then by
/// name. Inside a group rows follow the state order of the state groups,
/// then the last change, most recent first, then the display name.
/// The orchestrator rows keep their own Orchestrator group, first, ordered
/// as in [`group_by_state_with`]; interactive rows keep their own
/// Interactive group, last.
pub fn group_by_project_with<'a, T, F>(
    items: &'a [T],
    home: &Path,
    last_change: F,
) -> Vec<KeyedRows<'a, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
{
    group_by_project_pinned(items, home, last_change, |_| false)
}

/// [`group_by_project_with`] with the rows `pinned` names first within their
/// project (doc/0047).
pub fn group_by_project_pinned<'a, T, F, P>(
    items: &'a [T],
    home: &Path,
    last_change: F,
    pinned: P,
) -> Vec<KeyedRows<'a, T>>
where
    T: AsRef<Session>,
    F: Fn(&T) -> Option<i64>,
    P: Fn(&T) -> bool,
{
    // A pinned row sorts before an unpinned one: `false` comes first.
    type Row<'r, T> = (bool, Group, Option<i64>, String, &'r T);
    let mut projects: Vec<(String, Vec<Row<'a, T>>)> = Vec::new();
    let mut interactive: Vec<Row<'a, T>> = Vec::new();
    let mut orchestrators: Vec<&'a T> = Vec::new();
    for t in items {
        let s = t.as_ref();
        if orchestrator::is_orchestrator(s) {
            orchestrators.push(t);
            continue;
        }
        let row = (!pinned(t), classify(s), last_change(t), s.display_name(), t);
        if s.is_interactive() {
            interactive.push(row);
            continue;
        }
        let project = paths::shorten_home(&s.cwd, home);
        match projects.iter_mut().find(|(p, _)| *p == project) {
            Some((_, rows)) => rows.push(row),
            None => projects.push((project, vec![row])),
        }
    }
    let by_state = |a: &Row<'a, T>, b: &Row<'a, T>| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    };
    for (_, rows) in &mut projects {
        rows.sort_by(by_state);
    }
    let newest = |rows: &[Row<'a, T>]| rows.iter().filter_map(|r| r.2).max();
    projects.sort_by(|a, b| newest(&b.1).cmp(&newest(&a.1)).then(a.0.cmp(&b.0)));
    interactive.sort_by(by_state);
    orchestrators.sort_by_key(|t| std::cmp::Reverse(orchestrator::rank(t.as_ref())));
    let mut out: Vec<KeyedRows<'a, T>> = Vec::new();
    if !orchestrators.is_empty() {
        out.push(KeyedRows {
            key: GroupKey::State(Group::Orchestrator),
            rows: orchestrators,
        });
    }
    out.extend(projects.into_iter().map(|(p, rows)| KeyedRows {
        key: GroupKey::Project(p),
        rows: rows.into_iter().map(|r| r.4).collect(),
    }));
    if !interactive.is_empty() {
        out.push(KeyedRows {
            key: GroupKey::State(Group::Interactive),
            rows: interactive.into_iter().map(|r| r.4).collect(),
        });
    }
    out
}

/// Whether a row matches the text filter (doc/0006): every
/// whitespace-separated word of `query` occurs, ignoring case, in at least
/// one of `fields` (name, project, account). An empty query matches all.
pub fn filter_matches(query: &str, fields: &[&str]) -> bool {
    let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|word| fields.iter().any(|f| f.contains(&word)))
}

impl AsRef<Session> for Session {
    fn as_ref(&self) -> &Session {
        self
    }
}
