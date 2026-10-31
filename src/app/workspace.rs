//! The workspace style's state (doc/0045): which row the pane follows, who
//! has the keys and when the one PTY client opens and closes. Like the rest of
//! [`App`] it does no I/O: it returns [`Effect`]s, and the runtime runs the
//! client ([`crate::embed`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use super::{
    App, AttachRequest, Effect, Entry, Filter, Keep, Overlay, Purpose, Selected, Severity, row_key,
};
use crate::config::{Account, UiStyle};
use crate::embed::{self, PaneCell, PaneScreen};
use crate::merge::MergedSession;
use crate::rename;

/// How long the selection rests on a row before the pane attaches to it, so
/// the arrows can run over rows without starting anything.
pub const ATTACH_DEBOUNCE: Duration = Duration::from_millis(150);

/// A client that has not had the keys for this long is released: `claude
/// attach` keeps a session from its idle stop, and tower must not.
pub const RELEASE_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long a session just started may take to be listed before the pane stops
/// waiting to open it.
pub const WANTED_PATIENCE: Duration = Duration::from_secs(30);

/// How far apart two presses on a cell make a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// The lines one notch of the wheel moves the pane's history.
const WHEEL_LINES: isize = 3;

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

/// The workspace's shell to start ([`Effect::PaneShellOpen`]): `$SHELL` in the
/// row's folder (doc/0037, doc/0045).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneShellOpen {
    /// Names this shell in everything reported about it.
    pub generation: u64,
    pub dir: PathBuf,
    pub rows: u16,
    pub cols: u16,
}

/// The one shell: it lives beside the Claude client and outlasts browsing.
struct PaneShell {
    /// The row it was opened on, by [`row_key`].
    key: String,
    /// The folder as shown to the user.
    folder: String,
    generation: u64,
    screen: Option<PaneScreen>,
    size: (u16, u16),
}

/// The client of the row the pane shows.
struct PaneClient {
    key: String,
    id: String,
    name: String,
    /// The account whose command attached.
    account: String,
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

/// A session tower has just started, to open in the pane once it is listed.
struct Wanted {
    request: AttachRequest,
    deadline: Instant,
    /// Whether Home showed, and the selection, when it was asked: if the user
    /// moves on, it is not taken from them.
    from: (bool, Option<Selected>),
}

/// Tower's own text selection in the pane, for a program that did not ask for
/// the mouse (doc/0045). Its cells are the pane's, whatever the view's place in
/// the history: moving the view ends it.
struct PaneSelection {
    anchor: PaneCell,
    head: PaneCell,
    /// The release has not come, so it has not been copied.
    dragging: bool,
    /// What it covers, as last read: when that changes the selection ends.
    text: String,
    /// The program it was made on, and the pane's place then.
    generation: u64,
    rect: Rect,
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
    shell: Option<PaneShell>,
    /// On the shell's row the pane shows the shell, not what the row normally
    /// shows; `!` there toggles this (doc/0045).
    shell_shown: bool,
    /// The keys are with the shell rather than the client, while the focus
    /// is on the terminal.
    focus_shell: bool,
    ended: Option<Ended>,
    held: Option<Held>,
    /// The pane's inside, as the renderer last drew it; `None` while it does
    /// not draw the workspace.
    rect: Option<Rect>,
    /// The list bar on the screen, and the list entry on each of its rows.
    bar: Option<Rect>,
    bar_rows: Vec<(u16, usize)>,
    /// The rows of the hints that more is hidden above and below, for clicks.
    bar_more: (Option<u16>, Option<u16>),
    /// Why no client can start; tower then draws the dashboard.
    failed: Option<String>,
    /// The short id whose checked attach `Enter` asked for (a stopped row, or
    /// an unknown owner): it needs the selection to stay on that row.
    wake: Option<String>,
    wanted: Option<Wanted>,
    /// A press went to the child and its release has not come.
    gesture: bool,
    selection: Option<PaneSelection>,
    /// A press tower took on a program without the mouse: a drag from here
    /// selects, a release without one was a click.
    press: Option<PaneCell>,
    /// The last such press, for a double click.
    last_press: Option<(Instant, PaneCell)>,
    /// The terminal fills the window; only while it has the keys.
    zoom: bool,
    /// `z` in the dashboard a small terminal gets: the pane comes back to zoom
    /// when the next frame has made its place (doc/0045).
    zoom_asked: bool,
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
            shell: None,
            shell_shown: false,
            focus_shell: false,
            ended: None,
            held: None,
            rect: None,
            bar: None,
            bar_rows: Vec::new(),
            bar_more: (None, None),
            failed: None,
            wake: None,
            wanted: None,
            gesture: false,
            selection: None,
            press: None,
            last_press: None,
            zoom: false,
            zoom_asked: false,
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

    /// Whether the terminal fills the window, or will in this frame (doc/0045).
    pub fn pane_wants_zoom(&self) -> bool {
        self.pane.zoom || self.pane.zoom_asked
    }

    /// Called by the renderer when the workspace does not fit in any form: a
    /// zoom asked for has no place to come back to.
    pub fn pane_zoom_gives_way(&mut self) {
        self.pane.zoom_asked = false;
    }

    /// How many lines the pane's view is above the bottom of its history.
    pub fn pane_scrolled(&self) -> usize {
        self.pane_screen().map_or(0, PaneScreen::scrolled)
    }

    /// The cells of tower's selection in the pane, row, first and last column.
    pub fn pane_selection_spans(&self) -> Vec<(u16, u16, u16)> {
        match (&self.pane.selection, self.pane_screen()) {
            (Some(sel), Some(screen)) => screen.selected(sel.anchor, sel.head).spans,
            _ => Vec::new(),
        }
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
            && (self.pane.client.is_some() || self.pane_shell_showing())
            && self.pane_active()
            && !self.window_open()
            && !self.filter.editing
    }

    /// Whether the pane shows the shell: its row is selected and `!` has not
    /// toggled it away (doc/0045).
    pub fn pane_shell_showing(&self) -> bool {
        self.pane.shell_shown
            && self.pane.shell.as_ref().is_some_and(|shell| {
                self.selected_session()
                    .is_some_and(|m| row_key(m) == shell.key)
            })
    }

    /// The shell's folder as shown, while the pane shows it.
    pub fn pane_shell_folder(&self) -> Option<&str> {
        if !self.pane_shell_showing() {
            return None;
        }
        self.pane.shell.as_ref().map(|shell| shell.folder.as_str())
    }

    /// The screen to draw: the shell's while it shows, else the client's, or a
    /// failed client's last one.
    pub fn pane_screen(&self) -> Option<&PaneScreen> {
        if self.pane_shell_showing() {
            return self.pane.shell.as_ref().and_then(|s| s.screen.as_ref());
        }
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
        if self.pane_shell_showing() {
            return None;
        }
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
        self.pane.bar_more = (None, None);
    }

    /// Set by the renderer while zoom hides the bar: nothing to click on.
    pub fn clear_pane_bar(&mut self) {
        self.pane.bar = None;
        self.pane.bar_rows.clear();
        self.pane.bar_more = (None, None);
    }

    /// Set by the renderer: the rows of the `▲` and `▼` hints shown, for clicks.
    pub fn set_pane_bar_more(&mut self, above: Option<u16>, below: Option<u16>) {
        self.pane.bar_more = (above, below);
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
            account: row.account.name.clone(),
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
            // The shell waits out a terminal too small to show it, not another
            // style or a pane that cannot start.
            let close_shell = (self.style != UiStyle::Workspace || self.pane.failed.is_some())
                && self.pane.shell.is_some();
            let pane = &mut self.pane;
            let mut effects = Vec::new();
            if close_shell {
                pane.shell = None;
                pane.shell_shown = false;
                effects.push(Effect::PaneShellClose);
            }
            pane.focus_shell = false;
            pane.focus = PaneFocus::List;
            pane.gesture = false;
            pane.selection = None;
            pane.press = None;
            pane.zoom = false;
            pane.selected = None;
            pane.since = None;
            pane.ended = None;
            pane.held = None;
            pane.wake = None;
            pane.wanted = None;
            if pane.client.take().is_some() {
                effects.push(Effect::PaneClose);
            }
            return effects;
        }
        // The pane has the logs panel's place, and more (doc/0045).
        self.logs = None;
        let mut effects = self.pane_open_wanted(now);
        effects.extend(self.pane_shell_tick());
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
        if std::mem::take(&mut self.pane.zoom_asked) {
            effects.extend(self.pane_zoom_on());
        }
        // Zoom lasts while the terminal has the keys, however they left.
        if self.pane.focus != PaneFocus::Terminal {
            self.pane.zoom = false;
        }
        self.pane_selection_tick();
        self.pane_history_tick();
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
        // picks another row or the release below (doc/0045). The same while
        // the pane shows the shell, which needs no client.
        if rested
            && !self.pane_home()
            && !self.pane_shell_showing()
            && self.pane_client_key().map(str::to_owned) != key
        {
            match self.pane_auto_row() {
                Some(row) => effects.push(self.pane_start(row, now)),
                None => {
                    if self.pane.client.take().is_some() {
                        effects.push(Effect::PaneClose);
                    }
                }
            }
        }
        let client_has_keys = self.pane.focus == PaneFocus::Terminal && !self.pane.focus_shell;
        if let Some(client) = &mut self.pane.client {
            if client_has_keys {
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
        if client_has_keys
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
        if self.pane_active() && self.settings.is_none() && self.pane_due() {
            return Some(Duration::ZERO);
        }
        if !self.pane_active()
            || self.pane_home()
            || self.pane_shell_showing()
            || self.pane_client_key() == self.pane.selected.as_deref()
        {
            return None;
        }
        let rested_at = self.pane.since? + ATTACH_DEBOUNCE;
        Some(rested_at.saturating_duration_since(now))
    }

    /// The frame just drawn made [`App::pane_tick`] something to do: the pane
    /// has a new size, or a zoom waits for its place. A key that zooms does not
    /// wait for the next turn of the loop.
    fn pane_due(&self) -> bool {
        let Some(rect) = self.pane.rect else {
            return false;
        };
        let size = (rect.height, rect.width);
        self.pane.zoom_asked
            || self.pane.client.as_ref().is_some_and(|c| c.size != size)
            || self.pane.shell.as_ref().is_some_and(|s| s.size != size)
    }

    // ---- what the runtime reports ---------------------------------------------

    /// [`Effect::PaneOpen`] has started: the client's screen.
    pub fn pane_opened(&mut self, generation: u64, screen: PaneScreen) {
        if let Some(shell) = &mut self.pane.shell
            && shell.generation == generation
        {
            shell.screen = Some(screen);
            return;
        }
        if let Some(client) = &mut self.pane.client
            && client.generation == generation
        {
            client.screen = Some(screen);
        }
    }

    /// No client could be started: tower falls back to the dashboard until it
    /// restarts (doc/0045).
    pub fn pane_start_failed(&mut self, generation: u64, error: &str) {
        if let Some(shell) = self.pane_take_shell(generation) {
            self.set_message(
                format!("no shell started in {}: {error}", shell.folder),
                Severity::Error,
            );
            return;
        }
        if self
            .pane
            .client
            .as_ref()
            .is_some_and(|c| c.generation == generation)
        {
            self.pane_drop_rename();
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
        if let Some(shell) = self.pane_take_shell(generation) {
            let folder = shell.folder;
            match code {
                Some(0) => {
                    self.set_message(format!("back from the shell in {folder}"), Severity::Info);
                }
                Some(code) => self.set_message(
                    format!("the shell in {folder} exited with code {code}"),
                    Severity::Warning,
                ),
                None => self.set_message(
                    format!("the shell in {folder} was ended by a signal"),
                    Severity::Warning,
                ),
            }
            return vec![Effect::PollNow];
        }
        let Some(client) = self.pane.client.take_if(|c| c.generation == generation) else {
            return Vec::new();
        };
        self.pane.focus = PaneFocus::List;
        self.pane.gesture = false;
        self.pane.held = Some(Held {
            key: client.key,
            why: HELD_ENDED,
        });
        // A rename through this client (doc/0027) is checked on a clean end.
        let rename = code == Some(0) && self.pane_check_rename(&client.id, &client.account);
        if !rename
            && self
                .rename_attach
                .as_ref()
                .is_some_and(|i| i.id == client.id)
        {
            self.rename_attach = None;
        }
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
        vec![if rename {
            Effect::PollForRename
        } else {
            Effect::PollNow
        }]
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
            self.pane_drop_rename();
            self.pane.client = None;
            self.pane.focus = PaneFocus::List;
            self.pane.gesture = false;
            return true;
        }
        false
    }

    /// While the workspace is drawn every attach opens in the pane, with the
    /// keys, and selects its row: `Enter` on a stopped row, `o`, start-and-attach
    /// and the rename of a running session (doc/0045). A session not listed
    /// yet opens when it is ([`App::pane_open_wanted`]).
    pub(super) fn pane_take_attach(&mut self, request: &AttachRequest) -> Option<Vec<Effect>> {
        if !self.pane_active() {
            return None;
        }
        let from_enter = self.pane.wake.take_if(|id| *id == request.id).is_some();
        if let Some(note) = self.attach_note.take() {
            self.set_message(note, Severity::Warning);
        }
        let now = Instant::now();
        let listed = self.pane_listed(&request.id);
        let Some(key) = listed else {
            self.pane.wanted = Some(Wanted {
                request: request.clone(),
                deadline: now + WANTED_PATIENCE,
                from: (self.pane_home(), self.selected_key.clone()),
            });
            return Some(vec![Effect::PollNow]);
        };
        let moved = self
            .selected_session()
            .is_none_or(|m| m.session.id.as_deref() != Some(request.id.as_str()));
        if from_enter && moved {
            self.set_message(
                format!("{} was not attached: the selection moved", request.name),
                Severity::Info,
            );
            return Some(vec![Effect::PollNow]);
        }
        Some(self.pane_open_row(&key, request, now))
    }

    /// The session key of the listed session with the short id `id`.
    fn pane_listed(&self, id: &str) -> Option<String> {
        self.sessions
            .iter()
            .find(|m| m.session.id.as_deref() == Some(id))
            .and_then(|m| m.session.key())
            .map(str::to_owned)
    }

    /// Selects the row, attaches it unless its client is there already, and
    /// gives its terminal the keys.
    fn pane_open_row(&mut self, key: &str, request: &AttachRequest, now: Instant) -> Vec<Effect> {
        self.pane_select_row(key);
        // The selection moved on purpose: `pane_tick` must not take the keys
        // back for it.
        self.pane.selected = Some(key.to_owned());
        self.pane.since = Some(now);
        self.pane.wanted = None;
        self.pane.gesture = false;
        self.pane.zoom = false;
        let mut effects = Vec::new();
        if self.pane_client_key() != Some(key) {
            let cwd = self
                .sessions
                .iter()
                .find(|m| m.session.key() == Some(key))
                .map_or_else(PathBuf::new, |m| PathBuf::from(&m.session.cwd));
            effects.push(self.pane_start(
                RowInfo {
                    key: key.to_owned(),
                    id: request.id.clone(),
                    name: request.name.clone(),
                    cwd,
                    account: request.account.clone(),
                },
                now,
            ));
        }
        self.pane.ended = None;
        self.pane.held = None;
        // Claude's terminal was asked for: it shows instead of this row's shell.
        if self
            .pane
            .shell
            .as_ref()
            .is_some_and(|shell| shell.key == key)
        {
            self.pane.shell_shown = false;
        }
        // From the list, so an existing client hears that it has the focus.
        self.pane.focus = PaneFocus::List;
        effects.extend(self.pane_focus_terminal());
        if let Some(intent) = self.rename_attach.as_ref().filter(|i| i.id == request.id) {
            let text = format!(
                "type {} here; Ctrl+Z or F12 returns and tower checks the name",
                rename::rename_prompt(&intent.new)
            );
            self.set_message(text, Severity::Info);
        }
        effects
    }

    /// Selects the row with session key `key`, opening its group if it is
    /// folded and clearing a filter that hides it.
    fn pane_select_row(&mut self, key: &str) {
        self.pane.home = false;
        self.pane.chosen = true;
        let shown = self
            .sessions
            .iter()
            .any(|m| m.session.key() == Some(key) && self.matches_filter(m));
        if !shown {
            self.filter = Filter::default();
        }
        self.selected_key = Some(Selected::Row(key.to_owned()));
        self.rebuild_view(Keep::Same);
        // A folded group gives its header: open it and select the row itself.
        if self.selected_header().is_some() {
            self.toggle_group();
            self.selected_key = Some(Selected::Row(key.to_owned()));
            self.rebuild_view(Keep::Same);
        }
    }

    /// Opens the session waiting to be listed, once it is and no window or
    /// filter line has the keys. It lets go when the user moved the selection
    /// meanwhile, or after [`WANTED_PATIENCE`].
    fn pane_open_wanted(&mut self, now: Instant) -> Vec<Effect> {
        let Some(wanted) = &self.pane.wanted else {
            return Vec::new();
        };
        let name = wanted.request.name.clone();
        // From Home only leaving it counts: what is selected is hidden there.
        let moved = match &wanted.from {
            (true, _) => !self.pane_home(),
            (false, None) => false,
            (false, key) => *key != self.selected_key,
        };
        let late = now >= wanted.deadline;
        if moved || late {
            self.pane.wanted = None;
            let why = if moved {
                "the selection moved"
            } else {
                "it is not listed yet"
            };
            self.set_message(
                format!("{name} was not opened here: {why}; select it to open it"),
                Severity::Info,
            );
            return Vec::new();
        }
        if self.window_open() || self.filter.editing {
            return Vec::new();
        }
        let Some(key) = self.pane_listed(&wanted.request.id) else {
            return Vec::new();
        };
        let Some(wanted) = self.pane.wanted.take() else {
            return Vec::new();
        };
        self.pane_open_row(&key, &wanted.request, now)
    }

    // ---- the shell (doc/0037, doc/0045) ------------------------------------------

    /// `!` on the selected row, whose folder is `dir`: toggles the pane between
    /// its shell and what the row normally shows, or opens a shell here, ending
    /// the one on another row. Tower does nothing to the session itself.
    pub(super) fn pane_shell(&mut self, dir: PathBuf, folder: String) -> Vec<Effect> {
        let Some(key) = self.selected_session().map(row_key) else {
            return Vec::new();
        };
        if self.pane.shell.as_ref().is_some_and(|s| s.key == key) {
            self.pane.shell_shown = !self.pane.shell_shown;
            return if self.pane.shell_shown {
                self.pane_focus_terminal()
            } else {
                Vec::new()
            };
        }
        let mut effects = Vec::new();
        if let Some(old) = self.pane.shell.take() {
            self.set_message(
                format!("closed the shell in {}; a new one in {folder}", old.folder),
                Severity::Info,
            );
            effects.push(Effect::PaneShellClose);
        }
        self.pane.generations += 1;
        let generation = self.pane.generations;
        let (rows, cols) = self.pane.rect.map_or((1, 1), |r| (r.height, r.width));
        self.pane.shell = Some(PaneShell {
            key,
            folder,
            generation,
            screen: None,
            size: (rows, cols),
        });
        self.pane.shell_shown = true;
        effects.push(Effect::PaneShellOpen(PaneShellOpen {
            generation,
            dir,
            rows,
            cols,
        }));
        effects.extend(self.pane_focus_terminal());
        effects
    }

    /// Forgets the shell of `generation` and gives the list the keys if it had
    /// them; the pane goes back to what the row normally shows.
    fn pane_take_shell(&mut self, generation: u64) -> Option<PaneShell> {
        let shell = self
            .pane
            .shell
            .take_if(|shell| shell.generation == generation)?;
        self.pane.shell_shown = false;
        if self.pane.focus_shell {
            self.pane.focus_shell = false;
            self.pane.focus = PaneFocus::List;
            self.pane.gesture = false;
        }
        Some(shell)
    }

    /// The shell's folder is gone: nothing started (doc/0037).
    pub fn pane_shell_folder_gone(&mut self, generation: u64) {
        if let Some(shell) = self.pane_take_shell(generation) {
            self.set_message(
                format!("{} no longer exists; no shell started", shell.folder),
                Severity::Warning,
            );
        }
    }

    /// The shell follows the pane's size, and ends with its row.
    fn pane_shell_tick(&mut self) -> Vec<Effect> {
        let Some(shell) = &self.pane.shell else {
            return Vec::new();
        };
        if !self.sessions.iter().any(|m| row_key(m) == shell.key) {
            let (generation, folder) = (shell.generation, shell.folder.clone());
            self.pane_take_shell(generation);
            self.set_message(
                format!("closed the shell in {folder}: its session is gone"),
                Severity::Info,
            );
            return vec![Effect::PaneShellClose];
        }
        let Some(rect) = self.pane.rect else {
            return Vec::new();
        };
        let size = (rect.height, rect.width);
        match &mut self.pane.shell {
            Some(shell) if shell.size != size => {
                shell.size = size;
                vec![Effect::PaneShellResize {
                    rows: size.0,
                    cols: size.1,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// `bytes` for the child that has the keys.
    fn pane_write(&self, bytes: Vec<u8>) -> Effect {
        if self.pane.focus_shell {
            Effect::PaneShellWrite(bytes)
        } else {
            Effect::PaneWrite(bytes)
        }
    }

    // ---- a rename through the pane's client (doc/0027) ------------------------

    /// Whether a rename through an attach goes on in the pane's client.
    pub(super) fn pane_renames(&self) -> bool {
        match (&self.rename_attach, &self.pane.client) {
            (Some(intent), Some(client)) => intent.id == client.id,
            _ => false,
        }
    }

    fn pane_drop_rename(&mut self) {
        if self.pane_renames() {
            self.rename_attach = None;
        }
    }

    /// The keys left the client of `id`, or it ended: the name is compared once
    /// a poll started now has listed it. Returns whether a check is due.
    fn pane_check_rename(&mut self, id: &str, account: &str) -> bool {
        match self.rename_attach.as_mut() {
            Some(intent) if intent.id == id && intent.account.is_none() => {
                intent.account = Some(account.to_owned());
                intent.cycle = None;
                true
            }
            _ => false,
        }
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
        if self.pane_shell_showing() {
            return self.pane_focus_terminal();
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
        self.pane.focus_shell = self.pane_shell_showing();
        if self.pane.focus_shell {
            // The shell is no answer to what a row asks: `●` stays.
            if self.pane_screen().is_some_and(PaneScreen::focus_events) {
                effects.push(Effect::PaneShellWrite(b"\x1b[I".to_vec()));
            }
            return effects;
        }
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
        self.pane.zoom = false;
        self.pane_clear_selection();
        if self.pane.focus == PaneFocus::List {
            return Vec::new();
        }
        self.pane.focus = PaneFocus::List;
        if std::mem::take(&mut self.pane.focus_shell) {
            let events = self
                .pane
                .shell
                .as_ref()
                .and_then(|shell| shell.screen.as_ref())
                .is_some_and(PaneScreen::focus_events);
            return if events {
                vec![Effect::PaneShellWrite(b"\x1b[O".to_vec())]
            } else {
                Vec::new()
            };
        }
        let Some(client) = &mut self.pane.client else {
            return Vec::new();
        };
        client.last_keys = now;
        let mut effects = Vec::new();
        if client.screen.as_ref().is_some_and(PaneScreen::focus_events) {
            effects.push(Effect::PaneWrite(b"\x1b[O".to_vec()));
        }
        let (id, account) = (client.id.clone(), client.account.clone());
        if self.pane_check_rename(&id, &account) {
            effects.push(Effect::PollForRename);
        }
        effects
    }

    /// A key while the terminal has them: `Ctrl+Z` and `F12` are tower's and
    /// give the list the keys, `F2` zooms, `Esc` ends a selection and
    /// `Shift+PgUp` / `Shift+PgDn` move the history of a program that can
    /// (doc/0045). Every other key goes to the child and brings the view back
    /// to the bottom.
    pub(super) fn pane_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('z' | 'Z') if ctrl => return self.pane_focus_list(Instant::now()),
            KeyCode::F(12) => return self.pane_focus_list(Instant::now()),
            KeyCode::F(2) => {
                self.pane.zoom = !self.pane.zoom;
                return Vec::new();
            }
            KeyCode::Esc if self.pane.selection.is_some() => {
                self.pane_clear_selection();
                return Vec::new();
            }
            KeyCode::PageUp | KeyCode::PageDown
                if key.modifiers == KeyModifiers::SHIFT && self.pane_scrollable() =>
            {
                let page = self
                    .pane
                    .rect
                    .map_or(1, |r| r.height.saturating_sub(1))
                    .max(1);
                let page = isize::try_from(page).unwrap_or(1);
                self.pane_scroll(if key.code == KeyCode::PageUp {
                    page
                } else {
                    -page
                });
                return Vec::new();
            }
            _ => {}
        }
        let application = self
            .pane_screen()
            .is_some_and(PaneScreen::application_cursor);
        let bytes = embed::encode_key(&key, application);
        if bytes.is_empty() {
            return Vec::new();
        }
        self.pane_to_bottom();
        vec![self.pane_write(bytes)]
    }

    // ---- zoom (doc/0045) ----------------------------------------------------------

    /// `z` or `F2` with the list holding the keys: the terminal of the selected
    /// row fills the window and gets them. From the dashboard of a small
    /// terminal too, since zoom needs no room for the bar.
    pub(super) fn pane_zoom_key(&mut self) -> Vec<Effect> {
        if self.style != UiStyle::Workspace || self.pane.failed.is_some() {
            return Vec::new();
        }
        if self.pane_active() {
            return self.pane_zoom_on();
        }
        if let Some(why) = self.pane_zoom_refused() {
            self.set_message(why, Severity::Info);
            return Vec::new();
        }
        // The fallback's selection is a real one: Home does not stand for it.
        self.pane.home = false;
        self.pane.chosen = true;
        self.pane.zoom_asked = true;
        Vec::new()
    }

    /// Why the selected row has no terminal to zoom.
    fn pane_zoom_refused(&self) -> Option<&'static str> {
        if self.pane_shell_showing() {
            return None;
        }
        let Some(m) = self.selected_session() else {
            return Some("choose a session to zoom its terminal");
        };
        let attached = m.session.key().is_some() && self.pane_client_key() == m.session.key();
        let startable = m.session.is_attachable()
            && m.session.has_live_process()
            && !self.pane_row_busy(m)
            && self.pane_row_info(m).is_some();
        if attached || startable {
            None
        } else {
            Some("this row has no terminal to zoom; Enter opens one if it can")
        }
    }

    fn pane_zoom_on(&mut self) -> Vec<Effect> {
        if let Some(why) = self.pane_zoom_refused() {
            self.set_message(why, Severity::Info);
            return Vec::new();
        }
        let effects = self.pane_enter();
        self.pane.zoom = self.pane.focus == PaneFocus::Terminal;
        effects
    }

    // ---- the history and tower's selection (doc/0045) ---------------------------------

    /// Whether tower scrolls what the pane shows.
    fn pane_scrollable(&self) -> bool {
        !self.pane_home() && self.pane_screen().is_some_and(PaneScreen::scrollable)
    }

    /// Moves the view; a selection ends with what it was on.
    fn pane_scroll(&mut self, lines: isize) {
        if self
            .pane_screen()
            .is_some_and(|screen| screen.scroll(lines))
        {
            self.pane_clear_selection();
        }
    }

    /// What goes to the program brings the view back to the bottom.
    fn pane_to_bottom(&mut self) {
        if self.pane_screen().is_some_and(PaneScreen::to_bottom) {
            self.pane_clear_selection();
        }
    }

    fn pane_clear_selection(&mut self) {
        self.pane.selection = None;
        self.pane.press = None;
    }

    /// The program the pane shows, by its generation.
    fn pane_shown_generation(&self) -> Option<u64> {
        if self.pane_shell_showing() {
            return self.pane.shell.as_ref().map(|shell| shell.generation);
        }
        self.pane.client.as_ref().map(|client| client.generation)
    }

    /// A selection ends when what is under it changes, when the keys or the
    /// program shown change, when the pane moves or resizes, and when the
    /// program asks for the mouse.
    fn pane_selection_tick(&mut self) {
        let Some(sel) = &self.pane.selection else {
            return;
        };
        let same = self.pane_has_keys()
            && self.pane.rect == Some(sel.rect)
            && self.pane_shown_generation() == Some(sel.generation)
            && self.pane_screen().is_some_and(|screen| {
                screen.mouse_mode() == vt100::MouseProtocolMode::None
                    && screen.selected(sel.anchor, sel.head).text == sel.text
            });
        if !same {
            self.pane_clear_selection();
        }
    }

    /// A program that came to ask for the mouse scrolls itself: no view stays
    /// up there with nothing to bring it down.
    fn pane_history_tick(&self) {
        if let Some(screen) = self.pane.shell.as_ref().and_then(|s| s.screen.as_ref())
            && !screen.scrollable()
        {
            screen.to_bottom();
        }
    }

    /// Starts or moves a selection from `anchor` to `head`.
    fn pane_set_selection(&mut self, anchor: PaneCell, head: PaneCell, rect: Rect) {
        let (Some(generation), Some(screen)) = (self.pane_shown_generation(), self.pane_screen())
        else {
            return;
        };
        let text = screen.selected(anchor, head).text;
        self.pane.selection = Some(PaneSelection {
            anchor,
            head,
            dragging: true,
            text,
            generation,
            rect,
        });
    }

    /// The left button over a program that did not ask for the mouse: a drag
    /// selects, confined to the pane; a click ends a selection; a double click
    /// takes the word; the release copies what is highlighted.
    fn pane_select_mouse(&mut self, mouse: MouseEvent, rect: Rect, now: Instant) -> Vec<Effect> {
        let cell = (
            mouse.row.clamp(rect.y, rect.bottom() - 1) - rect.y,
            mouse.column.clamp(rect.x, rect.right() - 1) - rect.x,
        );
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.pane_clear_selection();
                let double = self.pane.last_press.take().is_some_and(|(at, on)| {
                    on == cell && now.saturating_duration_since(at) < DOUBLE_CLICK
                });
                let word = self.pane_screen().and_then(|s| s.word_at(cell));
                match word.filter(|_| double) {
                    Some((first, last)) => {
                        self.pane.press = Some(first);
                        self.pane_set_selection(first, last, rect);
                    }
                    None => {
                        self.pane.press = Some(cell);
                        self.pane.last_press = (!double).then_some((now, cell));
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(anchor) = self.pane.press {
                    self.pane_set_selection(anchor, cell, rect);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.pane.press = None;
                return self.pane_copy_selection();
            }
            _ => {}
        }
        Vec::new()
    }

    /// The release of a drag: what is highlighted goes to the real terminal's
    /// clipboard with `OSC 52`, and the status line says how much. The highlight
    /// stays until the selection ends.
    fn pane_copy_selection(&mut self) -> Vec<Effect> {
        let Some(sel) = self.pane.selection.as_mut().filter(|sel| sel.dragging) else {
            return Vec::new();
        };
        sel.dragging = false;
        if sel.text.is_empty() {
            self.pane_clear_selection();
            return Vec::new();
        }
        let chars = sel.text.chars().count();
        let Some(write) = embed::clipboard_write(&sel.text) else {
            self.set_message("the selection is too long to copy", Severity::Warning);
            return Vec::new();
        };
        let plural = if chars == 1 { "" } else { "s" };
        self.set_message(format!("copied {chars} character{plural}"), Severity::Info);
        vec![Effect::PaneCopy(write)]
    }

    /// Pasted text. The child takes it as its bracketed-paste mode says; a
    /// window or the filter takes it as typed, as without bracketed paste
    /// (tower only asks for it in this style); the list ignores it, so no
    /// pasted letter runs a key.
    pub fn handle_paste(&mut self, text: &str) -> Vec<Effect> {
        if self.pane_has_keys() {
            let bracketed = self.pane_screen().is_some_and(PaneScreen::bracketed_paste);
            self.pane_to_bottom();
            return vec![self.pane_write(embed::encode_paste(text, bracketed))];
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
    /// A program that did not ask for the mouse gets tower's own selection and,
    /// in the shell, the wheel moves its history.
    /// A window takes the keys and the mouse does nothing.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        self.handle_mouse_at(mouse, Instant::now())
    }

    /// [`App::handle_mouse`] at the time `now`, which a double click looks at.
    pub fn handle_mouse_at(&mut self, mouse: MouseEvent, now: Instant) -> Vec<Effect> {
        let Some(rect) = self.pane.rect.filter(|_| self.pane_active()) else {
            return Vec::new();
        };
        if self.window_open() {
            self.pane.gesture = false;
            self.pane_clear_selection();
            return Vec::new();
        }
        let at = Position::new(mouse.column, mouse.row);
        let inside = rect.contains(at);
        let mut effects = Vec::new();
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            // Zoomed, the footer's row is all there is outside: it takes nothing.
            if !inside && self.pane.zoom {
                return effects;
            }
            if !inside {
                effects.extend(self.pane_focus_list(Instant::now()));
                self.pane_click_list(at);
                return effects;
            }
            // Home hides the client, so the press does not reach it.
            if (self.pane.client.is_some() || self.pane_shell_showing()) && !self.pane_home() {
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
        // The wheel over the pane moves a history, whoever has the keys.
        let wheel = match mouse.kind {
            MouseEventKind::ScrollUp => WHEEL_LINES,
            MouseEventKind::ScrollDown => -WHEEL_LINES,
            _ => 0,
        };
        if inside && wheel != 0 && self.pane_scrollable() {
            self.pane_scroll(wheel);
            return effects;
        }
        if self.pane.focus != PaneFocus::Terminal {
            return effects;
        }
        let Some(screen) = self.pane_screen() else {
            return effects;
        };
        use vt100::MouseProtocolMode as Mode;
        let mode = screen.mouse_mode();
        if mode == Mode::None {
            self.pane.gesture = false;
            effects.extend(self.pane_select_mouse(mouse, rect, now));
            return effects;
        }
        let wanted = match mouse.kind {
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
            effects.push(self.pane_write(bytes));
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
        // The hints move a page, as the page keys do.
        let page = self.page_size.max(1) as isize;
        if self.pane.bar_more.0 == Some(at.y) {
            self.move_by(-page);
            return;
        }
        if self.pane.bar_more.1 == Some(at.y) {
            self.move_by(page);
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
