//! Stop and remove (doc/0009) against a fake `claude` (doc/0015).

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::Path;

use fake_claude::{Fake, cat, cat_err, lock};
use watchtower::actions::{
    RemoveError, StopError, is_valid_id, remove, remove_command, stop, stop_command,
};
use watchtower::cli::CliRunner;
use watchtower::config::Account;

/// A made-up `rm` refusal: the real format and exit code are not captured
/// yet (doc/0009 open items), so this only has the documented ingredients,
/// several lines and a suggested `--discard-unpushed` flag.
const REFUSAL: &str = "\
Refusing to remove b2f8d064: worktree has unpushed commits
  /home/user/workspace/demo-app/.claude/worktrees/say-ok: 2 commits not on any remote
To remove anyway and lose those commits, run:
  claude rm b2f8d064 --discard-unpushed 1a2b3c4@/home/user/workspace/demo-app/.claude/worktrees/say-ok";

fn runner() -> CliRunner {
    CliRunner::new()
}

#[test]
fn command_lines_use_the_short_id_and_no_force_flags() {
    let alt = Account::new(
        "alt",
        "claude-alt",
        Some("~/.claude-alt"),
        Path::new("/home/user"),
    )
    .unwrap();
    assert_eq!(
        stop_command(&alt, "b2f8d064").argv(),
        ["claude-alt", "stop", "b2f8d064"]
    );
    assert_eq!(
        remove_command(&alt, "b2f8d064").argv(),
        ["claude-alt", "rm", "b2f8d064"]
    );
    assert!(is_valid_id("b2f8d064"));
    for bad in ["", "--all", "two words", "a\nb"] {
        assert!(!is_valid_id(bad), "{bad:?}");
    }
}

#[test]
fn stop_success() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/stop.stdout"));
    stop(&runner(), &fake.account(), "b2f8d064").unwrap();
    assert_eq!(fake.argv(), ["stop", "b2f8d064"]);
    // Idempotent: the CLI prints the same line for an already stopped
    // session, so a second stop succeeds too.
    stop(&runner(), &fake.account(), "b2f8d064").unwrap();
    assert_eq!(fake.calls(), 2);
}

#[test]
fn stop_unknown_id() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!("{}\nexit 1", cat_err("cli-output/stop-unknown.stderr")),
    );
    match stop(&runner(), &fake.account(), "say-ok") {
        Err(StopError::UnknownId { message }) => {
            assert_eq!(
                message,
                "No job matching 'say-ok'. Run 'claude agents' to list running sessions."
            );
        }
        other => panic!("expected UnknownId, got {other:?}"),
    }
}

#[test]
fn stop_without_confirmation_is_a_failure() {
    let _g = lock();
    // Exit 0, but for another id: not a confirmed stop of ours.
    let fake = Fake::new("claude", "echo 'stopped 00000000'");
    assert!(matches!(
        stop(&runner(), &fake.account(), "b2f8d064"),
        Err(StopError::Failed { code: Some(0), .. })
    ));
    let fake = Fake::new("claude", "echo 'error: supervisor gone' >&2\nexit 3");
    match stop(&runner(), &fake.account(), "b2f8d064") {
        Err(StopError::Failed { code, message }) => {
            assert_eq!(code, Some(3));
            assert_eq!(message, "error: supervisor gone");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn invalid_id_runs_nothing() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/stop.stdout"));
    assert!(matches!(
        stop(&runner(), &fake.account(), "--all"),
        Err(StopError::InvalidId(_))
    ));
    assert!(matches!(
        remove(&runner(), &fake.account(), ""),
        Err(RemoveError::InvalidId(_))
    ));
    assert_eq!(fake.calls(), 0);
}

#[test]
fn remove_success() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/rm.stdout"));
    remove(&runner(), &fake.account(), "b2f8d064").unwrap();
    assert_eq!(fake.argv(), ["rm", "b2f8d064"]);
}

#[test]
fn remove_unknown_id() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        "echo \"No job matching 'nosuchjob'. Run 'claude agents' to list running sessions.\" >&2\nexit 1",
    );
    assert!(matches!(
        remove(&runner(), &fake.account(), "nosuchjob"),
        Err(RemoveError::UnknownId { .. })
    ));
}

#[test]
fn remove_refusal_is_returned_verbatim_without_retry() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!("cat >&2 <<'EOF'\n{REFUSAL}\nEOF\nexit 1"),
    );
    match remove(&runner(), &fake.account(), "b2f8d064") {
        Err(RemoveError::Refused { code, message }) => {
            assert_eq!(code, Some(1));
            assert_eq!(message, REFUSAL, "the full CLI text");
        }
        other => panic!("expected Refused, got {other:?}"),
    }
    assert_eq!(fake.calls(), 1, "never retried");
    let argv = fake.argv();
    assert_eq!(argv, ["rm", "b2f8d064"]);
    assert!(
        !argv
            .iter()
            .any(|a| a == "--discard-unpushed" || a == "--force-remove-worktree")
    );
}

#[test]
fn remove_without_confirmation_counts_as_refused() {
    let _g = lock();
    // The refusal's exit code is unverified: even with exit 0, no
    // `removed <id>` line means the session was not removed.
    let fake = Fake::new(
        "claude",
        "echo 'worktree in use: /home/user/workspace/demo-app/.claude/worktrees/say-ok'\n\
         echo 'pass --force-remove-worktree /home/user/workspace/demo-app/.claude/worktrees/say-ok to remove it'",
    );
    match remove(&runner(), &fake.account(), "b2f8d064") {
        Err(RemoveError::Refused { code, message }) => {
            assert_eq!(code, Some(0));
            assert!(message.contains("--force-remove-worktree"), "{message}");
            assert_eq!(message.lines().count(), 2);
        }
        other => panic!("expected Refused, got {other:?}"),
    }
    assert_eq!(fake.calls(), 1);
}
