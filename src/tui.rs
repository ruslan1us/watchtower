//! The terminal runtime: sets up and restores the terminal, runs the event
//! loop, feeds key presses, poll results and finished background jobs to
//! [`App`], and carries out the [`Effect`]s it returns (doc/0005, doc/0008,
//! doc/0009, doc/0014 milestones 1 and 2). Slow CLI calls run on worker
//! threads ([`Jobs`]); attach, the task editor and the trust flow take over
//! the terminal through the same hand-over ([`attach::run_foreground`]).
//!
//! The terminal is restored on normal exit, on error (a drop guard), on panic
//! (a panic hook) and on SIGTERM or SIGHUP: their handlers only set a flag
//! ([`attach::Termination`]), and the event loop checks it every tick and
//! ends, so tower exits with 128 + signal. After SIGHUP the terminal may be
//! gone, so only raw mode is turned off and no escape sequences are written.
//! During an attach the flag waits until the child has exited (doc/0005).
//! Tower never writes BEL or OSC notifications (doc/0007). It enables no
//! mouse capture or bracketed paste; where the terminal supports the
//! keyboard-enhancement protocol it pushes `DISAMBIGUATE_ESCAPE_CODES`, so
//! `Ctrl+Enter` is reported distinctly in the new-session dialog (doc/0011),
//! and pops it again before every hand-over and on exit.

use std::io::{self, IsTerminal, Stdout, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use crate::app::{App, Effect, Severity};
use crate::attach::{self, Foreground, Handover, InheritSpawner, Termination};
use crate::cli::CliRunner;
use crate::config::Config;
use crate::jobs::Jobs;
use crate::merge::ProcEnviron;
use crate::paths::{self, Dirs};
use crate::poller::Poller;
use crate::sidecar::Loaded;
use crate::{dispatch, editor, projects};

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

/// Runs the TUI until the user quits or a SIGTERM/SIGHUP arrives.
pub fn run(config: Config, dirs: Dirs, loaded: Loaded) -> io::Result<Exit> {
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
    app.color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    if let Some(warning) = loaded.warning {
        app.add_warning(warning);
    }

    let termination = Termination::install().unwrap_or_else(|err| {
        app.add_warning(format!("cannot handle SIGTERM/SIGHUP: {err}"));
        Termination::default()
    });

    install_panic_hook();
    let mut restore = RestoreGuard { write: true };
    let (mut terminal, enhanced) = enter()?;
    app.keyboard_enhanced = enhanced;

    let poller = Poller::start(
        config.accounts.clone(),
        Arc::new(CliRunner::new()),
        config.poll_interval(),
    );
    let jobs = Jobs::new(CliRunner::new());
    let mut rt = Runtime {
        terminal: &mut terminal,
        poller: &poller,
        jobs: &jobs,
        state_file: &state_file,
        termination: &termination,
        enhanced,
    };
    let result = event_loop(&mut rt, &mut app);
    let exit = exit_for(result, termination.pending());
    if let Ok(exit) = exit {
        restore.write = exit.may_write_to_terminal();
    }
    exit
}

/// What the event loop works with.
struct Runtime<'a> {
    terminal: &'a mut Term,
    poller: &'a Poller,
    jobs: &'a Jobs,
    state_file: &'a Path,
    termination: &'a Termination,
    /// Whether the terminal supports keyboard enhancement (re-pushed after
    /// every hand-over).
    enhanced: bool,
}

impl Runtime<'_> {
    fn handover(&mut self) -> TermHandover<'_> {
        TermHandover {
            terminal: self.terminal,
            termination: self.termination,
            enhanced: self.enhanced,
        }
    }
}

/// Runs until the user quits or `termination` is pending; the flag is checked
/// before every draw, so tower stops within one [`TICK`].
fn event_loop(rt: &mut Runtime<'_>, app: &mut App) -> io::Result<()> {
    loop {
        if rt.termination.pending().is_some() {
            return Ok(());
        }
        rt.terminal
            .draw(|frame| crate::ui::draw(frame, app, Instant::now()))?;
        let mut effects = Vec::new();
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    effects.extend(app.handle_key(key));
                }
                // Resize: the next draw picks up the new size.
                _ => {}
            }
        }
        while let Some(update) = rt.poller.try_recv() {
            effects.extend(app.apply_poll(update));
        }
        while let Some(done) = rt.jobs.try_recv() {
            effects.extend(done.apply(app));
        }
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
            Effect::PollNow => rt.poller.poll_now(),
            Effect::SaveSidecar => {
                if let Err(err) = app.sidecar.save(rt.state_file) {
                    app.set_message(
                        format!("cannot save {}: {err}", rt.state_file.display()),
                        Severity::Error,
                    );
                }
            }
            Effect::Attach(req) => {
                let termination = rt.termination;
                let outcome =
                    attach::attach(&mut rt.handover(), &mut InheritSpawner, &req, termination);
                if termination.pending().is_some() {
                    return Ok(false);
                }
                queue.extend(app.attach_finished(outcome));
            }
            Effect::OpenDialog => {
                let config = app.config();
                let list = projects::discover(
                    &config.project_roots,
                    &config.extra_projects,
                    &app.sidecar.recent_projects,
                    app.home(),
                );
                app.open_dialog(list);
            }
            Effect::Dispatch(job) => rt.jobs.dispatch(job),
            Effect::Trust(job) => {
                let req = &job.request;
                let line = dispatch::trust_command(&req.account, &req.project);
                let dir = paths::shorten_home(&req.project.to_string_lossy(), app.home());
                let fg = Foreground {
                    line: &line,
                    announce: Some(format!(
                        "tower: running `{}` in {dir} for account {}. Accept the trust prompt, \
                         then leave Claude Code with /exit (or Ctrl+C twice); tower then \
                         retries the dispatch.",
                        req.account.command, req.account.name
                    )),
                    label: "claude",
                    pause_on_failure: false,
                };
                let termination = rt.termination;
                let result = attach::run_foreground(
                    &mut rt.handover(),
                    &mut InheritSpawner,
                    &fg,
                    termination,
                );
                if termination.pending().is_some() {
                    return Ok(false);
                }
                queue.extend(app.trust_finished(job, result));
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
            Effect::FetchLogs(target) => rt.jobs.logs(target),
        }
    }
    Ok(true)
}

/// Raw mode, alternate screen, hidden cursor, and keyboard enhancement where
/// supported. Returns the terminal and whether enhancement is supported.
fn enter() -> io::Result<(Term, bool)> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let supported = supports_keyboard_enhancement().unwrap_or(false);
    push_enhancement(supported);
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    Ok((terminal, supported))
}

/// Undoes [`enter`]; safe to call more than once and in any state.
pub fn restore_terminal() {
    pop_enhancement();
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = io::stdout().flush();
}

/// Restores the terminal when dropped (normal exit, signal or `?` error).
/// With `write` false (after SIGHUP) it only turns raw mode off, which is a
/// `tcsetattr` call and writes nothing.
struct RestoreGuard {
    write: bool,
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.write {
            restore_terminal();
        } else {
            let _ = disable_raw_mode();
        }
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Discards input events that arrived before the attach, so no buffered key
/// reaches the child (doc/0005 step 4).
fn drain_events() {
    while let Ok(true) = event::poll(Duration::ZERO) {
        if event::read().is_err() {
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
}

impl Handover for TermHandover<'_> {
    fn release(&mut self) -> io::Result<()> {
        // Step 1: the event loop is paused simply because this thread is
        // here; no other thread reads stdin. Step 2: pop keyboard
        // enhancement (no mouse capture or bracketed paste is enabled).
        drain_events();
        pop_enhancement();
        execute!(io::stdout(), Show, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        io::stdout().flush()?;
        drain_events();
        Ok(())
    }

    fn announce(&mut self, line: &str) -> io::Result<()> {
        let mut out = io::stdout();
        writeln!(out, "{line}")?;
        out.flush()
    }

    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()> {
        let mut out = io::stdout();
        writeln!(out, "{prompt}")?;
        out.flush()?;
        enable_raw_mode()?;
        drain_events();
        // Polled, not a blocking read, so a SIGTERM/SIGHUP ends the wait.
        while self.termination.pending().is_none() {
            if event::poll(TICK)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                return Ok(());
            }
        }
        Ok(())
    }

    fn reclaim(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        push_enhancement(self.enhanced);
        self.terminal.clear()?;
        Ok(())
    }
}
