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
//! 2. `<target command> --resume <sessionId> --bg [<handoff_prompt>]` with
//!    the session's `cwd` as working directory. An empty `handoff_prompt`
//!    (after trimming) passes no prompt: the woken session then waits for
//!    the user under the target account and starts no turn (doc/0033).
//! 3. stdout `backgrounded · <id> · <name>` is parsed, with or without the
//!    `(idle — send a prompt to start)` suffix a wake without a prompt
//!    prints, and stderr `note:` lines are collected ([`interpret_resume`]).
//!    The same short id means the session continued
//!    ([`Outcome::Continued`]); a different one means Claude Code started a
//!    copy ([`Outcome::Copied`]).
//!
//! `Workspace not trusted` is reported as [`HandoffError::Untrusted`]; the
//! UI then offers the trust flow of doc/0008 for the target account and
//! retries once ([`HandoffRequest::after_stop`]). Nothing here touches the
//! sidecar; on success the caller uses [`record`].
//!
//! Which accounts a session may go to is decided by [`targets`] and
//! [`check_target`]: accounts of the owner's `group` only, unless
//! `allow_cross_group_handoff` is set. A hand-off moves the conversation and
//! project context under the target account's terms (doc/0004).

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::{self, StopError};
use crate::cli::{CliError, CliOutput, CliRunner, CommandLine};
use crate::config::{Account, Config};
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
    /// Empty or blank: no prompt is passed and the session waits for the
    /// user under the target account (doc/0033).
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

    /// The prompt passed to `--resume`, trimmed; `None` when it is empty,
    /// so the session wakes without a turn (doc/0033).
    pub fn prompt_arg(&self) -> Option<&str> {
        Some(self.prompt.trim()).filter(|p| !p.is_empty())
    }

    /// `<target command> --resume <sessionId> --bg [<prompt>]`, run in the
    /// session's `cwd`. The prompt is trimmed and passed as one argument;
    /// an empty one is left out ([`HandoffRequest::prompt_arg`]).
    pub fn resume_command(&self) -> CommandLine {
        let mut args = vec!["--resume", self.session_id.as_str(), "--bg"];
        args.extend(self.prompt_arg());
        CommandLine::for_account(&self.target, args).with_cwd(&self.cwd)
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
    /// preconditions): a valid short id and `sessionId`, a prompt that is
    /// empty (no prompt, doc/0033) or does not start with `-`, and an
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
        if self.prompt_arg().is_some_and(|p| p.starts_with('-')) {
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
    /// woke without a turn (expected without a prompt, doc/0033).
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

/// An account with its group for refusal messages: `main (work)`, or
/// `main (no group)` for an ungrouped account.
fn group_label(a: &Account) -> String {
    if a.group.is_empty() {
        format!("{} (no group)", a.name)
    } else {
        a.label()
    }
}

/// Whether the configured accounts are in more than one group.
fn spans_groups(config: &Config) -> bool {
    config
        .accounts
        .iter()
        .any(|a| a.group != config.accounts[0].group)
}

/// Whether an action that wakes a session under the account the user picks
/// must wait until its owner is known: the accounts span several groups
/// and `allow_cross_group_handoff` is not set (doc/0018). The quick reply
/// follows this rule (doc/0040).
pub fn owner_must_be_known(config: &Config) -> bool {
    !config.allow_cross_group_handoff && spans_groups(config)
}

/// The refusal for an unknown owner while the accounts span groups.
fn unknown_owner_refusal(name: &str) -> String {
    format!(
        "{name}'s account is unknown (?) and the accounts are in different groups; attach \
         first (Enter) so tower learns its account, then hand it off \
         (or set allow_cross_group_handoff = true)"
    )
}

/// The refusal for a hand-off from `owner` to accounts of other groups.
fn cross_group_refusal(owner: &Account, to: &[&Account]) -> String {
    let to: Vec<String> = to.iter().map(|a| group_label(a)).collect();
    format!(
        "hand-off from {} to {} is disabled: different account groups; set \
         allow_cross_group_handoff = true to allow",
        group_label(owner),
        to.join(" or ")
    )
}

/// The accounts the session `name` may be handed off to (doc/0004), in
/// config order, by name. `owner` is the owning account, `None` when it is
/// unknown (`?`) or not configured.
///
/// - Known owner: every other account of the owner's group; every other
///   account when `allow_cross_group_handoff` is set.
/// - Unknown owner: every account (the stop then uses the target's
///   command), but only while all accounts share one group or the flag is
///   set. Otherwise the session's account must be learned first.
///
/// `Err` holds the status message when no account is allowed because of
/// the groups; `Ok` may be empty with a single account.
pub fn targets(
    config: &Config,
    owner: Option<&Account>,
    name: &str,
) -> Result<Vec<String>, String> {
    let cross = config.allow_cross_group_handoff;
    let Some(owner) = owner else {
        if !cross && spans_groups(config) {
            return Err(unknown_owner_refusal(name));
        }
        return Ok(config.accounts.iter().map(|a| a.name.clone()).collect());
    };
    let others: Vec<&Account> = config
        .accounts
        .iter()
        .filter(|a| a.name != owner.name)
        .collect();
    let allowed: Vec<String> = others
        .iter()
        .filter(|a| cross || a.group == owner.group)
        .map(|a| a.name.clone())
        .collect();
    if allowed.is_empty() && !others.is_empty() {
        return Err(cross_group_refusal(owner, &others));
    }
    Ok(allowed)
}

/// Checks one target right before a hand-off is planned, by the same rules
/// as [`targets`]; the owner may have been learned since the picker opened.
pub fn check_target(
    config: &Config,
    owner: Option<&Account>,
    target: &Account,
    name: &str,
) -> Result<(), String> {
    if config.allow_cross_group_handoff {
        return Ok(());
    }
    match owner {
        None if spans_groups(config) => Err(unknown_owner_refusal(name)),
        None => Ok(()),
        Some(owner) if owner.group != target.group => Err(cross_group_refusal(owner, &[target])),
        Some(_) => Ok(()),
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

/// The row with `session_id` in a listing, else the one with short id
/// `id`.
fn find_row<'a>(rows: &'a [Session], id: &str, session_id: &str) -> Option<&'a Session> {
    rows.iter()
        .find(|s| s.session_id.as_deref() == Some(session_id))
        .or_else(|| rows.iter().find(|s| s.id.as_deref() == Some(id)))
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
    stop_evidence_of(listing, &request.id, &request.session_id)
}

/// What one listing result says about the row with short id `id` and
/// `sessionId` `session_id`.
fn stop_evidence_of(listing: &Result<Listing, String>, id: &str, session_id: &str) -> StopEvidence {
    let Ok(listing) = listing else {
        return StopEvidence::Nothing;
    };
    match find_row(&listing.sessions, id, session_id) {
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
    wait_for_stop(runner, account, &request.id, &request.session_id, timing)
}

/// [`wait_until_stopped`] for the row with short id `id` and `sessionId`
/// `session_id`; the quick reply waits the same way (doc/0040).
pub fn wait_for_stop(
    runner: &CliRunner,
    account: &Account,
    id: &str,
    session_id: &str,
    timing: &HandoffTiming,
) -> Result<(), HandoffError> {
    let start = Instant::now();
    let mut absent = 0;
    loop {
        let listing = runner.list_sessions(account).map_err(|err| err.to_string());
        let evidence = stop_evidence_of(&listing, id, session_id);
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
