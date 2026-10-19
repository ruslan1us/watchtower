//! The terminal runtime: sets up and restores the terminal, runs the event
//! loop, feeds key presses, poll results and finished background jobs to
//! [`App`], and carries out the [`Effect`]s it returns (doc/0004, doc/0005,
//! doc/0008, doc/0009, doc/0014 milestones 1–3). Slow CLI calls, including
//! the hand-off sequence, and file work (sidecar saves and reloads, the
//! project scan) run on worker threads ([`Jobs`]), and `claude logs` in the
//! log follower ([`Follower`], doc/0030), which keeps following working
//! sessions during every hand-over of the terminal; attach, the task
//! editor, the trust flow and Claude Code's picker of past conversations
//! (`R`, doc/0019) take over the terminal through the same hand-over
//! ([`attach::run_foreground`]).
//!
//! The terminal is restored on normal exit, on error (a drop guard), on a
//! panic of the main thread (a panic hook; other threads' panics are only
//! recorded, see [`jobs`]) and on SIGTERM or SIGHUP: their handlers only set
//! a flag ([`attach::Termination`]), and the event loop checks it every tick and
//! ends, so tower exits with 128 + signal. After SIGHUP the terminal may be
//! gone, so only the terminal settings are put back and no escape sequences
//! are written. A terminal whose input reaches EOF, fails or hangs up counts
//! as SIGHUP, whether or not the signal arrives ([`wait_input`]); every loop
//! that waits for input checks the flag on each turn. During an attach the
//! flag waits until the child has exited (doc/0005). When a hand-off runs,
//! tower first waits up to [`HANDOFF_EXIT_WAIT`] for its result, records it
//! and saves the sidecar ([`finish_handoff_for_exit`], doc/0004).
//!
//! The terminal settings from before the first raw mode are saved once and
//! put back after every foreground child and on exit, so a child that died
//! in raw mode cannot leave the user's shell in it.
//! Tower never writes BEL or OSC notifications (doc/0007). It enables no
//! mouse capture or bracketed paste; where the terminal supports the
//! keyboard-enhancement protocol it pushes `DISAMBIGUATE_ESCAPE_CODES`, so
//! `Ctrl+Enter` is reported distinctly in the new-session dialog (doc/0011),
//! and pops it again before every hand-over and on exit.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Stdout, Write};
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};

use crate::app::{App, Effect, LogsTarget, Severity};
use crate::attach::{self, Foreground, Handover, InheritSpawner, Termination};
use crate::cli::{CliError, CliRunner};
use crate::config::{Config, ConfigSource};
use crate::config_edit;
use crate::dispatch::DispatchError;
use crate::jobs::{self, JobResult, Jobs};
use crate::logfollow::{FollowSettings, Follower};
use crate::merge::ProcEnviron;
use crate::orchestrator::StartError;
use crate::paths::{self, Dirs};
use crate::poller::Poller;
use crate::sidecar::Loaded;
use crate::{dispatch, editor};

/// How long the loop waits for input before checking for poll results and
/// redrawing (the status line shows the age of the last refresh).
const TICK: Duration = Duration::from_millis(200);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Whether tower pushed keyboard-enhancement flags that are still active, so
/// every way out (normal exit, panic, hand-over) pops them exactly once.
static ENHANCED: AtomicBool = AtomicBool::new(false);

/// The keyboard-enhancement flags tower asks for: only the disambiguation of
/// modified keys such as `Ctrl+Enter` (doc/0011).
fn enhancement_flags() -> PushKeyboardEnhancementFlags {
    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
}

/// Pushes the flags when the terminal supports them. Raw mode must be on
/// (the support query reads the terminal's answer).
fn push_enhancement(supported: bool) {
    if supported && execute!(io::stdout(), enhancement_flags()).is_ok() {
        ENHANCED.store(true, Ordering::SeqCst);
    }
}

/// Pops the flags if tower pushed them; returns whether it did.
fn pop_enhancement() -> bool {
    let was = ENHANCED.swap(false, Ordering::SeqCst);
    if was {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    was
}

/// The file descriptor crossterm reads input from: stdin, which [`run`]
/// requires to be a terminal.
const INPUT_FD: RawFd = libc::STDIN_FILENO;

/// What [`wait_input`] saw on an input descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Input is waiting to be read.
    Ready,
    /// Nothing arrived within the timeout, or a signal interrupted the
    /// wait (so the caller checks the termination flag at once).
    Idle,
    /// The terminal is gone: hang-up, error, closed or invalid descriptor,
    /// or a failed `poll`.
    HangUp,
}

/// Waits up to `timeout` for input on `fd` with `poll(2)`, without reading.
///
/// This exists because crossterm's reader retries forever, without
/// sleeping, when a read returns EOF or an error, which is what a terminal
/// does once it has hung up (e.g. its tmux server or SSH connection is
/// gone): tower would spin at full CPU and never see a SIGTERM. Tower
/// therefore only calls crossterm once `poll` says input is ready, and treats
/// `POLLHUP`, `POLLERR` and `POLLNVAL` as a hang-up (doc/0005).
pub fn wait_input(fd: RawFd, timeout: Duration) -> Input {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid, initialised pollfd that outlives the call.
    let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
    if rc < 0 {
        return if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            Input::Idle
        } else {
            Input::HangUp
        };
    }
    if rc == 0 {
        Input::Idle
    } else if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        Input::HangUp
    } else if pfd.revents & libc::POLLIN != 0 {
        Input::Ready
    } else {
        Input::Idle
    }
}

/// The terminal's input is gone (EOF, an error or a hang-up). Tower then
/// ends as after SIGHUP: no more writes to the terminal, exit 129.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HungUp;

/// Turns one input wait into events, or into a hang-up recorded on
/// `termination` as SIGHUP (doc/0005): `None` means the caller must stop
/// waiting and let tower exit. Every input-waiting loop goes through this,
/// and checks `termination` on each turn.
pub fn take_input(
    termination: &Termination,
    waited: Result<Vec<Event>, HungUp>,
) -> Option<Vec<Event>> {
    match waited {
        Ok(events) => Some(events),
        Err(HungUp) => {
            termination.request(signal_hook::consts::SIGHUP);
            None
        }
    }
}

/// Waits for a key press, a pending termination or a hang-up. `next` waits
/// up to the given time for input events (the real one is [`next_events`]),
/// so a SIGTERM or SIGHUP is seen within one [`TICK`]. A hang-up is recorded
/// on `termination` and returned as `Err`.
pub fn wait_key(
    termination: &Termination,
    mut next: impl FnMut(Duration) -> Result<Vec<Event>, HungUp>,
) -> Result<(), HungUp> {
    while termination.pending().is_none() {
        let events = take_input(termination, next(TICK)).ok_or(HungUp)?;
        if events
            .iter()
            .any(|e| matches!(e, Event::Key(key) if key.kind == KeyEventKind::Press))
        {
            return Ok(());
        }
    }
    Ok(())
}

/// Set while the main thread is inside crossterm's input code (see
/// [`start_hangup_watchdog`]).
static IN_CROSSTERM_INPUT: AtomicBool = AtomicBool::new(false);

/// Runs `f`, a call into crossterm's input code, marked for the watchdog.
fn in_crossterm_input<T>(f: impl FnOnce() -> T) -> T {
    IN_CROSSTERM_INPUT.store(true, Ordering::SeqCst);
    let out = f();
    IN_CROSSTERM_INPUT.store(false, Ordering::SeqCst);
    out
}

/// Waits up to `timeout` for input and returns the events it carries: none
/// on a timeout or a signal, `Err` once the terminal is gone. crossterm is
/// only called when `poll` says input is ready ([`wait_input`]), and then
/// only with a zero timeout, to read that input and any events it has
/// already parsed.
fn next_events(timeout: Duration) -> Result<Vec<Event>, HungUp> {
    match wait_input(INPUT_FD, timeout) {
        Input::HangUp => Err(HungUp),
        Input::Idle => Ok(Vec::new()),
        Input::Ready => in_crossterm_input(|| {
            let mut events = Vec::new();
            while event::poll(Duration::ZERO).map_err(|_| HungUp)? {
                events.push(event::read().map_err(|_| HungUp)?);
            }
            Ok(events)
        }),
    }
}

/// A last resort against crossterm's spinning reader ([`wait_input`]): the
/// terminal could hang up in the instant between `poll` saying input is
/// ready and crossterm reading it. A thread checks once a second; if the
/// main thread is inside crossterm's input code and the terminal has hung
/// up, twice in a row, tower exits at once with the SIGHUP status (129),
/// writing nothing. The main thread is then only reading input, so no
/// sidecar write is cut short. Normally the hang-up is seen before crossterm
/// is called and this never fires.
fn start_hangup_watchdog() {
    let _ = thread::Builder::new()
        .name("hangup-watchdog".into())
        .spawn(|| {
            let mut strikes = 0;
            loop {
                thread::sleep(Duration::from_secs(1));
                let stuck = IN_CROSSTERM_INPUT.load(Ordering::SeqCst)
                    && wait_input(INPUT_FD, Duration::ZERO) == Input::HangUp;
                strikes = if stuck { strikes + 1 } else { 0 };
                if strikes >= 2 {
                    let status = attach::signal_exit_status(signal_hook::consts::SIGHUP);
                    // SAFETY: ends the process without running any more
                    // Rust code; nothing is left in an inconsistent state.
                    unsafe { libc::_exit(i32::from(status)) };
                }
            }
        });
}

/// Whether the terminal is gone: SIGHUP is pending or the input has hung up.
fn terminal_gone(termination: &Termination) -> bool {
    termination.pending() == Some(signal_hook::consts::SIGHUP)
        || wait_input(INPUT_FD, Duration::ZERO) == Input::HangUp
}

/// The terminal settings tower started with, saved once before it first
/// enters raw mode ([`save_original_termios`]).
static ORIGINAL_TERMIOS: OnceLock<libc::termios> = OnceLock::new();

/// Saves the current terminal settings as the ones to put back. Only the
/// first call counts: crossterm's own `enable_raw_mode` saves whatever is
/// current each time, which after a hand-over can be the raw settings of a
/// child that died, so tower keeps its own copy (doc/0005).
fn save_original_termios() {
    // SAFETY: termios is plain data; tcgetattr fills it on success.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(INPUT_FD, &mut saved) } == 0 {
        let _ = ORIGINAL_TERMIOS.set(saved);
    }
}

/// Puts back the settings saved by [`save_original_termios`], if any. An
/// `ioctl`, not a write: safe on a terminal that is gone.
fn restore_original_termios() {
    if let Some(saved) = ORIGINAL_TERMIOS.get() {
        // SAFETY: plain syscall with a valid termios.
        unsafe { libc::tcsetattr(INPUT_FD, libc::TCSANOW, saved) };
    }
}

/// Switches off what a child that died abnormally may have left on: mouse
/// tracking (normal, button, any-motion, SGR), bracketed paste, and one
/// level of keyboard enhancement.
pub const MODE_RESETS: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[<u";

/// Whether a finished background job calls for an immediate poll on top of
/// what the app asks for: a dispatch that timed out may have started its
/// session anyway, so the list is refreshed at once (doc/0008).
pub fn polls_after(done: &JobResult) -> bool {
    matches!(
        done,
        JobResult::Dispatch(_, Err(DispatchError::Cli(CliError::Timeout { .. })))
            | JobResult::Orchestrator(
                _,
                Err(StartError::Dispatch(DispatchError::Cli(
                    CliError::Timeout { .. }
                )))
            )
    )
}

/// How the TUI ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The user quit.
    Quit,
    /// A SIGTERM or SIGHUP ended it.
    Signal(i32),
}

impl Exit {
    /// The process exit status: 0, or 128 + signal.
    pub fn status(self) -> u8 {
        match self {
            Exit::Quit => 0,
            Exit::Signal(sig) => attach::signal_exit_status(sig),
        }
    }

    /// Whether restoring the terminal may write escape sequences. Not after
    /// SIGHUP: the terminal is probably gone and a write could fail or block.
    pub fn may_write_to_terminal(self) -> bool {
        self != Exit::Signal(signal_hook::consts::SIGHUP)
    }
}

/// How the event loop's end and a pending termination signal combine: a
/// pending signal wins, even over an error (after SIGHUP the terminal fails
/// first, and that error is a symptom, not the cause).
pub fn exit_for(result: io::Result<()>, pending: Option<i32>) -> io::Result<Exit> {
    match pending {
        Some(sig) => Ok(Exit::Signal(sig)),
        None => result.map(|()| Exit::Quit),
    }
}

/// How long tower waits, after a SIGTERM or SIGHUP, for a running
/// hand-off to report back before it exits (doc/0004): up to the stop wait
/// plus a `--resume` that does not have to start a supervisor.
pub const HANDOFF_EXIT_WAIT: Duration = Duration::from_secs(15);

/// The exit path after a SIGTERM or SIGHUP while a hand-off runs
/// (doc/0004): waits up to `timeout` for its result through `recv` (the
/// jobs' `recv_timeout`), feeding every job result that arrives meanwhile
/// to the app, so the `handoff` record is made as usual. Returns whether
/// the sidecar must be saved. Returns at once when no hand-off runs; never
/// touches the terminal, and the effects other than the save are dropped,
/// since tower is exiting.
pub fn finish_handoff_for_exit(
    app: &mut App,
    timeout: Duration,
    mut recv: impl FnMut(Duration) -> Option<JobResult>,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut save = false;
    while app.handoff_progress().is_some() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // None: the time is up (or no job can report any more).
        let Some(done) = recv(left) else {
            break;
        };
        save |= done.apply(app).contains(&Effect::SaveSidecar);
    }
    save
}

/// How long tower waits on exit for the sidecar saves still to be made.
pub const SAVE_EXIT_WAIT: Duration = Duration::from_secs(5);

/// The exit path of the sidecar (doc/0002): saves run on worker threads, so
/// tower waits up to `timeout` for the one running, then saves what is
/// still unsaved, at most twice. `start` runs a job on a worker, `recv` is
/// the jobs' `recv_timeout`; every job result that arrives meanwhile is
/// fed to the app and its other effects are dropped. Returns whether
/// nothing was left unsaved. Never touches the terminal.
pub fn flush_sidecar(
    app: &mut App,
    timeout: Duration,
    mut start: impl FnMut(crate::app::SidecarJob),
    mut recv: impl FnMut(Duration) -> Option<JobResult>,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut attempts = 0;
    loop {
        if !app.sidecar_busy() {
            if !app.sidecar.is_dirty() {
                return true;
            }
            if attempts >= 2 {
                return false;
            }
            if let Some(job) = app.start_save() {
                start(job);
                attempts += 1;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return !app.sidecar.is_dirty();
        }
        let Some(done) = recv(left) else {
            return !app.sidecar.is_dirty();
        };
        done.apply(app);
    }
}

/// Writes `<config dir>/claude-settings.json` for the running binary when
/// its content is not current (doc/0029). Each dispatch checks it again.
fn write_subagent_settings(config_dir: &std::path::Path) -> Result<bool, String> {
    let exe = crate::subagents::running_exe()
        .map_err(|e| format!("cannot find the running tower binary: {e}"))?;
    crate::subagents::ensure_settings(&crate::subagents::settings_path(config_dir), &exe)
}

/// Runs the TUI until the user quits or a SIGTERM/SIGHUP arrives.
/// `source` says which config file was used and whether it was found
/// (doc/0002); a missing one shows a warning.
pub fn run(config: Config, source: ConfigSource, dirs: Dirs, loaded: Loaded) -> io::Result<Exit> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "needs an interactive terminal (stdin and stdout must be a TTY)",
        ));
    }
    let state_file = dirs.state_file();
    let mut app = App::new(
        config.clone(),
        dirs.home.clone(),
        loaded.sidecar,
        Box::new(ProcEnviron),
    );
    app.set_config_dir(&dirs.config_dir);
    app.enable_subagents(&dirs.config_dir);
    if config.track_subagents
        && let Err(err) = write_subagent_settings(&dirs.config_dir)
    {
        app.add_warning(format!(
            "subagents: {err}; sessions start without the subagent hooks"
        ));
    }
    app.set_config_source(source);
    app.color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    app.style = config.ui_style;
    if let Some(warning) = loaded.warning {
        app.add_warning(warning);
    }
    let inherited = std::env::var_os(crate::cli::CONFIG_DIR_VAR);
    if let Some(warning) = crate::cli::inherited_config_dir_warning(inherited.as_deref()) {
        app.add_warning(warning);
    }

    let termination = Termination::install().unwrap_or_else(|err| {
        app.add_warning(format!("cannot handle SIGTERM/SIGHUP: {err}"));
        Termination::default()
    });

    install_panic_hook();
    save_original_termios();
    start_hangup_watchdog();
    let mut restore = RestoreGuard { write: true };
    let (mut terminal, enhanced) = enter()?;
    app.keyboard_enhanced = enhanced;

    let poller = Poller::start(
        config.accounts.clone(),
        Arc::new(CliRunner::new()),
        config.poll_interval(),
    );
    let jobs = Jobs::new(CliRunner::new()).with_state_file(state_file.clone());
    let follower = Follower::start(
        Arc::new(CliRunner::new()),
        Arc::new(ProcEnviron),
        FollowSettings::default(),
    );
    let mut rt = Runtime {
        terminal: &mut terminal,
        poller: &poller,
        jobs: &jobs,
        follower: &follower,
        state_file: &state_file,
        termination: &termination,
        enhanced,
        pending: VecDeque::new(),
    };
    let result = event_loop(&mut rt, &mut app);
    if let Some(sig) = termination.pending()
        && let Some(progress) = app.handoff_progress()
    {
        // Exiting now could leave the session stopped without having
        // continued, and would lose its `handoff` record (doc/0004).
        if Exit::Signal(sig).may_write_to_terminal() {
            let text = format!(
                "ending: waiting up to {}s for the running hand-off ({progress})",
                HANDOFF_EXIT_WAIT.as_secs()
            );
            app.set_message(text, Severity::Warning);
            let _ = rt
                .terminal
                .draw(|frame| crate::ui::draw(frame, &mut app, Instant::now()));
        }
        finish_handoff_for_exit(&mut app, HANDOFF_EXIT_WAIT, |t| jobs.recv_timeout(t));
    }
    // Saves run on worker threads: let the last ones finish (doc/0002).
    flush_sidecar(
        &mut app,
        SAVE_EXIT_WAIT,
        |job| jobs.sidecar(job, state_file.clone()),
        |t| jobs.recv_timeout(t),
    );
    let exit = exit_for(result, termination.pending());
    if let Ok(exit) = exit {
        restore.write = exit.may_write_to_terminal();
    }
    if !restore.write {
        // Dropping the ratatui terminal would write "show cursor".
        std::mem::forget(terminal);
    }
    exit
}

/// What the event loop works with.
struct Runtime<'a> {
    terminal: &'a mut Term,
    poller: &'a Poller,
    jobs: &'a Jobs,
    /// Fetches `claude logs` into each session's followed stream (doc/0030).
    follower: &'a Follower,
    state_file: &'a Path,
    termination: &'a Termination,
    /// Whether the terminal supports keyboard enhancement (re-pushed after
    /// every hand-over).
    enhanced: bool,
    /// Events read but not handled yet (one read can carry several keys).
    pending: VecDeque<Event>,
}

impl Runtime<'_> {
    fn handover(&mut self) -> TermHandover<'_> {
        TermHandover {
            terminal: self.terminal,
            termination: self.termination,
            enhanced: self.enhanced,
            pending: &mut self.pending,
        }
    }
}

/// Runs until the user quits or `termination` is pending; the flag is checked
/// before every draw, so tower stops within one [`TICK`]. When the terminal
/// is gone (input EOF, error or hang-up, or a failed draw on a hung-up
/// terminal) a SIGHUP is recorded and the loop ends, so tower exits as after
/// SIGHUP, writing nothing more (doc/0005).
fn event_loop(rt: &mut Runtime<'_>, app: &mut App) -> io::Result<()> {
    loop {
        if rt.termination.pending().is_some() {
            return Ok(());
        }
        if let Err(err) = rt
            .terminal
            .draw(|frame| crate::ui::draw(frame, app, Instant::now()))
        {
            if terminal_gone(rt.termination) {
                rt.termination.request(signal_hook::consts::SIGHUP);
                return Ok(());
            }
            return Err(err);
        }
        let mut effects = Vec::new();
        // One event per turn, as before: effects (e.g. an attach, whose
        // release drops the queue) run before the next key is handled.
        if rt.pending.is_empty() {
            let Some(events) = take_input(rt.termination, next_events(TICK)) else {
                return Ok(());
            };
            rt.pending.extend(events);
        }
        match rt.pending.pop_front() {
            Some(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                effects.extend(app.handle_key(key));
            }
            // Resize: the next draw picks up the new size.
            _ => {}
        }
        let mut polled = false;
        while let Some(update) = rt.poller.try_recv() {
            effects.extend(app.apply_poll(update));
            polled = true;
        }
        if polled {
            // Other towers' sidecar changes, looked for once per poll
            // (doc/0017 section 8), and changes to config.toml (doc/0022).
            effects.push(Effect::CheckSidecar);
            effects.push(Effect::CheckConfig);
            // The sessions to follow in the background (doc/0030).
            rt.follower.set_plan(app.follow_plan());
            // The subagents of the rows just listed (doc/0029).
            effects.push(Effect::ReadSubagents);
        }
        while let Some(message) = rt.poller.try_restart() {
            app.set_message(
                format!("polling stopped unexpectedly and was restarted: {message}"),
                Severity::Warning,
            );
        }
        while let Some((target, result)) = rt.follower.try_recv() {
            effects.extend(app.logs_fetched(target, result));
        }
        while let Some(done) = rt.jobs.try_recv() {
            if polls_after(&done) {
                effects.push(Effect::PollNow);
            }
            effects.extend(done.apply(app));
        }
        if let Some(panic) = jobs::take_stray_panic() {
            app.set_message(
                format!("a background thread failed: {panic}"),
                Severity::Error,
            );
        }
        effects.extend(app.tick(Instant::now()));
        if !run_effects(rt, app, effects)? {
            return Ok(());
        }
    }
}

/// Carries out effects in order. Returns `false` when the app should quit.
fn run_effects(rt: &mut Runtime<'_>, app: &mut App, effects: Vec<Effect>) -> io::Result<bool> {
    let mut queue: std::collections::VecDeque<Effect> = effects.into();
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::Quit => return Ok(false),
            Effect::PollNow => {
                rt.poller.poll_now();
            }
            Effect::PollForAttach => {
                let cycle = rt.poller.poll_now();
                app.attach_poll_started(cycle);
            }
            Effect::SaveSidecar => {
                if let Some(job) = app.start_save() {
                    rt.jobs.sidecar(job, rt.state_file.to_path_buf());
                }
            }
            Effect::CheckSidecar => {
                if let Some(job) = app.start_sidecar_check() {
                    rt.jobs.sidecar(job, rt.state_file.to_path_buf());
                }
            }
            Effect::ReadSubagents => {
                if let Some(job) = app.start_subagent_read(Instant::now()) {
                    rt.jobs.subagents(job);
                }
            }
            Effect::Attach(req) => {
                let termination = rt.termination;
                // Followed at the cadence while attached, so the repaint on
                // attach is caught however long the attach lasts (doc/0030).
                let target = LogsTarget {
                    account: req.account.clone(),
                    id: req.id.clone(),
                };
                rt.follower.attached(Some(app.follow_spec(&target)));
                let outcome =
                    attach::attach(&mut rt.handover(), &mut InheritSpawner, &req, termination);
                rt.follower.attached(None);
                if termination.pending().is_some() {
                    return Ok(false);
                }
                queue.extend(app.attach_finished(outcome));
            }
            Effect::OpenDialog => rt.jobs.projects(app.begin_project_scan()),
            Effect::ScanProjects(scan) => rt.jobs.projects(scan),
            Effect::ResumePast(req) => {
                let termination = rt.termination;
                let folder = paths::shorten_home(&req.folder.to_string_lossy(), app.home());
                let outcome = attach::resume_past(
                    &mut rt.handover(),
                    &mut InheritSpawner,
                    &req,
                    &folder,
                    termination,
                );
                if termination.pending().is_some() {
                    return Ok(false);
                }
                queue.extend(app.resume_finished(outcome));
            }
            Effect::Dispatch(job) => rt.jobs.dispatch(job),
            Effect::Trust(job) => {
                let req = &job.request;
                let Some(result) = run_trust(rt, app, &req.account, &req.project, "dispatch")
                else {
                    return Ok(false);
                };
                queue.extend(app.trust_finished(job, result));
            }
            Effect::HandoffTrust(job) => {
                let req = &job.request;
                let Some(result) = run_trust(rt, app, &req.target, &req.cwd, "hand-off") else {
                    return Ok(false);
                };
                queue.extend(app.handoff_trust_finished(job, result));
            }
            Effect::Handoff(job) => rt.jobs.handoff(job),
            Effect::StartOrchestrator(job) => rt.jobs.orchestrator(job),
            Effect::OrchestratorTrust(job) => {
                let req = &job.request;
                let Some(result) =
                    run_trust(rt, app, &req.account, &req.project, "orchestrator start")
                else {
                    return Ok(false);
                };
                queue.extend(app.orchestrator_trust_finished(job, result));
            }
            Effect::EditTask(text) => {
                let termination = rt.termination;
                let result = editor::editor_from_env().and_then(|cmd| {
                    editor::edit(
                        &mut rt.handover(),
                        &mut InheritSpawner,
                        &cmd,
                        &text,
                        &std::env::temp_dir(),
                        termination,
                    )
                });
                if termination.pending().is_some() {
                    return Ok(false);
                }
                app.editor_finished(result);
            }
            Effect::Stop(action) => rt.jobs.stop(action),
            Effect::Remove(action) => rt.jobs.remove(action),
            Effect::FetchLogs(target) => rt.follower.fetch(app.follow_spec(&target), true),
            Effect::FollowLogs(target) => rt.follower.fetch(app.follow_spec(&target), false),
            Effect::CheckConfig => {
                if let Some(job) = app.start_config_check() {
                    rt.jobs.config_check(job);
                }
            }
            Effect::SetPollInterval(interval) => rt.poller.set_interval(interval),
            Effect::SaveConfig(job) => rt.jobs.config_save(job),
            Effect::EditConfig(path) => {
                let termination = rt.termination;
                let home = app.home().to_path_buf();
                let result = config_edit::editor_from_env().and_then(|cmd| {
                    config_edit::edit_in_editor(
                        &mut rt.handover(),
                        &mut InheritSpawner,
                        &cmd,
                        &path,
                        &home,
                        termination,
                    )
                });
                if termination.pending().is_some() {
                    return Ok(false);
                }
                queue.extend(app.config_editor_finished(result));
            }
            Effect::OpenSettings(path) => {
                // A small local file, read like the config at start.
                let result = config_edit::EditSession::open(&path, app.home());
                queue.extend(app.settings_opened(result));
            }
            Effect::CheckFolders(job) => rt.jobs.folders(job),
            Effect::Rename(job) => rt.jobs.rename(job),
            Effect::RenameTrust(job) => {
                let req = &job.request;
                let Some(result) = run_trust(rt, app, &req.owner, &req.cwd, "rename") else {
                    return Ok(false);
                };
                queue.extend(app.rename_trust_finished(job, result));
            }
            Effect::PollForRename => {
                let cycle = rt.poller.poll_now();
                app.rename_poll_started(cycle);
            }
        }
    }
    Ok(true)
}

/// The trust flow of doc/0008, for a dispatch, a hand-off (doc/0004) or a
/// rename (doc/0027): hands the terminal to the interactive `<account
/// command>` in `dir` so the user accepts Claude Code's own trust prompt.
/// `None` when a termination signal arrived meanwhile and tower must exit.
fn run_trust(
    rt: &mut Runtime<'_>,
    app: &App,
    account: &crate::config::Account,
    dir: &Path,
    what: &str,
) -> Option<Result<Option<i32>, String>> {
    let line = dispatch::trust_command(account, dir);
    let shown = paths::shorten_home(&dir.to_string_lossy(), app.home());
    let fg = Foreground {
        line: &line,
        announce: Some(format!(
            "tower: running `{}` in {shown} for account {}. Accept the trust prompt, then \
             leave Claude Code with /exit (or Ctrl+C twice); tower then retries the {what}.",
            account.command, account.name
        )),
        label: "claude",
        pause_on_failure: false,
    };
    let termination = rt.termination;
    let result = attach::run_foreground(&mut rt.handover(), &mut InheritSpawner, &fg, termination);
    if termination.pending().is_some() {
        return None;
    }
    Some(result)
}

/// Raw mode, alternate screen, hidden cursor, and keyboard enhancement where
/// supported. Returns the terminal and whether enhancement is supported.
fn enter() -> io::Result<(Term, bool)> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    // The support query reads the terminal's answer through crossterm.
    let supported = in_crossterm_input(|| supports_keyboard_enhancement().unwrap_or(false));
    push_enhancement(supported);
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    Ok((terminal, supported))
}

/// Undoes [`enter`]; safe to call more than once and in any state. The
/// terminal settings go back to those saved at startup, not to whatever
/// crossterm saved last (doc/0005).
pub fn restore_terminal() {
    pop_enhancement();
    let _ = disable_raw_mode();
    restore_original_termios();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = io::stdout().flush();
}

/// Restores the terminal when dropped (normal exit, signal or `?` error).
/// With `write` false (after SIGHUP or a hang-up) it only puts the terminal
/// settings back, which is a `tcsetattr` call and writes nothing.
struct RestoreGuard {
    write: bool,
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.write {
            restore_terminal();
        } else {
            let _ = disable_raw_mode();
            restore_original_termios();
        }
    }
}

/// Installs the panic hook; call it on the main thread. A panic there
/// restores the terminal and then prints as usual. Any other thread (a job
/// worker, the poller) must not touch the terminal, which the UI or an
/// attached child still uses: its message is only recorded for the status
/// line ([`jobs::record_panic`]) and nothing is printed.
fn install_panic_hook() {
    let main = thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if thread::current().id() == main {
            restore_terminal();
            previous(info);
        } else {
            jobs::record_panic(jobs::panic_message(info));
        }
    }));
}

/// Discards input events that arrived before the attach, so no buffered key
/// reaches the child (doc/0005 step 4): the queue of read events and
/// whatever input is waiting. Stops at the first turn that reads nothing,
/// and at a hang-up.
fn drain_events(pending: &mut VecDeque<Event>) {
    pending.clear();
    while let Ok(events) = next_events(Duration::ZERO) {
        if events.is_empty() {
            break;
        }
    }
}

/// The real [`Handover`] over the crossterm terminal.
struct TermHandover<'a> {
    terminal: &'a mut Term,
    termination: &'a Termination,
    /// Re-push keyboard enhancement on reclaim.
    enhanced: bool,
    /// The event loop's queue of read events, dropped on release.
    pending: &'a mut VecDeque<Event>,
}

impl Handover for TermHandover<'_> {
    fn release(&mut self) -> io::Result<()> {
        // Step 1: the event loop is paused simply because this thread is
        // here; no other thread reads stdin. Step 2: pop keyboard
        // enhancement (no mouse capture or bracketed paste is enabled).
        drain_events(self.pending);
        pop_enhancement();
        execute!(io::stdout(), Show, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        io::stdout().flush()?;
        drain_events(self.pending);
        Ok(())
    }

    fn child_exited(&mut self, failed: bool) {
        // The settings from before tower started, not crossterm's last
        // saved copy, so a child that died in raw mode leaves nothing
        // behind; crossterm's next enable_raw_mode then saves these.
        restore_original_termios();
        if failed && !terminal_gone(self.termination) {
            let mut out = io::stdout();
            let _ = out.write_all(MODE_RESETS.as_bytes());
            let _ = out.flush();
        }
    }

    fn announce(&mut self, line: &str) -> io::Result<()> {
        let mut out = io::stdout();
        writeln!(out, "{}", attach::printable(line))?;
        out.flush()
    }

    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()> {
        let mut out = io::stdout();
        writeln!(out, "{}", attach::printable(prompt))?;
        out.flush()?;
        enable_raw_mode()?;
        drain_events(self.pending);
        // Polled, not a blocking read, so a SIGTERM/SIGHUP ends the wait; a
        // hang-up is recorded as SIGHUP and ends it too.
        wait_key(self.termination, next_events)
            .map_err(|HungUp| io::Error::other("the terminal is gone"))
    }

    fn reclaim(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        push_enhancement(self.enhanced);
        self.terminal.clear()?;
        Ok(())
    }
}
