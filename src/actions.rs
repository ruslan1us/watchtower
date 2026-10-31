//! Stopping and removing a session (doc/0009).
//!
//! Both run with the command of the owning account (doc/0003) and take the
//! short `id` only: `stop` rejects a name, a full `sessionId` or a pid with
//! `No job matching '<x>'`. Confirmation is the UI's job; these functions run
//! the command once and report the outcome.
//!
//! `rm` refuses when the worktree holds uncommitted changes or unpushed
//! commits, or is in use, and suggests `--discard-unpushed …` or
//! `--force-remove-worktree …` for a second try. Tower never passes those
//! flags and never retries: [`remove`] returns the CLI's full text as
//! [`RemoveError::Refused`] for the user to act on (doc/0009, doc/0016).
//!
//! One exception to the owner's command (doc/0034): when the owner's `rm`
//! answers that its background service may be restarting and the row has no
//! live process, [`remove_with_fallback`] retries with the commands of the
//! other accounts the hand-off guard allows (doc/0018), first success wins.
//!
//! Restarting a session onto the installed Claude Code ([`respawn`], doc/0047)
//! works the same way: the owner's command, the short `id`. Never `--all`: the
//! id check refuses anything that starts with `-`, and tower restarts several
//! sessions by looping over their ids.

use crate::cli::{CliError, CliOutput, CliRunner, CommandLine};
use crate::config::Account;

/// The CLI's message for an unknown id (reference doc).
pub const NO_JOB_MATCHING: &str = "No job matching";

/// The stable part of `rm`'s refusal when the owning account's supervisor
/// does not answer, matched case-insensitively (reference doc, doc/0034).
pub const SERVICE_NOT_ANSWERING: &str = "background service may be restarting";

/// `<account command> stop <id>`.
pub fn stop_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["stop", id])
}

/// `<account command> rm <id>`. Never with `--discard-unpushed` or
/// `--force-remove-worktree`.
pub fn remove_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["rm", id])
}

/// `<account command> respawn <id>` (doc/0047). Never `--all`.
pub fn respawn_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["respawn", id])
}

/// True when `id` can be passed as a short id: non-empty, no whitespace or
/// control characters, and not starting with `-` (it would be read as an
/// option).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Why `stop` did not confirm the stop.
#[derive(Debug, thiserror::Error)]
pub enum StopError {
    /// The id is empty or malformed; nothing was run.
    #[error("invalid session id `{0}`")]
    InvalidId(String),
    /// The command could not be run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
    /// `No job matching '<id>'`: the account knows no such session.
    #[error("{message}")]
    UnknownId {
        /// The CLI's message, trimmed.
        message: String,
    },
    /// Any other outcome: a non-zero exit, or exit 0 without
    /// `stopped <id>`.
    #[error("stop failed (exit {}): {message}", code_label(*code))]
    Failed {
        /// Exit code; `None` when killed by a signal.
        code: Option<i32>,
        /// stdout and stderr, trimmed.
        message: String,
    },
}

/// Why `rm` did not confirm the removal.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    /// The id is empty or malformed; nothing was run.
    #[error("invalid session id `{0}`")]
    InvalidId(String),
    /// The command could not be run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
    /// `No job matching '<id>'`: the account knows no such session.
    #[error("{message}")]
    UnknownId {
        /// The CLI's message, trimmed.
        message: String,
    },
    /// `rm` did not print `removed <id>`: a refusal (uncommitted changes,
    /// unpushed commits, worktree in use) or any other failure. The exact
    /// refusal format and exit code are not verified (doc/0009), so every
    /// such outcome lands here, with the CLI's full text for the user,
    /// including any suggested flag, which tower never passes itself.
    #[error("{message}")]
    Refused {
        /// Exit code; `None` when killed by a signal.
        code: Option<i32>,
        /// stdout and stderr, each trimmed, joined by a newline.
        message: String,
    },
}

/// Why `respawn` did not confirm the restart (doc/0047).
#[derive(Debug, thiserror::Error)]
pub enum RespawnError {
    /// The id is empty or malformed; nothing was run.
    #[error("invalid session id `{0}`")]
    InvalidId(String),
    /// The command could not be run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
    /// `No job matching '<id>'`: the account knows no such session.
    #[error("{message}")]
    UnknownId {
        /// The CLI's message, trimmed.
        message: String,
    },
    /// Any other outcome: a non-zero exit, or exit 0 without
    /// `respawned <id>`.
    #[error("respawn failed (exit {}): {message}", code_label(*code))]
    Failed {
        /// Exit code; `None` when killed by a signal.
        code: Option<i32>,
        /// stdout and stderr, trimmed.
        message: String,
    },
}

impl RemoveError {
    /// True for `rm`'s refusal `couldn't remove <id> — the background
    /// service may be restarting. …`, on stdout or stderr, in any case
    /// (doc/0034).
    pub fn service_not_answering(&self) -> bool {
        match self {
            RemoveError::Refused { message, .. } => {
                message.to_lowercase().contains(SERVICE_NOT_ANSWERING)
            }
            _ => false,
        }
    }
}

fn code_label(code: Option<i32>) -> String {
    code.map_or("signal".to_owned(), |c| c.to_string())
}

/// stdout and stderr, each trimmed, non-empty ones joined by a newline.
fn full_text(out: &CliOutput) -> String {
    let stdout = out.stdout_text();
    [stdout.trim(), out.stderr.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// True when some line of stdout reads exactly `<verb> <id>`.
fn confirms(out: &CliOutput, verb: &str, id: &str) -> bool {
    out.stdout_text().lines().any(|line| {
        line.trim()
            .strip_prefix(verb)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|rest| rest.trim() == id)
    })
}

fn unknown_id(out: &CliOutput) -> Option<String> {
    (!out.success() && full_text(out).contains(NO_JOB_MATCHING)).then(|| full_text(out))
}

/// Interprets the output of `stop <id>` (see [`stop`]).
pub fn interpret_stop(out: &CliOutput, id: &str) -> Result<(), StopError> {
    if out.success() && confirms(out, "stopped", id) {
        return Ok(());
    }
    if let Some(message) = unknown_id(out) {
        return Err(StopError::UnknownId { message });
    }
    Err(StopError::Failed {
        code: out.code,
        message: full_text(out),
    })
}

/// Interprets the output of `rm <id>` (see [`remove`]).
pub fn interpret_remove(out: &CliOutput, id: &str) -> Result<(), RemoveError> {
    if out.success() && confirms(out, "removed", id) {
        return Ok(());
    }
    if let Some(message) = unknown_id(out) {
        return Err(RemoveError::UnknownId { message });
    }
    Err(RemoveError::Refused {
        code: out.code,
        message: full_text(out),
    })
}

/// Interprets the output of `respawn <id>` (see [`respawn`]).
pub fn interpret_respawn(out: &CliOutput, id: &str) -> Result<(), RespawnError> {
    if out.success() && confirms(out, "respawned", id) {
        return Ok(());
    }
    if let Some(message) = unknown_id(out) {
        return Err(RespawnError::UnknownId { message });
    }
    Err(RespawnError::Failed {
        code: out.code,
        message: full_text(out),
    })
}

/// Runs `<account command> respawn <id>` once (doc/0047). Success means exit
/// 0 and `respawned <id>`. `account` must be the owning account; `id` the
/// short id, so `--all` can never get through here.
pub fn respawn(runner: &CliRunner, account: &Account, id: &str) -> Result<(), RespawnError> {
    if !is_valid_id(id) {
        return Err(RespawnError::InvalidId(id.to_owned()));
    }
    let out = runner.run_line(&respawn_command(account, id))?;
    interpret_respawn(&out, id)
}

/// Runs `<account command> stop <id>` once (doc/0009). Success means exit 0
/// and `stopped <id>`, which the CLI also prints for an already stopped
/// session, so stopping is idempotent. `account` must be the owning
/// account; `id` the short id.
pub fn stop(runner: &CliRunner, account: &Account, id: &str) -> Result<(), StopError> {
    if !is_valid_id(id) {
        return Err(StopError::InvalidId(id.to_owned()));
    }
    let out = runner.run_line(&stop_command(account, id))?;
    interpret_stop(&out, id)
}

/// Runs `<account command> rm <id>` once (doc/0009). Success means exit 0
/// and `removed <id>`. A refusal is returned as [`RemoveError::Refused`];
/// tower never retries with `--discard-unpushed` or
/// `--force-remove-worktree`.
pub fn remove(runner: &CliRunner, account: &Account, id: &str) -> Result<(), RemoveError> {
    if !is_valid_id(id) {
        return Err(RemoveError::InvalidId(id.to_owned()));
    }
    let out = runner.run_line(&remove_command(account, id))?;
    interpret_remove(&out, id)
}

/// Which other accounts' commands may retry a remove that the owner's
/// command refused because its background service did not answer
/// (doc/0034).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallback {
    /// The owner's answer is final: the row has a live process, or is no
    /// longer listed.
    Final,
    /// The row has no live process; these accounts may retry, in config
    /// order (never the owner).
    Others(Vec<Account>),
    /// The row has no live process, but the account groups allow no other
    /// account (doc/0018).
    Guarded,
    /// The row has no live process and no other account is configured.
    NoOthers,
}

/// How a remove ended (doc/0009, doc/0034).
#[derive(Debug)]
pub struct Removal {
    /// `Ok(None)`: the owner's command removed the session. `Ok(Some(name))`:
    /// account `name`'s command did, after the owner's service refusal.
    /// `Err`: the owner's error.
    pub result: Result<Option<String>, RemoveError>,
    /// The other accounts whose command was tried and did not remove the
    /// session, in the order tried.
    pub tried: Vec<String>,
}

impl From<Result<(), RemoveError>> for Removal {
    /// The owner's own result, with no other account tried.
    fn from(result: Result<(), RemoveError>) -> Removal {
        Removal {
            result: result.map(|()| None),
            tried: Vec::new(),
        }
    }
}

/// Runs `<owner command> rm <id>` (see [`remove`]). When that fails with
/// the service refusal ([`RemoveError::service_not_answering`]) and
/// `fallback` lists other accounts, runs `<command> rm <id>` with each of
/// them in turn until one prints `removed <id>` (doc/0034). Any other
/// error, or [`Fallback::Final`], ends with the owner's answer.
pub fn remove_with_fallback(
    runner: &CliRunner,
    owner: &Account,
    fallback: &Fallback,
    id: &str,
) -> Removal {
    let err = match remove(runner, owner, id) {
        Ok(()) => return Ok(()).into(),
        Err(err) => err,
    };
    let others = match fallback {
        Fallback::Others(others) if err.service_not_answering() => others.as_slice(),
        _ => return Err(err).into(),
    };
    let mut tried = Vec::new();
    for other in others.iter().filter(|a| a.name != owner.name) {
        if remove(runner, other, id).is_ok() {
            return Removal {
                result: Ok(Some(other.name.clone())),
                tried,
            };
        }
        tried.push(other.name.clone());
    }
    Removal {
        result: Err(err),
        tried,
    }
}
