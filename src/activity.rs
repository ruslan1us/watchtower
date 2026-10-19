//! Activity times tower tracks itself (doc/0006) and the attention
//! highlight (doc/0007), shared by the TUI and `tower status`.
//!
//! The TUI records each fresh listing's (`state`, `status`, `waitingFor`)
//! tuple in the sidecar ([`Sidecar::observe_state_at`]); everything here
//! only reads it, so `tower status --json` reports the same `lastChangeAt`
//! the TUI shows.

use crate::groups::{self, Group};
use crate::listing::Session;
use crate::sidecar::{Sidecar, StateTuple};

/// Below this age the age column says `now`.
pub const NOW_MS: i64 = 10_000;

/// When the session's state last changed (doc/0006): the sidecar's
/// `last_change_ms` while its `last_state` is the row's current tuple;
/// `now_ms` when the tuple has changed since (the change is observed by
/// this poll); `startedAt` when tower tracks no time for the session.
pub fn last_change_at(session: &Session, sidecar: &Sidecar, now_ms: i64) -> Option<i64> {
    let tracked = session
        .key()
        .and_then(|k| sidecar.sessions.get(k))
        .and_then(|r| Some((r.last_state.as_ref()?, r.last_change_ms?)));
    match tracked {
        Some((tuple, ms)) if *tuple == StateTuple::of(session) => Some(ms),
        Some(_) => Some(now_ms),
        None => session.started_at,
    }
}

/// Whether the row is in a state that wants the user: the Needs me or
/// Failed group (`state: blocked`, `status: waiting`, `state: failed` with a
/// live process). Interactive rows never are, and neither is the
/// orchestrator, which has its own group: waiting for the user is its
/// normal state (doc/0007).
pub fn needs_attention(session: &Session) -> bool {
    !session.is_interactive() && matches!(groups::classify(session), Group::NeedsMe | Group::Failed)
}

/// Whether the row is highlighted (doc/0007): it needs attention and the
/// user has not acknowledged its current tuple. The sidecar drops an
/// acknowledgement when the session leaves the acknowledged tuple, so a
/// new transition into an attention state highlights the row again.
pub fn highlighted(session: &Session, sidecar: &Sidecar) -> bool {
    needs_attention(session)
        && session
            .key()
            .and_then(|k| sidecar.sessions.get(k))
            .and_then(|r| r.acknowledged.as_ref())
            != Some(&StateTuple::of(session))
}

/// The compact age of a change `ms` milliseconds ago: `now` below
/// [`NOW_MS`] (and for a time in the future), then `42s`, `5m`, `3h`, `2d`,
/// rounded down.
pub fn age_label(ms: i64) -> String {
    if ms < NOW_MS {
        return "now".to_owned();
    }
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_labels() {
        assert_eq!(age_label(-5_000), "now");
        assert_eq!(age_label(0), "now");
        assert_eq!(age_label(9_999), "now");
        assert_eq!(age_label(42_000), "42s");
        assert_eq!(age_label(5 * 60_000 + 59_000), "5m");
        assert_eq!(age_label(3 * 3_600_000), "3h");
        assert_eq!(age_label(2 * 86_400_000 + 1), "2d");
    }
}
