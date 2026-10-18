//! Environment hygiene (doc/0017): tower started with `CLAUDE_CONFIG_DIR`
//! set never passes it on. Background calls (listing, stop, dispatch, the
//! hand-off resume) and inherited-stdio commands (attach, the trust flow)
//! run without it; a wrapper command that sets it itself still does.
//!
//! This binary holds a single test because it changes the process
//! environment, which is not safe while other test threads run.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::io;
use std::path::Path;
use std::time::Duration;

use fake_claude::Fake;
use watchtower::actions;
use watchtower::attach::{self, Foreground, Handover, InheritSpawner, Termination};
use watchtower::cli::{CONFIG_DIR_VAR, CliRunner, attach_command};
use watchtower::config::Account;
use watchtower::dispatch::{self, DispatchRequest};
use watchtower::handoff::{self, HandoffRequest, HandoffTiming};

/// A handover that does nothing: the test has no terminal to hand over.
struct NoTerminal;

impl Handover for NoTerminal {
    fn release(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn announce(&mut self, _line: &str) -> io::Result<()> {
        Ok(())
    }
    fn wait_for_key(&mut self, _prompt: &str) -> io::Result<()> {
        Ok(())
    }
    fn reclaim(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn foreground(line: &watchtower::cli::CommandLine) {
    let fg = Foreground {
        line,
        announce: None,
        label: "claude",
        pause_on_failure: false,
    };
    let result = attach::run_foreground(
        &mut NoTerminal,
        &mut InheritSpawner,
        &fg,
        &Termination::default(),
    );
    assert_eq!(result, Ok(Some(0)));
}

#[test]
fn spawned_commands_never_inherit_claude_config_dir() {
    // SAFETY: the only test in this binary, so no other thread reads or
    // writes the environment meanwhile.
    unsafe { std::env::set_var(CONFIG_DIR_VAR, "/home/user/.claude-alt") };

    let project = tempfile::tempdir().unwrap();
    // Answers every call the way the real CLI would, enough for each path.
    let body = r#"case "$1" in
  agents) echo '[]' ;;
  stop) echo "stopped $2" ;;
  --resume) echo "backgrounded · d4a1f7c8 · touch-config" ;;
  --bg) echo "backgrounded · 0a1b2c3d · new-task" ;;
esac
exit 0"#;
    let fake = Fake::new("claude", body);
    let main = fake.account_named("main", None);
    let runner = CliRunner::new();

    // 1: listing.
    runner.list_sessions(&main).unwrap();
    // 2: stop.
    actions::stop(&runner, &main, "d4a1f7c8").unwrap();
    // 3: dispatch.
    let mut req = DispatchRequest::new(project.path(), "say ok", main.clone());
    req.permission_mode = "default".into();
    dispatch::run(&runner, &req).unwrap();
    // 4: the hand-off resume (no live process, so no stop).
    let handoff = HandoffRequest {
        id: "d4a1f7c8".into(),
        session_id: "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42".into(),
        name: "touch-config".into(),
        cwd: project.path().to_path_buf(),
        owner: None,
        target: main.clone(),
        live: false,
        prompt: "continue".into(),
    };
    let timing = HandoffTiming {
        stop_timeout: Duration::from_secs(1),
        poll_interval: Duration::from_millis(20),
    };
    handoff::run(&runner, &handoff, &timing, &mut |_| {}).unwrap();
    // 5: attach, with inherited stdio.
    foreground(&attach_command(&main, "d4a1f7c8"));
    // 6: the trust flow, with inherited stdio.
    foreground(&dispatch::trust_command(&main, project.path()));

    assert_eq!(fake.calls(), 6);
    for n in 1..=6 {
        assert_eq!(
            fake.env_var_of(n, CONFIG_DIR_VAR),
            None,
            "call {n}: {:?}",
            fake.argv_of(n)
        );
        assert!(fake.env_var_of(n, "PATH").is_some(), "the rest is kept");
    }

    // A wrapper that sets the variable itself still does.
    let cmd = format!(
        "env CLAUDE_CONFIG_DIR=/home/user/.claude-work '{}'",
        fake.program().display()
    );
    let work = Account::new("work", cmd, None, Path::new("/home/user")).unwrap();
    runner.list_sessions(&work).unwrap();
    foreground(&attach_command(&work, "d4a1f7c8"));
    for n in 7..=8 {
        assert_eq!(
            fake.env_var_of(n, CONFIG_DIR_VAR).as_deref(),
            Some("/home/user/.claude-work"),
            "call {n}"
        );
    }

    // SAFETY: as above.
    unsafe { std::env::remove_var(CONFIG_DIR_VAR) };
}
