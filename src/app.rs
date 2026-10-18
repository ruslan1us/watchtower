//! The TUI state machine (doc/0014 milestones 1–3): the merged session
//! list, selection, the help overlay, the account picker, confirmations, the
//! new-session dialog ([`crate::dialog`], doc/0008), stop and remove, the
//! logs panel (doc/0009), the hand-off to another account (doc/0004), and
//! the key table (doc/0011).
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

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::actions::{RemoveError, StopError};
use crate::config::{Account, Config};
use crate::dialog::{Dialog, DialogAction, DialogStatus, Field};
use crate::dispatch::{self, DispatchError, DispatchRequest, Dispatched};
use crate::groups::{self, Group};
use crate::handoff::{self, HandoffError, HandoffRequest, Outcome, Resumed, Step};
use crate::listing::Session;
use crate::logview::{LogScreen, LogsError};
use crate::merge::{
    AccountListing, CanonCache, EnvironReader, KnownProcesses, MergedSession, Ownership, Resolver,
};
use crate::paths;
use crate::poller::PollUpdate;
use crate::projects::ProjectList;
use crate::sidecar::{
    self, AccountSource, CheckJob, OwnerRecord, RecentProject, Reloaded, SaveJob, Saved, Sidecar,
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

/// One line of the session list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A group header with its row count.
    Header { group: Group, count: usize },
    /// A session: index into [`App::sessions`].
    Row(usize),
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
    /// Hand off a working or blocked session: stopping it interrupts the
    /// turn (doc/0004); `detail` is its state label.
    Handoff {
        job: Box<HandoffJob>,
        detail: String,
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
    /// Candidate target accounts, in config order; never the owner.
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
    ToggleLogs,
    /// `Esc`: close the logs panel.
    Close,
    Refresh,
    Help,
    Quit,
    /// `Ctrl+Z`: reserved for detaching inside a session; a no-op here.
    DetachKeyHint,
    /// A key of a later milestone; the text names the feature.
    NotYet(&'static str),
}

/// Keys of the session list (doc/0011, milestones 1–3).
pub const KEY_TABLE: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move selection"),
    ("PgUp/PgDn", "move one page"),
    ("Home/End, g/G", "first / last row"),
    ("Enter", "attach to the selected session"),
    ("n", "new session"),
    ("a", "continue under another account (hand-off)"),
    ("s", "stop the session (confirm when working or blocked)"),
    ("d", "remove the session (confirm)"),
    ("Space, Esc", "toggle / close the logs panel"),
    ("r", "refresh now"),
    ("?", "this help"),
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

/// Keys of later milestones, shown in the help as not yet available.
pub const LATER_KEYS: &str = "o m / p i Tab";

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
        KeyCode::Char('o') => Action::NotYet("orchestrator"),
        KeyCode::Char('m') => Action::NotYet("mark seen"),
        KeyCode::Char('/') => Action::NotYet("filter"),
        KeyCode::Char('p') => Action::NotYet("group by project"),
        KeyCode::Char('i') => Action::NotYet("show/hide interactive"),
        KeyCode::Tab => Action::NotYet("collapse group (Tab)"),
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
    sessions: Vec<MergedSession>,
    entries: Vec<Entry>,
    /// Indices into `entries` of the selectable (session) lines.
    rows: Vec<usize>,
    /// Index into `rows`.
    selected: usize,
    selected_key: Option<String>,
    overlay: Overlay,
    dialog: Option<Dialog>,
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
    /// Why the last sidecar check failed; shown until one succeeds.
    check_error: Option<String>,
    /// Number of the latest project scan; the open dialog waits for it.
    scan: u64,
    /// Whether tower runs as root (hides `bypassPermissions`, doc/0008).
    root: bool,
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
            rows: Vec::new(),
            selected: 0,
            selected_key: None,
            overlay: Overlay::None,
            dialog: None,
            logs: None,
            popup: None,
            waiting: VecDeque::new(),
            pending_attach: None,
            attach_note: None,
            sidecar_io: SidecarIo::default(),
            check_error: None,
            scan: 0,
            root: dispatch::is_root(),
            message: None,
            handoff: None,
            quit_prompt: None,
            warnings: Vec::new(),
            last_refresh: None,
            page_size: 10,
            list_state: ListState::default(),
            color: true,
            keyboard_enhanced: false,
        }
    }

    /// Overrides the root check (tests; doc/0008 hides `bypassPermissions`
    /// for root).
    pub fn set_root(&mut self, root: bool) {
        self.root = root;
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

    /// Index into [`App::entries`] of the selected row, if any.
    pub fn selected_entry(&self) -> Option<usize> {
        self.rows.get(self.selected).copied()
    }

    /// The selected session, if any.
    pub fn selected_session(&self) -> Option<&MergedSession> {
        match self.entries.get(self.selected_entry()?)? {
            Entry::Row(i) => self.sessions.get(*i),
            Entry::Header { .. } => None,
        }
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
    /// accepted carry over between polls), records detections
    /// in the sidecar and rebuilds the list. Returns [`Effect::SaveSidecar`]
    /// when the sidecar changed, and a logs fetch when the panel is open and
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
        changed |= self.rebuild();
        let mut effects = Vec::new();
        // A save that failed (e.g. the lock was busy) is retried.
        if changed || self.sidecar.retry_due() {
            effects.push(Effect::SaveSidecar);
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
    /// list (see [`App::apply_poll`]). Returns true when the sidecar
    /// changed.
    fn rebuild(&mut self) -> bool {
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
        changed |= self
            .sidecar
            .observe_listing(seen, complete, sidecar::now_ms());
        self.set_sessions(merged);
        changed
    }

    /// Reports how an attach ended and asks for an immediate poll.
    pub fn attach_finished(&mut self, outcome: AttachOutcome) -> Vec<Effect> {
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
                    effects.push(Effect::Attach(AttachRequest {
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
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        if let Some(prompt) = self.quit_prompt.clone() {
            return self.quit_prompt_key(key, prompt);
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

    fn route_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.popup.is_some() {
            return self.popup_key(key);
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        match self.overlay.clone() {
            Overlay::Help => self.help_key(key),
            Overlay::Picker(picker) => self.picker_key(key, picker),
            Overlay::Confirm(confirm) => self.confirm_key(key, confirm),
            Overlay::Target(picker) => self.target_key(key, picker),
            Overlay::Untrusted(untrusted) => self.untrusted_key(key, *untrusted),
            Overlay::None => self.list_key(key),
        }
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
            Action::Last => self.select(self.rows.len().saturating_sub(1)),
            Action::Attach => effects = self.request_action(Purpose::Attach),
            Action::NewSession => return vec![Effect::OpenDialog],
            Action::Stop => effects = self.request_action(Purpose::Stop),
            Action::Remove => effects = self.request_action(Purpose::Remove),
            Action::Handoff => effects = self.request_handoff(),
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
                self.logs = None;
                return Vec::new();
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
            Action::NotYet(what) => {
                self.set_message(format!("{what}: not available yet"), Severity::Info)
            }
        }
        effects.extend(self.logs_follow(false));
        effects
    }

    /// A row action (attach, stop, remove) on the selected row: with the
    /// owning account, or after asking for it when it is unknown (doc/0003,
    /// doc/0005, doc/0009).
    fn request_action(&mut self, purpose: Purpose) -> Vec<Effect> {
        let Some(m) = self.selected_session() else {
            return Vec::new();
        };
        let name = m.session.display_name();
        let verb = purpose.verb();
        if m.session.is_interactive() {
            let text = match purpose {
                Purpose::Attach => format!(
                    "{name} is an interactive session in another terminal; it cannot be attached"
                ),
                _ => format!(
                    "{name} is an interactive session in another terminal; tower cannot {verb} it"
                ),
            };
            self.set_message(text, Severity::Info);
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
        effects.push(Effect::Attach(AttachRequest {
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
        if self.rebuild() {
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

    // ---- the project scan (doc/0008) --------------------------------------

    /// Opens the new-session dialog with an empty project list ("scanning
    /// projects…") and returns the scan to run on a worker thread.
    pub fn begin_project_scan(&mut self) -> ProjectScan {
        self.scan += 1;
        self.open_dialog(ProjectList::default());
        if let Some(d) = self.dialog.as_mut() {
            d.scanning = true;
        }
        ProjectScan {
            id: self.scan,
            roots: self.config.project_roots.clone(),
            extras: self.config.extra_projects.clone(),
            recent: self.sidecar.recent_projects.clone(),
            home: self.home.clone(),
        }
    }

    /// Fills the dialog's list with the result of scan `id`; a result for
    /// a dialog that was closed meanwhile is dropped.
    pub fn projects_found(&mut self, id: u64, list: ProjectList) {
        if id != self.scan {
            return;
        }
        if let Some(d) = self.dialog.as_mut()
            && d.scanning
        {
            d.set_projects(list);
        }
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
            self.set_message(
                format!(
                    "{name} is an interactive session in another terminal; it cannot be \
                     continued under another account"
                ),
                Severity::Info,
            );
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
        let targets: Vec<String> = self
            .config
            .accounts
            .iter()
            .filter(|a| owner.as_deref() != Some(a.name.as_str()))
            .map(|a| a.name.clone())
            .collect();
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
    /// the current row: confirmed first when the session is working or
    /// blocked (`stop` interrupts the turn), otherwise started.
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
        let request = HandoffRequest {
            id: s.id.clone().unwrap_or_default(),
            session_id: key.to_owned(),
            name: s.display_name(),
            cwd: paths::expand_tilde(&s.cwd, &self.home),
            owner: m
                .owner
                .account()
                .and_then(|a| self.config.account(a))
                .cloned(),
            target,
            live: s.has_live_process(),
            prompt: self.config.handoff_prompt.clone(),
        };
        let confirm = request.live && stop_needs_confirmation(s);
        let detail = groups::detail(s);
        let job = HandoffJob {
            request,
            after_trust: false,
            recorded: self.sidecar.base_owner(key),
        };
        if confirm {
            self.overlay = Overlay::Confirm(Confirm::Handoff {
                job: Box::new(job),
                detail,
            });
            return Vec::new();
        }
        self.start_handoff(job)
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
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let target = (self.selected as isize + delta).clamp(0, last as isize) as usize;
        self.select(target);
    }

    fn select(&mut self, index: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.selected_key = None;
            return;
        }
        self.selected = index.min(self.rows.len() - 1);
        self.selected_key = self.selected_session().map(row_key);
    }

    /// Replaces the merged sessions, ordered for display, and keeps the
    /// selection on the same session when it is still listed.
    fn set_sessions(&mut self, merged: Vec<MergedSession>) {
        let grouped = groups::group_by_state(&merged);
        let mut sessions = Vec::with_capacity(merged.len());
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        for g in &grouped {
            entries.push(Entry::Header {
                group: g.group,
                count: g.rows.len(),
            });
            for m in &g.rows {
                rows.push(entries.len());
                entries.push(Entry::Row(sessions.len()));
                sessions.push((*m).clone());
            }
        }
        self.sessions = sessions;
        self.entries = entries;
        self.rows = rows;
        let keep = self.selected_key.as_ref().and_then(|key| {
            self.rows.iter().position(|&e| match self.entries[e] {
                Entry::Row(i) => row_key(&self.sessions[i]) == *key,
                Entry::Header { .. } => false,
            })
        });
        self.select(keep.unwrap_or(self.selected));
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

/// Whether `stop` asks first: rows that are working or blocked, i.e. in the
/// Needs me or Working group (doc/0009). Rows without a live process are
/// shown as stopped and stop directly.
pub fn stop_needs_confirmation(s: &Session) -> bool {
    matches!(groups::classify(s), Group::NeedsMe | Group::Working)
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
