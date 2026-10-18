//! `tower status [--json]`: one merged snapshot of every account's sessions,
//! without a terminal or the TUI (doc/0002, doc/0010).
//!
//! Every account is polled once, in parallel, with the same runner the TUI
//! uses (its timeout applies, and `CLAUDE_CONFIG_DIR` is removed from the
//! children, doc/0017). The listings are merged and owners resolved by the
//! same [`Resolver`], `/proc` evidence and pid-reuse guard included; every
//! process is a first sighting.
//!
//! The sidecar is only read, never written, moved aside or locked, so a
//! running TUI is never raced: a missing or unparsable file is used as an
//! empty one, with a warning on stderr.
//!
//! The JSON (doc/0010) is an object with `sessions` and `errors`. A failed
//! account poll goes to `errors` and does not fail the command:
//! [`exit_code`] is 0 when at least one account was listed, 1 when none
//! was, and `main` uses 2 for a config error.

use std::path::Path;

use serde::Serialize;

use crate::attach::printable;
use crate::cli::CliRunner;
use crate::config::{Account, Config};
use crate::groups::{self, Group};
use crate::listing::{Kind, Listing, Session, State, Status, WaitingFor};
use crate::merge::{AccountListing, EnvironReader, Resolver};
use crate::paths;
use crate::sidecar::{Sidecar, StateTuple};

/// Exit code when no account could be listed.
pub const EXIT_ALL_FAILED: u8 = 1;

/// Exit code for an unusable config (or no home directory).
pub const EXIT_CONFIG: u8 = 2;

/// One session of the snapshot (doc/0010). Absent values are `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: Option<String>,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub cwd: String,
    pub kind: Kind,
    pub state: Option<State>,
    pub status: Option<Status>,
    pub waiting_for: Option<WaitingFor>,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    /// The owning account's name, or `"?"` when unknown (doc/0003).
    pub account: String,
    /// The owning account's command, `null` when the owner is unknown.
    pub account_command: Option<String>,
    /// The overview group ([`group_slug`]).
    pub group: &'static str,
    /// The row's state detail, as the overview shows it ([`groups::detail`]).
    pub detail: String,
    /// Unix ms of the last state change ([`last_change_at`]).
    pub last_change_at: Option<i64>,
}

/// An account whose listing failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountError {
    pub account: String,
    pub account_command: String,
    pub error: String,
}

/// The whole `tower status --json` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub sessions: Vec<Row>,
    pub errors: Vec<AccountError>,
}

/// A machine-readable name for an overview group (doc/0006).
pub fn group_slug(group: Group) -> &'static str {
    match group {
        Group::NeedsMe => "needs-me",
        Group::Failed => "failed",
        Group::ReadyForReview => "ready-for-review",
        Group::Working => "working",
        Group::Other => "other",
        Group::Stopped => "stopped",
        Group::Interactive => "interactive",
    }
}

/// When the session's state last changed, read-only by the rules of
/// doc/0006: the sidecar's `last_change_ms` while its `last_state` is the
/// row's current tuple; `now_ms` when the tuple has changed since (the
/// change is observed by this poll); `startedAt` when tower tracks no time
/// for the session.
pub fn last_change_at(session: &Session, sidecar: &Sidecar, now_ms: i64) -> Option<i64> {
    let tracked = session
        .key()
        .and_then(|k| sidecar.sessions.get(k))
        .and_then(|r| Some((r.last_state.as_ref()?, r.last_change_ms?)));
    match tracked {
        Some((tuple, ms)) if *tuple == current_tuple(session) => Some(ms),
        Some(_) => Some(now_ms),
        None => session.started_at,
    }
}

fn current_tuple(s: &Session) -> StateTuple {
    StateTuple {
        state: s.state.clone(),
        status: s.status.clone(),
        waiting_for: s.waiting_for.clone(),
    }
}

/// Lists every account once, each on its own thread, and returns the
/// results in config order. A listing call that panics, or whose thread
/// cannot be started, is that account's error.
pub fn poll_all(accounts: &[Account], runner: &CliRunner) -> Vec<Result<Listing, String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = accounts
            .iter()
            .map(|account| {
                std::thread::Builder::new()
                    .name(format!("status-{}", account.name))
                    .spawn_scoped(scope, move || {
                        runner.list_sessions(account).map_err(|e| e.to_string())
                    })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| match h {
                Ok(h) => h
                    .join()
                    .unwrap_or_else(|_| Err("the listing worker panicked".to_owned())),
                Err(e) => Err(format!("cannot start the listing worker: {e}")),
            })
            .collect()
    })
}

/// Builds the report from one poll result per configured account (in
/// config order). Also returns warnings for stderr: rows the parser
/// skipped.
pub fn build(
    config: &Config,
    home: &Path,
    results: Vec<Result<Listing, String>>,
    sidecar: &Sidecar,
    environ: &dyn EnvironReader,
    now_ms: i64,
) -> (Report, Vec<String>) {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    let mut listings = Vec::new();
    for (account, result) in config.accounts.iter().zip(results) {
        match result {
            Ok(listing) => {
                for w in &listing.warnings {
                    warnings.push(format!("account `{}`: {w}", account.name));
                }
                listings.push(
                    AccountListing::fresh(&account.name, listing.sessions)
                        .with_partial(!listing.warnings.is_empty()),
                );
            }
            Err(error) => {
                errors.push(AccountError {
                    account: account.name.clone(),
                    account_command: account.command.clone(),
                    error,
                });
                listings.push(AccountListing::missing(&account.name));
            }
        }
    }
    let mut resolver = Resolver::new(&config.accounts, home, environ);
    let merged = resolver.merge(&listings, sidecar);
    let sessions = merged
        .into_iter()
        .map(|m| {
            let account_command = m
                .owner
                .account()
                .and_then(|name| config.account(name))
                .map(|a| a.command.clone());
            let s = &m.session;
            Row {
                id: s.id.clone(),
                session_id: s.session_id.clone(),
                name: s.name.clone(),
                cwd: s.cwd.clone(),
                kind: s.kind.clone(),
                state: s.state.clone(),
                status: s.status.clone(),
                waiting_for: s.waiting_for.clone(),
                pid: s.pid,
                started_at: s.started_at,
                account: m.owner.label().to_owned(),
                account_command,
                group: group_slug(groups::classify(s)),
                detail: groups::detail(s),
                last_change_at: last_change_at(s, sidecar, now_ms),
            }
        })
        .collect();
    (Report { sessions, errors }, warnings)
}

/// Reads the sidecar without changing anything on disk: a missing file is
/// an empty sidecar; an unreadable or corrupt one is used as empty too,
/// with a warning, and left where it is (the TUI moves it aside).
pub fn read_sidecar(path: &Path) -> (Sidecar, Option<String>) {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Sidecar>(&bytes) {
            Ok(sidecar) => (sidecar, None),
            Err(err) => (
                Sidecar::default(),
                Some(format!(
                    "state file {} is corrupt ({err}); ignoring it",
                    path.display()
                )),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Sidecar::default(), None),
        Err(err) => (
            Sidecar::default(),
            Some(format!(
                "cannot read state file {} ({err}); ignoring it",
                path.display()
            )),
        ),
    }
}

/// 0 when at least one account was listed, else [`EXIT_ALL_FAILED`].
pub fn exit_code(config: &Config, report: &Report) -> u8 {
    if report.errors.len() < config.accounts.len() {
        0
    } else {
        EXIT_ALL_FAILED
    }
}

/// The report as pretty-printed JSON with a trailing newline. Strings are
/// not sanitised: serde escapes control characters.
pub fn to_json(report: &Report) -> String {
    let mut text = serde_json::to_string_pretty(report).expect("the report always serialises");
    text.push('\n');
    text
}

/// A short table for people: name, detail, account and folder, every cell
/// made printable (doc/0007). Errors are not included (they go to stderr).
pub fn to_table(report: &Report, home: &Path) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut rows: Vec<[String; 4]> = vec![[
        "NAME".to_owned(),
        "STATE".to_owned(),
        "ACCOUNT".to_owned(),
        "FOLDER".to_owned(),
    ]];
    for r in &report.sessions {
        let name = r
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| r.id.clone())
            .or_else(|| r.session_id.as_ref().map(|s| s.chars().take(8).collect()))
            .unwrap_or_else(|| r.pid.map_or("(unnamed)".to_owned(), |p| format!("pid {p}")));
        rows.push([
            printable(&name),
            printable(&r.detail),
            printable(&r.account),
            printable(&paths::shorten_home(&r.cwd, home)),
        ]);
    }
    let widths: Vec<usize> = (0..3)
        .map(|i| rows.iter().map(|r| r[i].width()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for r in &rows {
        for (i, w) in widths.iter().enumerate() {
            out.push_str(&r[i]);
            out.push_str(&" ".repeat(w - r[i].width() + 2));
        }
        out.push_str(&r[3]);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_slugs_are_distinct() {
        let mut slugs: Vec<&str> = Group::ALL.iter().map(|g| group_slug(*g)).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), Group::ALL.len());
    }

    #[test]
    fn last_change_follows_doc_0006() {
        let session = Session {
            session_id: Some("s1".into()),
            started_at: Some(10),
            state: Some(State::Working),
            status: Some(Status::Busy),
            ..Session::default()
        };
        let mut sidecar = Sidecar::default();
        assert_eq!(last_change_at(&session, &sidecar, 99), Some(10));
        let rec = sidecar.sessions.entry("s1".into()).or_default();
        rec.last_state = Some(current_tuple(&session));
        rec.last_change_ms = Some(50);
        assert_eq!(last_change_at(&session, &sidecar, 99), Some(50));
        let done = Session {
            state: Some(State::Done),
            status: Some(Status::Idle),
            ..session.clone()
        };
        assert_eq!(last_change_at(&done, &sidecar, 99), Some(99));
    }
}
