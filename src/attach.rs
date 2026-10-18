//! Handing the terminal to `<account command> attach <id>` and taking it back
//! (doc/0005).
//!
//! The procedure is written against two small traits so it can be tested
//! without a TTY: [`Handover`] releases and reclaims the terminal (the real
//! one lives in `tui`), and [`Spawner`] runs the child (the real one,
//! [`InheritSpawner`], runs it with inherited stdio and waits).

use std::io;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::app::{AttachOutcome, AttachRequest};
use crate::cli;

/// Releases and reclaims the terminal around an attach.
pub trait Handover {
    /// Stops using the terminal: drain pending input, leave the alternate
    /// screen and raw mode, show the cursor, flush (doc/0005 steps 1–4).
    fn release(&mut self) -> io::Result<()>;
    /// Prints one plain line on the normal screen.
    fn announce(&mut self, line: &str) -> io::Result<()>;
    /// Shows `prompt` and waits for any key, so an error printed by the
    /// child stays readable before tower redraws.
    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()>;
    /// Takes the terminal back: raw mode, alternate screen, hidden cursor,
    /// cleared so the next draw repaints everything.
    fn reclaim(&mut self) -> io::Result<()>;
}

/// Runs the attach child in the foreground.
pub trait Spawner {
    /// Runs `command` to completion.
    fn run(&mut self, command: Command) -> io::Result<ExitStatus>;
}

/// Runs the child with inherited stdin/stdout/stderr and waits for it.
///
/// If the child is stopped by a job-control signal (e.g. `Ctrl+Z` reaching it
/// as SIGTSTP while `claude attach` is still in cooked mode during a slow
/// wake), it is resumed with SIGCONT: tower does no job control, and a plain
/// `wait` would otherwise block forever on the stopped child.
#[derive(Debug, Default, Clone, Copy)]
pub struct InheritSpawner;

impl Spawner for InheritSpawner {
    fn run(&mut self, mut command: Command) -> io::Result<ExitStatus> {
        let child = command.spawn()?;
        wait_resuming(child.id())
    }
}

/// Waits for `pid` to exit, sending SIGCONT whenever it is stopped.
fn wait_resuming(pid: u32) -> io::Result<ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: plain syscall on our own child; `status` is a valid out-pointer.
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if rc == -1 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if libc::WIFSTOPPED(status) {
            // SAFETY: signalling our own (stopped, not yet reaped) child.
            unsafe { libc::kill(pid, libc::SIGCONT) };
            continue;
        }
        return Ok(ExitStatus::from_raw(status));
    }
}

/// The line printed before the child takes over (doc/0005 step 5).
pub fn announcement(req: &AttachRequest) -> String {
    format!(
        "tower: attaching to {} ({})… Ctrl+Z returns to tower",
        req.name, req.account.name
    )
}

/// Attaches to `req`: release the terminal, announce, run
/// `<owning account command> attach <id>` with no-op handlers for SIGINT,
/// SIGQUIT and SIGTSTP in place, then reclaim the terminal. On a non-zero exit
/// it waits for a key first, so Claude Code's error text can be read (doc/0005
/// open item: stderr stays inherited).
pub fn attach(
    handover: &mut dyn Handover,
    spawner: &mut dyn Spawner,
    req: &AttachRequest,
) -> AttachOutcome {
    let outcome = |result| AttachOutcome {
        name: req.name.clone(),
        account: req.account.name.clone(),
        result,
    };
    if let Err(err) = handover.release() {
        let _ = handover.reclaim();
        return outcome(Err(format!("cannot release the terminal: {err}")));
    }
    let _ = handover.announce(&announcement(req));

    let line = cli::attach_command(&req.account, &req.id);
    let status = {
        // A failure to install the handlers is not a reason to refuse the
        // attach; tower is then merely as exposed as without them.
        let _guard = SignalGuard::install().ok();
        spawner.run(line.to_command())
    };

    let result = match status {
        Ok(status) => Ok(status.code()),
        Err(err) => Err(format!("cannot run `{}`: {err}", line.program)),
    };
    match &result {
        Ok(Some(0)) | Err(_) => {}
        Ok(Some(code)) => {
            let _ = handover.wait_for_key(&format!(
                "tower: attach exited with code {code}. Press any key to return to tower."
            ));
        }
        Ok(None) => {
            let _ = handover.wait_for_key(
                "tower: attach was ended by a signal. Press any key to return to tower.",
            );
        }
    }
    if let Err(err) = handover.reclaim() {
        return outcome(Err(format!("cannot restore the terminal: {err}")));
    }
    outcome(result)
}

/// The signals tower must survive while a session is attached (doc/0005).
pub const GUARDED_SIGNALS: [i32; 3] = [
    signal_hook::consts::SIGINT,
    signal_hook::consts::SIGQUIT,
    signal_hook::consts::SIGTSTP,
];

/// `true` = the default action applies, `false` = no-op (attached).
static DEFAULT_ACTIVE: OnceLock<Result<Arc<AtomicBool>, String>> = OnceLock::new();

/// While alive, SIGINT, SIGQUIT and SIGTSTP are caught by a no-op handler in
/// tower, so a stray signal (e.g. `Ctrl+C` while `claude attach` is still in
/// cooked mode) cannot kill or stop tower. Dropping it restores the default
/// action.
///
/// These are real handlers, not `SIG_IGN`: caught signals are reset to the
/// default on `exec`, ignored ones would be inherited by the child (doc/0005).
///
/// signal-hook cannot put back `SIG_DFL` once its handler is installed, so the
/// handler is installed once, on the first attach, and switches between "no-op"
/// and "emulate the default action" with a flag
/// (`signal_hook::flag::register_conditional_default`).
pub struct SignalGuard {
    flag: Arc<AtomicBool>,
}

impl SignalGuard {
    /// Switches the guarded signals to no-op until the guard is dropped.
    pub fn install() -> io::Result<SignalGuard> {
        let registered = DEFAULT_ACTIVE.get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(true));
            for sig in GUARDED_SIGNALS {
                signal_hook::flag::register_conditional_default(sig, Arc::clone(&flag))
                    .map_err(|e| e.to_string())?;
            }
            Ok(flag)
        });
        let flag = registered
            .as_ref()
            .map_err(|e| io::Error::other(e.clone()))?;
        flag.store(false, Ordering::SeqCst);
        Ok(SignalGuard {
            flag: Arc::clone(flag),
        })
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}
