//! Hand-off to another account (doc/0004): the exact argv and working
//! directory of stop, the stop wait and `--resume … --bg`, run against two
//! fake accounts `main` (`claude`) and `alt` (`claude-alt`) that print
//! captured fixture output (doc/0015); parsing of the resume output; and
//! the sidecar record.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_claude::{Fake, cat, cat_err, fixture_path, lock};
use watchtower::actions::StopError;
use watchtower::app::HandoffJob;
use watchtower::cli::{CliOutput, CliRunner};
use watchtower::config::Account;
use watchtower::handoff::{
    self, HandoffError, HandoffRequest, HandoffTiming, Outcome, Resumed, Step, interpret_resume,
};
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::sidecar::{AccountSource, Sidecar};

const HOME: &str = "/home/user";
const ID: &str = "d4a1f7c8";
const SID: &str = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";

fn account(name: &str, command: &str) -> Account {
    Account::new(name, command, None, Path::new(HOME)).unwrap()
}

fn request(cwd: &Path, owner: Option<Account>, target: Account, live: bool) -> HandoffRequest {
    HandoffRequest {
        id: ID.into(),
        session_id: SID.into(),
        name: "touch-config".into(),
        cwd: cwd.to_path_buf(),
        owner,
        target,
        live,
        prompt: "continue".into(),
    }
}

fn out(code: i32, stdout: &str, stderr: &str) -> CliOutput {
    CliOutput {
        code: Some(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.into(),
    }
}

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(fixture_path(rel)).unwrap()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// For a stop wait that should end in a stop: the wait ends as soon as the
/// row is listed stopped, so a generous limit costs nothing when it works,
/// and a loaded machine (the fake command is a shell script, slow to spawn
/// on a busy CI runner) has time for the listings before it. The tests of
/// the timeout itself set a short one.
fn until_stopped() -> HandoffTiming {
    HandoffTiming {
        stop_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(20),
    }
}

/// One `touch-config` row, live (`pid`, `status`) or as after `stop`.
fn row_json(live: bool) -> String {
    let process = if live {
        r#""pid": 4377, "status": "waiting", "waitingFor": "permission prompt","#
    } else {
        ""
    };
    format!(
        r#"[{{ {process} "id": "{ID}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767232875668,
            "sessionId": "{SID}", "name": "touch-config", "state": "blocked" }}]"#
    )
}

/// Listing files, the project folder and the scripts' data, in one place.
struct World {
    data: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(data.path().join("demo-app")).unwrap();
        std::fs::write(data.path().join("live.json"), row_json(true)).unwrap();
        std::fs::write(data.path().join("stopped.json"), row_json(false)).unwrap();
        World { data }
    }

    fn project(&self) -> PathBuf {
        self.data.path().join("demo-app")
    }

    fn file(&self, name: &str) -> String {
        self.data.path().join(name).display().to_string()
    }

    /// A body for the account that stops: `stop` succeeds, `agents` lists
    /// the row live for the first `live_polls` calls and stopped after.
    fn stopper_body(&self, live_polls: u32) -> String {
        format!(
            r#"case "$1" in
  stop) echo "stopped {ID}" ;;
  agents)
    c=$(cat "$FAKE_DIR/agents.n" 2>/dev/null || echo 0); c=$((c+1)); echo $c > "$FAKE_DIR/agents.n"
    if [ $c -le {live_polls} ]; then cat '{live}'; else cat '{stopped}'; fi ;;
  *) echo "unexpected $1" >&2; exit 99 ;;
esac"#,
            live = self.file("live.json"),
            stopped = self.file("stopped.json"),
        )
    }
}

/// A body for the target account: `--resume` prints `stdout`/`stderr`
/// fixtures and exits `code`.
fn resumer_body(stdout: Option<&str>, stderr: Option<&str>, code: i32) -> String {
    let mut parts = Vec::new();
    if let Some(s) = stdout {
        parts.push(cat(s));
    }
    if let Some(s) = stderr {
        parts.push(cat_err(s));
    }
    parts.push(format!("exit {code}"));
    format!(
        "case \"$1\" in\n  --resume) {} ;;\n  *) echo \"unexpected $1\" >&2; exit 99 ;;\nesac",
        parts.join("; ")
    )
}

fn run(req: &HandoffRequest, timing: &HandoffTiming) -> (Result<Resumed, HandoffError>, Vec<Step>) {
    let mut steps = Vec::new();
    let result = handoff::run(&CliRunner::new(), req, timing, &mut |s| steps.push(s));
    (result, steps)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

// ---- command lines and rules ---------------------------------------------

#[test]
fn resume_command_is_target_resume_bg_prompt_in_cwd() {
    let alt = account("alt", "claude-alt");
    let mut req = request(Path::new("/home/user/workspace/demo-app"), None, alt, false);
    req.prompt = "  carry on from where you stopped \n".into();
    let line = req.resume_command();
    assert_eq!(
        line.argv(),
        strings(&[
            "claude-alt",
            "--resume",
            SID,
            "--bg",
            "carry on from where you stopped"
        ])
    );
    assert_eq!(
        line.cwd.as_deref(),
        Some(Path::new("/home/user/workspace/demo-app"))
    );
    // Account command words come first.
    let wrapped = account("alt", "env CLAUDE_CONFIG_DIR=/x claude");
    let req = request(Path::new("/tmp"), None, wrapped, false);
    assert_eq!(
        req.resume_command().argv()[..4],
        strings(&["env", "CLAUDE_CONFIG_DIR=/x", "claude", "--resume"])
    );
}

#[test]
fn stop_account_owner_target_or_none() {
    let main = account("main", "claude");
    let alt = account("alt", "claude-alt");
    let cwd = Path::new("/tmp");

    let owned = request(cwd, Some(main.clone()), alt.clone(), true);
    assert_eq!(owned.stop_account().map(|a| a.name.as_str()), Some("main"));
    assert_eq!(
        owned.stop_command().unwrap().argv(),
        strings(&["claude", "stop", ID])
    );
    assert_eq!(owned.plan(), "stop under main, continue under alt");

    // Unknown owner: `stop` works from any account, so the target stops it.
    let unknown = request(cwd, None, alt.clone(), true);
    assert_eq!(unknown.stop_account().map(|a| a.name.as_str()), Some("alt"));
    assert_eq!(
        unknown.stop_command().unwrap().argv(),
        strings(&["claude-alt", "stop", ID])
    );
    assert!(
        unknown.plan().contains("owner unknown"),
        "{}",
        unknown.plan()
    );

    // No live process: no stop.
    let stopped = request(cwd, Some(main), alt, false);
    assert!(stopped.stop_account().is_none());
    assert!(stopped.stop_command().is_none());
    assert_eq!(stopped.plan(), "not running; continue under alt");
    assert!(owned.after_stop().stop_account().is_none());
}

// ---- parsing the resume output -------------------------------------------

#[test]
fn captured_resume_without_prompt_is_continued_but_idle() {
    let o = out(
        0,
        &fixture("cli-output/resume-noprompt.stdout"),
        &fixture("cli-output/resume-noprompt.stderr"),
    );
    let r = interpret_resume(&o, ID, Path::new("/tmp")).unwrap();
    assert_eq!(r.id, ID);
    assert_eq!(r.name.as_deref(), Some("touch-config"));
    assert_eq!(r.outcome, Outcome::Continued);
    assert!(r.idle, "the noprompt capture woke idle");
    assert_eq!(
        r.notes,
        [format!(
            "note: woke session {ID} with its saved options (--name, --permission-mode, --model)."
        )]
    );
}

#[test]
fn resume_with_prompt_is_continued() {
    let o = out(
        0,
        &fixture("cli-output/resume-prompt.stdout"),
        &fixture("cli-output/resume-prompt.stderr"),
    );
    let r = interpret_resume(&o, ID, Path::new("/tmp")).unwrap();
    assert_eq!(r.outcome, Outcome::Continued);
    assert!(!r.idle);
    assert_eq!(r.notes.len(), 1);
    // A supervisor start before it changes nothing.
    let o = out(
        0,
        &fixture("cli-output/resume-prompt.stdout"),
        &format!(
            "Starting background service…\n{}",
            fixture("cli-output/resume-prompt.stderr")
        ),
    );
    let r = interpret_resume(&o, ID, Path::new("/tmp")).unwrap();
    assert_eq!(r.notes.len(), 1, "only note: lines are kept");
}

#[test]
fn different_id_is_a_copy() {
    // Made-up wording: the copy case has not been captured (doc/0004).
    let o = out(
        0,
        "backgrounded · 0a1b2c3d · touch-config\n  claude attach 0a1b2c3d    open in this terminal\n",
        &format!("note: session {ID} is still running; started a copy.\n"),
    );
    let r = interpret_resume(&o, ID, Path::new("/tmp")).unwrap();
    assert_eq!(
        r.outcome,
        Outcome::Copied {
            new_id: "0a1b2c3d".into()
        }
    );
    assert_eq!(r.id, "0a1b2c3d");
    assert!(r.notes[0].starts_with("note: session"));
}

#[test]
fn untrusted_needs_exit_1_and_the_message() {
    let stderr = fixture("cli-output/bg-untrusted.stderr");
    let dir = Path::new("/home/user/workspace/untrusted-repo");
    match interpret_resume(&out(1, "", &stderr), ID, dir) {
        Err(HandoffError::Untrusted { dir: d, message }) => {
            assert_eq!(d, dir);
            assert!(message.starts_with("Workspace not trusted"));
        }
        other => panic!("{other:?}"),
    }
    let with_start = format!("Starting background service…\n{stderr}");
    assert!(matches!(
        interpret_resume(&out(1, "", &with_start), ID, dir),
        Err(HandoffError::Untrusted { .. })
    ));
    assert!(matches!(
        interpret_resume(&out(2, "", &stderr), ID, dir),
        Err(HandoffError::Failed { code: Some(2), .. })
    ));
}

#[test]
fn other_failures_keep_code_and_text() {
    match interpret_resume(&out(1, "", "No conversation found\n"), ID, Path::new("/")) {
        Err(e @ HandoffError::Failed { .. }) => {
            assert!(e.to_string().contains("exit 1"), "{e}");
            assert!(e.to_string().contains("No conversation found"), "{e}");
            assert!(e.after_stop());
        }
        other => panic!("{other:?}"),
    }
    // Only stdout said something.
    match interpret_resume(&out(3, "boom\n", ""), ID, Path::new("/")) {
        Err(HandoffError::Failed { stderr, .. }) => assert_eq!(stderr, "boom"),
        other => panic!("{other:?}"),
    }
    match interpret_resume(&out(0, "something else\n", "x"), ID, Path::new("/")) {
        Err(HandoffError::Unparsed { stdout, stderr }) => {
            assert_eq!(stdout, "something else");
            assert_eq!(stderr, "x");
        }
        other => panic!("{other:?}"),
    }
}

// ---- the whole sequence against fake accounts ----------------------------

#[test]
fn stops_with_owner_waits_then_resumes_with_target_in_cwd() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(2));
    let alt_fake = Fake::new(
        "claude-alt",
        &resumer_body(
            Some("cli-output/resume-prompt.stdout"),
            Some("cli-output/resume-prompt.stderr"),
            0,
        ),
    );
    let main = main_fake.account_named("main", None);
    let alt = alt_fake.account_named("alt", Some("~/.claude-alt"));
    let req = request(&world.project(), Some(main), alt, true);

    let (result, steps) = run(&req, &until_stopped());
    let r = result.unwrap();
    assert_eq!(r.outcome, Outcome::Continued);
    assert_eq!(
        steps,
        [
            Step::Stopping {
                account: "main".into()
            },
            Step::WaitingForStop,
            Step::Resuming {
                account: "alt".into()
            },
        ]
    );
    // main: stop, then the listing until the process is gone (2 live, 1 stopped).
    let main_calls = main_fake.all_argv();
    assert_eq!(main_calls[0], strings(&["stop", ID]));
    assert_eq!(main_calls.len(), 4, "{main_calls:?}");
    for call in &main_calls[1..] {
        assert_eq!(call, &strings(&["agents", "--json", "--all"]));
    }
    // alt: exactly one call, the resume, run in the session's cwd.
    assert_eq!(alt_fake.calls(), 1);
    assert_eq!(
        alt_fake.argv_of(1),
        strings(&["--resume", SID, "--bg", "continue"])
    );
    assert_eq!(alt_fake.cwd_of(1), canon(&world.project()));
}

#[test]
fn no_live_process_skips_the_stop() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(0));
    let alt_fake = Fake::new(
        "claude-alt",
        &resumer_body(Some("cli-output/resume-prompt.stdout"), None, 0),
    );
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        false,
    );
    let (result, steps) = run(&req, &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(
        steps,
        [Step::Resuming {
            account: "alt".into()
        }]
    );
    assert_eq!(main_fake.calls(), 0, "nothing runs under the owner");
    assert_eq!(
        alt_fake.all_argv(),
        [strings(&["--resume", SID, "--bg", "continue"])]
    );
}

#[test]
fn unknown_owner_is_stopped_with_the_target_command() {
    let _l = lock();
    let world = World::new();
    // The target both stops (and lists) and resumes.
    let body = world.stopper_body(1).replace(
        "  *) echo",
        &format!(
            "  --resume) {}; {} ;;\n  *) echo",
            cat("cli-output/resume-prompt.stdout"),
            cat_err("cli-output/resume-prompt.stderr")
        ),
    );
    let alt_fake = Fake::new("claude-alt", &body);
    let req = request(
        &world.project(),
        None,
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, steps) = run(&req, &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(
        steps[0],
        Step::Stopping {
            account: "alt".into()
        }
    );
    let calls = alt_fake.all_argv();
    assert_eq!(calls[0], strings(&["stop", ID]));
    assert_eq!(calls[1], strings(&["agents", "--json", "--all"]));
    assert_eq!(calls[2], strings(&["agents", "--json", "--all"]));
    assert_eq!(calls[3], strings(&["--resume", SID, "--bg", "continue"]));
    assert_eq!(calls.len(), 4);
    assert_eq!(alt_fake.cwd_of(4), canon(&world.project()));
}

#[test]
fn still_running_after_the_timeout_is_not_resumed() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(u32::MAX / 2));
    let alt_fake = Fake::new("claude-alt", &resumer_body(None, None, 0));
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let timing = HandoffTiming {
        stop_timeout: Duration::from_millis(300),
        poll_interval: Duration::from_millis(50),
    };
    let (result, _) = run(&req, &timing);
    match result {
        Err(e @ HandoffError::StillRunning { .. }) => {
            assert!(e.to_string().contains("still runs"), "{e}");
            assert!(e.after_stop());
            // It kept listing until the timeout, not giving up on the first
            // live row. How many listings fit in it depends on how fast the
            // fake spawns, so only the wait is checked.
            let HandoffError::StillRunning { waited, .. } = e else {
                unreachable!()
            };
            assert!(waited >= timing.stop_timeout, "{waited:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(alt_fake.calls(), 0, "never resumed");
    assert!(main_fake.calls() >= 2, "stopped, then listed");
}

#[test]
fn listing_errors_are_retried_until_stopped() {
    let _l = lock();
    let world = World::new();
    // First listing fails, the second shows the row stopped.
    let body = format!(
        r#"case "$1" in
  stop) echo "stopped {ID}" ;;
  agents) if [ "$n" -le 2 ]; then echo "listing broke" >&2; exit 1; fi; cat '{}' ;;
esac"#,
        world.file("stopped.json")
    );
    let main_fake = Fake::new("claude", &body);
    let alt_fake = Fake::new(
        "claude-alt",
        &resumer_body(Some("cli-output/resume-prompt.stdout"), None, 0),
    );
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, _) = run(&req, &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(main_fake.calls(), 3);
}

/// Another session, so a listing can be non-empty without the handed-off row.
const OTHER_ROW: &str = r#"[{ "id": "0a1b2c3d", "cwd": "/home/user/workspace/demo-app",
    "kind": "background", "sessionId": "0a1b2c3d-1111-4222-8333-444455556666",
    "name": "other", "state": "done" }]"#;

#[test]
fn stop_evidence_needs_the_row_or_a_non_empty_complete_listing() {
    use handoff::{StopEvidence, stop_evidence};
    let req = request(Path::new("/tmp"), None, account("alt", "claude-alt"), true);
    let listed = |json: &str| Ok(parse_listing(json).unwrap());
    assert_eq!(
        stop_evidence(&listed(&row_json(false)), &req),
        StopEvidence::Stopped
    );
    assert_eq!(
        stop_evidence(&listed(&row_json(true)), &req),
        StopEvidence::Running
    );
    assert_eq!(stop_evidence(&listed("[]"), &req), StopEvidence::Nothing);
    assert_eq!(
        stop_evidence(&listed(OTHER_ROW), &req),
        StopEvidence::Absent
    );
    // A listing whose parser skipped a row may have skipped this one.
    let partial = format!(
        "[{}, 42]",
        OTHER_ROW
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
    );
    let partial = parse_listing(&partial).unwrap();
    assert!(!partial.warnings.is_empty());
    assert_eq!(stop_evidence(&Ok(partial), &req), StopEvidence::Nothing);
    assert_eq!(
        stop_evidence(&Err("listing broke".into()), &req),
        StopEvidence::Nothing
    );
}

/// A stopper whose listings after `stop` follow `files` (one per call,
/// the last one repeating).
fn scripted_stopper(world: &World, files: &[&str]) -> String {
    let cases: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, f)| format!("    {}) cat '{}' ;;", i + 1, world.file(f)))
        .collect();
    format!(
        r#"case "$1" in
  stop) echo "stopped {ID}" ;;
  agents)
    c=$(cat "$FAKE_DIR/agents.n" 2>/dev/null || echo 0); c=$((c+1)); echo $c > "$FAKE_DIR/agents.n"
    case $c in
{}
    *) cat '{}' ;;
    esac ;;
esac"#,
        cases.join("\n"),
        world.file(files.last().unwrap())
    )
}

fn alt_resumer() -> Fake {
    Fake::new(
        "claude-alt",
        &resumer_body(Some("cli-output/resume-prompt.stdout"), None, 0),
    )
}

#[test]
fn an_empty_listing_does_not_end_the_stop_wait() {
    let _l = lock();
    let world = World::new();
    std::fs::write(world.data.path().join("empty.json"), "[]").unwrap();
    // Live, then empty listings while the process still lives: never resumed.
    let main_fake = Fake::new(
        "claude",
        &scripted_stopper(&world, &["live.json", "empty.json"]),
    );
    let alt_fake = alt_resumer();
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let timing = HandoffTiming {
        stop_timeout: Duration::from_millis(300),
        poll_interval: Duration::from_millis(30),
    };
    let (result, _) = run(&req, &timing);
    assert!(
        matches!(result, Err(HandoffError::StillRunning { .. })),
        "{result:?}"
    );
    assert_eq!(alt_fake.calls(), 0, "never resumed");
}

#[test]
fn one_listing_without_the_row_is_not_enough_two_in_a_row_are() {
    let _l = lock();
    let world = World::new();
    std::fs::write(world.data.path().join("empty.json"), "[]").unwrap();
    std::fs::write(world.data.path().join("other.json"), OTHER_ROW).unwrap();
    // Missing, empty (breaks the run), missing, missing: gone after call 4.
    let main_fake = Fake::new(
        "claude",
        &scripted_stopper(
            &world,
            &[
                "other.json",
                "empty.json",
                "other.json",
                "other.json",
                "live.json",
            ],
        ),
    );
    let alt_fake = alt_resumer();
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, _) = run(&req, &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(main_fake.calls(), 1 + 4, "stop + four listings");
    assert_eq!(alt_fake.calls(), 1);

    // A live sighting in between resets the count too.
    let main_fake = Fake::new(
        "claude",
        &scripted_stopper(
            &world,
            &["other.json", "live.json", "other.json", "stopped.json"],
        ),
    );
    let alt_fake = alt_resumer();
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, _) = run(&req, &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(main_fake.calls(), 1 + 4, "ended by the stopped row");
}

#[test]
fn a_failed_stop_aborts_before_the_resume() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new(
        "claude",
        &format!("{}; exit 1", cat_err("cli-output/stop-unknown.stderr")),
    );
    let alt_fake = Fake::new("claude-alt", &resumer_body(None, None, 0));
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, steps) = run(&req, &until_stopped());
    match result {
        Err(e @ HandoffError::Stop(StopError::UnknownId { .. })) => assert!(!e.after_stop()),
        other => panic!("{other:?}"),
    }
    assert_eq!(steps.len(), 1);
    assert_eq!(main_fake.calls(), 1);
    assert_eq!(alt_fake.calls(), 0);
}

#[test]
fn untrusted_resume_then_retry_without_a_second_stop() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(0));
    // First resume: untrusted; the retry succeeds.
    let alt_body = format!(
        r#"if [ "$n" -eq 1 ]; then {}; exit 1; fi; {}; {}"#,
        cat_err("cli-output/bg-untrusted.stderr"),
        cat("cli-output/resume-prompt.stdout"),
        cat_err("cli-output/resume-prompt.stderr"),
    );
    let alt_fake = Fake::new("claude-alt", &alt_body);
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let (result, _) = run(&req, &until_stopped());
    match result {
        Err(HandoffError::Untrusted { dir, .. }) => assert_eq!(dir, world.project()),
        other => panic!("{other:?}"),
    }
    assert_eq!(main_fake.calls(), 2, "stop + one listing");

    let (result, steps) = run(&req.after_stop(), &until_stopped());
    assert_eq!(result.unwrap().outcome, Outcome::Continued);
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(main_fake.calls(), 2, "not stopped again");
    assert_eq!(alt_fake.calls(), 2);
    assert_eq!(
        alt_fake.argv_of(2),
        strings(&["--resume", SID, "--bg", "continue"])
    );
}

#[test]
fn preconditions_are_checked_before_anything_runs() {
    let _l = lock();
    let main_fake = Fake::new("claude", "echo stopped; exit 0");
    let alt_fake = Fake::new("claude-alt", "exit 0");
    let main = main_fake.account_named("main", None);
    let alt = alt_fake.account_named("alt", None);
    let world = World::new();

    let missing = request(
        Path::new("/nonexistent/watchtower-test"),
        Some(main.clone()),
        alt.clone(),
        true,
    );
    assert!(matches!(
        run(&missing, &until_stopped()).0,
        Err(HandoffError::CwdMissing(_))
    ));
    for prompt in ["", "   ", "-p"] {
        let mut req = request(&world.project(), Some(main.clone()), alt.clone(), true);
        req.prompt = prompt.into();
        assert!(
            matches!(run(&req, &until_stopped()).0, Err(HandoffError::Invalid(_))),
            "{prompt:?}"
        );
    }
    let mut req = request(&world.project(), Some(main.clone()), alt.clone(), true);
    req.id = "say-ok please".into();
    assert!(matches!(
        run(&req, &until_stopped()).0,
        Err(HandoffError::Invalid(_))
    ));
    let mut req = request(&world.project(), Some(main), alt, true);
    req.session_id = "--all".into();
    assert!(matches!(
        run(&req, &until_stopped()).0,
        Err(HandoffError::Invalid(_))
    ));
    assert_eq!(main_fake.calls() + alt_fake.calls(), 0);
}

// ---- the sidecar record -----------------------------------------------------

#[test]
fn record_continued_under_session_id_and_copy_under_its_short_id() {
    let req = request(
        Path::new("/tmp"),
        Some(account("main", "claude")),
        account("alt", "claude-alt"),
        true,
    );
    let mut sc = Sidecar::default();
    sc.record_owner(SID, "main", AccountSource::Started);

    let continued = Resumed {
        id: ID.into(),
        name: None,
        outcome: Outcome::Continued,
        notes: Vec::new(),
        idle: false,
    };
    assert!(handoff::record(&mut sc, &req, &continued));
    assert_eq!(sc.owner(SID), Some(("alt", &AccountSource::Handoff)));
    assert!(!handoff::record(&mut sc, &req, &continued), "no change");

    let mut sc = Sidecar::default();
    sc.record_owner(SID, "main", AccountSource::Started);
    let copied = Resumed {
        id: "0a1b2c3d".into(),
        outcome: Outcome::Copied {
            new_id: "0a1b2c3d".into(),
        },
        ..continued
    };
    assert!(handoff::record(&mut sc, &req, &copied));
    assert_eq!(sc.owner("0a1b2c3d"), Some(("alt", &AccountSource::Handoff)));
    // The next listing shows the copy's sessionId; the record moves there.
    let copy_sid = "0a1b2c3d-1111-4222-8333-444455556666";
    let listing = parse_listing(&format!(
        r#"[{{"id": "0a1b2c3d", "sessionId": "{copy_sid}", "cwd": "/tmp", "kind": "background"}}]"#
    ))
    .unwrap();
    assert!(sc.adopt_short_id_records(&listing.sessions));
    assert_eq!(sc.owner(copy_sid), Some(("alt", &AccountSource::Handoff)));
    assert_eq!(sc.owner("0a1b2c3d"), None);
    assert_eq!(
        sc.owner(SID),
        Some(("main", &AccountSource::Started)),
        "original untouched"
    );
}

// ---- the worker thread -------------------------------------------------------

fn kind(r: &JobResult) -> String {
    match r {
        JobResult::HandoffStep(Step::Stopping { account }) => format!("step: stopping {account}"),
        JobResult::HandoffStep(Step::WaitingForStop) => "step: waiting".into(),
        JobResult::HandoffStep(Step::Resuming { account }) => format!("step: resuming {account}"),
        JobResult::Handoff(..) => "handoff".into(),
        other => format!("{other:?}"),
    }
}

#[test]
fn jobs_report_each_step_then_the_result() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(1));
    let alt_fake = Fake::new(
        "claude-alt",
        &resumer_body(
            Some("cli-output/resume-prompt.stdout"),
            Some("cli-output/resume-prompt.stderr"),
            0,
        ),
    );
    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let jobs = Jobs::new(CliRunner::new()).with_handoff_timing(until_stopped());
    jobs.handoff(HandoffJob {
        request: req.clone(),
        after_trust: false,
        recorded: None,
    });
    let mut kinds = Vec::new();
    loop {
        let Some(done) = jobs.recv_timeout(Duration::from_secs(20)) else {
            panic!("no result after {kinds:?}");
        };
        kinds.push(kind(&done));
        if let JobResult::Handoff(job, result) = done {
            assert_eq!(job.request, req);
            assert_eq!(result.unwrap().outcome, Outcome::Continued);
            break;
        }
    }
    assert_eq!(
        kinds,
        [
            "step: stopping main",
            "step: waiting",
            "step: resuming alt",
            "handoff"
        ]
    );
}

/// Another tower's newer record is honoured before waking (doc/0017
/// section 8): when the sidecar file records the session under another
/// account than the one this tower planned with, nothing is stopped or
/// resumed; when it names the planned owner, the hand-off goes on.
#[test]
fn a_record_changed_by_another_tower_stops_the_handoff_before_waking() {
    let _l = lock();
    let world = World::new();
    let main_fake = Fake::new("claude", &world.stopper_body(1));
    let alt_fake = Fake::new(
        "claude-alt",
        &resumer_body(
            Some("cli-output/resume-prompt.stdout"),
            Some("cli-output/resume-prompt.stderr"),
            0,
        ),
    );
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("state.json");
    // This tower planned with `started main`; another tower then recorded
    // a hand-off to `spare`.
    let mut mine = Sidecar::default();
    mine.record_owner(SID, "main", AccountSource::Started);
    mine.save(&path).unwrap();
    let recorded = Sidecar::load(&path).sidecar.base_owner(SID);
    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner(SID, "spare", AccountSource::Handoff);
    other.save(&path).unwrap();

    let req = request(
        &world.project(),
        Some(main_fake.account_named("main", None)),
        alt_fake.account_named("alt", None),
        true,
    );
    let jobs = Jobs::new(CliRunner::new())
        .with_handoff_timing(until_stopped())
        .with_state_file(path.clone());
    let run = |recorded| {
        jobs.handoff(HandoffJob {
            request: req.clone(),
            after_trust: false,
            recorded,
        });
        loop {
            match jobs.recv_timeout(Duration::from_secs(20)).expect("reports") {
                JobResult::Handoff(_, result) => return result,
                JobResult::HandoffStep(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    match run(recorded.clone()) {
        Err(HandoffError::OwnerChanged { account, stopped }) => {
            assert_eq!(account, "spare");
            assert!(!stopped);
        }
        other => panic!("expected OwnerChanged, got {other:?}"),
    }
    assert_eq!(main_fake.calls(), 0, "nothing stopped");
    assert_eq!(alt_fake.calls(), 0, "nothing woken");

    // A record that still names the planned owner (here: this tower's own
    // later save) lets it go on.
    let mut again = Sidecar::load(&path).sidecar;
    again.record_owner(SID, "main", AccountSource::Proc);
    again.save(&path).unwrap();
    assert_eq!(run(recorded).unwrap().outcome, Outcome::Continued);
    assert_eq!(alt_fake.calls(), 1);
}
