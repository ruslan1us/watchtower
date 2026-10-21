//! The quick reply job (doc/0040): the exact argv, working directory and
//! order of the stop, the stop wait and `--resume <sessionId> --bg --
//! <text>` with the owner's command, run against a fake owner command
//! (doc/0015); a stopped row, a text that starts with `-`, a multi-line
//! text, a copy, an untrusted folder, a session that does not stop, a
//! request that runs nothing, and another tower's newer record.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_claude::{Fake, lock};
use watchtower::app::ReplyJob;
use watchtower::cli::CliRunner;
use watchtower::config::Account;
use watchtower::handoff::HandoffTiming;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::reply::{self, Replied, ReplyError, ReplyRequest, ReplyStep};
use watchtower::sidecar::{AccountSource, Sidecar};

const HOME: &str = "/home/user";
const ID: &str = "b2f8d064";
const SID: &str = "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81";
const COPY: &str = "0c0d0e0f";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// For a stop wait that should end in a stop: it ends as soon as the row
/// is listed stopped, so a generous limit costs nothing when it works and
/// gives a loaded machine time for the listings.
fn until_stopped() -> HandoffTiming {
    HandoffTiming {
        stop_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(20),
    }
}

/// For a session that never stops: the test waits the whole limit.
fn gives_up_soon() -> HandoffTiming {
    HandoffTiming {
        stop_timeout: Duration::from_millis(600),
        poll_interval: Duration::from_millis(20),
    }
}

/// The `say-ok` row, live and idle after its turn, or as after `stop`.
fn row_json(live: bool) -> String {
    let process = if live {
        r#""pid": 4342, "status": "idle","#
    } else {
        ""
    };
    format!(
        r#"[{{ {process} "id": "{ID}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767232822505,
            "sessionId": "{SID}", "name": "say-ok", "state": "done" }}]"#
    )
}

/// The project folder, the listings and the wake's output, in one place.
struct World {
    data: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(data.path().join("demo-app")).unwrap();
        let write = |f: &str, text: String| std::fs::write(data.path().join(f), text).unwrap();
        write("live.json", row_json(true));
        write("stopped.json", row_json(false));
        write(
            "woke.stdout",
            format!(
                "backgrounded · {ID} · say-ok\n  claude agents             list sessions\n  \
                 claude attach {ID}    open in this terminal\n"
            ),
        );
        write(
            "woke.stderr",
            format!("note: woke session {ID} with its saved options (--name, --model).\n"),
        );
        // What Claude Code printed for a live session (reference doc).
        write(
            "copy.stdout",
            format!("backgrounded · {COPY}\n  claude agents             list sessions\n"),
        );
        write(
            "copy.stderr",
            format!(
                "note: session {ID} is already running in the background, so this started a \
                 copy as {COPY}. `claude attach {ID}` opens the original.\n"
            ),
        );
        write(
            "untrusted.stderr",
            "Workspace not trusted. Run `claude` in /home/user/workspace/demo-app once and \
             accept the trust prompt, then retry.\n"
                .to_owned(),
        );
        World { data }
    }

    fn project(&self) -> PathBuf {
        self.data.path().join("demo-app")
    }

    fn file(&self, name: &str) -> String {
        self.data.path().join(name).display().to_string()
    }

    /// The owner's command: `stop` stops, `agents` lists the row live for
    /// the first `live_polls` calls and stopped after (never with
    /// `u32::MAX`), `--resume` prints `wake` (`woke`, `copy`; `untrusted`
    /// fails like an untrusted folder).
    fn owner_body(&self, wake: &str, live_polls: u32) -> String {
        let resume = if wake == "untrusted" {
            format!("cat '{}' >&2; exit 1", self.file("untrusted.stderr"))
        } else {
            format!(
                "cat '{}'; cat '{}' >&2",
                self.file(&format!("{wake}.stdout")),
                self.file(&format!("{wake}.stderr"))
            )
        };
        format!(
            r#"case "$1" in
  --resume) {resume} ;;
  agents)
    c=$(cat "$FAKE_DIR/agents.n" 2>/dev/null || echo 0); c=$((c+1)); echo $c > "$FAKE_DIR/agents.n"
    if [ $c -le {live_polls} ]; then cat '{live}'; else cat '{stopped}'; fi ;;
  stop) echo "stopped {ID}" ;;
  *) echo "unexpected $1" >&2; exit 99 ;;
esac"#,
            live = self.file("live.json"),
            stopped = self.file("stopped.json"),
        )
    }
}

fn request(cwd: &Path, owner: Account, live: bool, text: &str) -> ReplyRequest {
    ReplyRequest {
        id: ID.into(),
        session_id: SID.into(),
        name: "say-ok".into(),
        cwd: cwd.to_path_buf(),
        owner,
        live,
        text: text.into(),
    }
}

fn run(
    req: &ReplyRequest,
    timing: &HandoffTiming,
) -> (Result<Replied, ReplyError>, Vec<ReplyStep>) {
    let mut steps = Vec::new();
    let result = reply::run(
        &CliRunner::new(),
        req,
        timing,
        &mut |s| steps.push(s),
        &mut |_| Ok(()),
    );
    (result, steps)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

const REVIEW: &str = "Review the diff.\n- check the tests\n- then report back";

#[test]
fn the_commands_use_the_owner_and_end_the_options_before_the_text() {
    let owner = Account::new("alt", "claude-alt", None, Path::new(HOME)).unwrap();
    let req = request(
        Path::new("/home/user/workspace/demo-app"),
        owner,
        true,
        REVIEW,
    );
    let line = req.resume_command();
    assert_eq!(
        line.argv(),
        strings(&["claude-alt", "--resume", SID, "--bg", "--", REVIEW])
    );
    assert_eq!(
        line.cwd.as_deref(),
        Some(Path::new("/home/user/workspace/demo-app"))
    );
    assert_eq!(
        req.stop_command().unwrap().argv(),
        strings(&["claude-alt", "stop", ID])
    );
    assert!(req.after_stop().stop_command().is_none());
    assert!(!req.after_stop().live);
}

#[test]
fn a_live_session_is_stopped_then_woken_with_the_text() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 2));
    let req = request(&world.project(), fake.account(), true, REVIEW);
    let (result, steps) = run(&req, &until_stopped());
    let replied = result.expect("sent");
    assert_eq!(
        replied.notes,
        [format!(
            "note: woke session {ID} with its saved options (--name, --model)."
        )]
    );
    assert_eq!(
        steps,
        [
            ReplyStep::Stopping {
                account: "main".into()
            },
            ReplyStep::WaitingForStop,
            ReplyStep::Waking {
                account: "main".into()
            },
        ]
    );
    // Stop, three listings (two still live), the wake: all with the
    // owner's command.
    let calls = fake.all_argv();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert_eq!(calls[0], strings(&["stop", ID]));
    for listing in &calls[1..4] {
        assert_eq!(*listing, strings(&["agents", "--json", "--all"]));
    }
    // The multi-line text arrives as one argument, intact.
    assert_eq!(calls[4], strings(&["--resume", SID, "--bg", "--", REVIEW]));
    assert_eq!(fake.cwd_of(5), canon(&world.project()));
}

#[test]
fn a_stopped_session_is_only_woken() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 0));
    let req = request(
        &world.project(),
        fake.account(),
        false,
        "looks good, merge it",
    );
    let (result, steps) = run(&req, &until_stopped());
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        steps,
        [ReplyStep::Waking {
            account: "main".into()
        }]
    );
    assert_eq!(
        fake.all_argv(),
        [strings(&[
            "--resume",
            SID,
            "--bg",
            "--",
            "looks good, merge it"
        ])]
    );
    assert_eq!(fake.cwd(), canon(&world.project()));
}

#[test]
fn a_text_that_starts_with_a_dash_stays_one_argument_after_the_marker() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 0));
    for text in ["- fix the lint first", "--help is not an option here", "-v"] {
        let req = request(&world.project(), fake.account(), false, text);
        let (result, _) = run(&req, &until_stopped());
        assert!(result.is_ok(), "{text}: {result:?}");
        assert_eq!(fake.argv(), strings(&["--resume", SID, "--bg", "--", text]));
    }
}

#[test]
fn a_copy_is_an_error_naming_it_and_nothing_else_runs() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("copy", 0));
    let (result, steps) = run(
        &request(&world.project(), fake.account(), false, "go on"),
        &until_stopped(),
    );
    match result {
        Err(err @ ReplyError::Copied { .. }) => {
            assert!(err.to_string().contains(COPY), "{err}");
            let ReplyError::Copied { new_id, notes } = err else {
                unreachable!()
            };
            assert_eq!(new_id, COPY);
            assert!(notes[0].contains("started a copy as"), "{notes:?}");
        }
        other => panic!("expected a copy, got {other:?}"),
    }
    assert_eq!(steps.len(), 1, "only the wake");
    assert_eq!(fake.calls(), 1, "no stop, no rm of anything");
}

#[test]
fn an_untrusted_folder_is_reported_for_the_trust_flow() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("untrusted", 1));
    let req = request(&world.project(), fake.account(), true, "go on");
    let (result, _) = run(&req, &until_stopped());
    match result {
        Err(err @ ReplyError::Untrusted { .. }) => {
            assert!(err.after_stop(), "the session was stopped");
            let ReplyError::Untrusted { dir, message } = err else {
                unreachable!()
            };
            assert_eq!(dir, world.project());
            assert!(message.starts_with("Workspace not trusted"), "{message}");
        }
        other => panic!("expected Untrusted, got {other:?}"),
    }
    // Stop, two listings, the wake. The retry does not stop again.
    assert_eq!(fake.calls(), 4);
    let (_, steps) = run(&req.after_stop(), &until_stopped());
    assert_eq!(steps.len(), 1);
    assert_eq!(fake.calls(), 5);
    assert_eq!(fake.argv()[0], "--resume");
}

#[test]
fn a_session_that_keeps_running_is_not_woken() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", u32::MAX));
    let req = request(&world.project(), fake.account(), true, "go on");
    let (result, _) = run(&req, &gives_up_soon());
    assert!(
        matches!(result, Err(ReplyError::StillRunning { .. })),
        "{result:?}"
    );
    assert!(
        fake.all_argv().iter().all(|a| a[0] != "--resume"),
        "never woken"
    );
}

#[test]
fn a_bad_request_runs_nothing() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 0));
    let missing = request(
        Path::new("/nonexistent/demo-app"),
        fake.account(),
        true,
        "x",
    );
    let (result, steps) = run(&missing, &until_stopped());
    assert!(
        matches!(result, Err(ReplyError::CwdMissing(_))),
        "{result:?}"
    );
    assert!(steps.is_empty());
    for text in ["   \n ", "a\0b"] {
        let req = request(&world.project(), fake.account(), true, text);
        assert!(
            matches!(run(&req, &until_stopped()).0, Err(ReplyError::Invalid(_))),
            "{text:?}"
        );
    }
    let big = "x".repeat(reply::MAX_TEXT_BYTES + 1);
    let req = request(&world.project(), fake.account(), true, &big);
    assert!(matches!(
        run(&req, &until_stopped()).0,
        Err(ReplyError::Invalid(_))
    ));
    let mut req = request(&world.project(), fake.account(), true, "x");
    req.id = "not an id".into();
    assert!(matches!(
        run(&req, &until_stopped()).0,
        Err(ReplyError::Invalid(_))
    ));
    assert_eq!(fake.calls(), 0);
}

/// As for a hand-off (doc/0017 section 8): when the state file records the
/// session under another account than the owner this tower replies with,
/// nothing more is run; the steps and the result come back through the
/// jobs, keyed by the session.
#[test]
fn jobs_report_steps_and_honour_another_towers_record() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 0));
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("state.json");
    let mut mine = Sidecar::default();
    mine.record_owner(SID, "main", AccountSource::Started);
    mine.save(&path).unwrap();
    let recorded = Sidecar::load(&path).sidecar.base_owner(SID);
    let jobs = Jobs::new(CliRunner::new())
        .with_handoff_timing(until_stopped())
        .with_state_file(path.clone());
    let req = request(&world.project(), fake.account(), true, REVIEW);
    let run = |recorded| {
        jobs.reply(ReplyJob {
            request: req.clone(),
            after_trust: false,
            recorded,
        });
        let mut steps = Vec::new();
        loop {
            match jobs.recv_timeout(Duration::from_secs(20)).expect("reports") {
                JobResult::Reply(job, result) => {
                    assert_eq!(job.request, req);
                    return (steps, result);
                }
                JobResult::ReplyStep(session_id, step) => {
                    assert_eq!(session_id, SID);
                    steps.push(step);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let (steps, result) = run(recorded.clone());
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(fake.calls(), 3, "stop, one listing, the wake");

    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner(SID, "alt", AccountSource::Handoff);
    other.save(&path).unwrap();
    match run(recorded).1 {
        Err(ReplyError::OwnerChanged { account, stopped }) => {
            assert_eq!(account, "alt");
            assert!(!stopped);
        }
        other => panic!("expected OwnerChanged, got {other:?}"),
    }
    assert_eq!(fake.calls(), 3, "nothing more was run");
}
