//! CLI runner tests against a fake `claude` executable (doc/0015).

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use watchtower::cli::{CliError, CliRunner, CommandLine, ListError, OUTPUT_CAP, attach_command};
use watchtower::config::Account;
use watchtower::listing::State;

/// Serializes the tests in this file: writing an executable while another
/// thread forks can make the exec fail with ETXTBSY.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A temp dir with a fake executable that records its argv to `argv.txt`
/// (one argument per line) and then runs `body`.
struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(name: &str, body: &str) -> Fake {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(name);
        let argv = dir.path().join("argv.txt");
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{body}\n",
            argv.display()
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fake { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn account(&self, name: &str) -> Account {
        let cmd = self.path(name).to_string_lossy().into_owned();
        Account::new("main", cmd, None, Path::new("/home/user")).unwrap()
    }

    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("argv.txt"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn cat_fixture(rel: &str) -> String {
    format!("cat '{}'", common::fixture_path(rel).display())
}

#[test]
fn list_sessions_runs_agents_json_all_and_parses() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!(
            "echo 'Starting background service…' >&2\n{}",
            cat_fixture("agents-json/after-stop-all.json")
        ),
    );
    let listing = CliRunner::new()
        .list_sessions(&fake.account("claude"))
        .unwrap();
    assert_eq!(fake.argv(), ["agents", "--json", "--all"]);
    assert_eq!(listing.sessions.len(), 7);
    assert!(listing.warnings.is_empty());
    assert_eq!(
        common::by_name(&listing.sessions, "say-ok").state,
        Some(State::Done)
    );
}

#[test]
fn account_command_words_come_first() {
    let _g = lock();
    let fake = Fake::new("claude-alt", "echo '[]'");
    let cmd = format!("'{}' --flag 'two words'", fake.path("claude-alt").display());
    let account = Account::new("alt", cmd, Some("~/.claude-alt"), Path::new("/home/user")).unwrap();
    let listing = CliRunner::new().list_sessions(&account).unwrap();
    assert!(listing.sessions.is_empty());
    assert_eq!(
        fake.argv(),
        ["--flag", "two words", "agents", "--json", "--all"]
    );
}

#[test]
fn fake_claude_found_through_path_override() {
    let _g = lock();
    let fake = Fake::new("claude", &cat_fixture("agents-json/mixed-states.json"));
    let account = Account::new("main", "claude", None, Path::new("/home/user")).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake.dir.path().display());
    let listing = CliRunner::new()
        .with_path(path)
        .list_sessions(&account)
        .unwrap();
    assert_eq!(listing.sessions.len(), 6);
    assert_eq!(fake.argv(), ["agents", "--json", "--all"]);
}

#[test]
fn non_zero_exit_is_an_error_with_stderr() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        "echo '[]'\necho \"No job matching 'x'. Run 'claude agents' to list running sessions.\" >&2\nexit 3",
    );
    match CliRunner::new().list_sessions(&fake.account("claude")) {
        Err(ListError::Exit { code, stderr }) => {
            assert_eq!(code, Some(3));
            assert!(stderr.starts_with("No job matching 'x'."), "{stderr}");
        }
        other => panic!("expected Exit, got {other:?}"),
    }
}

#[test]
fn run_captures_code_stdout_stderr() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!(
            "{}\n{} >&2\nexit 1",
            cat_fixture("cli-output/rm.stdout"),
            cat_fixture("cli-output/logs-stopped.stderr")
        ),
    );
    let out = CliRunner::new()
        .run(&fake.account("claude"), ["logs", "b2f8d064"])
        .unwrap();
    assert_eq!(fake.argv(), ["logs", "b2f8d064"]);
    assert_eq!(out.code, Some(1));
    assert!(!out.success());
    assert_eq!(
        out.stdout,
        std::fs::read(common::fixture_path("cli-output/rm.stdout")).unwrap()
    );
    assert_eq!(
        out.stderr,
        common::fixture("cli-output/logs-stopped.stderr")
    );
}

#[test]
fn stdin_is_null() {
    let _g = lock();
    let fake = Fake::new("claude", "if read line; then echo got; else echo eof; fi");
    let out = CliRunner::new()
        .run(&fake.account("claude"), ["x"])
        .unwrap();
    assert_eq!(out.stdout_text(), "eof\n");
    assert!(out.success());
}

#[test]
fn environment_is_passed_through_unchanged() {
    let _g = lock();
    let fake = Fake::new("claude", "printf '%s' \"${CLAUDE_CONFIG_DIR-unset}\"");
    let out = CliRunner::new()
        .run(&fake.account("claude"), ["x"])
        .unwrap();
    let expected = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "unset".into());
    assert_eq!(
        out.stdout_text(),
        expected,
        "tower sets no CLAUDE_CONFIG_DIR itself"
    );
}

#[test]
fn bad_json_and_signal() {
    let _g = lock();
    let fake = Fake::new("claude", "echo 'not json'");
    assert!(matches!(
        CliRunner::new().list_sessions(&fake.account("claude")),
        Err(ListError::Parse(_))
    ));
    let fake = Fake::new("claude", "kill -9 $$");
    match CliRunner::new().list_sessions(&fake.account("claude")) {
        Err(ListError::Exit { code: None, .. }) => {}
        other => panic!("expected signal exit, got {other:?}"),
    }
}

#[test]
fn timeout_kills_the_call() {
    let _g = lock();
    let fake = Fake::new("claude", "echo partial >&2\nexec sleep 10");
    let start = Instant::now();
    let err = CliRunner::new()
        .with_timeout(Duration::from_millis(300))
        .list_sessions(&fake.account("claude"))
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    match err {
        ListError::Cli(CliError::Timeout {
            stderr, timeout, ..
        }) => {
            assert_eq!(timeout, Duration::from_millis(300));
            assert_eq!(stderr, "partial\n");
        }
        other => panic!("expected timeout, got {other:?}"),
    }
}

#[test]
fn grandchild_holding_pipes_does_not_block() {
    let _g = lock();
    // Like a supervisor started in the background that inherits stdout.
    let fake = Fake::new("claude", "sleep 10 &\necho '[]'");
    let start = Instant::now();
    let listing = CliRunner::new()
        .list_sessions(&fake.account("claude"))
        .unwrap();
    assert!(listing.sessions.is_empty());
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn missing_program_is_a_spawn_error() {
    let _g = lock();
    let account =
        Account::new("main", "/nonexistent/claude", None, Path::new("/home/user")).unwrap();
    assert!(matches!(
        CliRunner::new().list_sessions(&account),
        Err(ListError::Cli(CliError::Spawn { .. }))
    ));
}

#[test]
fn attach_command_line() {
    let home = Path::new("/home/user");
    let alt = Account::new("alt", "claude-alt", Some("~/.claude-alt"), home).unwrap();
    let line = attach_command(&alt, "1a2b3c4d");
    assert_eq!(
        line,
        CommandLine {
            program: "claude-alt".into(),
            args: vec!["attach".into(), "1a2b3c4d".into()],
            cwd: None,
        }
    );
    assert_eq!(line.argv(), ["claude-alt", "attach", "1a2b3c4d"]);
    let cmd = line.to_command();
    assert_eq!(cmd.get_program(), "claude-alt");
    assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["attach", "1a2b3c4d"]);
    assert_eq!(cmd.get_envs().count(), 0, "no environment changes");

    let wrapped = Account::new("alt", "env FOO=1 claude", None, home).unwrap();
    assert_eq!(
        attach_command(&wrapped, "1a2b3c4d").argv(),
        ["env", "FOO=1", "claude", "attach", "1a2b3c4d"]
    );
}

#[test]
fn run_in_sets_the_working_directory() {
    let _g = lock();
    let fake = Fake::new("claude", "pwd -P");
    let dir = tempfile::tempdir().unwrap();
    let out = CliRunner::new()
        .run_in(&fake.account("claude"), dir.path(), ["--bg", "x"])
        .unwrap();
    assert_eq!(fake.argv(), ["--bg", "x"]);
    assert_eq!(
        out.stdout_text().trim_end(),
        dir.path().canonicalize().unwrap().to_string_lossy()
    );

    let line = CommandLine::for_account(&fake.account("claude"), ["x"]).with_cwd(dir.path());
    assert_eq!(line.cwd.as_deref(), Some(dir.path()));
    assert_eq!(line.to_command().get_current_dir(), Some(dir.path()));
    assert_eq!(
        CommandLine::for_account(&fake.account("claude"), ["x"])
            .to_command()
            .get_current_dir(),
        None,
        "no cwd unless asked"
    );
}

#[test]
fn background_call_runs_in_its_own_process_group() {
    let _g = lock();
    // Field 5 of /proc/<pid>/stat is the process group: the call leads a
    // group of its own, so Ctrl+C or Ctrl+Z typed into an attached session
    // (tower's foreground group) never reaches it.
    let fake = Fake::new("claude", "echo $$; cut -d' ' -f5 /proc/$$/stat");
    let out = CliRunner::new()
        .run(&fake.account("claude"), ["x"])
        .unwrap();
    let text = out.stdout_text();
    let ids: Vec<i64> = text.lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{text}");
    assert_eq!(ids[0], ids[1], "the call leads its own group");
    assert_ne!(ids[1], i64::from(unsafe { libc::getpgrp() }));
}

#[test]
fn timeout_kills_the_whole_process_group() {
    let _g = lock();
    // A wrapper that does not exec: its child must die with it, or a timed
    // out dispatch could still start a (duplicate) session.
    let fake = Fake::new(
        "claude",
        "sleep 30 &\necho $! > \"$(dirname \"$0\")/grandchild\"\nsleep 30",
    );
    let err = CliRunner::new()
        .with_timeout(Duration::from_millis(500))
        .run(&fake.account("claude"), ["--bg", "x"])
        .unwrap_err();
    assert!(matches!(err, CliError::Timeout { .. }), "{err:?}");
    let pid: libc::pid_t = std::fs::read_to_string(fake.path("grandchild"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Killed, then reaped by whoever adopted it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let alive = |pid: libc::pid_t| {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    };
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(pid), "grandchild {pid} survived the timeout");
}

#[test]
fn output_is_capped_keeping_the_end() {
    let _g = lock();
    let fake = Fake::new("claude", "head -c 6000000 /dev/zero; printf END");
    let out = CliRunner::new()
        .run(&fake.account("claude"), ["logs", "x"])
        .unwrap();
    assert_eq!(out.stdout.len(), OUTPUT_CAP);
    assert!(out.stdout.ends_with(b"END"));
}
