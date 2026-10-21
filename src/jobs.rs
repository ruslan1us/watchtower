//! Background jobs: the CLI calls behind the app's [`Effect`]s that can take
//! a while — dispatch (`claude --bg`, which may first start a supervisor),
//! the orchestrator's start, `stop`, `rm`, the hand-off sequence,
//! the rename of a stopped session and the quick reply — and the file work that
//! could block on a slow disk or a hung network mount — sidecar saves and
//! reloads, the project scan, the template load — each run on its own
//! short-lived worker thread so the UI never blocks (doc/0001, doc/0002,
//! doc/0004, doc/0008, doc/0009, doc/0027, doc/0036, doc/0040). Results come back over a channel as [`JobResult`]s;
//! [`JobResult::apply`] hands them to the [`App`]. A hand-off, a rename and
//! a reply also report each step as it starts ([`JobResult::HandoffStep`],
//! [`JobResult::RenameStep`], [`JobResult::ReplyStep`]).
//!
//! Like every background call, these run with stdin null and captured
//! output ([`CliRunner`]), so nothing can touch the terminal (doc/0005).
//!
//! Every job reports back exactly once (doc/0001): a job that panics, or
//! whose thread cannot be started, yields an error result of its own kind
//! ([`CliError::Worker`]), so no dialog or hand-off waits forever. The
//! logs panel's fetches run in the log follower ([`crate::logfollow`],
//! doc/0030). The panic hook (`tui`) restores the terminal only on the main
//! thread; on any other thread it only records the message here
//! ([`record_panic`]): a job worker's goes into its error result, any other
//! thread's waits for the status line ([`take_stray_panic`]).
//!
//! [`Effect`]: crate::app::Effect

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::actions::{self, Removal, StopError};
use crate::app::{
    App, ConfigCheckJob, DispatchJob, Effect, HandoffJob, OrchestratorJob, ProjectScan, RemoveJob,
    RenameJob, ReplyJob, SessionAction, SidecarJob,
};
use crate::cli::{CliError, CliRunner};
use crate::config::ConfigError;
use crate::config_edit::{self, ConfigSnapshot, SaveError, SaveOutcome};
use crate::dispatch::{self, DispatchError, Dispatched};
use crate::gitinfo::{GitJob, GitRunner, GitState};
use crate::handoff::{self, HandoffError, HandoffTiming, Resumed, Step};
use crate::orchestrator::{self, StartError};
use crate::projects::{self, ProjectList};
use crate::rename::{self, RenameError, RenameStep, RenameTiming, Renamed};
use crate::reply::{self, Replied, ReplyError, ReplyStep};
use crate::settings::{FolderJob, FolderResult};
use crate::sidecar::{self, LOCK_WAIT, Reloaded, SaveJob, Saved};
use crate::subagents::{self, Subagent};
use crate::templates::{TemplateList, TemplateLoad};

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
    /// `rm <id>`, with its fallback (doc/0034).
    Remove(RemoveJob, Removal),
    /// A step of a running hand-off has started.
    HandoffStep(Step),
    /// A finished hand-off (doc/0004). The job is boxed to keep the enum
    /// small.
    Handoff(Box<HandoffJob>, Result<Resumed, HandoffError>),
    /// A step of a running rename has started (doc/0027).
    RenameStep(RenameStep),
    /// A finished rename of a stopped session (doc/0027), boxed to keep
    /// the enum small.
    Rename(Box<RenameJob>, Result<Renamed, RenameError>),
    /// A step of the running reply to the session with this `sessionId`
    /// has started (doc/0040).
    ReplyStep(String, ReplyStep),
    /// A finished quick reply (doc/0040), boxed to keep the enum small.
    Reply(Box<ReplyJob>, Result<Replied, ReplyError>),
    /// A finished sidecar save, with its job number (doc/0002).
    Saved(u64, SaveJob, io::Result<Saved>),
    /// A finished look for other towers' sidecar changes (doc/0017).
    Checked(u64, io::Result<Reloaded>),
    /// The projects for the dialog of scan `id` (doc/0008).
    Projects(u64, ProjectList),
    /// A finished look for changes to `config.toml`, with the app's config
    /// epoch when it started; `None`: unchanged (doc/0022).
    ConfigChecked(u64, Option<ConfigSnapshot>),
    /// A finished save of the Settings window (doc/0022), boxed to keep the
    /// enum small.
    ConfigSaved(Box<Result<SaveOutcome, SaveError>>),
    /// The Settings window's folder checks and completion (doc/0024).
    Folders(FolderResult),
    /// The running subagents by `sessionId`, read from tower's files
    /// (doc/0029).
    Subagents(HashMap<String, Vec<Subagent>>),
    /// The templates folder listed for the picker of load `id`
    /// (doc/0036).
    Templates(u64, TemplateList),
    /// A finished `git status` for the details panel (doc/0037).
    Git(GitJob, GitState),
}

impl JobResult {
    /// Feeds the result to the app and returns its follow-up effects.
    pub fn apply(self, app: &mut App) -> Vec<Effect> {
        match self {
            JobResult::Dispatch(job, result) => app.dispatch_finished(job, result),
            JobResult::Orchestrator(job, result) => app.orchestrator_finished(job, result),
            JobResult::Stop(action, result) => app.stop_finished(action, result),
            JobResult::Remove(job, removal) => app.remove_finished(job, removal),
            JobResult::HandoffStep(step) => {
                app.handoff_step(step);
                Vec::new()
            }
            JobResult::Handoff(job, result) => app.handoff_finished(*job, result),
            JobResult::RenameStep(step) => {
                app.rename_step(step);
                Vec::new()
            }
            JobResult::Rename(job, result) => app.rename_finished(*job, result),
            JobResult::ReplyStep(session_id, step) => {
                app.reply_step(&session_id, step);
                Vec::new()
            }
            JobResult::Reply(job, result) => app.reply_finished(*job, result),
            JobResult::Saved(seq, job, result) => app.sidecar_saved(seq, &job, result),
            JobResult::Checked(seq, result) => app.sidecar_checked(seq, result),
            JobResult::Projects(id, list) => {
                app.projects_found(id, list);
                Vec::new()
            }
            JobResult::ConfigChecked(epoch, found) => app.config_checked(epoch, found),
            JobResult::ConfigSaved(result) => app.config_saved(*result),
            JobResult::Folders(result) => app.folders_checked(result),
            JobResult::Subagents(found) => {
                app.subagents_read(found);
                Vec::new()
            }
            JobResult::Templates(id, list) => app.templates_loaded(id, list),
            JobResult::Git(job, state) => {
                app.git_finished(job, state);
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
    rename_timing: RenameTiming,
    /// The sidecar file a hand-off checks before waking (doc/0017).
    state_file: Option<PathBuf>,
    spawner: ThreadSpawner,
    /// Runs the details panel's `git status` (doc/0037).
    git: Arc<GitRunner>,
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
            rename_timing: RenameTiming::default(),
            state_file: None,
            spawner: spawn_thread,
            git: Arc::new(GitRunner::new()),
            tx,
            rx,
        }
    }

    /// Uses `runner` for `git status` (tests).
    pub fn with_git_runner(mut self, runner: GitRunner) -> Jobs {
        self.git = Arc::new(runner);
        self
    }

    /// Runs `git status` in `job`'s folder (doc/0037). A worker that panics
    /// or cannot start reports the folder's git state as unavailable, so
    /// the folder is asked again later.
    pub fn git(&self, job: GitJob) {
        let git = Arc::clone(&self.git);
        let fail = |job, err: CliError| JobResult::Git(job, GitState::Unavailable(err.to_string()));
        self.launch("git", job, fail, move |_, _, job| {
            let state = git.status(&job.dir());
            JobResult::Git(job, state)
        });
    }

    /// Uses `timing` for the stop wait of hand-offs (tests).
    pub fn with_handoff_timing(mut self, timing: HandoffTiming) -> Jobs {
        self.handoff_timing = timing;
        self
    }

    /// Uses `timing` for the name confirmation of renames (tests).
    pub fn with_rename_timing(mut self, timing: RenameTiming) -> Jobs {
        self.rename_timing = timing;
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

    /// Reads (and now and then prunes) the subagent files of `job`
    /// (doc/0029). A read that panics or cannot start reports none.
    pub fn subagents(&self, job: subagents::ReadJob) {
        let tx = self.tx.clone();
        let body = Box::new(move || {
            let found = guarded(|| job.run(std::time::SystemTime::now())).unwrap_or_default();
            let _ = tx.send(JobResult::Subagents(found));
        });
        if (self.spawner)("tower-subagents".to_owned(), body).is_err() {
            let _ = self.tx.send(JobResult::Subagents(HashMap::new()));
        }
    }

    /// Runs `<account command> stop <id>` (doc/0009).
    pub fn stop(&self, action: SessionAction) {
        let fail = |action, err: CliError| JobResult::Stop(action, Err(err.into()));
        self.launch("stop", action, fail, |runner, _, action| {
            let result = actions::stop(runner, &action.account, &action.id);
            JobResult::Stop(action, result)
        });
    }

    /// Runs `<account command> rm <id>`, then, after the owner's service
    /// refusal, the other accounts' commands the job allows (doc/0009,
    /// doc/0034).
    pub fn remove(&self, job: RemoveJob) {
        let fail = |job, err: CliError| JobResult::Remove(job, Err(err.into()).into());
        self.launch("rm", job, fail, |runner, _, job| {
            let removal = actions::remove_with_fallback(
                runner,
                &job.action.account,
                &job.fallback,
                &job.action.id,
            );
            JobResult::Remove(job, removal)
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

    /// Runs the rename of a stopped session (doc/0027): wake it with
    /// `/rename` under its owner, wait for the new name, stop it again.
    /// Each step is reported as it starts. Right before the wake the
    /// session's record in the state file is compared with
    /// [`RenameJob::recorded`], as for a hand-off (doc/0017 section 8). A
    /// failed worker counts as a failed wake.
    pub fn rename(&self, job: RenameJob) {
        let timing = self.rename_timing;
        let state_file = self.state_file.clone();
        let fail = |job, err| JobResult::Rename(Box::new(job), Err(RenameError::ResumeCli(err)));
        self.launch("rename", job, fail, move |runner, tx, job| {
            let mut progress = |step: RenameStep| {
                let _ = tx.send(JobResult::RenameStep(step));
            };
            let mut check = || match &state_file {
                Some(path) => rename_owner_unchanged(path, &job),
                None => Ok(()),
            };
            let result = rename::run(runner, &job.request, &timing, &mut progress, &mut check);
            JobResult::Rename(Box::new(job), result)
        });
    }

    /// Runs the quick reply of `job` (doc/0040): stop a live session and
    /// wait until it is gone, then wake it under its owner with the text.
    /// Each step is reported as it starts. Right before the stop and before
    /// the wake the session's record in the state file is compared with
    /// [`ReplyJob::recorded`], as for a hand-off (doc/0017 section 8). A
    /// failed worker counts as a failed wake.
    pub fn reply(&self, job: ReplyJob) {
        let timing = self.handoff_timing;
        let state_file = self.state_file.clone();
        let fail = |job, err| JobResult::Reply(Box::new(job), Err(ReplyError::ResumeCli(err)));
        self.launch("reply", job, fail, move |runner, tx, job| {
            let session_id = job.request.session_id.clone();
            let mut progress = |step: ReplyStep| {
                let _ = tx.send(JobResult::ReplyStep(session_id.clone(), step));
            };
            let mut check = |stopped: bool| match &state_file {
                Some(path) => reply_owner_unchanged(path, &job, stopped),
                None => Ok(()),
            };
            let result = reply::run(runner, &job.request, &timing, &mut progress, &mut check);
            JobResult::Reply(Box::new(job), result)
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

    /// Looks for changes to `config.toml` and reads it when it changed
    /// (doc/0022). A failed worker reports a read error, so the next check
    /// tries again.
    pub fn config_check(&self, job: ConfigCheckJob) {
        let fail = |job: ConfigCheckJob, err: CliError| {
            let snapshot = ConfigSnapshot {
                stamp: None,
                found: true,
                result: Err(ConfigError::Read {
                    path: job.check.path.clone(),
                    source: io::Error::other(err.to_string()),
                }),
            };
            JobResult::ConfigChecked(job.epoch, Some(snapshot))
        };
        self.launch("config-check", job, fail, |_, _, job| {
            JobResult::ConfigChecked(job.epoch, job.check.run())
        });
    }

    /// Saves the Settings window's changes to `config.toml` (doc/0022).
    pub fn config_save(&self, job: config_edit::SaveJob) {
        let fail = |job: config_edit::SaveJob, err: CliError| {
            JobResult::ConfigSaved(Box::new(Err(SaveError::Io {
                what: "save",
                path: job.path().to_path_buf(),
                source: io::Error::other(err.to_string()),
            })))
        };
        self.launch("config-save", job, fail, |_, _, job| {
            JobResult::ConfigSaved(Box::new(job.run()))
        });
    }

    /// Looks at the Settings window's folders and completes a folder name
    /// (doc/0024). A failed worker reports nothing found, so the window
    /// can ask again.
    pub fn folders(&self, job: FolderJob) {
        let fail = |_: FolderJob, _: CliError| {
            JobResult::Folders(FolderResult {
                probed: Vec::new(),
                completed: None,
            })
        };
        self.launch("folders", job, fail, |_, _, job| {
            JobResult::Folders(job.run())
        });
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

    /// Lists the templates folder for a picker (doc/0036). A load that
    /// panics or cannot start reports the folder as unreadable.
    pub fn templates(&self, load: TemplateLoad) {
        let fail = |load: TemplateLoad, err: CliError| {
            let list = TemplateList {
                error: Some(format!("the template load failed: {err}")),
                ..TemplateList::default()
            };
            JobResult::Templates(load.id, list)
        };
        self.launch("templates", load, fail, |_, _, load| {
            JobResult::Templates(load.id, load.run())
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

/// The rename's guard, as [`owner_unchanged`] for a hand-off: an error when
/// the state file now records the session under another account than the
/// owner the rename wakes it with, and the record differs from the one this
/// tower planned with. An unreadable file gives no evidence.
fn rename_owner_unchanged(path: &std::path::Path, job: &RenameJob) -> Result<(), RenameError> {
    let Ok(now) = sidecar::read_owner(path, &job.request.session_id) else {
        return Ok(());
    };
    if now == job.recorded {
        return Ok(());
    }
    let Some(account) = now.and_then(|r| r.account) else {
        return Ok(());
    };
    if account == job.request.owner.name {
        return Ok(());
    }
    Err(RenameError::OwnerChanged { account })
}

/// The reply's guard, as [`owner_unchanged`] for a hand-off: an error when
/// the state file now records the session under another account than the
/// owner the reply stops and wakes it with, and the record differs from the
/// one this tower planned with. An unreadable file gives no evidence.
fn reply_owner_unchanged(
    path: &std::path::Path,
    job: &ReplyJob,
    stopped: bool,
) -> Result<(), ReplyError> {
    let Ok(now) = sidecar::read_owner(path, &job.request.session_id) else {
        return Ok(());
    };
    if now == job.recorded {
        return Ok(());
    }
    let Some(account) = now.and_then(|r| r.account) else {
        return Ok(());
    };
    if account == job.request.owner.name {
        return Ok(());
    }
    Err(ReplyError::OwnerChanged { account, stopped })
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
    use crate::actions::RemoveError;
    use crate::config::Account;

    fn action() -> SessionAction {
        SessionAction {
            account: Account::new("main", "claude", None, Path::new("/home/user")).unwrap(),
            id: "a1b2c3d4".to_owned(),
            name: "demo".to_owned(),
        }
    }

    fn result(jobs: &Jobs) -> JobResult {
        jobs.recv_timeout(Duration::from_secs(10))
            .expect("every job reports back")
    }

    #[test]
    fn a_panicking_job_yields_an_error_result_of_its_kind() {
        let jobs = Jobs::new(CliRunner::new());
        let fail = |action, err: CliError| JobResult::Stop(action, Err(err.into()));
        jobs.launch("stop", action(), fail, |_, _, _| panic!("stop broke"));
        let JobResult::Stop(got, Err(StopError::Cli(CliError::Worker { job, message }))) =
            result(&jobs)
        else {
            panic!("expected a stop worker error");
        };
        assert_eq!(got, action());
        assert_eq!(job, "stop");
        assert!(message.contains("stop broke"), "{message}");
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
        jobs.stop(action());
        let JobResult::Stop(got, Err(err)) = result(&jobs) else {
            panic!("expected a stop error");
        };
        assert_eq!(got, action());
        assert!(
            matches!(&err, StopError::Cli(CliError::Worker { job, .. }) if job == "stop"),
            "{err:?}"
        );
        assert!(err.to_string().contains("cannot start its thread"), "{err}");
        jobs.remove(action().into());
        assert!(matches!(
            result(&jobs),
            JobResult::Remove(j, Removal { result: Err(RemoveError::Cli(CliError::Worker { .. })), .. })
                if j.action == action()
        ));
    }
}
