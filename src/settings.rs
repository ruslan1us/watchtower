//! The Settings window (doc/0024): tower's own settings edited in place,
//! saved to `config.toml` through [`crate::config_edit`] (doc/0022).
//!
//! [`SettingsWindow`] is pure state: the [`EditSession`] with the unsaved
//! changes, the selected row, and what the window is doing ([`Mode`]): a
//! pick list, an inline input, the list editor of project folders, or one
//! of its questions. Keys go in ([`SettingsWindow::handle_key`]) and a
//! [`SettingsAction`] comes out for the app to carry out (save on a worker
//! thread, close, open the editor, quit). The folder checks and the `Tab`
//! completion of the list editor read the disk, so they are a
//! [`FolderJob`] the runtime runs on a worker thread, one at a time.
//!
//! Live preview: while the window is open the app draws with
//! [`SettingsWindow::preview`] (the style and theme under the pick list's
//! cursor, else the unsaved value) over [`SettingsWindow::base`] (the
//! running config's), and falls back to the base when the window closes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{UiStyle, UiTheme};
use crate::config_edit::{
    ChangeSummary, Conflict, EditSession, SaveError, SaveOutcome, Section, SettingKey, SettingKind,
    SettingValue,
};
use crate::dialog::{KeyUse, LineInput};
use crate::paths;
use crate::projects;

/// Rows a page key moves.
const PAGE: usize = 8;

/// The pick list's entry that switches to typing a value (a full model
/// name).
pub const OTHER: &str = "other…";

/// A row the selection can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// An editable setting.
    Setting(SettingKey),
    /// A configured account, by index; read-only.
    Account(usize),
}

impl Row {
    /// The section the row is shown in.
    pub fn section(self) -> Section {
        match self {
            Row::Setting(key) => key.section(),
            Row::Account(_) => Section::Accounts,
        }
    }
}

/// The pick list of a choice setting (`Enter` on it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub key: SettingKey,
    /// The offered values, then [`OTHER`] when any word is accepted.
    pub items: Vec<String>,
    pub cursor: usize,
}

impl Pick {
    /// The value under the cursor; `None` on [`OTHER`].
    pub fn value(&self) -> Option<&str> {
        self.items
            .get(self.cursor)
            .map(String::as_str)
            .filter(|v| *v != OTHER)
    }
}

/// The inline input of a text or number setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub key: SettingKey,
    pub input: LineInput,
    /// Why the text is not a valid value, checked as it is typed.
    pub error: Option<String>,
}

/// The folder being added or edited in the list editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInput {
    /// The entry edited; `None` while adding.
    pub index: Option<usize>,
    pub input: LineInput,
    pub error: Option<String>,
}

/// The list editor of `project_roots` or `extra_projects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEditor {
    pub key: SettingKey,
    /// The folders as written (`~` kept).
    pub items: Vec<String>,
    pub selected: usize,
    pub input: Option<ListInput>,
}

/// What the window is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Moving over the rows.
    Browse,
    /// A choice's pick list.
    Pick(Pick),
    /// An inline input on the selected row.
    Input(Input),
    /// The list editor of a folder list.
    List(ListEditor),
    /// `Esc` with unsaved changes: save and close, discard, keep editing.
    Unsaved,
    /// `e` with unsaved changes: save, discard or cancel before the editor.
    EditPrompt,
    /// The save found keys changed in the file meanwhile.
    Conflict(Vec<Conflict>),
    /// Turning `allow_cross_group_handoff` on (doc/0018).
    CrossGroup,
    /// `Ctrl+C` with unsaved changes.
    QuitUnsaved,
    /// The window's keys.
    Help,
}

/// What follows a save that is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSave {
    /// Close the window (`y` in the unsaved question).
    Close,
    /// Open the file in the editor (`y` in the `e` question).
    Edit,
}

/// What the app does after a key or a save result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAction {
    None,
    /// Close the window; the preview reverts to the running config.
    Close,
    /// Save the changes on a worker thread ([`EditSession::save_job`]).
    Save {
        force: bool,
    },
    /// Close the window and open `config.toml` in `$VISUAL` / `$EDITOR`.
    Edit,
    /// Quit tower.
    Quit,
}

/// How the status line shows a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

/// The window's own status line message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
}

/// What a folder of the list editor is, from a [`FolderJob`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderInfo {
    /// Being looked at.
    Checking,
    /// Not there: the scan skips it.
    Missing,
    /// There, but not a folder.
    NotFolder,
    /// A folder: whether it is a git clone itself, and how many git clones
    /// it holds one level down (a project root's scan).
    Folder { git: bool, clones: usize },
}

/// The disk work of the list editor, run on a worker thread: looks at
/// folders and completes a folder name. Cheap (a `stat` and one `read_dir`
/// each), but a hung mount must never block the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderJob {
    /// Folders to look at, as written (`~` allowed).
    pub probe: Vec<String>,
    /// A folder name to complete (`Tab`).
    pub complete: Option<String>,
    pub home: PathBuf,
}

/// A finished [`FolderJob`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderResult {
    pub probed: Vec<(String, FolderInfo)>,
    pub completed: Option<Completion>,
}

/// The `Tab` completion of `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The input text the completion is for.
    pub text: String,
    /// The completed text, when it got longer.
    pub result: Option<String>,
    /// The folder names that matched.
    pub matches: Vec<String>,
}

impl FolderJob {
    /// Looks at the folders and completes the name.
    pub fn run(&self) -> FolderResult {
        FolderResult {
            probed: self
                .probe
                .iter()
                .map(|p| (p.clone(), probe_folder(p, &self.home)))
                .collect(),
            completed: self
                .complete
                .as_ref()
                .map(|text| complete_folder(text, &self.home)),
        }
    }
}

/// What the folder `path` (`~` expanded against `home`) is.
pub fn probe_folder(path: &str, home: &Path) -> FolderInfo {
    let dir = paths::expand_tilde(path.trim(), home);
    match std::fs::metadata(&dir) {
        Err(_) => FolderInfo::Missing,
        Ok(meta) if !meta.is_dir() => FolderInfo::NotFolder,
        Ok(_) => FolderInfo::Folder {
            git: projects::is_git_repo(&dir),
            clones: projects::scan_root(&dir).0.len(),
        },
    }
}

/// Completes the last folder name of `text` (`~/wo` → `~/workspace/`)
/// from the folders on disk: a single match gets a trailing `/`, several
/// their longest common start. Hidden folders only when the name starts
/// with `.`.
pub fn complete_folder(text: &str, home: &Path) -> Completion {
    let none = |matches| Completion {
        text: text.to_owned(),
        result: None,
        matches,
    };
    if text == "~" {
        return Completion {
            text: text.to_owned(),
            result: Some("~/".to_owned()),
            matches: Vec::new(),
        };
    }
    let Some(slash) = text.rfind('/') else {
        return none(Vec::new());
    };
    let (dir_part, prefix) = text.split_at(slash + 1);
    let dir = paths::expand_tilde(dir_part, home);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return none(Vec::new());
    };
    let mut matches: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let wanted =
                name.starts_with(prefix) && (!name.starts_with('.') || prefix.starts_with('.'));
            (wanted && e.path().is_dir()).then_some(name)
        })
        .collect();
    matches.sort();
    let result = match matches.as_slice() {
        [] => None,
        [one] => Some(format!("{dir_part}{one}/")),
        [first, rest @ ..] => {
            let mut common: String = first.clone();
            for m in rest {
                let n = common
                    .chars()
                    .zip(m.chars())
                    .take_while(|(a, b)| a == b)
                    .count();
                common = common.chars().take(n).collect();
            }
            (common.chars().count() > prefix.chars().count()).then(|| format!("{dir_part}{common}"))
        }
    };
    Completion {
        text: text.to_owned(),
        result,
        matches,
    }
}

/// A folder as stored: trailing slashes dropped (`~/src/` → `~/src`).
pub fn clean_folder(text: &str) -> String {
    let trimmed = text.trim();
    let cut = trimmed.trim_end_matches('/');
    if cut.is_empty() && !trimmed.is_empty() {
        "/".to_owned()
    } else {
        cut.to_owned()
    }
}

/// A value as TOML shows it: `"warm"`, `2`, `true`, `["~/a", "~/b"]`.
pub fn toml_text(value: &SettingValue) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    match value {
        SettingValue::Text(s) => quote(s),
        SettingValue::Bool(b) => b.to_string(),
        SettingValue::Int(n) => n.to_string(),
        SettingValue::Paths(p) => {
            let items: Vec<String> = p.iter().map(|s| quote(s)).collect();
            format!("[{}]", items.join(", "))
        }
    }
}

/// How the Settings window shows an empty hand-off prompt (doc/0033).
pub const NO_PROMPT: &str = "(none — waits for you)";

/// A value as a row shows it: `on`/`off`, `2 s`, the paths joined; a
/// logs-follow interval of 0 is `off`; an empty hand-off prompt is
/// [`NO_PROMPT`].
pub fn shown_value(key: SettingKey, value: &SettingValue) -> String {
    match (key, value) {
        (_, SettingValue::Bool(true)) => "on".to_owned(),
        (_, SettingValue::Bool(false)) => "off".to_owned(),
        (SettingKey::LogsFollowSecs, SettingValue::Int(0)) => "off".to_owned(),
        (SettingKey::PollIntervalSecs | SettingKey::LogsFollowSecs, SettingValue::Int(n)) => {
            format!("{n} s")
        }
        (SettingKey::HandoffPrompt, SettingValue::Text(s)) if s.trim().is_empty() => {
            NO_PROMPT.to_owned()
        }
        (SettingKey::HandoffPrompt, SettingValue::Text(s)) => format!("\"{s}\""),
        (_, v) => v.display(),
    }
}

/// One unsaved change for the questions: `neutral  →  warm`,
/// `+ ~/src/oss`, `high  →  default (removed from the file)`.
pub fn change_text(change: &ChangeSummary) -> String {
    let key = change.key;
    if let (SettingValue::Paths(from), SettingValue::Paths(to), false) =
        (&change.from, &change.to, change.reset)
    {
        let mut parts: Vec<String> = to
            .iter()
            .filter(|p| !from.contains(p))
            .map(|p| format!("+ {p}"))
            .collect();
        parts.extend(
            from.iter()
                .filter(|p| !to.contains(p))
                .map(|p| format!("− {p}")),
        );
        if parts.is_empty() {
            parts.push("new order".to_owned());
        }
        return parts.join(", ");
    }
    let from = shown_value(key, &change.from);
    let to = shown_value(key, &change.to);
    if change.reset {
        format!("{from}  →  {to} (default; removed from the file)")
    } else {
        format!("{from}  →  {to}")
    }
}

/// The Settings window's state.
#[derive(Debug, Clone)]
pub struct SettingsWindow {
    /// The file as opened, plus the unsaved changes.
    pub session: EditSession,
    /// The config file as shown (`~/.config/watchtower/config.toml`).
    pub shown_path: String,
    /// The rows the selection moves over, in display order.
    pub rows: Vec<Row>,
    /// Index into [`SettingsWindow::rows`].
    pub selected: usize,
    pub mode: Mode,
    /// The window's own status line message.
    pub notice: Option<Notice>,
    /// A save runs on a worker thread.
    pub saving: bool,
    /// What follows the running save.
    pub after_save: Option<AfterSave>,
    /// The last save wrote everything and nothing changed since.
    pub just_saved: bool,
    /// The running config's style and theme, drawn when nothing is
    /// previewed; the app keeps it current across reloads.
    pub base: (UiStyle, UiTheme),
    /// What the list editor knows about each folder.
    pub folders: BTreeMap<String, FolderInfo>,
    /// Folders still to look at.
    wanted: BTreeSet<String>,
    /// A `Tab` completion still to run.
    complete_wanted: Option<String>,
    /// A [`FolderJob`] runs.
    folder_busy: bool,
    /// The home folder, to show paths with `~`.
    home: PathBuf,
}

impl SettingsWindow {
    /// The window over `session`; `shown_path` is the file as shown, `base`
    /// the running style and theme.
    pub fn new(
        session: EditSession,
        shown_path: String,
        base: (UiStyle, UiTheme),
        home: &Path,
    ) -> Self {
        let mut rows: Vec<Row> = SettingKey::ALL.iter().map(|k| Row::Setting(*k)).collect();
        rows.extend((0..session.accounts().len()).map(Row::Account));
        SettingsWindow {
            session,
            shown_path,
            rows,
            selected: 0,
            mode: Mode::Browse,
            notice: None,
            saving: false,
            after_save: None,
            just_saved: false,
            base,
            folders: BTreeMap::new(),
            wanted: BTreeSet::new(),
            complete_wanted: None,
            folder_busy: false,
            home: home.to_path_buf(),
        }
    }

    /// The home folder paths are shown against (`~`).
    pub fn home_dir(&self) -> PathBuf {
        self.home.clone()
    }

    /// The selected row.
    pub fn selected_row(&self) -> Row {
        self.rows
            .get(self.selected)
            .copied()
            .unwrap_or(Row::Setting(SettingKey::UiStyle))
    }

    /// The selected setting, if the selection is on one.
    pub fn selected_key(&self) -> Option<SettingKey> {
        match self.selected_row() {
            Row::Setting(key) => Some(key),
            Row::Account(_) => None,
        }
    }

    /// Selects the row of `key`.
    pub fn select(&mut self, key: SettingKey) {
        if let Some(i) = self.rows.iter().position(|r| *r == Row::Setting(key)) {
            self.selected = i;
        }
    }

    /// Number of unsaved changes.
    pub fn change_count(&self) -> usize {
        self.session.changes().len()
    }

    /// Whether the keys go to a text field (typed Cyrillic stays as is,
    /// doc/0011).
    pub fn takes_text(&self) -> bool {
        match &self.mode {
            Mode::Input(_) => true,
            Mode::List(list) => list.input.is_some(),
            _ => false,
        }
    }

    /// The style and theme to draw while the window is open: the pick
    /// list's value under the cursor, else the unsaved value; `None` keeps
    /// [`SettingsWindow::base`].
    pub fn preview(&self) -> (Option<UiStyle>, Option<UiTheme>) {
        let shown = |key: SettingKey| -> Option<String> {
            if let Mode::Pick(pick) = &self.mode
                && pick.key == key
            {
                return pick.value().map(str::to_owned);
            }
            match self
                .session
                .is_modified(key)
                .then(|| self.session.value(key))
            {
                Some(SettingValue::Text(v)) => Some(v),
                _ => None,
            }
        };
        (
            shown(SettingKey::UiStyle).and_then(|v| UiStyle::parse(&v).ok()),
            shown(SettingKey::UiTheme).and_then(|v| UiTheme::parse(&v).ok()),
        )
    }

    /// What the header says is previewed while the pick list of the style
    /// or theme is open: `theme warm`, when it differs from the running
    /// one.
    pub fn previewing(&self) -> Option<String> {
        if !matches!(&self.mode, Mode::Pick(p) if matches!(p.key, SettingKey::UiStyle | SettingKey::UiTheme))
        {
            return None;
        }
        let (style, theme) = self.preview();
        let mut parts = Vec::new();
        if let Some(s) = style.filter(|s| *s != self.base.0) {
            parts.push(format!("style {}", s.as_str()));
        }
        if let Some(t) = theme.filter(|t| *t != self.base.1) {
            parts.push(format!("theme {}", t.as_str()));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: NoticeKind::Info,
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: NoticeKind::Error,
        });
    }

    /// Sets `key` to `value` as an unsaved change; an invalid value is
    /// refused with its message.
    fn set(&mut self, key: SettingKey, value: SettingValue) -> bool {
        match self.session.set(key, value) {
            Ok(()) => {
                self.just_saved = false;
                self.notice = None;
                true
            }
            Err(message) => {
                self.error(message);
                false
            }
        }
    }

    /// Handles a key; command keys arrive already mapped from a Cyrillic
    /// layout, text keys as typed ([`SettingsWindow::takes_text`]).
    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsAction {
        match self.mode.clone() {
            Mode::Browse => self.browse_key(key),
            Mode::Pick(pick) => self.pick_key(key, pick),
            Mode::Input(input) => self.input_key(key, input),
            Mode::List(list) => self.list_key(key, list),
            Mode::Unsaved => self.question_key(key, |w, yes| {
                if yes {
                    w.save(Some(AfterSave::Close))
                } else {
                    w.session.revert_all();
                    SettingsAction::Close
                }
            }),
            Mode::EditPrompt => self.question_key(key, |w, yes| {
                if yes {
                    w.save(Some(AfterSave::Edit))
                } else {
                    w.session.revert_all();
                    SettingsAction::Edit
                }
            }),
            Mode::Conflict(_) => self.question_key(key, |w, yes| {
                if yes {
                    w.saving = true;
                    w.info("saving…");
                    SettingsAction::Save { force: true }
                } else {
                    w.after_save = None;
                    w.info("not saved; your changes are still here");
                    SettingsAction::None
                }
            }),
            Mode::CrossGroup => self.question_key(key, |w, yes| {
                if yes {
                    w.set(SettingKey::AllowCrossGroupHandoff, SettingValue::Bool(true));
                }
                SettingsAction::None
            }),
            Mode::QuitUnsaved => self.question_key(key, |_, yes| {
                if yes {
                    SettingsAction::Quit
                } else {
                    SettingsAction::None
                }
            }),
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                    self.mode = Mode::Browse;
                }
                SettingsAction::None
            }
        }
    }

    /// A question: `y` answers yes and `n` no; `Esc` and `Enter` cancel
    /// (the default, doc/0011), as `n` does where no "no" is offered.
    fn question_key(
        &mut self,
        key: KeyEvent,
        answer: impl FnOnce(&mut SettingsWindow, bool) -> SettingsAction,
    ) -> SettingsAction {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return SettingsAction::None;
        }
        let cancels_with_n = matches!(
            self.mode,
            Mode::Conflict(_) | Mode::CrossGroup | Mode::QuitUnsaved
        );
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                self.mode = Mode::Browse;
                answer(self, true)
            }
            KeyCode::Char('n' | 'N') if !cancels_with_n => {
                self.mode = Mode::Browse;
                answer(self, false)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                self.mode = Mode::Browse;
                if cancels_with_n {
                    answer(self, false)
                } else {
                    SettingsAction::None
                }
            }
            _ => SettingsAction::None,
        }
    }

    fn browse_key(&mut self, key: KeyEvent) -> SettingsAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            return match key.code {
                KeyCode::Char('s' | 'S') => self.save(None),
                KeyCode::Char('c' | 'C') => self.quit(),
                _ => SettingsAction::None,
            };
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return SettingsAction::None;
        }
        let last = self.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = last,
            KeyCode::Tab => self.jump_section(true),
            KeyCode::BackTab => self.jump_section(false),
            KeyCode::Enter => self.edit(),
            KeyCode::Left => self.cycle(false),
            KeyCode::Right => self.cycle(true),
            KeyCode::Delete => self.reset(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('e') => {
                if self.session.has_changes() {
                    self.mode = Mode::EditPrompt;
                } else {
                    return SettingsAction::Edit;
                }
            }
            KeyCode::Esc => {
                if self.session.has_changes() {
                    self.mode = Mode::Unsaved;
                } else {
                    return SettingsAction::Close;
                }
            }
            _ => {}
        }
        SettingsAction::None
    }

    /// `Ctrl+C`: quits, asking first when something is unsaved.
    fn quit(&mut self) -> SettingsAction {
        if self.session.has_changes() {
            self.mode = Mode::QuitUnsaved;
            SettingsAction::None
        } else {
            SettingsAction::Quit
        }
    }

    /// `Tab` / `Shift+Tab`: the first row of the next / previous section,
    /// wrapping around.
    fn jump_section(&mut self, forward: bool) {
        let current = self.selected_row().section();
        let mut sections: Vec<Section> = Vec::new();
        for row in &self.rows {
            if sections.last() != Some(&row.section()) {
                sections.push(row.section());
            }
        }
        let Some(at) = sections.iter().position(|s| *s == current) else {
            return;
        };
        let n = sections.len();
        let target = if forward {
            sections[(at + 1) % n]
        } else {
            sections[(at + n - 1) % n]
        };
        if let Some(i) = self.rows.iter().position(|r| r.section() == target) {
            self.selected = i;
        }
    }

    /// `Enter` on the selected row.
    fn edit(&mut self) {
        let Some(key) = self.selected_key() else {
            self.info(
                "accounts are read-only here: e opens config.toml; restart tower after changing \
                 them",
            );
            return;
        };
        match self.session.kind(key) {
            SettingKind::Choice {
                mut values,
                free_text,
            } => {
                if free_text {
                    values.push(OTHER.to_owned());
                }
                let current = self.session.value(key).display();
                let cursor = values
                    .iter()
                    .position(|v| *v == current)
                    .unwrap_or(if free_text { values.len() - 1 } else { 0 });
                self.mode = Mode::Pick(Pick {
                    key,
                    items: values,
                    cursor,
                });
            }
            SettingKind::Steps { values, .. } => {
                let current = self.session.value(key);
                let items: Vec<String> = values
                    .iter()
                    .map(|v| shown_value(key, &SettingValue::Int(*v)))
                    .collect();
                let cursor = match current {
                    SettingValue::Int(n) => values.iter().position(|v| *v >= n).unwrap_or(0),
                    _ => 0,
                };
                self.mode = Mode::Pick(Pick { key, items, cursor });
            }
            SettingKind::Bool => self.toggle(key),
            SettingKind::Int { .. } | SettingKind::Text => self.start_input(key, None),
            SettingKind::Paths => {
                let items = match self.session.value(key) {
                    SettingValue::Paths(p) => p,
                    _ => Vec::new(),
                };
                for item in &items {
                    self.want_folder(item);
                }
                self.mode = Mode::List(ListEditor {
                    key,
                    items,
                    selected: 0,
                    input: None,
                });
            }
        }
    }

    /// The inline input of `key`, holding `text` or the current value.
    fn start_input(&mut self, key: SettingKey, text: Option<String>) {
        let text = text.unwrap_or_else(|| self.session.value(key).display());
        let mut input = Input {
            key,
            input: LineInput::new(&text),
            error: None,
        };
        input.error = self.parse_input(key, &text).err();
        self.mode = Mode::Input(input);
    }

    /// The value typed for `key`, checked ([`EditSession::check`]).
    fn parse_input(&self, key: SettingKey, text: &str) -> Result<SettingValue, String> {
        let value = match self.session.kind(key) {
            SettingKind::Int { min, max } => match text.trim().parse::<i64>() {
                Ok(n) => SettingValue::Int(n),
                Err(_) => return Err(format!("must be {min} to {max} seconds")),
            },
            // Free text is kept trimmed, so blanks save as an empty
            // hand-off prompt (no prompt, doc/0033).
            SettingKind::Text => SettingValue::Text(text.trim().to_owned()),
            _ => SettingValue::Text(text.to_owned()),
        };
        self.session.check(key, &value).map(|()| value)
    }

    /// Toggles a bool; turning the cross-group hand-off on asks first.
    fn toggle(&mut self, key: SettingKey) {
        let on = self.session.value(key) == SettingValue::Bool(true);
        if !on && key == SettingKey::AllowCrossGroupHandoff {
            self.mode = Mode::CrossGroup;
            return;
        }
        self.set(key, SettingValue::Bool(!on));
    }

    /// `←` / `→`: the previous / next offered value of a choice; a bool
    /// toggles.
    fn cycle(&mut self, forward: bool) {
        let Some(key) = self.selected_key() else {
            return;
        };
        match self.session.kind(key) {
            SettingKind::Choice { values, .. } if !values.is_empty() => {
                let current = self.session.value(key).display();
                let n = values.len();
                let next = match values.iter().position(|v| *v == current) {
                    Some(i) if forward => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                    None if forward => 0,
                    None => n - 1,
                };
                self.set(key, SettingValue::Text(values[next].clone()));
            }
            SettingKind::Steps { values, .. } if !values.is_empty() => {
                let n = match self.session.value(key) {
                    SettingValue::Int(n) => n,
                    _ => 0,
                };
                // The next step up or down, wrapping around; a hand-edited
                // value between two steps goes to the nearer one that way.
                let next = if forward {
                    values.iter().find(|v| **v > n).or(values.first())
                } else {
                    values.iter().rev().find(|v| **v < n).or(values.last())
                };
                if let Some(next) = next {
                    self.set(key, SettingValue::Int(*next));
                }
            }
            SettingKind::Bool => self.toggle(key),
            _ => self.info("Enter edits this setting"),
        }
    }

    /// `Del`: back to the built-in default; the save removes the key.
    fn reset(&mut self) {
        let Some(key) = self.selected_key() else {
            self.info("accounts are read-only here: e opens config.toml");
            return;
        };
        if self.session.saved(key).is_none() && !self.session.is_modified(key) {
            self.info(format!("{} is already the default", key.label()));
            return;
        }
        self.session.reset_to_default(key);
        self.just_saved = false;
        if self.session.is_modified(key) {
            self.info(format!(
                "{} back to the default; saving removes it from the file",
                key.label()
            ));
        } else {
            self.info(format!("{} back to the default", key.label()));
        }
    }

    /// `Ctrl+S`, or `y` in a question: saves on a worker thread, then
    /// `after`. Refused while a save runs or the changes would not give a
    /// valid config.
    fn save(&mut self, after: Option<AfterSave>) -> SettingsAction {
        if self.saving {
            self.info("a save is already running");
            return SettingsAction::None;
        }
        if !self.session.has_changes() {
            return match after {
                Some(AfterSave::Close) => SettingsAction::Close,
                Some(AfterSave::Edit) => SettingsAction::Edit,
                None => {
                    self.info("nothing to save");
                    SettingsAction::None
                }
            };
        }
        if let Err(err) = self.session.validate() {
            self.error(format!("not saved: {err}"));
            return SettingsAction::None;
        }
        self.saving = true;
        self.after_save = after;
        self.info("saving…");
        SettingsAction::Save { force: false }
    }

    /// Takes in a finished save: the saved changes are no longer unsaved
    /// and the status line says what was written; a conflict asks.
    pub fn save_finished(&mut self, result: Result<SaveOutcome, SaveError>) -> SettingsAction {
        self.saving = false;
        match result {
            Ok(outcome) => {
                let n = outcome.written.len();
                self.session.finish_save(&outcome);
                if n == 0 {
                    self.info("nothing to save");
                } else {
                    self.just_saved = !self.session.has_changes();
                    self.notice = Some(Notice {
                        text: saved_text(&outcome, &self.shown_path),
                        kind: NoticeKind::Success,
                    });
                }
                match self.after_save.take() {
                    Some(AfterSave::Close) => SettingsAction::Close,
                    Some(AfterSave::Edit) => SettingsAction::Edit,
                    None => SettingsAction::None,
                }
            }
            Err(SaveError::Conflict(conflicts)) => {
                self.notice = Some(Notice {
                    text: "changed in the file meanwhile: y overwrites, n cancels".to_owned(),
                    kind: NoticeKind::Warning,
                });
                self.mode = Mode::Conflict(conflicts);
                SettingsAction::None
            }
            Err(err) => {
                self.after_save = None;
                self.error(format!("not saved: {}", one_line(&err.to_string())));
                SettingsAction::None
            }
        }
    }

    fn pick_key(&mut self, key: KeyEvent, mut pick: Pick) -> SettingsAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(key.code, KeyCode::Char('c' | 'C')) {
                self.mode = Mode::Browse;
                return self.quit();
            }
            return SettingsAction::None;
        }
        let n = pick.items.len().max(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => pick.cursor = (pick.cursor + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => pick.cursor = (pick.cursor + 1) % n,
            KeyCode::Home | KeyCode::Char('g') => pick.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => pick.cursor = n - 1,
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                return SettingsAction::None;
            }
            KeyCode::Enter => {
                self.mode = Mode::Browse;
                match pick.value() {
                    Some(v) => {
                        let value = self.picked(pick.key, v);
                        self.set(pick.key, value);
                    }
                    None => {
                        let current = self.session.value(pick.key).display();
                        let text = if pick.items.contains(&current) {
                            String::new()
                        } else {
                            current
                        };
                        self.start_input(pick.key, Some(text));
                    }
                }
                return SettingsAction::None;
            }
            _ => {}
        }
        self.mode = Mode::Pick(pick);
        SettingsAction::None
    }

    /// The value of the pick list's entry `label` for `key`: the step a
    /// label like `off` or `5 s` stands for, else the text itself.
    fn picked(&self, key: SettingKey, label: &str) -> SettingValue {
        if let SettingKind::Steps { values, .. } = self.session.kind(key)
            && let Some(v) = values
                .iter()
                .find(|v| shown_value(key, &SettingValue::Int(**v)) == label)
        {
            return SettingValue::Int(*v);
        }
        SettingValue::Text(label.to_owned())
    }

    fn input_key(&mut self, key: KeyEvent, mut input: Input) -> SettingsAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('s' | 'S') if ctrl => {
                // Keep a valid field, then save; an invalid one stays.
                match self.parse_input(input.key, &input.input.text()) {
                    Ok(value) => {
                        self.mode = Mode::Browse;
                        self.set(input.key, value);
                        return self.save(None);
                    }
                    Err(message) => {
                        self.error(format!(
                            "not saved: fix {} first ({message})",
                            input.key.label()
                        ));
                        self.mode = Mode::Input(input);
                        return SettingsAction::None;
                    }
                }
            }
            KeyCode::Char('c' | 'C') if ctrl => {
                self.mode = Mode::Browse;
                return self.quit();
            }
            KeyCode::Char('u' | 'U') if ctrl => input.input.set(""),
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                return SettingsAction::None;
            }
            KeyCode::Enter => match self.parse_input(input.key, &input.input.text()) {
                Ok(value) => {
                    self.mode = Mode::Browse;
                    self.set(input.key, value);
                    return SettingsAction::None;
                }
                Err(message) => input.error = Some(message),
            },
            _ => {
                input.input.key(&key);
            }
        }
        input.error = self.parse_input(input.key, &input.input.text()).err();
        self.mode = Mode::Input(input);
        SettingsAction::None
    }
}

/// The status line after a save: what was written and what applies.
fn saved_text(outcome: &SaveOutcome, shown_path: &str) -> String {
    let n = outcome.written.len();
    let changes = if n == 1 {
        "1 change".to_owned()
    } else {
        format!("{n} changes")
    };
    let mut text = if outcome.created {
        format!("✓ Created {shown_path} with {changes}")
    } else {
        format!("✓ Saved {changes} to {shown_path} · comments and formatting kept")
    };
    if let Some(bak) = outcome.backup.as_ref().and_then(|b| b.file_name()) {
        text.push_str(&format!(" · backup {}", bak.to_string_lossy()));
    }
    text.push_str(" · applied now");
    text
}

/// `text` on one line: newlines become spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- the list editor and its folder checks ---------------------------------

impl SettingsWindow {
    fn list_key(&mut self, key: KeyEvent, mut list: ListEditor) -> SettingsAction {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            self.mode = Mode::Browse;
            return self.quit();
        }
        if let Some(input) = list.input.take() {
            self.list_input_key(key, &mut list, input);
            self.mode = Mode::List(list);
            return SettingsAction::None;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.mode = Mode::List(list);
            return SettingsAction::None;
        }
        let last = list.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => list.selected = list.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => list.selected = (list.selected + 1).min(last),
            KeyCode::Char('a') => {
                list.input = Some(ListInput {
                    index: None,
                    input: LineInput::new(""),
                    error: None,
                });
            }
            KeyCode::Enter if list.items.is_empty() => {
                list.input = Some(ListInput {
                    index: None,
                    input: LineInput::new(""),
                    error: None,
                });
            }
            KeyCode::Enter => {
                let text = list.items[list.selected].clone();
                list.input = Some(ListInput {
                    index: Some(list.selected),
                    input: LineInput::new(&text),
                    error: None,
                });
            }
            KeyCode::Char('d') | KeyCode::Delete if !list.items.is_empty() => {
                let removed = list.items.remove(list.selected);
                list.selected = list.selected.min(list.items.len().saturating_sub(1));
                if self.set(list.key, SettingValue::Paths(list.items.clone())) {
                    self.info(format!("removed {removed}; unsaved until Ctrl+S"));
                }
            }
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                return SettingsAction::None;
            }
            _ => {}
        }
        self.mode = Mode::List(list);
        SettingsAction::None
    }

    /// A key while a folder is typed: `Enter` keeps a valid folder, `Esc`
    /// cancels, `Tab` completes the name, `Ctrl+U` clears.
    fn list_input_key(&mut self, key: KeyEvent, list: &mut ListEditor, mut input: ListInput) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u' | 'U') if ctrl => input.input.set(""),
            KeyCode::Esc => return,
            KeyCode::Tab => {
                self.complete_wanted = Some(input.input.text());
            }
            KeyCode::Enter => {
                let text = clean_folder(&input.input.text());
                if text.is_empty() {
                    if input.index.is_none() {
                        return;
                    }
                    input.error = Some("a folder must not be empty; d removes it".to_owned());
                    list.input = Some(input);
                    return;
                }
                let candidate = candidate_items(&list.items, input.index, &text);
                match self
                    .session
                    .check(list.key, &SettingValue::Paths(candidate.clone()))
                {
                    Ok(()) => {
                        list.selected = input.index.unwrap_or(candidate.len() - 1);
                        list.items = candidate;
                        self.want_folder(&text);
                        self.set(list.key, SettingValue::Paths(list.items.clone()));
                        return;
                    }
                    Err(message) => input.error = Some(message),
                }
                list.input = Some(input);
                return;
            }
            _ => {
                if input.input.key(&key) == KeyUse::Changed {
                    self.complete_wanted = None;
                }
            }
        }
        let text = clean_folder(&input.input.text());
        input.error = if text.is_empty() {
            None
        } else {
            let candidate = candidate_items(&list.items, input.index, &text);
            self.want_folder(&text);
            self.session
                .check(list.key, &SettingValue::Paths(candidate))
                .err()
        };
        list.input = Some(input);
    }

    /// Asks for a look at `folder` unless it is known.
    fn want_folder(&mut self, folder: &str) {
        if !self.folders.contains_key(folder) {
            self.folders.insert(folder.to_owned(), FolderInfo::Checking);
            self.wanted.insert(folder.to_owned());
        }
    }

    /// The next [`FolderJob`] to run, unless one runs or nothing is
    /// wanted. At most one runs at a time, so a hung mount costs one
    /// thread and never blocks the window.
    pub fn take_folder_job(&mut self, home: &Path) -> Option<FolderJob> {
        if self.folder_busy || (self.wanted.is_empty() && self.complete_wanted.is_none()) {
            return None;
        }
        self.folder_busy = true;
        Some(FolderJob {
            probe: std::mem::take(&mut self.wanted).into_iter().collect(),
            complete: self.complete_wanted.take(),
            home: home.to_path_buf(),
        })
    }

    /// Takes in a finished [`FolderJob`]: the folders' notes, and the
    /// completion when the input still holds the text it was for.
    pub fn folders_checked(&mut self, result: FolderResult) {
        self.folder_busy = false;
        for (folder, info) in result.probed {
            self.folders.insert(folder, info);
        }
        let Some(done) = result.completed else {
            return;
        };
        let Mode::List(list) = &mut self.mode else {
            return;
        };
        let items = list.items.clone();
        let Some(input) = list.input.as_mut() else {
            return;
        };
        if input.input.text() != done.text {
            return;
        }
        match (&done.result, done.matches.len()) {
            (Some(text), _) => {
                input.input.set(text);
                let folder = clean_folder(text);
                let candidate = candidate_items(&items, input.index, &folder);
                input.error = self
                    .session
                    .check(list.key, &SettingValue::Paths(candidate))
                    .err();
                if !self.folders.contains_key(&folder) {
                    self.folders.insert(folder.clone(), FolderInfo::Checking);
                    self.wanted.insert(folder);
                }
                if done.matches.len() > 1 {
                    let names = done.matches.join("  ");
                    self.info(format!("{} folders: {names}", done.matches.len()));
                }
            }
            (None, 0) => self.info(format!("no folder matches {}", done.text)),
            (None, n) => {
                let names = done.matches.join("  ");
                self.info(format!("{n} folders: {names}"));
            }
        }
    }

    /// The note next to a folder: `✓ 6 git clones`, `not found; skipped
    /// when scanning`, … for the list editor of `key`.
    pub fn folder_note(&self, key: SettingKey, folder: &str) -> (String, NoticeKind) {
        let info = self
            .folders
            .get(folder)
            .cloned()
            .unwrap_or(FolderInfo::Checking);
        match info {
            FolderInfo::Checking => ("checking…".to_owned(), NoticeKind::Info),
            FolderInfo::Missing => (
                "not found; skipped when scanning".to_owned(),
                NoticeKind::Warning,
            ),
            FolderInfo::NotFolder => (
                "not a folder; skipped when scanning".to_owned(),
                NoticeKind::Warning,
            ),
            FolderInfo::Folder { git, clones } => {
                if key == SettingKey::ExtraProjects {
                    if git {
                        ("✓ git clone".to_owned(), NoticeKind::Success)
                    } else {
                        ("✓ folder (not a git clone)".to_owned(), NoticeKind::Success)
                    }
                } else if clones == 0 {
                    ("no git clones in it".to_owned(), NoticeKind::Warning)
                } else if clones == 1 {
                    ("✓ 1 git clone".to_owned(), NoticeKind::Success)
                } else {
                    (format!("✓ {clones} git clones"), NoticeKind::Success)
                }
            }
        }
    }
}

/// The list with `text` in place of entry `index`, or added.
fn candidate_items(items: &[String], index: Option<usize>, text: &str) -> Vec<String> {
    let mut items = items.to_vec();
    match index {
        Some(i) if i < items.len() => items[i] = text.to_owned(),
        _ => items.push(text.to_owned()),
    }
    items
}
