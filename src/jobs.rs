//! Background jobs: the CLI calls behind the app's [`Effect`]s that can take
//! a while — dispatch (`claude --bg`, which may first start a supervisor),
//! `stop`, `rm`, `logs` and the hand-off sequence — each run on its own
//! short-lived worker thread so the UI never blocks (doc/0001, doc/0004,
//! doc/0008, doc/0009). Results come back over a channel as [`JobResult`]s;
//! [`JobResult::apply`] hands them to the [`App`]. A hand-off also reports
//! each step as it starts ([`JobResult::HandoffStep`]).
//!
//! Like every background call, these run with stdin null and captured
//! output ([`CliRunner`]), so nothing can touch the terminal (doc/0005).
//!
//! [`Effect`]: crate::app::Effect

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::actions::{self, RemoveError, StopError};
use crate::app::{App, DispatchJob, Effect, HandoffJob, LogsTarget, SessionAction};
use crate::cli::CliRunner;
use crate::dispatch::{self, DispatchError, Dispatched};
use crate::handoff::{self, HandoffError, HandoffTiming, Resumed, Step};
use crate::logview::{self, LogScreen, LogsError};

/// A finished background job.
#[derive(Debug)]
pub enum JobResult {
    /// `claude --bg` from the dialog.
    Dispatch(DispatchJob, Result<Dispatched, DispatchError>),
    /// `stop <id>`.
    Stop(SessionAction, Result<(), StopError>),
    /// `rm <id>`.
    Remove(SessionAction, Result<(), RemoveError>),
    /// `logs <id>`, already replayed into a screen.
    Logs(LogsTarget, Result<LogScreen, LogsError>),
    /// A step of a running hand-off has started.
    HandoffStep(Step),
    /// A finished hand-off (doc/0004).
    Handoff(HandoffJob, Result<Resumed, HandoffError>),
}

impl JobResult {
    /// Feeds the result to the app and returns its follow-up effects.
    pub fn apply(self, app: &mut App) -> Vec<Effect> {
        match self {
            JobResult::Dispatch(job, result) => app.dispatch_finished(job, result),
            JobResult::Stop(action, result) => app.stop_finished(action, result),
            JobResult::Remove(action, result) => app.remove_finished(action, result),
            JobResult::Logs(target, result) => app.logs_fetched(target, result),
            JobResult::HandoffStep(step) => {
                app.handoff_step(step);
                Vec::new()
            }
            JobResult::Handoff(job, result) => app.handoff_finished(job, result),
        }
    }
}

/// Starts jobs and collects their results.
pub struct Jobs {
    runner: Arc<CliRunner>,
    handoff_timing: HandoffTiming,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
}

impl Jobs {
    /// Jobs that run their CLI calls with `runner`.
    pub fn new(runner: CliRunner) -> Jobs {
        let (tx, rx) = mpsc::channel();
        Jobs {
            runner: Arc::new(runner),
            handoff_timing: HandoffTiming::default(),
            tx,
            rx,
        }
    }

    /// Uses `timing` for the stop wait of hand-offs (tests).
    pub fn with_handoff_timing(mut self, timing: HandoffTiming) -> Jobs {
        self.handoff_timing = timing;
        self
    }

    fn spawn(&self, name: &str, work: impl FnOnce(&CliRunner) -> JobResult + Send + 'static) {
        self.spawn_with(name, move |runner, _| work(runner));
    }

    /// Like `spawn`, with a sender for intermediate results.
    fn spawn_with(
        &self,
        name: &str,
        work: impl FnOnce(&CliRunner, &Sender<JobResult>) -> JobResult + Send + 'static,
    ) {
        let runner = Arc::clone(&self.runner);
        let tx = self.tx.clone();
        // A thread that cannot be created means the job never runs and
        // nothing is reported back; nothing may be printed here, the
        // terminal belongs to the UI.
        let _ = thread::Builder::new()
            .name(format!("tower-{name}"))
            .spawn(move || {
                let result = work(&runner, &tx);
                let _ = tx.send(result);
            });
    }

    /// Runs the dispatch of `job` (doc/0008).
    pub fn dispatch(&self, job: DispatchJob) {
        self.spawn("dispatch", move |runner| {
            let result = dispatch::run(runner, &job.request);
            JobResult::Dispatch(job, result)
        });
    }

    /// Runs `<account command> stop <id>` (doc/0009).
    pub fn stop(&self, action: SessionAction) {
        self.spawn("stop", move |runner| {
            let result = actions::stop(runner, &action.account, &action.id);
            JobResult::Stop(action, result)
        });
    }

    /// Runs `<account command> rm <id>` (doc/0009).
    pub fn remove(&self, action: SessionAction) {
        self.spawn("rm", move |runner| {
            let result = actions::remove(runner, &action.account, &action.id);
            JobResult::Remove(action, result)
        });
    }

    /// Runs the hand-off sequence of `job` (doc/0004): stop, wait, resume
    /// under the target. Each step is reported as it starts.
    pub fn handoff(&self, job: HandoffJob) {
        let timing = self.handoff_timing;
        self.spawn_with("handoff", move |runner, tx| {
            let mut progress = |step: Step| {
                let _ = tx.send(JobResult::HandoffStep(step));
            };
            let result = handoff::run(runner, &job.request, &timing, &mut progress);
            JobResult::Handoff(job, result)
        });
    }

    /// Runs `<account command> logs <id>` and replays the stream (doc/0009).
    pub fn logs(&self, target: LogsTarget) {
        self.spawn("logs", move |runner| {
            let result =
                logview::fetch(runner, &target.account, &target.id).map(|b| logview::render(&b));
            JobResult::Logs(target, result)
        });
    }

    /// A finished job, if any, without blocking.
    pub fn try_recv(&self) -> Option<JobResult> {
        self.rx.try_recv().ok()
    }

    /// Waits up to `timeout` for a finished job.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<JobResult> {
        self.rx.recv_timeout(timeout).ok()
    }
}
