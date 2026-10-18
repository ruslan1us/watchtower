//! Merging the listings of all accounts and resolving session ownership
//! (doc/0003).
//!
//! Accounts that share session storage list the same sessions, so rows are
//! de-duplicated by `sessionId`, then short `id`, then `pid`. The owner of each
//! merged row is resolved in this order, first match wins:
//!
//! 1. live process (`pid`): `CLAUDE_CONFIG_DIR` from `/proc/<pid>/environ`
//!    (missing = `~/.claude`), compared with each account's `config_dir` as
//!    canonical paths. A readable environ that matches no account gives
//!    [`Ownership::Unknown`] with the raw path; an unreadable one falls
//!    through;
//! 2. seen in exactly one account's listing while every other account's poll
//!    succeeded without it (lists not shared);
//! 3. the sidecar record;
//! 4. unknown (`?`).
//!
//! With a single configured account, rule 2 is skipped (one listing proves
//! nothing about shared lists) and rule 4 gives that account instead of `?`,
//! with source [`AccountSource::Implicit`], which is never written to the
//! sidecar: if a second account with a shared listing is added later, no
//! stale `list` records claim its sessions. Rule 1 still applies, so a live
//! process whose config dir is not the account's is `?` with the raw path.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::config::{Account, DEFAULT_CLAUDE_CONFIG_DIR};
use crate::listing::Session;
use crate::paths;
use crate::sidecar::{AccountSource, Sidecar};

/// The environment variable that selects a Claude Code config dir.
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// Reads `CLAUDE_CONFIG_DIR` of a process. Abstracted so tests can fake it.
pub trait EnvironReader {
    /// `Ok(Some(value))` when set, `Ok(None)` when the process has no such
    /// variable, `Err` when the environment cannot be read (other user's
    /// process, process gone, …).
    fn config_dir_var(&self, pid: u32) -> std::io::Result<Option<OsString>>;
}

/// Reads `/proc/<pid>/environ` (the one OS-level read tower does, doc/0001).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcEnviron;

impl EnvironReader for ProcEnviron {
    fn config_dir_var(&self, pid: u32) -> std::io::Result<Option<OsString>> {
        let bytes = std::fs::read(format!("/proc/{pid}/environ"))?;
        Ok(find_var(&bytes, CONFIG_DIR_VAR))
    }
}

/// Finds `name=value` in a NUL-separated environment block.
pub fn find_var(environ: &[u8], name: &str) -> Option<OsString> {
    use std::os::unix::ffi::OsStrExt;
    let prefix = format!("{name}=");
    environ
        .split(|b| *b == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .map(|value| std::ffi::OsStr::from_bytes(value).to_os_string())
}

/// One account's poll result, the input to [`Resolver::merge`].
#[derive(Debug, Clone)]
pub struct AccountListing {
    /// Account name, as in the config.
    pub account: String,
    /// The rows, or `None` when this account's poll failed or has no data
    /// yet. A failed poll is not evidence for rule 2.
    pub sessions: Option<Vec<Session>>,
    /// True when `sessions` is the last good snapshot kept after a failed
    /// poll (doc/0001: the UI keeps showing it, marked stale). Stale rows are
    /// shown but, like `None`, are not evidence for rule 2, and a fresh row
    /// from another account replaces a stale one in the merge.
    pub stale: bool,
}

impl AccountListing {
    /// A successful poll.
    pub fn fresh(account: impl Into<String>, sessions: Vec<Session>) -> AccountListing {
        AccountListing {
            account: account.into(),
            sessions: Some(sessions),
            stale: false,
        }
    }

    /// No data: the account has not been polled yet, or its poll failed and
    /// there is nothing to keep.
    pub fn missing(account: impl Into<String>) -> AccountListing {
        AccountListing {
            account: account.into(),
            sessions: None,
            stale: false,
        }
    }

    /// The last good rows of an account whose latest poll failed.
    pub fn stale(account: impl Into<String>, sessions: Vec<Session>) -> AccountListing {
        AccountListing {
            account: account.into(),
            sessions: Some(sessions),
            stale: true,
        }
    }

    /// True when this is a successful, current poll result: the only kind
    /// that counts for rule 2.
    pub fn is_fresh(&self) -> bool {
        self.sessions.is_some() && !self.stale
    }
}

/// Who owns a merged session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// A configured account, and how it was determined.
    Known {
        account: String,
        source: AccountSource,
    },
    /// Not determined: shown as `?` (doc/0003). `config_dir` is the raw
    /// `CLAUDE_CONFIG_DIR` of the live process when it matched no account.
    Unknown { config_dir: Option<PathBuf> },
}

impl Ownership {
    /// The owning account name, if known.
    pub fn account(&self) -> Option<&str> {
        match self {
            Ownership::Known { account, .. } => Some(account),
            Ownership::Unknown { .. } => None,
        }
    }

    /// Account name for display: the name, or `?` when unknown.
    pub fn label(&self) -> &str {
        self.account().unwrap_or("?")
    }
}

/// A session after merging, with the accounts that listed it and its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedSession {
    /// The row. When several accounts list it, the first one with a live
    /// process wins, otherwise the first in config order.
    pub session: Session,
    /// Names of the accounts whose listing contained the row, in config order.
    pub seen_in: Vec<String>,
    /// Resolved owner.
    pub owner: Ownership,
}

impl AsRef<Session> for MergedSession {
    fn as_ref(&self) -> &Session {
        &self.session
    }
}

/// True when two rows describe the same session: equal `sessionId` when both
/// have one, else equal short `id`, else equal `pid` (doc/0001, doc/0003).
pub fn same_session(a: &Session, b: &Session) -> bool {
    if let (Some(x), Some(y)) = (&a.session_id, &b.session_id) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (&a.id, &b.id) {
        return x == y;
    }
    matches!((a.pid, b.pid), (Some(x), Some(y)) if x == y)
}

/// Inputs to ownership resolution that do not change between polls.
pub struct Resolver<'a> {
    accounts: &'a [Account],
    home: &'a Path,
    environ: &'a dyn EnvironReader,
}

impl<'a> Resolver<'a> {
    /// `home` is used to expand `~` and for the default `~/.claude`.
    pub fn new(
        accounts: &'a [Account],
        home: &'a Path,
        environ: &'a dyn EnvironReader,
    ) -> Resolver<'a> {
        Resolver {
            accounts,
            home,
            environ,
        }
    }

    /// Merges the listings and resolves each row's owner. Output order: rows
    /// in order of first appearance, accounts in the given order.
    pub fn merge(&self, listings: &[AccountListing], sidecar: &Sidecar) -> Vec<MergedSession> {
        let mut merged: Vec<MergedSession> = Vec::new();
        // Parallel to `merged`: whether the kept row came from a stale listing.
        let mut from_stale: Vec<bool> = Vec::new();
        for listing in listings {
            for row in listing.sessions.iter().flatten() {
                match merged.iter().position(|m| same_session(&m.session, row)) {
                    Some(i) => {
                        let m = &mut merged[i];
                        if !m.seen_in.contains(&listing.account) {
                            m.seen_in.push(listing.account.clone());
                        }
                        let fresher = from_stale[i] && !listing.stale;
                        let livelier = !m.session.has_live_process() && row.has_live_process();
                        if fresher || (livelier && from_stale[i] == listing.stale) {
                            m.session = row.clone();
                            from_stale[i] = listing.stale;
                        }
                    }
                    None => {
                        merged.push(MergedSession {
                            session: row.clone(),
                            seen_in: vec![listing.account.clone()],
                            owner: Ownership::Unknown { config_dir: None },
                        });
                        from_stale.push(listing.stale);
                    }
                }
            }
        }
        let canon_dirs: Vec<PathBuf> = self
            .accounts
            .iter()
            .map(|a| self.canonical(&a.config_dir))
            .collect();
        // Rule 2 needs a successful, current poll from every configured account.
        let all_polled = self
            .accounts
            .iter()
            .all(|a| listings.iter().any(|l| l.account == a.name && l.is_fresh()));
        for m in &mut merged {
            m.owner = self.resolve(m, &canon_dirs, all_polled, sidecar);
        }
        merged
    }

    fn resolve(
        &self,
        m: &MergedSession,
        canon_dirs: &[PathBuf],
        all_polled: bool,
        sidecar: &Sidecar,
    ) -> Ownership {
        // Rule 1: live process.
        if let Some(pid) = m.session.pid
            && let Ok(var) = self.environ.config_dir_var(pid)
        {
            let raw = match var {
                Some(v) if !v.is_empty() => PathBuf::from(v),
                _ => PathBuf::from(DEFAULT_CLAUDE_CONFIG_DIR),
            };
            let canon = self.canonical(&raw);
            return match canon_dirs.iter().position(|d| *d == canon) {
                Some(i) => known(&self.accounts[i].name, AccountSource::Proc),
                None => Ownership::Unknown {
                    config_dir: Some(raw),
                },
            };
        }
        // Rule 2: listed by exactly one account, every other poll succeeded.
        // Needs at least two accounts to compare.
        if self.accounts.len() > 1
            && all_polled
            && let [account] = m.seen_in.as_slice()
            && self.accounts.iter().any(|a| a.name == *account)
        {
            return known(account, AccountSource::List);
        }
        // Rule 3: sidecar, if it names a configured account.
        if let Some((account, source)) = m.session.key().and_then(|k| sidecar.owner(k))
            && self.accounts.iter().any(|a| a.name == account)
        {
            return known(account, source.clone());
        }
        // Rule 4, except that the only configured account owns the rest.
        match self.accounts {
            [only] => known(&only.name, AccountSource::Implicit),
            _ => Ownership::Unknown { config_dir: None },
        }
    }

    /// `~` expanded and symlinks resolved; a path that cannot be resolved
    /// (e.g. does not exist) compares as given after expansion.
    fn canonical(&self, path: &Path) -> PathBuf {
        let expanded = match path.to_str() {
            Some(s) => paths::expand_tilde(s, self.home),
            None => path.to_path_buf(),
        };
        std::fs::canonicalize(&expanded).unwrap_or(expanded)
    }
}

fn known(account: &str, source: AccountSource) -> Ownership {
    Ownership::Known {
        account: account.to_owned(),
        source,
    }
}
