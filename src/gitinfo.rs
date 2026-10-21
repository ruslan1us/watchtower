//! The `git` line of the dashboard's details panel (doc/0037): the branch,
//! the number of changed files and the commits not pushed to the upstream
//! of a session's folder.
//!
//! The summary comes from one call, `git --no-pager --no-optional-locks -C
//! <cwd> status --porcelain=v2 --branch`, run like a background CLI call
//! (stdin null, captured output, a process group of its own, killed after
//! [`GIT_TIMEOUT`]) on a worker thread, with `GIT_OPTIONAL_LOCKS=0` and
//! `GIT_TERMINAL_PROMPT=0`. It reads git's state through git's own stable
//! porcelain format; it reads no Claude Code file (doc/0001).
//!
//! [`GitCache`] keeps one result per folder for [`GIT_TTL`], runs at most
//! one job per folder at a time and at most [`MAX_RUNNING`] in all. A
//! refresh keeps showing the previous result until the new one arrives.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cli::{self, CliError};

/// How long one `git status` may take before it is killed.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a folder's result is used before it is asked for again.
pub const GIT_TTL: Duration = Duration::from_secs(5);

/// The most `git status` calls running at once, over all folders.
pub const MAX_RUNNING: usize = 3;

/// Above this many folders, results older than [`GIT_TTL`] are dropped.
const PRUNE_ABOVE: usize = 64;

/// What `HEAD` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// A branch, also one without commits yet.
    Branch(String),
    /// A detached `HEAD`: the short commit id, empty when unknown.
    Detached(String),
}

/// The branch's upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upstream {
    /// No upstream configured (or a detached `HEAD`).
    None,
    /// An upstream is configured but git has no such ref.
    Gone,
    /// Commits ahead of and behind the upstream.
    Tracking { ahead: u32, behind: u32 },
}

/// The summary of one work tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSummary {
    pub head: Head,
    /// Changed files: staged, unstaged, unmerged and untracked entries,
    /// each file once.
    pub changed: usize,
    pub upstream: Upstream,
}

/// The result of asking git about a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitState {
    Summary(GitSummary),
    /// The folder is not inside a git work tree.
    NotRepo,
    /// The folder does not exist (any more).
    Missing,
    /// git is missing, timed out, failed or printed something else.
    Unavailable(String),
}

impl GitState {
    /// The summary, when there is one to show.
    pub fn summary(&self) -> Option<&GitSummary> {
        match self {
            GitState::Summary(s) => Some(s),
            _ => None,
        }
    }
}

/// Parses `git status --porcelain=v2 --branch`. `None` when the text has
/// no `# branch.head` header, i.e. is not that format.
pub fn parse_status(text: &str) -> Option<GitSummary> {
    let mut oid: Option<&str> = None;
    let mut head: Option<&str> = None;
    let mut upstream = false;
    let mut ab: Option<(u32, u32)> = None;
    let mut changed = 0;
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => oid = Some(value),
                "branch.head" => head = Some(value),
                "branch.upstream" => upstream = true,
                "branch.ab" => ab = parse_ab(value),
                _ => {}
            }
            continue;
        }
        // Ordinary (1), renamed or copied (2), unmerged (u) and untracked
        // (?) entries; ignored ones (!) are not changes.
        if matches!(line.split_once(' '), Some(("1" | "2" | "u" | "?", _))) {
            changed += 1;
        }
    }
    let head = match head? {
        "(detached)" => {
            let id = oid.filter(|o| *o != "(initial)").unwrap_or("");
            Head::Detached(id.chars().take(7).collect())
        }
        name => Head::Branch(name.to_owned()),
    };
    let upstream = match (upstream, ab) {
        (true, Some((ahead, behind))) => Upstream::Tracking { ahead, behind },
        (true, None) => Upstream::Gone,
        (false, _) => Upstream::None,
    };
    Some(GitSummary {
        head,
        changed,
        upstream,
    })
}

/// `+2 -1` → (2, 1).
fn parse_ab(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.split_whitespace();
    let ahead = parts.next()?.strip_prefix('+')?.parse().ok()?;
    let behind = parts.next()?.strip_prefix('-')?.parse().ok()?;
    Some((ahead, behind))
}

impl GitSummary {
    /// `feature/login`, or `detached 1a2b3c4`.
    pub fn head_text(&self) -> String {
        match &self.head {
            Head::Branch(name) => name.clone(),
            Head::Detached(id) if id.is_empty() => "detached".to_owned(),
            Head::Detached(id) => format!("detached {id}"),
        }
    }

    /// ` · 3 changed · ↑2`: everything after the branch.
    fn tail(&self) -> String {
        let changed = match self.changed {
            0 => "clean".to_owned(),
            n => format!("{n} changed"),
        };
        let upstream = match self.upstream {
            Upstream::None => "no upstream".to_owned(),
            Upstream::Gone => "upstream gone".to_owned(),
            Upstream::Tracking { ahead, behind: 0 } => format!("↑{ahead}"),
            Upstream::Tracking { ahead, behind } => format!("↑{ahead} ↓{behind}"),
        };
        format!(" · {changed} · {upstream}")
    }

    /// The whole line: `feature/login · 3 changed · ↑2`.
    pub fn text(&self) -> String {
        format!("{}{}", self.head_text(), self.tail())
    }

    /// [`GitSummary::text`] in at most `width` columns: a long branch name
    /// is cut first (ending in `…`), so the counts stay readable; only when
    /// they alone do not fit is the end of the line cut. Control characters
    /// are shown as `?`.
    pub fn fitted(&self, width: usize) -> String {
        let head = crate::attach::printable(&self.head_text());
        let tail = self.tail();
        let full = format!("{head}{tail}");
        if full.width() <= width {
            return full;
        }
        let room = width.saturating_sub(tail.width());
        if room >= 2 {
            return format!("{}{tail}", cut(&head, room));
        }
        cut(&full, width)
    }
}

/// `text` cut to `width` columns, ending in `…` when anything was cut.
fn cut(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Runs `git status` for a folder.
#[derive(Debug, Clone)]
pub struct GitRunner {
    program: OsString,
    timeout: Duration,
    env: Vec<(OsString, OsString)>,
}

impl Default for GitRunner {
    fn default() -> Self {
        GitRunner {
            program: "git".into(),
            timeout: GIT_TIMEOUT,
            env: Vec::new(),
        }
    }
}

impl GitRunner {
    /// `git` from `PATH`, with [`GIT_TIMEOUT`].
    pub fn new() -> GitRunner {
        GitRunner::default()
    }

    /// Runs `program` instead of `git` (tests).
    pub fn with_program(mut self, program: impl Into<OsString>) -> GitRunner {
        self.program = program.into();
        self
    }

    /// Sets the timeout (tests).
    pub fn with_timeout(mut self, timeout: Duration) -> GitRunner {
        self.timeout = timeout;
        self
    }

    /// Sets an extra variable for the call (tests: an empty global git
    /// config, so the user's own settings do not change the result).
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> GitRunner {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The command for `dir`: `git --no-pager --no-optional-locks -C <dir>
    /// status --porcelain=v2 --branch`, with `GIT_OPTIONAL_LOCKS=0` and
    /// `GIT_TERMINAL_PROMPT=0`.
    pub fn command(&self, dir: &Path) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg("--no-pager")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(dir)
            .args(["status", "--porcelain=v2", "--branch"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0");
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }

    /// Asks git about `dir`. Blocks up to the timeout: run it on a worker
    /// thread.
    pub fn status(&self, dir: &Path) -> GitState {
        if !dir.is_dir() {
            return GitState::Missing;
        }
        let program = self.program.to_string_lossy().into_owned();
        match cli::run_captured(self.command(dir), &program, self.timeout) {
            Ok(out) if out.success() => match parse_status(&out.stdout_text()) {
                Some(summary) => GitState::Summary(summary),
                None => GitState::Unavailable("unexpected output of git status".to_owned()),
            },
            Ok(out) if out.stderr.contains("not a git repository") => GitState::NotRepo,
            Ok(out) => GitState::Unavailable(
                out.stderr
                    .lines()
                    .next()
                    .unwrap_or("git status failed")
                    .to_owned(),
            ),
            Err(CliError::Spawn { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                GitState::Unavailable("git is not installed".to_owned())
            }
            Err(CliError::Timeout { .. }) => {
                GitState::Unavailable(format!("git status timed out after {:?}", self.timeout))
            }
            Err(err) => GitState::Unavailable(err.to_string()),
        }
    }
}

/// A `git status` to run on a worker thread for one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitJob {
    /// The folder as the session lists it: the cache's key.
    pub cwd: String,
}

impl GitJob {
    /// The folder to run in.
    pub fn dir(&self) -> PathBuf {
        PathBuf::from(&self.cwd)
    }
}

#[derive(Debug, Default)]
struct Cached {
    state: Option<GitState>,
    /// When `state` arrived.
    at: Option<Instant>,
    running: bool,
}

/// The git results by folder (doc/0037).
#[derive(Debug, Default)]
pub struct GitCache {
    entries: HashMap<String, Cached>,
}

impl GitCache {
    /// The summary to show for `cwd`, if any; also while a refresh runs.
    pub fn summary(&self, cwd: &str) -> Option<&GitSummary> {
        self.entries.get(cwd)?.state.as_ref()?.summary()
    }

    /// The last result for `cwd`, whatever it was.
    pub fn state(&self, cwd: &str) -> Option<&GitState> {
        self.entries.get(cwd)?.state.as_ref()
    }

    /// Whether a job for `cwd` is running.
    pub fn is_running(&self, cwd: &str) -> bool {
        self.entries.get(cwd).is_some_and(|c| c.running)
    }

    /// Number of jobs running.
    pub fn running(&self) -> usize {
        self.entries.values().filter(|c| c.running).count()
    }

    /// The job to run for `cwd` now, marked running: `None` when `cwd` is
    /// empty, a job for it runs, its result is younger than [`GIT_TTL`],
    /// or [`MAX_RUNNING`] jobs run.
    pub fn start(&mut self, cwd: &str, now: Instant) -> Option<GitJob> {
        if cwd.is_empty() || self.running() >= MAX_RUNNING {
            return None;
        }
        if let Some(c) = self.entries.get(cwd) {
            let fresh =
                c.at.is_some_and(|at| now.saturating_duration_since(at) < GIT_TTL);
            if c.running || fresh {
                return None;
            }
        }
        if self.entries.len() > PRUNE_ABOVE {
            self.entries.retain(|_, c| {
                c.running
                    || c.at
                        .is_some_and(|at| now.saturating_duration_since(at) < GIT_TTL)
            });
        }
        self.entries.entry(cwd.to_owned()).or_default().running = true;
        Some(GitJob {
            cwd: cwd.to_owned(),
        })
    }

    /// Records the result of `job`, which arrived at `now`.
    pub fn finished(&mut self, job: &GitJob, state: GitState, now: Instant) {
        let c = self.entries.entry(job.cwd.clone()).or_default();
        c.running = false;
        c.state = Some(state);
        c.at = Some(now);
    }
}
