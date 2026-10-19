//! `config.toml` (doc/0002) and the configured accounts (doc/0003).
//!
//! This module reads the file; [`crate::config_edit`] edits it for the
//! Settings window (doc/0022). A missing file means built-in defaults: one
//! account `main` running `claude` with config dir `~/.claude`. Every key
//! documented in doc/0002 is accepted; unknown keys are ignored and missing
//! keys get defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::paths::Dirs;

/// Default `poll_interval_secs` (doc/0001).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;
/// The allowed `poll_interval_secs` range, in seconds (doc/0022).
pub const POLL_INTERVAL_RANGE: std::ops::RangeInclusive<u64> = 1..=60;
/// Default account config dir when `config_dir` is unset (doc/0003).
pub const DEFAULT_CLAUDE_CONFIG_DIR: &str = "~/.claude";

/// The look of the session list, `[ui] style` (doc/0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiStyle {
    /// Rounded panels: a header with per-state pills, the list with section
    /// separators, and a detail panel for the selected row. The default.
    #[default]
    Dashboard,
    /// The plain single-column list with a title, status line and footer.
    Minimal,
}

impl UiStyle {
    /// The config value: `dashboard` or `minimal`.
    pub fn as_str(self) -> &'static str {
        match self {
            UiStyle::Dashboard => "dashboard",
            UiStyle::Minimal => "minimal",
        }
    }

    /// Parses a config value; anything else is an error naming the choices.
    pub fn parse(value: &str) -> Result<UiStyle, ConfigError> {
        match value {
            "dashboard" => Ok(UiStyle::Dashboard),
            "minimal" => Ok(UiStyle::Minimal),
            other => Err(ConfigError::Invalid(format!(
                "[ui] style `{other}` is not known; use \"dashboard\" or \"minimal\""
            ))),
        }
    }
}

/// The dashboard style's colour theme, `[ui] theme` (doc/0021). The
/// minimal style keeps its own look; `NO_COLOR` overrides every theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiTheme {
    /// Neutral greys, colour only for states and attention. The default.
    #[default]
    Neutral,
    /// Warm greys with a clay-orange accent.
    Warm,
    /// Named ANSI colours only; follows the terminal's own theme.
    Terminal,
    /// The original blue-grey palette.
    TokyoNight,
}

impl UiTheme {
    /// Every theme, in the order the docs list them.
    pub const ALL: [UiTheme; 4] = [
        UiTheme::Neutral,
        UiTheme::Warm,
        UiTheme::Terminal,
        UiTheme::TokyoNight,
    ];

    /// The config value: `neutral`, `warm`, `terminal` or `tokyo-night`.
    pub fn as_str(self) -> &'static str {
        match self {
            UiTheme::Neutral => "neutral",
            UiTheme::Warm => "warm",
            UiTheme::Terminal => "terminal",
            UiTheme::TokyoNight => "tokyo-night",
        }
    }

    /// Parses a config value; anything else is an error naming the choices.
    pub fn parse(value: &str) -> Result<UiTheme, ConfigError> {
        UiTheme::ALL
            .into_iter()
            .find(|t| t.as_str() == value)
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "[ui] theme `{value}` is not known; use \"neutral\", \"warm\", \
                     \"terminal\" or \"tokyo-night\""
                ))
            })
    }
}

/// One Claude Code account (doc/0003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Name shown in the UI and stored in the sidecar.
    pub name: String,
    /// The command as written in the config, e.g. `claude-alt`. Run as-is;
    /// tower never sets `CLAUDE_CONFIG_DIR` itself (doc/0002).
    pub command: String,
    /// The account's config dir with `~` expanded (symlinks not resolved).
    /// Used only to recognise which account hosts a running session.
    pub config_dir: PathBuf,
    /// The account's group (`group` in the config), e.g. `work` or
    /// `personal`; `""` when unset. Ungrouped accounts share the implicit
    /// group `""`. A hand-off only goes to an account of the owner's group
    /// unless `allow_cross_group_handoff` is set (doc/0004).
    pub group: String,
    argv: Vec<String>,
}

impl Account {
    /// Creates an account. `command` is split like a shell would (quotes and
    /// escapes, no expansion); it must yield at least one word. `config_dir`
    /// defaults to `~/.claude` and has `~` expanded against `home`.
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        config_dir: Option<&str>,
        home: &Path,
    ) -> Result<Account, ConfigError> {
        let name = name.into();
        let command = command.into();
        if name.trim().is_empty() {
            return Err(ConfigError::Invalid("an account has an empty name".into()));
        }
        let argv = match shlex::split(&command) {
            Some(words) if !words.is_empty() => words,
            _ => {
                return Err(ConfigError::Invalid(format!(
                    "account `{name}`: command `{command}` is empty or badly quoted"
                )));
            }
        };
        let config_dir =
            crate::paths::expand_tilde(config_dir.unwrap_or(DEFAULT_CLAUDE_CONFIG_DIR), home);
        Ok(Account {
            name,
            command,
            config_dir,
            group: String::new(),
            argv,
        })
    }

    /// The same account in group `group`. A group must be non-empty and
    /// made of ASCII letters, digits, `-` and `_`.
    pub fn with_group(mut self, group: &str) -> Result<Account, ConfigError> {
        let name = &self.name;
        if group.trim().is_empty() {
            return Err(ConfigError::Invalid(format!(
                "account `{name}`: group is empty; leave `group` out for no group"
            )));
        }
        if !group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ConfigError::Invalid(format!(
                "account `{name}`: group `{group}` may only contain letters, digits, `-` and `_`"
            )));
        }
        self.group = group.to_owned();
        Ok(self)
    }

    /// The name with the group when one is set, e.g. `main (work)`.
    pub fn label(&self) -> String {
        if self.group.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.group)
        }
    }

    /// The command split into program and leading arguments, e.g.
    /// `["claude"]` or `["env", "CLAUDE_CONFIG_DIR=/x", "claude"]`.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }
}

/// The parsed configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Account used for new sessions and the orchestrator. Always names one
    /// of [`Config::accounts`].
    pub default_account: String,
    /// Seconds between polls; 1 to 60 ([`POLL_INTERVAL_RANGE`]).
    pub poll_interval_secs: u64,
    /// Configured accounts, in file order; never empty, names are unique.
    pub accounts: Vec<Account>,
    /// Folders scanned for git clones in the new-session dialog (doc/0008),
    /// `~` expanded. Not used before milestone 2.
    pub project_roots: Vec<PathBuf>,
    /// Extra folders listed in the new-session dialog, `~` expanded.
    pub extra_projects: Vec<PathBuf>,
    /// Dialog default for `--model`; `"default"` omits the flag.
    pub default_model: String,
    /// Dialog default for `--effort`; `"default"` omits the flag.
    pub default_effort: String,
    /// Dialog default for `--permission-mode`.
    pub default_permission_mode: String,
    /// Prompt used when a session continues under another account (doc/0004).
    pub handoff_prompt: String,
    /// Whether a hand-off may go to an account of another group (doc/0004).
    /// Default `false`: only accounts of the owner's group are targets.
    pub allow_cross_group_handoff: bool,
    /// `[ui] style`: the look of the session list (doc/0020).
    pub ui_style: UiStyle,
    /// `[ui] theme`: the dashboard's colour theme (doc/0021).
    pub ui_theme: UiTheme,
    /// `[sessions] track_subagents` (default `true`): pass the observe-only
    /// subagent hooks with `--settings` to the sessions tower starts and
    /// read the files they write (doc/0029). When `false`, no `--settings`
    /// is passed and the files are ignored.
    pub track_subagents: bool,
}

/// Where the config came from: the file tower looked at and whether it was
/// there. A missing file means the built-in defaults (doc/0002), which is
/// easy to miss, so `tower status`, `tower --help` and the TUI say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    /// The config file tower reads (`$XDG_CONFIG_HOME/watchtower/config.toml`).
    pub path: PathBuf,
    /// Whether the file existed and was loaded; `false` means defaults.
    pub found: bool,
}

impl ConfigSource {
    /// Looks at `path` without reading it, e.g. for `tower --help`.
    pub fn probe(path: &Path) -> ConfigSource {
        ConfigSource {
            path: path.to_path_buf(),
            found: path.try_exists().unwrap_or(false),
        }
    }

    /// The path as printed, with `home` shortened to `~` when given.
    pub fn shown(&self, home: Option<&Path>) -> String {
        let path = self.path.to_string_lossy();
        match home {
            Some(home) => crate::paths::shorten_home(&path, home),
            None => path.into_owned(),
        }
    }

    /// One line for people: `config: <path>`, or `config: <path> (not
    /// found — using defaults)`.
    pub fn line(&self, home: Option<&Path>) -> String {
        let path = self.shown(home);
        if self.found {
            format!("config: {path}")
        } else {
            format!("config: {path} (not found — using defaults)")
        }
    }
}

/// Errors from loading or validating the config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not valid TOML or a key has the wrong type.
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    /// The values are inconsistent (duplicate names, unknown default, …).
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    default_account: Option<String>,
    poll_interval_secs: Option<u64>,
    #[serde(default)]
    project_roots: Vec<String>,
    #[serde(default)]
    extra_projects: Vec<String>,
    default_model: Option<String>,
    default_effort: Option<String>,
    default_permission_mode: Option<String>,
    handoff_prompt: Option<String>,
    allow_cross_group_handoff: Option<bool>,
    #[serde(default)]
    accounts: Vec<RawAccount>,
    ui: Option<RawUi>,
    sessions: Option<RawSessions>,
}

/// The `[sessions]` table (doc/0029).
#[derive(Debug, Default, Deserialize)]
struct RawSessions {
    track_subagents: Option<bool>,
}

/// The `[ui]` table (doc/0020).
#[derive(Debug, Default, Deserialize)]
struct RawUi {
    style: Option<String>,
    theme: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    name: String,
    command: String,
    config_dir: Option<String>,
    group: Option<String>,
}

impl Config {
    /// Built-in defaults: one account `main` running `claude` (doc/0002).
    pub fn defaults(home: &Path) -> Config {
        Config::from_raw(RawConfig::default(), home).expect("built-in defaults are valid")
    }

    /// Loads `dirs.config_file()`; a missing file yields [`Config::defaults`].
    pub fn load(dirs: &Dirs) -> Result<Config, ConfigError> {
        Config::load_from(&dirs.config_file(), &dirs.home)
    }

    /// [`Config::load`], also reporting where tower looked and whether the
    /// file was there ([`ConfigSource`]).
    pub fn load_with_source(dirs: &Dirs) -> Result<(Config, ConfigSource), ConfigError> {
        Config::load_from_with_source(&dirs.config_file(), &dirs.home)
    }

    /// Loads a config file from `path`, expanding `~` against `home`.
    /// A missing file yields [`Config::defaults`].
    pub fn load_from(path: &Path, home: &Path) -> Result<Config, ConfigError> {
        Config::load_from_with_source(path, home).map(|(config, _)| config)
    }

    /// [`Config::load_from`], also reporting whether the file was found.
    pub fn load_from_with_source(
        path: &Path,
        home: &Path,
    ) -> Result<(Config, ConfigSource), ConfigError> {
        let source = |found| ConfigSource {
            path: path.to_path_buf(),
            found,
        };
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text, home)
                .map(|config| (config, source(true)))
                .map_err(|err| match err {
                    ConfigError::Parse { source, .. } => ConfigError::Parse {
                        path: path.to_path_buf(),
                        source,
                    },
                    other => other,
                }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok((Config::defaults(home), source(false)))
            }
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Parses config text. Without `[[accounts]]` the default `main` account
    /// is used; without `default_account` the first account is the default.
    pub fn parse(text: &str, home: &Path) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::new(),
            source: Box::new(source),
        })?;
        Config::from_raw(raw, home)
    }

    fn from_raw(raw: RawConfig, home: &Path) -> Result<Config, ConfigError> {
        let mut accounts = Vec::with_capacity(raw.accounts.len().max(1));
        for a in raw.accounts {
            let mut account = Account::new(a.name, a.command, a.config_dir.as_deref(), home)?;
            if let Some(group) = a.group.as_deref() {
                account = account.with_group(group)?;
            }
            if accounts.iter().any(|x: &Account| x.name == account.name) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate account name `{}`",
                    account.name
                )));
            }
            accounts.push(account);
        }
        if accounts.is_empty() {
            accounts.push(Account::new("main", "claude", None, home)?);
        }
        check_distinct_config_dirs(&accounts)?;
        let default_account = raw
            .default_account
            .unwrap_or_else(|| accounts[0].name.clone());
        if !accounts.iter().any(|a| a.name == default_account) {
            return Err(ConfigError::Invalid(format!(
                "default_account `{default_account}` is not a configured account"
            )));
        }
        let poll_interval_secs = raw.poll_interval_secs.unwrap_or(DEFAULT_POLL_INTERVAL_SECS);
        if !POLL_INTERVAL_RANGE.contains(&poll_interval_secs) {
            return Err(ConfigError::Invalid(format!(
                "poll_interval_secs must be {} to {} seconds, not {poll_interval_secs}",
                POLL_INTERVAL_RANGE.start(),
                POLL_INTERVAL_RANGE.end()
            )));
        }
        let ui = raw.ui.unwrap_or_default();
        let ui_style = match ui.style {
            Some(style) => UiStyle::parse(&style)?,
            None => UiStyle::default(),
        };
        let ui_theme = match ui.theme {
            Some(theme) => UiTheme::parse(&theme)?,
            None => UiTheme::default(),
        };
        let expand = |v: Vec<String>| -> Vec<PathBuf> {
            v.iter()
                .map(|p| crate::paths::expand_tilde(p, home))
                .collect()
        };
        Ok(Config {
            default_account,
            poll_interval_secs,
            accounts,
            project_roots: expand(raw.project_roots),
            extra_projects: expand(raw.extra_projects),
            default_model: raw.default_model.unwrap_or_else(|| "default".into()),
            default_effort: raw.default_effort.unwrap_or_else(|| "default".into()),
            default_permission_mode: raw.default_permission_mode.unwrap_or_else(|| "auto".into()),
            handoff_prompt: raw.handoff_prompt.unwrap_or_else(|| "continue".into()),
            allow_cross_group_handoff: raw.allow_cross_group_handoff.unwrap_or(false),
            ui_style,
            ui_theme,
            track_subagents: raw.sessions.and_then(|s| s.track_subagents).unwrap_or(true),
        })
    }

    /// Looks up an account by name.
    pub fn account(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.name == name)
    }

    /// The default account (doc/0003). Always present after validation.
    pub fn default_account(&self) -> &Account {
        self.account(&self.default_account)
            .expect("default_account is validated on load")
    }

    /// The poll interval as a `Duration`.
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_interval_secs)
    }
}

/// The path used to compare config dirs: symlinks resolved when the path
/// exists, otherwise the `~`-expanded path as given (doc/0017).
pub fn comparable_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.components().collect())
}

/// Two accounts whose config dirs are the same directory cannot be told
/// apart by `/proc` (doc/0003 rule 1): every session of one would be
/// attributed to the other. Such a config is rejected (doc/0017).
fn check_distinct_config_dirs(accounts: &[Account]) -> Result<(), ConfigError> {
    let dirs: Vec<PathBuf> = accounts
        .iter()
        .map(|a| comparable_dir(&a.config_dir))
        .collect();
    for (i, a) in accounts.iter().enumerate() {
        if let Some(j) = (0..i).find(|&j| dirs[j] == dirs[i]) {
            let b = &accounts[j];
            return Err(ConfigError::Invalid(format!(
                "accounts `{}` and `{}` use the same config dir {} (config_dir defaults to \
                 {DEFAULT_CLAUDE_CONFIG_DIR}); tower could not tell their sessions apart — give \
                 each account its own config_dir",
                b.name,
                a.name,
                dirs[i].display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key `RawConfig` accepts must be documented in
    /// `config.example.toml`. The exhaustive patterns stop compiling when a
    /// key is added, as a reminder; `tests/config.rs` then checks that the
    /// example lists exactly these keys.
    #[test]
    fn example_config_sets_every_accepted_key() {
        let raw: RawConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
        let RawConfig {
            default_account,
            poll_interval_secs,
            project_roots,
            extra_projects,
            default_model,
            default_effort,
            default_permission_mode,
            handoff_prompt,
            allow_cross_group_handoff,
            accounts,
            ui,
            sessions,
        } = raw;
        let RawSessions { track_subagents } = sessions.expect("[sessions] is documented");
        assert!(track_subagents.is_some());
        let RawUi { style, theme } = ui.expect("[ui] is documented");
        assert!(style.is_some());
        assert!(theme.is_some());
        assert!(default_account.is_some());
        assert!(poll_interval_secs.is_some());
        assert!(!project_roots.is_empty());
        assert!(!extra_projects.is_empty());
        assert!(default_model.is_some());
        assert!(default_effort.is_some());
        assert!(default_permission_mode.is_some());
        assert!(handoff_prompt.is_some());
        assert!(allow_cross_group_handoff.is_some());
        assert!(!accounts.is_empty());
        for RawAccount {
            name,
            command,
            config_dir,
            group,
        } in accounts
        {
            assert!(!name.is_empty() && !command.is_empty());
            assert!(config_dir.is_some());
            assert!(group.is_some());
        }
    }
}
