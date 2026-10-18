//! Background polling of every account's listing (doc/0001).
//!
//! A coordinator thread ticks every `poll_interval` and starts one listing
//! call per account on its own short-lived worker thread, so a slow account
//! (for example one whose supervisor is starting) never delays the others and
//! the UI thread never blocks. Results go to the UI over a channel.
//!
//! An account's poll is skipped while its previous one is still running, so
//! calls never pile up. A forced poll ([`Poller::poll_now`], used on start, on
//! `r` and after returning from an attach) that finds a poll still running
//! marks it for one re-run as soon as it finishes, so the UI is guaranteed a
//! result that started after the request.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::CliRunner;
use crate::config::Account;
use crate::listing::Listing;

/// Where listings come from. [`CliRunner`] in tower; fakes in tests.
pub trait ListSource: Send + Sync {
    /// Lists one account's sessions; the error is a message for the UI.
    fn list(&self, account: &Account) -> Result<Listing, String>;
}

impl ListSource for CliRunner {
    fn list(&self, account: &Account) -> Result<Listing, String> {
        self.list_sessions(account).map_err(|e| e.to_string())
    }
}

/// One finished poll of one account.
#[derive(Debug, Clone)]
pub struct PollUpdate {
    /// Account name, as in the config.
    pub account: String,
    /// The parsed listing, or the error message.
    pub result: Result<Listing, String>,
}

enum Command {
    PollNow,
}

#[derive(Default)]
struct Slot {
    running: bool,
    rerun: bool,
}

/// Handle to the polling threads. Dropping it stops the coordinator; polls
/// already running finish on their own and their results are discarded.
pub struct Poller {
    commands: Sender<Command>,
    results: Receiver<PollUpdate>,
}

impl Poller {
    /// Starts polling `accounts` every `interval`, with an immediate first
    /// poll.
    pub fn start(
        accounts: Vec<Account>,
        source: Arc<dyn ListSource>,
        interval: Duration,
    ) -> Poller {
        let (commands, command_rx) = mpsc::channel();
        let (result_tx, results) = mpsc::channel();
        let slots: Vec<Arc<Mutex<Slot>>> = accounts
            .iter()
            .map(|_| Arc::new(Mutex::new(Slot::default())))
            .collect();
        thread::Builder::new()
            .name("tower-poller".into())
            .spawn(move || {
                let trigger = |forced: bool| {
                    for (account, slot) in accounts.iter().zip(&slots) {
                        start_poll(account, slot, &source, &result_tx, forced);
                    }
                };
                trigger(true);
                let mut next_tick = Instant::now() + interval;
                loop {
                    let wait = next_tick.saturating_duration_since(Instant::now());
                    match command_rx.recv_timeout(wait) {
                        Ok(Command::PollNow) => trigger(true),
                        Err(RecvTimeoutError::Timeout) => {
                            trigger(false);
                            next_tick = Instant::now() + interval;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn poller thread");
        Poller { commands, results }
    }

    /// Requests an immediate poll of every account.
    pub fn poll_now(&self) {
        let _ = self.commands.send(Command::PollNow);
    }

    /// Returns a finished poll, if any, without blocking.
    pub fn try_recv(&self) -> Option<PollUpdate> {
        self.results.try_recv().ok()
    }

    /// Waits up to `timeout` for a finished poll.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<PollUpdate> {
        self.results.recv_timeout(timeout).ok()
    }
}

fn lock(slot: &Mutex<Slot>) -> std::sync::MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a poll of one account unless one is running; a forced request
/// against a running poll is remembered as one re-run.
fn start_poll(
    account: &Account,
    slot: &Arc<Mutex<Slot>>,
    source: &Arc<dyn ListSource>,
    results: &Sender<PollUpdate>,
    forced: bool,
) {
    {
        let mut s = lock(slot);
        if s.running {
            s.rerun |= forced;
            return;
        }
        s.running = true;
        s.rerun = false;
    }
    let account = account.clone();
    let worker_slot = Arc::clone(slot);
    let source = Arc::clone(source);
    let results = results.clone();
    let spawned = thread::Builder::new()
        .name(format!("tower-poll-{}", account.name))
        .spawn(move || {
            loop {
                let result = source.list(&account);
                let sent = results.send(PollUpdate {
                    account: account.name.clone(),
                    result,
                });
                let mut s = lock(&worker_slot);
                if s.rerun && sent.is_ok() {
                    s.rerun = false;
                    continue;
                }
                s.running = false;
                break;
            }
        });
    if spawned.is_err() {
        lock(slot).running = false;
    }
}
