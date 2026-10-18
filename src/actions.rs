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

use crate::cli::{CliError, CliOutput, CliRunner, CommandLine};
use crate::config::Account;

/// The CLI's message for an unknown id (reference doc).
pub const NO_JOB_MATCHING: &str = "No job matching";

/// `<account command> stop <id>`.
pub fn stop_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["stop", id])
}

/// `<account command> rm <id>`. Never with `--discard-unpushed` or
/// `--force-remove-worktree`.
pub fn remove_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["rm", id])
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
