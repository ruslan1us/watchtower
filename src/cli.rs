//! Running `<account command> <args…>` (doc/0001, doc/0003).
//!
//! Background calls run with stdin null and stdout/stderr captured, so nothing
//! tower spawns can touch the terminal (doc/0005), and with a timeout, because
//! any call may first start a supervisor (doc/0001). The calls block; the UI
//! runs them on worker threads.
//!
//! `attach` is the exception: it takes over the terminal, so this module only
//! builds its command line ([`attach_command`]) and the caller runs it with
//! inherited stdio. The same goes for the interactive trust flow of doc/0008
//! ([`crate::dispatch::trust_command`]).
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
    /// [`CommandLine::cwd`] is; no stdio or environment is set, so by default
    /// the child inherits them.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
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

/// `<account command> attach <id>` (doc/0005). Always the short `id`, never
/// the name. The caller runs it in the foreground with inherited stdio.
pub fn attach_command(account: &Account, id: &str) -> CommandLine {
    CommandLine::for_account(account, ["attach", id])
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
}

/// Runs background CLI calls.
///
/// The `PATH` used to find the account command can be overridden for tests
/// ([`CliRunner::with_path`]); by default the child inherits tower's
/// environment unchanged and tower adds no variables of its own (doc/0002,
/// doc/0016).
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
    /// [`CommandLine::cwd`] when set.
    pub fn run_line(&self, line: &CommandLine) -> Result<CliOutput, CliError> {
        let mut cmd = line.to_command();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CliError::Timeout {
                        program: line.program.clone(),
                        timeout: self.timeout,
                        stderr: String::from_utf8_lossy(&stderr.take()).into_owned(),
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(source) => {
                    let _ = child.kill();
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

/// Reads a pipe on its own thread into a shared buffer, so the output read so
/// far is available even if EOF never comes.
struct Collector {
    buf: Arc<Mutex<Vec<u8>>>,
    done: Receiver<()>,
}

impl Collector {
    fn start(mut pipe: impl Read + Send + 'static) -> Collector {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, done) = mpsc::channel();
        let shared = Arc::clone(&buf);
        thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend_from_slice(&chunk[..n]),
                }
            }
            let _ = tx.send(());
        });
        Collector { buf, done }
    }

    fn wait(&self, timeout: Duration) {
        let _ = self.done.recv_timeout(timeout);
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.buf.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
