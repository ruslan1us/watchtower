//! `!`: a shell in the selected session's folder (doc/0037). Tower hands
//! the terminal over exactly as for an attach ([`attach::run_foreground`],
//! doc/0005) and runs `$SHELL` (else `/bin/sh`) there, with no arguments
//! and no variables of its own; when the shell exits, tower takes the
//! terminal back and polls.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::attach::{self, Foreground, Handover, Spawner, Termination};
use crate::cli::CommandLine;

/// The shell when `$SHELL` is unset or empty.
pub const FALLBACK_SHELL: &str = "/bin/sh";

/// A shell to run in a session's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRequest {
    /// The folder, as the session lists it.
    pub dir: PathBuf,
    /// The folder as shown to the user (`~` form).
    pub folder: String,
}

/// How the shell ended, reported back by the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOutcome {
    /// The folder, as shown to the user.
    pub folder: String,
    pub result: ShellResult,
}

/// See [`ShellOutcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellResult {
    /// The folder no longer exists: nothing ran and the terminal was not
    /// handed over.
    FolderGone,
    /// As [`crate::app::AttachOutcome::result`].
    Ran(Result<Option<i32>, String>),
}

/// The shell program: `$SHELL` (passed in as `shell`) when set and not
/// empty, else [`FALLBACK_SHELL`].
pub fn program(shell: Option<&OsStr>) -> String {
    shell.filter(|s| !s.is_empty()).map_or_else(
        || FALLBACK_SHELL.to_owned(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// The shell from the environment's `$SHELL`.
pub fn program_from_env() -> String {
    program(std::env::var_os("SHELL").as_deref())
}

/// `<shell>` with no arguments in `dir`. On the terminal it is
/// interactive by itself. Its environment is tower's, without
/// `CLAUDE_CONFIG_DIR`, as for every command tower runs
/// ([`CommandLine::to_command`]).
pub fn command(shell: &str, dir: &Path) -> CommandLine {
    CommandLine {
        program: shell.to_owned(),
        args: Vec::new(),
        cwd: None,
    }
    .with_cwd(dir)
}

/// The line printed before the shell takes over.
pub fn announcement(folder: &str) -> String {
    format!("tower: shell in {folder} — type exit to return to tower")
}

/// An interactive shell with job control moves itself into a process group
/// of its own and makes that the terminal's foreground group; it gives the
/// terminal back when it exits normally. One that dies abnormally does
/// not, and tower would be left in a background group, stopped by its
/// next `tcsetattr`. So right after the shell has been reaped, tower makes
/// its own group the foreground group again ([`take_foreground`]), then
/// goes on as usual.
struct ForegroundBack<'a> {
    inner: &'a mut dyn Handover,
}

impl Handover for ForegroundBack<'_> {
    fn release(&mut self) -> io::Result<()> {
        self.inner.release()
    }
    fn announce(&mut self, line: &str) -> io::Result<()> {
        self.inner.announce(line)
    }
    fn child_exited(&mut self, failed: bool) {
        take_foreground();
        self.inner.child_exited(failed);
    }
    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()> {
        self.inner.wait_for_key(prompt)
    }
    fn reclaim(&mut self) -> io::Result<()> {
        self.inner.reclaim()
    }
}

/// Makes tower's process group the foreground group of the terminal on
/// stdin again, when stdin is a terminal and another group holds it.
/// `SIGTTOU` is blocked meanwhile, as `tcsetpgrp` from a background group
/// requires. A no-op in the usual case, where the shell gave the terminal
/// back itself.
pub fn take_foreground() {
    // SAFETY: plain syscalls on fd 0 and this thread's signal mask; every
    // pointer is to a local that outlives the call.
    unsafe {
        if libc::isatty(libc::STDIN_FILENO) != 1 {
            return;
        }
        let own = libc::getpgrp();
        if libc::tcgetpgrp(libc::STDIN_FILENO) == own {
            return;
        }
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGTTOU);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut old) != 0 {
            return;
        }
        libc::tcsetpgrp(libc::STDIN_FILENO, own);
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

/// Runs `shell` in `req`'s folder through [`attach::run_foreground`]: the
/// same release, announcement, no-op signal handlers, saved terminal
/// settings, key pause after a non-zero exit and reclaim as an attach, and
/// tower's process group back in the foreground first ([`take_foreground`]).
/// A folder that no longer exists runs nothing and keeps the terminal.
pub fn run(
    handover: &mut dyn Handover,
    spawner: &mut dyn Spawner,
    req: &ShellRequest,
    shell: &str,
    termination: &Termination,
) -> ShellOutcome {
    let folder = req.folder.clone();
    if !req.dir.is_dir() {
        return ShellOutcome {
            folder,
            result: ShellResult::FolderGone,
        };
    }
    let line = command(shell, &req.dir);
    let fg = Foreground {
        line: &line,
        announce: Some(announcement(&req.folder)),
        label: "shell",
        pause_on_failure: true,
    };
    let mut handover = ForegroundBack { inner: handover };
    ShellOutcome {
        folder,
        result: ShellResult::Ran(attach::run_foreground(
            &mut handover,
            spawner,
            &fg,
            termination,
        )),
    }
}
