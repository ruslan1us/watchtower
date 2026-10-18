//! The attach procedure against a fake `claude` and a fake terminal
//! (doc/0005, doc/0015): the owning account's command runs `attach <id>`,
//! the terminal is released before and reclaimed after, and tower survives
//! SIGINT/SIGQUIT/SIGTSTP while the child runs.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Mutex, MutexGuard};

use watchtower::app::AttachRequest;
use watchtower::attach::{
    Handover, InheritSpawner, SignalGuard, Spawner, Termination, announcement, attach,
};
use watchtower::config::Account;

/// Serializes tests: signal dispositions are process-wide, and writing an
/// executable while another thread forks can fail with ETXTBSY.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Records the order of terminal operations.
#[derive(Default)]
struct FakeTerm {
    log: Vec<String>,
}

impl Handover for FakeTerm {
    fn release(&mut self) -> io::Result<()> {
        self.log.push("release".into());
        Ok(())
    }
    fn announce(&mut self, line: &str) -> io::Result<()> {
        self.log.push(format!("announce {line}"));
        Ok(())
    }
    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()> {
        self.log.push(format!("wait {prompt}"));
        Ok(())
    }
    fn reclaim(&mut self) -> io::Result<()> {
        self.log.push("reclaim".into());
        Ok(())
    }
}

/// A fake account command: a script recording its argv, one per line.
struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(name: &str, body: &str) -> Fake {
        let fake = Fake {
            dir: tempfile::tempdir().unwrap(),
        };
        fake.add(name, body);
        fake
    }

    fn add(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let script = self.dir.path().join(name);
        let argv = self.dir.path().join(format!("{name}.argv"));
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{body}\n",
            argv.display()
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn account(&self, account: &str, script: &str) -> Account {
        let cmd = self.path(script).to_string_lossy().into_owned();
        Account::new(account, cmd, None, Path::new("/home/user")).unwrap()
    }

    fn argv(&self, script: &str) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(self.path(&format!("{script}.argv"))).ok()?;
        Some(text.lines().map(str::to_owned).collect())
    }
}

fn request(account: Account, id: &str, name: &str) -> AttachRequest {
    AttachRequest {
        account,
        id: id.into(),
        name: name.into(),
    }
}

#[test]
fn runs_owning_account_attach_with_short_id() {
    let _l = lock();
    let fake = Fake::new("claude", "exit 0");
    fake.add("claude-alt", "exit 0");
    let req = request(
        fake.account("alt", "claude-alt"),
        "d4a1f7c8",
        "touch-config",
    );
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut InheritSpawner,
        &req,
        &Termination::default(),
    );

    assert_eq!(outcome.result, Ok(Some(0)));
    assert_eq!(outcome.account, "alt");
    assert_eq!(outcome.name, "touch-config");
    assert_eq!(
        fake.argv("claude-alt"),
        Some(vec!["attach".into(), "d4a1f7c8".into()])
    );
    assert_eq!(fake.argv("claude"), None, "other account's command not run");
    assert_eq!(
        term.log,
        [
            "release".to_owned(),
            "announce tower: attaching to touch-config (alt)… Ctrl+Z returns to tower".into(),
            "reclaim".into(),
        ]
    );
}

#[test]
fn account_command_words_are_kept() {
    let _l = lock();
    let fake = Fake::new("claude", "exit 0");
    let cmd = format!("{} --flag-of-wrapper", fake.path("claude").display());
    let account = Account::new("main", cmd, None, Path::new("/home/user")).unwrap();
    let mut term = FakeTerm::default();
    attach(
        &mut term,
        &mut InheritSpawner,
        &request(account, "1a2b3c4d", "x"),
        &Termination::default(),
    );
    assert_eq!(
        fake.argv("claude"),
        Some(vec![
            "--flag-of-wrapper".into(),
            "attach".into(),
            "1a2b3c4d".into()
        ])
    );
}

#[test]
fn nonzero_exit_waits_for_key_and_reports_code() {
    let _l = lock();
    let fake = Fake::new(
        "claude",
        "echo \"No job matching '$2'. Run 'claude agents' to list running sessions.\" >&2; exit 1",
    );
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut InheritSpawner,
        &request(fake.account("main", "claude"), "0000aaaa", "gone"),
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(1)));
    assert_eq!(term.log.len(), 4, "{:?}", term.log);
    assert!(term.log[2].starts_with("wait tower: attach exited with code 1"));
    assert_eq!(term.log[3], "reclaim");
}

#[test]
fn missing_command_is_an_error_and_terminal_is_reclaimed() {
    let _l = lock();
    let account = Account::new(
        "main",
        "/nonexistent/tower-test/claude",
        None,
        Path::new("/home/user"),
    )
    .unwrap();
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut InheritSpawner,
        &request(account, "1a2b3c4d", "x"),
        &Termination::default(),
    );
    let err = outcome.result.unwrap_err();
    assert!(err.contains("cannot run"), "{err}");
    assert_eq!(term.log.last().map(String::as_str), Some("reclaim"));
}

/// A spawner that raises the guarded signals at tower itself while the
/// "child" runs, as a stray Ctrl+C or Ctrl+Z on the shared terminal would.
struct SignalRaisingSpawner;

impl Spawner for SignalRaisingSpawner {
    fn run(&mut self, command: Command) -> io::Result<ExitStatus> {
        for sig in watchtower::attach::GUARDED_SIGNALS {
            signal_hook::low_level::raise(sig)?;
        }
        InheritSpawner.run(command)
    }
}

#[test]
fn tower_survives_signals_while_attached() {
    let _l = lock();
    let fake = Fake::new("claude", "exit 0");
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut SignalRaisingSpawner,
        &request(fake.account("main", "claude"), "1a2b3c4d", "x"),
        &Termination::default(),
    );
    // Still alive and not stopped: SIGINT would have killed the test process
    // and SIGTSTP would have stopped it.
    assert_eq!(outcome.result, Ok(Some(0)));
}

#[test]
fn child_gets_default_signal_dispositions() {
    let _l = lock();
    // While the guard is active the child must not inherit an ignored
    // SIGINT: `SigIgn` in /proc/self/status has bit 1 (SIGINT) clear.
    let _guard = SignalGuard::install().unwrap();
    let out = Command::new("sh")
        .args(["-c", "grep '^SigIgn:' /proc/self/status"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let hex = text.split_whitespace().nth(1).expect("SigIgn line");
    let mask = u64::from_str_radix(hex, 16).unwrap();
    for sig in watchtower::attach::GUARDED_SIGNALS {
        assert_eq!(mask & (1 << (sig - 1)), 0, "signal {sig} ignored in child");
    }
}

#[test]
fn announcement_names_session_and_account() {
    let account = Account::new("alt", "claude-alt", None, Path::new("/home/user")).unwrap();
    assert_eq!(
        announcement(&request(account, "1a2b3c4d", "fix-flaky-test")),
        "tower: attaching to fix-flaky-test (alt)… Ctrl+Z returns to tower"
    );
}

#[test]
fn stopped_child_is_resumed_not_waited_on_forever() {
    let _l = lock();
    // The child stops itself as Ctrl+Z in cooked mode would; the spawner
    // must resume it, and its exit code must come through.
    let fake = Fake::new("claude", "kill -TSTP $$; exit 7");
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut InheritSpawner,
        &request(fake.account("main", "claude"), "1a2b3c4d", "x"),
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(7)));
}

/// Runs the child after a SIGTERM was "received" while it runs, as the
/// handler would record it.
struct TerminatingSpawner(Termination);

impl Spawner for TerminatingSpawner {
    fn run(&mut self, command: Command) -> io::Result<ExitStatus> {
        self.0.request(signal_hook::consts::SIGTERM);
        InheritSpawner.run(command)
    }
}

#[test]
fn termination_while_attached_waits_for_child_then_skips_pause_and_reclaim() {
    let _l = lock();
    // The child outlives the signal and exits on its own; its exit code
    // comes through, and the terminal is left released for tower's exit.
    let fake = Fake::new("claude", "sleep 0.2; exit 1");
    let termination = Termination::default();
    let mut term = FakeTerm::default();
    let outcome = attach(
        &mut term,
        &mut TerminatingSpawner(termination.clone()),
        &request(fake.account("main", "claude"), "1a2b3c4d", "x"),
        &termination,
    );
    assert_eq!(outcome.result, Ok(Some(1)));
    assert_eq!(termination.pending(), Some(signal_hook::consts::SIGTERM));
    assert_eq!(term.log.len(), 2, "{:?}", term.log);
    assert_eq!(term.log[0], "release");
    assert!(term.log[1].starts_with("announce "));
}
