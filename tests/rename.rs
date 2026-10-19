//! Renaming a stopped session (doc/0027): the exact argv, working directory
//! and order of the wake with `/rename`, the listing until the new name
//! shows and the stop, run against a fake owner command (doc/0015); a copy,
//! a name that never shows, an untrusted folder, a missing folder, and
//! another tower's newer record.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fake_claude::{Fake, lock};
use watchtower::app::RenameJob;
use watchtower::cli::CliRunner;
use watchtower::config::Account;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::rename::{self, RenameError, RenameRequest, RenameStep, RenameTiming};
use watchtower::sidecar::{AccountSource, Sidecar};

const HOME: &str = "/home/user";
const ID: &str = "d4a1f7c8";
const SID: &str = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";
const COPY: &str = "0c0d0e0f";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// For a rename that should be confirmed: the wait ends as soon as the new
/// name is listed, so a generous limit costs nothing when it works, and a
/// loaded machine (the fake command is a shell script, slow to spawn on a
/// busy CI runner) has time for the listings before it.
fn until_confirmed() -> RenameTiming {
    RenameTiming {
        confirm_timeout: Duration::from_secs(30),
        poll_interval: Duration::from_millis(20),
    }
}

/// For a name that never shows: the test waits the whole limit, so it is
/// short.
fn gives_up_soon() -> RenameTiming {
    RenameTiming {
        confirm_timeout: Duration::from_millis(600),
        poll_interval: Duration::from_millis(20),
    }
}

/// The `touch-config` row named `name`, live or stopped.
fn row_json(name: &str, live: bool) -> String {
    let process = if live {
        r#""pid": 4377, "status": "idle","#
    } else {
        ""
    };
    format!(
        r#"[{{ {process} "id": "{ID}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767232875668,
            "sessionId": "{SID}", "name": "{name}", "state": "working" }}]"#
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
        write("old.json", row_json("touch-config", true));
        write("new.json", row_json("tidy-config", true));
        write(
            "woke.stdout",
            format!(
                "backgrounded · {ID} · touch-config\n  claude agents             list sessions\n"
            ),
        );
        write(
            "woke.stderr",
            format!("note: woke session {ID} with its saved options (--name, --model).\n"),
        );
        write(
            "copy.stdout",
            format!("backgrounded · {COPY} · touch-config\n"),
        );
        write(
            "copy.stderr",
            format!(
                "note: background session {ID} keeps its own saved options, so the flags you \
                 passed started a copy as {COPY}.\n"
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

    /// The owner's command: `--resume` prints `wake` (`woke`, `copy`;
    /// `untrusted` fails like an untrusted folder), `agents` lists the old
    /// name for the first `old_polls` calls and the new one after (never
    /// with `old_polls` = `u32::MAX`), `stop` stops.
    fn owner_body(&self, wake: &str, old_polls: u32) -> String {
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
    if [ $c -le {old_polls} ]; then cat '{old}'; else cat '{new}'; fi ;;
  stop) echo "stopped {ID}" ;;
  *) echo "unexpected $1" >&2; exit 99 ;;
esac"#,
            old = self.file("old.json"),
            new = self.file("new.json"),
        )
    }
}

fn request(cwd: &Path, owner: Account) -> RenameRequest {
    RenameRequest {
        id: ID.into(),
        session_id: SID.into(),
        old_name: "touch-config".into(),
        new_name: "tidy-config".into(),
        cwd: cwd.to_path_buf(),
        owner,
    }
}

fn run(
    req: &RenameRequest,
    timing: &RenameTiming,
) -> (Result<rename::Renamed, RenameError>, Vec<RenameStep>) {
    let mut steps = Vec::new();
    let result = rename::run(
        &CliRunner::new(),
        req,
        timing,
        &mut |s| steps.push(s),
        &mut || Ok(()),
    );
    (result, steps)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

#[test]
fn the_wake_command_is_owner_resume_bg_rename_in_cwd() {
    let owner = Account::new("alt", "claude-alt", None, Path::new(HOME)).unwrap();
    let mut req = request(Path::new("/home/user/workspace/demo-app"), owner);
    req.new_name = "two words".into();
    let line = req.resume_command();
    assert_eq!(
        line.argv(),
        strings(&["claude-alt", "--resume", SID, "--bg", "/rename two words"])
    );
    assert_eq!(
        line.cwd.as_deref(),
        Some(Path::new("/home/user/workspace/demo-app"))
    );
    assert_eq!(
        req.stop_command().argv(),
        strings(&["claude-alt", "stop", ID])
    );
}

#[test]
fn a_stopped_session_is_woken_renamed_confirmed_and_stopped_again() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 2));
    let req = request(&world.project(), fake.account());
    let (result, steps) = run(&req, &until_confirmed());
    let renamed = result.expect("renamed");
    assert_eq!(
        renamed.notes,
        [format!(
            "note: woke session {ID} with its saved options (--name, --model)."
        )]
    );
    assert_eq!(
        steps,
        [
            RenameStep::Waking {
                account: "main".into()
            },
            RenameStep::Confirming,
            RenameStep::Stopping {
                account: "main".into()
            },
        ]
    );
    // Wake, three listings (two with the old name), stop: all with the
    // owner's command.
    let calls = fake.all_argv();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert_eq!(
        calls[0],
        strings(&["--resume", SID, "--bg", "/rename tidy-config"])
    );
    assert_eq!(fake.cwd_of(1), canon(&world.project()));
    for listing in &calls[1..4] {
        assert_eq!(*listing, strings(&["agents", "--json", "--all"]));
    }
    assert_eq!(calls[4], strings(&["stop", ID]));
}

#[test]
fn a_copy_is_reported_and_nothing_is_stopped() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("copy", 0));
    let (result, steps) = run(
        &request(&world.project(), fake.account()),
        &until_confirmed(),
    );
    match result {
        Err(RenameError::Copied { new_id, notes }) => {
            assert_eq!(new_id, COPY);
            assert!(notes[0].contains("started a copy"), "{notes:?}");
        }
        other => panic!("expected a copy, got {other:?}"),
    }
    assert_eq!(steps.len(), 1, "only the wake");
    assert_eq!(fake.calls(), 1, "no listing, no stop, no rm");
}

#[test]
fn a_name_that_never_shows_is_not_confirmed_and_the_session_is_left_running() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", u32::MAX));
    let (result, _) = run(&request(&world.project(), fake.account()), &gives_up_soon());
    match result {
        Err(err @ RenameError::NotConfirmed { .. }) => {
            assert!(err.left_running());
            let RenameError::NotConfirmed { last_name, .. } = err else {
                unreachable!()
            };
            assert_eq!(last_name.as_deref(), Some("touch-config"));
        }
        other => panic!("expected NotConfirmed, got {other:?}"),
    }
    assert!(
        fake.all_argv().iter().all(|a| a[0] != "stop"),
        "not stopped"
    );
}

#[test]
fn an_untrusted_folder_wakes_nothing() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("untrusted", 0));
    let (result, _) = run(
        &request(&world.project(), fake.account()),
        &until_confirmed(),
    );
    match result {
        Err(RenameError::Untrusted { dir, message }) => {
            assert_eq!(dir, world.project());
            assert!(message.starts_with("Workspace not trusted"), "{message}");
        }
        other => panic!("expected Untrusted, got {other:?}"),
    }
    assert_eq!(fake.calls(), 1);
}

#[test]
fn a_missing_folder_or_a_bad_name_runs_nothing() {
    let _l = lock();
    let world = World::new();
    let fake = Fake::new("claude", &world.owner_body("woke", 0));
    let (result, steps) = run(
        &request(Path::new("/nonexistent/demo-app"), fake.account()),
        &until_confirmed(),
    );
    assert!(
        matches!(result, Err(RenameError::CwdMissing(_))),
        "{result:?}"
    );
    assert!(steps.is_empty());
    let mut req = request(&world.project(), fake.account());
    req.new_name = "touch-config".into();
    assert!(matches!(
        run(&req, &until_confirmed()).0,
        Err(RenameError::Invalid(_))
    ));
    req.new_name = "a\nb".into();
    assert!(matches!(
        run(&req, &until_confirmed()).0,
        Err(RenameError::Invalid(_))
    ));
    assert_eq!(fake.calls(), 0);
}

/// As for a hand-off (doc/0017 section 8): when the state file records the
/// session under another account than the owner this tower wakes it with,
/// nothing is woken; the steps and the result come back through the jobs.
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
        .with_rename_timing(until_confirmed())
        .with_state_file(path.clone());
    let req = request(&world.project(), fake.account());
    let run = |recorded| {
        jobs.rename(RenameJob {
            request: req.clone(),
            after_trust: false,
            recorded,
        });
        let mut steps = 0;
        loop {
            match jobs.recv_timeout(Duration::from_secs(20)).expect("reports") {
                JobResult::Rename(job, result) => {
                    assert_eq!(job.request, req);
                    return (steps, result);
                }
                JobResult::RenameStep(_) => steps += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let (steps, result) = run(recorded.clone());
    assert_eq!(steps, 3);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(fake.calls(), 3, "wake, one listing, stop");

    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner(SID, "alt", AccountSource::Handoff);
    other.save(&path).unwrap();
    match run(recorded).1 {
        Err(RenameError::OwnerChanged { account }) => assert_eq!(account, "alt"),
        other => panic!("expected OwnerChanged, got {other:?}"),
    }
    assert_eq!(fake.calls(), 3, "nothing more was run");
}
