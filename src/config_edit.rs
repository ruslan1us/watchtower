//! Editing tower's own `config.toml` for the Settings window (doc/0022).
//!
//! The pieces:
//!
//! - [`SettingKey`]: the editable settings, each with its key path, section,
//!   label, help line and the kind of value it takes ([`SettingKind`]).
//!   Accounts are never edited; [`EditSession::accounts`] shows them.
//! - [`EditSession`]: the file as the window opened it, plus the changes
//!   made since ([`EditSession::set`], [`EditSession::reset_to_default`],
//!   [`EditSession::revert`]). [`EditSession::settings`] gives one
//!   [`Setting`] row per key for the UI, [`EditSession::validate`] the
//!   config the changes would give.
//! - Saving ([`EditSession::save`], or [`EditSession::save_job`] on a worker
//!   thread and [`EditSession::finish_save`] with its result): re-reads the
//!   file under `config.toml.lock`, reports a [`Conflict`] when a key this
//!   window changed was changed on disk meanwhile (unless forced), edits
//!   the text with `toml_edit` so comments, blank lines and key order stay,
//!   validates the result with [`Config::parse`], makes a one-time
//!   `config.toml.bak` before tower's first write, and writes atomically to
//!   the symlink's target with the file's permissions kept. A missing file
//!   is created with [`HEADER`] and only the changed keys.
//! - Reloading ([`ConfigCheck`], [`read_config`]): a running tower looks at
//!   the file's [`FileStamp`] at each poll and re-reads it when it changed.
//! - [`edit_in_editor`]: the file in `$VISUAL` / `$EDITOR`, created first
//!   when missing, read back and validated afterwards.
//!
//! Nothing here touches any Claude Code file (doc/0016).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};

use crate::attach::{self, Foreground, Handover, Spawner, Termination};
use crate::cli::CommandLine;
use crate::config::{
    Account, CardSize, ColorDepth, Config, ConfigError, DEFAULT_LOGS_FOLLOW_SECS,
    DEFAULT_POLL_INTERVAL_SECS, LOGS_FOLLOW_RANGE, LOGS_FOLLOW_STEPS, POLL_INTERVAL_RANGE, UiStyle,
    UiTheme,
};
use crate::dispatch::{
    BYPASS_PERMISSIONS, DEFAULT_CHOICE, DEFAULT_PERMISSION_MODE, EFFORT_CHOICES, MODEL_CHOICES,
    PERMISSION_MODES,
};
use crate::sidecar::{self, FileStamp};
use crate::usage;

/// The comment at the top of a config file tower creates.
pub const HEADER: &str = "\
# Watchtower (tower) config, created by tower.
#
# Every key is optional: a key that is not here has its built-in default.
# config.example.toml in tower's repository lists every key with its
# default. Tower's Settings window edits this file in place and keeps
# comments and formatting (doc/0002, doc/0022).
";

/// How long a save waits for `config.toml.lock` before it gives up.
pub const LOCK_WAIT: Duration = sidecar::LOCK_WAIT;

/// The sections of the Settings window, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    /// `[ui] style` and `[ui] theme`.
    Appearance,
    /// The new-session dialog's defaults.
    NewSessions,
    /// `project_roots` and `extra_projects`.
    Projects,
    /// The hand-off prompt and guard.
    Handoff,
    /// The poll interval, the logs-follow interval and subagent tracking.
    General,
    /// The configured accounts; read-only.
    Accounts,
}

impl Section {
    /// Every section, in display order.
    pub const ALL: [Section; 6] = [
        Section::Appearance,
        Section::NewSessions,
        Section::Projects,
        Section::Handoff,
        Section::General,
        Section::Accounts,
    ];

    /// The section's title.
    pub fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::NewSessions => "New sessions",
            Section::Projects => "Projects",
            Section::Handoff => "Hand-off",
            Section::General => "General",
            Section::Accounts => "Accounts",
        }
    }
}

/// One editable setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingKey {
    UiStyle,
    UiTheme,
    UiColorDepth,
    UiCardSize,
    DefaultAccount,
    DefaultModel,
    DefaultEffort,
    DefaultPermissionMode,
    ProjectRoots,
    ExtraProjects,
    HandoffPrompt,
    AllowCrossGroupHandoff,
    PollIntervalSecs,
    TrackSubagents,
    LogsFollowSecs,
    UsageShow,
    UsageRefreshSecs,
}

impl SettingKey {
    /// Every setting, in display order.
    pub const ALL: [SettingKey; 17] = [
        SettingKey::UiStyle,
        SettingKey::UiTheme,
        SettingKey::UiColorDepth,
        SettingKey::UiCardSize,
        SettingKey::DefaultAccount,
        SettingKey::DefaultModel,
        SettingKey::DefaultEffort,
        SettingKey::DefaultPermissionMode,
        SettingKey::ProjectRoots,
        SettingKey::ExtraProjects,
        SettingKey::HandoffPrompt,
        SettingKey::AllowCrossGroupHandoff,
        SettingKey::PollIntervalSecs,
        SettingKey::LogsFollowSecs,
        SettingKey::TrackSubagents,
        SettingKey::UsageShow,
        SettingKey::UsageRefreshSecs,
    ];

    /// The key's path in the file: `["ui", "style"]`, `["default_model"]`.
    pub fn path(self) -> &'static [&'static str] {
        match self {
            SettingKey::UiStyle => &["ui", "style"],
            SettingKey::UiTheme => &["ui", "theme"],
            SettingKey::UiColorDepth => &["ui", "color_depth"],
            SettingKey::UiCardSize => &["ui", "card_size"],
            SettingKey::DefaultAccount => &["default_account"],
            SettingKey::DefaultModel => &["default_model"],
            SettingKey::DefaultEffort => &["default_effort"],
            SettingKey::DefaultPermissionMode => &["default_permission_mode"],
            SettingKey::ProjectRoots => &["project_roots"],
            SettingKey::ExtraProjects => &["extra_projects"],
            SettingKey::HandoffPrompt => &["handoff_prompt"],
            SettingKey::AllowCrossGroupHandoff => &["allow_cross_group_handoff"],
            SettingKey::PollIntervalSecs => &["poll_interval_secs"],
            SettingKey::LogsFollowSecs => &["logs_follow_secs"],
            SettingKey::TrackSubagents => &["sessions", "track_subagents"],
            SettingKey::UsageShow => &["usage", "show"],
            SettingKey::UsageRefreshSecs => &["usage", "refresh_secs"],
        }
    }

    /// The key as the docs write it: `[ui] style`, `default_model`.
    pub fn toml_key(self) -> String {
        match self.path() {
            [table, key] => format!("[{table}] {key}"),
            path => path.join("."),
        }
    }

    /// The section the setting is shown in.
    pub fn section(self) -> Section {
        match self {
            SettingKey::UiStyle
            | SettingKey::UiTheme
            | SettingKey::UiColorDepth
            | SettingKey::UiCardSize => Section::Appearance,
            SettingKey::DefaultAccount
            | SettingKey::DefaultModel
            | SettingKey::DefaultEffort
            | SettingKey::DefaultPermissionMode => Section::NewSessions,
            SettingKey::ProjectRoots | SettingKey::ExtraProjects => Section::Projects,
            SettingKey::HandoffPrompt | SettingKey::AllowCrossGroupHandoff => Section::Handoff,
            SettingKey::PollIntervalSecs
            | SettingKey::LogsFollowSecs
            | SettingKey::TrackSubagents
            | SettingKey::UsageShow
            | SettingKey::UsageRefreshSecs => Section::General,
        }
    }

    /// The setting's label in the window.
    pub fn label(self) -> &'static str {
        match self {
            SettingKey::UiStyle => "style",
            SettingKey::UiTheme => "theme",
            SettingKey::UiColorDepth => "colour depth",
            SettingKey::UiCardSize => "card size",
            SettingKey::DefaultAccount => "default account",
            SettingKey::DefaultModel => "default model",
            SettingKey::DefaultEffort => "default effort",
            SettingKey::DefaultPermissionMode => "default permission mode",
            SettingKey::ProjectRoots => "project roots",
            SettingKey::ExtraProjects => "extra projects",
            SettingKey::HandoffPrompt => "hand-off prompt",
            SettingKey::AllowCrossGroupHandoff => "allow cross-group hand-off",
            SettingKey::PollIntervalSecs => "poll interval",
            SettingKey::LogsFollowSecs => "follow logs in background",
            SettingKey::TrackSubagents => "track subagents",
            SettingKey::UsageShow => "show plan usage",
            SettingKey::UsageRefreshSecs => "usage refresh",
        }
    }

    /// One line of help for the detail panel or help strip.
    pub fn help(self) -> &'static str {
        match self {
            SettingKey::UiStyle => {
                "The look of the session list: rounded panels (dashboard), the plain list \
                 (minimal), or the list beside Claude's real terminal (workspace, experimental)."
            }
            SettingKey::UiTheme => {
                "The dashboard's colours; the minimal style keeps its own, and NO_COLOR turns \
                 colour off."
            }
            SettingKey::UiColorDepth => {
                "The colours the terminal shows. auto reads COLORTERM and TERM; at 256 the \
                 truecolor themes take the nearest of 256 colours, at 16 they give way to ansi16."
            }
            SettingKey::UiCardSize => {
                "The workspace style's cards: large (4 rows), medium (3 rows, name, state and \
                 folder, account and age) or small (2 rows without an outline)."
            }
            SettingKey::DefaultAccount => {
                "The account new sessions and the orchestrator start under."
            }
            SettingKey::DefaultModel => {
                "The new-session dialog's model; \"default\" passes no --model. A full model \
                 name works too."
            }
            SettingKey::DefaultEffort => {
                "The new-session dialog's effort; \"default\" passes no --effort."
            }
            SettingKey::DefaultPermissionMode => {
                "The new-session dialog's --permission-mode. bypassPermissions is chosen per \
                 session only."
            }
            SettingKey::ProjectRoots => {
                "Folders scanned one level deep for git clones in the new-session dialog."
            }
            SettingKey::ExtraProjects => {
                "Folders listed in the new-session dialog even if not under a root or not a git \
                 clone."
            }
            SettingKey::HandoffPrompt => {
                "The prompt sent as the next turn when a session continues under another \
                 account. Empty: no prompt; the session waits for you there."
            }
            SettingKey::AllowCrossGroupHandoff => {
                "Let a hand-off move a session to an account of another group, under that \
                 account's terms."
            }
            SettingKey::PollIntervalSecs => "Seconds between polls of every account's list.",
            SettingKey::LogsFollowSecs => {
                "How often working and attached sessions' logs are fetched in the background, so \
                 the logs panel keeps the whole screen. At most 30 s: a capture covers only ~40 s. \
                 Off: only the open panel's polls and one fetch after a start or attach."
            }
            SettingKey::TrackSubagents => {
                "List the subagents of sessions tower starts, through observe-only hooks passed \
                 with --settings."
            }
            SettingKey::UsageShow => {
                "Show each account's plan usage as Claude Code's /usage reports it, for your \
                 information only. Runs /usage with --safe-mode: no model turn, no hooks."
            }
            SettingKey::UsageRefreshSecs => {
                "How often each account's /usage is asked. r also refreshes values older than a \
                 minute."
            }
        }
    }

    /// When a saved change takes effect in a running tower.
    pub fn applies(self) -> &'static str {
        match self {
            SettingKey::UiStyle
            | SettingKey::UiTheme
            | SettingKey::UiColorDepth
            | SettingKey::UiCardSize => "at once",
            SettingKey::DefaultAccount => {
                "the next new session; the orchestrator on its next start"
            }
            SettingKey::DefaultModel
            | SettingKey::DefaultEffort
            | SettingKey::DefaultPermissionMode => "the next new-session dialog",
            SettingKey::ProjectRoots | SettingKey::ExtraProjects => {
                "the next new-session dialog, which rescans the projects"
            }
            SettingKey::HandoffPrompt | SettingKey::AllowCrossGroupHandoff => "the next hand-off",
            SettingKey::PollIntervalSecs => "from the next poll",
            SettingKey::LogsFollowSecs => "at once",
            SettingKey::TrackSubagents => {
                "sessions started from now on; the list from the next poll"
            }
            SettingKey::UsageShow => "at once",
            SettingKey::UsageRefreshSecs => "from the next probe",
        }
    }

    /// Whether the window previews the setting while it is picked
    /// (doc/0024): the style, the theme and the colour depth.
    pub fn previewed(self) -> bool {
        matches!(
            self,
            SettingKey::UiStyle | SettingKey::UiTheme | SettingKey::UiColorDepth
        )
    }

    /// The type of the value in the file.
    fn shape(self) -> Shape {
        match self {
            SettingKey::ProjectRoots | SettingKey::ExtraProjects => Shape::Paths,
            SettingKey::AllowCrossGroupHandoff
            | SettingKey::TrackSubagents
            | SettingKey::UsageShow => Shape::Bool,
            SettingKey::PollIntervalSecs
            | SettingKey::LogsFollowSecs
            | SettingKey::UsageRefreshSecs => Shape::Int,
            _ => Shape::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Text,
    Bool,
    Int,
    Paths,
}

/// What a setting takes, for the editor the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingKind {
    /// One of `values` (a pick list). With `free_text`, any other
    /// non-blank word is accepted too (a full model name).
    Choice {
        values: Vec<String>,
        free_text: bool,
    },
    /// On or off.
    Bool,
    /// A whole number from `min` to `max`.
    Int { min: i64, max: i64 },
    /// A number of seconds stepped through `values` like a choice; 0 is
    /// off, any other value is `min` to `max` (a hand-edited file may hold
    /// one that is not a step).
    Steps {
        values: Vec<i64>,
        min: i64,
        max: i64,
    },
    /// Free text (the hand-off prompt): may be empty, which means no
    /// prompt (doc/0033), but must not start with `-`.
    Text,
    /// A list of folders, `~` allowed; entries not blank and not repeated.
    Paths,
}

/// A setting's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Text(String),
    Bool(bool),
    Int(i64),
    /// Paths as written, e.g. `~/workspace`, not expanded.
    Paths(Vec<String>),
}

impl SettingValue {
    /// The value for display: text as is, `true`/`false`, the number, the
    /// paths joined with `, ` (or `none`).
    pub fn display(&self) -> String {
        match self {
            SettingValue::Text(s) => s.clone(),
            SettingValue::Bool(b) => b.to_string(),
            SettingValue::Int(n) => n.to_string(),
            SettingValue::Paths(p) if p.is_empty() => "none".to_owned(),
            SettingValue::Paths(p) => p.join(", "),
        }
    }

    fn shape(&self) -> Shape {
        match self {
            SettingValue::Text(_) => Shape::Text,
            SettingValue::Bool(_) => Shape::Bool,
            SettingValue::Int(_) => Shape::Int,
            SettingValue::Paths(_) => Shape::Paths,
        }
    }

    /// The TOML value, without decor.
    fn to_toml(&self) -> Value {
        match self {
            SettingValue::Text(s) => Value::from(s.as_str()),
            SettingValue::Bool(b) => Value::from(*b),
            SettingValue::Int(n) => Value::from(*n),
            SettingValue::Paths(p) => Value::Array(p.iter().map(String::as_str).collect()),
        }
    }

    /// Reads a TOML value of `shape`; `None` when it has another type.
    fn from_toml(value: &Value, shape: Shape) -> Option<SettingValue> {
        match (shape, value) {
            (Shape::Text, Value::String(s)) => Some(SettingValue::Text(s.value().clone())),
            (Shape::Bool, Value::Boolean(b)) => Some(SettingValue::Bool(*b.value())),
            (Shape::Int, Value::Integer(n)) => Some(SettingValue::Int(*n.value())),
            (Shape::Paths, Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map(SettingValue::Paths),
            _ => None,
        }
    }
}

/// Where a setting's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    /// The key is in the file (or will be, once saved).
    File,
    /// The key is not in the file: tower's built-in default applies.
    Default,
}

/// One row of the Settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub key: SettingKey,
    pub section: Section,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: SettingKind,
    /// Tower's built-in default.
    pub default: SettingValue,
    /// The value with this window's changes applied.
    pub value: SettingValue,
    /// Whether [`Setting::value`] comes from the file or the default.
    pub source: ValueSource,
    /// The value in the file when the window opened (or last saved);
    /// `None` when the key was not there.
    pub saved: Option<SettingValue>,
    /// This window changed the setting and has not saved it yet.
    pub modified: bool,
}

impl Setting {
    /// Whether the value equals the built-in default (shown as `default`).
    pub fn is_default(&self) -> bool {
        self.value == self.default
    }
}

/// A change this window made and has not saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Write this value.
    Set(SettingValue),
    /// Remove the key, so the built-in default applies.
    Reset,
}

/// One unsaved change, for the "save these changes?" prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSummary {
    pub key: SettingKey,
    /// The value before the change (the file's, or the default).
    pub from: SettingValue,
    /// The value after it.
    pub to: SettingValue,
    /// The change removes the key (back to the default).
    pub reset: bool,
}

/// A key this window changed that was also changed in the file since the
/// window opened (another tower, or a hand edit). [`EditSession::save`]
/// with `force` overwrites it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: SettingKey,
    /// The file's value when the window opened; `None`: not in the file.
    pub opened: Option<SettingValue>,
    /// The file's value now; `None`: not in the file. A value of an
    /// unexpected type shows as its TOML text.
    pub on_disk: Option<SettingValue>,
    /// This window's change: `None` resets the key to its default.
    pub mine: Option<SettingValue>,
}

/// Why a save wrote nothing.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// Keys this window changed were changed on disk meanwhile; save again
    /// with `force` to overwrite them.
    #[error("{} changed in the file since the window opened", conflict_names(.0))]
    Conflict(Vec<Conflict>),
    /// The file on disk is not valid TOML any more.
    #[error("cannot parse {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    /// The result would not be a valid config.
    #[error("{0}")]
    Invalid(ConfigError),
    /// Reading, locking or writing failed.
    #[error("cannot {what} {}: {source}", path.display())]
    Io {
        what: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

fn conflict_names(conflicts: &[Conflict]) -> String {
    let names: Vec<String> = conflicts.iter().map(|c| c.key.toml_key()).collect();
    names.join(", ")
}

/// What a successful save did.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    /// The config file path tower uses (a symlink stays a symlink).
    pub path: PathBuf,
    /// The file written: `path`, or its symlink's target.
    pub target: PathBuf,
    /// The file did not exist and was created with [`HEADER`].
    pub created: bool,
    /// The one-time backup made by this save, if any (`config.toml.bak`).
    pub backup: Option<PathBuf>,
    /// The keys written or removed; empty when there was nothing to save
    /// (then nothing was written).
    pub written: Vec<SettingKey>,
    /// The config the file now holds.
    pub config: Config,
    /// The stamp of the file written, so the reload check does not reload
    /// tower's own write.
    pub stamp: Option<FileStamp>,
    /// The file's text now; `None` when it does not exist.
    text: Option<String>,
    /// The changes the save applied.
    applied: BTreeMap<SettingKey, Change>,
}

/// The Settings window's view of the config: the file as opened, plus
/// unsaved changes. Pure data; only [`EditSession::open`] and the save
/// touch the file.
#[derive(Debug, Clone)]
pub struct EditSession {
    path: PathBuf,
    home: PathBuf,
    /// The file's text when opened or last saved; `None`: no file.
    text: Option<String>,
    /// The config that text gives.
    config: Config,
    /// The file's values when opened or last saved; `None`: absent.
    base: BTreeMap<SettingKey, Option<SettingValue>>,
    changes: BTreeMap<SettingKey, Change>,
}

impl EditSession {
    /// Opens the config at `path` (`~` expands against `home`). A missing
    /// file is the built-in defaults; a file that does not parse is an
    /// error (fix it in an editor, [`edit_in_editor`]).
    pub fn open(path: &Path, home: &Path) -> Result<EditSession, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        EditSession::from_text(path, home, text)
    }

    /// A session over `text` as the content of `path` (`None`: no file).
    pub fn from_text(
        path: &Path,
        home: &Path,
        text: Option<String>,
    ) -> Result<EditSession, ConfigError> {
        let (config, base) = match &text {
            Some(text) => {
                let config = Config::parse(text, home).map_err(|err| with_path(err, path))?;
                let doc: DocumentMut = text
                    .parse()
                    .map_err(|e: toml_edit::TomlError| ConfigError::Invalid(e.to_string()))?;
                (config, file_values(&doc))
            }
            None => (
                Config::defaults(home),
                SettingKey::ALL.iter().map(|k| (*k, None)).collect(),
            ),
        };
        Ok(EditSession {
            path: path.to_path_buf(),
            home: home.to_path_buf(),
            text,
            config,
            base,
            changes: BTreeMap::new(),
        })
    }

    /// The config file this session edits.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file existed when opened or last saved.
    pub fn exists(&self) -> bool {
        self.text.is_some()
    }

    /// The config as opened or last saved, without this window's changes.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configured accounts, read-only (doc/0022): shown, never edited.
    pub fn accounts(&self) -> &[Account] {
        &self.config.accounts
    }

    /// What `key` takes. `default_account` offers the configured account
    /// names; `default_permission_mode` never offers `bypassPermissions`.
    pub fn kind(&self, key: SettingKey) -> SettingKind {
        let choice = |values: &[&str], free_text| SettingKind::Choice {
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            free_text,
        };
        match key {
            SettingKey::UiStyle => choice(
                &[
                    UiStyle::Dashboard.as_str(),
                    UiStyle::Minimal.as_str(),
                    UiStyle::Workspace.as_str(),
                ],
                false,
            ),
            SettingKey::UiTheme => {
                let names: Vec<&str> = UiTheme::ALL.iter().map(|t| t.as_str()).collect();
                choice(&names, false)
            }
            SettingKey::UiColorDepth => {
                let names: Vec<&str> = ColorDepth::ALL.iter().map(|d| d.as_str()).collect();
                choice(&names, false)
            }
            SettingKey::UiCardSize => {
                let names: Vec<&str> = CardSize::ALL.iter().map(|c| c.as_str()).collect();
                choice(&names, false)
            }
            SettingKey::DefaultAccount => SettingKind::Choice {
                values: self
                    .config
                    .accounts
                    .iter()
                    .map(|a| a.name.clone())
                    .collect(),
                free_text: false,
            },
            SettingKey::DefaultModel => choice(&MODEL_CHOICES, true),
            SettingKey::DefaultEffort => choice(&EFFORT_CHOICES, false),
            SettingKey::DefaultPermissionMode => {
                let modes: Vec<&str> = PERMISSION_MODES
                    .iter()
                    .copied()
                    .filter(|m| *m != BYPASS_PERMISSIONS)
                    .collect();
                choice(&modes, false)
            }
            SettingKey::ProjectRoots | SettingKey::ExtraProjects => SettingKind::Paths,
            SettingKey::HandoffPrompt => SettingKind::Text,
            SettingKey::AllowCrossGroupHandoff
            | SettingKey::TrackSubagents
            | SettingKey::UsageShow => SettingKind::Bool,
            SettingKey::PollIntervalSecs => SettingKind::Int {
                min: *POLL_INTERVAL_RANGE.start() as i64,
                max: *POLL_INTERVAL_RANGE.end() as i64,
            },
            SettingKey::LogsFollowSecs => SettingKind::Steps {
                values: LOGS_FOLLOW_STEPS.iter().map(|s| *s as i64).collect(),
                min: *LOGS_FOLLOW_RANGE.start() as i64,
                max: *LOGS_FOLLOW_RANGE.end() as i64,
            },
            SettingKey::UsageRefreshSecs => SettingKind::Steps {
                values: usage::REFRESH_STEPS.iter().map(|s| *s as i64).collect(),
                min: *usage::REFRESH_RANGE.start() as i64,
                max: *usage::REFRESH_RANGE.end() as i64,
            },
        }
    }

    /// The values the UI may offer for `key`: a choice's list, the steps'
    /// numbers, `false` and `true` for a bool, nothing for the other kinds. A value the file
    /// already holds but that is not offered (e.g. `bypassPermissions`)
    /// stays until changed; [`EditSession::revert`] brings it back.
    pub fn offered(&self, key: SettingKey) -> Vec<String> {
        match self.kind(key) {
            SettingKind::Choice { values, .. } => values,
            SettingKind::Steps { values, .. } => values.iter().map(i64::to_string).collect(),
            SettingKind::Bool => vec!["false".to_owned(), "true".to_owned()],
            _ => Vec::new(),
        }
    }

    /// Tower's built-in default for `key` (doc/0002). `default_account`'s
    /// is the first configured account.
    pub fn default_value(&self, key: SettingKey) -> SettingValue {
        let text = |s: &str| SettingValue::Text(s.to_owned());
        match key {
            SettingKey::UiStyle => text(UiStyle::default().as_str()),
            SettingKey::UiTheme => text(UiTheme::default().as_str()),
            SettingKey::UiColorDepth => text(ColorDepth::default().as_str()),
            SettingKey::UiCardSize => text(CardSize::default().as_str()),
            SettingKey::DefaultAccount => text(&self.config.accounts[0].name),
            SettingKey::DefaultModel | SettingKey::DefaultEffort => text(DEFAULT_CHOICE),
            SettingKey::DefaultPermissionMode => text(DEFAULT_PERMISSION_MODE),
            SettingKey::ProjectRoots | SettingKey::ExtraProjects => SettingValue::Paths(Vec::new()),
            SettingKey::HandoffPrompt => text("continue"),
            SettingKey::AllowCrossGroupHandoff => SettingValue::Bool(false),
            SettingKey::PollIntervalSecs => SettingValue::Int(DEFAULT_POLL_INTERVAL_SECS as i64),
            SettingKey::LogsFollowSecs => SettingValue::Int(DEFAULT_LOGS_FOLLOW_SECS as i64),
            SettingKey::TrackSubagents => SettingValue::Bool(true),
            SettingKey::UsageShow => SettingValue::Bool(false),
            SettingKey::UsageRefreshSecs => SettingValue::Int(usage::DEFAULT_REFRESH_SECS as i64),
        }
    }

    /// The value of `key` with this window's changes applied.
    pub fn value(&self, key: SettingKey) -> SettingValue {
        match self.changes.get(&key) {
            Some(Change::Set(v)) => v.clone(),
            Some(Change::Reset) => self.default_value(key),
            None => self.saved(key).unwrap_or_else(|| self.default_value(key)),
        }
    }

    /// The file's value of `key` when opened or last saved.
    pub fn saved(&self, key: SettingKey) -> Option<SettingValue> {
        self.base.get(&key).cloned().flatten()
    }

    /// Whether `key` has an unsaved change.
    pub fn is_modified(&self, key: SettingKey) -> bool {
        self.changes.contains_key(&key)
    }

    /// The row of `key` for the window.
    pub fn setting(&self, key: SettingKey) -> Setting {
        let source = match self.changes.get(&key) {
            Some(Change::Set(_)) => ValueSource::File,
            Some(Change::Reset) => ValueSource::Default,
            None if self.saved(key).is_some() => ValueSource::File,
            None => ValueSource::Default,
        };
        Setting {
            key,
            section: key.section(),
            label: key.label(),
            help: key.help(),
            kind: self.kind(key),
            default: self.default_value(key),
            value: self.value(key),
            source,
            saved: self.saved(key),
            modified: self.is_modified(key),
        }
    }

    /// Every row, in display order.
    pub fn settings(&self) -> Vec<Setting> {
        SettingKey::ALL.iter().map(|k| self.setting(*k)).collect()
    }

    /// Checks `value` for `key` without changing anything: the message for
    /// an invalid field (e.g. `must be 1 to 60 seconds`).
    pub fn check(&self, key: SettingKey, value: &SettingValue) -> Result<(), String> {
        if value.shape() != key.shape() {
            return Err(format!("{} takes another kind of value", key.label()));
        }
        match (self.kind(key), value) {
            (SettingKind::Choice { values, free_text }, SettingValue::Text(v)) => {
                if values.iter().any(|x| x == v) {
                    Ok(())
                } else if key == SettingKey::DefaultPermissionMode && v == BYPASS_PERMISSIONS {
                    Err(format!(
                        "{BYPASS_PERMISSIONS} is not offered as a default; pick it per session \
                         in the new-session dialog"
                    ))
                } else if free_text {
                    if v.trim().is_empty() {
                        Err(format!("{} must not be empty", key.label()))
                    } else if v.chars().any(char::is_whitespace) {
                        Err(format!("{} must be one word", key.label()))
                    } else {
                        Ok(())
                    }
                } else {
                    Err(format!(
                        "{} must be one of: {}",
                        key.label(),
                        values.join(", ")
                    ))
                }
            }
            (SettingKind::Int { min, max }, SettingValue::Int(n)) => {
                if (min..=max).contains(n) {
                    Ok(())
                } else {
                    Err(format!("must be {min} to {max} seconds"))
                }
            }
            (SettingKind::Steps { values, min, max }, SettingValue::Int(n)) => {
                // 0 is off only where off is one of the steps.
                let off = values.contains(&0);
                if (off && *n == 0) || (min..=max).contains(n) {
                    Ok(())
                } else if off {
                    Err(format!("must be off (0) or {min} to {max} seconds"))
                } else {
                    Err(format!("must be {min} to {max} seconds"))
                }
            }
            (SettingKind::Text, SettingValue::Text(v)) => {
                if v.trim_start().starts_with('-') {
                    Err(format!("{} must not start with '-'", key.label()))
                } else {
                    Ok(())
                }
            }
            (SettingKind::Paths, SettingValue::Paths(paths)) => {
                for (i, p) in paths.iter().enumerate() {
                    if p.trim().is_empty() {
                        return Err("a folder must not be empty".to_owned());
                    }
                    if paths[..i].contains(p) {
                        return Err(format!("{p} is listed twice"));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Changes `key` to `value` (unsaved). Setting it back to the file's
    /// value drops the change. An invalid value changes nothing and returns
    /// the message ([`EditSession::check`]).
    pub fn set(&mut self, key: SettingKey, value: SettingValue) -> Result<(), String> {
        self.check(key, &value)?;
        if self.saved(key).as_ref() == Some(&value) {
            self.changes.remove(&key);
        } else {
            self.changes.insert(key, Change::Set(value));
        }
        Ok(())
    }

    /// Back to the built-in default (`Del`): the save removes the key from
    /// the file, keeping the comment lines above it (doc/0022). A key that
    /// is not in the file just loses its unsaved change.
    pub fn reset_to_default(&mut self, key: SettingKey) {
        if self.saved(key).is_some() {
            self.changes.insert(key, Change::Reset);
        } else {
            self.changes.remove(&key);
        }
    }

    /// Drops the unsaved change of `key` (undo).
    pub fn revert(&mut self, key: SettingKey) {
        self.changes.remove(&key);
    }

    /// Drops every unsaved change.
    pub fn revert_all(&mut self) {
        self.changes.clear();
    }

    /// Whether anything is unsaved.
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// The unsaved changes, in display order.
    pub fn changes(&self) -> Vec<ChangeSummary> {
        self.changes
            .iter()
            .map(|(key, change)| ChangeSummary {
                key: *key,
                from: self.saved(*key).unwrap_or_else(|| self.default_value(*key)),
                to: self.value(*key),
                reset: *change == Change::Reset,
            })
            .collect()
    }

    /// The config the file would give with this window's changes, applied
    /// to the text as opened; the error a save would refuse with.
    pub fn validate(&self) -> Result<Config, ConfigError> {
        let text =
            apply_changes(self.text.as_deref(), &self.changes).map_err(ConfigError::Invalid)?;
        Config::parse(&text, &self.home).map_err(|err| with_path(err, &self.path))
    }

    /// The save of the current changes, to run on a worker thread
    /// ([`SaveJob::run`]); hand its result to [`EditSession::finish_save`].
    /// `force` overwrites keys changed on disk meanwhile.
    pub fn save_job(&self, force: bool) -> SaveJob {
        SaveJob {
            path: self.path.clone(),
            home: self.home.clone(),
            base: self.base.clone(),
            changes: self.changes.clone(),
            force,
            lock_wait: LOCK_WAIT,
        }
    }

    /// Takes in a finished save: the file as written becomes the session's
    /// base, and the changes it wrote are no longer unsaved (changes made
    /// while it ran stay).
    pub fn finish_save(&mut self, outcome: &SaveOutcome) {
        if outcome.written.is_empty() {
            return;
        }
        for (key, change) in &outcome.applied {
            if self.changes.get(key) == Some(change) {
                self.changes.remove(key);
            }
        }
        self.text = outcome.text.clone();
        self.config = outcome.config.clone();
        self.base = match &outcome.text {
            Some(text) => match text.parse::<DocumentMut>() {
                Ok(doc) => file_values(&doc),
                Err(_) => self.base.clone(),
            },
            None => self.base.clone(),
        };
        // A change equal to the new file value is no change.
        let base = &self.base;
        self.changes.retain(|key, change| match change {
            Change::Set(v) => base.get(key).cloned().flatten().as_ref() != Some(v),
            Change::Reset => base.get(key).cloned().flatten().is_some(),
        });
    }

    /// Saves on this thread: [`EditSession::save_job`], run, and
    /// [`EditSession::finish_save`].
    pub fn save(&mut self, force: bool) -> Result<SaveOutcome, SaveError> {
        let outcome = self.save_job(force).run()?;
        self.finish_save(&outcome);
        Ok(outcome)
    }
}

/// A save taken by [`EditSession::save_job`]; owns its data, so it can run
/// on a worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveJob {
    path: PathBuf,
    home: PathBuf,
    base: BTreeMap<SettingKey, Option<SettingValue>>,
    changes: BTreeMap<SettingKey, Change>,
    force: bool,
    lock_wait: Duration,
}

impl SaveJob {
    /// The config file the job writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The save (doc/0022): under `config.toml.lock`, re-reads the file,
    /// stops at a [`Conflict`] unless forced, applies only the changed keys
    /// to the current text, validates the result with [`Config::parse`]
    /// (an invalid result writes nothing), makes the one-time
    /// `config.toml.bak` before tower's first write to the file, and writes
    /// atomically to the file (the symlink's target, permissions kept).
    pub fn run(&self) -> Result<SaveOutcome, SaveError> {
        let io_err = |what, path: &Path| {
            let path = path.to_path_buf();
            move |source| SaveError::Io { what, path, source }
        };
        let target = resolve_link(&self.path).map_err(io_err("resolve", &self.path))?;
        let dir = target
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf);
        if let Some(dir) = &dir {
            fs::create_dir_all(dir).map_err(io_err("create", dir))?;
        }
        let lock_file = sidecar::lock_path(&self.path);
        let _lock =
            sidecar::lock(&lock_file, self.lock_wait).map_err(io_err("lock", &lock_file))?;
        let disk = match fs::read_to_string(&target) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(io_err("read", &target)(err)),
        };
        let parse_err = |e: toml_edit::TomlError| SaveError::Parse {
            path: self.path.clone(),
            message: e.to_string().trim_end().to_owned(),
        };
        let disk_doc = match &disk {
            Some(text) => Some(text.parse::<DocumentMut>().map_err(parse_err)?),
            None => None,
        };
        if !self.force {
            let conflicts = self.conflicts(disk_doc.as_ref());
            if !conflicts.is_empty() {
                return Err(SaveError::Conflict(conflicts));
            }
        }
        if self.changes.is_empty() {
            let config = match &disk {
                Some(text) => Config::parse(text, &self.home),
                None => Ok(Config::defaults(&self.home)),
            }
            .map_err(|err| SaveError::Invalid(with_path(err, &self.path)))?;
            return Ok(SaveOutcome {
                path: self.path.clone(),
                target,
                created: false,
                backup: None,
                written: Vec::new(),
                config,
                stamp: fs::metadata(&self.path).ok().map(|m| FileStamp::of(&m)),
                text: disk,
                applied: BTreeMap::new(),
            });
        }
        let text =
            apply_changes(disk.as_deref(), &self.changes).map_err(|message| SaveError::Parse {
                path: self.path.clone(),
                message,
            })?;
        let config = Config::parse(&text, &self.home)
            .map_err(|err| SaveError::Invalid(with_path(err, &self.path)))?;
        let perms = fs::metadata(&target).ok().map(|m| m.permissions());
        let backup = match &disk {
            Some(old) => {
                let bak = backup_path(&self.path);
                make_backup(&bak, old.as_bytes(), perms.clone())
                    .map_err(io_err("back up the config to", &bak))?
                    .then_some(bak)
            }
            None => None,
        };
        write_atomic(&target, text.as_bytes(), perms).map_err(io_err("write", &target))?;
        // Still under the lock: no other tower has written since.
        let stamp = fs::metadata(&target).ok().map(|m| FileStamp::of(&m));
        Ok(SaveOutcome {
            path: self.path.clone(),
            target,
            created: disk.is_none(),
            backup,
            written: self.changes.keys().copied().collect(),
            config,
            stamp,
            text: Some(text),
            applied: self.changes.clone(),
        })
    }

    /// The changed keys whose file value differs both from the one the
    /// window opened with and from what this window wants.
    fn conflicts(&self, disk: Option<&DocumentMut>) -> Vec<Conflict> {
        self.changes
            .iter()
            .filter_map(|(key, change)| {
                let opened = self.base.get(key).cloned().flatten();
                let now = disk.and_then(|doc| raw_value(doc, *key));
                let on_disk = now.as_ref().map(|v| {
                    SettingValue::from_toml(v, key.shape())
                        .unwrap_or_else(|| SettingValue::Text(v.to_string().trim().to_owned()))
                });
                let now_value = now
                    .as_ref()
                    .map(|v| SettingValue::from_toml(v, key.shape()));
                let same = |want: Option<&SettingValue>| match (want, &now_value) {
                    (None, None) => true,
                    (Some(w), Some(Some(v))) => w == v,
                    _ => false,
                };
                // Unchanged on disk, or already what this window wants.
                let wanted = match change {
                    Change::Set(v) => Some(v),
                    Change::Reset => None,
                };
                let fine = same(opened.as_ref()) || same(wanted);
                (!fine).then(|| Conflict {
                    key: *key,
                    opened,
                    on_disk,
                    mine: match change {
                        Change::Set(v) => Some(v.clone()),
                        Change::Reset => None,
                    },
                })
            })
            .collect()
    }
}

/// A config error with the file's path, as [`Config::load_from`] gives it.
fn with_path(err: ConfigError, path: &Path) -> ConfigError {
    match err {
        ConfigError::Parse { source, .. } => ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    }
}

// ---- editing the text with toml_edit --------------------------------------

/// The file's values of every setting.
fn file_values(doc: &DocumentMut) -> BTreeMap<SettingKey, Option<SettingValue>> {
    SettingKey::ALL
        .iter()
        .map(|key| {
            let value = raw_value(doc, *key).and_then(|v| SettingValue::from_toml(&v, key.shape()));
            (*key, value)
        })
        .collect()
}

/// The value of `key` in `doc`, if it is there and a value.
fn raw_value(doc: &DocumentMut, key: SettingKey) -> Option<Value> {
    let item = match key.path() {
        [name] => doc.get(name)?,
        [table, name] => doc.get(table)?.as_table_like()?.get(name)?,
        _ => return None,
    };
    item.as_value().cloned()
}

/// `text` (`None`: no file) with `changes` applied, keeping comments,
/// blank lines and key order. Without a file the result is [`HEADER`] and
/// the set keys. The error is `toml_edit`'s message.
fn apply_changes(
    text: Option<&str>,
    changes: &BTreeMap<SettingKey, Change>,
) -> Result<String, String> {
    let created = text.is_none();
    let mut doc = match text {
        Some(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| e.to_string().trim_end().to_owned())?,
        None => DocumentMut::new(),
    };
    for (key, change) in changes {
        match change {
            Change::Set(value) => set_value(&mut doc, *key, value),
            Change::Reset => remove_value(&mut doc, *key),
        }
    }
    if created {
        Ok(format!("{HEADER}\n{doc}"))
    } else {
        Ok(doc.to_string())
    }
}

/// Sets `key` to `value`. An existing value keeps its decor (the spaces
/// around it and a trailing comment); a new top-level key goes after the
/// last top-level key, a new `[ui]` key into `[ui]`, which is created at
/// the end when missing.
fn set_value(doc: &mut DocumentMut, key: SettingKey, value: &SettingValue) {
    let (table, name) = match key.path() {
        [name] => (doc.as_table_mut() as &mut dyn TableLike, *name),
        [table, name] => {
            if doc.get(table).and_then(Item::as_table_like).is_none() {
                let mut new = Table::new();
                new.decor_mut().set_prefix("\n");
                doc.insert(table, Item::Table(new));
            }
            let Some(t) = doc.get_mut(table).and_then(Item::as_table_like_mut) else {
                return;
            };
            (t, *name)
        }
        _ => return,
    };
    match table.get_mut(name) {
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            let mut new = formatted_like(old, value);
            *new.decor_mut() = decor;
            *old = new;
        }
        Some(item) => *item = Item::Value(value.to_toml()),
        None => {
            table.insert(name, Item::Value(value.to_toml()));
        }
    }
}

/// `value` as TOML, a list spread over lines like `old` when it was.
fn formatted_like(old: &Value, value: &SettingValue) -> Value {
    match (old, value) {
        (Value::Array(a), SettingValue::Paths(paths))
            if !paths.is_empty() && a.to_string().contains('\n') =>
        {
            let mut array = Array::new();
            for p in paths {
                let mut v = Value::from(p.as_str());
                v.decor_mut().set_prefix("\n    ");
                v.decor_mut().set_suffix("");
                array.push_formatted(v);
            }
            array.set_trailing_comma(true);
            array.set_trailing("\n");
            Value::Array(array)
        }
        _ => value.to_toml(),
    }
}

/// Removes `key`, so tower's default applies. The comment lines above it
/// stay: they move to the next key of the same table, else to the next
/// table header, else to the end of the file.
fn remove_value(doc: &mut DocumentMut, key: SettingKey) {
    let (table_name, name) = match key.path() {
        [name] => (None, *name),
        [table, name] => (Some(*table), *name),
        _ => return,
    };
    let after = match table_name {
        None => None,
        Some(t) => match doc.get(t) {
            Some(Item::Table(table)) => table.position(),
            _ => None,
        },
    };
    let table: &mut dyn TableLike = match table_name {
        None => doc.as_table_mut(),
        Some(t) => match doc.get_mut(t).and_then(Item::as_table_like_mut) {
            Some(table) => table,
            None => return,
        },
    };
    let Some(k) = table.key(name) else {
        return;
    };
    let prefix = decor_prefix(k.leaf_decor()).to_owned();
    let values: Vec<String> = table
        .iter()
        .filter(|(_, item)| item.is_value())
        .map(|(k, _)| k.to_owned())
        .collect();
    let next = values
        .iter()
        .position(|k| k == name)
        .and_then(|i| values.get(i + 1))
        .cloned();
    table.remove(name);
    if prefix.trim().is_empty() {
        return;
    }
    if let Some(next) = next
        && let Some(mut k) = table.key_mut(&next)
    {
        let old = decor_prefix(k.leaf_decor()).to_owned();
        k.leaf_decor_mut().set_prefix(format!("{prefix}{old}"));
        return;
    }
    let top_level = table_name.is_none();
    if !prepend_to_next_header(doc, after, top_level, &prefix) {
        let trailing = doc.trailing().as_str().unwrap_or("").to_owned();
        doc.set_trailing(format!("{prefix}{trailing}"));
    }
}

fn decor_prefix(decor: &toml_edit::Decor) -> &str {
    decor.prefix().and_then(|p| p.as_str()).unwrap_or("")
}

/// Puts `prefix` before the first table header after position `after`
/// (any header for the top-level table). `false` when there is none.
fn prepend_to_next_header(
    doc: &mut DocumentMut,
    after: Option<isize>,
    top_level: bool,
    prefix: &str,
) -> bool {
    let mut headers: Vec<&mut Table> = Vec::new();
    for (_, item) in doc.as_table_mut().iter_mut() {
        match item {
            Item::Table(t) => headers.push(t),
            Item::ArrayOfTables(a) => headers.extend(a.iter_mut()),
            _ => {}
        }
    }
    let best = headers
        .into_iter()
        .filter(|t| !t.is_dotted() && !t.is_implicit())
        .filter_map(|t| t.position().map(|pos| (pos, t)))
        .filter(|(pos, _)| top_level || after.is_some_and(|a| *pos > a))
        .min_by_key(|(pos, _)| *pos)
        .map(|(pos, t)| (pos, t.decor_mut()));
    let Some((_, decor)) = best else {
        return false;
    };
    // A header without its own decor is preceded by one line break.
    let old = decor.prefix().and_then(|p| p.as_str()).unwrap_or("\n");
    let new = format!("{prefix}{old}");
    decor.set_prefix(new);
    true
}

// ---- files ----------------------------------------------------------------

/// `<path>.bak`, the one-time copy made before tower's first write.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// The file a write to `path` goes to: the final target of a symlink
/// (which may not exist yet), else `path`.
pub fn resolve_link(path: &Path) -> io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&p) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&p)?;
                p = match p.parent() {
                    Some(dir) if target.is_relative() => dir.join(target),
                    _ => target,
                };
            }
            Ok(_) => return Ok(p),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(p),
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(format!(
        "{}: too many levels of symbolic links",
        path.display()
    )))
}

/// Writes `bytes` to `bak` unless it exists. `Ok(true)` when made now.
/// Never replaces an existing backup, even when two towers race.
fn make_backup(bak: &Path, bytes: &[u8], perms: Option<fs::Permissions>) -> io::Result<bool> {
    match fs::symlink_metadata(bak) {
        Ok(_) => return Ok(false),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let tmp = write_temp(bak, bytes, perms)?;
    // A hard link never replaces an existing file.
    let linked = match fs::hard_link(&tmp, bak) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        // A file system without hard links.
        Err(_) if !bak.exists() => fs::rename(&tmp, bak).map(|()| true),
        Err(err) => Err(err),
    };
    let _ = fs::remove_file(&tmp);
    linked
}

/// A fresh temp file next to `path` holding `bytes`, fsynced, with
/// `perms` when given.
fn write_temp(path: &Path, bytes: &[u8], perms: Option<fs::Permissions>) -> io::Result<PathBuf> {
    let (tmp, mut file) = loop {
        let tmp = sidecar::tmp_path(path);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    };
    let written = (|| {
        if let Some(perms) = perms {
            file.set_permissions(perms)?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    match written {
        Ok(()) => Ok(tmp),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// Writes `bytes` to `target` atomically, as `state.json` is written
/// (doc/0002): a temp file in the same directory, fsync, rename over
/// `target`, fsync of the directory. `perms` are the old file's.
fn write_atomic(target: &Path, bytes: &[u8], perms: Option<fs::Permissions>) -> io::Result<()> {
    let tmp = write_temp(target, bytes, perms)?;
    if let Err(err) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty())
        && let Ok(d) = fs::File::open(dir)
    {
        // Persist the rename itself; failure here is not fatal.
        let _ = d.sync_all();
    }
    Ok(())
}

/// Creates the config at `path` (its symlink's target) holding only
/// [`HEADER`] when it is missing, with its parent folders. `Ok(true)` when
/// created; an existing file is left alone.
pub fn ensure_exists(path: &Path) -> io::Result<bool> {
    let target = resolve_link(path)?;
    if target.exists() {
        return Ok(false);
    }
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(mut file) => {
            file.write_all(HEADER.as_bytes())?;
            file.sync_all()?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

// ---- reloading (doc/0022) ---------------------------------------------------

/// The config file as tower read it: the version's stamp and what it gives.
#[derive(Debug)]
pub struct ConfigSnapshot {
    /// The stamp of the version read; `None` when there is no file.
    pub stamp: Option<FileStamp>,
    /// Whether the file existed.
    pub found: bool,
    /// The config, or why the file is not a valid one. A missing file is
    /// the built-in defaults.
    pub result: Result<Config, ConfigError>,
}

/// Reads and validates the config at `path` (`~` expands against `home`).
pub fn read_config(path: &Path, home: &Path) -> ConfigSnapshot {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return ConfigSnapshot {
                stamp: None,
                found: false,
                result: Ok(Config::defaults(home)),
            };
        }
        Err(source) => {
            return ConfigSnapshot {
                stamp: None,
                found: true,
                result: Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                }),
            };
        }
    };
    let stamp = file.metadata().ok().map(|m| FileStamp::of(&m));
    let mut text = String::new();
    let result = match file.read_to_string(&mut text) {
        Ok(_) => Config::parse(&text, home).map_err(|err| with_path(err, path)),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    };
    ConfigSnapshot {
        stamp,
        found: true,
        result,
    }
}

/// The stamp of the config file now; `None` when it is missing or cannot
/// be looked at.
pub fn stamp_of(path: &Path) -> Option<FileStamp> {
    fs::metadata(path).ok().map(|m| FileStamp::of(&m))
}

/// A look at the config file for changes, run on a worker thread at each
/// poll like the sidecar's check (doc/0017 section 8, doc/0022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCheck {
    /// The config file.
    pub path: PathBuf,
    /// For `~` expansion.
    pub home: PathBuf,
    /// The stamp of the version tower last read or wrote; `None`: none.
    pub known: Option<FileStamp>,
}

impl ConfigCheck {
    /// `None` when the file's identity, size and modification time are the
    /// known ones (or it is still missing); else the file read again.
    pub fn run(&self) -> Option<ConfigSnapshot> {
        if stamp_of(&self.path) == self.known {
            return None;
        }
        Some(read_config(&self.path, &self.home))
    }
}

// ---- the editor (doc/0022) ------------------------------------------------

/// The editor command from `$VISUAL`, else `$EDITOR`
/// ([`crate::editor::editor_command`]), with a message about the config.
pub fn editor_from_env() -> Result<Vec<String>, String> {
    crate::editor::editor_from_env()
        .map_err(|e| e.replace("to edit the task in an editor", "to edit the config"))
}

/// Opens the config at `path` in `editor` (program and leading arguments,
/// from [`crate::editor::editor_from_env`]) in the foreground, like an
/// attach: the file is created first with [`HEADER`] when missing, and read
/// back and validated after a clean exit. An error message when the file
/// cannot be created or the editor fails (any change it saved is then
/// picked up by the next reload check).
pub fn edit_in_editor(
    handover: &mut dyn Handover,
    spawner: &mut dyn Spawner,
    editor: &[String],
    path: &Path,
    home: &Path,
    termination: &Termination,
) -> Result<ConfigSnapshot, String> {
    let Some((program, leading)) = editor.split_first() else {
        return Err("no editor command".to_owned());
    };
    ensure_exists(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut args = leading.to_vec();
    args.push(path.to_string_lossy().into_owned());
    let line = CommandLine {
        program: program.clone(),
        args,
        cwd: None,
    };
    let fg = Foreground {
        line: &line,
        announce: Some(format!("tower: editing {} with {program}…", path.display())),
        label: "the editor",
        pause_on_failure: true,
    };
    match attach::run_foreground(handover, spawner, &fg, termination)? {
        Some(0) => Ok(read_config(path, home)),
        Some(code) => Err(format!(
            "the editor exited with code {code}; the config was not reloaded"
        )),
        None => Err("the editor was ended by a signal; the config was not reloaded".to_owned()),
    }
}
