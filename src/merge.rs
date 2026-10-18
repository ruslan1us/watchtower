//! Merging the listings of all accounts and resolving session ownership
//! (doc/0003, refined by doc/0017).
//!
//! Accounts that share session storage list the same sessions, so rows are
//! de-duplicated by `sessionId`, then short `id`, then `pid`. When several
//! listings hold the same session, the row of a fresh listing beats one of a
//! stale listing, and among equals the most recently received listing wins.
//! The owner of each merged row is resolved in this order, first match wins:
//!
//! 1. live process (`pid`), only for a row from a fresh listing and only
//!    when the process is the one the row describes: on the first sighting
//!    of a (session, pid) pair its `/proc/<pid>/cmdline` (or `exe`) must
//!    look like Claude Code ([`looks_like_claude`]), and its raw start ticks
//!    (`/proc/<pid>/stat` field 22) are remembered in memory
//!    ([`KnownProcesses`]); on later polls the ticks must be unchanged, so a
//!    reused pid gives no evidence. Then
//!    `CLAUDE_CONFIG_DIR` from `/proc/<pid>/environ` (missing or empty =
//!    `~/.claude`) is compared with each account's `config_dir` as canonical
//!    paths. A value that matches no account, or is not an absolute path,
//!    gives [`Ownership::Unknown`] with the raw value; an unreadable process
//!    falls through;
//! 2. seen in exactly one account's listing, and only when all of these
//!    hold: every configured account has a fresh listing from the same poll
//!    cycle, none of them is empty or partial (rows skipped by the parser),
//!    no row of this merge is listed by more than one account (the lists are
//!    visibly not shared), and the sidecar holds no record other than `list`
//!    for the session;
//! 3. the sidecar record;
//! 4. unknown (`?`).
//!
//! With a single configured account, rule 2 is skipped (one listing proves
//! nothing about shared lists) and rule 4 gives that account instead of `?`,
//! with source [`AccountSource::Implicit`], which is never written to the
//! sidecar: if a second account with a shared listing is added later, no
//! stale `list` records claim its sessions. Rule 1 still applies, so a live
//! process whose config dir is not the account's is `?` with the raw path.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::config::{Account, DEFAULT_CLAUDE_CONFIG_DIR};
use crate::listing::Session;
use crate::paths;
use crate::sidecar::{AccountSource, Sidecar};

/// The environment variable that selects a Claude Code config dir.
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// What tower reads about a listed process: its `CLAUDE_CONFIG_DIR`, its
/// command line and executable, and its raw start ticks. Abstracted so
/// tests can fake it.
pub trait EnvironReader {
    /// `Ok(Some(value))` when set, `Ok(None)` when the process has no such
    /// variable, `Err` when the environment cannot be read (other user's
    /// process, process gone, …).
    fn config_dir_var(&self, pid: u32) -> std::io::Result<Option<OsString>>;

    /// The raw, NUL-separated `/proc/<pid>/cmdline`. `Err` when it cannot be
    /// read.
    fn cmdline(&self, pid: u32) -> std::io::Result<Vec<u8>>;

    /// The target of `/proc/<pid>/exe`. `Err` when it cannot be read (it
    /// often cannot for another user's process).
    fn exe(&self, pid: u32) -> std::io::Result<PathBuf>;

    /// Field 22 of `/proc/<pid>/stat`: when the process started, in clock
    /// ticks after boot, as is. Only ever compared with an earlier reading
    /// of the same field, never with a wall-clock time. `Err` when it
    /// cannot be read.
    fn start_ticks(&self, pid: u32) -> std::io::Result<u64>;
}

/// Reads `/proc/<pid>/environ`, `/proc/<pid>/cmdline`, `/proc/<pid>/exe` and
/// `/proc/<pid>/stat`: the only OS-level reads tower does (doc/0001,
/// doc/0017).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcEnviron;

impl EnvironReader for ProcEnviron {
    fn config_dir_var(&self, pid: u32) -> std::io::Result<Option<OsString>> {
        let bytes = std::fs::read(format!("/proc/{pid}/environ"))?;
        Ok(find_var(&bytes, CONFIG_DIR_VAR))
    }

    fn cmdline(&self, pid: u32) -> std::io::Result<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/cmdline"))
    }

    fn exe(&self, pid: u32) -> std::io::Result<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe"))
    }

    fn start_ticks(&self, pid: u32) -> std::io::Result<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        parse_start_ticks(&stat).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected /proc/<pid>/stat",
            )
        })
    }
}

/// Field 22 (`starttime`, clock ticks after boot) of a `/proc/<pid>/stat`
/// line. The command name (field 2) is in parentheses and may itself hold
/// spaces and parentheses, so fields are counted after the last `)`.
pub fn parse_start_ticks(stat: &str) -> Option<u64> {
    let after = &stat[stat.rfind(')')? + 1..];
    // Field 3 is the first one after the name, so field 22 is the 20th.
    after.split_whitespace().nth(19)?.parse().ok()
}

/// Whether a process looks like a Claude Code worker, from its raw
/// `/proc/<pid>/cmdline` and, when readable, its `/proc/<pid>/exe` target
/// (doc/0017). Any of:
///
/// - `argv[0]` or the executable is a Claude Code path
///   ([`is_claude_path`]): a file named `claude`, or a path inside a Claude
///   Code installation (`…/claude/versions/<version>`,
///   `…/@anthropic-ai/claude-code/…`). The native binary lives under
///   `versions/<version>`, so its `exe` is named after the version; a
///   deleted executable (`… (deleted)` after an update) still counts;
/// - `argv[0]` or the executable is `node` (or `nodejs`), and its script,
///   the first argument that is not an option, is such a path.
pub fn looks_like_claude(cmdline: &[u8], exe: Option<&Path>) -> bool {
    let mut args = cmdline
        .split(|b| *b == 0)
        .map(|a| Path::new(OsStr::from_bytes(a)));
    let argv0 = args.next().filter(|a| !a.as_os_str().is_empty());
    if argv0.is_some_and(is_claude_path) || exe.is_some_and(is_claude_path) {
        return true;
    }
    let is_node = |p: &Path| {
        let p = strip_deleted(p);
        matches!(
            p.file_name().and_then(OsStr::to_str),
            Some("node" | "nodejs")
        )
    };
    if !(argv0.is_some_and(is_node) || exe.is_some_and(is_node)) {
        return false;
    }
    args.find(|a| {
        let bytes = a.as_os_str().as_bytes();
        !bytes.is_empty() && !bytes.starts_with(b"-")
    })
    .is_some_and(is_claude_path)
}

/// A file named `claude`, or a path with a Claude Code installation
/// segment: `claude/versions/<version>` or `@anthropic-ai/claude-code`.
pub fn is_claude_path(path: &Path) -> bool {
    let path = strip_deleted(path);
    if path.file_name() == Some(OsStr::new("claude")) {
        return true;
    }
    let parts: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    parts
        .windows(3)
        .any(|w| w[0] == "claude" && w[1] == "versions")
        || parts
            .windows(2)
            .any(|w| w[0] == "@anthropic-ai" && w[1] == "claude-code")
}

/// `path` without the ` (deleted)` the kernel appends to the `exe` link of
/// a process whose executable was removed or replaced.
fn strip_deleted(path: &Path) -> &Path {
    let bytes = path.as_os_str().as_bytes();
    Path::new(OsStr::from_bytes(
        bytes.strip_suffix(b" (deleted)").unwrap_or(bytes),
    ))
}

/// Finds `name=value` in a NUL-separated environment block.
pub fn find_var(environ: &[u8], name: &str) -> Option<OsString> {
    let prefix = format!("{name}=");
    environ
        .split(|b| *b == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .map(|value| OsStr::from_bytes(value).to_os_string())
}

/// The processes rule 1 has accepted, kept in memory between polls (never
/// in the sidecar, doc/0017): (session key, pid) → the raw start ticks
/// read on the first sighting. A later poll accepts the pair only while
/// the ticks are unchanged; both readings come from the boot-relative
/// clock, so a suspended host or a stepped wall clock does not matter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownProcesses {
    ticks: HashMap<(String, u32), u64>,
}

impl KnownProcesses {
    /// The remembered start ticks of `pid` for the session `key`.
    pub fn get(&self, key: &str, pid: u32) -> Option<u64> {
        self.ticks.get(&(key.to_owned(), pid)).copied()
    }

    /// Number of remembered pairs.
    pub fn len(&self) -> usize {
        self.ticks.len()
    }

    /// True when nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.ticks.is_empty()
    }
}

/// At most this many `CLAUDE_CONFIG_DIR` values are remembered by a
/// [`CanonCache`]; when full it starts over.
const CANON_CACHE_MAX: usize = 64;

/// Canonical forms of config dirs, so a merge does not resolve symlinks on
/// every poll: a `canonicalize` on a hung network mount would block the UI
/// thread (doc/0001). The accounts' config dirs are resolved once
/// ([`CanonCache::for_accounts`], at startup), and each `CLAUDE_CONFIG_DIR`
/// value read from a process once, keyed by the raw value. Callers that poll
/// repeatedly carry it across merges like [`KnownProcesses`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanonCache {
    /// The accounts' config dirs as configured and resolved, in config
    /// order.
    accounts: Option<(Vec<PathBuf>, Vec<PathBuf>)>,
    /// Raw process value → its canonical form.
    values: HashMap<PathBuf, PathBuf>,
}

impl CanonCache {
    /// A cache with the canonical config dirs of `accounts` (`~` expanded
    /// against `home`).
    pub fn for_accounts(accounts: &[Account], home: &Path) -> CanonCache {
        let mut cache = CanonCache::default();
        cache.accounts(accounts, home);
        cache
    }

    /// Number of remembered process values.
    pub fn values(&self) -> usize {
        self.values.len()
    }

    fn accounts(&mut self, accounts: &[Account], home: &Path) -> Vec<PathBuf> {
        let raw: Vec<PathBuf> = accounts.iter().map(|a| a.config_dir.clone()).collect();
        match &self.accounts {
            Some((known, dirs)) if *known == raw => dirs.clone(),
            _ => {
                let dirs: Vec<PathBuf> = raw.iter().map(|d| canonical(d, home)).collect();
                self.accounts = Some((raw, dirs.clone()));
                dirs
            }
        }
    }

    fn value(&mut self, raw: &Path, home: &Path) -> PathBuf {
        if let Some(c) = self.values.get(raw) {
            return c.clone();
        }
        if self.values.len() >= CANON_CACHE_MAX {
            self.values.clear();
        }
        let c = canonical(raw, home);
        self.values.insert(raw.to_path_buf(), c.clone());
        c
    }
}

/// `~` expanded and symlinks resolved; a path that cannot be resolved
/// (e.g. does not exist) compares as given after expansion.
fn canonical(path: &Path, home: &Path) -> PathBuf {
    let expanded = match path.to_str() {
        Some(s) => paths::expand_tilde(s, home),
        None => path.to_path_buf(),
    };
    crate::config::comparable_dir(&expanded)
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
    /// shown but are no evidence: neither rule 1 nor rule 2 uses them, and a
    /// fresh row from another account replaces a stale one in the merge.
    pub stale: bool,
    /// The poll cycle the listing came from ([`crate::poller::PollUpdate`]).
    /// Rule 2 needs every account's listing from the same cycle.
    pub cycle: u64,
    /// When the listing was received, as an increasing sequence number:
    /// among listings of equal freshness, the row of the most recently
    /// received one wins the merge.
    pub received: u64,
    /// True when the parser skipped rows of this listing (it came with
    /// warnings, doc/0001): a row missing from it may just be a skipped one,
    /// so the listing is no evidence for rule 2.
    pub partial: bool,
}

impl AccountListing {
    /// A successful, complete poll (cycle 0, received 0; see
    /// [`Self::with_cycle`], [`Self::with_received`] and
    /// [`Self::with_partial`]).
    pub fn fresh(account: impl Into<String>, sessions: Vec<Session>) -> AccountListing {
        AccountListing {
            account: account.into(),
            sessions: Some(sessions),
            stale: false,
            cycle: 0,
            received: 0,
            partial: false,
        }
    }

    /// No data: the account has not been polled yet, or its poll failed and
    /// there is nothing to keep.
    pub fn missing(account: impl Into<String>) -> AccountListing {
        AccountListing {
            sessions: None,
            ..AccountListing::fresh(account, Vec::new())
        }
    }

    /// The last good rows of an account whose latest poll failed.
    pub fn stale(account: impl Into<String>, sessions: Vec<Session>) -> AccountListing {
        AccountListing {
            stale: true,
            ..AccountListing::fresh(account, sessions)
        }
    }

    /// The same listing, from poll cycle `cycle`.
    pub fn with_cycle(mut self, cycle: u64) -> AccountListing {
        self.cycle = cycle;
        self
    }

    /// The same listing, received as number `received`.
    pub fn with_received(mut self, received: u64) -> AccountListing {
        self.received = received;
        self
    }

    /// The same listing, marked as missing skipped rows or not.
    pub fn with_partial(mut self, partial: bool) -> AccountListing {
        self.partial = partial;
        self
    }

    /// True when this is a successful, current poll result: the only kind
    /// that counts as evidence.
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
    /// `CLAUDE_CONFIG_DIR` of the live process when it matched no account
    /// or was not an absolute path.
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
    /// The row. When several accounts list it: a fresh listing's row beats
    /// a stale one's, then the most recently received listing's, then (on a
    /// tie) the first with a live process, then the first in config order.
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

/// Inputs to ownership resolution that do not change between polls, plus
/// the processes rule 1 accepted on earlier polls ([`KnownProcesses`]).
/// Callers that poll repeatedly carry those across merges with
/// [`Self::with_known`] and [`Self::into_known`].
pub struct Resolver<'a> {
    accounts: &'a [Account],
    home: &'a Path,
    environ: &'a dyn EnvironReader,
    known: KnownProcesses,
    canon: CanonCache,
}

/// Where the row kept for a merged session came from.
#[derive(Debug, Clone, Copy)]
struct Origin {
    stale: bool,
    received: u64,
}

impl Origin {
    /// Whether a row from `other` should replace one from `self`; `livelier`
    /// breaks ties.
    fn replaced_by(self, other: Origin, livelier: bool) -> bool {
        if self.stale != other.stale {
            return self.stale;
        }
        if self.received != other.received {
            return other.received > self.received;
        }
        livelier
    }
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
            known: KnownProcesses::default(),
            canon: CanonCache::default(),
        }
    }

    /// The same resolver, starting from the processes accepted earlier.
    pub fn with_known(mut self, known: KnownProcesses) -> Resolver<'a> {
        self.known = known;
        self
    }

    /// The processes accepted so far, to pass to the next poll's resolver.
    pub fn into_known(self) -> KnownProcesses {
        self.known
    }

    /// The same resolver, with canonical paths resolved earlier.
    pub fn with_canon(mut self, canon: CanonCache) -> Resolver<'a> {
        self.canon = canon;
        self
    }

    /// The processes accepted and the paths resolved so far, to pass to the
    /// next poll's resolver.
    pub fn into_parts(self) -> (KnownProcesses, CanonCache) {
        (self.known, self.canon)
    }

    /// The processes accepted so far.
    pub fn known(&self) -> &KnownProcesses {
        &self.known
    }

    /// Merges the listings and resolves each row's owner. Output order: rows
    /// in order of first appearance, accounts in the given order. Updates
    /// the remembered processes: pairs accepted for the first time are
    /// added, and pairs whose row is gone, whose pid changed or whose
    /// process no longer matches are forgotten.
    pub fn merge(&mut self, listings: &[AccountListing], sidecar: &Sidecar) -> Vec<MergedSession> {
        let mut merged: Vec<MergedSession> = Vec::new();
        // Parallel to `merged`: where the kept row came from.
        let mut origins: Vec<Origin> = Vec::new();
        for listing in listings {
            let origin = Origin {
                stale: listing.stale,
                received: listing.received,
            };
            for row in listing.sessions.iter().flatten() {
                match merged.iter().position(|m| same_session(&m.session, row)) {
                    Some(i) => {
                        let m = &mut merged[i];
                        if !m.seen_in.contains(&listing.account) {
                            m.seen_in.push(listing.account.clone());
                        }
                        let livelier = !m.session.has_live_process() && row.has_live_process();
                        if origins[i].replaced_by(origin, livelier) {
                            m.session = row.clone();
                            origins[i] = origin;
                        }
                    }
                    None => {
                        merged.push(MergedSession {
                            session: row.clone(),
                            seen_in: vec![listing.account.clone()],
                            owner: Ownership::Unknown { config_dir: None },
                        });
                        origins.push(origin);
                    }
                }
            }
        }
        let canon_dirs = self.canon.accounts(self.accounts, self.home);
        let rule_two = self.rule_two_applies(listings, &merged);
        for (m, origin) in merged.iter_mut().zip(&origins) {
            let listed = !origin.stale && self.is_listed_process(&m.session);
            m.owner = self.resolve(m, listed, &canon_dirs, rule_two, sidecar);
        }
        // Forget the pairs of rows that are gone or now carry another pid.
        // A row kept only from a stale snapshot keeps its pair.
        self.known.ticks.retain(|(key, pid), _| {
            merged
                .iter()
                .any(|m| m.session.pid == Some(*pid) && process_key(&m.session) == key)
        });
        merged
    }

    /// Whether this merge can use rule 2 at all (doc/0017): at least two
    /// accounts, each with a fresh, complete, non-empty listing, all from one
    /// poll cycle, and no session listed by more than one account.
    fn rule_two_applies(&self, listings: &[AccountListing], merged: &[MergedSession]) -> bool {
        if self.accounts.len() < 2 {
            return false;
        }
        let mut cycle = None;
        for a in self.accounts {
            let Some(l) = listings
                .iter()
                .find(|l| l.account == a.name && l.is_fresh())
            else {
                return false;
            };
            if l.partial || l.sessions.as_ref().is_none_or(Vec::is_empty) {
                // An empty listing may be a supervisor that is starting up,
                // a partial one may lack the skipped row: neither says where
                // the other rows live.
                return false;
            }
            if *cycle.get_or_insert(l.cycle) != l.cycle {
                return false;
            }
        }
        merged.iter().all(|m| m.seen_in.len() <= 1)
    }

    /// `listed`: the row is from a fresh listing and its `pid` passed the
    /// pid-reuse guard ([`Self::is_listed_process`]).
    fn resolve(
        &mut self,
        m: &MergedSession,
        listed: bool,
        canon_dirs: &[PathBuf],
        rule_two: bool,
        sidecar: &Sidecar,
    ) -> Ownership {
        // Rule 1: live process, from a fresh listing, verified not reused.
        if listed
            && let Some(pid) = m.session.pid
            && let Ok(var) = self.environ.config_dir_var(pid)
        {
            return self.owner_of_config_dir(var, canon_dirs);
        }
        let configured = |name: &str| self.accounts.iter().any(|a| a.name == name);
        let record = m.session.key().and_then(|k| sidecar.owner(k));
        // Rule 2: listed by exactly one account, under the conditions of
        // `rule_two_applies`, unless the sidecar has better evidence.
        let better_record = record
            .is_some_and(|(account, source)| *source != AccountSource::List && configured(account));
        if rule_two
            && !better_record
            && let [account] = m.seen_in.as_slice()
            && configured(account)
        {
            return known(account, AccountSource::List);
        }
        // Rule 3: sidecar, if it names a configured account.
        if let Some((account, source)) = record
            && configured(account)
        {
            return known(account, source.clone());
        }
        // Rule 4, except that the only configured account owns the rest.
        match self.accounts {
            [only] => known(&only.name, AccountSource::Implicit),
            _ => Ownership::Unknown { config_dir: None },
        }
    }

    /// The pid-reuse guard of rule 1 (doc/0017). On the first sighting of a
    /// (session, pid) pair the process must look like Claude Code
    /// ([`looks_like_claude`]); its raw start ticks are then remembered. On
    /// later polls the ticks must equal the remembered ones: different
    /// ticks mean another process got the pid, so the pair is forgotten and
    /// the next poll treats it as a first sighting again. An unreadable
    /// `cmdline` or `stat` gives no evidence. No wall-clock time is used.
    fn is_listed_process(&mut self, session: &Session) -> bool {
        let Some(pid) = session.pid else {
            return false;
        };
        let pair = (process_key(session).to_owned(), pid);
        // Ticks first: if the pid changes hands between the reads, the next
        // poll sees different ticks.
        let Ok(ticks) = self.environ.start_ticks(pid) else {
            self.known.ticks.remove(&pair);
            return false;
        };
        if let Some(&known) = self.known.ticks.get(&pair) {
            if known == ticks {
                return true;
            }
            self.known.ticks.remove(&pair);
            return false;
        }
        let Ok(cmdline) = self.environ.cmdline(pid) else {
            return false;
        };
        let exe = self.environ.exe(pid).ok();
        if !looks_like_claude(&cmdline, exe.as_deref()) {
            return false;
        }
        self.known.ticks.insert(pair, ticks);
        true
    }

    /// Maps a process's `CLAUDE_CONFIG_DIR` to an account. Unset or empty
    /// means `~/.claude`. A value that is not an absolute path (relative, or
    /// starting with `~`, which the process sees literally) depends on the
    /// process's working directory and is never guessed: `?` with the raw
    /// value.
    fn owner_of_config_dir(&mut self, var: Option<OsString>, canon_dirs: &[PathBuf]) -> Ownership {
        let raw = match var.filter(|v| !v.is_empty()) {
            // `canonical` expands the default's `~` against tower's home.
            None => PathBuf::from(DEFAULT_CLAUDE_CONFIG_DIR),
            Some(v) if Path::new(&v).is_absolute() => PathBuf::from(v),
            Some(v) => {
                return Ownership::Unknown {
                    config_dir: Some(PathBuf::from(v)),
                };
            }
        };
        let canon = self.canon.value(&raw, self.home);
        match canon_dirs.iter().position(|d| *d == canon) {
            Some(i) => known(&self.accounts[i].name, AccountSource::Proc),
            None => Ownership::Unknown {
                config_dir: Some(raw),
            },
        }
    }
}

fn known(account: &str, source: AccountSource) -> Ownership {
    Ownership::Known {
        account: account.to_owned(),
        source,
    }
}

/// The session part of a [`KnownProcesses`] pair: `sessionId`, else the
/// short id, else empty (a row with neither is matched by `pid` alone).
fn process_key(session: &Session) -> &str {
    session.key().unwrap_or("")
}
