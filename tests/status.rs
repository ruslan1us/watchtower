//! `tower status --json` (doc/0010), run as the real binary against fake
//! `claude` and `claude-alt` scripts on `PATH` serving the fixture JSON.
//! Every run has stdin null and stdout and stderr piped: no terminal.
//!
//! Fixture pids are replaced with values above Linux's largest possible
//! pid, so `/proc` of the machine running the tests is never evidence.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use fake_claude::{Fake, fixture_path, lock};
use serde_json::{Value, json};

/// No Linux pid reaches this (`PID_MAX_LIMIT`).
const NO_PID: u64 = 4_194_304;

/// A temp home with `~/.config/watchtower/`, plus the fakes.
struct Setup {
    home: tempfile::TempDir,
    fakes: Vec<Fake>,
}

impl Setup {
    /// Accounts `main` (`claude`) and `alt` (`claude-alt`), each fake
    /// running its body.
    fn two(main_body: &str, alt_body: &str) -> Setup {
        let setup = Setup {
            home: tempfile::tempdir().unwrap(),
            fakes: vec![
                Fake::new("claude", main_body),
                Fake::new("claude-alt", alt_body),
            ],
        };
        std::fs::create_dir_all(setup.config_dir()).unwrap();
        setup.write_config(
            "default_account = \"main\"\n\
             [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
             [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\nconfig_dir = \"~/.claude-alt\"\n",
        );
        setup
    }

    fn config_dir(&self) -> PathBuf {
        self.home.path().join(".config/watchtower")
    }

    fn state_file(&self) -> PathBuf {
        self.config_dir().join("state.json")
    }

    fn write_config(&self, text: &str) {
        std::fs::write(self.config_dir().join("config.toml"), text).unwrap();
    }

    /// A file in the home dir holding `rows` as JSON; returns a snippet
    /// that prints it.
    fn serve(&self, file: &str, rows: &[Value]) -> String {
        let path = self.home.path().join(file);
        std::fs::write(&path, serde_json::to_string_pretty(rows).unwrap()).unwrap();
        format!("cat '{}'", path.display())
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut path: Vec<String> = self
            .fakes
            .iter()
            .map(|f| f.dir.path().display().to_string())
            .collect();
        path.push("/usr/bin:/bin".to_owned());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tower"));
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("PATH", path.join(":"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

/// The rows of an `agents-json/` fixture, with impossible pids.
fn rows(name: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(fixture_path(&format!("agents-json/{name}"))).unwrap();
    let mut rows: Vec<Value> = serde_json::from_str(&text).unwrap();
    for (i, row) in rows.iter_mut().enumerate() {
        if row.get("pid").is_some() {
            row["pid"] = json!(NO_PID + i as u64);
        }
    }
    rows
}

fn parse(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn sessions(report: &Value) -> &Vec<Value> {
    report["sessions"].as_array().expect("sessions is an array")
}

fn by_name<'a>(report: &'a Value, name: &str) -> &'a Value {
    sessions(report)
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no row named {name}"))
}

/// A setup whose fakes are not written yet: the bodies need the home dir.
fn split_setup() -> (Setup, Vec<Value>) {
    let all = rows("mixed-states.json");
    let setup = Setup::two("true", "true");
    let main = setup.serve("main.json", &all[..3]);
    let alt = setup.serve("alt.json", &all[3..]);
    let setup = Setup {
        fakes: vec![Fake::new("claude", &main), Fake::new("claude-alt", &alt)],
        ..setup
    };
    (setup, all)
}

/// Both accounts list the same rows (shared session storage, doc/0003).
fn shared_setup(fixture: &str) -> Setup {
    let all = rows(fixture);
    let setup = Setup::two("true", "true");
    let body = setup.serve("shared.json", &all);
    Setup {
        fakes: vec![Fake::new("claude", &body), Fake::new("claude-alt", &body)],
        ..setup
    }
}

#[test]
fn schema_fields_and_types() {
    let _guard = lock();
    let (setup, _) = split_setup();
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    let top: Vec<&String> = report.as_object().unwrap().keys().collect();
    assert_eq!(top, ["config", "errors", "sessions"]);
    assert_eq!(report["errors"], json!([]));
    let config_file = setup.config_dir().join("config.toml");
    assert_eq!(
        report["config"],
        json!({ "path": config_file.to_str().unwrap(), "found": true })
    );
    let strings = ["cwd", "kind", "account", "group", "detail"];
    let nullable_strings = [
        "id",
        "sessionId",
        "name",
        "state",
        "status",
        "waitingFor",
        "accountCommand",
    ];
    let nullable_numbers = ["pid", "startedAt", "lastChangeAt"];
    let groups = [
        "needs-me",
        "failed",
        "ready-for-review",
        "working",
        "other",
        "stopped",
        "interactive",
    ];
    assert!(!sessions(&report).is_empty());
    for row in sessions(&report) {
        let obj = row.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = strings
            .iter()
            .chain(&nullable_strings)
            .chain(&nullable_numbers)
            .chain(&["subagents"])
            .copied()
            .collect();
        keys.sort_unstable();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{row}");
        for k in strings {
            assert!(row[k].is_string(), "{k} in {row}");
        }
        for k in nullable_strings {
            assert!(row[k].is_string() || row[k].is_null(), "{k} in {row}");
        }
        for k in nullable_numbers {
            assert!(row[k].is_i64() || row[k].is_null(), "{k} in {row}");
        }
        assert!(groups.contains(&row["group"].as_str().unwrap()), "{row}");
        // No hook files in this home: always an empty array (doc/0029).
        assert_eq!(row["subagents"], json!([]), "{row}");
    }
    // Listing fields keep their names and values.
    let pick = by_name(&report, "pick-colour");
    assert_eq!(pick["id"], "9c6e1a35");
    assert_eq!(pick["sessionId"], "9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20");
    assert_eq!(pick["kind"], "background");
    assert_eq!(pick["state"], "blocked");
    assert_eq!(pick["status"], "waiting");
    assert_eq!(pick["waitingFor"], "input needed");
    assert_eq!(pick["group"], "needs-me");
    assert_eq!(pick["detail"], "input needed");
    // No tracked time: falls back to `startedAt` (doc/0006).
    assert_eq!(pick["lastChangeAt"], pick["startedAt"]);
    let interactive = sessions(&report)
        .iter()
        .find(|r| r["kind"] == "interactive")
        .unwrap();
    assert_eq!(interactive["id"], Value::Null);
    assert_eq!(interactive["state"], Value::Null);
    assert_eq!(interactive["group"], "interactive");
}

#[test]
fn merged_across_two_accounts() {
    let _guard = lock();
    let (setup, all) = split_setup();
    let report = parse(&setup.run(&["status", "--json"]));
    assert_eq!(sessions(&report).len(), all.len());
    // Separate, complete, non-empty lists from one poll: rule 2 (doc/0017).
    for (i, row) in all.iter().enumerate() {
        let got = sessions(&report)
            .iter()
            .find(|r| r["sessionId"] == row["sessionId"])
            .unwrap();
        let (account, command) = if i < 3 {
            ("main", "claude")
        } else {
            ("alt", "claude-alt")
        };
        assert_eq!(got["account"], account, "{got}");
        assert_eq!(got["accountCommand"], command, "{got}");
    }
    // Each account was listed once, with the listing arguments.
    for fake in &setup.fakes {
        assert_eq!(fake.calls(), 1);
        assert_eq!(fake.argv(), ["agents", "--json", "--all"]);
    }
}

#[test]
fn shared_lists_deduplicate_and_unknown_owner_is_null() {
    let _guard = lock();
    let setup = shared_setup("after-stop-all.json");
    let report = parse(&setup.run(&["status", "--json"]));
    assert_eq!(sessions(&report).len(), rows("after-stop-all.json").len());
    for row in sessions(&report) {
        assert_eq!(row["account"], "?", "{row}");
        assert_eq!(row["accountCommand"], Value::Null, "{row}");
    }
    let stopped = by_name(&report, "say-ok");
    assert_eq!(stopped["group"], "stopped");
    assert_eq!(stopped["pid"], Value::Null);
}

#[test]
fn sidecar_owner_and_tracked_time_are_used_read_only() {
    let _guard = lock();
    let setup = shared_setup("mixed-states.json");
    let sidecar = json!({
        "version": 1,
        "sessions": {
            "9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20": {
                "account": "alt",
                "account_source": "started",
                "last_state": { "state": "blocked", "status": "waiting", "waitingFor": "input needed" },
                "last_change_ms": 1767232999000_i64,
                "updated_ms": 1767232999000_i64
            }
        },
        "recent_projects": []
    });
    let text = serde_json::to_string_pretty(&sidecar).unwrap();
    std::fs::write(setup.state_file(), &text).unwrap();
    let before = std::fs::metadata(setup.state_file()).unwrap();
    let listing_before = dir_listing(&setup.config_dir());

    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    let pick = by_name(&report, "pick-colour");
    assert_eq!(pick["account"], "alt");
    assert_eq!(pick["accountCommand"], "claude-alt");
    assert_eq!(pick["lastChangeAt"], 1767232999000_i64);
    // No record: unknown with shared lists, and nothing is written for it.
    assert_eq!(by_name(&report, "print-date")["account"], "?");

    let after = std::fs::metadata(setup.state_file()).unwrap();
    assert_eq!(std::fs::read_to_string(setup.state_file()).unwrap(), text);
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    // No lock, temp or corrupt file appears next to it.
    assert_eq!(dir_listing(&setup.config_dir()), listing_before);
}

#[test]
fn corrupt_sidecar_is_ignored_and_left_in_place() {
    let _guard = lock();
    let (setup, _) = split_setup();
    std::fs::write(setup.state_file(), "{ not json").unwrap();
    let listing_before = dir_listing(&setup.config_dir());
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!sessions(&parse(&out)).is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("corrupt"));
    assert_eq!(
        std::fs::read_to_string(setup.state_file()).unwrap(),
        "{ not json"
    );
    assert_eq!(dir_listing(&setup.config_dir()), listing_before);
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn one_failing_account_is_reported_without_failing() {
    let _guard = lock();
    let all = rows("mixed-states.json");
    let setup = Setup::two("true", "true");
    let main = setup.serve("main.json", &all);
    let setup = Setup {
        fakes: vec![
            Fake::new("claude", &main),
            Fake::new("claude-alt", "echo 'not logged in' >&2; exit 3"),
        ],
        ..setup
    };
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    assert_eq!(sessions(&report).len(), all.len());
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["account"], "alt");
    assert_eq!(errors[0]["accountCommand"], "claude-alt");
    let message = errors[0]["error"].as_str().unwrap();
    assert!(message.contains("not logged in"), "{message}");
    assert!(message.contains("exit 3"), "{message}");
}

#[test]
fn all_accounts_failing_exits_1() {
    let _guard = lock();
    let setup = Setup::two("exit 1", "echo '[not json'");
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let report = parse(&out);
    assert_eq!(report["sessions"], json!([]));
    let errors = report["errors"].as_array().unwrap();
    let names: Vec<&str> = errors
        .iter()
        .map(|e| e["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main", "alt"]);
}

#[test]
fn config_error_exits_2() {
    let _guard = lock();
    let setup = Setup::two("true", "true");
    setup.write_config("[[accounts]]\nname = \"main\"\n"); // no command
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
    for fake in &setup.fakes {
        assert_eq!(fake.calls(), 0);
    }

    // Two accounts on one config dir are a config error too (doc/0017).
    setup.write_config(
        "[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
         [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n",
    );
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn polls_run_in_parallel_without_claude_config_dir() {
    let _guard = lock();
    // Each fake waits for the other's marker: run one after the other, the
    // first would give up and fail.
    let setup = Setup::two("true", "true");
    let marks = setup.home.path().join("marks");
    std::fs::create_dir(&marks).unwrap();
    let all = rows("mixed-states.json");
    let wait_for = |me: &str, other: &str, serve: String| {
        format!(
            "touch '{m}/{me}'; i=0; while [ ! -e '{m}/{other}' ]; do \
             i=$((i+1)); [ $i -gt 200 ] && exit 1; sleep 0.05; done; {serve}",
            m = marks.display()
        )
    };
    let main = wait_for("main", "alt", setup.serve("main.json", &all[..3]));
    let alt = wait_for("alt", "main", setup.serve("alt.json", &all[3..]));
    let setup = Setup {
        fakes: vec![Fake::new("claude", &main), Fake::new("claude-alt", &alt)],
        ..setup
    };
    let out = setup
        .command(&["status", "--json"])
        .env("CLAUDE_CONFIG_DIR", "/elsewhere/.claude")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(parse(&out)["errors"], json!([]));
    for fake in &setup.fakes {
        assert_eq!(fake.env_var_of(1, "CLAUDE_CONFIG_DIR"), None);
    }
    // Tower says it removed the variable.
    assert!(String::from_utf8_lossy(&out.stderr).contains("CLAUDE_CONFIG_DIR"));
}

#[test]
fn works_without_a_terminal() {
    let _guard = lock();
    let (setup, _) = split_setup();
    // stdin, stdout and stderr are all non-TTYs: null, a pipe, a file.
    let err_file = setup.home.path().join("stderr.txt");
    let out = setup
        .command(&["status", "--json"])
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(&err_file).unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    // Plain JSON: no escape sequence from a terminal set-up.
    assert!(!out.stdout.contains(&0x1b));
    assert!(out.stdout.ends_with(b"}\n"));
    parse(&out);
    assert!(std::fs::read(&err_file).unwrap().is_empty());
}

#[test]
fn table_without_json() {
    let _guard = lock();
    let (setup, _) = split_setup();
    let out = setup.run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines();
    let config_file = setup.config_dir().join("config.toml");
    assert_eq!(
        lines.next().unwrap(),
        format!("config: {}", config_file.display())
    );
    let header = lines.next().unwrap();
    assert!(header.starts_with("NAME"), "{header}");
    let pick = text.lines().find(|l| l.starts_with("pick-colour")).unwrap();
    assert!(pick.contains("input needed"), "{pick}");
    assert!(pick.contains("alt"), "{pick}");
    assert!(pick.contains("/home/user/workspace/demo-app"), "{pick}");
}

#[test]
fn help_mentions_status() {
    let _guard = lock();
    let setup = Setup::two("true", "true");
    let out = setup.run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("status --json"));
    let out = setup.run(&["status", "--yaml"]);
    assert_eq!(out.status.code(), Some(2));
    for fake in &setup.fakes {
        assert_eq!(fake.calls(), 0);
    }
}

#[test]
fn missing_config_is_reported_in_json_table_and_help() {
    let _guard = lock();
    let setup = Setup::two("echo '[]'", "echo '[]'");
    let config_file = setup.config_dir().join("config.toml");
    let out = setup.run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains(&format!("Config in use: {} (found)", config_file.display())),
        "{help}"
    );

    std::fs::remove_file(&config_file).unwrap();
    // The defaults: one account `main` running `claude`.
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    assert_eq!(
        report["config"],
        json!({ "path": config_file.to_str().unwrap(), "found": false })
    );
    assert_eq!(report["sessions"], json!([]));

    let out = setup.run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        text.lines().next().unwrap(),
        format!(
            "config: {} (not found — using defaults)",
            config_file.display()
        )
    );

    let out = setup.run(&["--help"]);
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains(&format!(
            "Config in use: {} (not found — built-in defaults apply",
            config_file.display()
        )),
        "{help}"
    );
}

#[test]
fn last_change_at_matches_the_tuis_tracking() {
    use watchtower::app::{App, SidecarJob};
    use watchtower::config::Config;
    use watchtower::listing::parse_listing;
    use watchtower::poller::PollUpdate;
    use watchtower::sidecar::{LOCK_WAIT, Sidecar};

    let _guard = lock();
    let setup = shared_setup("mixed-states.json");
    let config_text = std::fs::read_to_string(setup.config_dir().join("config.toml")).unwrap();
    let home = setup.home.path().to_path_buf();
    let config = Config::parse(&config_text, &home).unwrap();
    let mut app = App::new(
        config,
        home.clone(),
        Sidecar::default(),
        Box::new(common::NoEnviron),
    );
    let feed = |app: &mut App, cycle: u64, rows: &[Value]| {
        for account in ["main", "alt"] {
            app.apply_poll(PollUpdate {
                account: account.into(),
                cycle,
                result: Ok(parse_listing(&serde_json::to_string(rows).unwrap()).unwrap()),
            });
        }
    };
    // First sighting, then one row changes state (tower tracks the time).
    let mut list = rows("mixed-states.json");
    feed(&mut app, 1, &list);
    let changed = list
        .iter_mut()
        .find(|r| r["name"] == "pick-colour")
        .unwrap();
    changed["state"] = json!("working");
    changed["status"] = json!("busy");
    changed.as_object_mut().unwrap().remove("waitingFor");
    feed(&mut app, 2, &list);
    let Some(SidecarJob::Save { seq, job }) = app.start_save() else {
        panic!("a save expected");
    };
    let result = job.run(&setup.state_file(), LOCK_WAIT);
    app.sidecar_saved(seq, &job, result);
    assert!(!app.sidecar.is_dirty());
    // The fakes now list the changed rows, like the TUI's last poll.
    setup.serve("shared.json", &list);

    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    assert_eq!(sessions(&report).len(), app.sessions().len());
    let now = watchtower::sidecar::now_ms();
    for m in app.sessions() {
        let sid = m.session.session_id.as_deref().unwrap();
        let row = sessions(&report)
            .iter()
            .find(|r| r["sessionId"] == sid)
            .unwrap_or_else(|| panic!("{sid} missing"));
        let tui = app.last_change_at(m, now);
        assert_eq!(row["lastChangeAt"], json!(tui), "{row}");
    }
    let pick = by_name(&report, "pick-colour");
    assert_ne!(
        pick["lastChangeAt"], pick["startedAt"],
        "the tracked change"
    );
    let print = by_name(&report, "print-date");
    assert_eq!(print["lastChangeAt"], print["startedAt"], "first sighting");
}

#[test]
fn the_orchestrator_and_a_question_turn_in_json_and_table() {
    let _guard = lock();
    // Both end a turn with a question: `blocked`, `idle`, no `waitingFor`.
    let row = |id: &str, name: &str, pid: u64| {
        json!({
            "pid": pid, "id": id, "sessionId": format!("{id}-0000-4000-8000-000000000000"),
            "cwd": "/home/user/workspace/example-repo", "kind": "background",
            "startedAt": 1700000000000_i64, "name": name, "status": "idle", "state": "blocked"
        })
    };
    let rows = vec![
        row("0c0c0c0c", "orchestrator", NO_PID),
        row("1a2b3c4d", "ask-question", NO_PID + 1),
    ];
    let setup = Setup::two("true", "true");
    let body = setup.serve("main.json", &rows);
    let empty = setup.serve("alt.json", &[]);
    let setup = Setup {
        fakes: vec![Fake::new("claude", &body), Fake::new("claude-alt", &empty)],
        ..setup
    };
    let out = setup.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = parse(&out);
    let orch = by_name(&report, "orchestrator");
    assert_eq!(orch["group"], "orchestrator");
    assert_eq!(orch["detail"], "ready");
    let asking = by_name(&report, "ask-question");
    assert_eq!(asking["group"], "needs-me");
    assert_eq!(asking["detail"], "waiting for your reply");

    let out = setup.run(&["status"]);
    let text = String::from_utf8(out.stdout).unwrap();
    let line = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .unwrap()
            .to_owned()
    };
    assert!(
        line("ask-question").contains("waiting for your reply"),
        "{text}"
    );
    assert!(line("orchestrator").contains("ready"), "{text}");
}

/// `subagents` (doc/0029): the running agents from tower's own files, for
/// rows that are live and `working` only; read-only; none when
/// `[sessions] track_subagents = false`.
#[test]
fn subagents_of_working_rows_only() {
    let _guard = lock();
    let (setup, _) = split_setup();
    let dir = setup.config_dir().join("subagents");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({
        "version": 1,
        "agents": [
            { "id": "bb22", "type": "general-purpose", "description": "beta waiter",
              "started_ms": 1_790_000_000_500_i64, "status": "running", "background": true },
            { "id": "aa11", "type": "general-purpose", "description": "alpha waiter",
              "started_ms": 1_790_000_000_000_i64, "status": "running", "background": true }
        ],
        "stopped": []
    });
    let working = "5a7c2e91-0d4b-4c8e-8f21-6b3a9d0e7c12";
    let done = "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93";
    for sid in [working, done] {
        std::fs::write(dir.join(format!("{sid}.json")), file.to_string()).unwrap();
    }
    let report = parse(&setup.run(&["status", "--json"]));
    assert_eq!(
        by_name(&report, "run-test-suite")["subagents"],
        json!([
            { "id": "aa11", "type": "general-purpose", "description": "alpha waiter",
              "startedAt": 1_790_000_000_000_i64 },
            { "id": "bb22", "type": "general-purpose", "description": "beta waiter",
              "startedAt": 1_790_000_000_500_i64 }
        ])
    );
    assert_eq!(
        by_name(&report, "print-date")["subagents"],
        json!([]),
        "done"
    );
    assert_eq!(by_name(&report, "pick-colour")["subagents"], json!([]));
    // Read only: nothing pruned or rewritten.
    for sid in [working, done] {
        let text = std::fs::read_to_string(dir.join(format!("{sid}.json"))).unwrap();
        assert_eq!(text, file.to_string());
    }

    setup.write_config(
        "default_account = \"main\"\n\
         [sessions]\ntrack_subagents = false\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
         [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\nconfig_dir = \"~/.claude-alt\"\n",
    );
    let report = parse(&setup.run(&["status", "--json"]));
    assert_eq!(by_name(&report, "run-test-suite")["subagents"], json!([]));
}
