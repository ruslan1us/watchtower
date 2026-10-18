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
//!
//! Every trigger (tick or forced) is one poll cycle with an increasing
//! number, carried by each [`PollUpdate`]; [`Poller::poll_now`] returns the
//! number of the cycle it asked for, so a caller can tell a listing that
//! started after its request (an attach's refresh, doc/0005). Ownership rule 2 compares the
//! accounts' listings only when all of them come from the same cycle
//! (doc/0017): listings taken at different times can disagree just because
//! a session started, stopped or moved in between.
//!
//! A panic never ends polling: the coordinator is restarted with a short
//! backoff and the panic reported once ([`Poller::try_restart`]), and a
//! panic inside a listing call is that poll's error (doc/0001).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::CliRunner;
use crate::config::Account;
use crate::jobs;
use crate::listing::Listing;

/// The wait before the first restart of a coordinator that panicked; it
/// doubles with every further restart, up to [`MAX_RESTART_DELAY`].
pub const RESTART_DELAY: Duration = Duration::from_millis(500);

/// The longest wait between restarts.
pub const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);

/// A coordinator that ran this long before it panicked counts as healthy:
/// its restart waits [`RESTART_DELAY`] again.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// Where listings come from. [`CliRunner`] in tower; fakes in tests.
pub trait ListSource: Send + Sync {
    /// Lists one account's sessions; the error is a message for the UI.
    fn list(&self, account: &Account) -> Result<Listing, String>;

    /// Called on the coordinator thread when poll cycle `cycle` starts,
    /// before any of its polls. Does nothing by default; tests use it.
    fn cycle_started(&self, _cycle: u64) {}
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
    /// The poll cycle this listing belongs to (see the module docs). A poll
    /// skipped because the previous one was still running leaves the
    /// account a cycle behind; a re-run carries the cycle that asked for it.
    pub cycle: u64,
    /// The parsed listing, or the error message.
    pub result: Result<Listing, String>,
}

enum Command {
    /// A forced poll with the cycle number [`Poller::poll_now`] gave it.
    PollNow(u64),
}

#[derive(Default)]
struct Slot {
    running: bool,
    rerun: bool,
    /// The cycle of the requested re-run, when `rerun` is set.
    rerun_cycle: u64,
}

/// Handle to the polling threads. Dropping it stops the coordinator; polls
/// already running finish on their own and their results are discarded.
pub struct Poller {
    commands: Sender<Command>,
    results: Receiver<PollUpdate>,
    /// The last cycle number handed out (ticks and forced polls).
    cycle: Arc<AtomicU64>,
    /// One message per restart of the coordinator after a panic.
    restarts: Receiver<String>,
}

impl Poller {
    /// Starts polling `accounts` every `interval`, with an immediate first
    /// poll.
    ///
    /// The coordinator runs under `catch_unwind`: if it panics it is
    /// restarted after a short, growing wait ([`RESTART_DELAY`] up to
    /// [`MAX_RESTART_DELAY`]) with an immediate poll, and the panic is
    /// reported once through [`Poller::try_restart`]. A panic inside one
    /// listing call becomes that poll's error.
    pub fn start(
        accounts: Vec<Account>,
        source: Arc<dyn ListSource>,
        interval: Duration,
    ) -> Poller {
        let (commands, command_rx) = mpsc::channel();
        let (result_tx, results) = mpsc::channel();
        let (restart_tx, restarts) = mpsc::channel();
        let cycle = Arc::new(AtomicU64::new(0));
        let slots: Vec<Arc<Mutex<Slot>>> = accounts
            .iter()
            .map(|_| Arc::new(Mutex::new(Slot::default())))
            .collect();
        let counter = Arc::clone(&cycle);
        thread::Builder::new()
            .name("tower-poller".into())
            .spawn(move || {
                let mut delay = RESTART_DELAY;
                loop {
                    let started = Instant::now();
                    let ran = jobs::catch_panic(|| {
                        coordinate(
                            &accounts,
                            &slots,
                            &source,
                            &result_tx,
                            &command_rx,
                            &counter,
                            interval,
                        )
                    });
                    let Err(message) = ran else {
                        // The handle was dropped.
                        break;
                    };
                    if started.elapsed() >= HEALTHY_RUN {
                        delay = RESTART_DELAY;
                    }
                    if restart_tx.send(message).is_err() {
                        break;
                    }
                    thread::sleep(delay);
                    delay = (delay * 2).min(MAX_RESTART_DELAY);
                }
            })
            .expect("spawn poller thread");
        Poller {
            commands,
            results,
            cycle,
            restarts,
        }
    }

    /// Requests an immediate poll of every account and returns its cycle
    /// number: every [`PollUpdate`] of that account with this cycle or a
    /// later one comes from a listing that started after the request.
    pub fn poll_now(&self) -> u64 {
        let cycle = self.cycle.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.commands.send(Command::PollNow(cycle));
        cycle
    }

    /// The panic message of a coordinator restart, if one happened since
    /// the last call; one message per restart.
    pub fn try_restart(&self) -> Option<String> {
        self.restarts.try_recv().ok()
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

/// The coordinator loop: an immediate forced cycle, then one cycle per
/// tick and one per forced request. Returns when the [`Poller`] is dropped.
fn coordinate(
    accounts: &[Account],
    slots: &[Arc<Mutex<Slot>>],
    source: &Arc<dyn ListSource>,
    result_tx: &Sender<PollUpdate>,
    command_rx: &Receiver<Command>,
    counter: &AtomicU64,
    interval: Duration,
) {
    let trigger = |forced: bool, cycle: u64| {
        source.cycle_started(cycle);
        for (account, slot) in accounts.iter().zip(slots) {
            start_poll(account, slot, source, result_tx, forced, cycle);
        }
    };
    let next = || counter.fetch_add(1, Ordering::SeqCst) + 1;
    trigger(true, next());
    let mut next_tick = Instant::now() + interval;
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match command_rx.recv_timeout(wait) {
            Ok(Command::PollNow(cycle)) => trigger(true, cycle),
            Err(RecvTimeoutError::Timeout) => {
                trigger(false, next());
                next_tick = Instant::now() + interval;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn lock(slot: &Mutex<Slot>) -> std::sync::MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a poll of one account for `cycle` unless one is running; a
/// forced request against a running poll is remembered as one re-run, which
/// then reports the latest forced cycle.
fn start_poll(
    account: &Account,
    slot: &Arc<Mutex<Slot>>,
    source: &Arc<dyn ListSource>,
    results: &Sender<PollUpdate>,
    forced: bool,
    cycle: u64,
) {
    {
        let mut s = lock(slot);
        if s.running {
            if forced {
                // The latest request's cycle; a tick numbered after it may
                // have asked first.
                s.rerun_cycle = if s.rerun {
                    s.rerun_cycle.max(cycle)
                } else {
                    cycle
                };
                s.rerun = true;
            }
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
            let mut cycle = cycle;
            loop {
                // A panic in the listing call is this poll's error, so the
                // slot is freed and the account keeps being polled.
                let result = jobs::catch_panic(|| source.list(&account))
                    .unwrap_or_else(|panic| Err(format!("listing failed: {panic}")));
                let sent = results.send(PollUpdate {
                    account: account.name.clone(),
                    cycle,
                    result,
                });
                let mut s = lock(&worker_slot);
                if s.rerun && sent.is_ok() {
                    s.rerun = false;
                    cycle = s.rerun_cycle;
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
