//! The terminal runtime: sets up and restores the terminal, runs the event
//! loop, feeds key presses and poll results to [`App`], and carries out the
//! [`Effect`]s it returns (doc/0005, doc/0014 milestone 1).
//!
//! The terminal is restored on normal exit, on error (a drop guard) and on
//! panic (a panic hook). Tower never writes BEL or OSC notifications
//! (doc/0007); it does not enable mouse capture, bracketed paste or keyboard
//! enhancement in milestone 1, so there is nothing of that kind to undo.

use std::io::{self, IsTerminal, Stdout, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::app::{App, Effect, Severity};
use crate::attach::{self, Handover, InheritSpawner};
use crate::cli::CliRunner;
use crate::config::Config;
use crate::merge::ProcEnviron;
use crate::paths::Dirs;
use crate::poller::Poller;
use crate::sidecar::Loaded;

/// How long the loop waits for input before checking for poll results and
/// redrawing (the status line shows the age of the last refresh).
const TICK: Duration = Duration::from_millis(200);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Runs the TUI until the user quits.
pub fn run(config: Config, dirs: Dirs, loaded: Loaded) -> io::Result<()> {
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

    install_panic_hook();
    let _restore = RestoreGuard;
    let mut terminal = enter()?;

    let poller = Poller::start(
        config.accounts.clone(),
        Arc::new(CliRunner::new()),
        config.poll_interval(),
    );
    event_loop(&mut terminal, &mut app, &poller, &state_file)
}

fn event_loop(
    terminal: &mut Term,
    app: &mut App,
    poller: &Poller,
    state_file: &Path,
) -> io::Result<()> {
    loop {
        terminal.draw(|frame| crate::ui::draw(frame, app, Instant::now()))?;
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
        while let Some(update) = poller.try_recv() {
            effects.extend(app.apply_poll(update));
        }
        if !run_effects(terminal, app, poller, state_file, effects)? {
            return Ok(());
        }
    }
}

/// Carries out effects in order. Returns `false` when the app should quit.
fn run_effects(
    terminal: &mut Term,
    app: &mut App,
    poller: &Poller,
    state_file: &Path,
    effects: Vec<Effect>,
) -> io::Result<bool> {
    let mut queue: std::collections::VecDeque<Effect> = effects.into();
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::Quit => return Ok(false),
            Effect::PollNow => poller.poll_now(),
            Effect::SaveSidecar => {
                if let Err(err) = app.sidecar.save(state_file) {
                    app.set_message(
                        format!("cannot save {}: {err}", state_file.display()),
                        Severity::Error,
                    );
                }
            }
            Effect::Attach(req) => {
                let mut handover = TermHandover { terminal };
                let outcome = attach::attach(&mut handover, &mut InheritSpawner, &req);
                queue.extend(app.attach_finished(outcome));
            }
        }
    }
    Ok(true)
}

/// Raw mode, alternate screen, hidden cursor.
fn enter() -> io::Result<Term> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    Ok(terminal)
}

/// Undoes [`enter`]; safe to call more than once and in any state.
pub fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = io::stdout().flush();
}

/// Restores the terminal when dropped (normal exit or `?` error).
struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore_terminal();
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
}

impl Handover for TermHandover<'_> {
    fn release(&mut self) -> io::Result<()> {
        // Step 1: the event loop is paused simply because this thread is
        // here; no other thread reads stdin. Step 2: no extra modes are
        // enabled in milestone 1.
        drain_events();
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
        loop {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                return Ok(());
            }
        }
    }

    fn reclaim(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        self.terminal.clear()?;
        Ok(())
    }
}
