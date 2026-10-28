//! `tower dispatch` and `tower wake` (doc/0044), run as the real binary
//! against fake `claude` and `claude-alt` scripts on `PATH`. Each fake
//! answers by its first argument: `agents` prints a listing, `--bg` and
//! `--resume` print captured `backgrounded · …` output. Every run has stdin
//! null and stdout and stderr piped: no terminal.
//!
//! Live rows carry a pid above Linux's largest possible pid, so `/proc` of
//! the machine running the tests is never evidence. Nothing waits on a
//! clock beyond the dispatch's short `sessionId` lookups.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use fake_claude::{Fake, fixture_path, lock};
use serde_json::{Value, json};

/// No Linux pid reaches this (`PID_MAX_LIMIT`).
const NO_PID: u64 = 4_194_304;

/// The short id and name in `cli-output/bg-success.stdout`.
const BG_ID: &str = "3e9a1c07";
const BG_SID: &str = "3e9a1c07-0000-4000-8000-000000000000";

/// The short id in `cli-output/resume-*.stdout`.
const WAKE_ID: &str = "d4a1f7c8";
const WAKE_SID: &str = "d4a1f7c8-0000-4000-8000-000000000000";

const TWO_ACCOUNTS: &str = "default_account = \"main\"\n\
     [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
     [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\nconfig_dir = \"~/.claude-alt\"\n";

/// A temp home with `~/.config/watchtower/` and a project folder
/// `~/workspace/demo`, plus the fakes once written.
struct Setup {
    home: tempfile::TempDir,
    fakes: Vec<Fake>,
}

impl Setup {
    fn new(config: &str) -> Setup {
        let setup = Setup {
            home: tempfile::tempdir().unwrap(),
            fakes: Vec::new(),
        };
        std::fs::create_dir_all(setup.config_dir()).unwrap();
        std::fs::create_dir_all(setup.project()).unwrap();
        std::fs::write(setup.config_dir().join("config.toml"), config).unwrap();
        setup
    }

    /// Writes the fakes `claude` and `claude-alt` with these bodies.
    fn with_fakes(mut self, main: &str, alt: &str) -> Setup {
        self.fakes = vec![Fake::new("claude", main), Fake::new("claude-alt", alt)];
        self
    }

    fn main(&self) -> &Fake {
        &self.fakes[0]
    }

    fn alt(&self) -> &Fake {
        &self.fakes[1]
    }

    fn config_dir(&self) -> PathBuf {
        self.home.path().join(".config/watchtower")
    }

    fn state_file(&self) -> PathBuf {
        self.config_dir().join("state.json")
    }

    fn project(&self) -> PathBuf {
        self.home.path().join("workspace/demo")
    }

    /// A file in the home dir holding `value` as JSON; returns its path.
    fn file(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.home.path().join(name);
        std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
        path
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.state_file()).unwrap()).unwrap()
    }

    fn write_state(&self, value: &Value) {
        std::fs::write(self.state_file(), serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut path: Vec<String> = self
            .fakes
            .iter()
            .map(|f| f.dir.path().display().to_string())
            .collect();
        path.push("/usr/bin:/bin".to_owned());
        Command::new(env!("CARGO_BIN_EXE_tower"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("PATH", path.join(":"))
            .current_dir(self.home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Every call's argv of `fake`, in order.
fn calls(fake: &Fake) -> Vec<Vec<String>> {
    fake.all_argv()
}

/// Calls of `fake` whose first argument is `first`.
fn calls_of(fake: &Fake, first: &str) -> Vec<Vec<String>> {
    calls(fake)
        .into_iter()
        .filter(|a| a.first().map(String::as_str) == Some(first))
        .collect()
}

/// A fake body: `agents` prints `listing` (a file), `--bg` and `--resume`
/// run `start`.
fn body(listing: &Path, start: &str) -> String {
    format!(
        "case \"$1\" in\n\
         agents) cat '{}' ;;\n\
         --bg|--resume) {start} ;;\n\
         esac",
        listing.display()
    )
}

fn cat(rel: &str) -> String {
    format!("cat '{}'", fixture_path(rel).display())
}

fn cat_err(rel: &str) -> String {
    format!("cat '{}' >&2", fixture_path(rel).display())
}

// ---- tower dispatch --------------------------------------------------------

/// A dispatch setup: `main` starts `bad-model` (`3e9a1c07`) and lists it
/// with its sessionId afterwards; `alt` does the same.
fn dispatch_setup(config: &str) -> Setup {
    let setup = Setup::new(config);
    let row = json!([{
        "id": BG_ID, "sessionId": BG_SID, "name": "bad-model",
        "cwd": setup.project(), "kind": "background", "state": "working",
        "status": "busy", "pid": NO_PID, "startedAt": 1_700_000_000_000_i64
    }]);
    let listing = setup.file("listing.json", &row);
    let start = format!(
        "{}; {}",
        cat("cli-output/bg-success.stdout"),
        "echo 'Starting background service…' >&2"
    );
    let b = body(&listing, &start);
    setup.with_fakes(&b, &b)
}

fn settings_path(setup: &Setup) -> String {
    setup
        .config_dir()
        .join("claude-settings.json")
        .display()
        .to_string()
}

#[test]
fn dispatch_runs_the_default_account_with_the_config_defaults() {
    let _g = lock();
    let setup = dispatch_setup(&format!(
        "default_model = \"sonnet\"\ndefault_effort = \"high\"\n\
         default_permission_mode = \"acceptEdits\"\n{TWO_ACCOUNTS}"
    ));
    let project = setup.project();
    let out = setup.run(&[
        "dispatch",
        "--project",
        project.to_str().unwrap(),
        "fix",
        "the",
        "flaky",
        "test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("started {BG_ID} · bad-model under main\n")
    );
    let bg = calls_of(setup.main(), "--bg");
    assert_eq!(
        bg,
        [strings(&[
            "--bg",
            "--model",
            "sonnet",
            "--effort",
            "high",
            "--permission-mode",
            "acceptEdits",
            "--settings",
            &settings_path(&setup),
            "fix the flaky test",
        ])]
    );
    assert_eq!(
        setup.main().cwd_of(1),
        std::fs::canonicalize(&project).unwrap()
    );
    assert!(calls(setup.alt()).is_empty(), "alt never runs");
    // The settings file holds tower's hooks (doc/0029).
    let settings = std::fs::read_to_string(settings_path(&setup)).unwrap();
    assert!(settings.contains("SubagentStart"), "{settings}");
}

#[test]
fn dispatch_passes_each_option_as_the_dialog_does() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    let project = setup.project();
    let out = setup.run(&[
        "dispatch",
        "--project",
        project.to_str().unwrap(),
        "--account",
        "alt",
        "--name=schema-migration",
        "--model",
        "opus",
        "--effort",
        "max",
        "--mode",
        "plan",
        "--",
        "migrate the schema",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(calls(setup.main()).is_empty(), "main never runs");
    assert_eq!(
        calls_of(setup.alt(), "--bg"),
        [strings(&[
            "--bg",
            "--name",
            "schema-migration",
            "--model",
            "opus",
            "--effort",
            "max",
            "--permission-mode",
            "plan",
            "--settings",
            &settings_path(&setup),
            "migrate the schema",
        ])]
    );
    assert_eq!(
        stdout(&out),
        format!("started {BG_ID} · bad-model under alt\n")
    );
}

#[test]
fn dispatch_without_subagent_tracking_passes_no_settings() {
    let _g = lock();
    let setup = dispatch_setup(&format!(
        "{TWO_ACCOUNTS}\n[sessions]\ntrack_subagents = false\n"
    ));
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "write the docs",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        calls_of(setup.main(), "--bg"),
        [strings(&[
            "--bg",
            "--permission-mode",
            "auto",
            "write the docs"
        ])]
    );
    assert!(!setup.config_dir().join("claude-settings.json").exists());
}

#[test]
fn dispatch_passes_a_task_starting_with_a_dash_after_end_of_options() {
    let _g = lock();
    let setup = dispatch_setup(&format!(
        "{TWO_ACCOUNTS}\n[sessions]\ntrack_subagents = false\n"
    ));
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "-v: list the failing tests",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        calls_of(setup.main(), "--bg"),
        [strings(&[
            "--bg",
            "--permission-mode",
            "auto",
            "--",
            "-v: list the failing tests"
        ])]
    );
    // Without `--`, a dash word is an unknown option and nothing runs.
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "-v",
        "list",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("goes after `--`"), "{}", stderr(&out));
    assert_eq!(calls_of(setup.main(), "--bg").len(), 1);
}

#[test]
fn dispatch_records_the_owner_and_the_recent_project() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    let out = setup.run(&[
        "dispatch",
        "--project",
        "~/workspace/demo",
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let state = setup.state();
    // The listing showed the sessionId, so the record is keyed by it.
    assert_eq!(state["sessions"][BG_SID]["account"], "main");
    assert_eq!(state["sessions"][BG_SID]["account_source"], "started");
    assert!(state["sessions"].get(BG_ID).is_none(), "{state}");
    assert_eq!(state["recent_projects"][0]["path"], "~/workspace/demo");
    assert!(
        state["recent_projects"][0]["last_used_ms"]
            .as_i64()
            .unwrap()
            > 0
    );
}

#[test]
fn dispatch_keys_the_record_by_the_short_id_when_no_listing_shows_it() {
    let _g = lock();
    let setup = Setup::new(TWO_ACCOUNTS);
    let empty = setup.file("empty.json", &json!([]));
    let b = body(&empty, &cat("cli-output/bg-success.stdout"));
    let setup = setup.with_fakes(&b, &b);
    let out = setup.run(&[
        "dispatch",
        "--json",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({ "id": BG_ID, "sessionId": null, "name": "bad-model", "account": "main" })
    );
    let state = setup.state();
    assert_eq!(state["sessions"][BG_ID]["account"], "main");
    assert_eq!(state["sessions"][BG_ID]["account_source"], "started");
}

#[test]
fn dispatch_json_prints_id_session_id_name_and_account() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--account",
        "alt",
        "--json",
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out).lines().count(), 1, "{}", stdout(&out));
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        printed,
        json!({ "id": BG_ID, "sessionId": BG_SID, "name": "bad-model", "account": "alt" })
    );
    assert_eq!(setup.state()["sessions"][BG_SID]["account"], "alt");
}

#[test]
fn dispatch_checks_the_folder_before_running_anything() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    let missing = setup.home.path().join("workspace/missing");
    let out = setup.run(&[
        "dispatch",
        "--project",
        missing.to_str().unwrap(),
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("does not exist"), "{}", stderr(&out));

    let file = setup.home.path().join("workspace/notes.txt");
    std::fs::write(&file, "x").unwrap();
    let out = setup.run(&[
        "dispatch",
        "--project",
        file.to_str().unwrap(),
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("is not a folder"), "{}", stderr(&out));

    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--account",
        "nobody",
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("no account named `nobody`; the configured accounts are main, alt"),
        "{}",
        stderr(&out)
    );

    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "ab",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("at least 4 characters"),
        "{}",
        stderr(&out)
    );

    assert!(calls(setup.main()).is_empty());
    assert!(calls(setup.alt()).is_empty());
    assert!(!setup.state_file().exists());
}

#[test]
fn dispatch_in_an_untrusted_folder_exits_3_and_says_how_to_trust_it() {
    let _g = lock();
    let setup = Setup::new(TWO_ACCOUNTS);
    let empty = setup.file("empty.json", &json!([]));
    let b = body(
        &empty,
        &format!("{}; exit 1", cat_err("cli-output/bg-untrusted.stderr")),
    );
    let setup = setup.with_fakes(&b, &b);
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Workspace not trusted"), "{err}");
    assert!(
        err.contains("main has not trusted") && err.contains("(`n`, then `Ctrl+T`"),
        "{err}"
    );
    assert!(stdout(&out).is_empty());
    // Exactly one `--bg`: no trust flow, no retry.
    assert_eq!(calls_of(setup.main(), "--bg").len(), 1);
    assert!(!setup.state_file().exists());
}

#[test]
fn dispatch_failure_exits_1_with_claudes_message() {
    let _g = lock();
    let setup = Setup::new(TWO_ACCOUNTS);
    let empty = setup.file("empty.json", &json!([]));
    let b = body(&empty, "echo 'something broke' >&2; exit 7");
    let setup = setup.with_fakes(&b, &b);
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("claude --bg failed (exit 7): something broke"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn dispatch_warns_about_haiku_with_auto_mode() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    let out = setup.run(&[
        "dispatch",
        "--project",
        setup.project().to_str().unwrap(),
        "--model",
        "haiku",
        "--",
        "fix the flaky test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("warning: auto mode is unavailable on Haiku 4.5"),
        "{}",
        stderr(&out)
    );
    // The model is passed anyway, as the dialog does.
    assert!(calls_of(setup.main(), "--bg")[0].contains(&"haiku".to_owned()));
}

#[test]
fn dispatch_needs_a_project_and_a_task() {
    let _g = lock();
    let setup = dispatch_setup(TWO_ACCOUNTS);
    for args in [
        &["dispatch", "--", "fix the flaky test"][..],
        &["dispatch", "--project", "/tmp"],
        &["dispatch", "--project", "/tmp", "--"],
        &["dispatch", "--project"],
        &["dispatch", "--project", "/a", "--project", "/b", "task"],
    ] {
        let out = setup.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    assert!(calls(setup.main()).is_empty());
}

// ---- tower wake ------------------------------------------------------------

/// One listing row: the wake fixture's id, in the project folder.
fn row(setup: &Setup, live: Option<(&str, &str)>) -> Value {
    let mut row = json!({
        "id": WAKE_ID, "sessionId": WAKE_SID, "name": "touch-config",
        "cwd": setup.project(), "kind": "background", "state": "done",
        "startedAt": 1_700_000_000_000_i64
    });
    if let Some((state, status)) = live {
        row["pid"] = json!(NO_PID);
        row["state"] = json!(state);
        row["status"] = json!(status);
    }
    row
}

/// The state file records the wake fixture's session under `account`.
fn owned_by(setup: &Setup, account: &str, source: &str) {
    setup.write_state(&json!({
        "version": 1,
        "sessions": { WAKE_SID: {
            "account": account, "account_source": source, "updated_ms": 1
        } }
    }));
}

/// A wake setup: both accounts list `rows` (shared lists, doc/0003);
/// `--resume` prints `start`.
fn wake_setup(config: &str, rows: impl Fn(&Setup) -> Value, start: &str) -> Setup {
    let setup = Setup::new(config);
    let listing = setup.file("listing.json", &rows(&setup));
    let b = body(&listing, start);
    setup.with_fakes(&b, &b)
}

fn woke_prompt() -> String {
    format!(
        "{}; {}",
        cat("cli-output/resume-prompt.stdout"),
        cat_err("cli-output/resume-prompt.stderr")
    )
}

fn woke_idle() -> String {
    format!(
        "{}; {}",
        cat("cli-output/resume-noprompt.stdout"),
        cat_err("cli-output/resume-noprompt.stderr")
    )
}

#[test]
fn wake_without_a_text_resumes_with_the_owners_command() {
    let _g = lock();
    let setup = wake_setup(TWO_ACCOUNTS, |s| json!([row(s, None)]), &woke_idle());
    owned_by(&setup, "alt", "handoff");
    let out = setup.run(&["wake", WAKE_ID]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("woke {WAKE_ID} · touch-config under alt · waiting for your prompt\n")
    );
    assert!(calls_of(setup.main(), "--resume").is_empty());
    assert_eq!(
        calls_of(setup.alt(), "--resume"),
        [strings(&["--resume", WAKE_SID, "--bg"])]
    );
    let n = setup.alt().calls();
    assert_eq!(
        setup.alt().cwd_of(n),
        std::fs::canonicalize(setup.project()).unwrap()
    );
}

#[test]
fn wake_with_a_text_passes_it_after_end_of_options() {
    let _g = lock();
    let setup = wake_setup(TWO_ACCOUNTS, |s| json!([row(s, None)]), &woke_prompt());
    owned_by(&setup, "main", "started");
    // By name, with `--text`.
    let out = setup.run(&[
        "wake",
        "touch-config",
        "--text",
        "review the diff\nthen fix",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("woke {WAKE_ID} · touch-config under main · with your text as its next prompt\n")
    );
    // By sessionId, a text starting with `-` after `--`, words joined.
    let out = setup.run(&["wake", WAKE_SID, "--", "-v", "list", "--help"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // `--text` with a leading dash is a value too.
    let out = setup.run(&["wake", "--text=-x first", WAKE_ID]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        calls_of(setup.main(), "--resume"),
        [
            strings(&[
                "--resume",
                WAKE_SID,
                "--bg",
                "--",
                "review the diff\nthen fix"
            ]),
            strings(&["--resume", WAKE_SID, "--bg", "--", "-v list --help"]),
            strings(&["--resume", WAKE_SID, "--bg", "--", "-x first"]),
        ]
    );
    assert!(calls_of(setup.alt(), "--resume").is_empty());
}

#[test]
fn wake_refuses_a_live_session_with_the_reason() {
    let _g = lock();
    for (state, status, expected) in [
        ("done", "idle", "use the quick reply (`t`) in tower"),
        ("blocked", "idle", "use the quick reply (`t`) in tower"),
        ("working", "busy", "is running (working)"),
        (
            "blocked",
            "waiting",
            "only wakes sessions without a live process",
        ),
    ] {
        let setup = wake_setup(
            TWO_ACCOUNTS,
            |s| json!([row(s, Some((state, status)))]),
            &woke_prompt(),
        );
        owned_by(&setup, "main", "started");
        let out = setup.run(&["wake", WAKE_ID, "--text", "go on"]);
        assert_eq!(
            out.status.code(),
            Some(4),
            "{state}/{status}: {}",
            stderr(&out)
        );
        assert!(
            stderr(&out).contains(expected),
            "{state}/{status}: {}",
            stderr(&out)
        );
        assert!(calls_of(setup.main(), "--resume").is_empty());
        assert!(calls_of(setup.alt(), "--resume").is_empty());
    }
}

#[test]
fn wake_with_an_unknown_owner_needs_account() {
    let _g = lock();
    let setup = wake_setup(TWO_ACCOUNTS, |s| json!([row(s, None)]), &woke_idle());
    let out = setup.run(&["wake", WAKE_ID]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("account is unknown (?); pass `--account <name>`"),
        "{}",
        stderr(&out)
    );
    assert!(calls_of(setup.main(), "--resume").is_empty());
    assert!(calls_of(setup.alt(), "--resume").is_empty());

    // With --account (one group): woken under it, the answer recorded.
    let out = setup.run(&["wake", WAKE_ID, "--account", "alt"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        calls_of(setup.alt(), "--resume"),
        [strings(&["--resume", WAKE_SID, "--bg"])]
    );
    let state = setup.state();
    assert_eq!(state["sessions"][WAKE_SID]["account"], "alt");
    assert_eq!(state["sessions"][WAKE_SID]["account_source"], "asked");
}

#[test]
fn wake_respects_the_group_guard_for_an_unknown_owner() {
    let _g = lock();
    let grouped = "default_account = \"main\"\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\ngroup = \"work\"\n\
         [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\nconfig_dir = \"~/.claude-alt\"\n\
         group = \"personal\"\n";
    let setup = wake_setup(grouped, |s| json!([row(s, None)]), &woke_idle());
    let out = setup.run(&["wake", WAKE_ID, "--account", "main"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("the accounts are in different groups"),
        "{}",
        stderr(&out)
    );
    assert!(calls_of(setup.main(), "--resume").is_empty());

    // allow_cross_group_handoff lifts it, as for the hand-off (doc/0018).
    let open = format!("allow_cross_group_handoff = true\n{grouped}");
    let setup = wake_setup(&open, |s| json!([row(s, None)]), &woke_idle());
    let out = setup.run(&["wake", WAKE_ID, "--account", "main"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(calls_of(setup.main(), "--resume").len(), 1);
}

#[test]
fn wake_never_moves_a_session_to_another_account() {
    let _g = lock();
    let setup = wake_setup(TWO_ACCOUNTS, |s| json!([row(s, None)]), &woke_idle());
    owned_by(&setup, "main", "started");
    let out = setup.run(&["wake", WAKE_ID, "--account", "alt"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("belongs to main, not alt") && stderr(&out).contains("hand-off"),
        "{}",
        stderr(&out)
    );
    assert!(calls_of(setup.alt(), "--resume").is_empty());
    assert!(calls_of(setup.main(), "--resume").is_empty());
    // The owner's own name is fine.
    let out = setup.run(&["wake", WAKE_ID, "--account", "main"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // An account that is not configured is a usage error.
    let out = setup.run(&["wake", WAKE_ID, "--account", "nobody"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn wake_reports_a_copy_by_its_id_and_records_its_owner() {
    let _g = lock();
    let setup = wake_setup(
        TWO_ACCOUNTS,
        |s| json!([row(s, None)]),
        "echo 'backgrounded · 9f8e7d6c · touch-config'; \
         echo 'note: started a copy of that conversation as 9f8e7d6c.' >&2",
    );
    owned_by(&setup, "main", "started");
    let out = setup.run(&["wake", WAKE_ID, "--text", "go on"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("started a copy 9f8e7d6c instead of waking touch-config (d4a1f7c8)"),
        "{err}"
    );
    assert!(err.contains("note: started a copy"), "{err}");
    assert!(stdout(&out).is_empty());
    let state = setup.state();
    assert_eq!(state["sessions"]["9f8e7d6c"]["account"], "main");
    assert_eq!(state["sessions"]["9f8e7d6c"]["account_source"], "started");
    assert_eq!(state["sessions"][WAKE_SID]["account"], "main");
}

#[test]
fn wake_stops_when_another_tower_recorded_another_owner_meanwhile() {
    let _g = lock();
    let setup = Setup::new(TWO_ACCOUNTS);
    let listing = setup.file("listing.json", &json!([row(&setup, None)]));
    let newer = setup.file(
        "newer-state.json",
        &json!({
            "version": 1,
            "sessions": { WAKE_SID: {
                "account": "alt", "account_source": "handoff", "updated_ms": 2
            } }
        }),
    );
    // While tower lists, "another tower" hands the session off to alt.
    let main = format!(
        "case \"$1\" in\n\
         agents) cp '{newer}' '{state}'; cat '{listing}' ;;\n\
         --resume) {start} ;;\n\
         esac",
        newer = newer.display(),
        state = setup.state_file().display(),
        listing = listing.display(),
        start = woke_prompt(),
    );
    let empty = setup.file("empty.json", &json!([]));
    let alt = body(&empty, &woke_prompt());
    let setup = setup.with_fakes(&main, &alt);
    owned_by(&setup, "main", "started");
    let out = setup.run(&["wake", WAKE_ID, "--text", "go on"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("another tower recorded this session under alt meanwhile"),
        "{}",
        stderr(&out)
    );
    assert!(calls_of(setup.main(), "--resume").is_empty());
    assert!(calls_of(setup.alt(), "--resume").is_empty());
}

#[test]
fn wake_in_an_untrusted_folder_exits_3() {
    let _g = lock();
    let setup = wake_setup(
        TWO_ACCOUNTS,
        |s| json!([row(s, None)]),
        &format!("{}; exit 1", cat_err("cli-output/bg-untrusted.stderr")),
    );
    owned_by(&setup, "main", "started");
    let out = setup.run(&["wake", WAKE_ID]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("(`n`, then `Ctrl+T`"),
        "{}",
        stderr(&out)
    );
    assert_eq!(calls_of(setup.main(), "--resume").len(), 1);
}

#[test]
fn wake_finds_one_session_or_says_why_not() {
    let _g = lock();
    let setup = wake_setup(
        TWO_ACCOUNTS,
        |s| {
            let mut other = row(s, None);
            other["id"] = json!("5e6f7a8b");
            other["sessionId"] = json!("5e6f7a8b-0000-4000-8000-000000000000");
            json!([row(s, None), other])
        },
        &woke_prompt(),
    );
    owned_by(&setup, "main", "started");
    let out = setup.run(&["wake", "nothing-here"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("no session with the id or name `nothing-here`"),
        "{}",
        stderr(&out)
    );
    let out = setup.run(&["wake", "touch-config"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("2 sessions are named `touch-config` (d4a1f7c8, 5e6f7a8b)"),
        "{}",
        stderr(&out)
    );
    assert!(calls_of(setup.main(), "--resume").is_empty());
    // The short id still picks one.
    let out = setup.run(&["wake", WAKE_ID]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn wake_refuses_when_no_account_could_be_listed() {
    let _g = lock();
    let setup = Setup::new(TWO_ACCOUNTS).with_fakes("exit 1", "exit 1");
    let out = setup.run(&["wake", WAKE_ID]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no account could be listed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn wake_argument_errors_exit_2() {
    let _g = lock();
    let setup = wake_setup(TWO_ACCOUNTS, |s| json!([row(s, None)]), &woke_prompt());
    for args in [
        &["wake"][..],
        &["wake", WAKE_ID, "extra"],
        &["wake", WAKE_ID, "--text", "a", "--", "b"],
        &["wake", WAKE_ID, "--"],
        &["wake", WAKE_ID, "--text", "  "],
        &["wake", WAKE_ID, "--text"],
        &["wake", WAKE_ID, "--force"],
    ] {
        let out = setup.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    assert!(calls(setup.main()).is_empty());
}
