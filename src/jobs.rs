//! Background jobs: the CLI calls behind the app's [`Effect`]s that can take
//! a while — dispatch (`claude --bg`, which may first start a supervisor),
//! the orchestrator's start, `stop`, `rm`, `logs` and the hand-off sequence
//! — and the file work that
//! could block on a slow disk or a hung network mount — sidecar saves and
//! reloads, the project scan — each run on its own short-lived worker
//! thread so the UI never blocks (doc/0001, doc/0002, doc/0004, doc/0008,
//! doc/0009). Results come back over a channel as [`JobResult`]s;
//! [`JobResult::apply`] hands them to the [`App`]. A hand-off also reports
//! each step as it starts ([`JobResult::HandoffStep`]).
//!
//! Like every background call, these run with stdin null and captured
//! output ([`CliRunner`]), so nothing can touch the terminal (doc/0005).
//!
//! Every job reports back exactly once (doc/0001): a job that panics, or
//! whose thread cannot be started, yields an error result of its own kind
//! ([`CliError::Worker`]), so no dialog, hand-off or logs fetch waits
//! forever. The panic hook (`tui`) restores the terminal only on the main
//! thread; on any other thread it only records the message here
//! ([`record_panic`]): a job worker's goes into its error result, any other
//! thread's waits for the status line ([`take_stray_panic`]).
//!
//! [`Effect`]: crate::app::Effect

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::actions::{self, RemoveError, StopError};
use crate::app::{
    App, DispatchJob, Effect, HandoffJob, LogsTarget, OrchestratorJob, ProjectScan, SessionAction,
    SidecarJob,
};
use crate::cli::{CliError, CliRunner};
use crate::dispatch::{self, DispatchError, Dispatched};
use crate::handoff::{self, HandoffError, HandoffTiming, Resumed, Step};
use crate::logview::{self, LogScreen, LogsError};
use crate::orchestrator::{self, StartError};
use crate::projects::{self, ProjectList};
use crate::sidecar::{self, LOCK_WAIT, Reloaded, SaveJob, Saved};

/// A finished background job.
#[derive(Debug)]
pub enum JobResult {
    /// `claude --bg` from the dialog.
    Dispatch(DispatchJob, Result<Dispatched, DispatchError>),
    /// The orchestrator's folder prepared and its `claude --bg` run
    /// (doc/0010).
    Orchestrator(OrchestratorJob, Result<Dispatched, StartError>),
    /// `stop <id>`.
    Stop(SessionAction, Result<(), StopError>),
    /// `rm <id>`.
    Remove(SessionAction, Result<(), RemoveError>),
    /// `logs <id>`, already replayed into a screen.
    Logs(LogsTarget, Result<LogScreen, LogsError>),
    /// A step of a running hand-off has started.
    HandoffStep(Step),
    /// A finished hand-off (doc/0004). The job is boxed to keep the enum
    /// small.
    Handoff(Box<HandoffJob>, Result<Resumed, HandoffError>),
    /// A finished sidecar save, with its job number (doc/0002).
    Saved(u64, SaveJob, io::Result<Saved>),
    /// A finished look for other towers' sidecar changes (doc/0017).
    Checked(u64, io::Result<Reloaded>),
    /// The projects for the dialog of scan `id` (doc/0008).
    Projects(u64, ProjectList),
}

impl JobResult {
    /// Feeds the result to the app and returns its follow-up effects.
    pub fn apply(self, app: &mut App) -> Vec<Effect> {
        match self {
            JobResult::Dispatch(job, result) => app.dispatch_finished(job, result),
            JobResult::Orchestrator(job, result) => app.orchestrator_finished(job, result),
            JobResult::Stop(action, result) => app.stop_finished(action, result),
            JobResult::Remove(action, result) => app.remove_finished(action, result),
            JobResult::Logs(target, result) => app.logs_fetched(target, result),
            JobResult::HandoffStep(step) => {
                app.handoff_step(step);
                Vec::new()
            }
            JobResult::Handoff(job, result) => app.handoff_finished(*job, result),
            JobResult::Saved(seq, job, result) => app.sidecar_saved(seq, &job, result),
            JobResult::Checked(seq, result) => app.sidecar_checked(seq, result),
            JobResult::Projects(id, list) => {
                app.projects_found(id, list);
                Vec::new()
            }
        }
    }
}

/// Starts a job's thread: runs the closure on a new thread named `name`, or
/// returns why it could not. Replaceable for tests.
type ThreadSpawner = fn(String, Box<dyn FnOnce() + Send>) -> io::Result<()>;

fn spawn_thread(name: String, body: Box<dyn FnOnce() + Send>) -> io::Result<()> {
    thread::Builder::new().name(name).spawn(body).map(drop)
}

/// Starts jobs and collects their results.
pub struct Jobs {
    runner: Arc<CliRunner>,
    handoff_timing: HandoffTiming,
    /// The sidecar file a hand-off checks before waking (doc/0017).
    state_file: Option<PathBuf>,
    spawner: ThreadSpawner,
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
            state_file: None,
            spawner: spawn_thread,
            tx,
            rx,
        }
    }

    /// Uses `timing` for the stop wait of hand-offs (tests).
    pub fn with_handoff_timing(mut self, timing: HandoffTiming) -> Jobs {
        self.handoff_timing = timing;
        self
    }

    /// Hand-offs compare the session's record in `path` with
    /// [`HandoffJob::recorded`] right before stopping and before waking it,
    /// and stop there when another tower recorded another owner
    /// (doc/0017 section 8). Without it they do not look.
    pub fn with_state_file(mut self, path: PathBuf) -> Jobs {
        self.state_file = Some(path);
        self
    }

    /// Runs `work` for `job` on a worker thread named `tower-<name>`, with a
    /// sender for intermediate results. Exactly one final result is sent:
    /// `work`'s, or `fail(job, error)` when `work` panics or the thread
    /// cannot be started. Nothing is printed: the terminal belongs to the UI.
    fn launch<J: Clone + Send + 'static>(
        &self,
        name: &'static str,
        job: J,
        fail: fn(J, CliError) -> JobResult,
        work: impl FnOnce(&CliRunner, &Sender<JobResult>, J) -> JobResult + Send + 'static,
    ) {
        let runner = Arc::clone(&self.runner);
        let tx = self.tx.clone();
        let unstarted = job.clone();
        let body = Box::new(move || {
            let spare = job.clone();
            let result = match guarded(|| work(&runner, &tx, job)) {
                Ok(result) => result,
                Err(message) => fail(spare, worker_error(name, message)),
            };
            let _ = tx.send(result);
        });
        if let Err(err) = (self.spawner)(format!("tower-{name}"), body) {
            let error = worker_error(name, format!("cannot start its thread: {err}"));
            let _ = self.tx.send(fail(unstarted, error));
        }
    }

    /// Runs the dispatch of `job` (doc/0008).
    pub fn dispatch(&self, job: DispatchJob) {
        let fail = |job, err: CliError| JobResult::Dispatch(job, Err(err.into()));
        self.launch("dispatch", job, fail, |runner, _, job| {
            let result = dispatch::run(runner, &job.request);
            JobResult::Dispatch(job, result)
        });
    }

    /// Starts the orchestrator of `job` (doc/0010): creates its folder and
    /// instruction file when missing, naming the running `tower` binary in
    /// it, then runs its dispatch.
    pub fn orchestrator(&self, job: OrchestratorJob) {
        let fail = |job, err: CliError| {
            JobResult::Orchestrator(job, Err(StartError::Dispatch(err.into())))
        };
        self.launch("orchestrator", job, fail, |runner, _, job| {
            let tower = orchestrator::current_tower();
            let result = orchestrator::start(runner, &job.request, &tower);
            JobResult::Orchestrator(job, result)
        });
    }

    /// Runs `<account command> stop <id>` (doc/0009).
    pub fn stop(&self, action: SessionAction) {
        let fail = |action, err: CliError| JobResult::Stop(action, Err(err.into()));
        self.launch("stop", action, fail, |runner, _, action| {
            let result = actions::stop(runner, &action.account, &action.id);
            JobResult::Stop(action, result)
        });
    }

    /// Runs `<account command> rm <id>` (doc/0009).
    pub fn remove(&self, action: SessionAction) {
        let fail = |action, err: CliError| JobResult::Remove(action, Err(err.into()));
        self.launch("rm", action, fail, |runner, _, action| {
            let result = actions::remove(runner, &action.account, &action.id);
            JobResult::Remove(action, result)
        });
    }

    /// Runs the hand-off sequence of `job` (doc/0004): stop, wait, resume
    /// under the target. Each step is reported as it starts. A failed worker
    /// counts as a failed resume: the session may be stopped and was not
    /// continued.
    pub fn handoff(&self, job: HandoffJob) {
        let timing = self.handoff_timing;
        let state_file = self.state_file.clone();
        let fail = |job, err| JobResult::Handoff(Box::new(job), Err(HandoffError::ResumeCli(err)));
        self.launch("handoff", job, fail, move |runner, tx, job| {
            let mut progress = |step: Step| {
                let _ = tx.send(JobResult::HandoffStep(step));
            };
            let mut check = |stopped: bool| match &state_file {
                Some(path) => owner_unchanged(path, &job, stopped),
                None => Ok(()),
            };
            let result =
                handoff::run_checked(runner, &job.request, &timing, &mut progress, &mut check);
            JobResult::Handoff(Box::new(job), result)
        });
    }

    /// Runs a sidecar save or check on a worker thread (doc/0002); the
    /// file is `path`.
    pub fn sidecar(&self, job: SidecarJob, path: PathBuf) {
        let fail = |(job, _): (SidecarJob, PathBuf), err: CliError| {
            let err = io::Error::other(err.to_string());
            match job {
                SidecarJob::Save { seq, job } => JobResult::Saved(seq, job, Err(err)),
                SidecarJob::Check { seq, .. } => JobResult::Checked(seq, Err(err)),
            }
        };
        self.launch(
            "sidecar",
            (job, path),
            fail,
            |_, _, (job, path)| match job {
                SidecarJob::Save { seq, job } => {
                    let result = job.run(&path, LOCK_WAIT);
                    JobResult::Saved(seq, job, result)
                }
                SidecarJob::Check { seq, job } => JobResult::Checked(seq, job.run(&path)),
            },
        );
    }

    /// Scans for the dialog's candidate projects (doc/0008). A failed
    /// worker gives an empty list with the reason as a warning.
    pub fn projects(&self, scan: ProjectScan) {
        let fail = |scan: ProjectScan, err: CliError| {
            let list = ProjectList {
                projects: Vec::new(),
                warnings: vec![format!("the project scan failed: {err}")],
            };
            JobResult::Projects(scan.id, list)
        };
        self.launch("projects", scan, fail, |_, _, scan| {
            let list = projects::discover(&scan.roots, &scan.extras, &scan.recent, &scan.home);
            JobResult::Projects(scan.id, list)
        });
    }

    /// Runs `<account command> logs <id>` and replays the stream (doc/0009).
    pub fn logs(&self, target: LogsTarget) {
        let fail = |target, err: CliError| JobResult::Logs(target, Err(err.into()));
        self.launch("logs", target, fail, |runner, _, target| {
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

/// The hand-off's guard (doc/0017 section 8): an error when the sidecar
/// file now records the session under another account than the one the
/// hand-off stops it with, and the record differs from the one this tower
/// planned with, i.e. another tower changed it. An unreadable file gives no
/// evidence and lets the hand-off go on.
fn owner_unchanged(
    path: &std::path::Path,
    job: &HandoffJob,
    stopped: bool,
) -> Result<(), HandoffError> {
    let Ok(now) = sidecar::read_owner(path, &job.request.session_id) else {
        return Ok(());
    };
    if now == job.recorded {
        return Ok(());
    }
    let Some(account) = now.and_then(|r| r.account) else {
        return Ok(());
    };
    let owner = job.request.owner.as_ref().map(|a| a.name.as_str());
    if owner == Some(account.as_str()) {
        return Ok(());
    }
    Err(HandoffError::OwnerChanged { account, stopped })
}

fn worker_error(job: &str, message: String) -> CliError {
    CliError::Worker {
        job: job.to_owned(),
        message,
    }
}

thread_local! {
    /// Set while this thread runs a job's work ([`guarded`]).
    static IN_JOB: Cell<bool> = const { Cell::new(false) };
    /// The panic hook's message for the job running on this thread.
    static JOB_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The last panic message of a background thread that runs no job (e.g. the
/// poller), kept for the status line.
static STRAY_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Runs `work` on this thread, catching a panic: its message (as the panic
/// hook recorded it, with its location, else taken from the payload) is the
/// error. The message does not count as a stray panic. Used by the poller to
/// keep its threads alive (doc/0001).
pub fn catch_panic<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    guarded(work)
}

/// Runs `work`, catching a panic: its message (as recorded by the panic hook,
/// else taken from the payload) is the error.
fn guarded<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    IN_JOB.set(true);
    JOB_PANIC.take();
    let out = panic::catch_unwind(AssertUnwindSafe(work));
    IN_JOB.set(false);
    let recorded = JOB_PANIC.take();
    out.map_err(|payload| {
        recorded.unwrap_or_else(|| format!("panicked: {}", payload_text(&*payload)))
    })
}

/// The text of a panic payload (`panic!` with a literal or a format string).
fn payload_text(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s
    } else {
        "unknown cause"
    }
}

/// One line describing a panic, for the status line: where and why.
pub fn panic_message(info: &panic::PanicHookInfo<'_>) -> String {
    let why = payload_text(info.payload());
    match info.location() {
        Some(at) => format!("panicked at {at}: {why}"),
        None => format!("panicked: {why}"),
    }
}

/// Records the panic `message` of a thread other than the main one, from
/// the panic hook; prints nothing. A job worker's message goes into its
/// error result, any other thread's is kept for [`take_stray_panic`].
pub fn record_panic(message: String) {
    if IN_JOB.try_with(Cell::get).unwrap_or(false) {
        let _ = JOB_PANIC.try_with(|slot| *slot.borrow_mut() = Some(message));
    } else {
        let mut slot = STRAY_PANIC
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(message);
    }
}

/// The last panic of a background thread that runs no job, once.
pub fn take_stray_panic() -> Option<String> {
    STRAY_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::Account;

    fn target() -> LogsTarget {
        LogsTarget {
            account: Account::new("main", "claude", None, Path::new("/home/user")).unwrap(),
            id: "a1b2c3d4".to_owned(),
        }
    }

    fn result(jobs: &Jobs) -> JobResult {
        jobs.recv_timeout(Duration::from_secs(10))
            .expect("every job reports back")
    }

    #[test]
    fn a_panicking_job_yields_an_error_result_of_its_kind() {
        let jobs = Jobs::new(CliRunner::new());
        let fail = |target, err: CliError| JobResult::Logs(target, Err(err.into()));
        jobs.launch("logs", target(), fail, |_, _, _| panic!("replay broke"));
        let JobResult::Logs(got, Err(LogsError::Cli(CliError::Worker { job, message }))) =
            result(&jobs)
        else {
            panic!("expected a logs worker error");
        };
        assert_eq!(got, target());
        assert_eq!(job, "logs");
        assert!(message.contains("replay broke"), "{message}");
        assert!(jobs.try_recv().is_none(), "exactly one result");
    }

    #[test]
    fn a_panic_recorded_by_the_hook_keeps_its_location() {
        // What the panic hook does on a worker thread, without installing a
        // process-wide hook in a test binary.
        let out = thread::spawn(|| {
            guarded(|| {
                record_panic("panicked at src/x.rs:1:2: boom".to_owned());
                panic!("boom")
            })
        })
        .join()
        .unwrap();
        assert_eq!(out.unwrap_err(), "panicked at src/x.rs:1:2: boom");
        assert!(take_stray_panic().is_none());
    }

    #[test]
    fn a_thread_that_cannot_start_yields_an_error_result() {
        let mut jobs = Jobs::new(CliRunner::new());
        jobs.spawner = |_, _| Err(io::Error::other("Resource temporarily unavailable"));
        jobs.logs(target());
        let JobResult::Logs(got, Err(err)) = result(&jobs) else {
            panic!("expected a logs error");
        };
        assert_eq!(got, target());
        assert!(
            matches!(&err, LogsError::Cli(CliError::Worker { job, .. }) if job == "logs"),
            "{err:?}"
        );
        assert!(err.to_string().contains("cannot start its thread"), "{err}");

        let action = SessionAction {
            account: target().account,
            id: "a1b2c3d4".to_owned(),
            name: "demo".to_owned(),
        };
        jobs.stop(action.clone());
        assert!(matches!(
            result(&jobs),
            JobResult::Stop(a, Err(StopError::Cli(CliError::Worker { .. }))) if a == action
        ));
        jobs.remove(action.clone());
        assert!(matches!(
            result(&jobs),
            JobResult::Remove(a, Err(RemoveError::Cli(CliError::Worker { .. }))) if a == action
        ));
    }
}
