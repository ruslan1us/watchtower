//! Project discovery for the new-session dialog (doc/0008).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Map;
use watchtower::projects::{
    Project, ProjectList, discover, display_path, filter, is_git_repo, matches, scan_root,
};
use watchtower::sidecar::RecentProject;

/// A fake home with `workspace/` holding a few folders.
struct Tree {
    home: tempfile::TempDir,
}

impl Tree {
    fn new() -> Tree {
        let home = tempfile::tempdir().unwrap();
        let t = Tree { home };
        t.git_dir("workspace/demo-app");
        t.git_file("workspace/linked-worktree");
        t.dir("workspace/notes-plain");
        t.git_dir("workspace/Zeta-lib");
        t.git_dir("workspace/group/nested-repo");
        std::fs::write(t.path("workspace/README.txt"), "not a folder").unwrap();
        t
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.home().join(rel)
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.path(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn git_dir(&self, rel: &str) -> PathBuf {
        let p = self.dir(rel);
        std::fs::create_dir_all(p.join(".git")).unwrap();
        p
    }

    fn git_file(&self, rel: &str) -> PathBuf {
        let p = self.dir(rel);
        std::fs::write(p.join(".git"), "gitdir: /elsewhere/.git/worktrees/x\n").unwrap();
        p
    }

    fn discover(&self, roots: &[&str], extra: &[&str], recent: &[RecentProject]) -> ProjectList {
        let roots: Vec<PathBuf> = roots.iter().map(|r| self.path(r)).collect();
        let extra: Vec<PathBuf> = extra.iter().map(|r| self.path(r)).collect();
        discover(&roots, &extra, recent, self.home())
    }
}

fn recent(path: &str, last_used_ms: i64) -> RecentProject {
    RecentProject {
        path: path.into(),
        last_used_ms,
        extra: Map::new(),
    }
}

fn displays(list: &ProjectList) -> Vec<&str> {
    list.projects.iter().map(|p| p.display.as_str()).collect()
}

fn is_root() -> bool {
    watchtower::dispatch::is_root()
}

#[test]
fn scan_finds_git_dirs_and_git_files_one_level_deep() {
    let t = Tree::new();
    let (found, warnings) = scan_root(&t.path("workspace"));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        found,
        [
            t.path("workspace/Zeta-lib"),
            t.path("workspace/demo-app"),
            t.path("workspace/linked-worktree"),
        ]
    );
    assert!(is_git_repo(&t.path("workspace/linked-worktree")));
    assert!(!is_git_repo(&t.path("workspace/notes-plain")));
    assert!(!is_git_repo(&t.path("workspace/group")), "not recursive");
}

#[test]
fn discover_orders_alphabetically_without_history() {
    let t = Tree::new();
    let list = t.discover(&["workspace"], &[], &[]);
    assert!(list.warnings.is_empty(), "{:?}", list.warnings);
    assert_eq!(
        displays(&list),
        [
            "~/workspace/demo-app",
            "~/workspace/linked-worktree",
            "~/workspace/Zeta-lib",
        ],
        "case-insensitive alphabetical"
    );
    let p = &list.projects[0];
    assert_eq!(p.path, t.path("workspace/demo-app"));
    assert_eq!(p.last_used_ms, None);
    assert!(!p.extra);
}

#[test]
fn missing_root_is_a_warning_not_an_error() {
    let t = Tree::new();
    let list = t.discover(&["no-such-root", "workspace"], &[], &[]);
    assert_eq!(list.projects.len(), 3);
    assert_eq!(list.warnings.len(), 1);
    assert!(
        list.warnings[0].contains("no-such-root"),
        "{:?}",
        list.warnings
    );
}

#[test]
fn unreadable_folders_are_warnings() {
    if is_root() {
        eprintln!("skipped: permissions are not enforced for root");
        return;
    }
    let t = Tree::new();
    let locked_child = t.git_dir("workspace/locked-repo");
    let locked_root = t.git_dir("locked-root/inner");
    let locked_root = locked_root.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(&locked_root, std::fs::Permissions::from_mode(0o000)).unwrap();

    let list = t.discover(&["workspace", "locked-root"], &[], &[]);

    std::fs::set_permissions(&locked_child, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&locked_root, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(list.projects.len(), 3, "{:?}", displays(&list));
    assert_eq!(list.warnings.len(), 2, "{:?}", list.warnings);
    assert!(list.warnings.iter().any(|w| w.contains("locked-repo")));
    assert!(list.warnings.iter().any(|w| w.contains("locked-root")));
}

#[test]
fn extra_projects_are_listed_as_given_when_they_exist() {
    let t = Tree::new();
    t.dir("notes");
    std::fs::write(t.path("a-file"), "x").unwrap();
    let list = t.discover(&["workspace"], &["notes", "gone", "a-file"], &[]);
    let notes = list
        .projects
        .iter()
        .find(|p| p.display == "~/notes")
        .expect("non-git extra listed");
    assert!(notes.extra);
    assert_eq!(list.projects.len(), 4);
    assert_eq!(list.warnings.len(), 2, "{:?}", list.warnings);
    assert!(list.warnings.iter().any(|w| w.contains("gone")));
    assert!(list.warnings.iter().any(|w| w.contains("a-file")));
}

#[test]
fn duplicates_are_removed_by_canonical_path() {
    let t = Tree::new();
    std::os::unix::fs::symlink(t.path("workspace"), t.path("ws-link")).unwrap();
    let list = t.discover(
        &["workspace", "ws-link"],
        &["workspace/demo-app", "ws-link/Zeta-lib"],
        &[],
    );
    assert!(list.warnings.is_empty(), "{:?}", list.warnings);
    assert_eq!(
        displays(&list),
        [
            "~/workspace/demo-app",
            "~/workspace/linked-worktree",
            "~/workspace/Zeta-lib",
        ],
        "first occurrence wins, roots before extras"
    );
    assert!(list.projects.iter().all(|p| !p.extra));
}

#[test]
fn most_recently_used_first_then_alphabetical() {
    let t = Tree::new();
    t.git_dir("workspace/alpha");
    t.git_dir("workspace/beta");
    let recents = [
        recent("~/workspace/Zeta-lib", 100),
        // Absolute form matches too.
        recent(&t.path("workspace/linked-worktree").to_string_lossy(), 300),
        // A tie with linked-worktree: alphabetical between them.
        recent("~/workspace/beta", 300),
        // An older duplicate entry does not override the newer one.
        recent("~/workspace/Zeta-lib", 50),
        // Entries for folders that are gone match nothing.
        recent("~/workspace/deleted", 999),
    ];
    let list = t.discover(&["workspace"], &[], &recents);
    assert_eq!(
        displays(&list),
        [
            "~/workspace/beta",
            "~/workspace/linked-worktree",
            "~/workspace/Zeta-lib",
            "~/workspace/alpha",
            "~/workspace/demo-app",
        ]
    );
    assert_eq!(list.projects[0].last_used_ms, Some(300));
    assert_eq!(list.projects[2].last_used_ms, Some(100));
    assert_eq!(list.projects[3].last_used_ms, None);
}

#[test]
fn recent_entry_matches_through_symlink() {
    let t = Tree::new();
    std::os::unix::fs::symlink(t.path("workspace"), t.path("ws-link")).unwrap();
    let list = t.discover(&["workspace"], &[], &[recent("~/ws-link/demo-app", 7)]);
    assert_eq!(list.projects[0].display, "~/workspace/demo-app");
    assert_eq!(list.projects[0].last_used_ms, Some(7));
}

#[test]
fn filter_is_case_insensitive_and_word_based() {
    let t = Tree::new();
    let list = t.discover(&["workspace"], &[], &[]);
    let names = |v: Vec<&Project>| v.into_iter().map(|p| p.display.clone()).collect::<Vec<_>>();
    assert_eq!(names(list.filter("")).len(), 3);
    assert_eq!(names(list.filter("ZETA")), ["~/workspace/Zeta-lib"]);
    assert_eq!(
        names(filter(&list.projects, "demo")),
        ["~/workspace/demo-app"]
    );
    assert_eq!(
        names(list.filter("work tree")),
        ["~/workspace/linked-worktree"]
    );
    assert!(list.filter("nothing-matches").is_empty());
    assert!(matches(&list.projects[0], "   "));
}

#[test]
fn display_shortens_home() {
    let home = Path::new("/home/user");
    assert_eq!(
        display_path(Path::new("/home/user/workspace/demo-app"), home),
        "~/workspace/demo-app"
    );
    assert_eq!(display_path(Path::new("/srv/repo"), home), "/srv/repo");
    assert_eq!(
        display_path(Path::new("/home/username/x"), home),
        "/home/username/x"
    );
}
