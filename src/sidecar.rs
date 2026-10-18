//! Tower's own state file, `state.json` next to `config.toml` (doc/0002).
//!
//! It holds only what the CLI does not expose. Milestone 1 uses the account
//! of each session; milestone 2 adds the `started` owner of dispatched
//! sessions and `recent_projects` (doc/0008). The other fields defined in
//! doc/0002 (`last_state`, `last_change_ms`, `acknowledged`) are modelled and
//! carried through unchanged, and unknown fields are kept.
//!
//! Writes are atomic (`state.json.tmp`, fsync, rename). A corrupt file is moved
//! aside as `state.json.corrupt` and tower starts fresh with a warning.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::listing::{Session, State, Status, WaitingFor};
use crate::merge::{MergedSession, Ownership};
use crate::paths;

/// Current sidecar format version.
pub const VERSION: u32 = 1;

/// Maximum length of `recent_projects`; the least recently used entries are
/// dropped beyond it.
pub const MAX_RECENT_PROJECTS: usize = 50;

/// The current time as Unix milliseconds (the unit of `last_used_ms` and
/// `last_change_ms`). A clock before 1970 gives 0.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

string_enum! {
    /// How tower learned a session's account (doc/0002, doc/0003).
    pub enum AccountSource {
        /// Read from `/proc/<pid>/environ` (rule 1).
        Proc => "proc",
        /// Seen in only one account's list (rule 2).
        List => "list",
        /// Dispatched by tower.
        Started => "started",
        /// Moved by a hand-off (doc/0004).
        Handoff => "handoff",
        /// The user's answer for a session whose account was unknown.
        Asked => "asked",
        /// In memory only, never written: the single configured account,
        /// assumed for want of any evidence (see [`crate::merge`]).
        Implicit => "implicit",
    }
}

/// An observed (`state`, `status`, `waitingFor`) tuple (doc/0006, doc/0007).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateTuple {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(
        default,
        rename = "waitingFor",
        skip_serializing_if = "Option::is_none"
    )]
    pub waiting_for: Option<WaitingFor>,
}

/// What tower remembers about one session.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Account currently hosting the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// How [`SessionRecord::account`] was learned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_source: Option<AccountSource>,
    /// Last observed tuple (milestone 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<StateTuple>,
    /// When `last_state` was first observed, Unix ms (milestone 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_ms: Option<i64>,
    /// Tuple the user last acknowledged (milestone 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<StateTuple>,
    /// Fields this version does not know, kept as-is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One entry of `recent_projects` (doc/0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    #[serde(default)]
    pub last_used_ms: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The whole sidecar file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// Format version, for migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Per-session records keyed by `sessionId` (or the short `id` when a row
    /// has no `sessionId`, see [`crate::listing::Session::key`]).
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionRecord>,
    /// Most-recently-used projects for the new-session dialog.
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
    /// Top-level fields this version does not know, kept as-is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_version() -> u32 {
    VERSION
}

impl Default for Sidecar {
    fn default() -> Self {
        Sidecar {
            version: VERSION,
            sessions: BTreeMap::new(),
            recent_projects: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// Result of [`Sidecar::load`]: the state plus an optional warning for the UI.
#[derive(Debug)]
pub struct Loaded {
    pub sidecar: Sidecar,
    /// Set when the file was unreadable or corrupt and was started fresh.
    pub warning: Option<String>,
}

impl Sidecar {
    /// Loads `path`. A missing file gives an empty sidecar. An unreadable or
    /// corrupt file is renamed to `state.json.corrupt` (replacing an older
    /// one) and an empty sidecar is returned with a warning. Never fails.
    pub fn load(path: &Path) -> Loaded {
        let problem = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Sidecar>(&bytes) {
                Ok(sidecar) => {
                    return Loaded {
                        sidecar,
                        warning: None,
                    };
                }
                Err(err) => format!("corrupt: {err}"),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Loaded {
                    sidecar: Sidecar::default(),
                    warning: None,
                };
            }
            Err(err) => format!("unreadable: {err}"),
        };
        let aside = corrupt_path(path);
        let moved = match std::fs::rename(path, &aside) {
            Ok(()) => format!("moved to {}", aside.display()),
            Err(err) => format!("could not move it aside: {err}"),
        };
        Loaded {
            sidecar: Sidecar::default(),
            warning: Some(format!(
                "state file {} is {problem}; {moved}; starting with empty state",
                path.display()
            )),
        }
    }

    /// Writes atomically: `<path>.tmp` in the same directory, fsync, rename
    /// over `path`. Creates the parent directory if needed.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        if let Some(dir) = dir {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = tmp_path(path);
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        if let Some(dir) = dir {
            // Persist the rename itself; failure here is not fatal.
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(())
    }

    /// The recorded account and its source for a session key.
    pub fn owner(&self, key: &str) -> Option<(&str, &AccountSource)> {
        let rec = self.sessions.get(key)?;
        Some((rec.account.as_deref()?, rec.account_source.as_ref()?))
    }

    /// Records the account of a session. Returns true when this changed the
    /// state (so the caller knows a save is due; doc/0002 writes only on change).
    pub fn record_owner(&mut self, key: &str, account: &str, source: AccountSource) -> bool {
        let rec = self.sessions.entry(key.to_owned()).or_default();
        if rec.account.as_deref() == Some(account) && rec.account_source.as_ref() == Some(&source) {
            return false;
        }
        rec.account = Some(account.to_owned());
        rec.account_source = Some(source);
        true
    }

    /// Records the user's answer for a session whose account was unknown
    /// (`asked`, doc/0003). Returns true when the state changed.
    pub fn record_answer(&mut self, key: &str, account: &str) -> bool {
        self.record_owner(key, account, AccountSource::Asked)
    }

    /// Records the account of a session tower just dispatched
    /// (`account_source: started`, doc/0008).
    ///
    /// `claude --bg` reports only the short `id`, so the record is keyed by
    /// it until the listing shows the session's `sessionId`;
    /// [`Sidecar::adopt_short_id_records`] then moves it to the `sessionId`
    /// key that ownership rule 3 looks up (doc/0003). Returns true when the
    /// state changed.
    pub fn record_started(&mut self, id: &str, account: &str) -> bool {
        self.record_owner(id, account, AccountSource::Started)
    }

    /// Moves records keyed by a short `id` (see [`Sidecar::record_started`])
    /// to the `sessionId` of the listed row with that `id`. When a record
    /// under the `sessionId` already exists, it is newer evidence and wins;
    /// the short-id record is dropped. Rows without a `sessionId` keep their
    /// short-id key, as [`Session::key`] defines. Call it with the raw rows
    /// of a poll before merging. Returns true when the state changed.
    pub fn adopt_short_id_records<'a, I>(&mut self, sessions: I) -> bool
    where
        I: IntoIterator<Item = &'a Session>,
    {
        let mut changed = false;
        for s in sessions {
            let (Some(id), Some(sid)) = (s.id.as_deref(), s.session_id.as_deref()) else {
                continue;
            };
            if id == sid {
                continue;
            }
            let Some(rec) = self.sessions.remove(id) else {
                continue;
            };
            self.sessions.entry(sid.to_owned()).or_insert(rec);
            changed = true;
        }
        changed
    }

    /// Marks `project` as just used for a dispatch (doc/0008): its entry
    /// moves to the front of `recent_projects` with `last_used_ms = now_ms`,
    /// and the list is capped at [`MAX_RECENT_PROJECTS`].
    ///
    /// The path is stored in the `~/…` form (doc/0002 example). An existing
    /// entry matches when its `~`-expanded path, or its canonical path, is
    /// the same as `project`'s; its unknown fields are kept. Returns true
    /// when the state changed.
    pub fn touch_recent_project(&mut self, project: &Path, home: &Path, now_ms: i64) -> bool {
        let canon = std::fs::canonicalize(project).ok();
        let same = |entry: &RecentProject| {
            let expanded = paths::expand_tilde(&entry.path, home);
            expanded == project
                || canon
                    .as_ref()
                    .is_some_and(|c| std::fs::canonicalize(&expanded).ok().as_ref() == Some(c))
        };
        let stored = paths::shorten_home(&project.to_string_lossy(), home);
        let index = self.recent_projects.iter().position(&same);
        if index == Some(0) {
            let front = &self.recent_projects[0];
            if front.path == stored
                && front.last_used_ms == now_ms
                && self.recent_projects.len() <= MAX_RECENT_PROJECTS
                && !self.recent_projects[1..].iter().any(&same)
            {
                return false;
            }
        }
        let mut entry = match index {
            Some(i) => self.recent_projects.remove(i),
            None => RecentProject {
                path: String::new(),
                last_used_ms: 0,
                extra: Map::new(),
            },
        };
        entry.path = stored;
        entry.last_used_ms = now_ms;
        self.recent_projects.insert(0, entry);
        // Drop duplicates of the same folder (e.g. from a hand-edited file).
        let mut first = true;
        self.recent_projects
            .retain(|e| std::mem::take(&mut first) || !same(e));
        self.recent_projects.truncate(MAX_RECENT_PROJECTS);
        true
    }

    /// Stores every live detection (rule 1 `proc`, rule 2 `list`) from a merge
    /// result, as doc/0003 requires. The implicit owner of single-account mode
    /// is not a detection and is never stored. Returns true when anything
    /// changed.
    pub fn record_detections(&mut self, merged: &[MergedSession]) -> bool {
        let mut changed = false;
        for m in merged {
            let (Some(key), Ownership::Known { account, source }) = (m.session.key(), &m.owner)
            else {
                continue;
            };
            if matches!(source, AccountSource::Proc | AccountSource::List) {
                changed |= self.record_owner(key, account, source.clone());
            }
        }
        changed
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn tmp_path(path: &Path) -> PathBuf {
    sibling(path, ".tmp")
}

/// `<path>.corrupt`, where a corrupt sidecar is moved.
pub fn corrupt_path(path: &Path) -> PathBuf {
    sibling(path, ".corrupt")
}
