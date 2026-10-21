//! The quick reply (doc/0040): a text sent to a session as the user's own
//! next prompt, under the account that owns it, keeping its id.
//!
//! Claude Code delivers a prompt to a background session only through
//! `--resume <sessionId> --bg <prompt>` on a **stopped** session; on a live
//! one the same command starts a copy instead (reference doc). So
//! [`run`] does:
//!
//! 1. If the row has a live process: `<owner command> stop <id>`, then the
//!    hand-off's stop wait ([`handoff::wait_for_stop`]) until the listing
//!    shows no live process.
//! 2. `<owner command> --resume <sessionId> --bg -- <text>` in the session's
//!    `cwd`. The `--` ends Claude Code's options, so a text that starts with
//!    `-` is a prompt, not an option (verified, reference doc); the text is
//!    one argument, line breaks included.
//! 3. stdout `backgrounded · <id> · …` must name the session's own short id
//!    ([`handoff::interpret_resume`]); another id means Claude Code started
//!    a copy, which is reported as [`ReplyError::Copied`] and left alone.
//!
//! `Workspace not trusted` is [`ReplyError::Untrusted`]: the UI offers the
//! trust flow of doc/0008 for the owner and retries once without a second
//! stop ([`ReplyRequest::after_stop`]). Which rows are offered is
//! [`refusal`]. Nothing here touches the sidecar.

use std::path::PathBuf;
use std::time::Duration;

use crate::actions::{self, StopError};
use crate::cli::{CliError, CliRunner, CommandLine};
use crate::config::Account;
use crate::handoff::{self, HandoffError, HandoffTiming, Outcome};
use crate::listing::{Session, State, Status};

/// The end-of-options marker passed before the text (doc/0040).
pub const END_OF_OPTIONS: &str = "--";

/// The longest text tower sends, in bytes: well below Linux's limit of
/// 128 KiB for one argument, and above the 64 KiB of a template
/// (doc/0036).
pub const MAX_TEXT_BYTES: usize = 100 * 1024;

/// The text to send: `typed` trimmed, `None` when nothing is left
/// (doc/0040: an empty text sends nothing). Line breaks inside are kept.
pub fn text_to_send(typed: &str) -> Option<String> {
    let text = typed.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Why `t` is not offered on the row `s`, judged from the row alone
/// (doc/0040); `None` when it is. Offered: background rows with a short id
/// and a `sessionId` that either have no live process (stopped, finished,
/// failed), or are live and idle at the end of a turn: `done` / `idle`
/// (Ready for review), `blocked` / `idle` with no `waitingFor` (a question
/// that ended the turn), `failed` / `idle`, and `working` / `idle` (woken
/// without a prompt, doc/0033). The orchestrator follows the same rules.
/// Owner, group and running-job checks are the app's.
pub fn refusal(s: &Session) -> Option<String> {
    let name = s.display_name();
    if s.is_interactive() {
        return Some(format!(
            "{name} runs in another terminal — type your reply there"
        ));
    }
    if s.id.is_none() {
        return Some(format!("{name} has no short id; tower cannot reply to it"));
    }
    if s.session_id.is_none() {
        return Some(format!(
            "{name} has no sessionId; tower cannot wake it with your text"
        ));
    }
    if !s.has_live_process() {
        return None;
    }
    if s.status == Some(Status::Waiting) || s.waiting_for.is_some() {
        let what = s
            .waiting_for
            .as_ref()
            .map_or_else(String::new, |w| format!(" ({})", w.as_str()));
        return Some(format!(
            "{name} is waiting in Claude Code{what}; attach to answer it (Enter)"
        ));
    }
    if s.status == Some(Status::Busy) {
        return Some(if s.state == Some(State::Working) {
            format!(
                "{name} is working; a reply would interrupt its turn — wait until it ends, or attach"
            )
        } else {
            format!(
                "{name} still runs a background task; a reply would stop it — wait until it \
                 ends, or attach"
            )
        });
    }
    let idle = s.status == Some(Status::Idle);
    let offered = matches!(
        s.state,
        Some(State::Done) | Some(State::Blocked) | Some(State::Failed) | Some(State::Working)
    );
    if idle && offered {
        return None;
    }
    Some(format!(
        "tower cannot tell whether {name} is idle; attach to send your prompt there"
    ))
}

/// One quick reply (doc/0040).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyRequest {
    /// The short id, for `stop` and to recognise the wake.
    pub id: String,
    /// The full `sessionId` passed to `--resume`.
    pub session_id: String,
    /// Display name, for messages.
    pub name: String,
    /// The session's working directory; `--resume` runs there.
    pub cwd: PathBuf,
    /// The owning account: it stops and wakes the session.
    pub owner: Account,
    /// Whether the row has a live process, so it is stopped first.
    pub live: bool,
    /// The text, already trimmed ([`text_to_send`]).
    pub text: String,
}

impl ReplyRequest {
    /// The same request for the retry after the trust flow: the session
    /// was already stopped, so it is not stopped again.
    pub fn after_stop(&self) -> ReplyRequest {
        ReplyRequest {
            live: false,
            ..self.clone()
        }
    }

    /// `<owner command> stop <id>`, when the row is live.
    pub fn stop_command(&self) -> Option<CommandLine> {
        self.live
            .then(|| actions::stop_command(&self.owner, &self.id))
    }

    /// `<owner command> --resume <sessionId> --bg -- <text>`, run in the
    /// session's `cwd`. The text is one argument after the end of options,
    /// so a leading `-` stays text (doc/0040).
    pub fn resume_command(&self) -> CommandLine {
        CommandLine::for_account(
            &self.owner,
            [
                "--resume",
                self.session_id.as_str(),
                "--bg",
                END_OF_OPTIONS,
                self.text.as_str(),
            ],
        )
        .with_cwd(&self.cwd)
    }

    /// Checks what can be checked before anything runs: a valid short id
    /// and `sessionId`, a text that is not empty, has no NUL and fits
    /// [`MAX_TEXT_BYTES`], and an existing `cwd`.
    pub fn validate(&self) -> Result<(), ReplyError> {
        if !actions::is_valid_id(&self.id) {
            return Err(ReplyError::Invalid(format!(
                "invalid session id `{}`",
                self.id
            )));
        }
        if !actions::is_valid_id(&self.session_id) {
            return Err(ReplyError::Invalid(format!(
                "invalid sessionId `{}`",
                self.session_id
            )));
        }
        if self.text.trim().is_empty() {
            return Err(ReplyError::Invalid("the text is empty".to_owned()));
        }
        if self.text.contains('\0') {
            return Err(ReplyError::Invalid(
                "the text contains a NUL character".to_owned(),
            ));
        }
        if self.text.len() > MAX_TEXT_BYTES {
            return Err(ReplyError::Invalid(format!(
                "the text is {} KiB; at most {} KiB",
                self.text.len().div_ceil(1024),
                MAX_TEXT_BYTES / 1024
            )));
        }
        if !self.cwd.is_dir() {
            return Err(ReplyError::CwdMissing(self.cwd.clone()));
        }
        Ok(())
    }
}

/// Where a running reply is, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyStep {
    /// `stop` runs with the owner's command.
    Stopping { account: String },
    /// Waiting until the listing shows no live process.
    WaitingForStop,
    /// `--resume … --bg -- <text>` runs with the owner's command.
    Waking { account: String },
}

/// A delivered reply: the session woke under its own id with the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replied {
    /// The stderr `note:` lines of the wake, trimmed.
    pub notes: Vec<String>,
}

/// Why a reply did not arrive.
#[derive(Debug, thiserror::Error)]
pub enum ReplyError {
    /// The request is malformed; nothing was run.
    #[error("{0}")]
    Invalid(String),
    /// The session's folder no longer exists; nothing was run.
    #[error("the session's folder {} no longer exists", .0.display())]
    CwdMissing(PathBuf),
    /// `stop` failed; nothing was woken.
    #[error("stop failed: {0}")]
    Stop(StopError),
    /// The row still had a live process after the timeout; not woken.
    #[error("the session still runs {}s after stop; your text was not sent{}", .waited.as_secs(), .last_error.as_ref().map(|e| format!(" (last listing error: {e})")).unwrap_or_default())]
    StillRunning {
        /// How long tower waited.
        waited: Duration,
        /// The last listing error, when listings failed.
        last_error: Option<String>,
    },
    /// Another tower recorded the session under another account since this
    /// tower last read the sidecar (doc/0017 section 8); nothing more was
    /// run. `stopped`: this tower had already stopped it.
    #[error("another tower recorded this session under {account} meanwhile; {}", if *.stopped { "it was stopped and not woken" } else { "nothing was run" })]
    OwnerChanged {
        /// The account in the other tower's record.
        account: String,
        /// Whether `stop` had already run.
        stopped: bool,
    },
    /// `--resume` could not be run or timed out.
    #[error(transparent)]
    ResumeCli(CliError),
    /// The owner has not trusted the folder (exit 1, `Workspace not
    /// trusted…`); the session was not woken. Offer the trust flow.
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
    /// Exit 0, but no `backgrounded · <id> …` line on stdout.
    #[error("--resume printed no session id: {stdout}")]
    Unparsed {
        /// stdout, trimmed.
        stdout: String,
        /// stderr, trimmed.
        stderr: String,
    },
    /// Claude Code started a copy under `new_id` instead of waking the
    /// session; the copy got the text and is left running. Nothing else was
    /// stopped or removed.
    #[error("--resume started a copy {new_id} instead of waking the session")]
    Copied {
        /// The copy's short id.
        new_id: String,
        /// The stderr `note:` lines.
        notes: Vec<String>,
    },
}

impl ReplyError {
    /// True when the session may now be stopped without the text: the
    /// error came after `stop` ran.
    pub fn after_stop(&self) -> bool {
        matches!(
            self,
            ReplyError::StillRunning { .. }
                | ReplyError::ResumeCli(_)
                | ReplyError::Untrusted { .. }
                | ReplyError::Failed { .. }
                | ReplyError::Unparsed { .. }
                | ReplyError::OwnerChanged { stopped: true, .. }
        )
    }
}

impl From<HandoffError> for ReplyError {
    fn from(err: HandoffError) -> Self {
        match err {
            HandoffError::Invalid(m) => ReplyError::Invalid(m),
            HandoffError::CwdMissing(dir) => ReplyError::CwdMissing(dir),
            HandoffError::Stop(e) => ReplyError::Stop(e),
            HandoffError::StillRunning { waited, last_error } => {
                ReplyError::StillRunning { waited, last_error }
            }
            HandoffError::ResumeCli(e) => ReplyError::ResumeCli(e),
            HandoffError::Untrusted { dir, message } => ReplyError::Untrusted { dir, message },
            HandoffError::Failed { code, stderr } => ReplyError::Failed { code, stderr },
            HandoffError::OwnerChanged { account, stopped } => {
                ReplyError::OwnerChanged { account, stopped }
            }
            HandoffError::Unparsed { stdout, stderr } => ReplyError::Unparsed { stdout, stderr },
        }
    }
}

/// Carries out `request` (see the module docs), reporting each step to
/// `progress`. `check` runs before `stop` (with `false`) and again right
/// before the wake (with whether `stop` ran); an error from it ends the
/// reply there (the runtime uses it to honour another tower's newer
/// sidecar record, doc/0017 section 8). Blocks for up to the stop wait
/// plus the runner's timeouts; the UI calls it on a worker thread.
pub fn run(
    runner: &CliRunner,
    request: &ReplyRequest,
    timing: &HandoffTiming,
    progress: &mut dyn FnMut(ReplyStep),
    check: &mut dyn FnMut(bool) -> Result<(), ReplyError>,
) -> Result<Replied, ReplyError> {
    request.validate()?;
    check(false)?;
    let mut stopped = false;
    if request.live {
        progress(ReplyStep::Stopping {
            account: request.owner.name.clone(),
        });
        actions::stop(runner, &request.owner, &request.id).map_err(ReplyError::Stop)?;
        stopped = true;
        progress(ReplyStep::WaitingForStop);
        handoff::wait_for_stop(
            runner,
            &request.owner,
            &request.id,
            &request.session_id,
            timing,
        )?;
    }
    check(stopped)?;
    progress(ReplyStep::Waking {
        account: request.owner.name.clone(),
    });
    let out = runner
        .run_line(&request.resume_command())
        .map_err(ReplyError::ResumeCli)?;
    let resumed = handoff::interpret_resume(&out, &request.id, &request.cwd)?;
    match resumed.outcome {
        Outcome::Continued => Ok(Replied {
            notes: resumed.notes,
        }),
        Outcome::Copied { new_id } => Err(ReplyError::Copied {
            new_id,
            notes: resumed.notes,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_trimmed_and_blank_sends_nothing() {
        assert_eq!(text_to_send("  \n\t "), None);
        assert_eq!(
            text_to_send("\n  review it\n  - then fix\n\n").as_deref(),
            Some("review it\n  - then fix")
        );
    }
}
