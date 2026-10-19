//! Renaming a session (doc/0027).
//!
//! Claude Code has no rename command or option; a session renames itself
//! with its local `/rename <name>` command (no model turn), and the new name
//! survives a stop and a wake (reference doc). So:
//!
//! - A **running** session is renamed from inside it: the UI attaches and
//!   the user types `/rename <name>` there. Nothing here is involved beyond
//!   [`check_name`].
//! - A **stopped** session is renamed by tower without attaching
//!   ([`run`]): `<owner command> --resume <sessionId> --bg "/rename <name>"`
//!   in the session's `cwd` wakes it under its owner and runs the command;
//!   tower then lists with the owner until the row shows the new name
//!   ([`RenameTiming`]), and stops the session again with `<owner command>
//!   stop <id>`.
//!
//! `--resume` with the full `sessionId` continues the session itself; if
//! Claude Code starts a copy instead (another id in `backgrounded · <id> ·
//! …`), the rename stops there and reports the copy ([`RenameError::Copied`]):
//! nothing is stopped or removed. `Workspace not trusted` is reported as
//! [`RenameError::Untrusted`], so the UI can offer the trust flow of
//! doc/0008. Nothing here touches the sidecar.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::{self, StopError};
use crate::cli::{CliError, CliRunner, CommandLine};
use crate::config::Account;
use crate::handoff::{self, HandoffError, Outcome};
use crate::listing::Session;
use crate::orchestrator;

/// The longest name tower accepts, in characters. The list shows at most
/// 32 columns of a name; Claude Code documents no limit.
pub const MAX_NAME_CHARS: usize = 80;

/// The slash command Claude Code renames a session with (reference doc).
pub const RENAME_COMMAND: &str = "/rename";

/// What a typed name means for the rename (doc/0027).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameCheck {
    /// The trimmed name equals the current one: nothing to do.
    Unchanged,
    /// Rename to this trimmed name.
    Rename(String),
}

/// Checks the typed name `typed` against the session's current name `old`:
/// trimmed, not empty, no control characters (line breaks included), at
/// most [`MAX_NAME_CHARS`] characters, and not `orchestrator` (tower
/// identifies the orchestrator by that exact name, doc/0010). `Err` holds
/// the message for the window.
pub fn check_name(typed: &str, old: &str) -> Result<NameCheck, String> {
    let name = typed.trim();
    if name.is_empty() {
        return Err("the name must not be empty".to_owned());
    }
    if name.chars().any(char::is_control) {
        return Err("the name must be one line without control characters".to_owned());
    }
    let n = name.chars().count();
    if n > MAX_NAME_CHARS {
        return Err(format!(
            "the name is {n} characters long; at most {MAX_NAME_CHARS}"
        ));
    }
    if name == old {
        return Ok(NameCheck::Unchanged);
    }
    if name == orchestrator::NAME {
        return Err(format!(
            "`{}` is the orchestrator's name: tower would take this session for the \
             orchestrator",
            orchestrator::NAME
        ));
    }
    Ok(NameCheck::Rename(name.to_owned()))
}

/// The text to type inside a running session: `/rename <name>`.
pub fn rename_prompt(name: &str) -> String {
    format!("{RENAME_COMMAND} {}", name.trim())
}

/// How long [`run`] waits for the new name to show in the listing, and how
/// often it lists meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenameTiming {
    /// Give up when the listing does not show the new name after this long.
    pub confirm_timeout: Duration,
    /// Pause between two listings.
    pub poll_interval: Duration,
}

impl Default for RenameTiming {
    fn default() -> Self {
        RenameTiming {
            confirm_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(500),
        }
    }
}

/// The rename of one stopped session (doc/0027).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRequest {
    /// The short id, for `stop` and to recognise the wake.
    pub id: String,
    /// The full `sessionId` passed to `--resume`.
    pub session_id: String,
    /// The current name, for messages.
    pub old_name: String,
    /// The new name, already checked ([`check_name`]).
    pub new_name: String,
    /// The session's working directory; `--resume` runs there.
    pub cwd: PathBuf,
    /// The owning account: it wakes, lists and stops the session.
    pub owner: Account,
}

impl RenameRequest {
    /// `<owner command> --resume <sessionId> --bg "/rename <name>"`, run in
    /// the session's `cwd`. The prompt is one argument.
    pub fn resume_command(&self) -> CommandLine {
        let prompt = rename_prompt(&self.new_name);
        CommandLine::for_account(
            &self.owner,
            [
                "--resume",
                self.session_id.as_str(),
                "--bg",
                prompt.as_str(),
            ],
        )
        .with_cwd(&self.cwd)
    }

    /// `<owner command> stop <id>`.
    pub fn stop_command(&self) -> CommandLine {
        actions::stop_command(&self.owner, &self.id)
    }

    /// Checks what can be checked before anything runs: a valid short id
    /// and `sessionId`, a valid new name and an existing `cwd`.
    pub fn validate(&self) -> Result<(), RenameError> {
        if !actions::is_valid_id(&self.id) {
            return Err(RenameError::Invalid(format!(
                "invalid session id `{}`",
                self.id
            )));
        }
        if !actions::is_valid_id(&self.session_id) {
            return Err(RenameError::Invalid(format!(
                "invalid sessionId `{}`",
                self.session_id
            )));
        }
        match check_name(&self.new_name, &self.old_name) {
            Ok(NameCheck::Rename(_)) => {}
            Ok(NameCheck::Unchanged) => {
                return Err(RenameError::Invalid("the name is unchanged".to_owned()));
            }
            Err(message) => return Err(RenameError::Invalid(message)),
        }
        if !self.cwd.is_dir() {
            return Err(RenameError::CwdMissing(self.cwd.clone()));
        }
        Ok(())
    }
}

/// Where a running rename is, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameStep {
    /// `--resume … --bg "/rename …"` runs with the owner's command.
    Waking { account: String },
    /// Listing until the row shows the new name.
    Confirming,
    /// `stop` runs with the owner's command.
    Stopping { account: String },
}

/// A finished rename: woken, renamed and stopped again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    /// The stderr `note:` lines of the wake, trimmed.
    pub notes: Vec<String>,
}

/// Why a rename did not finish.
#[derive(Debug, thiserror::Error)]
pub enum RenameError {
    /// The request is malformed; nothing was run.
    #[error("{0}")]
    Invalid(String),
    /// The session's folder no longer exists; nothing was run.
    #[error("the session's folder {} no longer exists", .0.display())]
    CwdMissing(PathBuf),
    /// Another tower recorded the session under another account since this
    /// tower last read the sidecar (doc/0017 section 8); nothing was run.
    #[error("another tower recorded this session under {account} meanwhile; nothing was run")]
    OwnerChanged {
        /// The account in the other tower's record.
        account: String,
    },
    /// `--resume` could not be run or timed out.
    #[error(transparent)]
    ResumeCli(CliError),
    /// The owner has not trusted the folder (exit 1, `Workspace not
    /// trusted…`); nothing was woken. Offer the trust flow for `dir`.
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
    /// session; the copy got the `/rename` and is left running. Nothing was
    /// stopped or removed.
    #[error("--resume started a copy {new_id} instead of waking the session")]
    Copied {
        /// The copy's short id.
        new_id: String,
        /// The stderr `note:` lines.
        notes: Vec<String>,
    },
    /// The listing did not show the new name in time; the session was
    /// woken and is left running.
    #[error("the listing did not show the new name within {}s{}", .waited.as_secs(), .last_error.as_ref().map(|e| format!(" (last listing error: {e})")).unwrap_or_default())]
    NotConfirmed {
        /// How long tower waited.
        waited: Duration,
        /// The name the last good listing showed, when the row was listed.
        last_name: Option<String>,
        /// The last listing error, when listings failed.
        last_error: Option<String>,
    },
    /// The rename was confirmed, but `stop` failed: the session runs.
    #[error("stop failed: {0}")]
    Stop(StopError),
}

impl RenameError {
    /// True when the session was woken and is left running.
    pub fn left_running(&self) -> bool {
        matches!(
            self,
            RenameError::NotConfirmed { .. } | RenameError::Stop(_)
        )
    }
}

impl From<HandoffError> for RenameError {
    fn from(err: HandoffError) -> Self {
        match err {
            HandoffError::Invalid(m) => RenameError::Invalid(m),
            HandoffError::CwdMissing(dir) => RenameError::CwdMissing(dir),
            HandoffError::ResumeCli(e) => RenameError::ResumeCli(e),
            HandoffError::Untrusted { dir, message } => RenameError::Untrusted { dir, message },
            HandoffError::Failed { code, stderr } => RenameError::Failed { code, stderr },
            HandoffError::Unparsed { stdout, stderr } => RenameError::Unparsed { stdout, stderr },
            HandoffError::OwnerChanged { account, .. } => RenameError::OwnerChanged { account },
            other => RenameError::Invalid(other.to_string()),
        }
    }
}

/// The row of `request` in a listing: by `sessionId`, else by short id.
fn find_row<'a>(rows: &'a [Session], request: &RenameRequest) -> Option<&'a Session> {
    rows.iter()
        .find(|s| s.session_id.as_deref() == Some(request.session_id.as_str()))
        .or_else(|| {
            rows.iter()
                .find(|s| s.id.as_deref() == Some(request.id.as_str()))
        })
}

/// Lists with the owner until the row of `request` shows the new name, up
/// to `timing.confirm_timeout`. A failed listing is no evidence; the wait
/// goes on.
pub fn wait_for_name(
    runner: &CliRunner,
    request: &RenameRequest,
    timing: &RenameTiming,
) -> Result<(), RenameError> {
    let start = Instant::now();
    let mut last_name = None;
    let mut last_error: Option<String>;
    loop {
        match runner.list_sessions(&request.owner) {
            Ok(listing) => {
                last_error = None;
                if let Some(row) = find_row(&listing.sessions, request) {
                    if row.name.as_deref() == Some(request.new_name.as_str()) {
                        return Ok(());
                    }
                    last_name = row.name.clone();
                }
            }
            Err(err) => last_error = Some(err.to_string()),
        }
        let waited = start.elapsed();
        if waited >= timing.confirm_timeout {
            return Err(RenameError::NotConfirmed {
                waited,
                last_name,
                last_error,
            });
        }
        thread::sleep(
            timing
                .poll_interval
                .min(timing.confirm_timeout.saturating_sub(waited)),
        );
    }
}

/// Carries out `request` (see the module docs), reporting each step to
/// `progress`. `check` runs right before the wake; an error from it ends
/// the rename there (the runtime uses it to honour another tower's newer
/// sidecar record, doc/0017 section 8). Blocks for up to the runner's
/// timeouts plus the confirmation wait; the UI calls it on a worker thread.
pub fn run(
    runner: &CliRunner,
    request: &RenameRequest,
    timing: &RenameTiming,
    progress: &mut dyn FnMut(RenameStep),
    check: &mut dyn FnMut() -> Result<(), RenameError>,
) -> Result<Renamed, RenameError> {
    request.validate()?;
    check()?;
    progress(RenameStep::Waking {
        account: request.owner.name.clone(),
    });
    let out = runner
        .run_line(&request.resume_command())
        .map_err(RenameError::ResumeCli)?;
    let resumed = handoff::interpret_resume(&out, &request.id, &request.cwd)?;
    if let Outcome::Copied { new_id } = resumed.outcome {
        return Err(RenameError::Copied {
            new_id,
            notes: resumed.notes,
        });
    }
    progress(RenameStep::Confirming);
    wait_for_name(runner, request, timing)?;
    progress(RenameStep::Stopping {
        account: request.owner.name.clone(),
    });
    actions::stop(runner, &request.owner, &request.id).map_err(RenameError::Stop)?;
    Ok(Renamed {
        notes: resumed.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_checked() {
        assert_eq!(
            check_name("  new-name ", "old"),
            Ok(NameCheck::Rename("new-name".into()))
        );
        assert_eq!(check_name(" old ", "old"), Ok(NameCheck::Unchanged));
        assert!(check_name("   ", "old").is_err());
        assert!(check_name("a\nb", "old").is_err());
        assert!(check_name("a\tb", "old").is_err());
        assert!(check_name(&"x".repeat(MAX_NAME_CHARS), "old").is_ok());
        assert!(check_name(&"x".repeat(MAX_NAME_CHARS + 1), "old").is_err());
        assert!(check_name("orchestrator", "old").is_err());
        assert_eq!(
            check_name("два слова", "old"),
            Ok(NameCheck::Rename("два слова".into()))
        );
    }

    #[test]
    fn the_prompt_is_the_slash_command() {
        assert_eq!(rename_prompt(" fix-tests "), "/rename fix-tests");
    }
}
