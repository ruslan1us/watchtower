//! Following sessions' `claude logs` output in the background (doc/0030).
//!
//! A capture of `claude logs` holds only the last ~40 seconds of a working
//! session, and Claude Code repaints the whole screen only at the session's
//! start and on attach, so the screen is rebuilt from a [`LogStream`] per
//! session that every fetch feeds ([`crate::logstream`]). The [`Follower`]
//! owns those streams and decides when to fetch:
//!
//! - **Plan.** After every poll the app hands over the live background rows
//!   whose owning account is known ([`FollowSpec`], at most
//!   [`MAX_STREAMS`], the logs panel's row first). Rows that are working are
//!   fetched every [`CADENCE`], whatever the poll interval; the others keep
//!   the stream they have but are not fetched in the background. Streams of
//!   rows that left the plan (gone, stopped, not listed) are dropped after
//!   [`GRACE`], so a session dispatched a moment ago keeps its stream until
//!   a listing shows it.
//! - **Requests.** The logs panel's refreshes, and the fetch right after a
//!   dispatch or an attach, are [`Follower::fetch`]es: they start at once,
//!   or right after the fetch of the same session that is running, and the
//!   panel's come back as a [`LogScreen`] ([`Follower::try_recv`]).
//! - **Attached.** While tower has handed the terminal to `claude attach`,
//!   that session is fetched at the cadence too ([`Follower::attached`]),
//!   so its full repaint on attach is caught however long the attach lasts.
//!   The follower runs on its own thread, so background follows go on
//!   during any hand-over of the terminal.
//! - **Cadence.** `logs_follow_secs` sets it (doc/0031), and
//!   [`Follower::set_cadence`] changes it while the follower runs: the
//!   next fetch of each session is due one new cadence after its last one.
//!   `None` turns the periodic fetches off, for the plan's working rows and
//!   the attached session alike; requests still run, and streams are kept
//!   and fed by them as before.
//!
//! Bounds: at most [`MAX_PARALLEL`] background fetches run at a time, the
//! longest due first so none starves when fetches are slow (requests are
//! not counted, they are few); at most one fetch per session; and at most
//! [`MAX_STREAMS`] streams are kept. Beyond that the plan's rows that are
//! not working go first (from the plan's end), then streams outside the
//! plan (oldest request first), then working rows; the attached session's
//! stream goes last. A session's stream
//! is dropped when `logs` says the session has no live process or is not
//! the account's. Every fetch runs with the owning account's command
//! (doc/0003) on a short-lived worker thread; a worker that panics loses
//! its stream (the next fetch starts a new one) and, for a request,
//! reports the panic as its error.
//!
//! Ordering: a worker hands its stream and its request's result back to the
//! follower thread together, and that thread first puts the stream back in
//! its slot (no longer busy) and only then publishes the result. So once
//! [`Follower::try_recv`] or [`Follower::recv_timeout`] returns a result,
//! everything sent to the follower afterwards ([`Follower::streams`],
//! another [`Follower::fetch`]) sees the stream that fetch left behind.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::LogsTarget;
use crate::cli::{CliError, CliRunner};
use crate::config::Account;
use crate::jobs;
use crate::logstream::LogStream;
use crate::logview::{self, LogScreen, LogsError};
use crate::ptysize::{self, TerminalReader, TtyProbe};

/// How often a working session is fetched in the background by default
/// (`logs_follow_secs`, doc/0031). A capture covers about 40 seconds of a
/// working session's output, so a fetch every 10 seconds always overlaps
/// the previous one, even when frames come several times faster than the
/// spinner's.
pub const CADENCE: Duration = Duration::from_secs(crate::config::DEFAULT_LOGS_FOLLOW_SECS);

/// Most background fetches running at the same time.
pub const MAX_PARALLEL: usize = 3;

/// Most streams kept.
pub const MAX_STREAMS: usize = 16;

/// How long a stream outside the plan is kept after it was last requested.
pub const GRACE: Duration = Duration::from_secs(30);

/// Where `logs` output comes from: the owning account's command in tower,
/// fakes in tests.
pub trait LogSource: Send + Sync {
    /// The stdout of `<account command> logs <id>` ([`logview::fetch`]).
    fn logs(&self, account: &Account, id: &str) -> Result<Vec<u8>, LogsError>;
}

impl LogSource for CliRunner {
    fn logs(&self, account: &Account, id: &str) -> Result<Vec<u8>, LogsError> {
        logview::fetch(self, account, id)
    }
}

/// One session to follow, as the app knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowSpec {
    /// The owning account and the short id (doc/0003).
    pub target: LogsTarget,
    /// The listed `pid`; another pid starts the stream over.
    pub pid: Option<u32>,
    /// What the fetch needs to read the session terminal's real size
    /// (doc/0025), when the pid-reuse guard accepted the process.
    pub probe: Option<TtyProbe>,
    /// Whether the session is working (`state: working` or `status: busy`):
    /// only those are fetched at the cadence.
    pub working: bool,
}

/// The follower's limits; [`FollowSettings::default`] in tower, shorter
/// ones in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowSettings {
    /// How often working and attached sessions are fetched ([`CADENCE`]
    /// by default); `None`: never in the background (doc/0031).
    pub cadence: Option<Duration>,
    /// See [`MAX_PARALLEL`].
    pub max_parallel: usize,
    /// See [`MAX_STREAMS`].
    pub max_streams: usize,
    /// See [`GRACE`].
    pub grace: Duration,
}

impl Default for FollowSettings {
    fn default() -> Self {
        FollowSettings {
            cadence: Some(CADENCE),
            max_parallel: MAX_PARALLEL,
            max_streams: MAX_STREAMS,
            grace: GRACE,
        }
    }
}

/// What [`Follower::streams`] reports about one kept stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// The session.
    pub target: LogsTarget,
    /// Whether its stream has a base ([`LogStream::has_base`]); `None`
    /// while it has no stream yet or a fetch holds it.
    pub has_base: Option<bool>,
    /// Whether a fetch for it runs.
    pub busy: bool,
}

/// A request's result: the session and its screen, or why there is none.
pub type Fetched = (LogsTarget, Result<LogScreen, LogsError>);

type Key = (String, String);

fn key(target: &LogsTarget) -> Key {
    (target.account.name.clone(), target.id.clone())
}

enum Msg {
    Plan(Vec<FollowSpec>),
    Fetch(FollowSpec, bool),
    Attached(Option<FollowSpec>),
    Cadence(Option<Duration>),
    /// A fetch finished: the stream to keep and, for a request with a
    /// snapshot, its result, published once the stream is back
    /// (see the module docs).
    Done {
        key: Key,
        stream: Option<Box<LogStream>>,
        result: Option<Fetched>,
    },
    Streams(Sender<Vec<StreamInfo>>),
    Stop,
}

/// Handle to the follower thread. Dropping it stops the thread; fetches
/// already running finish on their own and their results are dropped.
pub struct Follower {
    tx: Sender<Msg>,
    results_tx: Sender<Fetched>,
    results: Receiver<Fetched>,
}

impl Follower {
    /// Starts following with `source` for `logs` and `terminal` for the
    /// session terminal's size (doc/0025).
    pub fn start(
        source: Arc<dyn LogSource>,
        terminal: Arc<dyn TerminalReader>,
        settings: FollowSettings,
    ) -> Follower {
        let (tx, rx) = mpsc::channel();
        let (results_tx, results) = mpsc::channel();
        let mut state = State {
            settings,
            source,
            terminal,
            tx: tx.clone(),
            results: results_tx.clone(),
            plan: Vec::new(),
            attached: None,
            slots: HashMap::new(),
        };
        let started = thread::Builder::new()
            .name("tower-follow".to_owned())
            .spawn(move || state.run(&rx));
        if let Err(err) = started {
            jobs::record_panic(format!("cannot start the log follower: {err}"));
        }
        Follower {
            tx,
            results_tx,
            results,
        }
    }

    /// The sessions to follow, most important first (see the module docs).
    pub fn set_plan(&self, plan: Vec<FollowSpec>) {
        let _ = self.tx.send(Msg::Plan(plan));
    }

    /// Fetches `spec`'s session now, or right after its running fetch. With
    /// `snapshot` the screen comes back through [`Follower::try_recv`].
    pub fn fetch(&self, spec: FollowSpec, snapshot: bool) {
        let target = spec.target.clone();
        if self.tx.send(Msg::Fetch(spec, snapshot)).is_err() && snapshot {
            let error = CliError::Worker {
                job: "logs".to_owned(),
                message: "the log follower has stopped".to_owned(),
            };
            let _ = self.results_tx.send((target, Err(error.into())));
        }
    }

    /// The session tower is attached to, followed at the cadence whatever
    /// its state; `None` once the attach returned.
    pub fn attached(&self, spec: Option<FollowSpec>) {
        let _ = self.tx.send(Msg::Attached(spec));
    }

    /// Changes the cadence while the follower runs (doc/0031): each
    /// session's next background fetch is due one new cadence after its
    /// last fetch; `None` stops the background fetches until a cadence is
    /// set again. Requests ([`Follower::fetch`]) are not affected.
    pub fn set_cadence(&self, cadence: Option<Duration>) {
        let _ = self.tx.send(Msg::Cadence(cadence));
    }

    /// A finished request with a snapshot, if any, without blocking.
    pub fn try_recv(&self) -> Option<Fetched> {
        self.results.try_recv().ok()
    }

    /// Waits up to `timeout` for a finished request with a snapshot.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Fetched> {
        self.results.recv_timeout(timeout).ok()
    }

    /// The streams kept now (tests, diagnostics), as of every message sent
    /// before and every result received before (see the module docs);
    /// empty when the thread does not answer within five seconds.
    pub fn streams(&self) -> Vec<StreamInfo> {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Msg::Streams(tx)).is_err() {
            return Vec::new();
        }
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

struct Slot {
    target: LogsTarget,
    /// `None` before the first fetch, while a fetch holds it, or after the
    /// session turned out gone.
    stream: Option<Box<LogStream>>,
    busy: bool,
    /// Whether the running fetch counts against `max_parallel`.
    background: bool,
    /// When the last fetch finished.
    last_done: Option<Instant>,
    /// When it was created or last requested.
    touched: Instant,
    /// A request that came while a fetch ran: the spec and whether it
    /// wants a snapshot.
    rerun: Option<(FollowSpec, bool)>,
}

struct State {
    settings: FollowSettings,
    source: Arc<dyn LogSource>,
    terminal: Arc<dyn TerminalReader>,
    tx: Sender<Msg>,
    results: Sender<Fetched>,
    plan: Vec<FollowSpec>,
    attached: Option<FollowSpec>,
    slots: HashMap<Key, Slot>,
}

impl State {
    fn run(&mut self, rx: &Receiver<Msg>) {
        let mut wait = Duration::ZERO;
        loop {
            let msg = match rx.recv_timeout(wait) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            if matches!(msg, Some(Msg::Stop)) {
                return;
            }
            let step = jobs::catch_panic(|| {
                if let Some(msg) = msg {
                    self.handle(msg);
                }
                while let Ok(msg) = rx.try_recv() {
                    if matches!(msg, Msg::Stop) {
                        return None;
                    }
                    self.handle(msg);
                }
                let now = Instant::now();
                self.prune(now);
                Some(self.schedule(now))
            });
            match step {
                Ok(Some(next)) => wait = next,
                Ok(None) => return,
                Err(message) => {
                    jobs::record_panic(format!("the log follower failed: {message}"));
                    wait = Duration::from_secs(1);
                }
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        let now = Instant::now();
        match msg {
            Msg::Plan(plan) => self.plan = plan,
            Msg::Attached(spec) => self.attached = spec,
            Msg::Cadence(cadence) => self.settings.cadence = cadence,
            Msg::Fetch(spec, snapshot) => {
                let slot = self.slot(&spec.target, now);
                slot.touched = now;
                if slot.busy {
                    let wants = snapshot || slot.rerun.as_ref().is_some_and(|r| r.1);
                    slot.rerun = Some((spec, wants));
                } else {
                    self.launch(spec, snapshot, false);
                }
            }
            Msg::Done {
                key,
                stream,
                result,
            } => {
                // The stream goes back before the result is published, so
                // whoever receives the result sees it (see the module docs).
                let rerun = self.slots.get_mut(&key).and_then(|slot| {
                    slot.busy = false;
                    slot.stream = stream;
                    slot.last_done = Some(now);
                    slot.rerun.take()
                });
                if let Some(result) = result {
                    let _ = self.results.send(result);
                }
                if let Some((spec, snapshot)) = rerun {
                    self.launch(spec, snapshot, false);
                }
            }
            Msg::Streams(reply) => {
                let mut out: Vec<StreamInfo> = self
                    .slots
                    .values()
                    .map(|s| StreamInfo {
                        target: s.target.clone(),
                        has_base: s.stream.as_ref().map(|st| st.has_base()),
                        busy: s.busy,
                    })
                    .collect();
                out.sort_by_key(|a| key(&a.target));
                let _ = reply.send(out);
            }
            Msg::Stop => {}
        }
    }

    fn slot(&mut self, target: &LogsTarget, now: Instant) -> &mut Slot {
        self.slots.entry(key(target)).or_insert_with(|| Slot {
            target: target.clone(),
            stream: None,
            busy: false,
            background: false,
            last_done: None,
            touched: now,
            rerun: None,
        })
    }

    /// The sessions fetched at the cadence, in order: the attached one,
    /// then the plan's working rows.
    fn cadence_specs(&self) -> Vec<FollowSpec> {
        let mut out: Vec<FollowSpec> = self.attached.iter().cloned().collect();
        for spec in self.plan.iter().filter(|s| s.working) {
            if !out.iter().any(|o| key(&o.target) == key(&spec.target)) {
                out.push(spec.clone());
            }
        }
        out
    }

    /// Starts the due background fetches, the longest due first, within
    /// `max_parallel`; returns how long to wait for the next one. Nothing
    /// is due while the cadence is off.
    fn schedule(&mut self, now: Instant) -> Duration {
        let mut next = Duration::from_secs(1);
        let Some(cadence) = self.settings.cadence else {
            return next;
        };
        let running = self
            .slots
            .values()
            .filter(|s| s.busy && s.background)
            .count();
        let mut due: Vec<(Option<Instant>, FollowSpec)> = Vec::new();
        for spec in self.cadence_specs() {
            let slot = self.slot(&spec.target, now);
            if slot.busy {
                continue;
            }
            let last = slot.last_done;
            let due_in = last.map_or(Duration::ZERO, |t| cadence.saturating_sub(now - t));
            if due_in.is_zero() {
                due.push((last, spec));
            } else {
                next = next.min(due_in);
            }
        }
        // Never fetched first, then the oldest fetch: no session starves
        // when fetches are slow. A finished fetch wakes the loop for the
        // ones that have to wait.
        due.sort_by_key(|(last, _)| *last);
        let free = self.settings.max_parallel.saturating_sub(running);
        for (_, spec) in due.into_iter().take(free) {
            self.launch(spec, false, true);
        }
        next
    }

    /// How much a stream is wanted; lower is kept longer: the attached
    /// session, then the plan's working rows, then streams outside the plan
    /// still in their grace time (most recently requested first), then the
    /// plan's other rows; within a class, in plan order.
    fn rank(&self, k: &Key, slot: &Slot) -> (u8, usize, Option<std::cmp::Reverse<Instant>>) {
        if self.attached.as_ref().is_some_and(|a| &key(&a.target) == k) {
            return (0, 0, None);
        }
        match self.plan.iter().position(|s| &key(&s.target) == k) {
            Some(p) if self.plan[p].working => (1, p, None),
            Some(p) => (3, p, None),
            None => (2, 0, Some(std::cmp::Reverse(slot.touched))),
        }
    }

    /// Drops the streams outside the plan not requested within the grace
    /// time, then the least wanted ones ([`State::rank`]) beyond
    /// `max_streams`; one whose fetch runs goes once it is back.
    fn prune(&mut self, now: Instant) {
        let grace = self.settings.grace;
        let wanted: Vec<Key> = self
            .attached
            .iter()
            .chain(self.plan.iter())
            .map(|s| key(&s.target))
            .collect();
        self.slots
            .retain(|k, s| s.busy || wanted.contains(k) || now - s.touched < grace);
        let excess = self.slots.len().saturating_sub(self.settings.max_streams);
        if excess == 0 {
            return;
        }
        let mut order: Vec<_> = self
            .slots
            .iter()
            .map(|(k, s)| (self.rank(k, s), s.busy, k.clone()))
            .collect();
        order.sort_by_key(|a| std::cmp::Reverse(a.0));
        for (_, busy, k) in order.into_iter().take(excess) {
            if !busy {
                self.slots.remove(&k);
            }
        }
    }

    /// Runs one fetch of `spec`'s session on a worker thread.
    fn launch(&mut self, spec: FollowSpec, snapshot: bool, background: bool) {
        let k = key(&spec.target);
        let now = Instant::now();
        let slot = self.slot(&spec.target, now);
        slot.target = spec.target.clone();
        slot.busy = true;
        slot.background = background;
        let stream = slot.stream.take();
        let source = Arc::clone(&self.source);
        let terminal = Arc::clone(&self.terminal);
        let tx = self.tx.clone();
        let target = spec.target.clone();
        let body = {
            let k = k.clone();
            let tx = tx.clone();
            let target = target.clone();
            move || {
                let out = jobs::catch_panic(|| {
                    fetch_and_feed(source.as_ref(), terminal.as_ref(), &spec, stream, snapshot)
                });
                let (stream, result) = match out {
                    Ok((stream, screen)) => (stream, screen.map(|s| (target, s))),
                    Err(message) => (None, snapshot.then(|| (target, Err(worker_error(message))))),
                };
                // The follower thread publishes the result once the stream
                // is back (see the module docs).
                let _ = tx.send(Msg::Done {
                    key: k,
                    stream,
                    result,
                });
            }
        };
        if let Err(err) = thread::Builder::new()
            .name("tower-logs".to_owned())
            .spawn(body)
        {
            let result = snapshot.then(|| {
                let message = format!("cannot start its thread: {err}");
                (target, Err(worker_error(message)))
            });
            let _ = tx.send(Msg::Done {
                key: k,
                stream: None,
                result,
            });
        }
    }
}

fn worker_error(message: String) -> LogsError {
    CliError::Worker {
        job: "logs".to_owned(),
        message,
    }
    .into()
}

/// One fetch: runs `logs`, reads the terminal size right after it returns
/// (doc/0025), feeds the stream (a new one when there is none) and, with
/// `snapshot`, converts its screen. Returns the stream to keep and the
/// request's result. A session without a live process, or unknown to the
/// account, loses its stream.
fn fetch_and_feed(
    source: &dyn LogSource,
    terminal: &dyn TerminalReader,
    spec: &FollowSpec,
    stream: Option<Box<LogStream>>,
    snapshot: bool,
) -> (Option<Box<LogStream>>, Option<Result<LogScreen, LogsError>>) {
    match source.logs(&spec.target.account, &spec.target.id) {
        Ok(bytes) => {
            let size = spec.probe.and_then(|p| ptysize::session_size(terminal, p));
            let mut stream = match stream {
                Some(mut s) => {
                    s.feed(bytes, size, spec.pid);
                    s
                }
                None => Box::new(LogStream::new(bytes, size, spec.pid)),
            };
            let screen = snapshot.then(|| Ok(stream.screen()));
            (Some(stream), screen)
        }
        Err(err) => {
            let gone = matches!(err, LogsError::NotRunning | LogsError::UnknownSession);
            let stream = if gone { None } else { stream };
            (stream, snapshot.then_some(Err(err)))
        }
    }
}
