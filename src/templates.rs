//! Prompt templates (doc/0036): plain files in `<config dir>/templates`, one
//! per template, and the reusable picker that inserts one into a text field.
//!
//! **Loading** ([`load`], I/O, run on a worker thread through
//! [`TemplateLoad`]): the `*.md` files directly in the folder, no
//! recursion, sorted by name ignoring case. The name is the file stem, the
//! text the content with trailing whitespace trimmed. Hidden files are
//! ignored; symlinks (never followed), other non-regular files, files over
//! [`MAX_TEMPLATE_BYTES`], empty files and non-UTF-8 files are skipped and
//! listed in [`TemplateList::skipped`], which [`TemplateList::skipped_note`]
//! sums up for the status line. A missing folder means no templates and is
//! created empty when its parent exists. Tower never creates, edits or
//! deletes a template file.
//!
//! **Picker** ([`TemplatePicker`], no I/O): a window that owns no text field
//! of its own. The window that hosts it (the new-session dialog today, a
//! quick reply later) uses it like this:
//!
//! 1. Create it with [`TemplatePicker::new`] (it starts loading) and ask the
//!    runtime to run a [`TemplateLoad`] for it
//!    (`App::new_template_picker` does both and returns the
//!    `Effect::LoadTemplates`).
//! 2. Hand the finished load to it with [`TemplatePicker::set_list`] when
//!    [`TemplatePicker::load_id`] matches (`App::templates_loaded`).
//! 3. While it is open, give it every key ([`TemplatePicker::handle_key`])
//!    and act on the [`PickerAction`]: `Pick(text)` inserts the text into
//!    the host's field and closes the picker, `Close` closes it, `Quit` is
//!    the host's `Ctrl+C`.
//! 4. Draw it over the host with `ui::templates::draw_picker`.

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::dialog::{KeyUse, LineInput};
use crate::layout;

/// The folder under tower's config dir that holds the templates.
pub const TEMPLATES_DIR: &str = "templates";

/// Template files larger than this (64 KiB) are skipped.
pub const MAX_TEMPLATE_BYTES: u64 = 64 * 1024;

/// The templates folder of tower's config dir (`<config dir>/templates`).
pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join(TEMPLATES_DIR)
}

/// One template: a file `<name>.md` and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The file stem.
    pub name: String,
    /// The content, trailing whitespace and newlines trimmed; never empty.
    pub text: String,
}

impl Template {
    /// The first non-blank line of the text, trimmed: the picker's preview.
    pub fn preview(&self) -> &str {
        self.text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
    }
}

/// Why a `*.md` file in the folder is not offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// A symbolic link; tower does not follow links inside the folder.
    Symlink,
    /// A folder, device, FIFO or socket.
    NotRegular,
    /// Larger than [`MAX_TEMPLATE_BYTES`].
    TooLarge,
    /// Empty, or whitespace only.
    Empty,
    /// The content or the file name is not UTF-8.
    NotUtf8,
    /// It could not be read.
    Unreadable(String),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::Symlink => f.write_str("a symlink, not followed"),
            SkipReason::NotRegular => f.write_str("not a regular file"),
            SkipReason::TooLarge => f.write_str("over 64 KiB"),
            SkipReason::Empty => f.write_str("empty"),
            SkipReason::NotUtf8 => f.write_str("not UTF-8"),
            SkipReason::Unreadable(err) => write!(f, "cannot read it: {err}"),
        }
    }
}

/// A `*.md` file that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The file name (lossy when not UTF-8).
    pub file: String,
    /// Why.
    pub reason: SkipReason,
}

/// What a load found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateList {
    /// The templates, sorted by name ignoring case.
    pub templates: Vec<Template>,
    /// The `*.md` files skipped, sorted by file name.
    pub skipped: Vec<Skipped>,
    /// The folder exists but could not be listed.
    pub error: Option<String>,
}

impl TemplateList {
    /// The status-line note for the skipped files, once per load: `None`
    /// when nothing was skipped.
    pub fn skipped_note(&self) -> Option<String> {
        let first = self.skipped.first()?;
        Some(match self.skipped.len() {
            1 => format!("skipped template {}: {}", first.file, first.reason),
            n => format!(
                "skipped {n} template files: {} ({}) and {} more",
                first.file,
                first.reason,
                n - 1
            ),
        })
    }
}

/// Lists the templates in `folder` (see the module docs for the rules). A
/// missing folder yields an empty list without an error; it is created
/// empty (mode `0700`) when its parent exists, so the user can put files
/// there straight away. A failed creation changes nothing.
pub fn load(folder: &Path) -> TemplateList {
    let mut list = TemplateList::default();
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = std::fs::DirBuilder::new().mode(0o700).create(folder);
            return list;
        }
        Err(err) => {
            list.error = Some(format!("cannot read {}: {err}", folder.display()));
            return list;
        }
    };
    for entry in entries.flatten() {
        let raw = entry.file_name();
        let lossy = raw.to_string_lossy().into_owned();
        if lossy.starts_with('.') || !lossy.ends_with(".md") || lossy.len() <= ".md".len() {
            continue;
        }
        let skip = |reason| Skipped {
            file: lossy.clone(),
            reason,
        };
        let Some(file) = raw.to_str() else {
            list.skipped.push(skip(SkipReason::NotUtf8));
            continue;
        };
        let name = file[..file.len() - ".md".len()].to_owned();
        match entry.file_type() {
            Ok(t) if t.is_symlink() => {
                list.skipped.push(skip(SkipReason::Symlink));
                continue;
            }
            Ok(t) if !t.is_file() => {
                list.skipped.push(skip(SkipReason::NotRegular));
                continue;
            }
            Ok(_) => {}
            Err(err) => {
                list.skipped
                    .push(skip(SkipReason::Unreadable(err.to_string())));
                continue;
            }
        }
        match read_text(&entry.path()) {
            Ok(text) => list.templates.push(Template { name, text }),
            Err(reason) => list.skipped.push(skip(reason)),
        }
    }
    list.templates.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    list.skipped.sort_by(|a, b| a.file.cmp(&b.file));
    list
}

/// Reads one template file: never through a symlink (`O_NOFOLLOW`), never
/// blocking on a FIFO (`O_NONBLOCK`), at most one byte over the limit.
fn read_text(path: &Path) -> Result<String, SkipReason> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|err| match err.raw_os_error() {
            Some(libc::ELOOP) => SkipReason::Symlink,
            _ => SkipReason::Unreadable(err.to_string()),
        })?;
    let meta = file
        .metadata()
        .map_err(|err| SkipReason::Unreadable(err.to_string()))?;
    if !meta.is_file() {
        return Err(SkipReason::NotRegular);
    }
    if meta.len() > MAX_TEMPLATE_BYTES {
        return Err(SkipReason::TooLarge);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_TEMPLATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| SkipReason::Unreadable(err.to_string()))?;
    if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err(SkipReason::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| SkipReason::NotUtf8)?;
    let text = text.trim_end();
    if text.is_empty() {
        return Err(SkipReason::Empty);
    }
    Ok(text.to_owned())
}

/// A load of the templates folder for the picker `id`, run on a worker
/// thread; the result goes to `App::templates_loaded`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateLoad {
    /// The picker the load is for ([`TemplatePicker::load_id`]).
    pub id: u64,
    /// The templates folder.
    pub folder: PathBuf,
}

impl TemplateLoad {
    /// Runs the load.
    pub fn run(&self) -> TemplateList {
        load(&self.folder)
    }
}

/// What a key in the picker asks its host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    /// Nothing beyond the picker's own state change.
    None,
    /// `Esc`: close the picker, leaving the host's field as it was.
    Close,
    /// `Enter` on a template: insert this text and close the picker.
    Pick(String),
    /// `Ctrl+C`: close the picker and treat it as the host's `Ctrl+C`.
    Quit,
}

/// The template picker (doc/0036): the names with a preview, a filter typed
/// into it, `↑` / `↓` to choose, `Enter` to pick, `Esc` to close. See the
/// module docs for how a window hosts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePicker {
    id: u64,
    folder: PathBuf,
    shown_folder: String,
    list: Option<TemplateList>,
    filter: LineInput,
    selected: usize,
}

/// At most this many rows a page key moves.
const PAGE: usize = 5;

impl TemplatePicker {
    /// A picker for load `id` of `folder`, still loading; `home` shortens
    /// the folder to `~/…` for display.
    pub fn new(id: u64, folder: PathBuf, home: &Path) -> TemplatePicker {
        let shown_folder = crate::paths::shorten_home(&folder.to_string_lossy(), home);
        TemplatePicker {
            id,
            folder,
            shown_folder,
            list: None,
            filter: LineInput::default(),
            selected: 0,
        }
    }

    /// The load this picker waits for.
    pub fn load_id(&self) -> u64 {
        self.id
    }

    /// The templates folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The templates folder for display (`~/…`).
    pub fn shown_folder(&self) -> &str {
        &self.shown_folder
    }

    /// True until [`TemplatePicker::set_list`].
    pub fn is_loading(&self) -> bool {
        self.list.is_none()
    }

    /// Takes the finished load; the filter typed meanwhile is kept.
    pub fn set_list(&mut self, list: TemplateList) {
        self.list = Some(list);
        self.clamp();
    }

    /// What the load found, once it has.
    pub fn list(&self) -> Option<&TemplateList> {
        self.list.as_ref()
    }

    /// Every template (none while loading).
    pub fn templates(&self) -> &[Template] {
        self.list.as_ref().map_or(&[], |l| &l.templates)
    }

    /// The filter typed so far.
    pub fn filter(&self) -> &LineInput {
        &self.filter
    }

    /// The templates whose name contains every word of the filter,
    /// ignoring case, in name order.
    pub fn filtered(&self) -> Vec<&Template> {
        let query = self.filter.text().to_lowercase();
        self.templates()
            .iter()
            .filter(|t| {
                let name = t.name.to_lowercase();
                query.split_whitespace().all(|w| name.contains(w))
            })
            .collect()
    }

    /// The index into [`TemplatePicker::filtered`] of the chosen template.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The chosen template, if any matches.
    pub fn selected(&self) -> Option<&Template> {
        self.filtered().get(self.selected).copied()
    }

    /// The note shown instead of the list when nothing is listed: loading,
    /// a folder that cannot be read, no templates at all (naming the
    /// folder), or none matching the filter. `None` while a template is
    /// listed.
    pub fn empty_text(&self) -> Option<String> {
        let Some(list) = &self.list else {
            return Some("loading templates…".to_owned());
        };
        if let Some(err) = &list.error {
            return Some(err.clone());
        }
        if list.templates.is_empty() {
            return Some(format!(
                "No templates yet: put .md files in {}",
                self.shown_folder
            ));
        }
        self.filtered()
            .is_empty()
            .then(|| "no template matches the filter".to_owned())
    }

    /// Handles one key: typed text filters (never translated from a
    /// Cyrillic layout), `Backspace` / `Del` / `←` / `→` / `Home` / `End`
    /// edit it and `Ctrl+U` empties it, `↑` / `↓` (`PgUp` / `PgDn`)
    /// choose, `Enter` picks, `Esc` closes, `Ctrl+C` quits.
    pub fn handle_key(&mut self, key: KeyEvent) -> PickerAction {
        let key = layout::shortcut_key(key);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.filtered().len();
        match key.code {
            KeyCode::Char('c' | 'C') if ctrl => return PickerAction::Quit,
            KeyCode::Char('u' | 'U') if ctrl => {
                self.filter.set("");
                self.selected = 0;
            }
            KeyCode::Esc => return PickerAction::Close,
            KeyCode::Enter => {
                return match self.selected() {
                    Some(t) => PickerAction::Pick(t.text.clone()),
                    None => PickerAction::None,
                };
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(n.saturating_sub(1)),
            _ => {
                if self.filter.key(&key) == KeyUse::Changed {
                    self.selected = 0;
                }
            }
        }
        self.clamp();
        PickerAction::None
    }

    fn clamp(&mut self) {
        let n = self.filtered().len();
        self.selected = self.selected.min(n.saturating_sub(1));
    }
}
