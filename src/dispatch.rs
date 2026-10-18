//! Starting a background session from the new-session dialog (doc/0008).
//!
//! A [`DispatchRequest`] holds the dialog's fields. [`DispatchRequest::validate`]
//! checks them, [`DispatchRequest::command_line`] builds
//!
//! ```text
//! <account command> --bg [--name N] [--model M] [--effort E] --permission-mode P <task>
//! ```
//!
//! with the project as working directory, and [`run`] executes it and parses
//! `backgrounded · <id> · <name>` from stdout. The task is always a single
//! argument; nothing goes through a shell.
//!
//! In an untrusted folder `--bg` fails ([`DispatchError::Untrusted`]); the UI
//! then runs [`trust_command`] with inherited stdio so the user accepts trust
//! in Claude Code's own dialog, and retries (doc/0008, doc/0016).
//!
//! The option lists the dialog offers are [`MODEL_CHOICES`],
//! [`EFFORT_CHOICES`] and [`permission_modes`]. After a successful dispatch
//! the caller records it with [`record_dispatch`].

use std::path::{Path, PathBuf};

use crate::cli::{CliError, CliOutput, CliRunner, CommandLine};
use crate::config::Account;
use crate::sidecar::Sidecar;

/// The value that omits `--model` or `--effort` (doc/0002, doc/0008).
pub const DEFAULT_CHOICE: &str = "default";

/// Model choices of the dialog; free text is accepted as well (doc/0008).
pub const MODEL_CHOICES: [&str; 5] = [DEFAULT_CHOICE, "fable", "opus", "sonnet", "haiku"];

/// Effort choices of the dialog (doc/0008).
pub const EFFORT_CHOICES: [&str; 6] = [DEFAULT_CHOICE, "low", "medium", "high", "xhigh", "max"];

/// Every permission mode, in dialog order (doc/0008). There is no `default`
/// entry: `--permission-mode` is always passed.
pub const PERMISSION_MODES: [&str; 6] = [
    "auto",
    "acceptEdits",
    "manual",
    "plan",
    "dontAsk",
    BYPASS_PERMISSIONS,
];

/// The built-in permission mode (doc/0008, doc/0016).
pub const DEFAULT_PERMISSION_MODE: &str = "auto";

/// The mode Claude Code refuses when running as root.
pub const BYPASS_PERMISSIONS: &str = "bypassPermissions";

/// Minimum task length in characters after trimming. Documented for
/// agent-view dispatch, not verified for `--bg` (reference doc).
pub const MIN_TASK_CHARS: usize = 4;

/// The text [`haiku_warning`] returns.
pub const HAIKU_AUTO_WARNING: &str = "auto mode is unavailable on Haiku 4.5 and older models; \
     the session will silently start in manual mode";

/// True when tower runs as root (effective uid 0).
pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The permission modes to offer: all of [`PERMISSION_MODES`], without
/// `bypassPermissions` when `root` (Claude Code refuses it there).
pub fn permission_modes(root: bool) -> Vec<&'static str> {
    PERMISSION_MODES
        .iter()
        .copied()
        .filter(|m| !(root && *m == BYPASS_PERMISSIONS))
        .collect()
}

/// True when `model` names a Haiku model of version 4.5 or older: the alias
/// `haiku`, or a full name containing `haiku` with such a version, e.g.
/// `claude-haiku-4-5-20251001` or `claude-3-5-haiku-20241022`. A Haiku name
/// without a recognisable version counts as old. Case-insensitive.
pub fn is_old_haiku(model: &str) -> bool {
    let lower = model.trim().to_lowercase();
    if !lower.contains("haiku") {
        return false;
    }
    // Version numbers are the short numeric words; 8-digit dates are skipped.
    let mut numbers = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty() && w.len() <= 2 && w.bytes().all(|b| b.is_ascii_digit()))
        .map(|w| w.parse::<u32>().unwrap_or(0));
    match numbers.next() {
        None => true,
        Some(major) => {
            let minor = numbers.next().unwrap_or(0);
            (major, minor) <= (4, 5)
        }
    }
}

/// The dialog warning for a Haiku 4.5-or-older model with `auto` mode
/// (doc/0008): auto mode is unavailable there and the session silently
/// starts in manual mode. `None` for any other combination.
pub fn haiku_warning(model: Option<&str>, permission_mode: &str) -> Option<&'static str> {
    let model = model?;
    (permission_mode == "auto" && is_old_haiku(model)).then_some(HAIKU_AUTO_WARNING)
}

/// The fields of the new-session dialog (doc/0008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    /// Project folder; the session starts here (its `cwd`).
    pub project: PathBuf,
    /// The task, passed as one argument after trimming.
    pub task: String,
    /// Optional session name (`--name`); blank means none.
    pub name: Option<String>,
    /// The account whose command starts the session.
    pub account: Account,
    /// `--model`; `None`, blank or `default` omits the flag.
    pub model: Option<String>,
    /// `--effort`; `None`, blank or `default` omits the flag.
    pub effort: Option<String>,
    /// `--permission-mode`, always passed (built-in default `auto`).
    pub permission_mode: String,
}

/// Why a [`DispatchRequest`] is not valid. The message is meant for the
/// dialog.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// No project chosen.
    #[error("choose a project")]
    NoProject,
    /// The project folder does not exist (or is not a folder).
    #[error("project folder {0} does not exist")]
    ProjectMissing(PathBuf),
    /// The task is shorter than [`MIN_TASK_CHARS`] after trimming.
    #[error("the task must be at least {min} characters")]
    TaskTooShort { min: usize },
    /// A value starts with `-`, so the CLI would read it as an option.
    #[error("the {field} must not start with '-'")]
    LeadingDash { field: &'static str },
    /// The name contains a line break or another control character.
    #[error("the session name must be a single line")]
    BadName,
    /// The model, effort or permission mode is blank or contains whitespace
    /// or control characters.
    #[error("invalid {field} `{value}`")]
    BadOption { field: &'static str, value: String },
    /// `bypassPermissions` while running as root.
    #[error("bypassPermissions is not available when running as root")]
    BypassAsRoot,
}

impl DispatchRequest {
    /// A request with the given project, task and account; no name, model
    /// and effort at `default`, permission mode [`DEFAULT_PERMISSION_MODE`].
    pub fn new(project: impl Into<PathBuf>, task: impl Into<String>, account: Account) -> Self {
        DispatchRequest {
            project: project.into(),
            task: task.into(),
            name: None,
            account,
            model: None,
            effort: None,
            permission_mode: DEFAULT_PERMISSION_MODE.to_owned(),
        }
    }

    /// The task as passed: trimmed.
    pub fn task_arg(&self) -> &str {
        self.task.trim()
    }

    /// The `--name` value: trimmed, `None` when blank.
    pub fn name_arg(&self) -> Option<&str> {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
    }

    /// The `--model` value: trimmed, `None` when blank or `default`.
    pub fn model_arg(&self) -> Option<&str> {
        choice_arg(self.model.as_deref())
    }

    /// The `--effort` value: trimmed, `None` when blank or `default`.
    pub fn effort_arg(&self) -> Option<&str> {
        choice_arg(self.effort.as_deref())
    }

    /// The `--permission-mode` value, trimmed.
    pub fn permission_mode_arg(&self) -> &str {
        self.permission_mode.trim()
    }

    /// The dialog's Haiku warning for this request, see [`haiku_warning`].
    pub fn haiku_warning(&self) -> Option<&'static str> {
        haiku_warning(self.model_arg(), self.permission_mode_arg())
    }

    /// Checks the fields, with the root check done for the current process
    /// ([`is_root`]). Returns the first problem found.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validate_for(is_root())
    }

    /// Like [`DispatchRequest::validate`], with `root` given explicitly.
    ///
    /// Rules: a project that exists as a folder; a task of at least
    /// [`MIN_TASK_CHARS`] characters after trimming; an optional single-line
    /// name; option values without whitespace; no value starting with `-`
    /// (the CLI would take it for an option); no `bypassPermissions` as
    /// root. The model is not checked against a list: free text is allowed
    /// and `--bg` does not validate it (doc/0008).
    pub fn validate_for(&self, root: bool) -> Result<(), ValidationError> {
        if self.project.as_os_str().is_empty() {
            return Err(ValidationError::NoProject);
        }
        if !self.project.is_dir() {
            return Err(ValidationError::ProjectMissing(self.project.clone()));
        }
        let task = self.task_arg();
        if task.chars().count() < MIN_TASK_CHARS {
            return Err(ValidationError::TaskTooShort {
                min: MIN_TASK_CHARS,
            });
        }
        if task.starts_with('-') {
            return Err(ValidationError::LeadingDash { field: "task" });
        }
        if let Some(name) = self.name_arg() {
            if name.chars().any(char::is_control) {
                return Err(ValidationError::BadName);
            }
            if name.starts_with('-') {
                return Err(ValidationError::LeadingDash {
                    field: "session name",
                });
            }
        }
        check_option("model", self.model_arg())?;
        check_option("effort", self.effort_arg())?;
        let mode = self.permission_mode_arg();
        check_option("permission mode", Some(mode))?;
        if mode.is_empty() {
            return Err(ValidationError::BadOption {
                field: "permission mode",
                value: String::new(),
            });
        }
        if root && mode == BYPASS_PERMISSIONS {
            return Err(ValidationError::BypassAsRoot);
        }
        Ok(())
    }

    /// The arguments after the account command:
    /// `--bg [--name N] [--model M] [--effort E] --permission-mode P <task>`.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--bg".to_owned()];
        let mut push = |flag: &str, value: Option<&str>| {
            if let Some(v) = value {
                args.push(flag.to_owned());
                args.push(v.to_owned());
            }
        };
        push("--name", self.name_arg());
        push("--model", self.model_arg());
        push("--effort", self.effort_arg());
        push("--permission-mode", Some(self.permission_mode_arg()));
        args.push(self.task_arg().to_owned());
        args
    }

    /// The full command line, with the project as working directory.
    pub fn command_line(&self) -> CommandLine {
        CommandLine::for_account(&self.account, self.args()).with_cwd(&self.project)
    }

    /// The full argv: the account command's words, then [`Self::args`].
    pub fn argv(&self) -> Vec<String> {
        self.command_line().argv()
    }
}

fn choice_arg(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != DEFAULT_CHOICE)
}

fn check_option(field: &'static str, value: Option<&str>) -> Result<(), ValidationError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.starts_with('-') {
        return Err(ValidationError::LeadingDash { field });
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ValidationError::BadOption {
            field,
            value: value.to_owned(),
        });
    }
    Ok(())
}

/// A started session, as reported by `backgrounded · <id> · <name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatched {
    /// The short id, for `attach`, `logs`, `stop`, `rm`.
    pub id: String,
    /// The session name, when the line carries one.
    pub name: Option<String>,
}

/// Why a dispatch did not start a session.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// The request failed [`DispatchRequest::validate`]; nothing was run.
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    /// The command could not be run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
    /// The account has not trusted the folder (exit 1, `Workspace not
    /// trusted…`). Offer [`trust_command`] for `dir`, then retry.
    #[error("{message}")]
    Untrusted {
        /// The folder the dispatch ran in.
        dir: PathBuf,
        /// The CLI's message, trimmed.
        message: String,
    },
    /// Any other non-zero exit; shown verbatim with the code (doc/0008).
    #[error("claude --bg failed (exit {}): {stderr}", code.map_or("signal".to_owned(), |c| c.to_string()))]
    Failed {
        /// Exit code; `None` when killed by a signal.
        code: Option<i32>,
        /// stderr, trimmed.
        stderr: String,
    },
    /// Exit 0, but no `backgrounded · <id> …` line on stdout.
    #[error("claude --bg printed no session id: {stdout}")]
    Unparsed {
        /// stdout, trimmed.
        stdout: String,
        /// stderr, trimmed.
        stderr: String,
    },
}

/// The stderr prefix of the untrusted-folder failure (reference doc).
pub const UNTRUSTED_PREFIX: &str = "Workspace not trusted";

/// Parses the first `backgrounded · <id> · <name>` line of `stdout`,
/// ignoring the hint lines. A trailing ` (idle — …)` suffix, printed when a
/// session is woken without a prompt, is dropped from the name. Also
/// accepts the line without a name. Returns `None` when there is no such
/// line or its id is empty.
pub fn parse_backgrounded(stdout: &str) -> Option<Dispatched> {
    stdout.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("backgrounded")?;
        let rest = rest.trim_start().strip_prefix('·')?;
        let (id, name) = match rest.split_once('·') {
            Some((id, name)) => (id.trim(), Some(name.trim())),
            None => (rest.trim(), None),
        };
        if id.is_empty() || id.contains(char::is_whitespace) {
            return None;
        }
        let name = name
            .map(strip_idle_suffix)
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        Some(Dispatched {
            id: id.to_owned(),
            name,
        })
    })
}

fn strip_idle_suffix(name: &str) -> &str {
    match name.rfind(" (idle") {
        Some(i) if name.ends_with(')') => name[..i].trim_end(),
        _ => name,
    }
}

/// Interprets the output of a `--bg` call (see [`run`]). `dir` is the folder
/// it ran in.
pub fn interpret(out: &CliOutput, dir: &Path) -> Result<Dispatched, DispatchError> {
    if !out.success() {
        let untrusted = out.code == Some(1)
            && out
                .stderr
                .lines()
                .any(|l| l.trim_start().starts_with(UNTRUSTED_PREFIX));
        if untrusted {
            return Err(DispatchError::Untrusted {
                dir: dir.to_path_buf(),
                message: out.stderr.trim().to_owned(),
            });
        }
        return Err(DispatchError::Failed {
            code: out.code,
            stderr: out.stderr.trim().to_owned(),
        });
    }
    let stdout = out.stdout_text();
    parse_backgrounded(&stdout).ok_or_else(|| DispatchError::Unparsed {
        stdout: stdout.trim().to_owned(),
        stderr: out.stderr.trim().to_owned(),
    })
}

/// Validates `request`, runs it in the project folder and parses the new
/// session's id and name. Blocks for up to the runner's timeout; the UI
/// calls it on a worker thread. Does not touch the sidecar: on success the
/// caller uses [`record_dispatch`].
pub fn run(runner: &CliRunner, request: &DispatchRequest) -> Result<Dispatched, DispatchError> {
    request.validate()?;
    let out = runner.run_line(&request.command_line())?;
    interpret(&out, &request.project)
}

/// Records a successful dispatch in the sidecar (doc/0008): the account as
/// owner with source `started` (keyed by the short id until the listing
/// shows the `sessionId`, see [`Sidecar::adopt_short_id_records`]) and the
/// project at the front of `recent_projects`. Returns true when the sidecar
/// changed and is due for a save.
pub fn record_dispatch(
    sidecar: &mut Sidecar,
    request: &DispatchRequest,
    dispatched: &Dispatched,
    home: &Path,
    now_ms: i64,
) -> bool {
    let owner = sidecar.record_started(&dispatched.id, &request.account.name);
    let recent = sidecar.touch_recent_project(&request.project, home, now_ms);
    owner || recent
}

/// The interactive `<account command>` in `dir`, with no extra arguments,
/// for the trust flow (doc/0008): the UI suspends itself, runs this with
/// inherited stdio, the user accepts Claude Code's trust dialog and exits,
/// and the UI retries the dispatch. Tower never accepts trust itself
/// (doc/0016).
pub fn trust_command(account: &Account, dir: &Path) -> CommandLine {
    CommandLine::for_account(account, std::iter::empty::<String>()).with_cwd(dir)
}
