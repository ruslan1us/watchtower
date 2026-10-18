//! `config.toml` (doc/0002) and the configured accounts (doc/0003).
//!
//! Tower only reads this file. A missing file means built-in defaults: one
//! account `main` running `claude` with config dir `~/.claude`. Every key
//! documented in doc/0002 is accepted; unknown keys are ignored and missing
//! keys get defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::paths::Dirs;

/// Default `poll_interval_secs` (doc/0001).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;
/// Default account config dir when `config_dir` is unset (doc/0003).
pub const DEFAULT_CLAUDE_CONFIG_DIR: &str = "~/.claude";

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
            argv,
        })
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
    /// Seconds between polls; at least 1.
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
    #[serde(default)]
    accounts: Vec<RawAccount>,
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    name: String,
    command: String,
    config_dir: Option<String>,
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

    /// Loads a config file from `path`, expanding `~` against `home`.
    /// A missing file yields [`Config::defaults`].
    pub fn load_from(path: &Path, home: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text, home).map_err(|err| match err {
                ConfigError::Parse { source, .. } => ConfigError::Parse {
                    path: path.to_path_buf(),
                    source,
                },
                other => other,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::defaults(home)),
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
            let account = Account::new(a.name, a.command, a.config_dir.as_deref(), home)?;
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
        if poll_interval_secs == 0 {
            return Err(ConfigError::Invalid(
                "poll_interval_secs must be at least 1".into(),
            ));
        }
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
            accounts,
        } = raw;
        assert!(default_account.is_some());
        assert!(poll_interval_secs.is_some());
        assert!(!project_roots.is_empty());
        assert!(!extra_projects.is_empty());
        assert!(default_model.is_some());
        assert!(default_effort.is_some());
        assert!(default_permission_mode.is_some());
        assert!(handoff_prompt.is_some());
        assert!(!accounts.is_empty());
        for RawAccount {
            name,
            command,
            config_dir,
        } in accounts
        {
            assert!(!name.is_empty() && !command.is_empty());
            assert!(config_dir.is_some());
        }
    }
}
