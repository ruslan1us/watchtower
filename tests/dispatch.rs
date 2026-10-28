//! New-session dispatch (doc/0008): validation, exact argv, and runs against
//! a fake `claude` that prints captured fixture output (doc/0015).

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};

use fake_claude::{Fake, cat, cat_err, fixture_path, lock};
use watchtower::cli::{CliOutput, CliRunner, CommandLine};
use watchtower::config::Account;
use watchtower::dispatch::{
    self, DispatchError, DispatchRequest, Dispatched, EFFORT_CHOICES, MODEL_CHOICES,
    PERMISSION_MODES, ValidationError, haiku_warning, is_old_haiku, parse_backgrounded,
    permission_modes, record_dispatch, trust_command,
};
use watchtower::listing::Session;
use watchtower::sidecar::{AccountSource, Sidecar};

const HOME: &str = "/home/user";

fn main_account() -> Account {
    Account::new("main", "claude", None, Path::new(HOME)).unwrap()
}

/// A request for an existing temp folder.
fn request(project: &Path, task: &str) -> DispatchRequest {
    DispatchRequest::new(project, task, main_account())
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// ---- option lists and the Haiku warning ---------------------------------

#[test]
fn option_lists_match_doc_0008() {
    assert_eq!(
        MODEL_CHOICES,
        ["default", "fable", "opus", "sonnet", "haiku"]
    );
    assert_eq!(
        EFFORT_CHOICES,
        ["default", "low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(
        PERMISSION_MODES,
        [
            "auto",
            "acceptEdits",
            "manual",
            "plan",
            "dontAsk",
            "bypassPermissions"
        ]
    );
    assert_eq!(permission_modes(false), PERMISSION_MODES);
    assert_eq!(
        permission_modes(true),
        ["auto", "acceptEdits", "manual", "plan", "dontAsk"]
    );
    assert_eq!(dispatch::DEFAULT_PERMISSION_MODE, "auto");
}

#[test]
fn is_root_matches_the_process() {
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    assert_eq!(dispatch::is_root(), euid == 0);
}

#[test]
fn old_haiku_detection() {
    for old in [
        "haiku",
        "Haiku",
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "claude-3-5-haiku-20241022",
        "claude-3-haiku-20240307",
        "us.anthropic.claude-haiku-4-5-20251001-v1:0",
        "claude-haiku-4-5@20251001",
        "claude-haiku-4",
    ] {
        assert!(is_old_haiku(old), "{old}");
    }
    for new in [
        "claude-haiku-4-6",
        "claude-haiku-5",
        "claude-haiku-5-0-20270101",
        "sonnet",
        "opus",
        "claude-sonnet-4-5",
        "default",
    ] {
        assert!(!is_old_haiku(new), "{new}");
    }
}

#[test]
fn haiku_warning_only_with_auto() {
    assert!(haiku_warning(Some("haiku"), "auto").is_some());
    assert!(haiku_warning(Some("claude-haiku-4-5"), "auto").is_some());
    assert!(haiku_warning(Some("haiku"), "manual").is_none());
    assert!(haiku_warning(Some("sonnet"), "auto").is_none());
    assert!(haiku_warning(None, "auto").is_none());

    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "summarise the README");
    req.model = Some(" haiku ".into());
    assert!(req.haiku_warning().is_some());
    req.permission_mode = "plan".into();
    assert!(req.haiku_warning().is_none());
}

// ---- validation -----------------------------------------------------------

#[test]
fn validation_rules() {
    let dir = tempfile::tempdir().unwrap();
    let ok = request(dir.path(), "fix the flaky test");
    assert_eq!(ok.validate_for(false), Ok(()));

    let mut r = ok.clone();
    r.project = PathBuf::new();
    assert_eq!(r.validate_for(false), Err(ValidationError::NoProject));

    let mut r = ok.clone();
    r.project = dir.path().join("missing");
    assert!(matches!(
        r.validate_for(false),
        Err(ValidationError::ProjectMissing(_))
    ));

    for short in ["", "abc", "  ab \n ", "\t\n"] {
        let r = request(dir.path(), short);
        assert_eq!(
            r.validate_for(false),
            Err(ValidationError::TaskTooShort { min: 4 }),
            "{short:?}"
        );
    }
    // Exactly four characters after trimming, counted as characters.
    assert_eq!(request(dir.path(), "  abcd \n").validate_for(false), Ok(()));
    assert_eq!(request(dir.path(), "ünïç").validate_for(false), Ok(()));

    assert_eq!(
        request(dir.path(), "- item one\n- item two").validate_for(false),
        Err(ValidationError::LeadingDash { field: "task" })
    );

    let mut r = ok.clone();
    r.name = Some("two\nlines".into());
    assert_eq!(r.validate_for(false), Err(ValidationError::BadName));
    r.name = Some("--model".into());
    assert!(matches!(
        r.validate_for(false),
        Err(ValidationError::LeadingDash { .. })
    ));
    r.name = Some("   ".into());
    assert_eq!(r.validate_for(false), Ok(()), "blank name means none");

    let mut r = ok.clone();
    r.model = Some("claude opus".into());
    assert!(matches!(
        r.validate_for(false),
        Err(ValidationError::BadOption { field: "model", .. })
    ));
    r.model = Some("no-such-model-xyz".into());
    assert_eq!(r.validate_for(false), Ok(()), "free text is not checked");

    let mut r = ok.clone();
    r.permission_mode = " ".into();
    assert!(matches!(
        r.validate_for(false),
        Err(ValidationError::BadOption { .. })
    ));

    let mut r = ok.clone();
    r.permission_mode = "bypassPermissions".into();
    assert_eq!(r.validate_for(false), Ok(()));
    assert_eq!(r.validate_for(true), Err(ValidationError::BypassAsRoot));
}

// ---- argv -------------------------------------------------------------------

#[test]
fn minimal_argv_always_has_permission_mode() {
    let req = request(
        Path::new("/home/user/workspace/demo-app"),
        "  fix the bug\n",
    );
    assert_eq!(
        req.argv(),
        ["claude", "--bg", "--permission-mode", "auto", "fix the bug"]
    );
    let line = req.command_line();
    assert_eq!(
        line.cwd.as_deref(),
        Some(Path::new("/home/user/workspace/demo-app"))
    );
    assert_eq!(line.program, "claude");
    assert_eq!(
        line.to_command().get_current_dir(),
        Some(Path::new("/home/user/workspace/demo-app"))
    );
}

#[test]
fn full_argv_in_documented_order() {
    let alt = Account::new("alt", "claude-alt", Some("~/.claude-alt"), Path::new(HOME)).unwrap();
    let mut req = DispatchRequest::new(
        "/home/user/workspace/demo-app",
        "Refactor the config loader.\n\nKeep the public API.",
        alt,
    );
    req.name = Some(" refactor-config ".into());
    req.model = Some("opus".into());
    req.effort = Some("xhigh".into());
    req.permission_mode = "plan".into();
    assert_eq!(
        req.argv(),
        strings(&[
            "claude-alt",
            "--bg",
            "--name",
            "refactor-config",
            "--model",
            "opus",
            "--effort",
            "xhigh",
            "--permission-mode",
            "plan",
            "Refactor the config loader.\n\nKeep the public API.",
        ])
    );
}

#[test]
fn default_choices_are_omitted() {
    let mut req = request(Path::new("/srv/x"), "update dependencies");
    req.name = Some(String::new());
    req.model = Some("default".into());
    req.effort = Some(" default ".into());
    req.permission_mode = "acceptEdits".into();
    assert_eq!(
        req.args(),
        [
            "--bg",
            "--permission-mode",
            "acceptEdits",
            "update dependencies"
        ]
    );
}

#[test]
fn account_command_words_come_first() {
    let wrapped = Account::new(
        "alt",
        "env CLAUDE_CONFIG_DIR=/home/user/.claude-alt claude",
        None,
        Path::new(HOME),
    )
    .unwrap();
    let req = DispatchRequest::new("/srv/x", "say OK please", wrapped);
    assert_eq!(
        req.argv(),
        [
            "env",
            "CLAUDE_CONFIG_DIR=/home/user/.claude-alt",
            "claude",
            "--bg",
            "--permission-mode",
            "auto",
            "say OK please"
        ]
    );
}

// ---- parsing ------------------------------------------------------------------

#[test]
fn parse_backgrounded_lines() {
    let ok = |id: &str, name: Option<&str>| {
        Some(Dispatched {
            id: id.into(),
            name: name.map(Into::into),
        })
    };
    let stdout = std::fs::read_to_string(fixture_path("cli-output/bg-success.stdout")).unwrap();
    assert_eq!(
        parse_backgrounded(&stdout),
        ok("3e9a1c07", Some("bad-model"))
    );
    let resumed =
        std::fs::read_to_string(fixture_path("cli-output/resume-noprompt.stdout")).unwrap();
    assert_eq!(
        parse_backgrounded(&resumed),
        ok("d4a1f7c8", Some("touch-config"))
    );
    assert_eq!(
        parse_backgrounded("backgrounded · 1a2b3c4d\n"),
        ok("1a2b3c4d", None)
    );
    assert_eq!(
        parse_backgrounded("  claude agents   list sessions\nbackgrounded · 1a2b3c4d · a (b)\n"),
        ok("1a2b3c4d", Some("a (b)"))
    );
    assert_eq!(parse_backgrounded(""), None);
    assert_eq!(parse_backgrounded("backgrounded · · x"), None);
    assert_eq!(parse_backgrounded("background job started"), None);
}

#[test]
fn interpret_untrusted_needs_exit_1() {
    let stderr = std::fs::read_to_string(fixture_path("cli-output/bg-untrusted.stderr")).unwrap();
    let out = |code| CliOutput {
        code: Some(code),
        stdout: Vec::new(),
        stderr: format!("Starting background service…\n{stderr}"),
    };
    let dir = Path::new("/home/user/workspace/untrusted-repo");
    assert!(matches!(
        dispatch::interpret(&out(1), dir),
        Err(DispatchError::Untrusted { .. })
    ));
    assert!(matches!(
        dispatch::interpret(&out(2), dir),
        Err(DispatchError::Failed { code: Some(2), .. })
    ));
}

// ---- runs against the fake claude ---------------------------------------------

fn runner() -> CliRunner {
    CliRunner::new()
}

#[test]
fn dispatch_success_runs_in_project_dir() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!(
            "{}\n{}",
            cat_err("cli-output/bg-success.stderr"),
            cat("cli-output/bg-success.stdout")
        ),
    );
    let project = tempfile::tempdir().unwrap();
    let mut req = DispatchRequest::new(project.path(), "say OK", fake.account());
    req.name = Some("bad-model".into());
    req.model = Some("no-such-model-xyz".into());

    let got = dispatch::run(&runner(), &req).unwrap();
    assert_eq!(
        got,
        Dispatched {
            id: "3e9a1c07".into(),
            name: Some("bad-model".into())
        }
    );
    assert_eq!(
        fake.argv(),
        [
            "--bg",
            "--name",
            "bad-model",
            "--model",
            "no-such-model-xyz",
            "--permission-mode",
            "auto",
            "say OK"
        ]
    );
    assert_eq!(fake.cwd(), project.path().canonicalize().unwrap());
    assert_eq!(fake.calls(), 1);
}

#[test]
fn task_is_one_argument_never_through_a_shell() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/bg-success.stdout"));
    let project = tempfile::tempdir().unwrap();
    let task = "Check $(touch pwned) and `id`; it's \"quoted\" * \\n\n\nsecond paragraph";
    let req = DispatchRequest::new(project.path(), task, fake.account());
    dispatch::run(&runner(), &req).unwrap();
    assert_eq!(fake.argv().last().map(String::as_str), Some(task));
    assert!(!project.path().join("pwned").exists());
}

#[test]
fn idle_suffix_is_accepted() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!(
            "{}\n{}",
            cat_err("cli-output/resume-noprompt.stderr"),
            cat("cli-output/resume-noprompt.stdout")
        ),
    );
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "touch the config", fake.account());
    let got = dispatch::run(&runner(), &req).unwrap();
    assert_eq!(got.id, "d4a1f7c8");
    assert_eq!(got.name.as_deref(), Some("touch-config"));
}

#[test]
fn untrusted_folder() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        &format!("{}\nexit 1", cat_err("cli-output/bg-untrusted.stderr")),
    );
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "fix the bug", fake.account());
    match dispatch::run(&runner(), &req) {
        Err(DispatchError::Untrusted { dir, message }) => {
            assert_eq!(dir, project.path());
            assert!(message.starts_with("Workspace not trusted."), "{message}");
        }
        other => panic!("expected Untrusted, got {other:?}"),
    }
    assert_eq!(fake.calls(), 1, "no automatic retry");
}

#[test]
fn other_failure_keeps_code_and_stderr() {
    let _g = lock();
    let fake = Fake::new(
        "claude",
        "echo 'Starting background service…' >&2\necho 'error: something broke' >&2\nexit 2",
    );
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "fix the bug", fake.account());
    match dispatch::run(&runner(), &req) {
        Err(DispatchError::Failed { code, stderr }) => {
            assert_eq!(code, Some(2));
            assert_eq!(
                stderr,
                "Starting background service…\nerror: something broke"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn unparsable_stdout() {
    let _g = lock();
    let fake = Fake::new("claude", "echo 'something new'");
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "fix the bug", fake.account());
    match dispatch::run(&runner(), &req) {
        Err(DispatchError::Unparsed { stdout, .. }) => assert_eq!(stdout, "something new"),
        other => panic!("expected Unparsed, got {other:?}"),
    }
}

#[test]
fn invalid_request_runs_nothing() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/bg-success.stdout"));
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "ok", fake.account());
    assert!(matches!(
        dispatch::run(&runner(), &req),
        Err(DispatchError::Invalid(ValidationError::TaskTooShort { .. }))
    ));
    assert_eq!(fake.calls(), 0);
}

#[test]
fn account_found_through_path_override() {
    let _g = lock();
    let fake = Fake::new("claude", &cat("cli-output/bg-success.stdout"));
    let project = tempfile::tempdir().unwrap();
    let req = DispatchRequest::new(project.path(), "fix the bug", main_account());
    let path = format!("{}:/usr/bin:/bin", fake.dir.path().display());
    let got = dispatch::run(&CliRunner::new().with_path(path), &req).unwrap();
    assert_eq!(got.id, "3e9a1c07");
    assert_eq!(fake.cwd(), project.path().canonicalize().unwrap());
}

// ---- trust flow and sidecar -------------------------------------------------------

#[test]
fn trust_command_is_the_bare_account_command_in_dir() {
    let alt = Account::new(
        "alt",
        "claude-alt --verbose",
        Some("~/.claude-alt"),
        Path::new(HOME),
    )
    .unwrap();
    let dir = Path::new("/home/user/workspace/untrusted-repo");
    assert_eq!(
        trust_command(&alt, dir),
        CommandLine {
            program: "claude-alt".into(),
            args: vec!["--verbose".into()],
            cwd: Some(dir.to_path_buf()),
        }
    );
    assert_eq!(
        trust_command(&main_account(), dir).argv(),
        ["claude"],
        "no extra arguments"
    );
}

#[test]
fn record_dispatch_writes_started_owner_and_recent_project() {
    let mut sidecar = Sidecar::default();
    let alt = Account::new("alt", "claude-alt", None, Path::new(HOME)).unwrap();
    let req = DispatchRequest::new("/home/user/workspace/demo-app", "fix the bug", alt);
    let got = Dispatched {
        id: "3e9a1c07".into(),
        name: Some("bad-model".into()),
    };
    assert!(record_dispatch(
        &mut sidecar,
        &req,
        &got,
        Path::new(HOME),
        1_790_000_000_000
    ));
    assert_eq!(
        sidecar.owner("3e9a1c07"),
        Some(("alt", &AccountSource::Started))
    );
    assert_eq!(sidecar.recent_projects[0].path, "~/workspace/demo-app");
    assert_eq!(sidecar.recent_projects[0].last_used_ms, 1_790_000_000_000);
    // Same dispatch recorded again at the same time: nothing to save.
    assert!(!record_dispatch(
        &mut sidecar,
        &req,
        &got,
        Path::new(HOME),
        1_790_000_000_000
    ));

    // The first listing that shows the sessionId moves the record there.
    let row = Session {
        id: Some("3e9a1c07".into()),
        session_id: Some("3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60".into()),
        ..Session::default()
    };
    assert!(sidecar.adopt_short_id_records([&row]));
    assert_eq!(sidecar.owner("3e9a1c07"), None);
    assert_eq!(
        sidecar.owner("3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60"),
        Some(("alt", &AccountSource::Started))
    );
    assert!(!sidecar.adopt_short_id_records([&row]));
}

#[test]
fn a_timeout_says_the_session_may_have_started() {
    // The timed-out `claude --bg` may have registered the session before it
    // was killed; a blind retry would start a duplicate.
    let err = DispatchError::Cli(watchtower::cli::CliError::Timeout {
        program: "claude".into(),
        timeout: std::time::Duration::from_secs(30),
        stderr: String::new(),
    });
    let text = err.to_string();
    assert!(text.contains("timed out"), "{text}");
    assert!(text.contains("may have started — check the list"), "{text}");

    let spawn = DispatchError::Cli(watchtower::cli::CliError::Spawn {
        program: "claude".into(),
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    });
    assert!(!spawn.to_string().contains("may have started"));
}

#[test]
fn end_of_options_lets_a_dash_task_through_after_dashes() {
    // doc/0044: `tower dispatch` passes a task starting with `-` after
    // `--`; the dialog (doc/0008) keeps refusing it.
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "-v: list the tests");
    assert_eq!(
        req.validate_for(false),
        Err(ValidationError::LeadingDash { field: "task" })
    );
    req.end_options = true;
    assert_eq!(req.validate_for(false), Ok(()));
    assert_eq!(
        req.args(),
        strings(&[
            "--bg",
            "--permission-mode",
            "auto",
            "--",
            "-v: list the tests"
        ])
    );
    // Any other task is passed as the dialog passes it, without `--`.
    let mut plain = request(dir.path(), "list the tests");
    plain.end_options = true;
    assert_eq!(
        plain.args(),
        strings(&["--bg", "--permission-mode", "auto", "list the tests"])
    );
    // Names and options still must not start with `-`.
    let mut named = request(dir.path(), "-v: list the tests");
    named.end_options = true;
    named.name = Some("-n".into());
    assert!(named.validate_for(false).is_err());
}
