//! The new-session dialog's state and keys (doc/0008, doc/0011).
//!
//! Fields, in `Tab` order: project (a type-to-filter list), task (multi-line),
//! session name, account, model (free text with presets on `↑`/`↓`), effort
//! and permission mode. Nothing here does I/O; [`Dialog::handle_key`]
//! returns a [`DialogAction`] and the app turns it into effects
//! ([`crate::app`]).

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Account, Config};
use crate::dispatch::{
    self, BYPASS_PERMISSIONS, DEFAULT_CHOICE, DEFAULT_PERMISSION_MODE, DispatchRequest,
    EFFORT_CHOICES, MODEL_CHOICES, ValidationError,
};
use crate::projects::{self, Project, ProjectList};

/// The dialog's fields, in `Tab` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Project,
    Task,
    Name,
    Account,
    Model,
    Effort,
    Mode,
}

impl Field {
    /// All fields in `Tab` order.
    pub const ALL: [Field; 7] = [
        Field::Project,
        Field::Task,
        Field::Name,
        Field::Account,
        Field::Model,
        Field::Effort,
        Field::Mode,
    ];

    /// The label shown in front of the field.
    pub fn label(self) -> &'static str {
        match self {
            Field::Project => "Project",
            Field::Task => "Task",
            Field::Name => "Name",
            Field::Account => "Account",
            Field::Model => "Model",
            Field::Effort => "Effort",
            Field::Mode => "Mode",
        }
    }

    fn index(self) -> usize {
        Field::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// The next field, wrapping around.
    pub fn next(self) -> Field {
        Field::ALL[(self.index() + 1) % Field::ALL.len()]
    }

    /// The previous field, wrapping around.
    pub fn prev(self) -> Field {
        Field::ALL[(self.index() + Field::ALL.len() - 1) % Field::ALL.len()]
    }

    /// The field a validation error is about.
    pub fn of_error(err: &ValidationError) -> Field {
        match err {
            ValidationError::NoProject | ValidationError::ProjectMissing(_) => Field::Project,
            ValidationError::TaskTooShort { .. } => Field::Task,
            ValidationError::BadName => Field::Name,
            ValidationError::BypassAsRoot => Field::Mode,
            ValidationError::LeadingDash { field } | ValidationError::BadOption { field, .. } => {
                match *field {
                    "task" => Field::Task,
                    "session name" => Field::Name,
                    "model" => Field::Model,
                    "effort" => Field::Effort,
                    _ => Field::Mode,
                }
            }
        }
    }
}

/// A single-line text input with a cursor (a char index).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    chars: Vec<char>,
    cursor: usize,
}

impl LineInput {
    /// An input holding `text`, cursor at the end.
    pub fn new(text: &str) -> LineInput {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        LineInput { chars, cursor }
    }

    /// The text.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// The cursor as a char index.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Replaces the text, cursor at the end.
    pub fn set(&mut self, text: &str) {
        *self = LineInput::new(text);
    }

    /// Applies an editing key and reports what it did.
    pub fn key(&mut self, key: &KeyEvent) -> KeyUse {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return KeyUse::Ignored;
        }
        match key.code {
            KeyCode::Char(c) if !c.is_control() => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                KeyUse::Changed
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
                KeyUse::Changed
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
                KeyUse::Changed
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                KeyUse::Moved
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.chars.len());
                KeyUse::Moved
            }
            KeyCode::Home => {
                self.cursor = 0;
                KeyUse::Moved
            }
            KeyCode::End => {
                self.cursor = self.chars.len();
                KeyUse::Moved
            }
            KeyCode::Backspace | KeyCode::Delete => KeyUse::Moved,
            _ => KeyUse::Ignored,
        }
    }
}

/// What an editing key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyUse {
    /// Not an editing key for this input.
    Ignored,
    /// Only the cursor moved (or nothing happened at an edge).
    Moved,
    /// The text changed.
    Changed,
}

/// A multi-line text area with a cursor (`row`, char `col`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextArea {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

impl Default for TextArea {
    fn default() -> Self {
        TextArea {
            lines: vec![Vec::new()],
            row: 0,
            col: 0,
        }
    }
}

impl TextArea {
    /// A text area holding `text` (lines split on `\n`), cursor at the end.
    pub fn new(text: &str) -> TextArea {
        let lines: Vec<Vec<char>> = text
            .replace("\r\n", "\n")
            .split('\n')
            .map(|l| l.chars().collect())
            .collect();
        let row = lines.len() - 1;
        let col = lines[row].len();
        TextArea { lines, row, col }
    }

    /// The text, lines joined with `\n`.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The lines as strings.
    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// The cursor as (line, char column).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// True when the text is empty or whitespace only.
    pub fn is_blank(&self) -> bool {
        self.lines
            .iter()
            .all(|l| l.iter().all(|c| c.is_whitespace()))
    }

    /// Inserts a line break at the cursor.
    pub fn newline(&mut self) {
        let rest = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    /// Applies an editing key (not `Enter`, which the dialog handles).
    pub fn key(&mut self, key: &KeyEvent) -> KeyUse {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return KeyUse::Ignored;
        }
        match key.code {
            KeyCode::Char(c) if c == '\t' || !c.is_control() => {
                self.lines[self.row].insert(self.col, c);
                self.col += 1;
                KeyUse::Changed
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    self.col -= 1;
                    self.lines[self.row].remove(self.col);
                    KeyUse::Changed
                } else if self.row > 0 {
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.lines[self.row].len();
                    self.lines[self.row].extend(line);
                    KeyUse::Changed
                } else {
                    KeyUse::Moved
                }
            }
            KeyCode::Delete => {
                if self.col < self.lines[self.row].len() {
                    self.lines[self.row].remove(self.col);
                    KeyUse::Changed
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].extend(next);
                    KeyUse::Changed
                } else {
                    KeyUse::Moved
                }
            }
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.lines[self.row].len();
                }
                KeyUse::Moved
            }
            KeyCode::Right => {
                if self.col < self.lines[self.row].len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
                KeyUse::Moved
            }
            KeyCode::Up => {
                if self.row > 0 {
                    self.row -= 1;
                    self.col = self.col.min(self.lines[self.row].len());
                } else {
                    self.col = 0;
                }
                KeyUse::Moved
            }
            KeyCode::Down => {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = self.col.min(self.lines[self.row].len());
                } else {
                    self.col = self.lines[self.row].len();
                }
                KeyUse::Moved
            }
            KeyCode::Home => {
                self.col = 0;
                KeyUse::Moved
            }
            KeyCode::End => {
                self.col = self.lines[self.row].len();
                KeyUse::Moved
            }
            _ => KeyUse::Ignored,
        }
    }
}

/// A choice among fixed values, changed with `←`/`→` or `↑`/`↓` (wrapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    items: Vec<String>,
    index: usize,
}

impl Choice {
    /// A choice of `items` with `value` selected; a `value` not among them is
    /// added at the end and selected, so a config default is never lost.
    pub fn new<S: Into<String>>(items: impl IntoIterator<Item = S>, value: &str) -> Choice {
        let mut items: Vec<String> = items.into_iter().map(Into::into).collect();
        let index = match items.iter().position(|i| i == value) {
            Some(i) => i,
            None => {
                items.push(value.to_owned());
                items.len() - 1
            }
        };
        Choice { items, index }
    }

    /// The selected value.
    pub fn value(&self) -> &str {
        &self.items[self.index]
    }

    /// The selected index.
    pub fn index(&self) -> usize {
        self.index
    }

    /// All values.
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Selects the next value, wrapping.
    pub fn next(&mut self) {
        self.index = (self.index + 1) % self.items.len();
    }

    /// Selects the previous value, wrapping.
    pub fn prev(&mut self) {
        self.index = (self.index + self.items.len() - 1) % self.items.len();
    }

    fn key(&mut self, key: &KeyEvent) -> KeyUse {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return KeyUse::Ignored;
        }
        match key.code {
            KeyCode::Right | KeyCode::Down | KeyCode::Char(' ') => {
                self.next();
                KeyUse::Changed
            }
            KeyCode::Left | KeyCode::Up => {
                self.prev();
                KeyUse::Changed
            }
            _ => KeyUse::Ignored,
        }
    }
}

/// What the dialog is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogStatus {
    /// The user is filling in the fields.
    Editing,
    /// A dispatch runs on a worker thread; the fields are kept but locked.
    Starting {
        /// Whether tower attaches once it has started.
        attach: bool,
    },
    /// The last dispatch failed with `Workspace not trusted`; `Ctrl+T` runs
    /// the account command in `dir` so the user can accept trust.
    Untrusted {
        /// The folder to trust.
        dir: PathBuf,
        /// Whether the dispatch that failed was dispatch-and-attach.
        attach: bool,
    },
}

/// A yes/no question asked inside the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogPrompt {
    /// `Esc` with a non-empty task: discard it?
    Discard,
    /// `Ctrl+C` while the dialog is open: quit tower? (doc/0011)
    Quit,
}

/// What a key in the dialog asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    /// Nothing beyond the dialog's own state change.
    None,
    /// Close the dialog without starting anything.
    Cancel,
    /// Quit tower.
    Quit,
    /// Validate and dispatch; `attach` for dispatch-and-attach.
    Dispatch { attach: bool },
    /// Open the task in the editor.
    EditTask,
    /// Run the trust flow for the untrusted folder, then retry.
    Trust,
}

/// The new-session dialog (doc/0008).
#[derive(Debug, Clone)]
pub struct Dialog {
    /// The focused field.
    pub field: Field,
    /// Candidate projects, in display order.
    pub projects: Vec<Project>,
    /// Non-fatal problems from the project scan.
    pub project_warnings: Vec<String>,
    /// The project scan still runs on a worker thread; the list is empty
    /// until it reports back ([`Dialog::set_projects`]).
    pub scanning: bool,
    /// The project filter text.
    pub filter: LineInput,
    /// Index into [`Dialog::filtered`] of the chosen project.
    pub project_index: usize,
    /// The task.
    pub task: TextArea,
    /// The optional session name.
    pub name: LineInput,
    /// Account names; index matches the configured accounts.
    pub account: Choice,
    /// The model: free text; `↑`/`↓` step through the presets.
    pub model: LineInput,
    /// The effort.
    pub effort: Choice,
    /// The permission mode.
    pub mode: Choice,
    /// A validation or dispatch error, shown until the next attempt.
    pub error: Option<String>,
    /// What the dialog is doing.
    pub status: DialogStatus,
    /// An open yes/no question.
    pub prompt: Option<DialogPrompt>,
}

impl Dialog {
    /// A fresh dialog with the config's defaults (doc/0008): the default
    /// account, `default_model`, `default_effort` and
    /// `default_permission_mode`. `root` hides `bypassPermissions`.
    pub fn new(config: &Config, projects: ProjectList, root: bool) -> Dialog {
        let names: Vec<String> = config.accounts.iter().map(|a| a.name.clone()).collect();
        let account = Choice::new(names, &config.default_account);
        let effort = Choice::new(EFFORT_CHOICES, config.default_effort.trim());
        let modes = dispatch::permission_modes(root);
        let wanted = config.default_permission_mode.trim();
        let mode_default = if root && wanted == BYPASS_PERMISSIONS {
            DEFAULT_PERMISSION_MODE
        } else {
            wanted
        };
        let mode = Choice::new(modes, mode_default);
        Dialog {
            field: Field::Project,
            projects: projects.projects,
            project_warnings: projects.warnings,
            scanning: false,
            filter: LineInput::default(),
            project_index: 0,
            task: TextArea::default(),
            name: LineInput::default(),
            account,
            model: LineInput::new(config.default_model.trim()),
            effort,
            mode,
            error: None,
            status: DialogStatus::Editing,
            prompt: None,
        }
    }

    /// Takes the result of the project scan; the filter typed meanwhile is
    /// kept and the choice is clamped to the matching projects.
    pub fn set_projects(&mut self, projects: ProjectList) {
        self.projects = projects.projects;
        self.project_warnings = projects.warnings;
        self.scanning = false;
        let n = self.filtered().len();
        self.project_index = self.project_index.min(n.saturating_sub(1));
    }

    /// The projects matching the filter, in display order.
    pub fn filtered(&self) -> Vec<&Project> {
        projects::filter(&self.projects, &self.filter.text())
    }

    /// The chosen project, if any matches the filter.
    pub fn selected_project(&self) -> Option<&Project> {
        self.filtered().get(self.project_index).copied()
    }

    /// The Haiku warning for the current model and mode (doc/0008).
    pub fn haiku_warning(&self) -> Option<&'static str> {
        let model = self.model.text();
        let model = model.trim();
        let model = (!model.is_empty() && model != DEFAULT_CHOICE).then_some(model);
        dispatch::haiku_warning(model, self.mode.value())
    }

    /// True while a dispatch is running.
    pub fn is_starting(&self) -> bool {
        matches!(self.status, DialogStatus::Starting { .. })
    }

    /// The request the fields describe, for `accounts` (the configured
    /// accounts; the account choice indexes them). Not validated.
    pub fn request(&self, accounts: &[Account]) -> DispatchRequest {
        let project = self
            .selected_project()
            .map(|p| p.path.clone())
            .unwrap_or_default();
        let account = accounts
            .get(self.account.index())
            .or_else(|| accounts.first())
            .cloned()
            .expect("at least one account is configured");
        let mut req = DispatchRequest::new(project, self.task.text(), account);
        let name = self.name.text();
        req.name = (!name.trim().is_empty()).then_some(name);
        req.model = Some(self.model.text());
        req.effort = Some(self.effort.value().to_owned());
        req.permission_mode = self.mode.value().to_owned();
        req
    }

    /// Replaces the task text (after the editor returns).
    pub fn set_task(&mut self, text: &str) {
        self.task = TextArea::new(text);
        self.field = Field::Task;
    }

    /// Handles one key.
    pub fn handle_key(&mut self, key: KeyEvent) -> DialogAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if let Some(prompt) = self.prompt {
            return self.prompt_key(prompt, &key);
        }
        if ctrl && matches!(key.code, KeyCode::Char('c' | 'C')) {
            self.prompt = Some(DialogPrompt::Quit);
            return DialogAction::None;
        }
        if self.is_starting() {
            // The fields stay as they are until the dispatch reports back.
            return DialogAction::None;
        }
        match key.code {
            KeyCode::Esc => {
                if self.task.is_blank() {
                    return DialogAction::Cancel;
                }
                self.prompt = Some(DialogPrompt::Discard);
                return DialogAction::None;
            }
            KeyCode::Enter if ctrl || alt => return DialogAction::Dispatch { attach: true },
            KeyCode::Char('s' | 'S') if ctrl => return DialogAction::Dispatch { attach: false },
            KeyCode::Char('g' | 'G') if ctrl => return DialogAction::EditTask,
            KeyCode::Char('t' | 'T') if ctrl => {
                if matches!(self.status, DialogStatus::Untrusted { .. }) {
                    return DialogAction::Trust;
                }
                return DialogAction::None;
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.field = self.field.prev();
                return DialogAction::None;
            }
            KeyCode::BackTab => {
                self.field = self.field.prev();
                return DialogAction::None;
            }
            KeyCode::Tab => {
                self.field = self.field.next();
                return DialogAction::None;
            }
            KeyCode::Enter => {
                if self.field == Field::Task {
                    self.task.newline();
                } else {
                    self.field = self.field.next();
                }
                return DialogAction::None;
            }
            // A line feed: some terminals send it for Ctrl+Enter, and pasted
            // text may contain it. Only ever a new line, never a dispatch.
            KeyCode::Char('j') if ctrl && self.field == Field::Task => {
                self.task.newline();
                return DialogAction::None;
            }
            _ => {}
        }
        self.field_key(&key);
        DialogAction::None
    }

    fn prompt_key(&mut self, prompt: DialogPrompt, key: &KeyEvent) -> DialogAction {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('y' | 'Y') if plain => {
                self.prompt = None;
                match prompt {
                    DialogPrompt::Discard => DialogAction::Cancel,
                    DialogPrompt::Quit => DialogAction::Quit,
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                self.prompt = None;
                DialogAction::None
            }
            _ => DialogAction::None,
        }
    }

    fn field_key(&mut self, key: &KeyEvent) {
        match self.field {
            Field::Project => self.project_key(key),
            Field::Task => {
                self.task.key(key);
            }
            Field::Name => {
                self.name.key(key);
            }
            Field::Account => {
                if self.account.key(key) == KeyUse::Changed {
                    self.untrusted_no_longer_applies();
                }
            }
            Field::Model => self.model_key(key),
            Field::Effort => {
                self.effort.key(key);
            }
            Field::Mode => {
                self.mode.key(key);
            }
        }
    }

    fn project_key(&mut self, key: &KeyEvent) {
        let n = self.filtered().len();
        let before = self.selected_project().map(|p| p.path.clone());
        match key.code {
            KeyCode::Up => self.project_index = self.project_index.saturating_sub(1),
            KeyCode::Down => self.project_index = (self.project_index + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.project_index = self.project_index.saturating_sub(5),
            KeyCode::PageDown => {
                self.project_index = (self.project_index + 5).min(n.saturating_sub(1))
            }
            _ => {
                if self.filter.key(key) == KeyUse::Changed {
                    self.project_index = 0;
                }
            }
        }
        let n = self.filtered().len();
        self.project_index = self.project_index.min(n.saturating_sub(1));
        if self.selected_project().map(|p| p.path.clone()) != before {
            self.untrusted_no_longer_applies();
        }
    }

    fn model_key(&mut self, key: &KeyEvent) {
        let current = self.model.text();
        let pos = MODEL_CHOICES.iter().position(|m| *m == current.trim());
        let n = MODEL_CHOICES.len();
        match key.code {
            KeyCode::Down if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let i = pos.map_or(0, |i| (i + 1) % n);
                self.model.set(MODEL_CHOICES[i]);
            }
            KeyCode::Up if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let i = pos.map_or(n - 1, |i| (i + n - 1) % n);
                self.model.set(MODEL_CHOICES[i]);
            }
            _ => {
                self.model.key(key);
            }
        }
    }

    /// The trust offer is for one folder and one account; changing either
    /// withdraws it.
    fn untrusted_no_longer_applies(&mut self) {
        if matches!(self.status, DialogStatus::Untrusted { .. }) {
            self.status = DialogStatus::Editing;
            self.error = None;
        }
    }
}
