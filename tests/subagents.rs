//! Subagent tracking (doc/0029): the hook payloads, the per-session files,
//! the settings file passed with `--settings`, the `--settings` flag on the
//! sessions tower starts, and reading and pruning in the TUI. The hook runs
//! as the real `tower hook` binary where its silence and exit code matter.
//! Payloads are made up, with the shape observed on Claude Code 2.1.303
//! (and 2.1.306 for a subagent continued with `SendMessage`, doc/0035).

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use common::{HOME, NoEnviron, ch, ctrl, key, project_list, type_text};
use fake_claude::{Fake, cat, lock};
use ratatui::crossterm::event::KeyCode;
use serde::Deserialize;
use serde_json::{Value, json};
use watchtower::app::{App, Effect};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::dispatch::{self, DispatchRequest};
use watchtower::listing::{Kind, Listing, Session, State, Status};
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;
use watchtower::subagents::{
    self, Event, MAX_AGENTS, MAX_PAYLOAD, MAX_STOPPED, SessionFile, Subagent, Task, clean,
    ensure_settings, hook_command, is_current, parse, prune, read, read_many, record,
    settings_json,
};

const SID: &str = "1a2b3c4d-0000-4000-8000-000000000000";
const SID2: &str = "5e6f7a8b-0000-4000-8000-000000000000";
const TOWER: &str = env!("CARGO_BIN_EXE_tower");

/// Serializes the tests that start processes with the one that needs the
/// lock file free: a child holds a copy of every open descriptor from its
/// fork until its exec, the lock's among them, and on a loaded machine that
/// is long enough for `prune` (which skips a held lock) to find it taken.
static SPAWNS: Mutex<()> = Mutex::new(());

/// Takes [`SPAWNS`].
fn spawns() -> MutexGuard<'static, ()> {
    SPAWNS.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- payloads ----------------------------------------------------------------

fn start(agent: &str, agent_type: &str) -> Value {
    json!({
        "session_id": SID,
        "transcript_path": "/home/user/.claude/projects/x/transcript.jsonl",
        "cwd": "/home/user/workspace/demo-app",
        "hook_event_name": "SubagentStart",
        "prompt_id": "p1",
        "agent_id": agent,
        "agent_type": agent_type,
    })
}

fn launched(agent: &str, description: &str) -> Value {
    json!({
        "session_id": SID,
        "transcript_path": "/home/user/.claude/projects/x/transcript.jsonl",
        "hook_event_name": "PostToolUse",
        "tool_name": "Agent",
        "tool_input": {
            "description": description,
            "prompt": "a long prompt that is never stored",
            "subagent_type": "general-purpose",
            "run_in_background": true
        },
        "tool_response": {
            "status": "async_launched",
            "agentId": agent,
            "description": description,
            "prompt": "a long prompt that is never stored"
        }
    })
}

fn stop(agent: &str, agent_type: &str, tasks: Value) -> Value {
    json!({
        "session_id": SID,
        "transcript_path": "/home/user/.claude/projects/x/transcript.jsonl",
        "hook_event_name": "SubagentStop",
        "stop_hook_active": false,
        "agent_id": agent,
        "agent_type": agent_type,
        "agent_transcript_path": "/home/user/.claude/projects/x/subagent.jsonl",
        "last_assistant_message": "DONE, with a secret that is never stored",
        "background_tasks": tasks,
    })
}

fn task(id: &str, description: &str, status: &str) -> Value {
    json!({
        "id": id,
        "type": "subagent",
        "status": status,
        "description": description,
        "agent_type": "general-purpose"
    })
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

fn event(v: &Value) -> Event {
    let (session, event) = parse(&bytes(v)).expect("parses");
    assert_eq!(session, SID);
    event
}

fn ids(file: &SessionFile) -> Vec<&str> {
    file.agents.iter().map(|a| a.id.as_str()).collect()
}

// ---- parsing -------------------------------------------------------------------

#[test]
fn each_event_parses() {
    assert_eq!(
        event(&start("a0b1c2d3e4f506172", "general-purpose")),
        Event::Start {
            id: "a0b1c2d3e4f506172".into(),
            agent_type: "general-purpose".into()
        }
    );
    assert_eq!(
        event(&launched("a9f8e7d6c5b4a3921", "alpha\nwaiter\u{7}")),
        Event::Launched {
            id: "a9f8e7d6c5b4a3921".into(),
            agent_type: Some("general-purpose".into()),
            description: Some("alpha waiter".into()),
        }
    );
    let mut sync = launched("a9f8e7d6c5b4a3921", "x");
    sync["tool_response"]["status"] = json!("completed");
    assert_eq!(
        event(&sync),
        Event::Finished {
            id: "a9f8e7d6c5b4a3921".into()
        }
    );
    assert_eq!(
        event(&stop(
            "a9f8e7d6c5b4a3921",
            "general-purpose",
            json!([task("a9f8e7d6c5b4a3921", "alpha waiter", "running")])
        )),
        Event::Stop {
            id: "a9f8e7d6c5b4a3921".into(),
            tasks: Some(vec![Task {
                id: "a9f8e7d6c5b4a3921".into(),
                agent_type: Some("general-purpose".into()),
                description: Some("alpha waiter".into()),
                running: true,
            }])
        }
    );
    // A stop without `background_tasks`.
    let mut bare = stop("a1", "general-purpose", json!(null));
    bare.as_object_mut().unwrap().remove("background_tasks");
    assert_eq!(
        event(&bare),
        Event::Stop {
            id: "a1".into(),
            tasks: None
        }
    );
}

#[test]
fn other_events_tools_and_garbage_are_ignored() {
    let mut other_tool = launched("a1", "x");
    other_tool["tool_name"] = json!("Bash");
    let mut other_event = start("a1", "general-purpose");
    other_event["hook_event_name"] = json!("PreToolUse");
    let mut no_status = launched("a1", "x");
    no_status["tool_response"]
        .as_object_mut()
        .unwrap()
        .remove("status");
    // Claude Code's own helpers: no type.
    let helper_start = start("a1", "");
    for v in [other_tool, other_event, no_status, helper_start] {
        assert_eq!(parse(&bytes(&v)), None, "{v}");
    }
    for garbage in [
        &b""[..],
        b"null",
        b"[]",
        b"{",
        b"\xff\xfe",
        b"{\"session_id\": 5}",
    ] {
        assert_eq!(parse(garbage), None);
    }
}

#[test]
fn invalid_session_and_agent_ids_are_ignored() {
    for sid in [
        "../../../etc/passwd",
        "1A2B3C4D-0000-4000-8000-000000000000",
        "1a2b3c4d",
        "1a2b3c4d-0000-4000-8000-000000000000/../x",
        "",
    ] {
        let mut v = start("a1", "general-purpose");
        v["session_id"] = json!(sid);
        assert_eq!(parse(&bytes(&v)), None, "{sid}");
    }
    for agent in ["", "A1", "a/1", "a 1", "../a", &"a".repeat(65)] {
        assert_eq!(parse(&bytes(&start(agent, "general-purpose"))), None);
        assert_eq!(parse(&bytes(&launched(agent, "x"))), None);
        assert_eq!(parse(&bytes(&stop(agent, "x", json!([])))), None);
    }
    assert_eq!(subagents::file_path(Path::new("/x"), "../y"), None);
}

#[test]
fn descriptions_and_types_are_cleaned_and_capped() {
    let long = "word ".repeat(100);
    let Event::Launched { description, .. } = event(&launched("a1", &long)) else {
        panic!()
    };
    let d = description.unwrap();
    assert_eq!(d.chars().count(), subagents::MAX_DESCRIPTION_CHARS);
    assert!(d.ends_with('…'));
    assert_eq!(
        clean("\u{1b}]0;title\u{7}ok", 50).as_deref(),
        Some("]0;title ok")
    );
    assert_eq!(clean("\u{202e}evil", 50).as_deref(), Some("evil"));
}

// ---- applying events --------------------------------------------------------------

#[test]
fn start_launch_and_stop_reconcile_with_background_tasks() {
    let mut f = SessionFile::default();
    assert!(f.apply(event(&start("a1", "general-purpose")), 100));
    assert!(f.apply(event(&launched("a1", "alpha waiter")), 150));
    assert!(f.apply(event(&start("b2", "general-purpose")), 200));
    assert!(f.apply(event(&launched("b2", "beta waiter")), 250));
    // A third, started but its launch not seen yet: kept by reconciliation.
    assert!(f.apply(event(&start("c3", "Explore")), 300));
    assert_eq!(ids(&f), ["a1", "b2", "c3"]);
    assert_eq!(f.agents[0].description.as_deref(), Some("alpha waiter"));
    assert_eq!(f.agents[0].started_ms, 100, "first sighting");
    assert!(f.agents[0].background);
    // Duplicate start: nothing changes.
    assert!(!f.apply(event(&start("a1", "general-purpose")), 400));

    // b2 stops; the snapshot still lists it, and a1 has ended meanwhile
    // (its own stop was lost): a1 goes, c3 (not known as background) stays.
    let snapshot = json!([
        task("b2", "beta waiter", "running"),
        task("a1", "alpha waiter", "completed"),
        task("c3", "gamma", "running"),
    ]);
    assert!(f.apply(event(&stop("b2", "general-purpose", snapshot)), 500));
    assert_eq!(ids(&f), ["c3"]);
    assert_eq!(
        f.agents[0].description.as_deref(),
        Some("gamma"),
        "filled from background_tasks"
    );
    // A background agent missing from the snapshot goes too.
    assert!(f.apply(event(&launched("d4", "delta")), 600));
    assert!(f.apply(
        event(&stop(
            "c3",
            "Explore",
            json!([task("c3", "gamma", "running")])
        )),
        700
    ));
    assert!(f.agents.is_empty(), "{:?}", f.agents);
    // A launch reported after its agent stopped is not taken as running.
    assert!(!f.apply(event(&launched("b2", "beta waiter")), 800));
    assert!(f.agents.is_empty());
    // A start after a stop is a continuation (doc/0035).
    assert!(f.apply(event(&start("b2", "general-purpose")), 900));
    assert_eq!(ids(&f), ["b2"]);
}

// ---- a subagent continued with SendMessage (doc/0035) ----------------------------

const PROBE: &str = "a5e6d7c8b9a001122";
const HELPER: &str = "a7f8e9d0c1b233445";

fn start_prompt(agent: &str, prompt_id: &str) -> Value {
    let mut v = start(agent, "general-purpose");
    v["prompt_id"] = json!(prompt_id);
    v
}

fn send_message(agent: &str) -> Value {
    json!({
        "session_id": SID,
        "transcript_path": "/home/user/.claude/projects/x/transcript.jsonl",
        "cwd": "/home/user/workspace/demo-app",
        "hook_event_name": "PostToolUse",
        "prompt_id": "p2",
        "tool_name": "SendMessage",
        "tool_input": {
            "to": agent,
            "recipient": agent,
            "recipient_kind": "agent",
            "type": "message",
            "summary": "continue",
            "message": "a message that is never stored",
            "content": "a message that is never stored"
        },
        "tool_response": {}
    })
}

/// A `SubagentStop` as 2.1.306 sends it: `background_tasks` lists the
/// probe as running, also in the probe's own stop.
fn probe_stop(agent: &str, agent_type: &str) -> Value {
    let mut v = stop(
        agent,
        agent_type,
        json!([{
            "id": PROBE,
            "type": "subagent",
            "status": "running",
            "description": "probe agent"
        }]),
    );
    v["session_crons"] = json!([]);
    v
}

fn launched_probe() -> Value {
    let mut v = launched(PROBE, "probe agent");
    v["tool_response"]["isAsync"] = json!(true);
    v["tool_response"]["resolvedModel"] = json!("a-model");
    v
}

/// Records `v` through the file, as the hook does.
fn record_payload(sub: &Path, v: &Value, now_ms: i64) {
    if let Some((sid, ev)) = parse(&bytes(v)) {
        record(sub, &sid, ev, now_ms).unwrap();
    }
}

#[test]
fn a_subagent_continued_with_send_message_shows_again() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subagents");
    // 1-3: launched in the background, then finished.
    record_payload(&sub, &start_prompt(PROBE, "p1"), 1_000);
    record_payload(&sub, &launched_probe(), 1_100);
    assert_eq!(
        read(&sub, SID)[0].description.as_deref(),
        Some("probe agent")
    );
    record_payload(&sub, &probe_stop(PROBE, "general-purpose"), 2_000);
    assert!(read(&sub, SID).is_empty(), "stopped");
    // 4: the SendMessage itself is not an event tower records.
    assert_eq!(parse(&bytes(&send_message(PROBE))), None);
    // 5: the continued run starts under the same id.
    record_payload(&sub, &start_prompt(PROBE, "p2"), 3_000);
    let want = [Subagent {
        id: PROBE.into(),
        agent_type: "general-purpose".into(),
        description: Some("probe agent".into()),
        started_ms: 3_000,
    }];
    assert_eq!(read(&sub, SID), want, "running again, with its description");
    let file: SessionFile =
        serde_json::from_str(&std::fs::read_to_string(sub.join(format!("{SID}.json"))).unwrap())
            .unwrap();
    assert!(file.stopped.is_empty(), "{:?}", file.stopped);
    assert!(file.agents[0].background, "still a background agent");
    // 6: a helper stops, never started; the probe stays.
    record_payload(&sub, &probe_stop(HELPER, ""), 3_500);
    assert_eq!(read(&sub, SID), want);
    // 7: the continued run ends.
    record_payload(&sub, &probe_stop(PROBE, "general-purpose"), 63_000);
    assert!(read(&sub, SID).is_empty());
    // A launch reported after that stop is still ignored.
    record_payload(&sub, &launched_probe(), 64_000);
    assert!(read(&sub, SID).is_empty());
}

#[test]
fn a_late_launch_after_a_stop_is_ignored_but_a_start_continues() {
    let mut f = SessionFile::default();
    f.apply(event(&start("a1", "general-purpose")), 100);
    f.apply(event(&stop("a1", "general-purpose", json!([]))), 200);
    assert!(!f.apply(event(&launched("a1", "alpha")), 300));
    assert!(f.agents.is_empty());
    assert_eq!(f.stopped, ["a1"]);
    // A foreground agent continued: no description, not background.
    assert!(f.apply(event(&start("a1", "Explore")), 400));
    assert_eq!(ids(&f), ["a1"]);
    assert_eq!(f.agents[0].agent_type, "Explore", "the start's type");
    assert_eq!(f.agents[0].description, None);
    assert!(!f.agents[0].background);
    assert_eq!(f.agents[0].started_ms, 400);
    assert!(f.stopped.is_empty() && f.stopped_agents.is_empty());
    // Now running, a launch completes it.
    assert!(f.apply(event(&launched("a1", "alpha")), 500));
    assert_eq!(f.agents[0].description.as_deref(), Some("alpha"));
}

#[test]
fn any_stop_fills_missing_descriptions_without_dropping() {
    let mut f = SessionFile::default();
    f.apply(event(&start(PROBE, "general-purpose")), 100);
    f.apply(event(&launched("b2", "beta")), 200);
    // A helper's stop lists the probe but not b2, a background agent: b2
    // stays (only the stop of an agent in the file drops), the probe gets
    // its description.
    assert!(f.apply(event(&probe_stop(HELPER, "")), 300));
    assert_eq!(ids(&f), [PROBE, "b2"]);
    assert_eq!(f.agents[0].description.as_deref(), Some("probe agent"));
    assert!(f.stopped.is_empty());
    // Nothing more to fill: nothing changes.
    assert!(!f.apply(event(&probe_stop(HELPER, "")), 400));
    // An ended entry fills nothing.
    let mut g = SessionFile::default();
    g.apply(event(&start("c3", "general-purpose")), 100);
    let ended = json!([task("c3", "gamma", "completed")]);
    assert!(!g.apply(event(&stop(HELPER, "", ended)), 200));
    assert_eq!(g.agents[0].description, None);
}

#[test]
fn stopped_agents_are_remembered_within_a_bound() {
    let mut f = SessionFile::default();
    let n = MAX_STOPPED + 6;
    for i in 0..n {
        let id = format!("a{i}");
        f.apply(event(&launched(&id, &format!("agent {i}"))), i as i64);
        f.apply(event(&stop(&id, "general-purpose", json!([]))), i as i64);
    }
    assert!(f.agents.is_empty());
    assert_eq!(f.stopped.len(), MAX_STOPPED);
    assert_eq!(f.stopped_agents.len(), MAX_STOPPED);
    assert_eq!(f.stopped[0], "a6", "the oldest went");
    assert_eq!(f.stopped_agents[0].id, "a6");
    let last = f.stopped_agents.last().unwrap();
    assert_eq!(last.id, format!("a{}", n - 1));
    assert_eq!(
        last.description.as_deref(),
        Some(&*format!("agent {}", n - 1))
    );
    assert!(last.background);
    // A forgotten agent's start is a plain new start.
    assert!(f.apply(event(&start("a0", "general-purpose")), 1_000));
    assert_eq!(f.agents[0].description, None);
    // A remembered one comes back with its description.
    assert!(f.apply(event(&start("a6", "general-purpose")), 1_001));
    assert_eq!(f.agents[1].description.as_deref(), Some("agent 6"));
    assert_eq!(f.stopped.len(), MAX_STOPPED - 1);
    assert_eq!(f.stopped_agents.len(), MAX_STOPPED - 1);
}

/// `SessionFile` as tower 0.4.2 declares it: a TUI of that version reads
/// files an updated hook binary writes.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OldSessionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    agents: Vec<OldAgentRecord>,
    #[serde(default)]
    stopped: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OldAgentRecord {
    id: String,
    #[serde(rename = "type", default)]
    agent_type: String,
    #[serde(default)]
    description: Option<String>,
    started_ms: i64,
    #[serde(default)]
    status: String,
    #[serde(default)]
    background: bool,
}

#[test]
fn the_file_format_stays_readable_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subagents");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join(format!("{SID}.json"));
    // A file of the old format: the probe stopped, nothing remembered.
    let old = json!({
        "version": 1,
        "agents": [{
            "id": "b2", "type": "Explore", "description": "beta",
            "started_ms": 50, "status": "running", "background": false
        }],
        "stopped": [PROBE]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
    assert_eq!(read(&sub, SID).len(), 1);
    // Its continuation runs, its description comes from a later snapshot.
    record_payload(&sub, &start_prompt(PROBE, "p2"), 100);
    assert_eq!(read(&sub, SID)[1].description, None);
    record_payload(&sub, &probe_stop(HELPER, ""), 200);
    assert_eq!(
        read(&sub, SID)[1].description.as_deref(),
        Some("probe agent")
    );
    // A stop writes the new field; the version stays 1.
    record_payload(&sub, &probe_stop(PROBE, "general-purpose"), 300);
    let text = std::fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(
        v["stopped_agents"],
        json!([{
            "id": PROBE, "type": "general-purpose", "description": "probe agent",
            "background": false
        }])
    );
    // The old reader accepts it and sees the same running agents.
    let parsed: OldSessionFile = serde_json::from_str(&text).expect("old reader");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.stopped, [PROBE]);
    let running: Vec<&str> = parsed.agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(running, ["b2"]);
    assert_eq!(read(&sub, SID).len(), 1);
}

#[test]
fn stops_of_unknown_agents_change_nothing() {
    let mut f = SessionFile::default();
    f.apply(event(&launched("a1", "alpha")), 100);
    let before = f.clone();
    // An internal helper: no type, no prior start, an empty snapshot.
    assert!(!f.apply(event(&stop("ffff0001", "", json!([]))), 200));
    assert_eq!(f, before, "no reconciliation either");
}

#[test]
fn a_finished_foreground_agent_goes() {
    let mut f = SessionFile::default();
    f.apply(event(&start("a1", "general-purpose")), 100);
    let mut done = launched("a1", "x");
    done["tool_response"]["status"] = json!("completed");
    assert!(f.apply(event(&done), 200));
    assert!(f.agents.is_empty());
}

#[test]
fn the_number_of_agents_is_capped() {
    let mut f = SessionFile::default();
    for i in 0..(MAX_AGENTS + 5) {
        f.apply(event(&start(&format!("a{i}"), "general-purpose")), i as i64);
    }
    assert_eq!(f.agents.len(), MAX_AGENTS);
    assert_eq!(f.agents[0].id, "a5", "the oldest went");
}

// ---- the file --------------------------------------------------------------------------

#[test]
fn record_keeps_only_the_listed_fields() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subagents");
    for v in [
        start("a1", "general-purpose"),
        launched("a1", "alpha waiter"),
    ] {
        let (sid, ev) = parse(&bytes(&v)).unwrap();
        record(&sub, &sid, ev, 1_790_000_000_000).unwrap();
    }
    let text = std::fs::read_to_string(sub.join(format!("{SID}.json"))).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v,
        json!({
            "version": 1,
            "agents": [{
                "id": "a1",
                "type": "general-purpose",
                "description": "alpha waiter",
                "started_ms": 1_790_000_000_000_i64,
                "status": "running",
                "background": true
            }],
            "stopped": []
        })
    );
    for secret in ["transcript", "prompt", "secret", "/home/user"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    assert_eq!(
        read(&sub, SID),
        [Subagent {
            id: "a1".into(),
            agent_type: "general-purpose".into(),
            description: Some("alpha waiter".into()),
            started_ms: 1_790_000_000_000,
        }]
    );
    // A corrupt file counts as empty and is replaced.
    std::fs::write(sub.join(format!("{SID}.json")), "{ not json").unwrap();
    assert!(read(&sub, SID).is_empty());
    let (sid, ev) = parse(&bytes(&start("b2", "general-purpose"))).unwrap();
    record(&sub, &sid, ev, 5).unwrap();
    assert_eq!(read(&sub, SID).len(), 1);
}

fn hook(dir: &Path, stdin: &[u8]) -> Output {
    let home = tempfile::tempdir().unwrap();
    hook_args(
        home.path(),
        &["hook", "--dir", dir.to_str().unwrap()],
        stdin,
    )
}

/// Runs `tower` with `args`, `HOME` set to `home` and no
/// `XDG_CONFIG_HOME`, writing `stdin` to it.
fn hook_args(home: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(TOWER)
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    // A large payload may be cut short by the hook: a broken pipe is fine.
    let _ = input.write_all(stdin);
    drop(input);
    child.wait_with_output().unwrap()
}

fn assert_silent_success(out: &Output) {
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_hook_binary_records_silently() {
    let _s = spawns();
    let dir = tempfile::tempdir().unwrap();
    for v in [
        start("a1", "general-purpose"),
        launched("a1", "alpha waiter"),
    ] {
        assert_silent_success(&hook(dir.path(), &bytes(&v)));
    }
    let agents = read(&dir.path().join("subagents"), SID);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].description.as_deref(), Some("alpha waiter"));
    assert_silent_success(&hook(
        dir.path(),
        &bytes(&stop(
            "a1",
            "general-purpose",
            json!([task("a1", "alpha waiter", "running")]),
        )),
    ));
    assert!(read(&dir.path().join("subagents"), SID).is_empty());
}

#[test]
fn the_hook_is_silent_and_exits_0_on_every_error() {
    let _s = spawns();
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subagents");
    let mut oversized = bytes(&start("a1", "general-purpose"));
    oversized.extend(std::iter::repeat_n(b' ', MAX_PAYLOAD));
    let mut traversal = start("a1", "general-purpose");
    traversal["session_id"] = json!("../../escape");
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"garbage \x00\xff".to_vec(),
        b"{\"hook_event_name\":".to_vec(),
        oversized,
        bytes(&traversal),
        bytes(&json!({ "hook_event_name": "SubagentStart", "session_id": SID })),
    ];
    for stdin in &cases {
        assert_silent_success(&hook(dir.path(), stdin));
    }
    assert!(!sub.exists(), "nothing recorded");
    assert!(!dir.path().join("escape.json").exists());

    // A config dir that cannot be written: a regular file in its place.
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, "").unwrap();
    assert_silent_success(&hook(&blocked, &bytes(&start("a1", "general-purpose"))));
    // Arguments it does not understand, or a relative dir: still silent,
    // nothing written anywhere (not under HOME either).
    let home = tempfile::tempdir().unwrap();
    let payload = bytes(&start("a1", "general-purpose"));
    for args in [
        &["hook", "--what"][..],
        &["hook", "--dir"],
        &["hook", "--dir", ""],
        &["hook", "--dir", "relative/dir"],
    ] {
        assert_silent_success(&hook_args(home.path(), args, &payload));
    }
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    // A non-UTF-8 argument does not panic the argument parser.
    use std::os::unix::ffi::OsStrExt;
    let out = Command::new(TOWER)
        .arg("hook")
        .arg(std::ffi::OsStr::from_bytes(b"--dir\xff"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_silent_success(&out);
}

#[test]
fn without_dir_the_hook_uses_the_usual_config_dir() {
    let _s = spawns();
    let home = tempfile::tempdir().unwrap();
    let payload = bytes(&start("a1", "general-purpose"));
    assert_silent_success(&hook_args(home.path(), &["hook"], &payload));
    let sub = home.path().join(".config/watchtower/subagents");
    assert_eq!(read(&sub, SID).len(), 1);
}

#[test]
fn concurrent_hooks_do_not_lose_or_interleave_writes() {
    let _s = spawns();
    let dir = tempfile::tempdir().unwrap();
    let n = 16;
    let children: Vec<_> = (0..n)
        .map(|i| {
            let mut child = Command::new(TOWER)
                .args(["hook", "--dir", dir.path().to_str().unwrap()])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let payload = if i % 2 == 0 {
                bytes(&start(&format!("a{i}"), "general-purpose"))
            } else {
                bytes(&launched(&format!("a{i}"), &format!("agent {i}")))
            };
            child.stdin.take().unwrap().write_all(&payload).unwrap();
            child
        })
        .collect();
    for child in children {
        assert_silent_success(&child.wait_with_output().unwrap());
    }
    let sub = dir.path().join("subagents");
    let text = std::fs::read_to_string(sub.join(format!("{SID}.json"))).unwrap();
    let file: SessionFile = serde_json::from_str(&text).expect("valid JSON");
    let mut got: Vec<String> = file.agents.iter().map(|a| a.id.clone()).collect();
    got.sort();
    let mut want: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    want.sort();
    assert_eq!(got, want, "every write kept");
    let leftovers: Vec<_> = std::fs::read_dir(&sub)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

// ---- the settings file --------------------------------------------------------------

#[test]
fn hook_commands_are_quoted_or_refused() {
    assert_eq!(
        hook_command(
            Path::new("/usr/local/bin/tower"),
            Path::new("/home/user/.config/watchtower")
        )
        .unwrap(),
        "/usr/local/bin/tower hook --dir /home/user/.config/watchtower"
    );
    assert_eq!(
        hook_command(Path::new("/opt/my tools/tower"), Path::new("/a'b")).unwrap(),
        r"'/opt/my tools/tower' hook --dir '/a'\''b'"
    );
    assert!(hook_command(Path::new("tower"), Path::new("/x")).is_err());
    assert!(hook_command(Path::new("/x/tower"), Path::new("rel")).is_err());
    assert!(hook_command(Path::new("/x/to\nwer"), Path::new("/x")).is_err());
    assert!(hook_command(Path::new("/x/tower"), Path::new("/x\u{1b}")).is_err());
}

#[test]
fn the_settings_file_holds_only_the_three_hooks() {
    let v: Value = serde_json::from_str(&settings_json("/x/tower hook --dir /y")).unwrap();
    let hook = json!([{ "type": "command", "command": "/x/tower hook --dir /y", "timeout": 10 }]);
    assert_eq!(
        v,
        json!({
            "hooks": {
                "SubagentStart": [{ "hooks": hook }],
                "SubagentStop": [{ "hooks": hook }],
                "PostToolUse": [{ "matcher": "Agent", "hooks": hook }]
            }
        })
    );
}

#[test]
fn ensure_settings_writes_only_on_change() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("new config");
    let settings = config_dir.join("claude-settings.json");
    let exe = Path::new("/usr/local/bin/tower");
    assert_eq!(ensure_settings(&settings, exe), Ok(true));
    let first = std::fs::metadata(&settings).unwrap().modified().unwrap();
    assert_eq!(ensure_settings(&settings, exe), Ok(false), "unchanged");
    assert_eq!(
        std::fs::metadata(&settings).unwrap().modified().unwrap(),
        first
    );
    let v: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    let command = v["hooks"]["SubagentStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert_eq!(
        command,
        format!("/usr/local/bin/tower hook --dir '{}'", config_dir.display())
    );
    assert_eq!(
        ensure_settings(&settings, Path::new("/opt/tower")),
        Ok(true)
    );
    assert!(ensure_settings(&settings, Path::new("relative")).is_err());
}

/// The command in the settings file, run by a shell the way Claude Code
/// runs hooks, reaches `tower hook` with the right folder even when the
/// path needs quoting.
#[test]
fn the_settings_command_runs_through_a_shell() {
    let _s = spawns();
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("it's a dir");
    let settings = config_dir.join("claude-settings.json");
    ensure_settings(&settings, Path::new(TOWER)).unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    let command = v["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut child = Command::new("/bin/sh")
        .args(["-c", &command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&bytes(&launched("a1", "alpha waiter")))
        .unwrap();
    assert_silent_success(&child.wait_with_output().unwrap());
    let agents = read(&config_dir.join("subagents"), SID);
    assert_eq!(agents.len(), 1, "{command}");
}

// ---- --settings on the sessions tower starts ------------------------------------------

fn runner() -> CliRunner {
    CliRunner::new().with_timeout(Duration::from_secs(20))
}

#[test]
fn dispatch_passes_settings_and_writes_the_file() {
    let _g = lock();
    let _s = spawns();
    let fake = Fake::new("claude", &cat("cli-output/bg-success.stdout"));
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let settings = config.path().join("claude-settings.json");
    let mut req = DispatchRequest::new(project.path(), "say OK", fake.account());
    req.settings = Some(settings.clone());
    assert_eq!(
        req.args(),
        [
            "--bg",
            "--permission-mode",
            "auto",
            "--settings",
            settings.to_str().unwrap(),
            "say OK"
        ]
    );
    dispatch::run(&runner(), &req).unwrap();
    assert_eq!(
        fake.argv(),
        [
            "--bg",
            "--permission-mode",
            "auto",
            "--settings",
            settings.to_str().unwrap(),
            "say OK"
        ]
    );
    let v: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert!(v["hooks"]["SubagentStop"].is_array());

    // A settings file that cannot be written: the session starts without.
    let blocked = config.path().join("file");
    std::fs::write(&blocked, "").unwrap();
    req.settings = Some(blocked.join("claude-settings.json"));
    dispatch::run(&runner(), &req).unwrap();
    assert_eq!(fake.argv(), ["--bg", "--permission-mode", "auto", "say OK"]);

    // A relative settings path is refused before anything runs.
    req.settings = Some(PathBuf::from("rel.json"));
    assert!(req.validate().is_err());
}

fn config(extra: &str) -> Config {
    Config::parse(
        &format!(
            "default_account = \"main\"\n{extra}\n\
             [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
             [[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\nconfig_dir = \"~/.claude-alt\"\n"
        ),
        Path::new(HOME),
    )
    .unwrap()
}

fn app(extra: &str) -> App {
    App::new(
        config(extra),
        HOME.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    )
}

/// The dialog's dispatch request.
fn dialog_request(app: &mut App) -> DispatchRequest {
    let root = tempfile::tempdir().unwrap();
    let list = project_list(root.path(), &["api"]);
    assert_eq!(app.handle_key(ch('n')), [Effect::OpenDialog]);
    app.open_dialog(list);
    app.handle_key(key(KeyCode::Tab));
    type_text(app, "fix the flaky test");
    match app.handle_key(ctrl('s')).as_slice() {
        [Effect::Dispatch(job)] => job.request.clone(),
        other => panic!("expected a dispatch, got {other:?}"),
    }
}

/// The orchestrator's start request (`o` with no orchestrator listed).
fn orchestrator_request(app: &mut App) -> DispatchRequest {
    for account in ["main", "alt"] {
        app.apply_poll(PollUpdate {
            account: account.into(),
            cycle: 0,
            result: Ok(Listing::default()),
        });
    }
    match app.handle_key(ch('o')).as_slice() {
        [Effect::StartOrchestrator(job)] => job.request.clone(),
        other => panic!("expected an orchestrator start, got {other:?}"),
    }
}

#[test]
fn new_sessions_and_the_orchestrator_get_settings_while_tracking() {
    let config_dir = Path::new("/home/user/.config/watchtower");
    let settings = config_dir.join("claude-settings.json");
    let mut on = app("");
    on.enable_subagents(config_dir);
    assert_eq!(on.claude_settings(), Some(settings.clone()));
    let req = dialog_request(&mut on);
    assert_eq!(req.settings, Some(settings.clone()));
    assert!(req.args().contains(&"--settings".to_owned()));
    let mut on = app("");
    on.enable_subagents(config_dir);
    let req = orchestrator_request(&mut on);
    assert_eq!(req.settings, Some(settings.clone()));
    let args = req.args();
    let at = args.iter().position(|a| a == "--settings").unwrap();
    assert_eq!(args[at + 1], settings.to_str().unwrap());

    // Never enabled (as in the other tests): no --settings.
    let mut never = app("");
    assert_eq!(never.claude_settings(), None);
    assert_eq!(dialog_request(&mut never).settings, None);
    // Off in the config: no --settings either.
    let mut off = app("[sessions]\ntrack_subagents = false");
    off.enable_subagents(config_dir);
    assert_eq!(off.claude_settings(), None);
    assert_eq!(dialog_request(&mut off).settings, None);
    let mut off = app("[sessions]\ntrack_subagents = false");
    off.enable_subagents(config_dir);
    let req = orchestrator_request(&mut off);
    assert!(!req.args().contains(&"--settings".to_owned()));
}

// ---- reading and pruning ------------------------------------------------------------------

fn row(sid: &str, name: &str, state: State, live: bool) -> Session {
    Session {
        kind: Kind::Background,
        id: Some(sid[..8].to_owned()),
        session_id: Some(sid.to_owned()),
        name: Some(name.to_owned()),
        cwd: "/home/user/workspace/demo-app".into(),
        started_at: Some(1_790_000_000_000),
        state: Some(state),
        status: live.then_some(Status::Busy),
        pid: live.then_some(4_194_304),
        ..Session::default()
    }
}

fn write_agents(dir: &Path, sid: &str, agents: &[&str]) {
    for (i, a) in agents.iter().enumerate() {
        let (_, ev) = parse(&bytes(&start(a, "general-purpose"))).unwrap();
        record(dir, sid, ev, 1_000 + i as i64).unwrap();
    }
}

#[test]
fn only_live_working_rows_have_current_subagents() {
    assert!(is_current(&row(SID, "a", State::Working, true)));
    assert!(!is_current(&row(SID, "a", State::Working, false)));
    assert!(!is_current(&row(SID, "a", State::Done, true)));
    assert!(!is_current(&row(SID, "a", State::Blocked, true)));

    let dir = tempfile::tempdir().unwrap();
    write_agents(dir.path(), SID, &["a1", "b2"]);
    let map = read_many(dir.path(), [SID, SID2, "../x"]);
    assert_eq!(map.len(), 1);
    assert_eq!(map[SID].len(), 2);
}

#[test]
fn the_app_reads_after_polls_and_maps_by_session_id() {
    let config_dir = tempfile::tempdir().unwrap();
    let dir = subagents::dir(config_dir.path());
    let mut app = app("");
    // Not enabled: no read.
    assert_eq!(app.start_subagent_read(Instant::now()), None);
    app.enable_subagents(config_dir.path());
    let working = row(SID, "working", State::Working, true);
    let done = row(SID2, "done", State::Done, true);
    for (account, rows) in [
        ("main", vec![working.clone(), done.clone()]),
        ("alt", vec![]),
    ] {
        app.apply_poll(PollUpdate {
            account: account.into(),
            cycle: 0,
            result: Ok(Listing {
                sessions: rows,
                warnings: Vec::new(),
            }),
        });
    }
    write_agents(&dir, SID, &["a1"]);
    write_agents(&dir, SID2, &["b2"]);
    let job = app.start_subagent_read(Instant::now()).expect("a read");
    assert_eq!(job.current, [SID.to_owned()]);
    let (listed, complete) = job.prune.clone().expect("first read prunes");
    assert!(complete);
    assert_eq!(listed, HashSet::from([SID.to_owned(), SID2.to_owned()]));
    assert_eq!(
        app.start_subagent_read(Instant::now()),
        None,
        "one at a time"
    );
    app.subagents_read(job.run(SystemTime::now()));
    assert_eq!(app.subagents_of(&working).len(), 1);
    assert_eq!(app.subagents_of(&working)[0].id, "a1");
    assert!(app.subagents_of(&done).is_empty(), "not working");
    let stopped = row(SID, "working", State::Working, false);
    assert!(app.subagents_of(&stopped).is_empty(), "not live");
    // The next read within a minute does not prune again.
    let job = app.start_subagent_read(Instant::now()).unwrap();
    assert_eq!(job.prune, None);
    app.subagents_read(job.run(SystemTime::now()));

    // Tracking turned off: the subagents are forgotten and not read.
    let mut off = App::new(
        config("[sessions]\ntrack_subagents = false"),
        HOME.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    off.enable_subagents(config_dir.path());
    assert_eq!(off.start_subagent_read(Instant::now()), None);
    assert!(off.subagents_of(&working).is_empty());
}

fn age(path: &Path, ago: Duration) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - ago).unwrap();
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn pruning_follows_the_sidecar_rules() {
    let _s = spawns();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let listed_sid = "aaaaaaaa-0000-4000-8000-000000000000";
    let fresh_unlisted = "bbbbbbbb-0000-4000-8000-000000000000";
    let old_unlisted = "cccccccc-0000-4000-8000-000000000000";
    let ancient_listed = "dddddddd-0000-4000-8000-000000000000";
    for sid in [listed_sid, fresh_unlisted, old_unlisted, ancient_listed] {
        write_agents(d, sid, &["a1"]);
    }
    let file = |sid: &str| d.join(format!("{sid}.json"));
    age(&file(listed_sid), 2 * HOUR);
    age(&file(old_unlisted), 2 * HOUR);
    age(&file(ancient_listed), 15 * 24 * HOUR);
    let tmp = d.join(format!("{listed_sid}.json.tmp.1.0"));
    std::fs::write(&tmp, "").unwrap();
    age(&tmp, 2 * HOUR);
    let other = d.join("notes.txt");
    std::fs::write(&other, "").unwrap();
    age(&other, 30 * 24 * HOUR);
    let listed = HashSet::from([listed_sid.to_owned(), ancient_listed.to_owned()]);

    // An incomplete listing: only the 14-day rule and old temp files.
    assert_eq!(prune(d, &listed, false, SystemTime::now()), 2);
    assert!(!file(ancient_listed).exists());
    assert!(!tmp.exists());
    assert!(file(old_unlisted).exists());
    // A complete one: unlisted files older than an hour go too.
    assert_eq!(prune(d, &listed, true, SystemTime::now()), 1);
    assert!(!file(old_unlisted).exists());
    assert!(
        file(fresh_unlisted).exists(),
        "written less than an hour ago"
    );
    assert!(file(listed_sid).exists());
    assert!(other.exists(), "not tower's file name");
    assert!(d.join(".lock").exists(), "the lock file stays");
    assert!(!read(d, listed_sid).is_empty());
}
