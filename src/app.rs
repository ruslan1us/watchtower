//! The TUI state machine for milestone 1 (doc/0014): the merged session list,
//! selection, the help overlay and the account picker, and the key table
//! (doc/0011).
//!
//! Nothing here touches the terminal, spawns processes or does file I/O. Key
//! presses, poll results and attach outcomes go in; [`Effect`]s come out and
//! the runtime (`tui`) carries them out. That keeps the whole flow testable
//! without a terminal (doc/0015).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::config::{Account, Config};
use crate::groups::{self, Group};
use crate::listing::Session;
use crate::merge::{AccountListing, EnvironReader, MergedSession, Ownership, Resolver};
use crate::poller::PollUpdate;
use crate::sidecar::{AccountSource, Sidecar};

/// How long a transient status message stays visible.
pub const MESSAGE_TTL: Duration = Duration::from_secs(6);

/// Something the runtime must do on the app's behalf, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Leave tower.
    Quit,
    /// Poll every account now.
    PollNow,
    /// Write the sidecar file; it changed.
    SaveSidecar,
    /// Hand the terminal to `<account command> attach <id>` (doc/0005).
    Attach(AttachRequest),
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
}

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
    Refresh,
    Help,
    Quit,
    /// `Ctrl+Z`: reserved for detaching inside a session; a no-op here.
    DetachKeyHint,
    /// A key of a later milestone; the text names the feature.
    NotYet(&'static str),
}

/// Keys of the session list (doc/0011, milestone 1 subset).
pub const KEY_TABLE: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move selection"),
    ("PgUp/PgDn", "move one page"),
    ("Home/End, g/G", "first / last row"),
    ("Enter", "attach to the selected session"),
    ("r", "refresh now"),
    ("?", "this help"),
    ("q, Ctrl+C", "quit"),
    ("Ctrl+Z", "inside a session: return to tower"),
];

/// Keys of later milestones, shown in the help as not yet available.
pub const LATER_KEYS: &str = "n a o s d m Space / p i Tab";

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
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('?') => Action::Help,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('n') => Action::NotYet("new session"),
        KeyCode::Char('a') => Action::NotYet("continue under another account"),
        KeyCode::Char('o') => Action::NotYet("orchestrator"),
        KeyCode::Char('s') => Action::NotYet("stop"),
        KeyCode::Char('d') => Action::NotYet("remove"),
        KeyCode::Char('m') => Action::NotYet("mark seen"),
        KeyCode::Char(' ') => Action::NotYet("logs panel (Space)"),
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
    /// Tower's sidecar state; the runtime saves it on [`Effect::SaveSidecar`].
    pub sidecar: Sidecar,
    polls: Vec<AccountPoll>,
    sessions: Vec<MergedSession>,
    entries: Vec<Entry>,
    /// Indices into `entries` of the selectable (session) lines.
    rows: Vec<usize>,
    /// Index into `rows`.
    selected: usize,
    selected_key: Option<String>,
    overlay: Overlay,
    message: Option<Message>,
    warnings: Vec<String>,
    last_refresh: Option<Instant>,
    /// Rows per page for PgUp/PgDn; the renderer keeps it current.
    pub page_size: usize,
    /// Scroll state of the list widget, kept between frames.
    pub list_state: ListState,
    /// Whether colours may be used (`NO_COLOR` unset).
    pub color: bool,
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
            })
            .collect();
        App {
            config,
            home,
            environ,
            sidecar,
            polls,
            sessions: Vec::new(),
            entries: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            selected_key: None,
            overlay: Overlay::None,
            message: None,
            warnings: Vec::new(),
            last_refresh: None,
            page_size: 10,
            list_state: ListState::default(),
            color: true,
        }
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

    /// The status line at time `now`: the transient message (if fresh), then
    /// the refresh time, per-account errors and warnings.
    pub fn status_segments(&self, now: Instant) -> Vec<StatusSegment> {
        let mut out = Vec::new();
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
        for w in &self.warnings {
            out.push(StatusSegment {
                text: one_line(w),
                severity: Severity::Warning,
            });
        }
        out
    }

    /// Applies one finished poll: merges all accounts, records detections in
    /// the sidecar and rebuilds the list. Returns [`Effect::SaveSidecar`] when
    /// the sidecar changed.
    pub fn apply_poll(&mut self, update: PollUpdate) -> Vec<Effect> {
        let Some(poll) = self.polls.iter_mut().find(|p| p.name == update.account) else {
            return Vec::new();
        };
        match update.result {
            Ok(listing) => {
                poll.rows = Some(listing.sessions);
                poll.warnings = listing.warnings;
                poll.error = None;
                self.last_refresh = Some(Instant::now());
            }
            Err(err) => poll.error = Some(err),
        }
        let listings: Vec<AccountListing> = self
            .polls
            .iter()
            .map(|p| match (&p.rows, &p.error) {
                (Some(rows), None) => AccountListing::fresh(&p.name, rows.clone()),
                (Some(rows), Some(_)) => AccountListing::stale(&p.name, rows.clone()),
                (None, _) => AccountListing::missing(&p.name),
            })
            .collect();
        let merged = Resolver::new(&self.config.accounts, &self.home, self.environ.as_ref())
            .merge(&listings, &self.sidecar);
        let changed = self.sidecar.record_detections(&merged);
        self.set_sessions(merged);
        if changed {
            vec![Effect::SaveSidecar]
        } else {
            Vec::new()
        }
    }

    /// Reports how an attach ended and asks for an immediate poll.
    pub fn attach_finished(&mut self, outcome: AttachOutcome) -> Vec<Effect> {
        let who = format!("{} ({})", outcome.name, outcome.account);
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

    /// Handles one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        match self.overlay.clone() {
            Overlay::Help => self.help_key(key),
            Overlay::Picker(picker) => self.picker_key(key, picker),
            Overlay::None => self.list_key(key),
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
                self.set_message("attach cancelled", Severity::Info);
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

    /// Records the picked account (`asked`, doc/0003) and attaches with it.
    fn pick_account(&mut self, picker: Picker) -> Vec<Effect> {
        self.overlay = Overlay::None;
        let Some(account) = self.config.accounts.get(picker.selected).cloned() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if self.sidecar.record_answer(&picker.key, &account.name) {
            effects.push(Effect::SaveSidecar);
        }
        for m in &mut self.sessions {
            if m.session.key() == Some(picker.key.as_str()) {
                m.owner = Ownership::Known {
                    account: account.name.clone(),
                    source: AccountSource::Asked,
                };
            }
        }
        effects.push(Effect::Attach(AttachRequest {
            account,
            id: picker.id,
            name: picker.name,
        }));
        effects
    }

    fn list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(action) = list_action(&key) else {
            return Vec::new();
        };
        match action {
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-(self.page_size.max(1) as isize)),
            Action::PageDown => self.move_by(self.page_size.max(1) as isize),
            Action::First => self.select(0),
            Action::Last => self.select(self.rows.len().saturating_sub(1)),
            Action::Attach => return self.request_attach(),
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
        Vec::new()
    }

    /// Enter on a row: attach with the owning account, or ask for the
    /// account first when it is unknown (doc/0003, doc/0005).
    fn request_attach(&mut self) -> Vec<Effect> {
        let Some(m) = self.selected_session() else {
            return Vec::new();
        };
        let name = m.session.display_name();
        let (Some(id), true) = (m.session.id.clone(), m.session.is_attachable()) else {
            self.set_message(
                format!(
                    "{name} is an interactive session in another terminal; it cannot be attached"
                ),
                Severity::Info,
            );
            return Vec::new();
        };
        match &m.owner {
            Ownership::Known { account, .. } => match self.config.account(account) {
                Some(account) => vec![Effect::Attach(AttachRequest {
                    account: account.clone(),
                    id,
                    name,
                })],
                None => {
                    self.set_message(
                        format!("account `{account}` is not configured"),
                        Severity::Error,
                    );
                    Vec::new()
                }
            },
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
                });
                Vec::new()
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
        // The picker refers to a session by key; drop it if it vanished.
        if let Overlay::Picker(p) = &self.overlay
            && !self
                .sessions
                .iter()
                .any(|m| m.session.key() == Some(p.key.as_str()))
        {
            self.overlay = Overlay::None;
            self.set_message("session no longer listed", Severity::Info);
        }
    }
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
