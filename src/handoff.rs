//! Continuing a session under another account (doc/0004).
//!
//! A [`HandoffRequest`] describes one hand-off; [`run`] carries it out:
//!
//! 1. If the row has a live process, `<stop account> stop <id>` with the
//!    owning account's command, or the target's when the owner is unknown
//!    (`stop` works from any account, doc/0004). Then the listing is polled
//!    until the row is listed without a live process, or is missing from
//!    two non-empty listings in a row ([`wait_until_stopped`],
//!    [`HandoffTiming`]).
//! 2. `<target command> --resume <sessionId> --bg <handoff_prompt>` with the
//!    session's `cwd` as working directory. The prompt is always passed:
//!    without it the woken session sits idle (reference doc).
//! 3. stdout `backgrounded · <id> · <name>` is parsed and stderr `note:`
//!    lines are collected ([`interpret_resume`]). The same short id means
//!    the session continued ([`Outcome::Continued`]); a different one means
//!    Claude Code started a copy ([`Outcome::Copied`]).
//!
//! `Workspace not trusted` is reported as [`HandoffError::Untrusted`]; the
//! UI then offers the trust flow of doc/0008 for the target account and
//! retries once ([`HandoffRequest::after_stop`]). Nothing here touches the
//! sidecar; on success the caller uses [`record`].

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::{self, StopError};
use crate::cli::{CliError, CliOutput, CliRunner, CommandLine};
use crate::config::Account;
use crate::dispatch::{UNTRUSTED_PREFIX, parse_backgrounded};
use crate::listing::{Listing, Session};
use crate::sidecar::{AccountSource, Sidecar};

/// How long [`run`] waits for the stopped process to go away, and how often
/// it lists in the meantime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffTiming {
    /// Give up when the row still has a live process after this long.
    pub stop_timeout: Duration,
    /// Pause between two listings.
    pub poll_interval: Duration,
}

impl Default for HandoffTiming {
    fn default() -> Self {
        HandoffTiming {
            stop_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(500),
        }
    }
}

/// One hand-off (doc/0004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRequest {
    /// The short id, for `stop`; `--resume` takes the `sessionId`.
    pub id: String,
    /// The full `sessionId` passed to `--resume`.
    pub session_id: String,
    /// Display name, for messages.
    pub name: String,
    /// The session's working directory; `--resume` runs there.
    pub cwd: PathBuf,
    /// The owning account, `None` when it is unknown (`?`).
    pub owner: Option<Account>,
    /// The account the session continues under.
    pub target: Account,
    /// Whether the row has a live process, so it must be stopped first.
    pub live: bool,
    /// The prompt for the next turn (`handoff_prompt`, default `continue`).
    pub prompt: String,
}

impl HandoffRequest {
    /// The account whose command stops the session: the owner, or the
    /// target when the owner is unknown (doc/0004). `None` when the row
    /// has no live process and `stop` is skipped.
    pub fn stop_account(&self) -> Option<&Account> {
        if !self.live {
            return None;
        }
        Some(self.owner.as_ref().unwrap_or(&self.target))
    }

    /// The same request for the retry after the trust flow: the session
    /// was already stopped, so it is not stopped again.
    pub fn after_stop(&self) -> HandoffRequest {
        HandoffRequest {
            live: false,
            ..self.clone()
        }
    }

    /// `<account command> stop <id>`, when a stop is due.
    pub fn stop_command(&self) -> Option<CommandLine> {
        self.stop_account()
            .map(|a| actions::stop_command(a, &self.id))
    }

    /// `<target command> --resume <sessionId> --bg <prompt>`, run in the
    /// session's `cwd`. The prompt is trimmed and passed as one argument.
    pub fn resume_command(&self) -> CommandLine {
        CommandLine::for_account(
            &self.target,
            [
                "--resume",
                self.session_id.as_str(),
                "--bg",
                self.prompt.trim(),
            ],
        )
        .with_cwd(&self.cwd)
    }

    /// What will happen, for the confirmation and the picker, e.g.
    /// `stop under main, continue under alt`.
    pub fn plan(&self) -> String {
        let target = &self.target.name;
        match (self.stop_account(), &self.owner) {
            (None, _) => format!("not running; continue under {target}"),
            (Some(_), Some(owner)) => {
                format!("stop under {}, continue under {target}", owner.name)
            }
            (Some(_), None) => {
                format!("stop with {target}'s command (owner unknown), continue under {target}")
            }
        }
    }

    /// Checks what can be checked before anything runs (doc/0004
    /// preconditions): a valid short id and `sessionId`, a prompt, and an
    /// existing `cwd`.
    pub fn validate(&self) -> Result<(), HandoffError> {
        if !actions::is_valid_id(&self.id) {
            return Err(HandoffError::Invalid(format!(
                "invalid session id `{}`",
                self.id
            )));
        }
        if !actions::is_valid_id(&self.session_id) {
            return Err(HandoffError::Invalid(format!(
                "invalid sessionId `{}`",
                self.session_id
            )));
        }
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err(HandoffError::Invalid(
                "handoff_prompt is empty; without a prompt the session would wake idle".into(),
            ));
        }
        if prompt.starts_with('-') {
            return Err(HandoffError::Invalid(
                "handoff_prompt must not start with '-'".into(),
            ));
        }
        if !self.cwd.is_dir() {
            return Err(HandoffError::CwdMissing(self.cwd.clone()));
        }
        Ok(())
    }
}

/// Where a running hand-off is, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `stop` runs with this account's command.
    Stopping { account: String },
    /// Waiting until the listing shows no live process.
    WaitingForStop,
    /// `--resume … --bg` runs with this account's command.
    Resuming { account: String },
}

/// What `--resume <sessionId> --bg` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The same short id: the session continues under the target.
    Continued,
    /// A different short id: Claude Code started a copy (doc/0004).
    Copied {
        /// The copy's short id.
        new_id: String,
    },
}

/// A successful `--resume … --bg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumed {
    /// The short id printed in `backgrounded · <id> · <name>`.
    pub id: String,
    /// The name printed there, when present.
    pub name: Option<String>,
    /// Continued or copied.
    pub outcome: Outcome,
    /// The stderr `note:` lines, trimmed, e.g. `note: woke session <id>
    /// with its saved options (--name, --model).`
    pub notes: Vec<String>,
    /// The line ended in `(idle — send a prompt to start)`: the session
    /// woke without a turn (only expected without a prompt).
    pub idle: bool,
}

/// Why a hand-off did not finish.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    /// The request is malformed; nothing was run.
    #[error("{0}")]
    Invalid(String),
    /// The session's folder no longer exists; nothing was run.
    #[error("the session's folder {} no longer exists", .0.display())]
    CwdMissing(PathBuf),
    /// `stop` failed; nothing was resumed.
    #[error("stop failed: {0}")]
    Stop(#[from] StopError),
    /// The row still had a live process after the timeout; not resumed.
    #[error("the session still runs {}s after stop; not resumed{}", .waited.as_secs(), .last_error.as_ref().map(|e| format!(" (last listing error: {e})")).unwrap_or_default())]
    StillRunning {
        /// How long tower waited.
        waited: Duration,
        /// The last listing error, when listings failed.
        last_error: Option<String>,
    },
    /// `--resume` could not be run or timed out.
    #[error(transparent)]
    ResumeCli(CliError),
    /// The target account has not trusted the folder (exit 1, `Workspace
    /// not trusted…`). Offer the trust flow for `dir`, then retry.
    #[error("{message}")]
    Untrusted {
        /// The session's folder.
        dir: PathBuf,
        /// The CLI's message, trimmed.
        message: String,
    },
    /// Any other non-zero exit of `--resume`.
    #[error("--resume failed (exit {}): {stderr}", .code.map_or("signal".to_owned(), |c| c.to_string()))]
    Failed {
        /// Exit code; `None` when killed by a signal.
        code: Option<i32>,
        /// stderr, trimmed (stdout when stderr is empty).
        stderr: String,
    },
    /// Another tower recorded the session under another account since this
    /// tower last read the sidecar (doc/0017 section 8); nothing more was
    /// run. `stopped`: this tower had already stopped it.
    #[error("another tower recorded this session under {account} meanwhile; {}", if *.stopped { "it was stopped and not continued" } else { "nothing was run" })]
    OwnerChanged {
        /// The account in the other tower's record.
        account: String,
        /// Whether `stop` had already run.
        stopped: bool,
    },
    /// Exit 0, but no `backgrounded · <id> …` line on stdout.
    #[error("--resume printed no session id: {stdout}")]
    Unparsed {
        /// stdout, trimmed.
        stdout: String,
        /// stderr, trimmed.
        stderr: String,
    },
}

impl HandoffError {
    /// True when the error came after `--resume` was attempted or after
    /// `stop` succeeded, i.e. the session may now be stopped without having
    /// continued.
    pub fn after_stop(&self) -> bool {
        matches!(
            self,
            HandoffError::StillRunning { .. }
                | HandoffError::ResumeCli(_)
                | HandoffError::Untrusted { .. }
                | HandoffError::Failed { .. }
                | HandoffError::Unparsed { .. }
                | HandoffError::OwnerChanged { stopped: true, .. }
        )
    }
}

/// Interprets the output of `--resume <sessionId> --bg <prompt>` for the
/// session with short id `id`, run in `dir`.
pub fn interpret_resume(out: &CliOutput, id: &str, dir: &Path) -> Result<Resumed, HandoffError> {
    if !out.success() {
        let untrusted = out.code == Some(1)
            && out
                .stderr
                .lines()
                .any(|l| l.trim_start().starts_with(UNTRUSTED_PREFIX));
        if untrusted {
            return Err(HandoffError::Untrusted {
                dir: dir.to_path_buf(),
                message: out.stderr.trim().to_owned(),
            });
        }
        let stderr = out.stderr.trim();
        let stderr = if stderr.is_empty() {
            out.stdout_text().trim().to_owned()
        } else {
            stderr.to_owned()
        };
        return Err(HandoffError::Failed {
            code: out.code,
            stderr,
        });
    }
    let stdout = out.stdout_text();
    let Some(started) = parse_backgrounded(&stdout) else {
        return Err(HandoffError::Unparsed {
            stdout: stdout.trim().to_owned(),
            stderr: out.stderr.trim().to_owned(),
        });
    };
    let idle = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("backgrounded"))
        .is_some_and(|l| l.contains("(idle"));
    let notes = out
        .stderr
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("note:"))
        .map(str::to_owned)
        .collect();
    let outcome = if started.id == id {
        Outcome::Continued
    } else {
        Outcome::Copied {
            new_id: started.id.clone(),
        }
    };
    Ok(Resumed {
        id: started.id,
        name: started.name,
        outcome,
        notes,
        idle,
    })
}

/// The row of `request` in a listing: by `sessionId`, else by short id.
fn find_row<'a>(rows: &'a [Session], request: &HandoffRequest) -> Option<&'a Session> {
    rows.iter()
        .find(|s| s.session_id.as_deref() == Some(request.session_id.as_str()))
        .or_else(|| {
            rows.iter()
                .find(|s| s.id.as_deref() == Some(request.id.as_str()))
        })
}

/// How many successful, non-empty, complete listings in a row must miss
/// the row before [`wait_until_stopped`] counts it as gone.
pub const ABSENT_LISTINGS: u32 = 2;

/// What one listing says about the stopped row (doc/0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopEvidence {
    /// Listed without a live process: stopped.
    Stopped,
    /// Listed with a live process: still running.
    Running,
    /// Not listed in a non-empty, complete listing: gone, if it repeats.
    Absent,
    /// No evidence: an empty or partial listing (a supervisor that is still
    /// starting, or a parser that skipped rows) or a failed one.
    Nothing,
}

/// What one listing result says about the row of `request`.
pub fn stop_evidence(listing: &Result<Listing, String>, request: &HandoffRequest) -> StopEvidence {
    let Ok(listing) = listing else {
        return StopEvidence::Nothing;
    };
    match find_row(&listing.sessions, request) {
        Some(row) if row.has_live_process() => StopEvidence::Running,
        Some(_) => StopEvidence::Stopped,
        None if listing.sessions.is_empty() || !listing.warnings.is_empty() => {
            StopEvidence::Nothing
        }
        None => StopEvidence::Absent,
    }
}

/// Lists with `account` until the row of `request` is listed without a
/// live process, or is missing from [`ABSENT_LISTINGS`] successful,
/// non-empty, complete listings in a row, up to `timing.stop_timeout`.
/// An empty or partial listing is no evidence: it may come while the
/// process still lives, and resuming then would start a copy.
pub fn wait_until_stopped(
    runner: &CliRunner,
    account: &Account,
    request: &HandoffRequest,
    timing: &HandoffTiming,
) -> Result<(), HandoffError> {
    let start = Instant::now();
    let mut absent = 0;
    loop {
        let listing = runner.list_sessions(account).map_err(|err| err.to_string());
        let evidence = stop_evidence(&listing, request);
        absent = match evidence {
            StopEvidence::Stopped => return Ok(()),
            StopEvidence::Absent => absent + 1,
            _ => 0,
        };
        if absent >= ABSENT_LISTINGS {
            return Ok(());
        }
        let last_error = listing.err();
        let waited = start.elapsed();
        if waited >= timing.stop_timeout {
            return Err(HandoffError::StillRunning { waited, last_error });
        }
        thread::sleep(
            timing
                .poll_interval
                .min(timing.stop_timeout.saturating_sub(waited)),
        );
    }
}

/// Carries out `request` (see the module docs), reporting each step to
/// `progress`. Blocks for up to the stop wait plus the runner's timeouts;
/// the UI calls it on a worker thread. Does not touch the sidecar: on
/// success the caller uses [`record`].
pub fn run(
    runner: &CliRunner,
    request: &HandoffRequest,
    timing: &HandoffTiming,
    progress: &mut dyn FnMut(Step),
) -> Result<Resumed, HandoffError> {
    run_checked(runner, request, timing, progress, &mut |_| Ok(()))
}

/// [`run`], calling `check` before `stop` (with `false`) and again right
/// before `--resume` wakes the session (with whether `stop` ran). An error
/// from `check` ends the hand-off there; the runtime uses it to honour
/// another tower's newer sidecar record (doc/0017 section 8).
pub fn run_checked(
    runner: &CliRunner,
    request: &HandoffRequest,
    timing: &HandoffTiming,
    progress: &mut dyn FnMut(Step),
    check: &mut dyn FnMut(bool) -> Result<(), HandoffError>,
) -> Result<Resumed, HandoffError> {
    request.validate()?;
    check(false)?;
    let mut stopped = false;
    if let Some(account) = request.stop_account() {
        progress(Step::Stopping {
            account: account.name.clone(),
        });
        actions::stop(runner, account, &request.id)?;
        stopped = true;
        progress(Step::WaitingForStop);
        wait_until_stopped(runner, account, request, timing)?;
    }
    check(stopped)?;
    progress(Step::Resuming {
        account: request.target.name.clone(),
    });
    let out = runner
        .run_line(&request.resume_command())
        .map_err(HandoffError::ResumeCli)?;
    interpret_resume(&out, &request.id, &request.cwd)
}

/// Records a successful hand-off in the sidecar (doc/0004): the target as
/// owner with `account_source: handoff`, under the `sessionId` when the
/// session continued. A copy is recorded under its short id, which the next
/// poll moves to the copy's `sessionId`
/// ([`Sidecar::adopt_short_id_records`]); the original's entry is left as
/// it was. Returns true when the sidecar changed.
pub fn record(sidecar: &mut Sidecar, request: &HandoffRequest, resumed: &Resumed) -> bool {
    let key = match &resumed.outcome {
        Outcome::Continued => request.session_id.as_str(),
        Outcome::Copied { new_id } => new_id.as_str(),
    };
    sidecar.record_owner(key, &request.target.name, AccountSource::Handoff)
}
