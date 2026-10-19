//! The TUI state machine (doc/0014 milestones 1–5): the merged session
//! list with its view controls (grouping by state or project, collapsed
//! groups, the interactive toggle and the text filter, doc/0006), the
//! selection over headers and rows, the help overlay, the account picker, confirmations, the
//! new-session dialog ([`crate::dialog`], doc/0008), stop and remove, the
//! logs panel (doc/0009), the hand-off to another account (doc/0004), the
//! orchestrator key `o` (doc/0010), the tracked activity times and the
//! attention highlight with `m` (doc/0006, doc/0007, via
//! [`crate::activity`]), and the key table (doc/0011).
//!
//! Nothing here touches the terminal, spawns processes or does file I/O
//! (apart from the `/proc` reads of ownership rule 1, and resolving the
//! accounts' config dirs once when the app is created). Key presses, poll
//! results and the outcomes of background jobs go in; [`Effect`]s come out
//! and the runtime (`tui`, `jobs`) carries them out. Every CLI call that can
//! take a while (dispatch, stop, remove, logs, hand-off) and every file
//! operation (sidecar saves and reloads, the project scan) is an effect the
//! runtime runs on a worker thread, reporting back through the `*_finished`
//! / [`App::logs_fetched`] / `sidecar_*` methods. That keeps the whole flow
//! testable without a terminal (doc/0015).
//!
//! Windows that a background result opens (the hand-off trust window, the
//! `rm` refusal) never replace one that is open: they wait in a queue and
//! open, in order, once nothing else is open (doc/0011).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::actions::{RemoveError, StopError};
use crate::activity;
use crate::config::ConfigError;
use crate::config::{Account, Config, ConfigSource, UiStyle, UiTheme};
use crate::config_edit::{self, ConfigCheck, ConfigSnapshot, EditSession, SaveError, SaveOutcome};
use crate::dialog::{Dialog, DialogAction, DialogStatus, Field, ResumeAction, ResumeWindow};
use crate::dispatch::{self, DispatchError, DispatchRequest, Dispatched};
use crate::groups::{self, Group, GroupKey, Grouping};
use crate::handoff::{self, HandoffError, HandoffRequest, Outcome, Resumed, Step};
use crate::layout;
use crate::listing::Session;
use crate::logview::{LogScreen, LogsError};
use crate::merge::{
    AccountListing, CanonCache, EnvironReader, KnownProcesses, MergedSession, Ownership, Resolver,
};
use crate::orchestrator::{self, StartError};
use crate::paths;
use crate::poller::PollUpdate;
use crate::projects::ProjectList;
use crate::ptysize::TtyProbe;
use crate::settings::{FolderResult, SettingsAction, SettingsWindow};
use crate::sidecar::{
    self, AccountSource, CheckJob, OwnerRecord, RecentProject, Reloaded, SaveJob, Saved, Sidecar,
    StateTuple,
};

/// How long a transient status message stays visible.
pub const MESSAGE_TTL: Duration = Duration::from_secs(6);

/// How long `Enter` waits for the refresh before an attach (doc/0005); then
/// tower attaches anyway and says so.
pub const ATTACH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Something the runtime must do on the app's behalf, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave tower.
    Quit,
    /// Poll every account now.
    PollNow,
    /// Poll every account now and report the poll's cycle to
    /// [`App::attach_poll_started`]: an attach waits for it (doc/0005).
    PollForAttach,
    /// Write the sidecar file; it changed. The runtime takes the job from
    /// [`App::start_save`] (none while another sidecar job runs; it is then
    /// queued) and runs it on a worker thread, then calls
    /// [`App::sidecar_saved`].
    SaveSidecar,
    /// Look for changes another tower made to the sidecar file: the job
    /// from [`App::start_sidecar_check`], run on a worker thread, then
    /// [`App::sidecar_checked`] (doc/0017 section 8).
    CheckSidecar,
    /// Hand the terminal to `<account command> attach <id>` (doc/0005).
    Attach(AttachRequest),
    /// Open the new-session dialog (doc/0008): [`App::begin_project_scan`]
    /// opens it while the projects are scanned on a worker thread, then
    /// [`App::projects_found`] fills the list.
    OpenDialog,
    /// Scan the projects on a worker thread for a window that is already
    /// open (the resume window of `R`, doc/0019), then call
    /// [`App::projects_found`].
    ScanProjects(ProjectScan),
    /// Hand the terminal to `<account command> --resume` in a folder, Claude
    /// Code's picker of past conversations, exactly like an attach, then
    /// call [`App::resume_finished`] (doc/0019).
    ResumePast(ResumeRequest),
    /// Run `claude --bg` on a worker thread, then call
    /// [`App::dispatch_finished`] (doc/0008).
    Dispatch(DispatchJob),
    /// Suspend tower like an attach and run the interactive account command
    /// in the untrusted folder ([`dispatch::trust_command`]), then call
    /// [`App::trust_finished`], which retries the dispatch once (doc/0008).
    Trust(DispatchJob),
    /// Suspend tower like an attach and edit this task text in `$VISUAL`,
    /// else `$EDITOR`, then call [`App::editor_finished`] (doc/0008).
    EditTask(String),
    /// Run `<owning account command> stop <id>` on a worker thread, then
    /// call [`App::stop_finished`] (doc/0009).
    Stop(SessionAction),
    /// Run `<owning account command> rm <id>` on a worker thread, then call
    /// [`App::remove_finished`] (doc/0009).
    Remove(SessionAction),
    /// Run `<owning account command> logs <id>` and replay it on a worker
    /// thread, then call [`App::logs_fetched`] (doc/0009).
    FetchLogs(LogsTarget),
    /// Run the hand-off sequence ([`handoff::run`]) on a worker thread,
    /// reporting steps to [`App::handoff_step`], then call
    /// [`App::handoff_finished`] (doc/0004).
    Handoff(HandoffJob),
    /// Suspend tower like an attach and run the target account's
    /// interactive command in the session's folder
    /// ([`dispatch::trust_command`]), then call
    /// [`App::handoff_trust_finished`], which retries the hand-off once
    /// (doc/0004, doc/0008).
    HandoffTrust(HandoffJob),
    /// Prepare the orchestrator's folder and instruction file and run its
    /// `claude --bg` on a worker thread ([`orchestrator::start`]), then
    /// call [`App::orchestrator_finished`] (doc/0010).
    StartOrchestrator(OrchestratorJob),
    /// Suspend tower like an attach and run the default account's
    /// interactive command in the orchestrator's folder
    /// ([`dispatch::trust_command`]), then call
    /// [`App::orchestrator_trust_finished`], which retries the start once
    /// (doc/0008, doc/0010).
    OrchestratorTrust(OrchestratorJob),
    /// Look for changes to `config.toml`: the job from
    /// [`App::start_config_check`], run on a worker thread, then
    /// [`App::config_checked`] (doc/0022).
    CheckConfig,
    /// A reloaded config changed `poll_interval_secs`: the poller ticks at
    /// this interval from now on (doc/0022).
    SetPollInterval(Duration),
    /// Save the Settings window's changes to `config.toml` on a worker
    /// thread ([`config_edit::SaveJob::run`]), then call
    /// [`App::config_saved`] (doc/0022).
    SaveConfig(config_edit::SaveJob),
    /// Suspend tower like an attach and open this config file in
    /// `$VISUAL`, else `$EDITOR` ([`config_edit::edit_in_editor`]), then
    /// call [`App::config_editor_finished`] (doc/0022).
    EditConfig(PathBuf),
    /// Read this config file into a [`config_edit::EditSession`] and open
    /// the Settings window with it: [`App::settings_opened`] (doc/0024).
    OpenSettings(PathBuf),
    /// Look at the list editor's folders or complete a folder name on a
    /// worker thread ([`crate::settings::FolderJob::run`]), then call
    /// [`App::folders_checked`] (doc/0024).
    CheckFolders(crate::settings::FolderJob),
}

/// The note shown while the config's accounts differ from the ones tower
/// started with: they are not applied live (doc/0022).
pub const RESTART_NOTE: &str = "accounts changed — restart tower to apply";

/// A look at `config.toml` for changes (doc/0022), run on a worker thread;
/// report with [`App::config_checked`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCheckJob {
    /// The app's config epoch when the check started: a result from before
    /// a save or an editor run is dropped.
    pub epoch: u64,
    /// The check itself.
    pub check: ConfigCheck,
}

/// The start of the orchestrator session (doc/0010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorJob {
    /// The request from [`orchestrator::request`]; its project is the
    /// orchestrator's folder.
    pub request: DispatchRequest,
    /// This is the retry after the trust flow.
    pub after_trust: bool,
}

/// An orchestrator start that failed with `Workspace not trusted`: `Ctrl+T`
/// runs the trust flow and retries once (doc/0008, doc/0010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedOrchestrator {
    /// The start to retry.
    pub job: OrchestratorJob,
    /// The CLI's message.
    pub message: String,
}

/// A hand-off of one session to another account (doc/0004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffJob {
    /// What to stop and resume, with which accounts.
    pub request: HandoffRequest,
    /// This is the retry after the trust flow.
    pub after_trust: bool,
    /// The session's owner record as this tower last read or wrote it in
    /// the sidecar file. Right before stopping and before waking, the job
    /// compares the file with it and does not wake the session when
    /// another tower recorded another owner meanwhile (doc/0017 section 8).
    pub recorded: Option<OwnerRecord>,
}

/// Sidecar file work the runtime runs on a worker thread (doc/0002): at
/// most one at a time, numbered in the order started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarJob {
    /// A save; report with [`App::sidecar_saved`].
    Save { seq: u64, job: SaveJob },
    /// A check for another tower's changes; report with
    /// [`App::sidecar_checked`].
    Check { seq: u64, job: CheckJob },
}

/// The project scan of a new-session dialog, run on a worker thread
/// (doc/0008); report with [`App::projects_found`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectScan {
    /// Which dialog the scan is for.
    pub id: u64,
    /// `project_roots`, `~` expanded.
    pub roots: Vec<PathBuf>,
    /// `extra_projects`, `~` expanded.
    pub extras: Vec<PathBuf>,
    /// The sidecar's recent projects, for the order.
    pub recent: Vec<RecentProject>,
    /// For `~` in displayed paths.
    pub home: PathBuf,
}

/// A dispatch from the new-session dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchJob {
    /// The validated request.
    pub request: DispatchRequest,
    /// Attach to the new session once it has started (`Ctrl+Enter`).
    pub attach: bool,
    /// This is the automatic retry after the trust flow.
    pub after_trust: bool,
}

/// A stop or remove of one session, with the owning account (doc/0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAction {
    /// The owning account (doc/0003).
    pub account: Account,
    /// The short id; never the name.
    pub id: String,
    /// Display name, for messages.
    pub name: String,
}

/// Whose logs the panel shows: the owning account and the short id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsTarget {
    /// The owning account; `logs` only works with it (doc/0009).
    pub account: Account,
    /// The short id.
    pub id: String,
}

/// What to attach to, and with which account's command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachRequest {
    /// The owning account (doc/0003).
    pub account: Account,
    /// The short id; never the name (doc/0005).
    pub id: String,
    /// Display name, for the "attaching to …" line.
    pub name: String,
}

/// Claude Code's picker of past conversations to run (doc/0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRequest {
    /// The account whose command runs `--resume`; a conversation resumed
    /// and backgrounded there runs under it.
    pub account: Account,
    /// The project folder the picker starts in.
    pub folder: PathBuf,
}

/// How the picker of past conversations ended, reported back by the
/// runtime (doc/0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOutcome {
    /// Account name whose command was used.
    pub account: String,
    /// The folder, as shown to the user.
    pub folder: String,
    /// As [`AttachOutcome::result`].
    pub result: Result<Option<i32>, String>,
}

/// How an attach ended, reported back by the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachOutcome {
    /// Display name of the session.
    pub name: String,
    /// Account name whose command was used.
    pub account: String,
    /// `Ok(Some(code))` on exit, `Ok(None)` when killed by a signal, `Err`
    /// when the command could not be run at all.
    pub result: Result<Option<i32>, String>,
}

/// One line of the session list. Headers and rows can both be selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A group header (doc/0006, doc/0007).
    Header {
        /// The group: a state group, or a project in project mode.
        group: GroupKey,
        /// Its rows; with a filter, the matching ones.
        count: usize,
        /// How many of those rows are highlighted.
        highlighted: usize,
        /// The group is collapsed: its rows are not listed.
        collapsed: bool,
    },
    /// A session: index into [`App::sessions`].
    Row(usize),
}

/// What the selection is on, kept across rebuilds of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    /// A session row, by [`row_key`].
    Row(String),
    /// A group header.
    Header(GroupKey),
}

/// Where the selection goes when the list is rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// The same session or header; a session whose group is collapsed
    /// gives its header; otherwise the same position.
    Same,
    /// The first session row (the filter changed).
    FirstRow,
}

/// The text filter of the list (doc/0006).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filter {
    text: String,
    /// The input line has the keys.
    editing: bool,
}

/// The account picker shown before attaching to a session whose owner is
/// unknown (doc/0003 rule 4, doc/0005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    /// Sidecar key of the session (`sessionId`, or the short id).
    pub key: String,
    /// Short id to attach to.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `CLAUDE_CONFIG_DIR` of the live process when it matched no account.
    pub config_dir: Option<PathBuf>,
    /// Index into the configured accounts.
    pub selected: usize,
    /// What happens with the chosen account.
    pub purpose: Purpose,
}

/// What a row action asks the account picker for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Attach (doc/0005).
    Attach,
    /// Stop (doc/0009).
    Stop,
    /// Remove (doc/0009).
    Remove,
}

impl Purpose {
    /// The verb for messages: `attach`, `stop`, `remove`.
    pub fn verb(self) -> &'static str {
        match self {
            Purpose::Attach => "attach",
            Purpose::Stop => "stop",
            Purpose::Remove => "remove",
        }
    }
}

/// A destructive action waiting for `y` (doc/0009, doc/0011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Stop a working or blocked session; `detail` is its state label.
    Stop {
        action: SessionAction,
        detail: String,
    },
    /// Remove a session from the list.
    Remove { action: SessionAction },
    /// Hand off a session; every hand-off asks, since it moves the
    /// conversation and project context under the target account and its
    /// terms (doc/0004). `detail` is its state label; `interrupts` is set for
    /// a working or blocked session, whose stop interrupts the turn.
    Handoff {
        job: Box<HandoffJob>,
        detail: String,
        interrupts: bool,
    },
}

/// A running hand-off, for the status line and the row-action guard.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandoffProgress {
    name: String,
    /// Short id of the session being handed off.
    id: String,
    /// Its `sessionId`.
    session_id: String,
    text: String,
}

/// The question asked when the user quits while a hand-off runs (doc/0004,
/// doc/0011): quitting may leave the session stopped without having
/// continued, and the `handoff` record would be lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitPrompt {
    /// Display name of the session being handed off.
    pub name: String,
}

impl QuitPrompt {
    /// The question, e.g. `a hand-off of say-ok is running; quitting may
    /// leave it stopped — quit?`.
    pub fn text(&self) -> String {
        format!(
            "a hand-off of {} is running; quitting may leave it stopped — quit?",
            self.name
        )
    }
}

/// The target picker of a hand-off with more than one candidate account
/// (doc/0004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPicker {
    /// `sessionId` of the session.
    pub key: String,
    /// Display name.
    pub name: String,
    /// The owning account, `None` when unknown (`?`).
    pub owner: Option<String>,
    /// Candidate target accounts, in config order; never the owner, and
    /// only the owner's group unless `allow_cross_group_handoff` is set
    /// ([`handoff::targets`]).
    pub targets: Vec<String>,
    /// Index into `targets`.
    pub selected: usize,
}

/// A hand-off that failed with `Workspace not trusted` (doc/0004): `Ctrl+T`
/// runs the trust flow for the target account and retries once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedHandoff {
    /// The hand-off to retry.
    pub job: HandoffJob,
    /// The CLI's message.
    pub message: String,
}

/// What is drawn on top of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// Just the list.
    None,
    /// The key help.
    Help,
    /// The account picker.
    Picker(Picker),
    /// A yes/no confirmation.
    Confirm(Confirm),
    /// The hand-off target picker.
    Target(TargetPicker),
    /// A hand-off stopped at `Workspace not trusted`.
    Untrusted(Box<UntrustedHandoff>),
    /// The orchestrator's folder is not trusted by its account.
    OrchestratorUntrusted(Box<UntrustedOrchestrator>),
}

/// A scrollable text window above everything else, e.g. the full text of
/// an `rm` refusal (doc/0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Popup {
    /// Window title.
    pub title: String,
    /// The text, one entry per line (the renderer wraps long lines).
    pub lines: Vec<String>,
    /// First visible (wrapped) line; the renderer clamps it.
    pub scroll: usize,
}

/// A window a background result opened while another window was open; it
/// waits until nothing else is open (doc/0011).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Waiting {
    Overlay(Box<Overlay>),
    Popup(Popup),
}

/// An attach waiting for its refresh (doc/0005): a poll that started after
/// `Enter` and a look at the sidecar file, at most
/// [`ATTACH_CHECK_TIMEOUT`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingAttach {
    /// Sidecar key of the session.
    key: String,
    /// What to attach to, with the account known at `Enter`.
    request: AttachRequest,
    /// The account is the picker's answer: recorded as `asked` once the
    /// refresh still shows no owner (doc/0003).
    asked: bool,
    deadline: Instant,
    /// Cycle of the poll started for it, once the runtime reports it.
    cycle: Option<u64>,
    /// The fresh listings so far, by account: whether each lists the
    /// short id, or why it failed.
    listed: Vec<(String, Result<bool, String>)>,
    /// Sidecar jobs numbered from here started after `Enter`.
    sidecar_from: u64,
    /// One of them has finished.
    sidecar_done: bool,
}

/// The orchestrator as this tower started it (doc/0010).
#[derive(Debug, Clone, Default)]
struct OrchestratorState {
    /// A start runs (or its trust flow); `o` waits for it.
    starting: bool,
    /// The session this tower just started, until a listing shows it: `o`
    /// attaches to it instead of starting a second one. Dropped after
    /// [`JUST_STARTED_POLLS`] good listings of its account without it.
    started: Option<JustStarted>,
}

/// A just started orchestrator not listed yet.
#[derive(Debug, Clone)]
struct JustStarted {
    account: Account,
    id: String,
    /// Good listings of `account` since, none of them showing `id`.
    misses: u32,
}

/// How many good listings of its account may miss a just started
/// orchestrator before `o` stops counting on it.
const JUST_STARTED_POLLS: u32 = 3;

/// Sidecar file jobs: one at a time, plus at most one queued save and one
/// queued check (doc/0002).
#[derive(Debug, Clone, Default)]
struct SidecarIo {
    /// Number of the job in flight.
    busy: Option<u64>,
    save_queued: bool,
    check_queued: bool,
    next_seq: u64,
}

impl PendingAttach {
    /// What the fresh listing of `account` said, once it arrived.
    fn listed_by(&self, account: &str) -> Option<&Result<bool, String>> {
        self.listed
            .iter()
            .find(|(a, _)| a == account)
            .map(|(_, r)| r)
    }
}

/// The config reload check in flight (doc/0022).
#[derive(Debug, Default)]
struct ConfigIo {
    /// A check runs; at most one at a time.
    busy: bool,
    /// Bumped by every save and editor run: a check that started before
    /// is stale and dropped.
    epoch: u64,
    /// The stamp of the config file version tower last read or wrote;
    /// `None` at start, so the first check reads the file once.
    stamp: Option<crate::sidecar::FileStamp>,
    /// Why the last reload was refused; shown until a reload succeeds.
    error: Option<String>,
    /// [`RESTART_NOTE`] while the file's accounts differ from tower's.
    restart_note: Option<&'static str>,
    /// The last finished save, for the Settings window to take.
    saved: Option<Result<SaveOutcome, SaveError>>,
}

impl SidecarIo {
    fn take_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.busy = Some(seq);
        seq
    }
}

/// What the logs panel shows.
#[derive(Debug, Clone, PartialEq)]
pub enum LogsView {
    /// A short note instead of output (stopped, interactive, unknown owner…).
    Note(String),
    /// The first fetch for this row is running.
    Loading,
    /// The replayed screen of the latest fetch.
    Screen(LogScreen),
    /// The latest fetch failed.
    Error(String),
}

/// The logs panel of the selected row (doc/0009).
#[derive(Debug, Clone, PartialEq)]
pub struct LogsPanel {
    /// Name and account of the row, for the panel title.
    pub title: String,
    /// What to draw.
    pub view: LogsView,
    /// The row the panel shows, when it has fetchable logs.
    target: Option<LogsTarget>,
    /// The fetch currently running, if any (at most one at a time).
    in_flight: Option<LogsTarget>,
    /// Fetch again as soon as the running fetch reports back.
    pending: bool,
}

impl LogsPanel {
    /// The row whose logs are fetched, if any.
    pub fn target(&self) -> Option<&LogsTarget> {
        self.target.as_ref()
    }

    /// The fetch currently running, if any.
    pub fn in_flight(&self) -> Option<&LogsTarget> {
        self.in_flight.as_ref()
    }
}

/// The note for a row without a live process (doc/0009).
pub const NOTE_STOPPED: &str = "no live output — session stopped; attach to view it";
/// The note for an interactive row.
pub const NOTE_INTERACTIVE: &str =
    "interactive session in another terminal — tower shows no logs for it";
/// The note for a row whose owning account is unknown (doc/0009).
pub const NOTE_UNKNOWN_OWNER: &str = "owning account unknown (?) — logs need the account that hosts the session; attach once to choose it";

/// Severity of a status-line segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// One piece of the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSegment {
    pub text: String,
    pub severity: Severity,
}

#[derive(Debug, Clone)]
struct Message {
    text: String,
    severity: Severity,
    at: Instant,
}

/// Per-account poll state.
#[derive(Debug, Clone)]
struct AccountPoll {
    name: String,
    /// Last good rows; kept after a failed poll (doc/0001).
    rows: Option<Vec<Session>>,
    /// Error of the latest poll, when it failed.
    error: Option<String>,
    /// Warnings of the latest successful listing (skipped rows, …).
    warnings: Vec<String>,
    /// Poll cycle of the last good rows ([`PollUpdate::cycle`]).
    cycle: u64,
    /// When the last good rows arrived, as [`App::received`] numbered them.
    received: u64,
}

/// List actions, looked up from keys through [`list_action`] (doc/0011:
/// table-driven so remapping can be added later).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    Attach,
    NewSession,
    Stop,
    Remove,
    /// `a`: continue under another account (doc/0004).
    Handoff,
    /// `o`: attach to the orchestrator, starting it if missing (doc/0010).
    Orchestrator,
    /// `R`: resume a past conversation into the background through Claude
    /// Code's own picker (doc/0019).
    ResumePast,
    /// `m`: mark the highlighted row as seen (doc/0007).
    MarkSeen,
    ToggleLogs,
    /// `Esc`: cancel a waiting attach, else close the logs panel, else
    /// clear the filter.
    Close,
    Refresh,
    Help,
    Quit,
    /// `Ctrl+Z`: reserved for detaching inside a session; a no-op here.
    DetachKeyHint,
    /// `/`: type or edit the text filter (doc/0006).
    Filter,
    /// `p`: group by state / by project (doc/0006).
    ToggleGrouping,
    /// `i`: show / hide the Interactive group (doc/0006).
    ToggleInteractive,
    /// `Tab`: collapse / expand the group of the selected line (doc/0006).
    ToggleGroup,
    /// `c`: the Settings window (doc/0024).
    Settings,
}

/// Keys of the session list (doc/0011), for the help. Short enough that
/// the help fits a 24-row terminal.
pub const KEY_TABLE: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move selection"),
    (
        "PgUp/PgDn",
        "move one page; Home/End, g/G: first / last line",
    ),
    ("Enter", "attach; on a group header: collapse / expand"),
    ("Tab", "collapse / expand the group of the selected line"),
    ("/", "filter by name, project, account; Esc clears it"),
    ("p, i", "group by state / project; show / hide interactive"),
    ("n, o", "new session; attach to the orchestrator (start it)"),
    ("R", "resume a past conversation (claude --resume)"),
    ("a", "continue under another account (hand-off)"),
    ("s, d", "stop (confirm when working or blocked); remove"),
    ("m", "mark the highlighted session as seen"),
    ("Space, Esc", "toggle / close the logs panel"),
    ("c, r, ?", "settings; refresh now; this help"),
    ("q, Ctrl+C", "quit"),
    ("Ctrl+Z", "inside a session: return to tower"),
];

/// Keys of the new-session dialog, for the help (doc/0011).
pub const DIALOG_KEY_TABLE: &[(&str, &str)] = &[
    ("Tab/Shift+Tab", "next / previous field"),
    ("Ctrl+S", "start in the background"),
    ("Ctrl+Enter, Alt+Enter", "start and attach"),
    ("Ctrl+G", "edit the task in $VISUAL / $EDITOR"),
    ("Ctrl+T", "after \"not trusted\": open claude there, retry"),
    ("Esc", "cancel"),
];

/// Maps a key to a list action. Letters only match without Ctrl/Alt, so
/// `Ctrl+C` is never mistaken for `c`. `←` is never bound (it is Claude
/// Code's key, doc/0011).
pub fn list_action(key: &KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::Quit),
            KeyCode::Char('z') | KeyCode::Char('Z') => Some(Action::DetachKeyHint),
            _ => None,
        };
    }
    if alt {
        return None;
    }
    let action = match key.code {
        KeyCode::Up | KeyCode::Char('k') => Action::Up,
        KeyCode::Down | KeyCode::Char('j') => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Home | KeyCode::Char('g') => Action::First,
        KeyCode::End | KeyCode::Char('G') => Action::Last,
        KeyCode::Enter => Action::Attach,
        KeyCode::Esc => Action::Close,
        KeyCode::Char('n') => Action::NewSession,
        KeyCode::Char('s') => Action::Stop,
        KeyCode::Char('d') => Action::Remove,
        KeyCode::Char(' ') => Action::ToggleLogs,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('?') => Action::Help,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('a') => Action::Handoff,
        KeyCode::Char('o') => Action::Orchestrator,
        KeyCode::Char('R') => Action::ResumePast,
        KeyCode::Char('m') => Action::MarkSeen,
        KeyCode::Char('/') => Action::Filter,
        KeyCode::Char('p') => Action::ToggleGrouping,
        KeyCode::Char('i') => Action::ToggleInteractive,
        KeyCode::Tab => Action::ToggleGroup,
        KeyCode::Char('c') => Action::Settings,
        _ => return None,
    };
    Some(action)
}

/// The application state.
pub struct App {
    config: Config,
    home: PathBuf,
    environ: Box<dyn EnvironReader>,
    /// Processes rule 1 accepted on earlier polls, in memory only
    /// (doc/0017).
    known: KnownProcesses,
    /// Canonical config dirs: the accounts' resolved once, process values
    /// once each (doc/0001: no `canonicalize` per poll).
    canon: CanonCache,
    /// Tower's sidecar state; the runtime saves it on [`Effect::SaveSidecar`].
    pub sidecar: Sidecar,
    polls: Vec<AccountPoll>,
    /// Number of successful listings received so far: orders the accounts'
    /// last good rows by arrival (doc/0017).
    received: u64,
    /// Every merged session, in the display order of the current grouping,
    /// including rows a filter, a collapsed group or `i` hides.
    sessions: Vec<MergedSession>,
    /// The visible lines: headers and rows.
    entries: Vec<Entry>,
    /// Index into `entries`.
    selected: usize,
    selected_key: Option<Selected>,
    /// Grouping by state or by project (`p`, doc/0006).
    grouping: Grouping,
    /// Whether the Interactive group is shown (`i`, doc/0006).
    show_interactive: bool,
    /// Groups the user collapsed or expanded, against their default; kept
    /// for the session only (doc/0006).
    folded: HashMap<GroupKey, bool>,
    /// The text filter (`/`, doc/0006).
    filter: Filter,
    overlay: Overlay,
    dialog: Option<Dialog>,
    /// The resume window of `R` (doc/0019), when open.
    resume: Option<ResumeWindow>,
    /// Where the config came from, once the runtime says
    /// ([`App::set_config_source`]).
    config_source: Option<ConfigSource>,
    logs: Option<LogsPanel>,
    popup: Option<Popup>,
    /// Windows opened by background results while another was open.
    waiting: VecDeque<Waiting>,
    /// An attach waiting for its refresh.
    pending_attach: Option<PendingAttach>,
    /// Added to the message after the next attach (e.g. the refresh timed
    /// out).
    attach_note: Option<String>,
    /// Sidecar saves and checks on worker threads.
    sidecar_io: SidecarIo,
    /// Poll cycle of the last save a poll asked for: activity times are
    /// saved at most once per cycle (doc/0006).
    save_cycle: Option<u64>,
    /// Activity times changed after this cycle's save; the next cycle
    /// saves them.
    activity_pending: bool,
    /// Short id of the session the running attach went to: a successful
    /// attach acknowledges its highlight (doc/0007).
    attached_id: Option<String>,
    /// Why the last sidecar check failed; shown until one succeeds.
    check_error: Option<String>,
    /// `config.toml` reloads and saves (doc/0022).
    config_io: ConfigIo,
    /// The Settings window, when open (doc/0024).
    settings: Option<SettingsWindow>,
    /// The Settings window handed over to the editor (`e`): it opens again
    /// once the editor has saved a valid file.
    reopen_settings: bool,
    /// Number of the latest project scan; the open dialog waits for it.
    scan: u64,
    /// Whether tower runs as root (hides `bypassPermissions`, doc/0008).
    root: bool,
    /// The orchestrator's folder, `<config dir>/orchestrator` (doc/0010).
    orchestrator_dir: PathBuf,
    orchestrator: OrchestratorState,
    message: Option<Message>,
    handoff: Option<HandoffProgress>,
    /// Open while asking whether to quit during a hand-off; above
    /// everything else.
    quit_prompt: Option<QuitPrompt>,
    warnings: Vec<String>,
    last_refresh: Option<Instant>,
    /// Rows per page for PgUp/PgDn; the renderer keeps it current.
    pub page_size: usize,
    /// Scroll state of the list widget, kept between frames.
    pub list_state: ListState,
    /// Whether colours may be used (`NO_COLOR` unset).
    pub color: bool,
    /// The renderer (doc/0020). [`App::new`] starts with the minimal one;
    /// the runtime applies the config's `[ui] style`, as it applies
    /// `NO_COLOR` to [`App::color`].
    pub style: UiStyle,
    /// The dashboard's colour theme (doc/0021), from the config's
    /// `[ui] theme`; [`App::set_theme`] switches it live. `NO_COLOR`
    /// ([`App::color`]) overrides it.
    pub theme: UiTheme,
    /// The dashboard fell back to the minimal layout on a small terminal
    /// and the status line said so; said once per run.
    style_fallback_noted: bool,
    /// Whether the terminal reports `Ctrl+Enter` distinctly (keyboard
    /// enhancement is on); the dialog footer then shows it instead of
    /// `Alt+Enter` (doc/0011).
    pub keyboard_enhanced: bool,
}

impl App {
    /// Creates the app. `home` shortens paths for display; `environ` reads
    /// `/proc/<pid>/environ` for ownership (fakes in tests).
    pub fn new(
        config: Config,
        home: PathBuf,
        sidecar: Sidecar,
        environ: Box<dyn EnvironReader>,
    ) -> App {
        let polls = config
            .accounts
            .iter()
            .map(|a| AccountPoll {
                name: a.name.clone(),
                rows: None,
                error: None,
                warnings: Vec::new(),
                cycle: 0,
                received: 0,
            })
            .collect();
        let canon = CanonCache::for_accounts(&config.accounts, &home);
        let orchestrator_dir = orchestrator::dir(&paths::Dirs::new(home.clone(), None).config_dir);
        let theme = config.ui_theme;
        App {
            config,
            home,
            environ,
            known: KnownProcesses::default(),
            canon,
            sidecar,
            polls,
            received: 0,
            sessions: Vec::new(),
            entries: Vec::new(),
            selected: 0,
            selected_key: None,
            grouping: Grouping::State,
            show_interactive: true,
            folded: HashMap::new(),
            filter: Filter::default(),
            overlay: Overlay::None,
            dialog: None,
            resume: None,
            config_source: None,
            logs: None,
            popup: None,
            waiting: VecDeque::new(),
            pending_attach: None,
            attach_note: None,
            sidecar_io: SidecarIo::default(),
            save_cycle: None,
            activity_pending: false,
            attached_id: None,
            check_error: None,
            config_io: ConfigIo::default(),
            settings: None,
            reopen_settings: false,
            scan: 0,
            root: dispatch::is_root(),
            orchestrator_dir,
            orchestrator: OrchestratorState::default(),
            message: None,
            handoff: None,
            quit_prompt: None,
            warnings: Vec::new(),
            last_refresh: None,
            page_size: 10,
            list_state: ListState::default(),
            color: true,
            style: UiStyle::Minimal,
            theme,
            style_fallback_noted: false,
            keyboard_enhanced: false,
        }
    }

    /// Overrides the root check (tests; doc/0008 hides `bypassPermissions`
    /// for root).
    pub fn set_root(&mut self, root: bool) {
        self.root = root;
    }

    /// Sets tower's config dir; the orchestrator runs in its
    /// `orchestrator` folder (doc/0010). Without it, `~/.config/watchtower`.
    pub fn set_config_dir(&mut self, config_dir: &std::path::Path) {
        self.orchestrator_dir = orchestrator::dir(config_dir);
    }

    /// The orchestrator's folder (doc/0010).
    pub fn orchestrator_dir(&self) -> &std::path::Path {
        &self.orchestrator_dir
    }

    /// Records where the config came from (doc/0002). A missing file means
    /// the built-in defaults, which is easy to miss: tower then shows a
    /// warning in the status line.
    pub fn set_config_source(&mut self, source: ConfigSource) {
        if !source.found {
            self.add_warning(self.missing_config_warning(&source));
        }
        self.config_source = Some(source);
    }

    /// The warning for a missing config file.
    fn missing_config_warning(&self, source: &ConfigSource) -> String {
        format!(
            "no config at {}; using defaults — see config.example.toml",
            source.shown(Some(&self.home))
        )
    }

    /// The config file now exists (`found`) or not: updates the source and
    /// its warning.
    fn set_config_found(&mut self, found: bool) {
        let Some(mut source) = self.config_source.clone() else {
            return;
        };
        if source.found == found {
            return;
        }
        let warning = self.missing_config_warning(&source);
        source.found = found;
        if found {
            self.warnings.retain(|w| *w != warning);
        } else {
            self.add_warning(warning);
        }
        self.config_source = Some(source);
    }

    /// Where the config came from, when the runtime said.
    pub fn config_source(&self) -> Option<&ConfigSource> {
        self.config_source.as_ref()
    }

    /// The project list's text when no project was found (doc/0008): it
    /// names the config file and whether it was loaded.
    pub fn no_projects_text(&self) -> String {
        match &self.config_source {
            Some(source) if source.found => format!(
                "no projects found — set project_roots or extra_projects in {} (loaded)",
                source.shown(Some(&self.home))
            ),
            Some(source) => format!(
                "no projects found — no config at {} (using defaults); create it with \
                 project_roots or extra_projects (see config.example.toml)",
                source.shown(Some(&self.home))
            ),
            None => {
                "no projects found — set project_roots or extra_projects in config.toml".to_owned()
            }
        }
    }

    /// The resume window of `R`, when open (doc/0019).
    pub fn resume_window(&self) -> Option<&ResumeWindow> {
        self.resume.as_ref()
    }

    /// The new-session dialog, when open.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// The logs panel, when open.
    pub fn logs(&self) -> Option<&LogsPanel> {
        self.logs.as_ref()
    }

    /// The popup window, when open.
    pub fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    /// The popup, for the renderer to clamp its scroll offset.
    pub fn popup_mut(&mut self) -> Option<&mut Popup> {
        self.popup.as_mut()
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Home directory used for `~` shortening.
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }

    /// Merged sessions in display order.
    pub fn sessions(&self) -> &[MergedSession] {
        &self.sessions
    }

    /// The list lines: group headers and rows.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Index into [`App::entries`] of the selected line (a row or a group
    /// header), if any.
    pub fn selected_entry(&self) -> Option<usize> {
        (self.selected < self.entries.len()).then_some(self.selected)
    }

    /// The group header that is selected, if the selection is on one.
    pub fn selected_header(&self) -> Option<&GroupKey> {
        match self.entries.get(self.selected_entry()?)? {
            Entry::Header { group, .. } => Some(group),
            Entry::Row(_) => None,
        }
    }

    /// The current grouping (`p`, doc/0006).
    pub fn grouping(&self) -> Grouping {
        self.grouping
    }

    /// Whether the Interactive group is shown (`i`, doc/0006).
    pub fn shows_interactive(&self) -> bool {
        self.show_interactive
    }

    /// Whether `group` is collapsed; while a filter is active every group
    /// shows its matches (doc/0006).
    pub fn is_collapsed(&self, group: &GroupKey) -> bool {
        !self.filter_active() && !self.folded_open(group)
    }

    /// The group's remembered state: the user's choice this session, else
    /// its default.
    fn folded_open(&self, group: &GroupKey) -> bool {
        self.folded
            .get(group)
            .copied()
            .unwrap_or_else(|| group.default_expanded())
    }

    /// The filter text; empty when no filter is set.
    pub fn filter_text(&self) -> &str {
        &self.filter.text
    }

    /// Whether the filter's input line has the keys.
    pub fn filter_editing(&self) -> bool {
        self.filter.editing
    }

    /// Whether a non-empty filter restricts the list.
    pub fn filter_active(&self) -> bool {
        !self.filter.text.trim().is_empty()
    }

    /// Whether the session matches the filter (doc/0006): name, project
    /// (`cwd` as listed and with `~`) and account. True without a filter.
    pub fn matches_filter(&self, m: &MergedSession) -> bool {
        let s = &m.session;
        let short = paths::shorten_home(&s.cwd, &self.home);
        groups::filter_matches(
            &self.filter.text,
            &[&s.display_name(), &s.cwd, &short, m.owner.label()],
        )
    }

    /// Number of sessions the filter lets through, whatever the groups'
    /// collapse state; interactive rows only while they are shown.
    pub fn filter_match_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|m| self.show_interactive || !m.session.is_interactive())
            .filter(|m| self.matches_filter(m))
            .count()
    }

    /// The line the list shows when it has nothing to draw after the first
    /// poll: no sessions at all, or every row hidden by the filter or the
    /// `i` toggle (doc/0006). Collapsed groups keep their headers, so they
    /// never empty the list on their own.
    pub fn empty_list_text(&self) -> String {
        if self.sessions.is_empty() {
            return "No sessions. Press n to start one.".to_owned();
        }
        let mut reasons = Vec::new();
        if self.filter_active() {
            reasons.push(format!(
                "no match for filter \"{}\" (Esc clears)",
                self.filter.text.trim()
            ));
        }
        if !self.show_interactive {
            let hidden = self
                .sessions
                .iter()
                .filter(|m| m.session.is_interactive())
                .count();
            if hidden > 0 {
                reasons.push(format!("{hidden} interactive hidden (i shows)"));
            }
        }
        if reasons.is_empty() {
            "Nothing to show.".to_owned()
        } else {
            format!("Nothing to show: {}", reasons.join("; "))
        }
    }

    /// Number of highlighted sessions the filter hides: the attention
    /// counter still counts them and says "(filtered)" (doc/0006, doc/0007).
    pub fn attention_filtered(&self) -> usize {
        if !self.filter_active() {
            return 0;
        }
        self.sessions
            .iter()
            .filter(|m| self.is_highlighted(m) && !self.matches_filter(m))
            .count()
    }

    /// The selected session, if any.
    pub fn selected_session(&self) -> Option<&MergedSession> {
        match self.entries.get(self.selected_entry()?)? {
            Entry::Row(i) => self.sessions.get(*i),
            Entry::Header { .. } => None,
        }
    }

    /// When the session last changed state, Unix ms (doc/0006): the same
    /// value `tower status --json` reports as `lastChangeAt`.
    pub fn last_change_at(&self, m: &MergedSession, now_ms: i64) -> Option<i64> {
        activity::last_change_at(&m.session, &self.sidecar, now_ms)
    }

    /// Whether the row is highlighted for attention (doc/0007).
    pub fn is_highlighted(&self, m: &MergedSession) -> bool {
        activity::highlighted(&m.session, &self.sidecar)
    }

    /// Number of highlighted sessions, for the status bar's `N need you`.
    /// Counts every row, whatever the view shows (doc/0007).
    pub fn attention_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|m| self.is_highlighted(m))
            .count()
    }

    /// Number of highlighted rows in `group`, for its header (doc/0007).
    pub fn highlighted_in(&self, group: Group) -> usize {
        self.sessions
            .iter()
            .filter(|m| groups::classify(&m.session) == group && self.is_highlighted(m))
            .count()
    }

    /// Acknowledges the highlight of the session with `key` (doc/0007):
    /// stores its current tuple as `acknowledged`. Only rows that need
    /// attention are acknowledged. Returns true when the sidecar changed.
    fn acknowledge(&mut self, key: &str) -> bool {
        let Some(m) = self.sessions.iter().find(|m| m.session.key() == Some(key)) else {
            return false;
        };
        if !activity::needs_attention(&m.session) {
            return false;
        }
        let tuple = StateTuple::of(&m.session);
        self.sidecar.acknowledge(key, &tuple)
    }

    /// `m`: marks the selected row as seen (doc/0007, doc/0011).
    fn mark_seen(&mut self) -> Vec<Effect> {
        let Some(m) = self.selected_session() else {
            return Vec::new();
        };
        let name = m.session.display_name();
        if !self.is_highlighted(m) {
            self.set_message(format!("{name} is not highlighted"), Severity::Info);
            return Vec::new();
        }
        let Some(key) = m.session.key().map(str::to_owned) else {
            return Vec::new();
        };
        if !self.acknowledge(&key) {
            return Vec::new();
        }
        self.set_message(format!("marked {name} as seen"), Severity::Info);
        vec![Effect::SaveSidecar]
    }

    /// The current overlay.
    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// The quit question while a hand-off runs, when open (doc/0004). It is
    /// drawn and handled above every other window.
    pub fn quit_prompt(&self) -> Option<&QuitPrompt> {
        self.quit_prompt.as_ref()
    }

    /// True once at least one account has reported (successfully or not).
    pub fn has_polled(&self) -> bool {
        self.last_refresh.is_some() || self.polls.iter().any(|p| p.error.is_some())
    }

    /// Adds a persistent warning (e.g. the sidecar was corrupt, a save failed).
    pub fn add_warning(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.warnings.contains(&text) {
            self.warnings.push(text);
        }
    }

    /// Switches the dashboard's colour theme; the next frame draws with it
    /// (doc/0021). For a settings window; no key changes it.
    pub fn set_theme(&mut self, theme: UiTheme) {
        self.theme = theme;
    }

    /// Called by the renderer when the dashboard falls back to the minimal
    /// layout because the terminal is smaller than `min` (columns, rows):
    /// the first time, the status line says so (doc/0020).
    pub fn note_style_fallback(&mut self, min: (u16, u16)) {
        if self.style_fallback_noted {
            return;
        }
        self.style_fallback_noted = true;
        self.set_message(
            format!(
                "the dashboard needs {}x{}; minimal layout until the terminal is larger",
                min.0, min.1
            ),
            Severity::Info,
        );
    }

    /// Shows a transient status message.
    pub fn set_message(&mut self, text: impl Into<String>, severity: Severity) {
        self.message = Some(Message {
            text: text.into(),
            severity,
            at: Instant::now(),
        });
    }

    /// The status line at time `now`: a running hand-off's progress, the
    /// transient message (if fresh), then the refresh time, per-account
    /// errors and warnings.
    pub fn status_segments(&self, now: Instant) -> Vec<StatusSegment> {
        let mut out = Vec::new();
        // The view's own state: the filter with its match count, hidden
        // interactive rows (doc/0006).
        if self.filter_active() {
            out.push(StatusSegment {
                text: format!(
                    "filter \"{}\": {}",
                    self.filter.text.trim(),
                    match self.filter_match_count() {
                        0 => "no matches".to_owned(),
                        1 => "1 match".to_owned(),
                        n => format!("{n} matches"),
                    }
                ),
                severity: Severity::Info,
            });
        }
        let hidden = if self.show_interactive {
            0
        } else {
            self.sessions
                .iter()
                .filter(|m| m.session.is_interactive())
                .count()
        };
        if hidden > 0 {
            out.push(StatusSegment {
                text: format!("{hidden} interactive hidden (i shows)"),
                severity: Severity::Info,
            });
        }
        // A running hand-off stays visible until it reports back.
        if let Some(h) = &self.handoff {
            out.push(StatusSegment {
                text: h.text.clone(),
                severity: Severity::Info,
            });
        }
        // So is an attach waiting for its refresh (doc/0005).
        if let Some(p) = &self.pending_attach {
            out.push(StatusSegment {
                text: format!("checking {}… (Esc cancels)", p.request.name),
                severity: Severity::Info,
            });
        }
        // And windows waiting for the open one to close (doc/0011).
        if !self.waiting.is_empty() {
            let n = self.waiting.len();
            let what = if n == 1 {
                "1 window is waiting".to_owned()
            } else {
                format!("{n} windows are waiting")
            };
            out.push(StatusSegment {
                text: format!("{what}; it opens when this one closes"),
                severity: Severity::Warning,
            });
        }
        if let Some(m) = &self.message
            && now.saturating_duration_since(m.at) < MESSAGE_TTL
        {
            out.push(StatusSegment {
                text: m.text.clone(),
                severity: m.severity,
            });
        }
        let refreshed = match self.last_refresh {
            Some(at) => format!(
                "refreshed {} ago",
                short_duration(now.saturating_duration_since(at))
            ),
            None if self.has_polled() => "no successful refresh yet".to_owned(),
            None => "loading…".to_owned(),
        };
        out.push(StatusSegment {
            text: refreshed,
            severity: Severity::Info,
        });
        for p in &self.polls {
            if let Some(err) = &p.error {
                let kept = if p.rows.is_some() {
                    " (showing last good list)"
                } else {
                    ""
                };
                out.push(StatusSegment {
                    text: format!("{}: {}{kept}", p.name, one_line(err)),
                    severity: Severity::Error,
                });
            }
            for w in &p.warnings {
                out.push(StatusSegment {
                    text: format!("{}: {}", p.name, one_line(w)),
                    severity: Severity::Warning,
                });
            }
        }
        if let Some(err) = &self.check_error {
            out.push(StatusSegment {
                text: err.clone(),
                severity: Severity::Warning,
            });
        }
        if let Some(err) = &self.config_io.error {
            out.push(StatusSegment {
                text: err.clone(),
                severity: Severity::Error,
            });
        }
        if let Some(note) = self.config_io.restart_note {
            out.push(StatusSegment {
                text: note.to_owned(),
                severity: Severity::Warning,
            });
        }
        for w in &self.warnings {
            out.push(StatusSegment {
                text: one_line(w),
                severity: Severity::Warning,
            });
        }
        out
    }

    /// Applies one finished poll: moves `started` records keyed by a short
    /// id to their `sessionId`, merges all accounts (each with the cycle and
    /// arrival order of its last good rows, doc/0017; the processes rule 1
    /// accepted carry over between polls), records detections and the
    /// listed rows' state tuples (doc/0006) in the sidecar and rebuilds the
    /// list. Returns [`Effect::SaveSidecar`] when the sidecar changed (for
    /// activity times alone, at most once per poll cycle), and a logs fetch
    /// when the panel is open and
    /// this was the poll of the shown session's account (one refresh per
    /// poll, doc/0009).
    pub fn apply_poll(&mut self, update: PollUpdate) -> Vec<Effect> {
        let Some(poll) = self.polls.iter_mut().find(|p| p.name == update.account) else {
            return Vec::new();
        };
        // The fresh listings an attach waits for (doc/0005): polls that
        // started after `Enter`.
        if let Some(p) = self.pending_attach.as_mut()
            && p.listed_by(&update.account).is_none()
            && p.cycle.is_some_and(|c| update.cycle >= c)
        {
            let listed = match &update.result {
                Ok(listing) => Ok(listing
                    .sessions
                    .iter()
                    .any(|s| s.id.as_deref() == Some(p.request.id.as_str()))),
                Err(err) => Err(err.clone()),
            };
            p.listed.push((update.account.clone(), listed));
        }
        let mut changed = false;
        let listed_ok = update.result.is_ok();
        let fresh = listed_ok.then(|| update.account.clone());
        match update.result {
            Ok(listing) => {
                // Before the merge, so rule 3 finds the record (doc/0008).
                changed |= self.sidecar.adopt_short_id_records(&listing.sessions);
                poll.rows = Some(listing.sessions);
                poll.warnings = listing.warnings;
                poll.error = None;
                poll.cycle = update.cycle;
                self.received += 1;
                poll.received = self.received;
                self.last_refresh = Some(Instant::now());
            }
            Err(err) => poll.error = Some(err),
        }
        let (owners, activity) = self.rebuild(fresh.as_deref());
        changed |= owners;
        self.track_started_orchestrator(&update.account, listed_ok);
        let mut effects = Vec::new();
        // A save that failed (e.g. the lock was busy) is retried. Changed
        // activity times alone are saved at most once per poll cycle; the
        // rest wait for the next cycle (doc/0006).
        let activity = activity || self.activity_pending;
        if changed
            || self.sidecar.retry_due()
            || (activity && self.save_cycle != Some(update.cycle))
        {
            effects.push(Effect::SaveSidecar);
            self.save_cycle = Some(update.cycle);
            self.activity_pending = false;
        } else if activity {
            self.activity_pending = true;
        }
        let refresh = self
            .logs
            .as_ref()
            .and_then(|l| l.target.as_ref())
            .is_some_and(|t| t.account.name == update.account);
        effects.extend(self.logs_follow(refresh));
        effects.extend(self.attach_progress(Instant::now()));
        effects
    }

    /// Merges the accounts' last listings with the sidecar and rebuilds the
    /// list (see [`App::apply_poll`]). `fresh` names the account whose
    /// listing just arrived: the tuples of its rows are recorded as activity
    /// (doc/0006). Rows of older listings are not, so a reload never records
    /// a stale tuple over another tower's newer one. Returns whether the
    /// sidecar's owners, seen times or records changed, and whether its
    /// activity times did.
    fn rebuild(&mut self, fresh: Option<&str>) -> (bool, bool) {
        let mut changed = false;
        let listings: Vec<AccountListing> = self
            .polls
            .iter()
            .map(|p| {
                let listing = match (&p.rows, &p.error) {
                    (Some(rows), None) => AccountListing::fresh(&p.name, rows.clone()),
                    (Some(rows), Some(_)) => AccountListing::stale(&p.name, rows.clone()),
                    (None, _) => AccountListing::missing(&p.name),
                };
                listing
                    .with_cycle(p.cycle)
                    .with_received(p.received)
                    .with_partial(!p.warnings.is_empty())
            })
            .collect();
        let mut resolver = Resolver::new(&self.config.accounts, &self.home, self.environ.as_ref())
            .with_known(std::mem::take(&mut self.known))
            .with_canon(std::mem::take(&mut self.canon));
        let merged = resolver.merge(&listings, &self.sidecar);
        (self.known, self.canon) = resolver.into_parts();
        changed |= self.sidecar.record_detections(&merged);
        let now_ms = sidecar::now_ms();
        let mut activity = false;
        if let Some(account) = fresh {
            for m in merged
                .iter()
                .filter(|m| m.seen_in.iter().any(|a| a == account))
            {
                if let Some(key) = m.session.key() {
                    activity |= self.sidecar.observe_state_at(
                        key,
                        &StateTuple::of(&m.session),
                        m.session.started_at,
                        now_ms,
                    );
                }
            }
        }
        // Last-seen times and pruning (doc/0017 section 9): rows of every
        // good listing count as seen; pruning only while every account's
        // latest poll succeeded with a full listing.
        let complete = self
            .polls
            .iter()
            .all(|p| p.rows.is_some() && p.error.is_none() && p.warnings.is_empty());
        let seen = self
            .polls
            .iter()
            .filter(|p| p.error.is_none())
            .flat_map(|p| p.rows.iter().flatten())
            .filter_map(Session::key);
        changed |= self.sidecar.observe_listing(seen, complete, now_ms);
        self.set_sessions(merged);
        (changed, activity)
    }

    /// [`Effect::Attach`] for `request`, remembering its session so a
    /// successful attach acknowledges the highlight (doc/0007).
    fn attach_effect(&mut self, request: AttachRequest) -> Effect {
        self.attached_id = Some(request.id.clone());
        Effect::Attach(request)
    }

    /// Reports how an attach ended and asks for an immediate poll. A
    /// successful attach (exit code 0) acknowledges the session's highlight
    /// with the tuple it had (doc/0007).
    pub fn attach_finished(&mut self, outcome: AttachOutcome) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(id) = self.attached_id.take()
            && matches!(outcome.result, Ok(Some(0)))
            && let Some(key) = self
                .sessions
                .iter()
                .find(|m| m.session.id.as_deref() == Some(id.as_str()))
                .and_then(|m| m.session.key())
                .map(str::to_owned)
            && self.acknowledge(&key)
        {
            effects.push(Effect::SaveSidecar);
        }
        effects.extend(self.attach_report(outcome));
        effects
    }

    fn attach_report(&mut self, outcome: AttachOutcome) -> Vec<Effect> {
        let who = format!("{} ({})", outcome.name, outcome.account);
        if let Some(note) = self.attach_note.take() {
            let severity = match outcome.result {
                Ok(Some(0)) => Severity::Warning,
                _ => Severity::Error,
            };
            let how = match &outcome.result {
                Ok(Some(0)) => format!("back from {who}"),
                Ok(Some(code)) => format!("attach to {who} exited with code {code}"),
                Ok(None) => format!("attach to {who} was ended by a signal"),
                Err(err) => format!("cannot attach to {who}: {}", one_line(err)),
            };
            self.set_message(format!("{how} ({note})"), severity);
            return vec![Effect::PollNow];
        }
        match outcome.result {
            Ok(Some(0)) => self.set_message(format!("back from {who}"), Severity::Info),
            Ok(Some(code)) => self.set_message(
                format!("attach to {who} exited with code {code}"),
                Severity::Error,
            ),
            Ok(None) => self.set_message(
                format!("attach to {who} was ended by a signal"),
                Severity::Error,
            ),
            Err(err) => self.set_message(
                format!("cannot attach to {who}: {}", one_line(&err)),
                Severity::Error,
            ),
        }
        vec![Effect::PollNow]
    }

    /// Opens the new-session dialog with the scanned projects (doc/0008).
    pub fn open_dialog(&mut self, projects: ProjectList) {
        self.overlay = Overlay::None;
        self.dialog = Some(Dialog::new(&self.config, projects, self.root));
    }

    /// Reports a finished dispatch (doc/0008). On success: record the
    /// `started` owner and the recent project, close the dialog, report,
    /// poll, and attach for dispatch-and-attach. On failure the dialog keeps
    /// its contents and shows the error; `Workspace not trusted` offers
    /// `Ctrl+T`.
    pub fn dispatch_finished(
        &mut self,
        job: DispatchJob,
        result: Result<Dispatched, DispatchError>,
    ) -> Vec<Effect> {
        match result {
            Ok(started) => {
                let mut effects = Vec::new();
                if dispatch::record_dispatch(
                    &mut self.sidecar,
                    &job.request,
                    &started,
                    &self.home,
                    sidecar::now_ms(),
                ) {
                    effects.push(Effect::SaveSidecar);
                }
                self.dialog = None;
                let name = started
                    .name
                    .clone()
                    .or_else(|| job.request.name_arg().map(str::to_owned))
                    .unwrap_or_else(|| started.id.clone());
                let account = job.request.account.clone();
                self.set_message(
                    format!("started {name} ({}) under {}", started.id, account.name),
                    Severity::Info,
                );
                if job.attach {
                    // attach_finished polls once tower is back.
                    effects.push(self.attach_effect(AttachRequest {
                        account,
                        id: started.id,
                        name,
                    }));
                } else {
                    effects.push(Effect::PollNow);
                }
                effects
            }
            Err(err) => {
                let Some(dialog) = self.dialog.as_mut() else {
                    self.set_message(
                        format!("new session failed: {}", one_line(&err.to_string())),
                        Severity::Error,
                    );
                    return Vec::new();
                };
                match err {
                    DispatchError::Untrusted { dir, message } => {
                        let mut text = message;
                        if job.after_trust {
                            text.push_str("\n(still not trusted after running claude there)");
                        }
                        dialog.error = Some(text);
                        dialog.status = DialogStatus::Untrusted {
                            dir,
                            attach: job.attach,
                        };
                    }
                    DispatchError::Invalid(v) => {
                        dialog.field = Field::of_error(&v);
                        dialog.error = Some(v.to_string());
                        dialog.status = DialogStatus::Editing;
                    }
                    other => {
                        dialog.error = Some(other.to_string());
                        dialog.status = DialogStatus::Editing;
                    }
                }
                Vec::new()
            }
        }
    }

    /// Reports the end of the trust flow (doc/0008): the interactive account
    /// command exited, so the same dispatch is retried once. When it could
    /// not run at all the dialog shows why and nothing is retried.
    pub fn trust_finished(
        &mut self,
        job: DispatchJob,
        result: Result<Option<i32>, String>,
    ) -> Vec<Effect> {
        match result {
            Ok(_) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.status = DialogStatus::Starting { attach: job.attach };
                    dialog.error = None;
                }
                vec![Effect::Dispatch(DispatchJob {
                    after_trust: true,
                    ..job
                })]
            }
            Err(err) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.error = Some(err);
                    dialog.status = DialogStatus::Untrusted {
                        dir: job.request.project.clone(),
                        attach: job.attach,
                    };
                } else {
                    self.set_message(one_line(&err), Severity::Error);
                }
                Vec::new()
            }
        }
    }

    /// Takes the task text back from the editor (doc/0008); an error leaves
    /// the task unchanged and is shown in the dialog.
    pub fn editor_finished(&mut self, result: Result<String, String>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        match result {
            Ok(text) => {
                dialog.set_task(&text);
                dialog.error = None;
            }
            Err(err) => dialog.error = Some(err),
        }
    }

    /// Reports a finished stop and polls (doc/0009).
    pub fn stop_finished(
        &mut self,
        action: SessionAction,
        result: Result<(), StopError>,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => self.set_message(
                format!("stopped {} ({})", action.name, action.id),
                Severity::Info,
            ),
            Err(err) => self.set_message(
                format!(
                    "cannot stop {}: {}",
                    action.name,
                    one_line(&err.to_string())
                ),
                Severity::Error,
            ),
        }
        vec![Effect::PollNow]
    }

    /// Reports a finished remove and polls (doc/0009). A refusal opens a
    /// scrollable window with the CLI's full text; tower never retries and
    /// never passes the flags it suggests.
    pub fn remove_finished(
        &mut self,
        action: SessionAction,
        result: Result<(), RemoveError>,
    ) -> Vec<Effect> {
        match result {
            Ok(()) => self.set_message(
                format!("removed {} ({})", action.name, action.id),
                Severity::Info,
            ),
            Err(RemoveError::Refused { code, message }) => {
                let code = code.map_or("a signal".to_owned(), |c| format!("code {c}"));
                let mut lines = vec![
                    format!(
                        "`rm {}` did not remove {} (exit {code}). Claude Code says:",
                        action.id, action.name
                    ),
                    String::new(),
                ];
                lines.extend(message.lines().map(str::to_owned));
                lines.extend([
                    String::new(),
                    "Tower never passes --discard-unpushed or --force-remove-worktree: they \
                     destroy work. Attach and commit or push, or run the suggested command \
                     yourself."
                        .to_owned(),
                ]);
                self.open_late(Waiting::Popup(Popup {
                    title: format!("{} was not removed", action.name),
                    lines,
                    scroll: 0,
                }));
                self.set_message(
                    format!("{} was not removed", action.name),
                    Severity::Warning,
                );
            }
            Err(err) => self.set_message(
                format!(
                    "cannot remove {}: {}",
                    action.name,
                    one_line(&err.to_string())
                ),
                Severity::Error,
            ),
        }
        vec![Effect::PollNow]
    }

    /// Takes a finished logs fetch (doc/0009). A result for a row the panel
    /// no longer shows is dropped, and the current row is fetched instead.
    pub fn logs_fetched(
        &mut self,
        target: LogsTarget,
        result: Result<LogScreen, LogsError>,
    ) -> Vec<Effect> {
        let Some(panel) = self.logs.as_mut() else {
            return Vec::new();
        };
        if panel.in_flight.as_ref() == Some(&target) {
            panel.in_flight = None;
        }
        let current = panel.target.as_ref() == Some(&target);
        if current {
            panel.view = match result {
                Ok(screen) => LogsView::Screen(screen),
                Err(LogsError::NotRunning) => LogsView::Note(NOTE_STOPPED.to_owned()),
                Err(LogsError::UnknownSession) => LogsView::Note(format!(
                    "account {} does not host this session (it may run under another \
                     account, or was removed)",
                    target.account.name
                )),
                Err(err) => LogsView::Error(one_line(&err.to_string())),
            };
        }
        if panel.in_flight.is_none() && (panel.pending || !current) {
            panel.pending = false;
            if let Some(next) = panel.target.clone() {
                panel.in_flight = Some(next.clone());
                return vec![Effect::FetchLogs(next)];
            }
        }
        Vec::new()
    }

    /// Handles one key press. Any way of quitting (`q`, `Ctrl+C`, the
    /// dialog's own quit) asks first while a hand-off runs (doc/0004).
    ///
    /// Keys typed under a Cyrillic layout are normalised here (doc/0011):
    /// `Ctrl`/`Alt` combinations everywhere, plain keys only in command
    /// contexts (see [`App::route_key`]), never in text input.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let key = layout::shortcut_key(key);
        if let Some(prompt) = self.quit_prompt.clone() {
            return self.quit_prompt_key(layout::command_key(key), prompt);
        }
        let ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'));
        if ctrl_c && self.handoff.is_some() {
            // Straight to the question, also from the new-session dialog,
            // which would otherwise ask its own first.
            return self.guard_quit(vec![Effect::Quit]);
        }
        let effects = self.route_key(key);
        let effects = self.guard_quit(effects);
        self.show_waiting();
        effects
    }

    /// While a hand-off runs, turns [`Effect::Quit`] into the quit
    /// question; other effects pass through.
    fn guard_quit(&mut self, effects: Vec<Effect>) -> Vec<Effect> {
        if !effects.contains(&Effect::Quit) {
            return effects;
        }
        let Some(running) = &self.handoff else {
            return effects;
        };
        self.quit_prompt = Some(QuitPrompt {
            name: running.name.clone(),
        });
        effects.into_iter().filter(|e| *e != Effect::Quit).collect()
    }

    /// `y` quits; anything that cancels (`n`, `Esc`, `Enter`: the default
    /// is no, doc/0011) closes the question; other keys are ignored.
    fn quit_prompt_key(&mut self, key: KeyEvent, prompt: QuitPrompt) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            _ if ctrl || alt => Vec::new(),
            KeyCode::Char('y' | 'Y') => {
                self.quit_prompt = None;
                vec![Effect::Quit]
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                self.quit_prompt = None;
                self.set_message(
                    format!("not quitting; the hand-off of {} continues", prompt.name),
                    Severity::Info,
                );
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Sends a key to the context that owns the keys. The new-session
    /// dialog, the resume window and the filter input take text, so they get
    /// the key as typed; every other context is a command context and gets
    /// Cyrillic letters as their Latin keys (doc/0011).
    fn route_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let text_input = self.popup.is_none()
            && (self.dialog.is_some()
                || self.resume.is_some()
                || self
                    .settings
                    .as_ref()
                    .is_some_and(SettingsWindow::takes_text)
                || (self.overlay == Overlay::None && self.filter.editing));
        let key = if text_input {
            key
        } else {
            layout::command_key(key)
        };
        if self.popup.is_some() {
            return self.popup_key(key);
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        if self.resume.is_some() {
            return self.resume_key(key);
        }
        if self.settings.is_some() {
            return self.settings_key(key);
        }
        match self.overlay.clone() {
            Overlay::Help => self.help_key(key),
            Overlay::Picker(picker) => self.picker_key(key, picker),
            Overlay::Confirm(confirm) => self.confirm_key(key, confirm),
            Overlay::Target(picker) => self.target_key(key, picker),
            Overlay::Untrusted(untrusted) => self.untrusted_key(key, *untrusted),
            Overlay::OrchestratorUntrusted(u) => self.orchestrator_untrusted_key(key, *u),
            Overlay::None if self.filter.editing => self.filter_key(key),
            Overlay::None => self.list_key(key),
        }
    }

    /// Keys of the filter's input line (doc/0006, doc/0011): typing filters
    /// live and the selection follows the first match; `Enter` keeps the
    /// filter and gives the keys back to the list; `Esc` clears it.
    /// `↑` / `↓` move the selection meanwhile; `Ctrl+C` quits.
    fn filter_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => return vec![Effect::Quit],
            KeyCode::Char('z' | 'Z') if ctrl => {
                self.set_message(
                    "Ctrl+Z does nothing here; inside a session it returns to tower",
                    Severity::Info,
                );
                return Vec::new();
            }
            KeyCode::Char('u' | 'U') if ctrl => {
                self.filter.text.clear();
                self.rebuild_view(Keep::Same);
            }
            _ if ctrl || alt => return Vec::new(),
            KeyCode::Enter => {
                self.filter.editing = false;
                if !self.filter_active() {
                    self.filter.text.clear();
                }
            }
            KeyCode::Esc => {
                self.filter = Filter::default();
                self.rebuild_view(Keep::Same);
                self.set_message("filter cleared", Severity::Info);
            }
            KeyCode::Backspace => {
                self.filter.text.pop();
                let keep = if self.filter_active() {
                    Keep::FirstRow
                } else {
                    Keep::Same
                };
                self.rebuild_view(keep);
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char(c) if !c.is_control() => {
                self.filter.text.push(c);
                let keep = if self.filter_active() {
                    Keep::FirstRow
                } else {
                    Keep::Same
                };
                self.rebuild_view(keep);
            }
            _ => return Vec::new(),
        }
        self.logs_follow(false)
    }

    fn popup_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.page_size.max(1);
        let Some(popup) = self.popup.as_mut() else {
            return Vec::new();
        };
        match key.code {
            // Ctrl+C follows the rules of the window beneath (doc/0011):
            // the dialog asks its own question, so the popup steps back
            // until the dialog is answered; over the list it quits (with
            // the hand-off question while one runs, see `handle_key`).
            KeyCode::Char('c' | 'C') if ctrl => {
                if self.dialog.is_some() {
                    if let Some(popup) = self.popup.take() {
                        self.waiting.push_front(Waiting::Popup(popup));
                    }
                    return self.dialog_key(key);
                }
                return vec![Effect::Quit];
            }
            _ if ctrl => {}
            KeyCode::Up | KeyCode::Char('k') => popup.scroll = popup.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => popup.scroll += 1,
            KeyCode::PageUp => popup.scroll = popup.scroll.saturating_sub(page),
            KeyCode::PageDown => popup.scroll += page,
            KeyCode::Home | KeyCode::Char('g') => popup.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => popup.scroll = usize::MAX / 2,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.popup = None,
            _ => {}
        }
        Vec::new()
    }

    fn dialog_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(dialog) = self.dialog.as_mut() else {
            return Vec::new();
        };
        match dialog.handle_key(key) {
            DialogAction::None => Vec::new(),
            DialogAction::Cancel => {
                self.dialog = None;
                self.set_message("new session cancelled", Severity::Info);
                Vec::new()
            }
            DialogAction::Quit => vec![Effect::Quit],
            DialogAction::EditTask => vec![Effect::EditTask(dialog.task.text())],
            DialogAction::Dispatch { attach } => self.start_dispatch(attach, false),
            DialogAction::Trust => {
                let attach = match dialog.status {
                    DialogStatus::Untrusted { attach, .. } => attach,
                    _ => false,
                };
                self.start_dispatch(attach, true)
            }
        }
    }

    /// Validates the dialog and starts a dispatch, or (`trust`) the trust
    /// flow that is followed by one. Validation errors stay in the dialog.
    fn start_dispatch(&mut self, attach: bool, trust: bool) -> Vec<Effect> {
        let root = self.root;
        let Some(dialog) = self.dialog.as_mut() else {
            return Vec::new();
        };
        let request = dialog.request(&self.config.accounts);
        if let Err(err) = request.validate_for(root) {
            dialog.field = Field::of_error(&err);
            dialog.error = Some(err.to_string());
            dialog.status = DialogStatus::Editing;
            return Vec::new();
        }
        dialog.error = None;
        dialog.status = DialogStatus::Starting { attach };
        let job = DispatchJob {
            request,
            attach,
            after_trust: false,
        };
        if trust {
            vec![Effect::Trust(job)]
        } else {
            vec![Effect::Dispatch(job)]
        }
    }

    fn help_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match list_action(&key) {
            Some(Action::Quit) => vec![Effect::Quit],
            _ => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                    self.overlay = Overlay::None;
                }
                Vec::new()
            }
        }
    }

    fn picker_key(&mut self, key: KeyEvent, mut picker: Picker) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.config.accounts.len();
        match key.code {
            KeyCode::Char('c') if ctrl => return vec![Effect::Quit],
            _ if ctrl => {}
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                self.set_message(
                    format!("{} cancelled", picker.purpose.verb()),
                    Severity::Info,
                );
            }
            KeyCode::Up | KeyCode::Char('k') => {
                picker.selected = picker.selected.saturating_sub(1);
                self.overlay = Overlay::Picker(picker);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1).min(n.saturating_sub(1));
                self.overlay = Overlay::Picker(picker);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                picker.selected = 0;
                self.overlay = Overlay::Picker(picker);
            }
            KeyCode::End | KeyCode::Char('G') => {
                picker.selected = n.saturating_sub(1);
                self.overlay = Overlay::Picker(picker);
            }
            KeyCode::Enter => return self.pick_account(picker),
            _ => {}
        }
        Vec::new()
    }

    /// Records the picked account (`asked`, doc/0003) and carries on with
    /// the action the picker was opened for. When a poll made the owner
    /// known while the picker was open, the known owner is used instead and
    /// nothing is recorded: the answer was given without that evidence. An
    /// attach records the answer only after its refresh (doc/0005).
    fn pick_account(&mut self, picker: Picker) -> Vec<Effect> {
        self.overlay = Overlay::None;
        let Some(picked) = self.config.accounts.get(picker.selected).cloned() else {
            return Vec::new();
        };
        let learned = self
            .sessions
            .iter()
            .find(|m| m.session.key() == Some(picker.key.as_str()))
            .and_then(|m| m.owner.account())
            .and_then(|a| self.config.account(a))
            .cloned();
        let mut effects = Vec::new();
        if picker.purpose == Purpose::Attach {
            // The answer is recorded only once the refresh still shows no
            // owner (doc/0005): another tower may have recorded one.
            let (account, asked) = match learned {
                Some(known) => {
                    self.set_message(
                        learned_text(&picker.name, &known.name, &picked.name),
                        Severity::Info,
                    );
                    (known, false)
                }
                None => (picked, true),
            };
            let request = AttachRequest {
                account,
                id: picker.id,
                name: picker.name,
            };
            effects.extend(self.begin_attach(picker.key, request, asked));
            effects.extend(self.logs_follow(false));
            return effects;
        }
        let account = match learned {
            Some(known) => {
                self.set_message(
                    learned_text(&picker.name, &known.name, &picked.name),
                    Severity::Info,
                );
                known
            }
            None => {
                if self.sidecar.record_answer(&picker.key, &picked.name) {
                    effects.push(Effect::SaveSidecar);
                }
                for m in &mut self.sessions {
                    if m.session.key() == Some(picker.key.as_str()) {
                        m.owner = Ownership::Known {
                            account: picked.name.clone(),
                            source: AccountSource::Asked,
                        };
                    }
                }
                picked
            }
        };
        let confirm_stop = self
            .sessions
            .iter()
            .find(|m| m.session.key() == Some(picker.key.as_str()))
            .map(|m| stop_needs_confirmation(&m.session));
        let action = SessionAction {
            account,
            id: picker.id,
            name: picker.name,
        };
        match picker.purpose {
            // Handled above, through the refresh.
            Purpose::Attach => {}
            Purpose::Stop => effects.extend(self.stop_or_confirm(action, confirm_stop)),
            Purpose::Remove => self.confirm_remove(action),
        }
        effects.extend(self.logs_follow(false));
        effects
    }

    fn confirm_key(&mut self, key: KeyEvent, confirm: Confirm) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl => vec![Effect::Quit],
            _ if ctrl || alt => Vec::new(),
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.overlay = Overlay::None;
                match confirm {
                    Confirm::Stop { action, .. } => self.run_stop(action),
                    Confirm::Remove { action } => self.run_remove(action),
                    Confirm::Handoff { job, .. } => self.start_handoff(*job),
                }
            }
            // The default is cancel (doc/0011), so Enter cancels too.
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Enter => {
                self.overlay = Overlay::None;
                let verb = match confirm {
                    Confirm::Stop { .. } => "stop",
                    Confirm::Remove { .. } => "remove",
                    Confirm::Handoff { .. } => "hand-off",
                };
                self.set_message(format!("{verb} cancelled"), Severity::Info);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(action) = list_action(&key) else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        match action {
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-(self.page_size.max(1) as isize)),
            Action::PageDown => self.move_by(self.page_size.max(1) as isize),
            Action::First => self.select(0),
            Action::Last => self.select(self.entries.len().saturating_sub(1)),
            Action::Attach if self.selected_header().is_some() => self.toggle_group(),
            Action::Attach => effects = self.request_action(Purpose::Attach),
            Action::NewSession => return vec![Effect::OpenDialog],
            Action::Stop => effects = self.request_action(Purpose::Stop),
            Action::Remove => effects = self.request_action(Purpose::Remove),
            Action::Handoff => effects = self.request_handoff(),
            Action::Orchestrator => effects = self.request_orchestrator(),
            Action::ResumePast => return self.open_resume(),
            Action::Settings => return self.request_settings(),
            Action::MarkSeen => effects = self.mark_seen(),
            Action::ToggleLogs => {
                if self.logs.take().is_none() {
                    self.logs = Some(LogsPanel {
                        title: String::new(),
                        view: LogsView::Loading,
                        target: None,
                        in_flight: None,
                        pending: false,
                    });
                    // Force the first fetch even if the target is unchanged.
                    return self.logs_follow(true);
                }
                return Vec::new();
            }
            Action::Close => {
                if let Some(p) = self.pending_attach.take() {
                    self.set_message(
                        format!("attach to {} cancelled", p.request.name),
                        Severity::Info,
                    );
                    return Vec::new();
                }
                if self.logs.take().is_some() {
                    return Vec::new();
                }
                if self.filter_active() {
                    self.filter = Filter::default();
                    self.rebuild_view(Keep::Same);
                    self.set_message("filter cleared", Severity::Info);
                }
            }
            Action::Refresh => {
                self.set_message("refreshing…", Severity::Info);
                return vec![Effect::PollNow];
            }
            Action::Help => self.overlay = Overlay::Help,
            Action::Quit => return vec![Effect::Quit],
            Action::DetachKeyHint => self.set_message(
                "Ctrl+Z does nothing here; inside a session it returns to tower",
                Severity::Info,
            ),
            Action::Filter => self.filter.editing = true,
            Action::ToggleGrouping => {
                self.grouping = match self.grouping {
                    Grouping::State => Grouping::Project,
                    Grouping::Project => Grouping::State,
                };
                self.rebuild_view(Keep::Same);
                let how = match self.grouping {
                    Grouping::State => "grouped by state",
                    Grouping::Project => "grouped by project",
                };
                self.set_message(format!("{how} (p toggles)"), Severity::Info);
            }
            Action::ToggleInteractive => {
                self.show_interactive = !self.show_interactive;
                self.rebuild_view(Keep::Same);
                // While hidden, the status bar says so on its own.
                if self.show_interactive {
                    self.set_message("interactive sessions shown", Severity::Info);
                } else {
                    self.message = None;
                }
            }
            Action::ToggleGroup => self.toggle_group(),
        }
        effects.extend(self.logs_follow(false));
        effects
    }

    /// A row action (attach, stop, remove) on the selected row: with the
    /// owning account, or after asking for it when it is unknown (doc/0003,
    /// doc/0005, doc/0009).
    fn request_action(&mut self, purpose: Purpose) -> Vec<Effect> {
        let Some(&Entry::Row(index)) = self.selected_entry().and_then(|e| self.entries.get(e))
        else {
            return Vec::new();
        };
        self.request_action_on(index, purpose)
    }

    /// [`App::request_action`] on the session at `index` in
    /// [`App::sessions`].
    fn request_action_on(&mut self, index: usize, purpose: Purpose) -> Vec<Effect> {
        let Some(m) = self.sessions.get(index) else {
            return Vec::new();
        };
        let name = m.session.display_name();
        let verb = purpose.verb();
        if m.session.is_interactive() {
            self.set_message(interactive_hint(&name), Severity::Info);
            return Vec::new();
        }
        let Some(id) = m.session.id.clone() else {
            self.set_message(
                format!("{name} has no short id; tower cannot {verb} it"),
                Severity::Info,
            );
            return Vec::new();
        };
        if self.handing_off(&m.session) {
            // Attaching now would wake it under the old owner's account;
            // stop and remove would race the hand-off (doc/0004).
            self.set_message(
                format!("{name} is being handed off to another account; wait until it finishes"),
                Severity::Info,
            );
            return Vec::new();
        }
        let confirm_stop = stop_needs_confirmation(&m.session);
        match &m.owner {
            Ownership::Known { account, .. } => {
                let Some(account) = self.config.account(account).cloned() else {
                    self.set_message(
                        format!("account `{account}` is not configured"),
                        Severity::Error,
                    );
                    return Vec::new();
                };
                let key = m.session.key().unwrap_or(&id).to_owned();
                let action = SessionAction { account, id, name };
                match purpose {
                    Purpose::Attach => self.begin_attach(
                        key,
                        AttachRequest {
                            account: action.account,
                            id: action.id,
                            name: action.name,
                        },
                        false,
                    ),
                    Purpose::Stop => self.stop_or_confirm(action, Some(confirm_stop)),
                    Purpose::Remove => {
                        self.confirm_remove(action);
                        Vec::new()
                    }
                }
            }
            Ownership::Unknown { config_dir } => {
                let key = m.session.key().unwrap_or(&id).to_owned();
                let selected = self
                    .config
                    .accounts
                    .iter()
                    .position(|a| a.name == self.config.default_account)
                    .unwrap_or(0);
                self.overlay = Overlay::Picker(Picker {
                    key,
                    id,
                    name,
                    config_dir: config_dir.clone(),
                    selected,
                    purpose,
                });
                Vec::new()
            }
        }
    }

    // ---- the orchestrator (doc/0010) --------------------------------------

    /// `o`: attaches to the background row named `orchestrator` exactly as
    /// `Enter` would (owning account, refresh first, account picker for an
    /// unknown owner, refused during its hand-off). With several such rows
    /// a live one wins, then the most recent, and the status line says so.
    /// Without one, the orchestrator is started under `default_account`
    /// and attached once it runs. Nothing happens before every account has
    /// been listed once, so a listing still loading never starts a second
    /// orchestrator.
    fn request_orchestrator(&mut self) -> Vec<Effect> {
        if self.orchestrator.starting {
            self.set_message(
                "the orchestrator is starting; tower attaches when it runs",
                Severity::Info,
            );
            return Vec::new();
        }
        if let Some(p) = self.polls.iter().find(|p| p.rows.is_none()) {
            let text = format!(
                "cannot look for the orchestrator yet: {} has not been listed",
                p.name
            );
            self.set_message(text, Severity::Info);
            return Vec::new();
        }
        if let Some((index, count)) = orchestrator::find(&self.sessions) {
            if count > 1 {
                let s = &self.sessions[index].session;
                let which = if s.has_live_process() {
                    "the most recent live one"
                } else {
                    "the most recent"
                };
                let text = format!(
                    "{count} sessions are named {}; using {which} ({})",
                    orchestrator::NAME,
                    s.id.as_deref().unwrap_or("no id")
                );
                self.set_message(text, Severity::Warning);
            }
            return self.request_action_on(index, Purpose::Attach);
        }
        if let Some(started) = &self.orchestrator.started {
            // Started by this tower and not listed yet: attach without a
            // refresh, as dispatch-and-attach does (doc/0005).
            if let Some(p) = &self.pending_attach {
                let text = format!("still checking {} before attaching", p.request.name);
                self.set_message(text, Severity::Info);
                return Vec::new();
            }
            return vec![self.attach_effect(AttachRequest {
                account: started.account.clone(),
                id: started.id.clone(),
                name: orchestrator::NAME.to_owned(),
            })];
        }
        let request = orchestrator::request(&self.config, self.orchestrator_dir.clone());
        self.orchestrator.starting = true;
        self.set_message(
            format!("starting the orchestrator under {}…", request.account.name),
            Severity::Info,
        );
        vec![Effect::StartOrchestrator(OrchestratorJob {
            request,
            after_trust: false,
        })]
    }

    /// Reports a finished orchestrator start (doc/0010). On success the
    /// owner is recorded (`started`, as for any dispatch) and tower
    /// attaches at once, without a refresh: the session is not listed yet.
    /// When a window is open meanwhile it does not attach and says to
    /// press `o`. `Workspace not trusted` opens the trust window.
    pub fn orchestrator_finished(
        &mut self,
        job: OrchestratorJob,
        result: Result<Dispatched, StartError>,
    ) -> Vec<Effect> {
        self.orchestrator.starting = false;
        let account = job.request.account.clone();
        match result {
            Ok(started) => {
                let mut effects = Vec::new();
                if self.sidecar.record_started(&started.id, &account.name) {
                    effects.push(Effect::SaveSidecar);
                }
                self.orchestrator.started = Some(JustStarted {
                    account: account.clone(),
                    id: started.id.clone(),
                    misses: 0,
                });
                let what = format!(
                    "started the orchestrator ({}) under {}",
                    started.id, account.name
                );
                if self.window_open() || self.pending_attach.is_some() {
                    self.set_message(format!("{what}; press o to attach"), Severity::Info);
                    effects.push(Effect::PollNow);
                } else {
                    self.set_message(what, Severity::Info);
                    // attach_finished polls once tower is back.
                    effects.push(self.attach_effect(AttachRequest {
                        account,
                        id: started.id,
                        name: orchestrator::NAME.to_owned(),
                    }));
                }
                effects
            }
            Err(StartError::Dispatch(DispatchError::Untrusted { message, .. })) => {
                let mut message = message;
                if job.after_trust {
                    message.push_str(&format!(
                        "\n(still not trusted after running {} there)",
                        account.command
                    ));
                }
                self.set_message(
                    format!(
                        "{} has not trusted the orchestrator's folder; Ctrl+T to trust it and \
                         retry",
                        account.name
                    ),
                    Severity::Warning,
                );
                self.open_orchestrator_untrusted(job, message);
                Vec::new()
            }
            Err(err) => {
                self.set_message(
                    format!(
                        "cannot start the orchestrator: {}",
                        one_line(&err.to_string())
                    ),
                    Severity::Error,
                );
                Vec::new()
            }
        }
    }

    /// Reports the end of the orchestrator's trust flow: the interactive
    /// account command exited, so the start is retried once. When it could
    /// not run at all the trust window shows why.
    pub fn orchestrator_trust_finished(
        &mut self,
        job: OrchestratorJob,
        result: Result<Option<i32>, String>,
    ) -> Vec<Effect> {
        match result {
            Ok(_) => {
                self.orchestrator.starting = true;
                self.set_message("starting the orchestrator again…", Severity::Info);
                vec![Effect::StartOrchestrator(OrchestratorJob {
                    after_trust: true,
                    ..job
                })]
            }
            Err(err) => {
                self.orchestrator.starting = false;
                self.open_orchestrator_untrusted(job, err);
                Vec::new()
            }
        }
    }

    /// The orchestrator's trust window, opened by a late result (doc/0011).
    fn open_orchestrator_untrusted(&mut self, job: OrchestratorJob, message: String) {
        let overlay =
            Overlay::OrchestratorUntrusted(Box::new(UntrustedOrchestrator { job, message }));
        self.open_late(Waiting::Overlay(Box::new(overlay)));
    }

    fn orchestrator_untrusted_key(
        &mut self,
        key: KeyEvent,
        untrusted: UntrustedOrchestrator,
    ) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => vec![Effect::Quit],
            KeyCode::Char('t' | 'T') if ctrl => {
                self.overlay = Overlay::None;
                self.orchestrator.starting = true;
                vec![Effect::OrchestratorTrust(untrusted.job)]
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') if !ctrl => {
                self.overlay = Overlay::None;
                self.set_message("the orchestrator was not started", Severity::Info);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// After a poll of `account`: forgets the just started orchestrator
    /// once a listing shows it, or after [`JUST_STARTED_POLLS`] good
    /// listings of its account without it.
    fn track_started_orchestrator(&mut self, account: &str, listed_ok: bool) {
        let Some(started) = self.orchestrator.started.as_mut() else {
            return;
        };
        let listed = self
            .sessions
            .iter()
            .any(|m| m.session.id.as_deref() == Some(started.id.as_str()));
        if !listed && listed_ok && started.account.name == account {
            started.misses += 1;
        }
        if listed || started.misses >= JUST_STARTED_POLLS {
            self.orchestrator.started = None;
        }
    }

    // ---- attach after a refresh (doc/0005) -------------------------------

    /// Starts the refresh an attach waits for: a poll of every account and
    /// a look at the sidecar file for another tower's changes. The attach
    /// itself follows from [`App::apply_poll`], [`App::sidecar_saved`] /
    /// [`App::sidecar_checked`] or [`App::tick`]. `asked`: the account is
    /// the picker's answer, recorded only if the owner is still unknown then.
    fn begin_attach(&mut self, key: String, request: AttachRequest, asked: bool) -> Vec<Effect> {
        if let Some(p) = &self.pending_attach {
            let text = format!("still checking {} before attaching", p.request.name);
            self.set_message(text, Severity::Info);
            return Vec::new();
        }
        self.pending_attach = Some(PendingAttach {
            key,
            request,
            asked,
            deadline: Instant::now() + ATTACH_CHECK_TIMEOUT,
            cycle: None,
            listed: Vec::new(),
            sidecar_from: self.sidecar_io.next_seq,
            sidecar_done: false,
        });
        vec![Effect::PollForAttach, Effect::CheckSidecar]
    }

    /// The runtime started the poll of [`Effect::PollForAttach`]: listings
    /// of this cycle or later count as fresh.
    pub fn attach_poll_started(&mut self, cycle: u64) {
        if let Some(p) = self.pending_attach.as_mut()
            && p.cycle.is_none()
        {
            p.cycle = Some(cycle);
        }
    }

    /// Whether an attach waits for its refresh.
    pub fn attach_pending(&self) -> bool {
        self.pending_attach.is_some()
    }

    /// Attaches once the refresh is complete, or anyway after
    /// [`ATTACH_CHECK_TIMEOUT`].
    fn attach_progress(&mut self, now: Instant) -> Vec<Effect> {
        let Some(p) = &self.pending_attach else {
            return Vec::new();
        };
        let ready = p.listed_by(&p.request.account.name).is_some() && p.sidecar_done;
        if !ready && now < p.deadline {
            return Vec::new();
        }
        let Some(p) = self.pending_attach.take() else {
            return Vec::new();
        };
        self.finish_attach(p, !ready)
    }

    /// The refresh is in (or `timed_out`): attach unless the session is no
    /// longer listed, with the owner the refresh shows. An owner another
    /// tower recorded meanwhile wins over the one known at `Enter`, and over
    /// the picker's answer, which is recorded only when the owner is still
    /// unknown. The fresh listing checked is that of the account whose
    /// command attaches.
    fn finish_attach(&mut self, p: PendingAttach, timed_out: bool) -> Vec<Effect> {
        let name = p.request.name.clone();
        let asked_for = p.request.account.name.clone();
        if self.window_open() {
            self.set_message(
                format!("attach to {name} cancelled: another window is open"),
                Severity::Info,
            );
            return Vec::new();
        }
        let Some(m) = self
            .sessions
            .iter()
            .find(|m| m.session.key() == Some(p.key.as_str()))
        else {
            self.set_message(format!("session no longer listed: {name}"), Severity::Info);
            return Vec::new();
        };
        if self.handing_off(&m.session) {
            self.set_message(
                format!("{name} is being handed off to another account; wait until it finishes"),
                Severity::Info,
            );
            return Vec::new();
        }
        let owner = m.owner.clone();
        let mut notes = Vec::new();
        // The account to attach with, and whether it is the picker's answer
        // to record.
        let (chosen, record) = match &owner {
            Ownership::Known { account, .. } if *account != asked_for => {
                match self.config.account(account).cloned() {
                    Some(a) => {
                        notes.push(learned_text(&name, account, &asked_for));
                        (a, false)
                    }
                    None => (p.request.account.clone(), false),
                }
            }
            Ownership::Known { .. } => (p.request.account.clone(), false),
            Ownership::Unknown { .. } => (p.request.account.clone(), p.asked),
        };
        match p.listed_by(&chosen.name) {
            Some(Ok(false)) => {
                self.set_message(
                    format!(
                        "session no longer listed: {} does not list {name} any more",
                        chosen.name
                    ),
                    Severity::Info,
                );
                return Vec::new();
            }
            Some(Err(err)) => notes.push(format!(
                "{} could not be listed ({}), so tower attached without a fresh list",
                chosen.name,
                one_line(err)
            )),
            Some(Ok(true)) if timed_out => notes.push(format!(
                "the state file could not be checked for other towers' changes within {}s",
                ATTACH_CHECK_TIMEOUT.as_secs()
            )),
            Some(Ok(true)) => {}
            None if timed_out => notes.push(format!(
                "the refresh did not finish within {}s, so tower attached without it",
                ATTACH_CHECK_TIMEOUT.as_secs()
            )),
            None => {}
        }
        if let Ownership::Unknown { config_dir } = owner
            && !p.asked
        {
            // The owner became unknown: ask, as for any such row.
            let selected = self
                .config
                .accounts
                .iter()
                .position(|a| a.name == self.config.default_account)
                .unwrap_or(0);
            self.overlay = Overlay::Picker(Picker {
                key: p.key,
                id: p.request.id,
                name,
                config_dir,
                selected,
                purpose: Purpose::Attach,
            });
            return Vec::new();
        }
        let mut effects = Vec::new();
        if record {
            if self.sidecar.record_answer(&p.key, &chosen.name) {
                effects.push(Effect::SaveSidecar);
            }
            for m in &mut self.sessions {
                if m.session.key() == Some(p.key.as_str()) {
                    m.owner = Ownership::Known {
                        account: chosen.name.clone(),
                        source: AccountSource::Asked,
                    };
                }
            }
        }
        self.attach_note = (!notes.is_empty()).then(|| notes.join("; "));
        effects.push(self.attach_effect(AttachRequest {
            account: chosen,
            id: p.request.id,
            name,
        }));
        effects
    }

    /// Time passes: opens waiting windows once nothing else is open, and
    /// attaches when an attach's refresh has timed out. The runtime calls it
    /// on every turn of its loop.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        self.show_waiting();
        self.attach_progress(now)
    }

    // ---- windows opened by background results (doc/0011) -----------------

    /// Whether any window is open above the list: a popup, the dialog, an
    /// overlay or the quit question.
    fn window_open(&self) -> bool {
        self.popup.is_some()
            || self.dialog.is_some()
            || self.resume.is_some()
            || self.settings.is_some()
            || self.overlay != Overlay::None
            || self.quit_prompt.is_some()
    }

    /// Number of windows waiting for the open one to close.
    pub fn waiting_windows(&self) -> usize {
        self.waiting.len()
    }

    /// Opens `window` now, or queues it while another window is open, so a
    /// late result never replaces the window the user is answering.
    fn open_late(&mut self, window: Waiting) {
        if self.window_open() || !self.waiting.is_empty() {
            self.waiting.push_back(window);
            self.show_waiting();
        } else {
            self.open_window(window);
        }
    }

    /// The hand-off trust window (doc/0004), opened by a late result.
    fn open_untrusted(&mut self, job: HandoffJob, message: String) {
        let overlay = Overlay::Untrusted(Box::new(UntrustedHandoff { job, message }));
        self.open_late(Waiting::Overlay(Box::new(overlay)));
    }

    fn open_window(&mut self, window: Waiting) {
        match window {
            Waiting::Overlay(overlay) => self.overlay = *overlay,
            Waiting::Popup(popup) => self.popup = Some(popup),
        }
    }

    /// Opens the next waiting window when nothing else is open.
    fn show_waiting(&mut self) {
        if !self.window_open()
            && let Some(window) = self.waiting.pop_front()
        {
            self.open_window(window);
        }
    }

    // ---- sidecar file jobs (doc/0002, doc/0017 section 8) ----------------

    /// The save to run for [`Effect::SaveSidecar`], or `None` when another
    /// sidecar job runs: the save is then queued (at most one) and asked
    /// for again when that job reports back.
    pub fn start_save(&mut self) -> Option<SidecarJob> {
        if self.sidecar_io.busy.is_some() {
            self.sidecar_io.save_queued = true;
            return None;
        }
        self.sidecar_io.save_queued = false;
        let seq = self.sidecar_io.take_seq();
        Some(SidecarJob::Save {
            seq,
            job: self.sidecar.begin_save(),
        })
    }

    /// The check to run for [`Effect::CheckSidecar`], or `None` when
    /// another sidecar job runs (then queued, at most one).
    pub fn start_sidecar_check(&mut self) -> Option<SidecarJob> {
        if self.sidecar_io.busy.is_some() {
            self.sidecar_io.check_queued = true;
            return None;
        }
        self.sidecar_io.check_queued = false;
        let seq = self.sidecar_io.take_seq();
        Some(SidecarJob::Check {
            seq,
            job: self.sidecar.begin_check(),
        })
    }

    /// Whether a sidecar job runs.
    pub fn sidecar_busy(&self) -> bool {
        self.sidecar_io.busy.is_some()
    }

    /// Takes back a finished save (see [`Sidecar::finish_save`]): changes
    /// made meanwhile stay on top, another tower's changes merged by the
    /// save show at once. A failure is reported and retried later.
    pub fn sidecar_saved(
        &mut self,
        seq: u64,
        job: &SaveJob,
        result: std::io::Result<Saved>,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        match self.sidecar.finish_save(job, result) {
            Ok(true) => effects.extend(self.after_reload()),
            Ok(false) => {}
            Err(err) => self.set_message(
                format!("cannot save the state file: {}", one_line(&err.to_string())),
                Severity::Error,
            ),
        }
        effects.extend(self.sidecar_job_done(seq));
        effects
    }

    /// Takes back a finished check: another tower's changes are merged
    /// under this instance's unsaved ones (see [`Sidecar::apply_reload`]).
    pub fn sidecar_checked(&mut self, seq: u64, result: std::io::Result<Reloaded>) -> Vec<Effect> {
        let mut effects = Vec::new();
        match result {
            Ok(reloaded) => {
                self.check_error = None;
                if self.sidecar.apply_reload(reloaded) {
                    effects.extend(self.after_reload());
                }
            }
            Err(err) => {
                self.check_error = Some(format!(
                    "cannot read the state file for other towers' changes: {}",
                    one_line(&err.to_string())
                ));
            }
        }
        effects.extend(self.sidecar_job_done(seq));
        effects
    }

    /// Owners may have changed: merge again with the new sidecar.
    fn after_reload(&mut self) -> Vec<Effect> {
        if self.polls.iter().all(|p| p.rows.is_none()) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if self.rebuild(None).0 {
            effects.push(Effect::SaveSidecar);
        }
        effects.extend(self.logs_follow(false));
        effects
    }

    /// A sidecar job reported back: the next queued one, and an attach
    /// waiting for it.
    fn sidecar_job_done(&mut self, seq: u64) -> Vec<Effect> {
        if self.sidecar_io.busy == Some(seq) {
            self.sidecar_io.busy = None;
        }
        let mut effects = Vec::new();
        if self.sidecar_io.save_queued {
            effects.push(Effect::SaveSidecar);
        } else if self.sidecar_io.check_queued {
            effects.push(Effect::CheckSidecar);
        }
        if let Some(p) = self.pending_attach.as_mut()
            && seq >= p.sidecar_from
        {
            p.sidecar_done = true;
        }
        effects.extend(self.attach_progress(Instant::now()));
        effects
    }

    // ---- config.toml: reload, save, editor (doc/0022) ----------------------

    /// Applies a new config to the running tower (doc/0022): the style and
    /// theme redraw at once; the dialog defaults, project roots and extras
    /// (rescanned when the dialog next opens), the hand-off prompt and
    /// guard apply from their next use; a changed poll interval gives
    /// [`Effect::SetPollInterval`]. Accounts are not applied live: while
    /// the new config's accounts differ from tower's, tower keeps its own
    /// (and its `default_account` when the new one names none of them) and
    /// the status line shows [`RESTART_NOTE`].
    pub fn apply_config(&mut self, new: Config) -> Vec<Effect> {
        let mut live = new;
        let accounts_changed = live.accounts != self.config.accounts;
        if accounts_changed {
            live.accounts = self.config.accounts.clone();
            if live.account(&live.default_account).is_none() {
                live.default_account = self.config.default_account.clone();
            }
        }
        self.config_io.restart_note = accounts_changed.then_some(RESTART_NOTE);
        let mut effects = Vec::new();
        if live.poll_interval_secs != self.config.poll_interval_secs {
            effects.push(Effect::SetPollInterval(live.poll_interval()));
        }
        self.style = live.ui_style;
        self.set_theme(live.ui_theme);
        self.config = live;
        // An open Settings window keeps its preview over the new look
        // (doc/0024).
        if let Some(window) = self.settings.as_mut() {
            window.base = (self.style, self.theme);
        }
        self.sync_settings_preview();
        effects
    }

    /// The note shown while the config file's accounts differ from the
    /// running ones ([`RESTART_NOTE`]).
    pub fn restart_note(&self) -> Option<&'static str> {
        self.config_io.restart_note
    }

    /// Why the last reload of `config.toml` was refused, until one works.
    pub fn config_error(&self) -> Option<&str> {
        self.config_io.error.as_deref()
    }

    /// The check to run for [`Effect::CheckConfig`]; `None` while one runs
    /// or when the runtime has not said where the config is.
    pub fn start_config_check(&mut self) -> Option<ConfigCheckJob> {
        let path = self.config_source.as_ref()?.path.clone();
        if self.config_io.busy {
            return None;
        }
        self.config_io.busy = true;
        Some(ConfigCheckJob {
            epoch: self.config_io.epoch,
            check: ConfigCheck {
                path,
                home: self.home.clone(),
                known: self.config_io.stamp,
            },
        })
    }

    /// Takes back a finished check: a changed file is applied
    /// ([`App::config_reloaded`]). A check older than the last save or
    /// editor run is dropped.
    pub fn config_checked(&mut self, epoch: u64, found: Option<ConfigSnapshot>) -> Vec<Effect> {
        self.config_io.busy = false;
        match found {
            Some(snapshot) if epoch == self.config_io.epoch => self.config_reloaded(snapshot),
            _ => Vec::new(),
        }
    }

    /// Takes in the config file as read: a valid one is applied
    /// ([`App::apply_config`]) and said in the status line when anything
    /// changed; an invalid one keeps the current config and shows the
    /// error until the file is fixed.
    pub fn config_reloaded(&mut self, snapshot: ConfigSnapshot) -> Vec<Effect> {
        let (effects, changed) = self.take_in_config(snapshot);
        if changed {
            let file = self.config_file_shown();
            match self.config_io.restart_note {
                Some(note) => {
                    self.set_message(format!("reloaded {file}; {note}"), Severity::Warning)
                }
                None => self.set_message(
                    format!("reloaded {file}; the new settings apply now"),
                    Severity::Info,
                ),
            }
        }
        effects
    }

    /// Records the version read and applies it. Returns the effects and
    /// whether the running config or the restart note changed. A refused
    /// reload sets the error and its message.
    fn take_in_config(&mut self, snapshot: ConfigSnapshot) -> (Vec<Effect>, bool) {
        self.config_io.stamp = snapshot.stamp;
        match snapshot.result {
            Ok(config) => {
                self.config_io.error = None;
                self.set_config_found(snapshot.found);
                let before = (self.config.clone(), self.config_io.restart_note);
                let effects = self.apply_config(config);
                let changed = before != (self.config.clone(), self.config_io.restart_note);
                (effects, changed)
            }
            Err(err) => {
                let text = format!(
                    "config not reloaded: {}; keeping the current settings",
                    one_line(&err.to_string())
                );
                self.config_io.error = Some(text.clone());
                self.set_message(text, Severity::Error);
                (Vec::new(), false)
            }
        }
    }

    /// The config file as shown (`~/…`), or `config.toml`.
    fn config_file_shown(&self) -> String {
        self.config_source
            .as_ref()
            .map_or_else(|| "config.toml".to_owned(), |s| s.shown(Some(&self.home)))
    }

    /// Takes back a finished save of the Settings window
    /// ([`Effect::SaveConfig`]): a written file is applied at once like a
    /// reload ([`App::apply_config`]) and becomes the known version, so the
    /// reload check does not read it again; the status line says what was
    /// written. A conflict sets no message (the window asks). The result
    /// is kept for the window: [`App::take_config_save_result`].
    pub fn config_saved(&mut self, result: Result<SaveOutcome, SaveError>) -> Vec<Effect> {
        let mut effects = Vec::new();
        match &result {
            Ok(outcome) if !outcome.written.is_empty() => {
                self.config_io.epoch += 1;
                self.config_io.stamp = outcome.stamp;
                self.config_io.error = None;
                self.set_config_found(true);
                effects = self.apply_config(outcome.config.clone());
                let file = paths::shorten_home(&outcome.path.to_string_lossy(), &self.home);
                let mut text = if outcome.created {
                    format!("created {file}")
                } else {
                    format!("saved {file}; comments and formatting kept")
                };
                if let Some(bak) = &outcome.backup {
                    let bak = paths::shorten_home(&bak.to_string_lossy(), &self.home);
                    text.push_str(&format!("; the previous file is in {bak} (once)"));
                }
                match self.config_io.restart_note {
                    Some(note) => {
                        text.push_str(&format!("; {note}"));
                        self.set_message(text, Severity::Warning);
                    }
                    None => {
                        text.push_str("; applied now");
                        self.set_message(text, Severity::Info);
                    }
                }
            }
            Ok(_) => self.set_message("nothing to save", Severity::Info),
            Err(SaveError::Conflict(_)) => {}
            Err(err) => self.set_message(
                format!("not saved: {}", one_line(&err.to_string())),
                Severity::Error,
            ),
        }
        self.config_io.saved = Some(result);
        // The open Settings window takes the result: its confirmation
        // line, a conflict question, or what follows the save (doc/0024).
        if self.settings.is_some()
            && let Some(result) = self.config_io.saved.take()
            && let Some(window) = self.settings.as_mut()
        {
            let action = window.save_finished(result);
            effects.extend(self.settings_action(action));
            self.sync_settings_preview();
        }
        effects
    }

    /// The last finished config save, once ([`App::config_saved`]); the
    /// Settings window takes it to update its session
    /// ([`config_edit::EditSession::finish_save`]) or to ask about a
    /// conflict.
    pub fn take_config_save_result(&mut self) -> Option<Result<SaveOutcome, SaveError>> {
        self.config_io.saved.take()
    }

    /// [`Effect::EditConfig`] for the config file, or a message when the
    /// runtime has not said where it is.
    pub fn edit_config(&mut self) -> Vec<Effect> {
        match &self.config_source {
            Some(source) => vec![Effect::EditConfig(source.path.clone())],
            None => {
                self.set_message("no config file is known", Severity::Error);
                Vec::new()
            }
        }
    }

    /// The editor of [`Effect::EditConfig`] exited: the file read back is
    /// applied like a reload, or the error (an invalid file keeps the
    /// current config) or the editor's failure is shown.
    pub fn config_editor_finished(
        &mut self,
        result: Result<ConfigSnapshot, String>,
    ) -> Vec<Effect> {
        self.config_io.epoch += 1;
        let reopen = std::mem::take(&mut self.reopen_settings);
        match result {
            Ok(snapshot) => {
                let ok = snapshot.result.is_ok();
                let (effects, changed) = self.take_in_config(snapshot);
                if ok {
                    let file = self.config_file_shown();
                    let text = match (changed, self.config_io.restart_note) {
                        (_, Some(note)) => format!("reloaded {file}; {note}"),
                        (true, None) => format!("reloaded {file}; the new settings apply now"),
                        (false, None) => format!("{file}: no setting changed"),
                    };
                    let severity = if self.config_io.restart_note.is_some() {
                        Severity::Warning
                    } else {
                        Severity::Info
                    };
                    self.set_message(text, severity);
                }
                let mut effects = effects;
                // Back to the Settings window the editor was opened from,
                // when the file is valid.
                if ok
                    && reopen
                    && let Some(source) = &self.config_source
                {
                    effects.push(Effect::OpenSettings(source.path.clone()));
                }
                effects
            }
            Err(err) => {
                self.set_message(err, Severity::Error);
                Vec::new()
            }
        }
    }

    // ---- the Settings window (doc/0024) -----------------------------------

    /// `c`: [`Effect::OpenSettings`] for the config file, or a message
    /// when the runtime has not said where it is.
    fn request_settings(&mut self) -> Vec<Effect> {
        match &self.config_source {
            Some(source) => vec![Effect::OpenSettings(source.path.clone())],
            None => {
                self.set_message("no config file is known", Severity::Error);
                Vec::new()
            }
        }
    }

    /// Opens the Settings window over the file as read
    /// ([`Effect::OpenSettings`]). A file that does not parse is not
    /// opened: the status line says why. Nothing opens over another
    /// window.
    pub fn settings_opened(&mut self, result: Result<EditSession, ConfigError>) -> Vec<Effect> {
        if self.window_open() {
            return Vec::new();
        }
        match result {
            Ok(session) => {
                let shown = paths::shorten_home(&session.path().to_string_lossy(), &self.home);
                self.settings = Some(SettingsWindow::new(
                    session,
                    shown,
                    (self.style, self.theme),
                    &self.home,
                ));
            }
            Err(err) => self.set_message(
                format!(
                    "settings not opened: {}; fix the file in an editor",
                    one_line(&err.to_string())
                ),
                Severity::Error,
            ),
        }
        Vec::new()
    }

    /// The Settings window, when open.
    pub fn settings(&self) -> Option<&SettingsWindow> {
        self.settings.as_ref()
    }

    /// A key for the Settings window.
    fn settings_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(window) = self.settings.as_mut() else {
            return Vec::new();
        };
        let action = window.handle_key(key);
        let mut effects = self.settings_action(action);
        self.sync_settings_preview();
        effects.extend(self.folder_job());
        effects
    }

    /// Carries out what the Settings window asked for.
    fn settings_action(&mut self, action: SettingsAction) -> Vec<Effect> {
        match action {
            SettingsAction::None => Vec::new(),
            SettingsAction::Close => {
                self.close_settings();
                Vec::new()
            }
            SettingsAction::Save { force } => match &self.settings {
                Some(window) => vec![Effect::SaveConfig(window.session.save_job(force))],
                None => Vec::new(),
            },
            SettingsAction::Edit => {
                self.close_settings();
                let effects = self.edit_config();
                self.reopen_settings = !effects.is_empty();
                effects
            }
            SettingsAction::Quit => vec![Effect::Quit],
        }
    }

    /// Closes the Settings window; a preview reverts to the running style
    /// and theme.
    fn close_settings(&mut self) {
        if let Some(window) = self.settings.take() {
            self.style = window.base.0;
            self.set_theme(window.base.1);
        }
    }

    /// Draws with the Settings window's preview of the style and theme,
    /// over the running ones (doc/0024).
    fn sync_settings_preview(&mut self) {
        let Some(window) = &self.settings else {
            return;
        };
        let (style, theme) = window.preview();
        self.style = style.unwrap_or(window.base.0);
        self.set_theme(theme.unwrap_or(window.base.1));
    }

    /// The list editor's next folder job, if any ([`Effect::CheckFolders`]).
    fn folder_job(&mut self) -> Vec<Effect> {
        let home = self.home.clone();
        self.settings
            .as_mut()
            .and_then(|w| w.take_folder_job(&home))
            .map(Effect::CheckFolders)
            .into_iter()
            .collect()
    }

    /// Takes back a finished [`Effect::CheckFolders`]; the next one starts
    /// when more is wanted.
    pub fn folders_checked(&mut self, result: FolderResult) -> Vec<Effect> {
        match self.settings.as_mut() {
            Some(window) => {
                window.folders_checked(result);
                self.folder_job()
            }
            None => Vec::new(),
        }
    }

    // ---- the project scan (doc/0008) --------------------------------------

    /// Opens the new-session dialog with an empty project list ("scanning
    /// projects…") and returns the scan to run on a worker thread.
    pub fn begin_project_scan(&mut self) -> ProjectScan {
        self.open_dialog(ProjectList::default());
        if let Some(d) = self.dialog.as_mut() {
            d.scanning = true;
        }
        self.next_scan()
    }

    /// A new project scan; only the latest one's result is used.
    fn next_scan(&mut self) -> ProjectScan {
        self.scan += 1;
        ProjectScan {
            id: self.scan,
            roots: self.config.project_roots.clone(),
            extras: self.config.extra_projects.clone(),
            recent: self.sidecar.recent_projects.clone(),
            home: self.home.clone(),
        }
    }

    /// Fills the list of the dialog or the resume window with the result
    /// of scan `id`; a result for a window that was closed meanwhile is
    /// dropped.
    pub fn projects_found(&mut self, id: u64, list: ProjectList) {
        if id != self.scan {
            return;
        }
        if let Some(d) = self.dialog.as_mut()
            && d.scanning
        {
            d.set_projects(list);
        } else if let Some(r) = self.resume.as_mut()
            && r.scanning
        {
            r.set_projects(list);
        }
    }

    // ---- resuming a past conversation (doc/0019) --------------------------

    /// `R`: opens the resume window. The account is the selected row's
    /// owner when it is known, else `default_account`; the folder is the
    /// selected row's `cwd` (or the project containing it) once the project
    /// scan arrives, else the most recently used project.
    fn open_resume(&mut self) -> Vec<Effect> {
        let (account, preferred) = match self.selected_session() {
            Some(m) => (
                m.owner
                    .account()
                    .map_or_else(|| self.config.default_account.clone(), str::to_owned),
                (!m.session.cwd.is_empty())
                    .then(|| paths::expand_tilde(&m.session.cwd, &self.home)),
            ),
            None => (self.config.default_account.clone(), None),
        };
        self.resume = Some(ResumeWindow::new(&self.config, &account, preferred));
        vec![Effect::ScanProjects(self.next_scan())]
    }

    fn resume_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(window) = self.resume.as_mut() else {
            return Vec::new();
        };
        match window.handle_key(key) {
            ResumeAction::None => Vec::new(),
            ResumeAction::Quit => vec![Effect::Quit],
            ResumeAction::Cancel => {
                self.resume = None;
                Vec::new()
            }
            ResumeAction::Confirm => {
                let account = window.selected_account(&self.config.accounts).cloned();
                let folder = window.selected_project().map(|p| p.path.clone());
                let (Some(account), Some(folder)) = (account, folder) else {
                    return Vec::new();
                };
                self.resume = None;
                vec![Effect::ResumePast(ResumeRequest { account, folder })]
            }
        }
    }

    /// Reports how the picker of past conversations ended and polls at
    /// once: a conversation the user moved to the background shows up as a
    /// normal background row, its owner detected through `/proc` as usual
    /// (doc/0019).
    pub fn resume_finished(&mut self, outcome: ResumeOutcome) -> Vec<Effect> {
        let who = format!(
            "claude --resume in {} ({})",
            outcome.folder, outcome.account
        );
        match outcome.result {
            Ok(Some(0)) => self.set_message(
                format!(
                    "back from {who}; a conversation moved to the background appears in the list"
                ),
                Severity::Info,
            ),
            Ok(Some(code)) => {
                self.set_message(format!("{who} exited with code {code}"), Severity::Error)
            }
            Ok(None) => self.set_message(format!("{who} was ended by a signal"), Severity::Error),
            Err(err) => self.set_message(
                format!("cannot run {who}: {}", one_line(&err)),
                Severity::Error,
            ),
        }
        vec![Effect::PollNow]
    }

    /// Working and blocked sessions ask first; others stop directly
    /// (doc/0009). `confirm` is `None` when the row is no longer listed.
    fn stop_or_confirm(&mut self, action: SessionAction, confirm: Option<bool>) -> Vec<Effect> {
        if confirm.unwrap_or(true) {
            let detail = self
                .sessions
                .iter()
                .find(|m| m.session.id.as_deref() == Some(action.id.as_str()))
                .map(|m| groups::detail(&m.session))
                .unwrap_or_else(|| "active".to_owned());
            self.overlay = Overlay::Confirm(Confirm::Stop { action, detail });
            return Vec::new();
        }
        self.run_stop(action)
    }

    fn confirm_remove(&mut self, action: SessionAction) {
        self.overlay = Overlay::Confirm(Confirm::Remove { action });
    }

    fn run_stop(&mut self, action: SessionAction) -> Vec<Effect> {
        self.set_message(format!("stopping {}…", action.name), Severity::Info);
        vec![Effect::Stop(action)]
    }

    fn run_remove(&mut self, action: SessionAction) -> Vec<Effect> {
        self.set_message(format!("removing {}…", action.name), Severity::Info);
        vec![Effect::Remove(action)]
    }

    /// The text of a running hand-off, if any (doc/0004).
    pub fn handoff_progress(&self) -> Option<&str> {
        self.handoff.as_ref().map(|h| h.text.as_str())
    }

    /// Whether `s` is the session of the running hand-off, by `sessionId`
    /// or short id. Its row actions are refused until the hand-off reports
    /// back (doc/0004).
    fn handing_off(&self, s: &Session) -> bool {
        self.handoff.as_ref().is_some_and(|h| {
            s.session_id.as_deref() == Some(h.session_id.as_str())
                || s.id.as_deref() == Some(h.id.as_str())
        })
    }

    /// `a` on the selected row (doc/0004): background rows with a short id
    /// and a `sessionId` only. With one candidate account the hand-off is
    /// planned right away, with more the target picker opens.
    fn request_handoff(&mut self) -> Vec<Effect> {
        if let Some(running) = &self.handoff {
            let text = format!(
                "a hand-off is already running ({}); wait until it finishes",
                running.name
            );
            self.set_message(text, Severity::Info);
            return Vec::new();
        }
        let Some(m) = self.selected_session() else {
            return Vec::new();
        };
        let name = m.session.display_name();
        if m.session.is_interactive() {
            self.set_message(interactive_hint(&name), Severity::Info);
            return Vec::new();
        }
        if m.session.id.is_none() {
            self.set_message(
                format!("{name} has no short id; tower cannot hand it off"),
                Severity::Info,
            );
            return Vec::new();
        }
        let Some(key) = m.session.session_id.clone() else {
            self.set_message(
                format!("{name} has no sessionId; it cannot be resumed under another account"),
                Severity::Info,
            );
            return Vec::new();
        };
        if self.config.accounts.len() < 2 {
            self.set_message(
                "only one account is configured; a hand-off needs a second account in config.toml",
                Severity::Info,
            );
            return Vec::new();
        }
        let owner = m.owner.account().map(str::to_owned);
        let owner_account = owner.as_deref().and_then(|a| self.config.account(a));
        let targets = match handoff::targets(&self.config, owner_account, &name) {
            Ok(targets) => targets,
            Err(refusal) => {
                self.set_message(refusal, Severity::Warning);
                return Vec::new();
            }
        };
        match targets.as_slice() {
            [] => Vec::new(),
            [only] => {
                let only = only.clone();
                self.plan_handoff(&key, &only)
            }
            _ => {
                self.overlay = Overlay::Target(TargetPicker {
                    key,
                    name,
                    owner,
                    targets,
                    selected: 0,
                });
                Vec::new()
            }
        }
    }

    /// The hand-off of the session with `sessionId` `key` to `target`, from
    /// the current row: checked against the account groups again, then
    /// always confirmed first (doc/0004).
    fn plan_handoff(&mut self, key: &str, target: &str) -> Vec<Effect> {
        let Some(m) = self
            .sessions
            .iter()
            .find(|m| m.session.session_id.as_deref() == Some(key))
        else {
            self.set_message("session no longer listed", Severity::Info);
            return Vec::new();
        };
        let Some(target) = self.config.account(target).cloned() else {
            self.set_message(
                format!("account `{target}` is not configured"),
                Severity::Error,
            );
            return Vec::new();
        };
        let s = &m.session;
        let owner = m
            .owner
            .account()
            .and_then(|a| self.config.account(a))
            .cloned();
        if let Err(refusal) =
            handoff::check_target(&self.config, owner.as_ref(), &target, &s.display_name())
        {
            self.set_message(refusal, Severity::Warning);
            return Vec::new();
        }
        let request = HandoffRequest {
            id: s.id.clone().unwrap_or_default(),
            session_id: key.to_owned(),
            name: s.display_name(),
            cwd: paths::expand_tilde(&s.cwd, &self.home),
            owner,
            target,
            live: s.has_live_process(),
            prompt: self.config.handoff_prompt.clone(),
        };
        let interrupts = request.live && stop_needs_confirmation(s);
        let detail = groups::detail(s);
        let job = HandoffJob {
            request,
            after_trust: false,
            recorded: self.sidecar.base_owner(key),
        };
        self.overlay = Overlay::Confirm(Confirm::Handoff {
            job: Box::new(job),
            detail,
            interrupts,
        });
        Vec::new()
    }

    fn start_handoff(&mut self, job: HandoffJob) -> Vec<Effect> {
        let r = &job.request;
        self.handoff = Some(HandoffProgress {
            name: r.name.clone(),
            id: r.id.clone(),
            session_id: r.session_id.clone(),
            text: format!("hand-off: {} — {}…", r.name, r.plan()),
        });
        vec![Effect::Handoff(job)]
    }

    fn target_key(&mut self, key: KeyEvent, mut picker: TargetPicker) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = picker.targets.len();
        match key.code {
            KeyCode::Char('c') if ctrl => return vec![Effect::Quit],
            _ if ctrl => {}
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Overlay::None;
                self.set_message("hand-off cancelled", Severity::Info);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                picker.selected = picker.selected.saturating_sub(1);
                self.overlay = Overlay::Target(picker);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1).min(n.saturating_sub(1));
                self.overlay = Overlay::Target(picker);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                picker.selected = 0;
                self.overlay = Overlay::Target(picker);
            }
            KeyCode::End | KeyCode::Char('G') => {
                picker.selected = n.saturating_sub(1);
                self.overlay = Overlay::Target(picker);
            }
            KeyCode::Enter => {
                self.overlay = Overlay::None;
                let Some(target) = picker.targets.get(picker.selected).cloned() else {
                    return Vec::new();
                };
                return self.plan_handoff(&picker.key, &target);
            }
            _ => {}
        }
        Vec::new()
    }

    fn untrusted_key(&mut self, key: KeyEvent, untrusted: UntrustedHandoff) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => vec![Effect::Quit],
            KeyCode::Char('t' | 'T') if ctrl => {
                self.overlay = Overlay::None;
                vec![Effect::HandoffTrust(untrusted.job)]
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') if !ctrl => {
                self.overlay = Overlay::None;
                self.set_message(
                    format!(
                        "hand-off cancelled; {} stays stopped",
                        untrusted.job.request.name
                    ),
                    Severity::Info,
                );
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A step of the running hand-off has started (doc/0004).
    pub fn handoff_step(&mut self, step: Step) {
        let Some(h) = self.handoff.as_mut() else {
            return;
        };
        h.text = match step {
            Step::Stopping { account } => {
                format!("hand-off: stopping {} under {account}…", h.name)
            }
            Step::WaitingForStop => format!("hand-off: waiting for {} to stop…", h.name),
            Step::Resuming { account } => {
                format!("hand-off: continuing {} under {account}…", h.name)
            }
        };
    }

    /// Reports a finished hand-off (doc/0004). On success the target is
    /// recorded in the sidecar (`account_source: handoff`): for the session
    /// itself when it continued, for the copy (by its short id until the
    /// next poll shows its `sessionId`) when Claude Code started one; the
    /// original's entry is then left as it was. `Workspace not trusted`
    /// offers the trust flow; every outcome polls.
    pub fn handoff_finished(
        &mut self,
        job: HandoffJob,
        result: Result<Resumed, HandoffError>,
    ) -> Vec<Effect> {
        self.handoff = None;
        // Nothing runs any more, so the quit question no longer applies.
        self.quit_prompt = None;
        let r = &job.request;
        let target = r.target.name.clone();
        let mut effects = Vec::new();
        match result {
            Ok(resumed) => {
                if handoff::record(&mut self.sidecar, r, &resumed) {
                    effects.push(Effect::SaveSidecar);
                }
                match &resumed.outcome {
                    Outcome::Continued => {
                        for m in &mut self.sessions {
                            if m.session.session_id.as_deref() == Some(r.session_id.as_str()) {
                                m.owner = Ownership::Known {
                                    account: target.clone(),
                                    source: AccountSource::Handoff,
                                };
                            }
                        }
                        if resumed.idle {
                            self.set_message(
                                format!(
                                    "continued {} under {target}, but it woke idle; attach and \
                                     send a prompt",
                                    r.name
                                ),
                                Severity::Warning,
                            );
                        } else {
                            self.set_message(
                                format!("continued {} under {target}", r.name),
                                Severity::Info,
                            );
                        }
                    }
                    Outcome::Copied { new_id } => {
                        let note = resumed
                            .notes
                            .first()
                            .map(|n| format!(" ({})", one_line(n)))
                            .unwrap_or_default();
                        self.set_message(
                            format!(
                                "--resume started a copy {new_id} of {} under {target} instead \
                                 of continuing {}; the original was left as it was{note}",
                                r.name, r.id
                            ),
                            Severity::Warning,
                        );
                    }
                }
            }
            Err(HandoffError::Untrusted { message, .. }) => {
                let mut message = message;
                if job.after_trust {
                    message.push_str(&format!(
                        "\n(still not trusted after running {} there)",
                        r.target.command
                    ));
                }
                self.set_message(
                    format!(
                        "{target} has not trusted {}'s folder; Ctrl+T to trust it and retry",
                        r.name
                    ),
                    Severity::Warning,
                );
                self.open_untrusted(job.clone(), message);
            }
            Err(HandoffError::OwnerChanged { account, stopped }) => {
                let what = if stopped {
                    "it was stopped and not continued"
                } else {
                    "nothing was run"
                };
                self.set_message(
                    format!(
                        "another tower recorded {} under {account} meanwhile; {what} — check \
                         the row and retry",
                        r.name
                    ),
                    Severity::Warning,
                );
                // Take in the other tower's record now.
                effects.push(Effect::CheckSidecar);
            }
            Err(err) => {
                let text = match &err {
                    HandoffError::ResumeCli(_)
                    | HandoffError::Failed { .. }
                    | HandoffError::Unparsed { .. } => format!(
                        "cannot continue {} under {target}: {}; it was not continued",
                        r.name,
                        one_line(&err.to_string())
                    ),
                    _ => format!(
                        "hand-off of {} to {target} failed: {}",
                        r.name,
                        one_line(&err.to_string())
                    ),
                };
                self.set_message(text, Severity::Error);
            }
        }
        effects.push(Effect::PollNow);
        effects.extend(self.logs_follow(false));
        effects
    }

    /// Reports the end of the trust flow of a hand-off (doc/0004,
    /// doc/0008): the interactive target command exited, so the hand-off is
    /// retried once, without stopping again. When it could not run at all
    /// the trust window shows why and nothing is retried.
    pub fn handoff_trust_finished(
        &mut self,
        job: HandoffJob,
        result: Result<Option<i32>, String>,
    ) -> Vec<Effect> {
        match result {
            Ok(_) => self.start_handoff(HandoffJob {
                request: job.request.after_stop(),
                after_trust: true,
                recorded: job.recorded,
            }),
            Err(err) => {
                self.open_untrusted(job, err);
                Vec::new()
            }
        }
    }

    /// What the logs panel should show for the selected row: a fetchable
    /// target, or a note (doc/0009).
    fn logs_wanted(&self) -> (String, Result<LogsTarget, String>) {
        let Some(m) = self.selected_session() else {
            return (String::new(), Err("no session selected".to_owned()));
        };
        let title = format!("{} ({})", m.session.display_name(), m.owner.label());
        let s = &m.session;
        if s.is_interactive() {
            return (title, Err(NOTE_INTERACTIVE.to_owned()));
        }
        let Some(id) = s.id.clone() else {
            return (title, Err("this row has no short id — no logs".to_owned()));
        };
        if !s.has_live_process() {
            return (title, Err(NOTE_STOPPED.to_owned()));
        }
        match &m.owner {
            Ownership::Unknown { .. } => (title, Err(NOTE_UNKNOWN_OWNER.to_owned())),
            Ownership::Known { account, .. } => match self.config.account(account) {
                Some(account) => (
                    title,
                    Ok(LogsTarget {
                        account: account.clone(),
                        id,
                    }),
                ),
                None => (title, Err(format!("account `{account}` is not configured"))),
            },
        }
    }

    /// What the logs job needs to read the real terminal size of
    /// `target`'s session (doc/0025): the row's live `pid` and the start
    /// ticks the pid-reuse guard remembered when it accepted that process
    /// (doc/0017). `None` when the row is gone, has no `pid`, is not owned
    /// by the target's account, or the guard has not accepted its process;
    /// the replay then infers the size. The job re-reads the ticks, so a
    /// pid reused meanwhile gives no size either.
    pub fn logs_probe(&self, target: &LogsTarget) -> Option<TtyProbe> {
        let m = self.sessions.iter().find(|m| {
            m.session.id.as_deref() == Some(target.id.as_str())
                && m.owner.account() == Some(target.account.name.as_str())
        })?;
        let pid = m.session.pid?;
        let start_ticks = self.known.get(m.session.key()?, pid)?;
        Some(TtyProbe { pid, start_ticks })
    }

    /// Points the open logs panel at the selected row: a note for rows
    /// without fetchable logs, otherwise a fetch when the row changed or
    /// `refresh` is set. At most one fetch runs at a time; a request while
    /// one runs is remembered and issued when it reports back.
    fn logs_follow(&mut self, refresh: bool) -> Vec<Effect> {
        if self.logs.is_none() {
            return Vec::new();
        }
        let (title, wanted) = self.logs_wanted();
        let Some(panel) = self.logs.as_mut() else {
            return Vec::new();
        };
        panel.title = title;
        match wanted {
            Err(note) => {
                panel.target = None;
                panel.pending = false;
                panel.view = LogsView::Note(note);
                Vec::new()
            }
            Ok(target) => {
                let changed = panel.target.as_ref() != Some(&target);
                if changed {
                    panel.target = Some(target.clone());
                    panel.view = LogsView::Loading;
                } else if !refresh {
                    return Vec::new();
                }
                if panel.in_flight.is_some() {
                    panel.pending = true;
                    return Vec::new();
                }
                panel.pending = false;
                panel.in_flight = Some(target.clone());
                vec![Effect::FetchLogs(target)]
            }
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() - 1;
        let target = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        self.select(target);
    }

    /// Selects line `index` (clamped). An empty list keeps the key, so
    /// the selection comes back to the same session when a filter that
    /// matched nothing is cleared.
    fn select(&mut self, index: usize) {
        if self.entries.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = index.min(self.entries.len() - 1);
        self.selected_key = match &self.entries[self.selected] {
            Entry::Header { group, .. } => Some(Selected::Header(group.clone())),
            Entry::Row(i) => Some(Selected::Row(row_key(&self.sessions[*i]))),
        };
    }

    /// `Tab`, or `Enter` on a header: collapses or expands the group of the
    /// selected line and remembers it for the session (doc/0006). The
    /// selection moves to the header of a group it collapses. While a
    /// filter is active every group shows its matches, so nothing folds.
    fn toggle_group(&mut self) {
        let Some(group) = self.selected_group() else {
            return;
        };
        if self.filter_active() {
            self.set_message(
                "groups stay open while a filter is active (Esc clears it)",
                Severity::Info,
            );
            return;
        }
        let open = !self.folded_open(&group);
        if open == group.default_expanded() {
            self.folded.remove(&group);
        } else {
            self.folded.insert(group.clone(), open);
        }
        if !open {
            self.selected_key = Some(Selected::Header(group));
        }
        self.rebuild_view(Keep::Same);
    }

    /// The group of the selected line: the header itself, or the header
    /// above the selected row.
    fn selected_group(&self) -> Option<GroupKey> {
        let at = self.selected_entry()?;
        self.entries[..=at].iter().rev().find_map(|e| match e {
            Entry::Header { group, .. } => Some(group.clone()),
            Entry::Row(_) => None,
        })
    }

    /// Replaces the merged sessions and rebuilds the list, keeping the
    /// selection on the same session when it is still listed.
    fn set_sessions(&mut self, merged: Vec<MergedSession>) {
        self.sessions = merged;
        self.rebuild_view(Keep::Same);
        // The pickers refer to a session by key; drop them if it vanished.
        let picked = match &self.overlay {
            Overlay::Picker(p) => Some(p.key.clone()),
            Overlay::Target(p) => Some(p.key.clone()),
            _ => None,
        };
        if let Some(key) = picked
            && !self
                .sessions
                .iter()
                .any(|m| m.session.key() == Some(key.as_str()))
        {
            self.overlay = Overlay::None;
            self.set_message("session no longer listed", Severity::Info);
        }
    }

    /// Orders the sessions for the current grouping and builds the visible
    /// lines (doc/0006): groups without a row (or, with a filter, without
    /// a match) are hidden, a collapsed group shows only its header, the
    /// Interactive group only while `i` shows it. Then puts the selection
    /// back as `keep` says.
    fn rebuild_view(&mut self, keep: Keep) {
        let now_ms = sidecar::now_ms();
        let all = std::mem::take(&mut self.sessions);
        let grouped = groups::group_with(self.grouping, &all, &self.home, |m| {
            activity::last_change_at(&m.session, &self.sidecar, now_ms)
        });
        let order: Vec<(GroupKey, Vec<MergedSession>)> = grouped
            .into_iter()
            .map(|g| (g.key, g.rows.into_iter().cloned().collect()))
            .collect();
        drop(all);
        let mut sessions = Vec::with_capacity(order.iter().map(|g| g.1.len()).sum());
        let mut entries = Vec::new();
        // Rows of collapsed groups, with the index of their header.
        let mut folded_rows: Vec<(String, usize)> = Vec::new();
        for (group, rows) in order {
            let first = sessions.len();
            sessions.extend(rows);
            if group.is_interactive() && !self.show_interactive {
                continue;
            }
            let shown: Vec<usize> = (first..sessions.len())
                .filter(|&i| self.matches_filter(&sessions[i]))
                .collect();
            if shown.is_empty() {
                continue;
            }
            let collapsed = self.is_collapsed(&group);
            let highlighted = shown
                .iter()
                .filter(|&&i| self.is_highlighted(&sessions[i]))
                .count();
            let header = entries.len();
            entries.push(Entry::Header {
                group,
                count: shown.len(),
                highlighted,
                collapsed,
            });
            if collapsed {
                folded_rows.extend(shown.iter().map(|&i| (row_key(&sessions[i]), header)));
            } else {
                entries.extend(shown.into_iter().map(Entry::Row));
            }
        }
        self.sessions = sessions;
        self.entries = entries;
        let first_row = || {
            self.entries
                .iter()
                .position(|e| matches!(e, Entry::Row(_)))
                .unwrap_or(0)
        };
        let index = match (keep, &self.selected_key) {
            (Keep::FirstRow, _) | (Keep::Same, None) => first_row(),
            (Keep::Same, Some(Selected::Header(key))) => self
                .entries
                .iter()
                .position(|e| matches!(e, Entry::Header { group, .. } if group == key))
                .unwrap_or(self.selected),
            (Keep::Same, Some(Selected::Row(key))) => self
                .entries
                .iter()
                .position(|e| match e {
                    Entry::Row(i) => row_key(&self.sessions[*i]) == *key,
                    Entry::Header { .. } => false,
                })
                .or_else(|| {
                    folded_rows
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, header)| *header)
                })
                .unwrap_or(self.selected),
        };
        self.select(index);
    }
}

/// The message when a poll learned the owner of a session the picker was
/// asked about.
fn learned_text(name: &str, known: &str, picked: &str) -> String {
    if known == picked {
        format!("{name} turned out to run under {known}")
    } else {
        format!("{name} turned out to run under {known}; using {known} instead of {picked}")
    }
}

/// The message for a row action (`Enter`, `a`, `s`, `d`) on an interactive
/// row (doc/0005): it runs in another terminal, and moving it to the
/// background there makes it a row tower can manage.
pub fn interactive_hint(name: &str) -> String {
    format!(
        "{name} runs in another terminal — press ← on an empty prompt there (or /bg) to move \
         it into the background; then it can be managed here"
    )
}

/// Whether `stop` asks first: rows that are working or blocked, i.e. whose
/// state puts them in the Needs me or Working group (doc/0009), the
/// orchestrator included. Rows without a live process are shown as stopped
/// and stop directly.
pub fn stop_needs_confirmation(s: &Session) -> bool {
    matches!(groups::state_group(s), Group::NeedsMe | Group::Working)
}

/// Identity of a row for keeping the selection across polls.
fn row_key(m: &MergedSession) -> String {
    match (m.session.key(), m.session.pid) {
        (Some(k), _) => k.to_owned(),
        (None, Some(pid)) => format!("pid:{pid}"),
        (None, None) => format!("name:{}", m.session.display_name()),
    }
}

/// Collapses whitespace (including newlines from stderr) to single spaces.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "3s", "5m", "2h", "4d".
pub fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(short_duration(Duration::from_secs(3)), "3s");
        assert_eq!(short_duration(Duration::from_secs(300)), "5m");
        assert_eq!(short_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(short_duration(Duration::from_secs(4 * 86400)), "4d");
    }

    #[test]
    fn one_line_collapses() {
        assert_eq!(one_line(" a\n b\tc "), "a b c");
    }
}
