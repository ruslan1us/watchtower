//! `tower wake` (doc/0044): wakes a background session that has no live
//! process, with the command of the account that owns it, keeping its id.
//!
//! - [`find`] picks the row: by short id or `sessionId`, else by exact name.
//! - [`plan`] refuses what tower would refuse in the TUI: interactive rows,
//!   rows without ids, a live process (a live idle session takes the quick
//!   reply `t`, a working one nothing), an unknown owner without
//!   `--account` or while the accounts span groups (doc/0018), and an
//!   `--account` that differs from a known owner: a wake never moves a
//!   session to another account; that is the hand-off (doc/0004).
//! - [`run`] runs, in the session's `cwd`,
//!   `<owner command> --resume <sessionId> --bg` without a text (the
//!   session wakes idle, doc/0033) or `… --bg -- <text>` with one (the text
//!   is its next prompt, doc/0040). The same short id in `backgrounded ·
//!   <id> …` is success; another id is a copy ([`WakeError::Copied`]).
//!
//! Nothing here touches the sidecar; the caller records an `asked` owner
//! or a copy's `started` owner and runs the check against another tower's
//! newer record (doc/0017 section 8).

use std::path::PathBuf;

use crate::actions;
use crate::cli::{CliError, CliRunner, CommandLine};
use crate::config::{Account, Config};
use crate::groups;
use crate::handoff::{self, HandoffError, Outcome};
use crate::merge::MergedSession;
use crate::reply::{self, END_OF_OPTIONS, MAX_TEXT_BYTES};

/// One wake (doc/0044).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRequest {
    /// The short id, to recognise the wake.
    pub id: String,
    /// The full `sessionId` passed to `--resume`.
    pub session_id: String,
    /// Display name, for messages.
    pub name: String,
    /// The session's working directory; `--resume` runs there.
    pub cwd: PathBuf,
    /// The account whose command wakes the session: its owner.
    pub owner: Account,
    /// The text for the next turn, trimmed; `None` wakes it without one.
    pub text: Option<String>,
}

impl WakeRequest {
    /// `<owner command> --resume <sessionId> --bg [-- <text>]`, run in the
    /// session's `cwd`. The text is one argument after the end of options,
    /// so a leading `-` stays text (doc/0040).
    pub fn resume_command(&self) -> CommandLine {
        let mut args = vec!["--resume", self.session_id.as_str(), "--bg"];
        if let Some(text) = &self.text {
            args.push(END_OF_OPTIONS);
            args.push(text);
        }
        CommandLine::for_account(&self.owner, args).with_cwd(&self.cwd)
    }

    /// Checks what can be checked before anything runs: valid ids, a text
    /// that is not blank, has no NUL and fits [`MAX_TEXT_BYTES`], and an
    /// existing `cwd`.
    pub fn validate(&self) -> Result<(), WakeError> {
        for (what, id) in [("session id", &self.id), ("sessionId", &self.session_id)] {
            if !actions::is_valid_id(id) {
                return Err(WakeError::Invalid(format!("invalid {what} `{id}`")));
            }
        }
        if let Some(text) = &self.text {
            if text.trim().is_empty() {
                return Err(WakeError::Invalid("the text is empty".to_owned()));
            }
            if text.contains('\0') {
                return Err(WakeError::Invalid(
                    "the text contains a NUL character".to_owned(),
                ));
            }
            if text.len() > MAX_TEXT_BYTES {
                return Err(WakeError::Invalid(format!(
                    "the text is {} KiB; at most {} KiB",
                    text.len().div_ceil(1024),
                    MAX_TEXT_BYTES / 1024
                )));
            }
        }
        if !self.cwd.is_dir() {
            return Err(WakeError::CwdMissing(self.cwd.clone()));
        }
        Ok(())
    }
}

/// A planned wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePlan {
    /// What runs.
    pub request: WakeRequest,
    /// False when the owner was unknown and `--account` named it: the
    /// caller records that answer as `asked` after the wake (doc/0003).
    pub owner_known: bool,
}

/// The row `target` names: a background or interactive row whose short id
/// or `sessionId` is `target`, else the only row named exactly `target`.
/// `Err` says why there is none, or lists the ids when several rows share
/// the name.
pub fn find<'a>(sessions: &'a [MergedSession], target: &str) -> Result<&'a MergedSession, String> {
    let by_id = sessions.iter().find(|m| {
        m.session.id.as_deref() == Some(target) || m.session.session_id.as_deref() == Some(target)
    });
    if let Some(m) = by_id {
        return Ok(m);
    }
    let named: Vec<&MergedSession> = sessions
        .iter()
        .filter(|m| m.session.name.as_deref() == Some(target))
        .collect();
    match named.as_slice() {
        [] => Err(format!("no session with the id or name `{target}`")),
        [one] => Ok(one),
        several => {
            let ids: Vec<String> = several
                .iter()
                .map(|m| m.session.id.clone().unwrap_or_else(|| "(no id)".to_owned()))
                .collect();
            Err(format!(
                "{} sessions are named `{target}` ({}); pass the short id",
                several.len(),
                ids.join(", ")
            ))
        }
    }
}

/// Plans the wake of `m` (see the module docs). `account` is the
/// `--account` given, already looked up in the config. `Err` is the
/// refusal, for people.
pub fn plan(
    config: &Config,
    m: &MergedSession,
    account: Option<&Account>,
    text: Option<String>,
) -> Result<WakePlan, String> {
    let s = &m.session;
    let name = s.display_name();
    if s.is_interactive() {
        return Err(format!(
            "{name} runs in another terminal; tower wakes only background sessions"
        ));
    }
    let (Some(id), Some(session_id)) = (s.id.clone(), s.session_id.clone()) else {
        return Err(format!(
            "{name} has no short id or sessionId; tower cannot wake it"
        ));
    };
    if s.has_live_process() {
        return Err(if reply::refusal(s).is_none() {
            format!(
                "{name} is running and idle; `tower wake` only wakes sessions without a live \
                 process. To send it a text, use the quick reply (`t`) in tower"
            )
        } else {
            format!(
                "{name} is running ({}); `tower wake` only wakes sessions without a live process",
                groups::detail(s)
            )
        });
    }
    let known = m.owner.account().and_then(|n| config.account(n));
    let (owner, owner_known) = match (known, account) {
        (Some(owner), None) => (owner, true),
        (Some(owner), Some(given)) if given.name == owner.name => (owner, true),
        (Some(owner), Some(given)) => {
            return Err(format!(
                "{name} belongs to {}, not {}; `tower wake` runs only the owner's command and \
                 never moves a session to another account (the hand-off `a` in tower does)",
                owner.name, given.name
            ));
        }
        (None, None) => {
            return Err(format!(
                "{name}'s account is unknown (?); pass `--account <name>` if you know which \
                 account it belongs to, or attach once from tower (Enter) so tower learns it"
            ));
        }
        (None, Some(_)) if handoff::owner_must_be_known(config) => {
            return Err(format!(
                "{name}'s account is unknown (?) and the accounts are in different groups; \
                 attach once from tower (Enter) so tower learns its account (or set \
                 allow_cross_group_handoff = true)"
            ));
        }
        (None, Some(given)) => (given, false),
    };
    Ok(WakePlan {
        request: WakeRequest {
            id,
            session_id,
            name,
            cwd: PathBuf::from(&s.cwd),
            owner: owner.clone(),
            text,
        },
        owner_known,
    })
}

/// A wake that kept the session's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Woke {
    /// The name printed in `backgrounded · <id> · <name>`, when present.
    pub name: Option<String>,
    /// The stderr `note:` lines, trimmed.
    pub notes: Vec<String>,
}

/// Why a wake did not keep the session's id.
#[derive(Debug, thiserror::Error)]
pub enum WakeError {
    /// The request is malformed; nothing was run.
    #[error("{0}")]
    Invalid(String),
    /// The session's folder no longer exists; nothing was run.
    #[error("the session's folder {} no longer exists", .0.display())]
    CwdMissing(PathBuf),
    /// Another tower recorded the session under another account since
    /// this run read the sidecar (doc/0017 section 8); nothing was run.
    #[error("another tower recorded this session under {account} meanwhile; nothing was run")]
    OwnerChanged {
        /// The account in the other tower's record.
        account: String,
    },
    /// `--resume` could not be run or timed out.
    #[error(transparent)]
    ResumeCli(CliError),
    /// The owner has not trusted the folder (exit 1, `Workspace not
    /// trusted…`); the session was not woken.
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
    /// session; the copy is left as it is.
    #[error("--resume started a copy {new_id} instead of waking the session")]
    Copied {
        /// The copy's short id.
        new_id: String,
        /// The stderr `note:` lines.
        notes: Vec<String>,
    },
}

impl From<HandoffError> for WakeError {
    fn from(err: HandoffError) -> Self {
        match err {
            HandoffError::Untrusted { dir, message } => WakeError::Untrusted { dir, message },
            HandoffError::Failed { code, stderr } => WakeError::Failed { code, stderr },
            HandoffError::Unparsed { stdout, stderr } => WakeError::Unparsed { stdout, stderr },
            HandoffError::ResumeCli(e) => WakeError::ResumeCli(e),
            HandoffError::CwdMissing(dir) => WakeError::CwdMissing(dir),
            HandoffError::OwnerChanged { account, .. } => WakeError::OwnerChanged { account },
            // `interpret_resume` returns none of these.
            other => WakeError::Invalid(other.to_string()),
        }
    }
}

/// Validates `request`, calls `check` (an error ends the wake before
/// anything runs; the caller compares the sidecar file with what it planned
/// with), then runs [`WakeRequest::resume_command`] and interprets its
/// output as the hand-off does ([`handoff::interpret_resume`]). Blocks for
/// up to the runner's timeout.
pub fn run(
    runner: &CliRunner,
    request: &WakeRequest,
    check: &mut dyn FnMut() -> Result<(), WakeError>,
) -> Result<Woke, WakeError> {
    request.validate()?;
    check()?;
    let out = runner
        .run_line(&request.resume_command())
        .map_err(WakeError::ResumeCli)?;
    let resumed = handoff::interpret_resume(&out, &request.id, &request.cwd)?;
    match resumed.outcome {
        Outcome::Continued => Ok(Woke {
            name: resumed.name,
            notes: resumed.notes,
        }),
        Outcome::Copied { new_id } => Err(WakeError::Copied {
            new_id,
            notes: resumed.notes,
        }),
    }
}
