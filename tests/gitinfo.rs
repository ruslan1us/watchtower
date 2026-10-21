//! The details panel's `git` line (doc/0037): parsing `git status
//! --porcelain=v2 --branch`, running it (real `git` in temp repos, a
//! missing folder, a folder outside any repo, no git, a timeout), the
//! cache with one job per folder, the job on a worker thread, and when the
//! app asks for it. No Claude Code runs.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use watchtower::app::{App, Effect};
use watchtower::cli::CliRunner;
use watchtower::config::UiStyle;
use watchtower::gitinfo::{
    GIT_TTL, GitCache, GitJob, GitRunner, GitState, GitSummary, Head, MAX_RUNNING, Upstream,
    parse_status,
};
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;

// ---- parsing ----------------------------------------------------------------

const AHEAD: &str = "\
# branch.oid 1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d
# branch.head feature/login
# branch.upstream origin/feature/login
# branch.ab +2 -0
1 .M N... 100644 100644 100644 1111111111111111111111111111111111111111 1111111111111111111111111111111111111111 src/login.rs
1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 2222222222222222222222222222222222222222 src/session.rs
? notes.txt
";

const DETACHED: &str = "\
# branch.oid 9c6e1a35f00dbabe0123456789abcdef01234567
# branch.head (detached)
";

const NO_UPSTREAM: &str = "\
# branch.oid 5e6f7a8b00000000000000000000000000000000
# branch.head main
1 MM N... 100644 100644 100644 3333333333333333333333333333333333333333 4444444444444444444444444444444444444444 README.md
";

const AHEAD_BEHIND: &str = "\
# branch.oid 5e6f7a8b00000000000000000000000000000000
# branch.head main
# branch.upstream origin/main
# branch.ab +3 -5
";

const UNTRACKED_AND_IGNORED: &str = "\
# branch.oid 5e6f7a8b00000000000000000000000000000000
# branch.head main
? scratch.txt
? docs/new page.md
! target/
";

const RENAMES: &str = "\
# branch.oid 5e6f7a8b00000000000000000000000000000000
# branch.head topic
# branch.upstream origin/topic
# branch.ab +0 -0
2 R. N... 100644 100644 100644 5555555555555555555555555555555555555555 5555555555555555555555555555555555555555 R100 src/new_name.rs\tsrc/old_name.rs
2 C. N... 100644 100644 100644 6666666666666666666666666666666666666666 6666666666666666666666666666666666666666 C75 src/copy.rs\tsrc/orig.rs
u UU N... 100644 100644 100644 100644 7777777777777777777777777777777777777777 8888888888888888888888888888888888888888 9999999999999999999999999999999999999999 src/conflict.rs
";

const INITIAL: &str = "\
# branch.oid (initial)
# branch.head main
? first.txt
";

const UPSTREAM_GONE: &str = "\
# branch.oid 5e6f7a8b00000000000000000000000000000000
# branch.head old-topic
# branch.upstream origin/old-topic
";

#[test]
fn a_branch_ahead_of_its_upstream() {
    let s = parse_status(AHEAD).unwrap();
    assert_eq!(s.head, Head::Branch("feature/login".into()));
    assert_eq!(s.changed, 3, "staged, unstaged and untracked");
    assert_eq!(
        s.upstream,
        Upstream::Tracking {
            ahead: 2,
            behind: 0
        }
    );
    assert_eq!(s.text(), "feature/login · 3 changed · ↑2");
}

#[test]
fn a_detached_head_shows_the_short_commit() {
    let s = parse_status(DETACHED).unwrap();
    assert_eq!(s.head, Head::Detached("9c6e1a3".into()));
    assert_eq!(s.upstream, Upstream::None);
    assert_eq!(s.text(), "detached 9c6e1a3 · clean · no upstream");
}

#[test]
fn a_branch_without_upstream() {
    let s = parse_status(NO_UPSTREAM).unwrap();
    assert_eq!(s.changed, 1, "a file both staged and changed counts once");
    assert_eq!(s.text(), "main · 1 changed · no upstream");
}

#[test]
fn ahead_and_behind() {
    let s = parse_status(AHEAD_BEHIND).unwrap();
    assert_eq!(
        s.upstream,
        Upstream::Tracking {
            ahead: 3,
            behind: 5
        }
    );
    assert_eq!(s.text(), "main · clean · ↑3 ↓5");
}

#[test]
fn untracked_files_count_and_ignored_ones_do_not() {
    let s = parse_status(UNTRACKED_AND_IGNORED).unwrap();
    assert_eq!(s.changed, 2);
}

#[test]
fn renamed_copied_and_unmerged_entries_count_once_each() {
    let s = parse_status(RENAMES).unwrap();
    assert_eq!(s.changed, 3);
    assert_eq!(s.text(), "topic · 3 changed · ↑0");
}

#[test]
fn a_branch_without_commits() {
    let s = parse_status(INITIAL).unwrap();
    assert_eq!(s.head, Head::Branch("main".into()));
    assert_eq!(s.text(), "main · 1 changed · no upstream");
}

#[test]
fn an_upstream_that_is_gone() {
    let s = parse_status(UPSTREAM_GONE).unwrap();
    assert_eq!(s.upstream, Upstream::Gone);
    assert_eq!(s.text(), "old-topic · clean · upstream gone");
}

#[test]
fn other_text_is_not_a_summary() {
    assert_eq!(parse_status(""), None);
    assert_eq!(parse_status("On branch main\nnothing to commit\n"), None);
}

#[test]
fn a_long_branch_name_is_cut_before_the_counts() {
    let s = GitSummary {
        head: Head::Branch("feature/a-very-long-branch-name-that-goes-on".into()),
        changed: 3,
        upstream: Upstream::Tracking {
            ahead: 2,
            behind: 0,
        },
    };
    let fitted = s.fitted(30);
    assert_eq!(fitted, "feature/a-ve… · 3 changed · ↑2");
    assert_eq!(unicode_width::UnicodeWidthStr::width(fitted.as_str()), 30);
    // Too narrow for the counts: the end of the line is cut.
    let tiny = s.fitted(12);
    assert_eq!(unicode_width::UnicodeWidthStr::width(tiny.as_str()), 12);
    assert!(tiny.ends_with('…'), "{tiny}");
    // Wide enough: unchanged.
    assert_eq!(s.fitted(80), s.text());
}

#[test]
fn control_characters_in_a_branch_name_are_not_drawn() {
    let s = GitSummary {
        head: Head::Branch("bad\u{1b}[31m".into()),
        changed: 0,
        upstream: Upstream::None,
    };
    assert_eq!(s.fitted(80), "bad?[31m · clean · no upstream");
}

// ---- running git --------------------------------------------------------------

/// Serializes the tests that write executables or start processes, so a
/// script being written is never inherited by a fork (ETXTBSY).
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A git runner that ignores the user's and the system's git config and
/// never looks above `ceiling` for a repository.
fn runner(ceiling: &Path) -> GitRunner {
    GitRunner::new()
        .with_env("GIT_CONFIG_GLOBAL", "/dev/null")
        .with_env("GIT_CONFIG_NOSYSTEM", "1")
        .with_env("GIT_CEILING_DIRECTORIES", ceiling)
}

/// Runs `git <args>` in `dir` for a test's setup.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit on `main` in a new temp dir.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("work");
    std::fs::create_dir(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    (tmp, dir)
}

fn summary(state: GitState) -> GitSummary {
    match state {
        GitState::Summary(s) => s,
        other => panic!("not a summary: {other:?}"),
    }
}

#[test]
fn the_command_asks_porcelain_v2_without_locks_or_prompts() {
    let cmd = GitRunner::new().command(Path::new("/home/user/workspace/api"));
    assert_eq!(cmd.get_program(), "git");
    let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
    assert_eq!(
        args,
        [
            "--no-pager",
            "--no-optional-locks",
            "-C",
            "/home/user/workspace/api",
            "status",
            "--porcelain=v2",
            "--branch"
        ]
    );
    let envs: Vec<_> = cmd
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
        .collect();
    assert!(envs.contains(&("GIT_OPTIONAL_LOCKS".into(), Some("0".into()))));
    assert!(envs.contains(&("GIT_TERMINAL_PROMPT".into(), Some("0".into()))));
}

#[test]
fn a_real_repository_with_changes_and_no_upstream() {
    let _l = lock();
    let (tmp, dir) = repo();
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    std::fs::write(dir.join("staged.txt"), "staged\n").unwrap();
    git(&dir, &["add", "staged.txt"]);
    let s = summary(runner(tmp.path()).status(&dir));
    assert_eq!(s.head, Head::Branch("main".into()));
    assert_eq!(s.changed, 3);
    assert_eq!(s.upstream, Upstream::None);
    assert_eq!(s.text(), "main · 3 changed · no upstream");
    // A subfolder is inside the same work tree.
    std::fs::create_dir(dir.join("sub")).unwrap();
    assert_eq!(summary(runner(tmp.path()).status(&dir.join("sub"))), s);
}

#[test]
fn a_real_repository_ahead_of_its_upstream() {
    let _l = lock();
    let (tmp, dir) = repo();
    let remote = tmp.path().join("remote.git");
    git(tmp.path(), &["init", "-q", "--bare", "remote.git"]);
    git(
        &dir,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    git(&dir, &["push", "-q", "-u", "origin", "main"]);
    std::fs::write(dir.join("README.md"), "two\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "second"]);
    std::fs::write(dir.join("README.md"), "three\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "third"]);
    let s = summary(runner(tmp.path()).status(&dir));
    assert_eq!(s.text(), "main · clean · ↑2");
}

#[test]
fn a_real_detached_head() {
    let _l = lock();
    let (tmp, dir) = repo();
    git(&dir, &["checkout", "-q", "--detach"]);
    let s = summary(runner(tmp.path()).status(&dir));
    let Head::Detached(id) = &s.head else {
        panic!("{s:?}")
    };
    assert_eq!(id.len(), 7);
    assert!(s.text().starts_with("detached "), "{}", s.text());
}

#[test]
fn a_folder_outside_any_repository() {
    let _l = lock();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plain");
    std::fs::create_dir(&dir).unwrap();
    assert_eq!(runner(tmp.path()).status(&dir), GitState::NotRepo);
}

#[test]
fn a_folder_that_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("removed");
    assert_eq!(runner(tmp.path()).status(&gone), GitState::Missing);
}

#[test]
fn no_git_installed() {
    let _l = lock();
    let tmp = tempfile::tempdir().unwrap();
    let state = runner(tmp.path())
        .with_program(tmp.path().join("no-such-git"))
        .status(tmp.path());
    assert_eq!(
        state,
        GitState::Unavailable("git is not installed".to_owned())
    );
}

/// An executable `git` stand-in running `body`.
fn fake_git(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("git");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_hanging_git_is_killed_after_the_timeout() {
    let _l = lock();
    let tmp = tempfile::tempdir().unwrap();
    let program = fake_git(tmp.path(), "sleep 30");
    let start = Instant::now();
    let state = runner(tmp.path())
        .with_program(program)
        .with_timeout(Duration::from_millis(200))
        .status(tmp.path());
    let GitState::Unavailable(why) = state else {
        panic!("{state:?}")
    };
    assert!(why.contains("timed out"), "{why}");
    // Killed, not waited for: far less than the script's 30 s.
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn unexpected_output_is_no_summary() {
    let _l = lock();
    let tmp = tempfile::tempdir().unwrap();
    let program = fake_git(tmp.path(), "echo 'On branch main'");
    let state = runner(tmp.path()).with_program(program).status(tmp.path());
    assert!(matches!(state, GitState::Unavailable(_)), "{state:?}");
    assert_eq!(state.summary(), None);
}

#[test]
fn the_job_runs_on_a_worker_thread_and_reports_once() {
    let _l = lock();
    let (tmp, dir) = repo();
    let jobs = Jobs::new(CliRunner::new()).with_git_runner(runner(tmp.path()));
    let job = GitJob {
        cwd: dir.to_string_lossy().into_owned(),
    };
    jobs.git(job.clone());
    let Some(JobResult::Git(done, state)) = jobs.recv_timeout(Duration::from_secs(30)) else {
        panic!("no git result")
    };
    assert_eq!(done, job);
    assert_eq!(summary(state).text(), "main · clean · no upstream");
    assert!(jobs.try_recv().is_none(), "exactly one result");
}

// ---- the cache ------------------------------------------------------------------

fn clean(branch: &str) -> GitState {
    GitState::Summary(GitSummary {
        head: Head::Branch(branch.into()),
        changed: 0,
        upstream: Upstream::None,
    })
}

#[test]
fn one_job_per_folder_and_results_kept_for_the_ttl() {
    let mut cache = GitCache::default();
    let t0 = Instant::now();
    let job = cache.start("/w/api", t0).expect("first ask");
    assert_eq!(job.cwd, "/w/api");
    assert!(cache.is_running("/w/api"));
    // Single flight: no second job while the first runs.
    assert_eq!(cache.start("/w/api", t0), None);
    assert_eq!(cache.start("/w/api", t0 + GIT_TTL * 3), None);
    // Another folder may run meanwhile.
    assert!(cache.start("/w/web", t0).is_some());
    assert_eq!(cache.running(), 2);

    cache.finished(&job, clean("main"), t0);
    assert!(!cache.is_running("/w/api"));
    assert_eq!(
        cache.summary("/w/api").unwrap().text(),
        "main · clean · no upstream"
    );
    // Fresh: not asked again.
    assert_eq!(cache.start("/w/api", t0 + GIT_TTL / 2), None);
    // Expired: asked again, and the old result stays shown meanwhile.
    let again = cache.start("/w/api", t0 + GIT_TTL).expect("expired");
    assert!(cache.summary("/w/api").is_some());
    cache.finished(&again, GitState::NotRepo, t0 + GIT_TTL);
    assert_eq!(cache.summary("/w/api"), None);
    assert_eq!(cache.state("/w/api"), Some(&GitState::NotRepo));
}

#[test]
fn at_most_a_few_jobs_run_at_once_and_no_job_for_no_folder() {
    let mut cache = GitCache::default();
    let t0 = Instant::now();
    assert_eq!(cache.start("", t0), None);
    let jobs: Vec<GitJob> = (0..MAX_RUNNING)
        .map(|k| cache.start(&format!("/w/p{k}"), t0).unwrap())
        .collect();
    assert_eq!(cache.start("/w/one-more", t0), None);
    cache.finished(&jobs[0], clean("main"), t0);
    assert!(cache.start("/w/one-more", t0).is_some());
}

// ---- the app asks only while the line is shown ---------------------------------

/// The app with two background rows in different folders.
fn app() -> App {
    let mut app = common::app_single();
    let rows = r#"[
        {"id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000001",
         "cwd": "/home/user/workspace/api", "kind": "background", "name": "fix-login",
         "state": "done", "pid": 4242, "status": "idle"},
        {"id": "5e6f7a8b", "sessionId": "5e6f7a8b-0000-4000-8000-000000000002",
         "cwd": "/home/user/workspace/web", "kind": "background", "name": "pick-colour",
         "state": "done", "pid": 4243, "status": "idle"}
    ]"#;
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 1,
        result: Ok(parse_listing(rows).unwrap()),
    });
    app
}

fn draw(app: &mut App, w: u16, h: u16) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
}

fn git_jobs(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::GitStatus(job) => Some(job.cwd.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_selected_folder_is_asked_once_the_panel_shows_it() {
    let mut app = app();
    app.style = UiStyle::Dashboard;
    common::select_name(&mut app, "fix-login");
    // Nothing drawn yet: nothing asked.
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    draw(&mut app, 160, 40);
    assert_eq!(
        git_jobs(&app.tick(Instant::now())),
        ["/home/user/workspace/api"]
    );
    // Running: not asked again.
    draw(&mut app, 160, 40);
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    // A new selection is asked at once.
    common::select_name(&mut app, "pick-colour");
    assert_eq!(
        git_jobs(&app.tick(Instant::now())),
        ["/home/user/workspace/web"]
    );
    // Back to the first folder: its job still runs.
    common::select_name(&mut app, "fix-login");
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    app.git_finished(
        GitJob {
            cwd: "/home/user/workspace/api".into(),
        },
        clean("main"),
    );
    // Fresh now: not asked again.
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    assert!(app.git_summary("/home/user/workspace/api").is_some());
}

#[test]
fn nothing_is_asked_without_a_details_panel() {
    // The minimal style has no details panel.
    let mut app = app();
    app.style = UiStyle::Minimal;
    common::select_name(&mut app, "fix-login");
    draw(&mut app, 160, 40);
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    // The dashboard below 26 rows has no strip either.
    app.style = UiStyle::Dashboard;
    draw(&mut app, 100, 24);
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
    // The logs panel takes the details panel's place.
    draw(&mut app, 100, 30);
    app.handle_key(common::ch(' '));
    draw(&mut app, 100, 30);
    assert!(git_jobs(&app.tick(Instant::now())).is_empty());
}
