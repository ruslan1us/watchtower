//! The workspace style's state (doc/0045): which row the pane follows, who
//! has the keys and when the one PTY client opens and closes. Like the rest of
//! [`App`] it does no I/O: it returns [`Effect`]s, and the runtime runs the
//! client ([`crate::embed`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{App, AttachRequest, Effect, Entry, Overlay, Purpose, Severity};
use crate::config::{Account, UiStyle};
use crate::embed::{self, PaneScreen};
use crate::merge::MergedSession;

/// How long the selection rests on a row before the pane attaches to it, so
/// the arrows can run over rows without starting anything.
pub const ATTACH_DEBOUNCE: Duration = Duration::from_millis(150);

/// A client that has not had the keys for this long is released: `claude
/// attach` keeps a session from its idle stop, and tower must not.
pub const RELEASE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Why a row is not attached.
const HELD_ENDED: &str = "its terminal client ended";
const HELD_ACTION: &str = "tower closed its terminal for your last action on it";
const HELD_RELEASED: &str = "its terminal was released after 10 minutes without the keys";

/// Who has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFocus {
    /// The session list, and every window above it.
    #[default]
    List,
    /// Claude's terminal in the pane.
    Terminal,
}

/// A client to start ([`Effect::PaneOpen`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneOpen {
    /// Names this client in everything reported about it.
    pub generation: u64,
    /// The owning account: its command attaches (doc/0003).
    pub account: Account,
    /// The short id; never the name (doc/0005).
    pub id: String,
    /// The session's folder.
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
}

/// The client of the row the pane shows.
struct PaneClient {
    key: String,
    id: String,
    name: String,
    generation: u64,
    screen: Option<PaneScreen>,
    size: (u16, u16),
    /// When the client last had the keys, or opened.
    last_keys: Instant,
}

/// A failed client's last screen, kept until the selection moves so its error
/// text can be read.
struct Ended {
    name: String,
    code: Option<i32>,
    screen: PaneScreen,
}

/// A row the pane does not attach to until the selection comes back to it or
/// `Enter` asks.
struct Held {
    key: String,
    why: &'static str,
}

/// What a client needs of its row.
struct RowInfo {
    key: String,
    id: String,
    name: String,
    cwd: PathBuf,
    account: Account,
}

/// The pane's state in [`App`].
pub(super) struct PaneState {
    focus: PaneFocus,
    /// The pane shows Home and no row is selected, until a row is chosen. It
    /// counts only while the workspace is drawn ([`App::pane_home`]).
    home: bool,
    /// The user chose a row since the style came on: an arrow leaving Home
    /// returns to that row, not to the first.
    chosen: bool,
    /// The row the pane follows, `None` on a header or an empty list.
    selected: Option<String>,
    /// When the selection last changed.
    since: Option<Instant>,
    client: Option<PaneClient>,
    ended: Option<Ended>,
    held: Option<Held>,
    /// The pane's inside, as the renderer last drew it; `None` while it does
    /// not draw the workspace.
    rect: Option<Rect>,
    /// The list bar on the screen, and the list entry on each of its rows.
    bar: Option<Rect>,
    bar_rows: Vec<(u16, usize)>,
    /// Why no client can start; tower then draws the dashboard.
    failed: Option<String>,
    /// The short id whose checked attach (`Enter` on a stopped row, or on an
    /// unknown owner) opens here instead of taking the terminal.
    wake: Option<String>,
    /// A press went to the child and its release has not come.
    gesture: bool,
    generations: u64,
    narrow_noted: bool,
}

impl Default for PaneState {
    fn default() -> PaneState {
        PaneState {
            focus: PaneFocus::default(),
            home: true,
            chosen: false,
            selected: None,
            since: None,
            client: None,
            ended: None,
            held: None,
            rect: None,
            bar: None,
            bar_rows: Vec::new(),
            failed: None,
            wake: None,
            gesture: false,
            generations: 0,
            narrow_noted: false,
        }
    }
}

/// The session a tower action works on, when the action can stop, wake or
/// remove it: the pane's client on that session goes first.
fn action_id(effect: &Effect) -> Option<&str> {
    match effect {
        Effect::Stop(a) => Some(&a.id),
        Effect::Remove(job) => Some(&job.action.id),
        Effect::Handoff(job) => Some(&job.request.id),
        Effect::Rename(job) => Some(&job.request.id),
        Effect::Reply(job) => Some(&job.request.id),
        Effect::Attach(req) => Some(&req.id),
        _ => None,
    }
}

impl App {
    // ---- what the renderer and the runtime read -----------------------------

    /// Whether the workspace is on the screen.
    pub fn pane_active(&self) -> bool {
        self.style == UiStyle::Workspace && self.pane.rect.is_some() && self.pane.failed.is_none()
    }

    /// Whether the pane shows Home: the workspace is drawn and no row is
    /// chosen, so nothing is selected or attached by itself (doc/0045).
    pub fn pane_home(&self) -> bool {
        self.pane.home && self.pane_active()
    }

    /// Whether a client failed to start, so tower draws the dashboard.
    pub fn pane_failed(&self) -> bool {
        self.pane.failed.is_some()
    }

    /// Whether the real terminal should report the mouse (doc/0045).
    pub fn wants_mouse(&self) -> bool {
        self.pane_active() && self.config.ui_mouse
    }

    /// Who has the keys, apart from a window that is open.
    pub fn pane_focus(&self) -> PaneFocus {
        self.pane.focus
    }

    /// Whether keys and paste go to the child now: no window takes them.
    pub fn pane_has_keys(&self) -> bool {
        self.pane.focus == PaneFocus::Terminal
            && self.pane.client.is_some()
            && self.pane_active()
            && !self.window_open()
            && !self.filter.editing
    }

    /// The screen to draw: the client's, or a failed client's last one.
    pub fn pane_screen(&self) -> Option<&PaneScreen> {
        match &self.pane.client {
            Some(client) => client.screen.as_ref(),
            None => self.pane.ended.as_ref().map(|e| &e.screen),
        }
    }

    /// The session key of the row whose client the pane shows.
    pub fn pane_client_key(&self) -> Option<&str> {
        self.pane.client.as_ref().map(|c| c.key.as_str())
    }

    /// The name of the client's row, kept for when the row has left the list.
    pub fn pane_client_name(&self) -> Option<&str> {
        match &self.pane.client {
            Some(client) => Some(&client.name),
            None => self.pane.ended.as_ref().map(|e| e.name.as_str()),
        }
    }

    /// How the client whose last screen is shown ended.
    pub fn pane_ended_code(&self) -> Option<Option<i32>> {
        self.pane.ended.as_ref().map(|e| e.code)
    }

    /// Why the selected row is not attached, when it is held.
    pub fn pane_held(&self) -> Option<&'static str> {
        let held = self.pane.held.as_ref()?;
        (self.pane.selected.as_deref() == Some(held.key.as_str())).then_some(held.why)
    }

    /// Whether tower itself is working on the row (hand-off, rename, quick
    /// reply), so nothing attaches to it.
    pub fn pane_row_busy(&self, m: &MergedSession) -> bool {
        self.handing_off(&m.session)
            || self.renaming_row(&m.session)
            || self.replying_row(&m.session)
    }

    /// Set by the renderer on every frame: the pane's inside, while the
    /// workspace is drawn.
    pub fn set_pane_rect(&mut self, rect: Option<Rect>) {
        self.pane.rect = rect;
    }

    /// Set by the renderer: the list bar and which list entry each of its
    /// rows shows, for clicks.
    pub fn set_pane_bar(&mut self, bar: Rect, rows: Vec<(u16, usize)>) {
        self.pane.bar = Some(bar);
        self.pane.bar_rows = rows;
    }

    /// Called by the renderer while the workspace gives way to the dashboard
    /// because the terminal is smaller than `min` (columns, rows): the first
    /// time, the status line says so (doc/0045).
    pub fn note_pane_fallback(&mut self, min: (u16, u16)) {
        if self.pane.narrow_noted || self.pane.failed.is_some() {
            return;
        }
        self.pane.narrow_noted = true;
        self.set_message(
            format!(
                "the workspace needs {}x{}; dashboard until the terminal is larger",
                min.0, min.1
            ),
            Severity::Info,
        );
    }

    // ---- following the selection ---------------------------------------------

    fn pane_account(&self, m: &MergedSession) -> Option<Account> {
        m.owner
            .account()
            .and_then(|name| self.config.account(name))
            .cloned()
    }

    fn pane_row_info(&self, m: &MergedSession) -> Option<RowInfo> {
        Some(RowInfo {
            key: m.session.key()?.to_owned(),
            id: m.session.id.clone()?,
            name: m.session.display_name(),
            cwd: PathBuf::from(&m.session.cwd),
            account: self.pane_account(m)?,
        })
    }

    /// The selected row, when the pane attaches to it by itself: running,
    /// with an account tower knows, and nothing else going on with it.
    /// Stopped rows wake only on `Enter` (doc/0045).
    fn pane_auto_row(&self) -> Option<RowInfo> {
        let m = self.selected_session()?;
        let s = &m.session;
        let usable = s.is_attachable()
            && s.has_live_process()
            && !self.pane_row_busy(m)
            && self.pane_held().is_none();
        if usable { self.pane_row_info(m) } else { None }
    }

    /// A client for `row`, replacing the one there is (the runtime closes it
    /// first).
    fn pane_start(&mut self, row: RowInfo, now: Instant) -> Effect {
        self.pane.generations += 1;
        let generation = self.pane.generations;
        let (rows, cols) = self.pane.rect.map_or((1, 1), |r| (r.height, r.width));
        self.pane.ended = None;
        self.pane.held = None;
        self.pane.client = Some(PaneClient {
            key: row.key,
            id: row.id.clone(),
            name: row.name,
            generation,
            screen: None,
            size: (rows, cols),
            last_keys: now,
        });
        Effect::PaneOpen(PaneOpen {
            generation,
            account: row.account,
            id: row.id,
            cwd: row.cwd,
            rows,
            cols,
        })
    }

    /// Time passes: the runtime calls this on every turn of its loop, after
    /// the turn's other effects. Follows the selection after the debounce,
    /// resizes and releases the client, and acknowledges the row whose
    /// terminal has the keys.
    pub fn pane_tick(&mut self, now: Instant) -> Vec<Effect> {
        // Over the Settings window nothing changes, whatever it previews.
        if self.settings.is_some() {
            return Vec::new();
        }
        if !self.pane_active() {
            // Whenever the style comes on, it starts on Home (doc/0045).
            if self.style != UiStyle::Workspace {
                self.pane.home = true;
                self.pane.chosen = false;
            }
            let pane = &mut self.pane;
            pane.focus = PaneFocus::List;
            pane.gesture = false;
            pane.selected = None;
            pane.since = None;
            pane.ended = None;
            pane.held = None;
            pane.wake = None;
            return match pane.client.take() {
                Some(_) => vec![Effect::PaneClose],
                None => Vec::new(),
            };
        }
        // The pane has the logs panel's place, and more (doc/0045).
        self.logs = None;
        let mut effects = Vec::new();
        let key = self
            .selected_session()
            .and_then(|m| m.session.key())
            .map(str::to_owned);
        if key != self.pane.selected || self.pane.since.is_none() {
            if key != self.pane.selected {
                self.pane.ended = None;
                if self.pane.held.as_ref().map(|h| &h.key) != key.as_ref() {
                    self.pane.held = None;
                }
                effects.extend(self.pane_focus_list(now));
                self.pane.gesture = false;
            }
            self.pane.selected.clone_from(&key);
            self.pane.since = Some(now);
        }
        if self.pane.wake.is_some()
            && self.pending_attach.is_none()
            && !matches!(self.overlay, Overlay::Picker(_))
        {
            self.pane.wake = None;
        }
        // The pane's size, followed at once.
        if let (Some(client), Some(rect)) = (self.pane.client.as_mut(), self.pane.rect) {
            let size = (rect.height, rect.width);
            if client.size != size {
                client.size = size;
                effects.push(Effect::PaneResize {
                    rows: size.0,
                    cols: size.1,
                });
            }
        }
        let rested = self
            .pane
            .since
            .is_some_and(|at| now.saturating_duration_since(at) >= ATTACH_DEBOUNCE);
        // Home closes nothing: a client stays, hidden, until the selection
        // picks another row or the release below (doc/0045).
        if rested && !self.pane_home() && self.pane_client_key().map(str::to_owned) != key {
            match self.pane_auto_row() {
                Some(row) => effects.push(self.pane_start(row, now)),
                None => {
                    if self.pane.client.take().is_some() {
                        effects.push(Effect::PaneClose);
                    }
                }
            }
        }
        if let Some(client) = &mut self.pane.client {
            if self.pane.focus == PaneFocus::Terminal {
                client.last_keys = now;
            } else if now.saturating_duration_since(client.last_keys) >= RELEASE_AFTER {
                let key = client.key.clone();
                self.pane.client = None;
                self.pane.held = Some(Held {
                    key,
                    why: HELD_RELEASED,
                });
                effects.push(Effect::PaneClose);
            }
        }
        // The row whose terminal has the keys is being looked at: what it
        // asks now is seen, as after an attach (doc/0007).
        if self.pane.focus == PaneFocus::Terminal
            && let Some(key) = self.pane_client_key().map(str::to_owned)
            && self.acknowledge(&key)
        {
            effects.push(Effect::SaveSidecar);
        }
        effects
    }

    /// How soon [`App::pane_tick`] has something to do: when the debounce of
    /// a selection ends. The runtime's wait is at most this long.
    pub fn pane_wait(&self, now: Instant) -> Option<Duration> {
        if !self.pane_active()
            || self.pane_home()
            || self.pane_client_key() == self.pane.selected.as_deref()
        {
            return None;
        }
        let rested_at = self.pane.since? + ATTACH_DEBOUNCE;
        Some(rested_at.saturating_duration_since(now))
    }

    // ---- what the runtime reports ---------------------------------------------

    /// [`Effect::PaneOpen`] has started: the client's screen.
    pub fn pane_opened(&mut self, generation: u64, screen: PaneScreen) {
        if let Some(client) = &mut self.pane.client
            && client.generation == generation
        {
            client.screen = Some(screen);
        }
    }

    /// No client could be started: tower falls back to the dashboard until it
    /// restarts (doc/0045).
    pub fn pane_start_failed(&mut self, generation: u64, error: &str) {
        if self
            .pane
            .client
            .as_ref()
            .is_some_and(|c| c.generation == generation)
        {
            self.pane.client = None;
            self.pane.focus = PaneFocus::List;
            self.pane.failed = Some(error.to_owned());
            self.set_message(
                format!("the workspace cannot start a terminal ({error}); using the dashboard"),
                Severity::Error,
            );
        }
    }

    /// The client ended by itself: focus returns to the list, the row is
    /// polled and not attached again until asked. A failed client's last
    /// screen stays until the selection moves, with its error text.
    pub fn pane_exited(&mut self, generation: u64, code: Option<i32>) -> Vec<Effect> {
        let Some(client) = self.pane.client.take_if(|c| c.generation == generation) else {
            return Vec::new();
        };
        self.pane.focus = PaneFocus::List;
        self.pane.gesture = false;
        self.pane.held = Some(Held {
            key: client.key,
            why: HELD_ENDED,
        });
        let name = client.name;
        match code {
            Some(0) => self.set_message(format!("the terminal of {name} ended"), Severity::Info),
            Some(code) => self.set_message(
                format!("the terminal of {name} exited with code {code}"),
                Severity::Error,
            ),
            None => self.set_message(
                format!("the terminal of {name} was ended by a signal"),
                Severity::Error,
            ),
        }
        if code != Some(0)
            && let Some(screen) = client.screen
        {
            self.pane.ended = Some(Ended { name, code, screen });
        }
        vec![Effect::PollNow]
    }

    /// Called before an effect runs: an action that can stop, wake or remove
    /// a session closes that session's client first, and the row is not
    /// attached again by itself. Returns whether the runtime closes the
    /// client.
    pub fn pane_before(&mut self, effect: &Effect) -> bool {
        let Some(id) = action_id(effect) else {
            return false;
        };
        if self.style != UiStyle::Workspace {
            return false;
        }
        if let Some(key) = self
            .sessions
            .iter()
            .find(|m| m.session.id.as_deref() == Some(id))
            .and_then(|m| m.session.key())
        {
            self.pane.held = Some(Held {
                key: key.to_owned(),
                why: HELD_ACTION,
            });
        }
        if self.pane.client.as_ref().is_some_and(|c| c.id == id) {
            self.pane.client = None;
            self.pane.focus = PaneFocus::List;
            self.pane.gesture = false;
            return true;
        }
        false
    }

    /// The checked attach of `Enter` opens in the pane instead of taking the
    /// terminal, when it is the one `Enter` asked for.
    pub(super) fn pane_take_attach(&mut self, request: &AttachRequest) -> Option<Effect> {
        if !self.pane_active() || self.pane.wake.as_deref() != Some(request.id.as_str()) {
            return None;
        }
        self.pane.wake = None;
        if let Some(note) = self.attach_note.take() {
            self.set_message(note, Severity::Warning);
        }
        let row = self
            .selected_session()
            .filter(|m| m.session.id.as_deref() == Some(request.id.as_str()))
            .and_then(|m| m.session.key())
            .map(|key| RowInfo {
                key: key.to_owned(),
                id: request.id.clone(),
                name: request.name.clone(),
                cwd: self
                    .selected_session()
                    .map_or_else(PathBuf::new, |m| PathBuf::from(&m.session.cwd)),
                account: request.account.clone(),
            });
        let Some(row) = row else {
            self.set_message(
                format!("{} was not attached: the selection moved", request.name),
                Severity::Info,
            );
            return Some(Effect::PollNow);
        };
        let now = Instant::now();
        let open = self.pane_start(row, now);
        self.pane.focus = PaneFocus::Terminal;
        Some(open)
    }

    // ---- Home ---------------------------------------------------------------

    /// `h`, or a click on `◆ tower`: the pane shows Home and no row is
    /// selected. Nothing closes (doc/0045). Nothing outside the workspace.
    pub(super) fn pane_go_home(&mut self) {
        if self.pane_active() {
            self.pane.home = true;
        }
    }

    /// A row is being chosen by a move of the selection. Returns whether the
    /// pane was on Home, where that move only lands: on the row chosen before
    /// it went Home, else on the first row (doc/0045).
    pub(super) fn pane_leave_home(&mut self) -> bool {
        let was_home = self.pane_home();
        self.pane.home = false;
        if was_home && !self.pane.chosen {
            let first = self
                .entries
                .iter()
                .position(|e| matches!(e, Entry::Row(_)))
                .unwrap_or(0);
            self.select(first);
        }
        self.pane.chosen = true;
        was_home
    }

    // ---- keys, paste and mouse -------------------------------------------------

    /// `Enter` on the selected row: the terminal gets the keys. A running row
    /// attaches now if it has not yet; a stopped row (or one whose account is
    /// unknown) goes through the checked attach of doc/0005, and wakes here.
    pub(super) fn pane_enter(&mut self) -> Vec<Effect> {
        if self.pane_home() {
            self.set_message("choose a session first: ↓ or a click", Severity::Info);
            return Vec::new();
        }
        let now = Instant::now();
        let Some(m) = self.selected_session() else {
            return Vec::new();
        };
        let key = m.session.key().map(str::to_owned);
        let busy = !m.session.is_attachable() || self.pane_row_busy(m);
        if busy {
            // Says why not.
            return self.request_action(Purpose::Attach);
        }
        if key.is_some() && self.pane_client_key().map(str::to_owned) == key {
            return self.pane_focus_terminal();
        }
        let live = m.session.has_live_process();
        let id = m.session.id.clone();
        match self.pane_row_info(m).filter(|_| live) {
            Some(row) => {
                let open = self.pane_start(row, now);
                let mut effects = vec![open];
                effects.extend(self.pane_focus_terminal());
                effects
            }
            None => {
                self.pane.wake = id;
                self.request_action(Purpose::Attach)
            }
        }
    }

    /// Gives the terminal the keys; `●` clears here, not on selection
    /// (doc/0045). Any gesture ends.
    pub(super) fn pane_focus_terminal(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.pane.gesture = false;
        if self.pane.focus == PaneFocus::Terminal {
            return effects;
        }
        self.pane.focus = PaneFocus::Terminal;
        if let Some(client) = &self.pane.client
            && client.screen.as_ref().is_some_and(PaneScreen::focus_events)
        {
            effects.push(Effect::PaneWrite(b"\x1b[I".to_vec()));
        }
        if let Some(key) = self.pane_client_key().map(str::to_owned)
            && self.acknowledge(&key)
        {
            effects.push(Effect::SaveSidecar);
        }
        effects
    }

    /// Gives the list the keys; the client stays. Any gesture ends.
    fn pane_focus_list(&mut self, now: Instant) -> Vec<Effect> {
        self.pane.gesture = false;
        if self.pane.focus == PaneFocus::List {
            return Vec::new();
        }
        self.pane.focus = PaneFocus::List;
        let Some(client) = &mut self.pane.client else {
            return Vec::new();
        };
        client.last_keys = now;
        if client.screen.as_ref().is_some_and(PaneScreen::focus_events) {
            vec![Effect::PaneWrite(b"\x1b[O".to_vec())]
        } else {
            Vec::new()
        }
    }

    /// A key while the terminal has them: `Ctrl+Z` and `F12` are tower's and
    /// give the list the keys; every other key goes to the child.
    pub(super) fn pane_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('z' | 'Z') if ctrl => return self.pane_focus_list(Instant::now()),
            KeyCode::F(12) => return self.pane_focus_list(Instant::now()),
            _ => {}
        }
        let application = self
            .pane_screen()
            .is_some_and(PaneScreen::application_cursor);
        let bytes = embed::encode_key(&key, application);
        if bytes.is_empty() {
            Vec::new()
        } else {
            vec![Effect::PaneWrite(bytes)]
        }
    }

    /// Pasted text. The child takes it as its bracketed-paste mode says; a
    /// window or the filter takes it as typed, as without bracketed paste
    /// (tower only asks for it in this style); the list ignores it, so no
    /// pasted letter runs a key.
    pub fn handle_paste(&mut self, text: &str) -> Vec<Effect> {
        if self.pane_has_keys() {
            let bracketed = self.pane_screen().is_some_and(PaneScreen::bracketed_paste);
            return vec![Effect::PaneWrite(embed::encode_paste(text, bracketed))];
        }
        if !self.window_open() && !self.filter.editing {
            return Vec::new();
        }
        let mut effects = Vec::new();
        for c in text.replace("\r\n", "\n").chars() {
            let code = match c {
                '\r' | '\n' => KeyCode::Enter,
                '\t' => KeyCode::Tab,
                c if c.is_control() => continue,
                c => KeyCode::Char(c),
            };
            effects.extend(self.handle_key(KeyEvent::new(code, KeyModifiers::NONE)));
        }
        effects
    }

    /// A mouse event, in this style only. A press on the pane gives it the
    /// keys, a press on the list gives them back (and selects the row under
    /// it). While the terminal has the keys and the child's mouse mode is on,
    /// events go on in SGR form with the pane's coordinates, clamped to it: a
    /// gesture that started inside goes on outside; a press outside starts
    /// nothing. The wheel over the list moves the selection like the arrows.
    /// A window takes the keys and the mouse does nothing.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        let Some(rect) = self.pane.rect.filter(|_| self.pane_active()) else {
            return Vec::new();
        };
        if self.window_open() {
            self.pane.gesture = false;
            return Vec::new();
        }
        let at = Position::new(mouse.column, mouse.row);
        let inside = rect.contains(at);
        let mut effects = Vec::new();
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            if !inside {
                effects.extend(self.pane_focus_list(Instant::now()));
                self.pane_click_list(at);
                return effects;
            }
            // Home hides the client, so the press does not reach it.
            if self.pane.client.is_some() && !self.pane_home() {
                effects.extend(self.pane_focus_terminal());
            }
        }
        if !inside && self.pane.bar.is_some_and(|bar| bar.contains(at)) {
            let delta = match mouse.kind {
                MouseEventKind::ScrollUp => -1,
                MouseEventKind::ScrollDown => 1,
                _ => 0,
            };
            if delta != 0 {
                effects.extend(self.pane_focus_list(Instant::now()));
                self.move_by(delta);
                return effects;
            }
        }
        if self.pane.focus != PaneFocus::Terminal {
            return effects;
        }
        let Some(screen) = self.pane_screen() else {
            return effects;
        };
        use vt100::MouseProtocolMode as Mode;
        let mode = screen.mouse_mode();
        let wanted = match mouse.kind {
            _ if mode == Mode::None => false,
            MouseEventKind::Down(_) => inside,
            MouseEventKind::Up(_) => self.pane.gesture && mode != Mode::Press,
            MouseEventKind::Drag(_) => {
                self.pane.gesture && matches!(mode, Mode::ButtonMotion | Mode::AnyMotion)
            }
            MouseEventKind::Moved => inside && mode == Mode::AnyMotion,
            _ => inside,
        };
        match mouse.kind {
            MouseEventKind::Down(_) => self.pane.gesture = wanted,
            MouseEventKind::Up(_) => self.pane.gesture = false,
            _ => {}
        }
        if !wanted {
            return effects;
        }
        let x = mouse.column.clamp(rect.x, rect.right() - 1) - rect.x + 1;
        let y = mouse.row.clamp(rect.y, rect.bottom() - 1) - rect.y + 1;
        if let Some(bytes) = embed::encode_mouse(mouse.kind, mouse.modifiers, x, y) {
            effects.push(Effect::PaneWrite(bytes));
        }
        effects
    }

    /// A press on the list bar: its top row (`◆ tower`) goes Home; any row of
    /// a card selects it; a group header is selected and folds or unfolds,
    /// as `Tab` does there.
    fn pane_click_list(&mut self, at: Position) {
        let Some(bar) = self.pane.bar.filter(|bar| bar.contains(at)) else {
            return;
        };
        if at.y == bar.y {
            self.pane_go_home();
            return;
        }
        let entry = self
            .pane
            .bar_rows
            .iter()
            .find(|(y, _)| *y == at.y)
            .map(|(_, entry)| *entry);
        if let Some(entry) = entry {
            self.pane.home = false;
            self.pane.chosen = true;
            self.select(entry);
            if matches!(self.entries.get(entry), Some(Entry::Header { .. })) {
                self.toggle_group();
            }
        }
    }
}

/// Whether two mouse events in a row can be taken as the later one: motion of
/// the same kind (doc/0045).
pub fn coalesces(event: &MouseEvent, next: &MouseEvent) -> bool {
    matches!(event.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
        && event.kind == next.kind
        && event.modifiers == next.modifiers
}
