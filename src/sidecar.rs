//! Tower's own state file, `state.json` next to `config.toml` (doc/0002).
//!
//! It holds only what the CLI does not expose. Milestone 1 uses the account
//! of each session; milestone 2 adds the `started` owner of dispatched
//! sessions and `recent_projects` (doc/0008). The other fields defined in
//! doc/0002 (`last_state`, `last_change_ms`, `acknowledged`) are modelled and
//! carried through unchanged, and unknown fields are kept.
//!
//! Writes are atomic (a temp file unique to the process and save, fsync,
//! rename). A corrupt file is moved aside as `state.json.corrupt` and tower
//! starts fresh with a warning.
//!
//! Several tower instances may share the file (doc/0017 section 8). A save
//! takes an exclusive lock on `state.json.lock`, re-reads the file, applies
//! only what this instance changed since its last load or save, writes the
//! result and adopts it as the new in-memory state. Records not seen in a
//! listing for a long time are pruned (doc/0017 section 9).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// How long [`Sidecar::save`] waits for the lock held by another instance
/// before it gives up and keeps the change for a later save.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

/// A record whose session no listing showed for this long is pruned.
pub const PRUNE_UNSEEN_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// A short-id `started` record whose row never appeared within this long of
/// its `updated_ms` is pruned (the dispatch never produced a listed row).
pub const PRUNE_UNSTARTED_MS: i64 = 60 * 60 * 1000;

/// `last_seen_ms` is rewritten only when it is at least this old, so a poll
/// that sees the same sessions does not cause a write every time.
pub const SEEN_RESOLUTION_MS: i64 = 60 * 60 * 1000;

/// After a failed save, how long before [`Sidecar::retry_due`] asks for
/// another attempt.
const RETRY_DELAY: Duration = Duration::from_secs(10);

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
    /// When tower last changed `account` / `account_source`, Unix ms. Decides
    /// between two instances that both changed the owner (doc/0017).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_ms: Option<i64>,
    /// When the session last appeared in a successful listing, Unix ms, to
    /// [`SEEN_RESOLUTION_MS`]. Drives pruning (doc/0017).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<i64>,
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
    /// What the file held at the last load or save, to tell this instance's
    /// changes from another instance's (merge on save).
    #[serde(skip)]
    tracking: Tracking,
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
            tracking: Tracking {
                base: Snapshot {
                    version: VERSION,
                    ..Snapshot::default()
                },
                retry_after: None,
            },
        }
    }
}

/// The persisted part of a [`Sidecar`], as of its last load or save.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Snapshot {
    version: u32,
    sessions: BTreeMap<String, SessionRecord>,
    recent_projects: Vec<RecentProject>,
    extra: Map<String, Value>,
}

/// Bookkeeping for merge on save. Not part of the sidecar's value: two
/// sidecars with the same content compare equal whatever their history.
#[derive(Debug, Clone, Default)]
struct Tracking {
    /// The state this instance last read from or wrote to the file. What
    /// differs from it in memory is this instance's change.
    base: Snapshot,
    /// Set by a failed save: when [`Sidecar::retry_due`] turns true.
    retry_after: Option<Instant>,
}

impl PartialEq for Tracking {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Tracking {}

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
                Ok(mut sidecar) => {
                    sidecar.tracking.base = sidecar.snapshot();
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

    /// Saves this instance's changes (doc/0017 section 8), waiting up to
    /// [`LOCK_WAIT`] for the lock. See [`Sidecar::save_with_wait`].
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        self.save_with_wait(path, LOCK_WAIT)
    }

    /// Saves this instance's changes. Under an exclusive lock on
    /// `<path>.lock`: re-reads `path`, applies on top of it only what this
    /// instance changed since its last load or save, writes the result
    /// atomically (a temp file unique to the process and save in the same
    /// directory, fsync, rename, fsync of the directory) and makes it the
    /// new in-memory state. A missing or corrupt file (the latter moved
    /// aside as on load) is replaced by the whole in-memory state. Creates
    /// the parent directory if needed.
    ///
    /// The lock is retried without blocking for up to `wait`. When it stays
    /// held, nothing is written and the error has kind
    /// [`std::io::ErrorKind::WouldBlock`]. After any failed save the change
    /// stays pending: a later save writes it, and [`Sidecar::retry_due`]
    /// says when to try again without a new change.
    pub fn save_with_wait(&mut self, path: &Path, wait: Duration) -> std::io::Result<()> {
        let result = self.save_locked(path, wait);
        self.tracking.retry_after = result.is_err().then(|| Instant::now() + RETRY_DELAY);
        result
    }

    fn save_locked(&mut self, path: &Path, wait: Duration) -> std::io::Result<()> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        if let Some(dir) = dir {
            std::fs::create_dir_all(dir)?;
        }
        let _lock = lock(&lock_path(path), wait)?;
        let mine = self.snapshot();
        let merged = match read_for_merge(path)? {
            Some(disk) => merge_snapshot(&self.tracking.base, &mine, disk),
            None => mine,
        };
        let out = Sidecar {
            version: merged.version,
            sessions: merged.sessions.clone(),
            recent_projects: merged.recent_projects.clone(),
            extra: merged.extra.clone(),
            tracking: Tracking::default(),
        };
        let json = serde_json::to_vec_pretty(&out).map_err(std::io::Error::other)?;
        write_atomic(path, dir, &json)?;
        self.version = out.version;
        self.sessions = out.sessions;
        self.recent_projects = out.recent_projects;
        self.extra = out.extra;
        self.tracking.base = merged;
        Ok(())
    }

    /// True when the in-memory state has changes no save has written yet.
    pub fn is_dirty(&self) -> bool {
        let base = &self.tracking.base;
        self.sessions != base.sessions
            || self.recent_projects != base.recent_projects
            || self.extra != base.extra
    }

    /// True when a save failed (for example on a lock timeout), the change
    /// is still pending and the retry delay has passed: the caller should
    /// save again even though nothing new changed.
    pub fn retry_due(&self) -> bool {
        self.tracking
            .retry_after
            .is_some_and(|at| Instant::now() >= at)
            && self.is_dirty()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            version: self.version,
            sessions: self.sessions.clone(),
            recent_projects: self.recent_projects.clone(),
            extra: self.extra.clone(),
        }
    }

    /// The recorded account and its source for a session key.
    pub fn owner(&self, key: &str) -> Option<(&str, &AccountSource)> {
        let rec = self.sessions.get(key)?;
        Some((rec.account.as_deref()?, rec.account_source.as_ref()?))
    }

    /// Records the account of a session. Returns true when this changed the
    /// state (so the caller knows a save is due; doc/0002 writes only on change).
    pub fn record_owner(&mut self, key: &str, account: &str, source: AccountSource) -> bool {
        self.record_owner_at(key, account, source, now_ms())
    }

    /// [`Sidecar::record_owner`] at a given time: a change sets the record's
    /// `updated_ms` to `now_ms`.
    pub fn record_owner_at(
        &mut self,
        key: &str,
        account: &str,
        source: AccountSource,
        now_ms: i64,
    ) -> bool {
        let rec = self.sessions.entry(key.to_owned()).or_default();
        if rec.account.as_deref() == Some(account) && rec.account_source.as_ref() == Some(&source) {
            return false;
        }
        rec.account = Some(account.to_owned());
        rec.account_source = Some(source);
        rec.updated_ms = Some(now_ms);
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
    /// the short-id record is dropped. The exception is a `list` record
    /// there: it yields its account and source to a short-id record of any
    /// other source (doc/0017). Rows without a `sessionId` keep their
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
            match self.sessions.get_mut(sid) {
                // A `list` detection never beats a record of another source
                // (doc/0017), even when it reached the sessionId key first,
                // e.g. a poll that ran before `record_started`.
                Some(existing)
                    if existing.account_source == Some(AccountSource::List)
                        && rec.account.is_some()
                        && !matches!(rec.account_source, None | Some(AccountSource::List)) =>
                {
                    existing.account = rec.account;
                    existing.account_source = rec.account_source;
                    // The evidence dates from the short-id record, so a
                    // later detection by another instance still wins.
                    existing.updated_ms = rec.updated_ms;
                }
                Some(_) => {}
                None => {
                    self.sessions.insert(sid.to_owned(), rec);
                }
            }
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
    /// result, as doc/0003 requires. A `proc` detection replaces any record:
    /// it is the live process's own config dir. A `list` detection is the
    /// weakest evidence and never replaces a record of another source
    /// (`started`, `handoff`, `asked`, `proc`; doc/0017). The implicit owner
    /// of single-account mode is not a detection and is never stored.
    /// Returns true when anything changed.
    pub fn record_detections(&mut self, merged: &[MergedSession]) -> bool {
        let mut changed = false;
        for m in merged {
            let (Some(key), Ownership::Known { account, source }) = (m.session.key(), &m.owner)
            else {
                continue;
            };
            let weaker_than_record = *source == AccountSource::List
                && self
                    .owner(key)
                    .is_some_and(|(_, recorded)| *recorded != AccountSource::List);
            if matches!(source, AccountSource::Proc | AccountSource::List) && !weaker_than_record {
                changed |= self.record_owner(key, account, source.clone());
            }
        }
        changed
    }

    /// Records one poll's listings for pruning (doc/0017 section 9). The app
    /// calls it once per applied poll with the keys of every row of every
    /// account's latest successful listing, and `complete` true only when
    /// every account's latest poll succeeded with a full listing.
    ///
    /// - A record whose key is in `seen` gets `last_seen_ms = now_ms` when
    ///   the stored value is missing or at least [`SEEN_RESOLUTION_MS`] old.
    /// - A record with neither `updated_ms` nor `last_seen_ms` (written by
    ///   an older tower) gets `updated_ms = now_ms`, so its age is known.
    /// - Only when `complete`: records not seen for [`PRUNE_UNSEEN_MS`]
    ///   (counting from `updated_ms` when never seen) are removed, and so are
    ///   short-id `started` records never seen [`PRUNE_UNSTARTED_MS`] after
    ///   their `updated_ms`.
    ///
    /// Returns true when the state changed. A removal is a change like any
    /// other, so it survives the merge on save.
    pub fn observe_listing<'a, I>(&mut self, seen: I, complete: bool, now_ms: i64) -> bool
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut changed = false;
        for key in seen {
            if let Some(rec) = self.sessions.get_mut(key)
                && rec
                    .last_seen_ms
                    .is_none_or(|t| now_ms.saturating_sub(t) >= SEEN_RESOLUTION_MS)
            {
                rec.last_seen_ms = Some(now_ms);
                changed = true;
            }
        }
        for rec in self.sessions.values_mut() {
            if rec.updated_ms.is_none() && rec.last_seen_ms.is_none() {
                rec.updated_ms = Some(now_ms);
                changed = true;
            }
        }
        if complete {
            let before = self.sessions.len();
            self.sessions.retain(|key, rec| !prunable(key, rec, now_ms));
            changed |= self.sessions.len() != before;
        }
        changed
    }
}

/// Whether [`Sidecar::observe_listing`] drops a record (doc/0017 section 9).
fn prunable(key: &str, rec: &SessionRecord, now_ms: i64) -> bool {
    let age = |t: Option<i64>| t.map(|t| now_ms.saturating_sub(t));
    let unstarted = rec.account_source == Some(AccountSource::Started)
        && rec.last_seen_ms.is_none()
        && is_short_id(key)
        && age(rec.updated_ms).is_some_and(|a| a >= PRUNE_UNSTARTED_MS);
    let unseen = age(rec.last_seen_ms.max(rec.updated_ms)).is_some_and(|a| a >= PRUNE_UNSEEN_MS);
    unstarted || unseen
}

/// A short `id` key rather than a `sessionId` (a UUID, which has dashes;
/// the short id is its first 8 hex digits, see the CLI reference).
fn is_short_id(key: &str) -> bool {
    !key.contains('-')
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Numbers the saves of this process, for [`tmp_path`].
static SAVES: AtomicU64 = AtomicU64::new(0);

/// A fresh temp file name next to `path`: `<path>.tmp.<pid>.<n>`, with `n`
/// new on every call, so two saves never share a temp file.
pub fn tmp_path(path: &Path) -> PathBuf {
    let n = SAVES.fetch_add(1, Ordering::Relaxed);
    sibling(path, &format!(".tmp.{}.{n}", std::process::id()))
}

/// `<path>.lock`, the lock file held around a save. It is never removed:
/// removing it would let two instances lock different files.
pub fn lock_path(path: &Path) -> PathBuf {
    sibling(path, ".lock")
}

/// Takes `flock(LOCK_EX)` on `path` (created if missing), retrying
/// `LOCK_NB` until `wait` has passed. The lock lasts as long as the file.
fn lock(path: &Path, wait: Duration) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let start = Instant::now();
    loop {
        // SAFETY: flock on the open descriptor of `file`, which outlives
        // the call; no memory is passed.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => {}
            Some(libc::EWOULDBLOCK) if start.elapsed() < wait => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Some(libc::EWOULDBLOCK) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!(
                        "{} is held by another tower instance; the change is kept for a later save",
                        path.display()
                    ),
                ));
            }
            _ => return Err(err),
        }
    }
}

/// Reads the file under the lock. `None` when there is nothing to merge
/// with: the file is missing, or corrupt (then moved aside, as on load).
fn read_for_merge(path: &Path) -> std::io::Result<Option<Snapshot>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    match serde_json::from_slice::<Sidecar>(&bytes) {
        Ok(disk) => Ok(Some(disk.snapshot())),
        Err(_) => {
            let _ = std::fs::rename(path, corrupt_path(path));
            Ok(None)
        }
    }
}

/// Writes `json` to a fresh temp file next to `path`, fsyncs it, renames
/// it over `path` and fsyncs the directory. The temp file is removed when
/// anything fails.
fn write_atomic(path: &Path, dir: Option<&Path>, json: &[u8]) -> std::io::Result<()> {
    let (tmp, mut file) = loop {
        let tmp = tmp_path(path);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            // Left over by a crashed process with the same pid.
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    };
    let written = file
        .write_all(json)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Some(dir) = dir {
        // Persist the rename itself; failure here is not fatal.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// `<path>.corrupt`, where a corrupt sidecar is moved.
pub fn corrupt_path(path: &Path) -> PathBuf {
    sibling(path, ".corrupt")
}

// ---- merge on save (doc/0017 section 8) ----------------------------------

/// Applies this instance's changes (`mine` compared with `base`, the state
/// of its last load or save) on top of what the file holds now (`disk`).
/// What this instance did not change comes from `disk`, including the
/// format version and unknown fields.
fn merge_snapshot(base: &Snapshot, mine: &Snapshot, disk: Snapshot) -> Snapshot {
    let mut out = disk;
    let keys: BTreeSet<&String> = base.sessions.keys().chain(mine.sessions.keys()).collect();
    for key in keys {
        let merged = merge_record(
            base.sessions.get(key),
            mine.sessions.get(key),
            out.sessions.get(key),
        );
        match merged {
            Some(rec) => out.sessions.insert(key.clone(), rec),
            None => out.sessions.remove(key),
        };
    }
    if mine.recent_projects != base.recent_projects {
        out.recent_projects = merge_recent(
            &base.recent_projects,
            &mine.recent_projects,
            &out.recent_projects,
        );
    }
    merge_map(&base.extra, &mine.extra, &mut out.extra);
    out
}

/// Merges one session record; `None` means no record.
///
/// - Unchanged here: the file's record.
/// - Removed here (pruned, or moved to its `sessionId`): removed, unless
///   the file's record changed since, which keeps it.
/// - Changed here and absent from the file (new, or removed by the other
///   instance): this instance's record.
/// - Otherwise field by field: a field changed only here is taken from
///   here. When both changed the owner, see [`mine_wins`]. `last_seen_ms`
///   takes the later time. Other fields changed here are taken from here.
fn merge_record(
    base: Option<&SessionRecord>,
    mine: Option<&SessionRecord>,
    disk: Option<&SessionRecord>,
) -> Option<SessionRecord> {
    if mine == base {
        return disk.cloned();
    }
    let Some(m) = mine else {
        return if disk == base { None } else { disk.cloned() };
    };
    let Some(d) = disk else {
        return Some(m.clone());
    };
    let empty = SessionRecord::default();
    let b = base.unwrap_or(&empty);
    let owner = |r: &SessionRecord| (r.account.clone(), r.account_source.clone());
    let mut out = d.clone();
    if owner(m) != owner(b) {
        if owner(d) == owner(b) || mine_wins(m, d) {
            out.account = m.account.clone();
            out.account_source = m.account_source.clone();
            out.updated_ms = m.updated_ms;
        }
    } else if out.updated_ms.is_none() {
        // Only the age stamp of an older record (see `observe_listing`).
        out.updated_ms = m.updated_ms;
    }
    if (&m.last_state, m.last_change_ms) != (&b.last_state, b.last_change_ms) {
        out.last_state = m.last_state.clone();
        out.last_change_ms = m.last_change_ms;
    }
    if m.acknowledged != b.acknowledged {
        out.acknowledged = m.acknowledged.clone();
    }
    if m.last_seen_ms != b.last_seen_ms {
        out.last_seen_ms = m.last_seen_ms.max(d.last_seen_ms);
    }
    merge_map(&b.extra, &m.extra, &mut out.extra);
    Some(out)
}

/// Both instances changed a record's owner: does this instance's win?
/// A `list` detection never beats another source (doc/0017 section 5, so a
/// `proc` detection never loses to a `list` one); otherwise the newer
/// `updated_ms` wins, and this instance on a tie.
fn mine_wins(mine: &SessionRecord, disk: &SessionRecord) -> bool {
    let is_list = |r: &SessionRecord| r.account_source == Some(AccountSource::List);
    match (is_list(mine), is_list(disk)) {
        (true, false) => false,
        (false, true) => true,
        _ => mine.updated_ms.unwrap_or(0) >= disk.updated_ms.unwrap_or(0),
    }
}

/// Applies the keys this instance added, changed or removed in a map of
/// unknown fields.
fn merge_map(base: &Map<String, Value>, mine: &Map<String, Value>, out: &mut Map<String, Value>) {
    let keys: BTreeSet<&String> = base.keys().chain(mine.keys()).collect();
    for key in keys {
        match (mine.get(key), base.get(key)) {
            (m, b) if m == b => {}
            (Some(v), _) => {
                out.insert(key.clone(), v.clone());
            }
            (None, _) => {
                out.remove(key);
            }
        }
    }
}

/// Merges `recent_projects` changed by both instances: the union of both
/// lists without the entries this instance dropped, one entry per path
/// with the later `last_used_ms`, newest first, capped at
/// [`MAX_RECENT_PROJECTS`].
fn merge_recent(
    base: &[RecentProject],
    mine: &[RecentProject],
    disk: &[RecentProject],
) -> Vec<RecentProject> {
    if disk == base {
        return mine.to_vec();
    }
    let has = |list: &[RecentProject], path: &str| list.iter().any(|e| e.path == path);
    let dropped_here = |e: &&RecentProject| has(base, &e.path) && !has(mine, &e.path);
    let mut out: Vec<RecentProject> = Vec::new();
    for entry in mine.iter().chain(disk.iter().filter(|e| !dropped_here(e))) {
        match out.iter_mut().find(|e| e.path == entry.path) {
            Some(e) if e.last_used_ms < entry.last_used_ms => *e = entry.clone(),
            Some(_) => {}
            None => out.push(entry.clone()),
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.last_used_ms));
    out.truncate(MAX_RECENT_PROJECTS);
    out
}
