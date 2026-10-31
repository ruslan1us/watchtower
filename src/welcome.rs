//! The welcome window (doc/0048): the first start without a config file, and
//! `tower setup` at any time. It is a [`SettingsWindow`] with a [`Welcome`]
//! on top, so the draft, the previews, the save with its conflicts and the
//! folder jobs are the Settings window's (doc/0024, doc/0022); this module
//! adds the steps and their keys. The window is pure state, like the Settings
//! window: keys go in and a [`SettingsAction`] comes out for the app.

use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{ColorDepth, UiStyle, UiTheme};
use crate::config_edit::{EditSession, SettingKey, SettingValue, first_run_template};
use crate::dialog::{KeyUse, LineInput};
use crate::paths;
use crate::settings::{
    AfterSave, Completion, FolderInfo, FolderResult, SettingsAction, SettingsWindow, clean_folder,
};

/// The smallest terminal the steps fit: below it no window opens at a start,
/// and a window already open shows a plain notice until the terminal grows.
pub const MIN_SIZE: (u16, u16) = (80, 24);

/// The folders a new user's projects usually live in, suggested on the
/// Projects step when they exist.
pub const COMMON_FOLDERS: [&str; 8] = [
    "~/projects",
    "~/workspace",
    "~/code",
    "~/src",
    "~/dev",
    "~/repos",
    "~/git",
    "~/work",
];

/// The switches of the Options step.
pub const OPTION_KEYS: [SettingKey; 3] = [
    SettingKey::UsageShow,
    SettingKey::TrackSubagents,
    SettingKey::UiTabTitle,
];

/// Why the window opened; it decides what `Esc` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `tower` with no config file: `Esc` saves the current choices, so the
    /// window never comes back.
    FirstRun,
    /// `tower setup`: `Esc` cancels without writing.
    Setup,
}

impl Origin {
    /// The window a start opens: `tower setup` always, `tower` only when no
    /// config file was found.
    pub fn for_start(setup: bool, config_found: bool) -> Option<Origin> {
        match (setup, config_found) {
            (true, _) => Some(Origin::Setup),
            (false, false) => Some(Origin::FirstRun),
            (false, true) => None,
        }
    }
}

/// The steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Look,
    Projects,
    Options,
    Done,
}

impl Step {
    pub const ALL: [Step; 5] = [
        Step::Welcome,
        Step::Look,
        Step::Projects,
        Step::Options,
        Step::Done,
    ];

    /// The step's name in the header.
    pub fn title(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::Look => "Look",
            Step::Projects => "Projects",
            Step::Options => "Options",
            Step::Done => "Done",
        }
    }

    fn at(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The step after this one; the last has none.
    pub fn next(self) -> Option<Step> {
        Step::ALL.get(self.at() + 1).copied()
    }

    /// The step before this one; the first has none.
    pub fn prev(self) -> Option<Step> {
        self.at().checked_sub(1).map(|i| Step::ALL[i])
    }
}

/// What `<command> --version` said, asked on a worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeProbe {
    Checking,
    /// The version as printed, without the product name.
    Found(String),
    /// Why the command could not be run.
    Failed(String),
}

/// The version in the first line of `claude --version` (`2.1.316 (Claude
/// Code)`, see doc/reference/claude-code-cli.md). Anything else is shown as
/// it is, without control characters.
pub fn version_text(stdout: &str) -> String {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty());
    let line = line.unwrap_or("");
    let version = line.strip_suffix("(Claude Code)").unwrap_or(line).trim();
    crate::attach::printable(version)
}

/// A folder on the Projects step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChoice {
    /// The folder as written (`~` kept).
    pub path: String,
    /// `project_roots` or `extra_projects`.
    pub list: SettingKey,
    pub ticked: bool,
}

/// The welcome window's state.
#[derive(Debug, Clone)]
pub struct Welcome {
    pub origin: Origin,
    pub step: Step,
    /// The selected row of the Look step, in [`SettingsWindow::look_rows`].
    pub look_row: usize,
    /// The folders offered: the config's first, then the suggestions in
    /// [`COMMON_FOLDERS`] order, then the typed ones.
    pub projects: Vec<ProjectChoice>,
    /// The selected row of the Projects step; `projects.len()` is the field
    /// for another folder.
    pub project_row: usize,
    /// The field for another folder.
    pub field: LineInput,
    /// Why the field's folder was refused.
    pub field_error: Option<String>,
    /// A folder `Enter` asked to add, being looked at.
    pub adding: Option<String>,
    /// The selected row of the Options step, in [`OPTION_KEYS`].
    pub option_row: usize,
    /// What `claude --version` said.
    pub claude: ClaudeProbe,
    /// The suggested folders have been looked at.
    pub scanned: bool,
    /// Why the save failed: the window only says so now, and goes on.
    pub failed: Option<String>,
    /// The terminal has become too small for the steps ([`MIN_SIZE`]).
    pub too_small: bool,
    /// Preselect the suggested folders that hold git clones: only for a
    /// draft that starts from no file.
    preselect: bool,
}

impl SettingsWindow {
    /// The window over `session` for `origin`. Without a file the draft is a
    /// first start's: `workspace` preselected, and the save writes the whole
    /// commented example ([`first_run_template`]). With one it starts from
    /// the file's values and changes only what the user changes.
    pub fn welcome(
        session: EditSession,
        shown_path: String,
        base: (UiStyle, UiTheme, ColorDepth),
        home: &Path,
        origin: Origin,
    ) -> SettingsWindow {
        let new = !session.exists();
        let session = if new {
            session.with_template(first_run_template())
        } else {
            session
        };
        let mut window = SettingsWindow::new(session, shown_path, base, home);
        if new {
            // Always a change, so the file is written even if every other
            // choice stays as it is.
            let style = SettingValue::Text(UiStyle::Workspace.as_str().to_owned());
            let _ = window.session.set(SettingKey::UiStyle, style);
        }
        let mut projects = Vec::new();
        for key in [SettingKey::ProjectRoots, SettingKey::ExtraProjects] {
            if let SettingValue::Paths(paths) = window.session.value(key) {
                for path in paths {
                    projects.push(ProjectChoice {
                        path,
                        list: key,
                        ticked: true,
                    });
                }
            }
        }
        for choice in &projects {
            window.want_folder(&choice.path);
        }
        for folder in COMMON_FOLDERS {
            window.want_folder(folder);
        }
        window.welcome = Some(Welcome {
            origin,
            step: Step::Welcome,
            look_row: 0,
            project_row: 0,
            projects,
            field: LineInput::new(""),
            field_error: None,
            adding: None,
            option_row: 0,
            claude: ClaudeProbe::Checking,
            scanned: false,
            failed: None,
            too_small: false,
            preselect: new,
        });
        window
    }

    /// The rows of the Look step: the card size only means something in the
    /// workspace style.
    pub fn look_rows(&self) -> Vec<SettingKey> {
        let mut rows = vec![SettingKey::UiStyle, SettingKey::UiTheme];
        let workspace = SettingValue::Text(UiStyle::Workspace.as_str().to_owned());
        if self.session.value(SettingKey::UiStyle) == workspace {
            rows.push(SettingKey::UiCardSize);
        }
        rows
    }

    /// Whether the keys go to the field for another folder: typed Cyrillic
    /// stays as it is (doc/0011).
    pub fn welcome_takes_text(&self) -> bool {
        self.welcome.as_ref().is_some_and(|w| {
            w.step == Step::Projects
                && w.project_row == w.projects.len()
                && w.failed.is_none()
                && !w.too_small
        })
    }

    /// A key for the steps.
    pub(crate) fn welcome_key(&mut self, key: KeyEvent) -> SettingsAction {
        let Some(w) = self.welcome.as_ref() else {
            return SettingsAction::None;
        };
        let (origin, step) = (w.origin, w.step);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Quits at once, also during a save: nothing was asked for yet.
        if ctrl && matches!(key.code, KeyCode::Char('c' | 'C')) {
            return SettingsAction::Quit;
        }
        if self.saving {
            return SettingsAction::None;
        }
        if w.failed.is_some() {
            return match key.code {
                KeyCode::Enter | KeyCode::Esc => SettingsAction::Close,
                _ => SettingsAction::None,
            };
        }
        if w.too_small {
            return match key.code {
                KeyCode::Esc => {
                    self.session.revert_all();
                    SettingsAction::Close
                }
                _ => SettingsAction::None,
            };
        }
        if self.welcome_takes_text()
            && let Some(action) = self.field_key(key)
        {
            return action;
        }
        if ctrl || key.modifiers.contains(KeyModifiers::ALT) {
            return SettingsAction::None;
        }
        match key.code {
            KeyCode::Esc => match origin {
                Origin::FirstRun => self.finish(),
                Origin::Setup => {
                    self.session.revert_all();
                    SettingsAction::Close
                }
            },
            KeyCode::Enter => match step.next() {
                Some(next) => {
                    self.go_to(next);
                    SettingsAction::None
                }
                None => self.finish(),
            },
            KeyCode::Backspace | KeyCode::BackTab => {
                if let Some(prev) = step.prev() {
                    self.go_to(prev);
                }
                SettingsAction::None
            }
            code => {
                self.step_key(step, code);
                SettingsAction::None
            }
        }
    }

    /// Saves the draft, then the window closes and tower carries on. In a
    /// first start it always writes: the draft has at least the style.
    fn finish(&mut self) -> SettingsAction {
        self.save(Some(AfterSave::Close))
    }

    fn go_to(&mut self, step: Step) {
        if let Some(w) = self.welcome.as_mut() {
            w.step = step;
            // Leaving the field keeps what is in it.
            w.project_row = w.project_row.min(w.projects.len());
        }
        self.notice = None;
    }

    /// `↑↓` move, `←→` and `Space` change, on the step's rows.
    fn step_key(&mut self, step: Step, code: KeyCode) {
        let delta = match code {
            KeyCode::Up | KeyCode::Char('k') => Some(false),
            KeyCode::Down | KeyCode::Char('j') => Some(true),
            _ => None,
        };
        let change = match code {
            KeyCode::Left => Some(false),
            KeyCode::Right | KeyCode::Char(' ') => Some(true),
            _ => None,
        };
        let look_rows = self.look_rows().len();
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        if let Some(down) = delta {
            let (row, last) = match step {
                Step::Look => (&mut w.look_row, look_rows - 1),
                Step::Projects => (&mut w.project_row, w.projects.len()),
                Step::Options => (&mut w.option_row, OPTION_KEYS.len() - 1),
                Step::Welcome | Step::Done => return,
            };
            *row = if down {
                (*row + 1).min(last)
            } else {
                row.saturating_sub(1)
            };
            return;
        }
        let Some(forward) = change else {
            return;
        };
        match step {
            Step::Look => {
                let key = self.look_rows()[self.welcome.as_ref().map_or(0, |w| w.look_row)];
                self.cycle_key(key, forward);
                self.session.drop_if_unchanged(key);
            }
            Step::Projects => {
                let row = w.project_row;
                if let Some(choice) = w.projects.get_mut(row) {
                    choice.ticked = !choice.ticked;
                }
                self.sync_projects();
            }
            Step::Options => {
                let key = OPTION_KEYS[w.option_row];
                let on = self.session.value(key) == SettingValue::Bool(true);
                self.set(key, SettingValue::Bool(!on));
                self.session.drop_if_unchanged(key);
            }
            Step::Welcome | Step::Done => {}
        }
    }

    /// The draft's folder lists from what is ticked.
    fn sync_projects(&mut self) {
        let Some(w) = self.welcome.as_ref() else {
            return;
        };
        let wanted = |list: SettingKey| -> Vec<String> {
            let ticked = w.projects.iter().filter(|c| c.ticked && c.list == list);
            ticked.map(|c| c.path.clone()).collect()
        };
        let lists = [
            (SettingKey::ProjectRoots, wanted(SettingKey::ProjectRoots)),
            (SettingKey::ExtraProjects, wanted(SettingKey::ExtraProjects)),
        ];
        for (key, paths) in lists {
            let value = SettingValue::Paths(paths);
            if self.session.value(key) != value {
                self.set(key, value);
                self.session.drop_if_unchanged(key);
            }
        }
    }

    /// A key while the field for another folder has the keys. `None` passes
    /// it on to the step: `Enter` with nothing typed goes on, as anywhere.
    fn field_key(&mut self, key: KeyEvent) -> Option<SettingsAction> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let w = self.welcome.as_mut()?;
        let empty = w.field.text().is_empty();
        match key.code {
            KeyCode::Char('u' | 'U') if ctrl => {
                w.field.set("");
                w.adding = None;
                w.field_error = None;
            }
            KeyCode::Up => {
                w.project_row = w.project_row.saturating_sub(1);
                w.adding = None;
            }
            KeyCode::Tab if empty => w.field.set("~/"),
            KeyCode::Tab => {
                let text = w.field.text();
                self.want_completion(text);
            }
            KeyCode::Esc if !empty => {
                w.field.set("");
                w.adding = None;
                w.field_error = None;
            }
            KeyCode::Enter if !empty => self.start_adding(),
            KeyCode::Esc | KeyCode::Enter | KeyCode::BackTab | KeyCode::Down => return None,
            KeyCode::Backspace if empty => return None,
            _ => {
                if w.field.key(&key) == KeyUse::Changed {
                    w.adding = None;
                    w.field_error = None;
                    let text = clean_folder(&w.field.text());
                    if !text.is_empty() {
                        self.want_folder(&text);
                    }
                }
            }
        }
        Some(SettingsAction::None)
    }

    /// `Enter` on a typed folder: looked at once more on the worker thread,
    /// then [`SettingsWindow::finish_adding`] takes it or says why not.
    fn start_adding(&mut self) {
        let home = self.home_dir();
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        let text = clean_folder(&w.field.text());
        w.field_error = None;
        let same = |c: &ProjectChoice| {
            paths::expand_tilde(&c.path, &home) == paths::expand_tilde(&text, &home)
        };
        if let Some(choice) = w.projects.iter_mut().find(|c| same(c)) {
            choice.ticked = true;
            w.field.set("");
            self.sync_projects();
            return;
        }
        w.adding = Some(text.clone());
        self.recheck_folder(&text);
    }

    /// The typed folder as looked at: a folder that holds git clones is
    /// scanned for them, any other is offered itself in the new-session
    /// dialog; a missing one is refused. `~` stays as typed.
    fn finish_adding(&mut self) {
        let Some(w) = self.welcome.as_ref() else {
            return;
        };
        let Some(text) = w.adding.clone() else {
            return;
        };
        let (list, error) = match self.folders.get(&text) {
            None | Some(FolderInfo::Checking) => return,
            Some(FolderInfo::Missing) => (None, Some("not found")),
            Some(FolderInfo::NotFolder) => (None, Some("not a folder")),
            Some(FolderInfo::Folder { clones, .. }) if *clones > 0 => {
                (Some(SettingKey::ProjectRoots), None)
            }
            Some(FolderInfo::Folder { .. }) => (Some(SettingKey::ExtraProjects), None),
        };
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        w.adding = None;
        w.field_error = error.map(str::to_owned);
        if let Some(list) = list {
            w.projects.push(ProjectChoice {
                path: text,
                list,
                ticked: true,
            });
            w.project_row = w.projects.len();
            w.field.set("");
            self.sync_projects();
        }
    }

    /// Takes in a finished [`crate::settings::FolderJob`]: the suggested
    /// folders that exist join the list, those holding git clones ticked in
    /// a first start's draft, and a typed folder is added or refused.
    pub(crate) fn welcome_folders_checked(&mut self, result: &FolderResult) {
        let home = self.home_dir();
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        for folder in COMMON_FOLDERS {
            let Some((_, FolderInfo::Folder { clones, .. })) =
                result.probed.iter().find(|(p, _)| p == folder)
            else {
                continue;
            };
            let expanded = paths::expand_tilde(folder, &home);
            if w.projects
                .iter()
                .any(|c| paths::expand_tilde(&c.path, &home) == expanded)
            {
                continue;
            }
            // The cursor stays on the field only while something is typed.
            let on_field = w.project_row == w.projects.len();
            w.projects.push(ProjectChoice {
                path: folder.to_owned(),
                list: SettingKey::ProjectRoots,
                ticked: w.preselect && *clones > 0,
            });
            if on_field {
                w.project_row = if w.field.text().is_empty() {
                    0
                } else {
                    w.projects.len()
                };
            }
        }
        w.scanned = true;
        self.sync_projects();
        self.finish_adding();
        if let Some(done) = &result.completed {
            self.field_completed(done);
        }
    }

    /// A finished `Tab` completion, when the field still holds its text.
    fn field_completed(&mut self, done: &Completion) {
        let Some(w) = self.welcome.as_mut() else {
            return;
        };
        if w.field.text() != done.text {
            return;
        }
        if let Some(text) = &done.result {
            w.field.set(text);
            let folder = clean_folder(text);
            self.want_folder(&folder);
        }
        match (&done.result, done.matches.len()) {
            (Some(_), 0 | 1) => {}
            (_, 0) => self.info(format!("no folder matches {}", done.text)),
            (_, n) => {
                let names = done.matches.join("  ");
                self.info(format!("{n} folders: {names}"));
            }
        }
    }

    /// What the status line says when the window closes after a failed save
    /// or on a terminal too small for it; `None` leaves what the save said.
    pub fn welcome_closing_note(&self) -> Option<String> {
        let w = self.welcome.as_ref()?;
        if let Some(err) = &w.failed {
            return Some(match w.origin {
                Origin::FirstRun if !self.session.exists() => format!(
                    "not saved ({err}); running on the built-in defaults, the welcome window \
                     comes back next time"
                ),
                _ => format!("not saved ({err}); running on the current settings"),
            });
        }
        if w.too_small {
            return Some(too_small_note(self.session.exists()));
        }
        None
    }
}

/// What the status line says when the terminal was too small for the window.
pub fn too_small_note(exists: bool) -> String {
    let (w, h) = MIN_SIZE;
    if exists {
        format!("tower setup needs a {w}x{h} terminal; nothing changed")
    } else {
        format!(
            "the welcome window needs a {w}x{h} terminal; running on the built-in defaults, \
             nothing written. It opens at the next start, or with tower setup"
        )
    }
}
