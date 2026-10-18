//! Overview groups and per-row detail labels (doc/0006).

use crate::listing::{Session, State, Status};

/// The overview groups, in display order (doc/0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
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
    pub const ALL: [Group; 7] = [
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

/// Maps a row to its group (doc/0006). Interactive rows go to Interactive.
/// For other rows the no-live-process check comes first: no `pid` and no
/// `status` means Stopped & finished even when `state` is `done` or `failed`.
pub fn classify(s: &Session) -> Group {
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

/// The state detail shown in a row (doc/0006): `waitingFor` text or
/// "blocked (no reason given)" for Needs me, "failed", "done", "working" or
/// "idle" for Working, the raw `state` for Other, "stopped", and the `status`
/// for interactive rows. Unknown values are shown as-is.
pub fn detail(s: &Session) -> String {
    match classify(s) {
        Group::NeedsMe => match &s.waiting_for {
            Some(w) => w.to_string(),
            None => "blocked (no reason given)".to_owned(),
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
    }
}

/// Rows of one non-empty group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRows<'a, T> {
    pub group: Group,
    pub rows: Vec<&'a T>,
}

/// Groups rows by state for the overview: non-empty groups only, in display
/// order (doc/0006).
///
/// Within a group rows are sorted by `startedAt`, most recent first, then by
/// display name. doc/0006 sorts by last change, which needs the activity
/// times tower tracks itself (milestone 5); `startedAt` is the stand-in until
/// then, and is reset whenever a session is woken.
pub fn group_by_state<T: AsRef<Session>>(items: &[T]) -> Vec<GroupRows<'_, T>> {
    let mut out: Vec<GroupRows<'_, T>> = Vec::new();
    for group in Group::ALL {
        let mut rows: Vec<&T> = items
            .iter()
            .filter(|t| classify(t.as_ref()) == group)
            .collect();
        if rows.is_empty() {
            continue;
        }
        rows.sort_by(|a, b| {
            let (a, b) = (a.as_ref(), b.as_ref());
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.display_name().cmp(&b.display_name()))
        });
        out.push(GroupRows { group, rows });
    }
    out
}

impl AsRef<Session> for Session {
    fn as_ref(&self) -> &Session {
        self
    }
}
