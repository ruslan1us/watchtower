//! Running `<account command> <args…>` (doc/0001, doc/0003).
//!
//! Background calls run with stdin null and stdout/stderr captured, so nothing
//! tower spawns can touch the terminal (doc/0005), and with a timeout, because
//! any call may first start a supervisor (doc/0001). The calls block; the UI
//! runs them on worker threads.
//!
//! Each background call runs in a process group of its own, outside tower's
//! foreground group, so `Ctrl+C` or `Ctrl+Z` typed into an attached session or
//! the editor never reaches an in-flight poll, `stop`, `rm`, dispatch or
//! hand-off. On a timeout the whole group is killed, so a wrapper's own
//! children (e.g. the real `claude` behind a script) die with it. The output
//! kept from each pipe is capped ([`OUTPUT_CAP`]).
//!
//! `attach` is the exception: it takes over the terminal, so this module only
//! builds its command line ([`attach_command`]) and the caller runs it with
//! inherited stdio. The same goes for the interactive trust flow of doc/0008
//! ([`crate::dispatch::trust_command`]).
//!
//! Every command tower spawns, background or foreground, runs without an
//! inherited `CLAUDE_CONFIG_DIR` ([`CommandLine::to_command`]): the account
//! command alone decides the account (doc/0017).
//!
//! A [`CommandLine`] may carry a working directory ([`CommandLine::cwd`]):
//! `claude --bg` starts the session in the current directory, so dispatch
//! runs in the project folder (doc/0008).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Account;
use crate::listing::{self, Listing};

/// Default timeout for a background CLI call. Generous, because a call may
/// start a supervisor first (`Starting background service…`).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for the pipes to close after the child has exited.
const EOF_GRACE: Duration = Duration::from_millis(500);

/// The most output kept from one pipe of a background call. Beyond it the
/// oldest bytes are dropped, so the last [`OUTPUT_CAP`] bytes remain (for
/// `logs`, the end of the stream is what the screen shows).
pub const OUTPUT_CAP: usize = 4 * 1024 * 1024;

/// A program plus arguments, ready to turn into a [`Command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// The program, resolved through `PATH` unless it contains a `/`.
    pub program: String,
    /// Arguments: the account command's own leading words, then the action's.
    pub args: Vec<String>,
    /// Working directory for the child; `None` inherits tower's.
    pub cwd: Option<PathBuf>,
}

impl CommandLine {
    /// The command line for `<account command> <args…>`.
    pub fn for_account<I, S>(account: &Account, args: I) -> CommandLine
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let (program, leading) = account
            .argv()
            .split_first()
            .expect("account argv is never empty");
        let mut all = leading.to_vec();
        all.extend(args.into_iter().map(Into::into));
        CommandLine {
            program: program.clone(),
            args: all,
            cwd: None,
        }
    }

    /// The same line, run in `dir`.
    pub fn with_cwd(mut self, dir: impl Into<PathBuf>) -> CommandLine {
        self.cwd = Some(dir.into());
        self
    }

    /// A [`Command`] for this line. The working directory is set when
    /// [`CommandLine::cwd`] is; no stdio is set, so by default the child
    /// inherits it. The environment is tower's, without
    /// [`CONFIG_DIR_VAR`]: an inherited value would send a plain `claude`
    /// to another account's config dir (doc/0017, environment hygiene). A
    /// wrapper command that sets the variable itself still does.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd.env_remove(CONFIG_DIR_VAR);
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        cmd
    }

    /// The full argv: program followed by arguments.
    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

/// The variable that selects Claude Code's config dir, i.e. the account
/// (doc/0003). Tower removes it from every command it runs
/// ([`CommandLine::to_command`]).
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// The startup warning when tower itself was started with
/// [`CONFIG_DIR_VAR`] set to `value` (doc/0017, environment hygiene). An
/// unset or empty value means the default config dir and gives no warning.
pub fn inherited_config_dir_warning(value: Option<&std::ffi::OsStr>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    Some(format!(
        "tower was started with {CONFIG_DIR_VAR}={}; tower removes it from every command it \
         runs, so each account uses its own command (set the variable in an account's \
         command to use another config dir)",
        value.to_string_lossy()
    ))
}

/// `<account command> attach <id>` (doc/0005). Always the short `id`, never
/// the name. The caller runs it in the foreground with inherited stdio.
pub fn attach_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["attach", id])
}

/// `<account command> --resume` in `dir`, with no session id and no other
/// argument: Claude Code's own interactive picker of past conversations
/// (doc/0019). Run in the foreground like an attach.
pub fn resume_picker_command(account: &Account, dir: &Path) -> CommandLine {
    CommandLine::for_account(account, ["--resume"]).with_cwd(dir)
}

/// The arguments of the listing call: `agents --json --all` (doc/0001).
pub const LIST_ARGS: [&str; 3] = ["agents", "--json", "--all"];

/// Captured result of a finished call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    /// Exit code; `None` when the process was killed by a signal.
    pub code: Option<i32>,
    /// Raw stdout bytes (for `logs` this is a terminal byte stream).
    pub stdout: Vec<u8>,
    /// stderr, decoded lossily as UTF-8.
    pub stderr: String,
}

impl CliOutput {
    /// True when the process exited with code 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout decoded lossily as UTF-8.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// A call that did not produce a [`CliOutput`].
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The program could not be started (e.g. not found on `PATH`).
    #[error("cannot run `{program}`: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    /// Waiting for the process failed.
    #[error("error waiting for `{program}`: {source}")]
    Wait {
        program: String,
        source: std::io::Error,
    },
    /// The call took longer than the timeout and was killed.
    #[error("`{program}` timed out after {timeout:?}")]
    Timeout {
        program: String,
        timeout: Duration,
        /// Whatever stderr was written before the kill.
        stderr: String,
    },
    /// The worker thread running the call could not be started or panicked
    /// (`jobs`), so the call's own result never came back; whether it ran is
    /// unknown.
    #[error("the {job} worker failed: {message}")]
    Worker {
        /// Which job, e.g. `logs`.
        job: String,
        /// Why: the spawn error or the panic message.
        message: String,
    },
}

/// Runs background CLI calls.
///
/// The `PATH` used to find the account command can be overridden for tests
/// ([`CliRunner::with_path`]); otherwise the child inherits tower's
/// environment, minus [`CONFIG_DIR_VAR`] ([`CommandLine::to_command`]),
/// and tower adds no variables of its own (doc/0002, doc/0016).
#[derive(Debug, Clone)]
pub struct CliRunner {
    timeout: Duration,
    path: Option<OsString>,
}

impl Default for CliRunner {
    fn default() -> Self {
        CliRunner {
            timeout: DEFAULT_TIMEOUT,
            path: None,
        }
    }
}

impl CliRunner {
    /// A runner with [`DEFAULT_TIMEOUT`] and the inherited `PATH`.
    pub fn new() -> CliRunner {
        CliRunner::default()
    }

    /// Sets the timeout for each call.
    pub fn with_timeout(mut self, timeout: Duration) -> CliRunner {
        self.timeout = timeout;
        self
    }

    /// Uses `path` as the child's `PATH` (and so for finding the program).
    /// Meant for tests that put a fake `claude` first on `PATH` (doc/0015).
    pub fn with_path(mut self, path: impl Into<OsString>) -> CliRunner {
        self.path = Some(path.into());
        self
    }

    /// The timeout applied to each call.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Runs `<account command> <args…>` and waits for it, up to the timeout.
    pub fn run<I, S>(&self, account: &Account, args: I) -> Result<CliOutput, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.run_line(&CommandLine::for_account(account, args))
    }

    /// Like [`CliRunner::run`], with `dir` as the working directory.
    pub fn run_in<I, S>(
        &self,
        account: &Account,
        dir: &Path,
        args: I,
    ) -> Result<CliOutput, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.run_line(&CommandLine::for_account(account, args).with_cwd(dir))
    }

    /// Runs a prepared command line with stdin null and captured output, in
    /// [`CommandLine::cwd`] when set, in a new process group of its own.
    pub fn run_line(&self, line: &CommandLine) -> Result<CliOutput, CliError> {
        use std::os::unix::process::CommandExt;
        let mut cmd = line.to_command();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        let mut child = cmd.spawn().map_err(|source| CliError::Spawn {
            program: line.program.clone(),
            source,
        })?;
        let stdout = Collector::start(child.stdout.take().expect("stdout is piped"));
        let stderr = Collector::start(child.stderr.take().expect("stderr is piped"));
        let deadline = Instant::now() + self.timeout;

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(&mut child);
                    let _ = child.wait();
                    return Err(CliError::Timeout {
                        program: line.program.clone(),
                        timeout: self.timeout,
                        stderr: String::from_utf8_lossy(&stderr.take()).into_owned(),
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(source) => {
                    kill_group(&mut child);
                    let _ = child.wait();
                    return Err(CliError::Wait {
                        program: line.program.clone(),
                        source,
                    });
                }
            }
        };

        // A grandchild (e.g. a freshly started supervisor) may keep the pipes
        // open after the child exits, so EOF may never come. Take what was
        // written after a short grace instead of waiting for it.
        let grace_end = Instant::now() + EOF_GRACE;
        stdout.wait(grace_end.saturating_duration_since(Instant::now()));
        stderr.wait(grace_end.saturating_duration_since(Instant::now()));
        Ok(CliOutput {
            code: status.code(),
            stdout: stdout.take(),
            stderr: String::from_utf8_lossy(&stderr.take()).into_owned(),
        })
    }

    /// `<account command> agents --json --all`, parsed (doc/0001).
    ///
    /// A non-zero exit is an error even when stdout looks like JSON; the
    /// caller keeps the last good snapshot and marks it stale.
    pub fn list_sessions(&self, account: &Account) -> Result<Listing, ListError> {
        let out = self.run(account, LIST_ARGS)?;
        if !out.success() {
            return Err(ListError::Exit {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        Ok(listing::parse_listing(&out.stdout_text())?)
    }
}

/// Why a listing call failed.
#[derive(Debug, thiserror::Error)]
pub enum ListError {
    /// The call could not run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
    /// The call exited non-zero (or was killed by a signal: `code` is `None`).
    #[error("listing failed (exit {}): {stderr}", code.map_or("signal".to_owned(), |c| c.to_string()))]
    Exit { code: Option<i32>, stderr: String },
    /// The output could not be parsed as a listing.
    #[error(transparent)]
    Parse(#[from] listing::ParseError),
}

/// Kills the process group of `child` (its own, see
/// [`CliRunner::run_line`]) with SIGKILL, then the child itself in case the
/// group could not be signalled. The caller reaps it with `wait`.
fn kill_group(child: &mut std::process::Child) {
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: plain syscall; the child is not reaped yet, so its pid
        // (which is also its group id) cannot have been reused.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    let _ = child.kill();
}

/// What a [`Collector`] has read so far.
#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    /// The result was taken: keep draining the pipe, but discard the data.
    taken: bool,
}

impl Captured {
    /// Appends `chunk`, keeping at most the last `cap` bytes (amortised:
    /// the front is cut only once the buffer reaches twice the cap).
    fn push(&mut self, chunk: &[u8], cap: usize) {
        if self.taken {
            return;
        }
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() >= cap.saturating_mul(2) {
            self.trim(cap);
        }
    }

    fn trim(&mut self, cap: usize) {
        let excess = self.bytes.len().saturating_sub(cap);
        self.bytes.drain(..excess);
    }
}

/// Reads a pipe on its own thread into a shared buffer, so the output read so
/// far is available even if EOF never comes. At most the last `cap` bytes are
/// kept; after [`Collector::take`] the pipe is still drained (so the writer
/// never blocks on a full pipe) but nothing more is stored.
struct Collector {
    buf: Arc<Mutex<Captured>>,
    done: Receiver<()>,
    cap: usize,
}

impl Collector {
    fn start(pipe: impl Read + Send + 'static) -> Collector {
        Collector::with_cap(pipe, OUTPUT_CAP)
    }

    fn with_cap(mut pipe: impl Read + Send + 'static, cap: usize) -> Collector {
        let buf = Arc::new(Mutex::new(Captured::default()));
        let (tx, done) = mpsc::channel();
        let shared = Arc::clone(&buf);
        thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(&chunk[..n], cap),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        });
        Collector { buf, done, cap }
    }

    fn wait(&self, timeout: Duration) {
        let _ = self.done.recv_timeout(timeout);
    }

    /// The output read so far (at most the cap); later output is discarded.
    fn take(&self) -> Vec<u8> {
        let mut captured = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        captured.trim(self.cap);
        captured.taken = true;
        std::mem::take(&mut captured.bytes)
    }
}
