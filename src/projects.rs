//! Candidate projects for the new-session dialog (doc/0008).
//!
//! A candidate is a git clone found one level deep under a `project_roots`
//! entry (a child folder containing `.git`, as a directory or, for worktrees
//! and submodules, a file), or an `extra_projects` entry, taken as given when
//! it exists. Candidates are de-duplicated by canonical path, then ordered
//! most-recently-used first by the sidecar's `recent_projects` (doc/0002),
//! with ties and never-used projects alphabetical.
//!
//! The scan is a cheap `read_dir` per root, never recursive. Missing roots,
//! unreadable folders and missing extra entries never fail the scan; they are
//! collected as warnings for the UI to show.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::paths;
use crate::sidecar::RecentProject;

/// One candidate project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The folder as found (root joined with the child name, or the extra
    /// entry with `~` expanded). Dispatch runs here; symlinks are kept, so
    /// the session's `cwd` reads the way the user configured it.
    pub path: PathBuf,
    /// The canonical path (symlinks resolved), used for de-duplication and
    /// for matching `recent_projects`. Equals `path` when it cannot be
    /// resolved.
    pub canonical: PathBuf,
    /// `path` for display, with the home directory shortened to `~`.
    pub display: String,
    /// When tower last dispatched in this project (Unix ms), if recorded.
    pub last_used_ms: Option<i64>,
    /// True when the project came from `extra_projects` (it need not be a
    /// git clone).
    pub extra: bool,
}

/// The result of [`discover`]: ordered candidates plus non-fatal warnings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectList {
    /// Candidates in display order (most recently used first).
    pub projects: Vec<Project>,
    /// One message per problem (missing root, unreadable folder, …). Never
    /// printed by the library.
    pub warnings: Vec<String>,
}

impl ProjectList {
    /// The projects whose display path matches `query`; see [`matches`].
    pub fn filter(&self, query: &str) -> Vec<&Project> {
        filter(&self.projects, query)
    }
}

/// True when `dir` is a git work tree root: it contains `.git`, either a
/// directory (a normal clone) or a file (a worktree or submodule). Errors,
/// e.g. an unreadable `dir`, count as "no".
pub fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// Finds the git clones directly under `root`, sorted by path. A missing or
/// unreadable root yields no projects and a warning, as does a child folder
/// whose contents cannot be checked.
pub fn scan_root(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!("project root {}: {err}", root.display()));
            return (found, warnings);
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warnings.push(format!("project root {}: {err}", root.display()));
                continue;
            }
        };
        let child = entry.path();
        // Follows symlinks: a link to a clone counts like the clone.
        if !child.is_dir() {
            continue;
        }
        match child.join(".git").symlink_metadata() {
            Ok(_) => found.push(child),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warnings.push(format!("cannot check {}: {err}", child.display())),
        }
    }
    found.sort();
    (found, warnings)
}

/// Lists the candidate projects (doc/0008).
///
/// - `roots`: `project_roots` from the config (`~` already expanded);
/// - `extra`: `extra_projects` (`~` already expanded), listed when they exist
///   as folders, git clone or not;
/// - `recent`: the sidecar's `recent_projects`; their paths may use `~`;
/// - `home`: for `~` expansion and display.
///
/// Projects are de-duplicated by canonical path (the first occurrence wins,
/// roots before extras). Order: by `last_used_ms`, newest first, then
/// alphabetically by display path (case-insensitive).
pub fn discover(
    roots: &[PathBuf],
    extra: &[PathBuf],
    recent: &[RecentProject],
    home: &Path,
) -> ProjectList {
    let mut list = ProjectList::default();
    let mut found: Vec<(PathBuf, bool)> = Vec::new();
    for root in roots {
        let (repos, warnings) = scan_root(root);
        found.extend(repos.into_iter().map(|p| (p, false)));
        list.warnings.extend(warnings);
    }
    for path in extra {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => found.push((path.clone(), true)),
            Ok(_) => list
                .warnings
                .push(format!("extra project {} is not a folder", path.display())),
            Err(err) => list
                .warnings
                .push(format!("extra project {}: {err}", path.display())),
        }
    }

    // Newest use per canonical path; recent entries that no longer exist
    // simply match nothing.
    let mut used: HashMap<PathBuf, i64> = HashMap::new();
    for r in recent {
        let canon = canonical(&paths::expand_tilde(&r.path, home));
        let slot = used.entry(canon).or_insert(r.last_used_ms);
        *slot = (*slot).max(r.last_used_ms);
    }

    for (path, is_extra) in found {
        let canon = canonical(&path);
        if list.projects.iter().any(|p| p.canonical == canon) {
            continue;
        }
        list.projects.push(Project {
            display: display_path(&path, home),
            last_used_ms: used.get(&canon).copied(),
            canonical: canon,
            path,
            extra: is_extra,
        });
    }
    sort_projects(&mut list.projects);
    list
}

/// Sorts most-recently-used first, then alphabetically by display path
/// (case-insensitive, then exact as a tie-break).
pub fn sort_projects(projects: &mut [Project]) {
    projects.sort_by(|a, b| {
        b.last_used_ms
            .cmp(&a.last_used_ms)
            .then_with(|| a.display.to_lowercase().cmp(&b.display.to_lowercase()))
            .then_with(|| a.display.cmp(&b.display))
    });
}

/// Type-to-filter: true when every whitespace-separated word of `query`
/// occurs in `project.display`, ignoring case. An empty query matches all.
pub fn matches(project: &Project, query: &str) -> bool {
    let haystack = project.display.to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// The projects matching `query` (see [`matches`]), in their given order.
pub fn filter<'a>(projects: &'a [Project], query: &str) -> Vec<&'a Project> {
    projects.iter().filter(|p| matches(p, query)).collect()
}

/// A path for display, with `home` shortened to `~` (e.g.
/// `~/workspace/demo-app`). Non-UTF-8 parts are shown lossily.
pub fn display_path(path: &Path, home: &Path) -> String {
    paths::shorten_home(&path.to_string_lossy(), home)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
