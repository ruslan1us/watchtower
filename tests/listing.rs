//! Parser tests from `tests/fixtures/agents-json/` (doc/0015).

mod common;

use common::{by_name, fixture_path, listing};
use watchtower::listing::{Kind, ParseError, State, Status, WaitingFor, parse_listing};

#[test]
fn every_agents_json_fixture_parses() {
    let dir = fixture_path("agents-json");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let listing = parse_listing(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        assert!(
            listing.warnings.is_empty(),
            "{path:?}: {:?}",
            listing.warnings
        );
        assert!(!listing.sessions.is_empty(), "{path:?}");
        count += 1;
    }
    assert!(count >= 4);
}

#[test]
fn mixed_states_rows() {
    let l = listing("mixed-states.json");
    assert_eq!(l.sessions.len(), 6);

    let cases: [(&str, State, Status, Option<WaitingFor>); 5] = [
        ("bad-model", State::Failed, Status::Idle, None),
        ("run-test-suite", State::Working, Status::Busy, None),
        ("print-date", State::Done, Status::Idle, None),
        ("say-ok", State::Done, Status::Idle, None),
        (
            "pick-colour",
            State::Blocked,
            Status::Waiting,
            Some(WaitingFor::InputNeeded),
        ),
    ];
    for (name, state, status, waiting) in cases {
        let s = by_name(&l.sessions, name);
        assert_eq!(s.kind, Kind::Background, "{name}");
        assert_eq!(s.state, Some(state), "{name}");
        assert_eq!(s.status, Some(status), "{name}");
        assert_eq!(s.waiting_for, waiting, "{name}");
        assert!(s.id.is_some() && s.pid.is_some(), "{name}");
        assert!(s.has_live_process() && s.is_attachable(), "{name}");
    }

    let s = by_name(&l.sessions, "pick-colour");
    assert_eq!(s.id.as_deref(), Some("9c6e1a35"));
    assert_eq!(
        s.session_id.as_deref(),
        Some("9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20")
    );
    assert_eq!(s.cwd, "/home/user/workspace/demo-app");
    assert_eq!(s.started_at, Some(1767232818776));
    assert_eq!(s.pid, Some(4315));
    assert_eq!(s.key(), s.session_id.as_deref());
}

#[test]
fn interactive_row_has_no_id_or_state() {
    let l = listing("mixed-states.json");
    let s = by_name(&l.sessions, "demo-app-3f");
    assert_eq!(s.kind, Kind::Interactive);
    assert_eq!(s.id, None);
    assert_eq!(s.state, None);
    assert_eq!(s.status, Some(Status::Busy));
    assert_eq!(s.pid, Some(1201));
    assert!(s.is_interactive());
    assert!(!s.is_attachable());
    assert!(s.has_live_process());
}

#[test]
fn permission_prompt_row() {
    let l = listing("permission-prompt.json");
    let s = by_name(&l.sessions, "touch-config");
    assert_eq!(s.state, Some(State::Blocked));
    assert_eq!(s.status, Some(Status::Waiting));
    assert_eq!(s.waiting_for, Some(WaitingFor::PermissionPrompt));
    // run-test-suite moved on from working/busy to done/idle.
    let s = by_name(&l.sessions, "run-test-suite");
    assert_eq!(
        (s.state.clone(), s.status.clone()),
        (Some(State::Done), Some(Status::Idle))
    );
}

#[test]
fn after_stop_versus_after_stop_all() {
    let active = listing("after-stop.json");
    let all = listing("after-stop-all.json");
    assert!(
        active
            .sessions
            .iter()
            .all(|s| s.name.as_deref() != Some("say-ok"))
    );
    assert_eq!(all.sessions.len(), active.sessions.len() + 1);

    let stopped = by_name(&all.sessions, "say-ok");
    assert_eq!(stopped.state, Some(State::Done), "keeps its last state");
    assert_eq!(stopped.pid, None);
    assert_eq!(stopped.status, None);
    assert!(!stopped.has_live_process());
    assert!(stopped.is_attachable(), "attach wakes a stopped session");

    // Every other row is identical in both listings.
    for s in &active.sessions {
        assert!(all.sessions.contains(s), "{:?}", s.name);
    }
}

#[test]
fn unknown_fields_and_values_are_kept() {
    let json = r#"[
      { "kind": "background", "id": "1a2b3c4d", "cwd": "/home/user/workspace/x",
        "startedAt": 1790000000000, "pid": 4242, "status": "napping",
        "waitingFor": "coffee", "state": "paused", "summary": "new field",
        "nested": { "a": [1, 2] } },
      { "kind": "remote", "cwd": "/home/user/workspace/y", "startedAt": 1 }
    ]"#;
    let l = parse_listing(json).unwrap();
    assert!(l.warnings.is_empty());
    let s = &l.sessions[0];
    assert_eq!(s.state, Some(State::Unknown("paused".into())));
    assert_eq!(s.status, Some(Status::Unknown("napping".into())));
    assert_eq!(s.waiting_for, Some(WaitingFor::Unknown("coffee".into())));
    assert_eq!(l.sessions[1].kind, Kind::Unknown("remote".into()));
}

#[test]
fn missing_fields_get_defaults() {
    let l = parse_listing(r#"[ {} , { "id": "1a2b3c4d", "state": null } ]"#).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.sessions.len(), 2);
    let s = &l.sessions[0];
    assert_eq!(s.kind, Kind::Unknown(String::new()));
    assert_eq!(s.cwd, "");
    assert_eq!(
        (s.id.clone(), s.session_id.clone(), s.pid, s.started_at),
        (None, None, None, None)
    );
    assert!(!s.has_live_process());
    assert_eq!(l.sessions[1].state, None);
    assert_eq!(l.sessions[1].key(), Some("1a2b3c4d"));
}

#[test]
fn status_alone_counts_as_live() {
    let l = parse_listing(r#"[ { "kind": "background", "id": "1a2b3c4d", "status": "idle" } ]"#)
        .unwrap();
    assert!(l.sessions[0].has_live_process());
}

#[test]
fn empty_list() {
    let l = parse_listing("[]\n").unwrap();
    assert!(l.sessions.is_empty() && l.warnings.is_empty());
}

#[test]
fn empty_output_is_an_empty_listing_with_warning() {
    let l = parse_listing("  \n").unwrap();
    assert!(l.sessions.is_empty());
    assert_eq!(l.warnings.len(), 1);
}

#[test]
fn malformed_rows_are_skipped_with_warnings() {
    let json = r#"[
      { "kind": "background", "id": "1a2b3c4d", "pid": "not-a-number" },
      "just a string",
      { "kind": "background", "id": "5e6f7a8b", "state": "done", "pid": 4343, "status": "idle" },
      { "kind": "background", "id": "9c8d7e6f", "state": 7 }
    ]"#;
    let l = parse_listing(json).unwrap();
    assert_eq!(l.sessions.len(), 1);
    assert_eq!(l.sessions[0].id.as_deref(), Some("5e6f7a8b"));
    assert_eq!(l.warnings.len(), 3, "{:?}", l.warnings);
    assert!(l.warnings[0].contains("row 0"));
    assert!(l.warnings[1].contains("row 1"));
    assert!(l.warnings[2].contains("row 3"));
}

#[test]
fn non_array_and_invalid_json_fail() {
    assert!(matches!(parse_listing("{}"), Err(ParseError::NotAnArray)));
    assert!(matches!(parse_listing("[{"), Err(ParseError::Json(_))));
    assert!(matches!(
        parse_listing("Starting background service…"),
        Err(ParseError::Json(_))
    ));
}

#[test]
fn session_serializes_back_to_cli_field_names() {
    let l = listing("after-stop-all.json");
    let s = by_name(&l.sessions, "pick-colour");
    let v = serde_json::to_value(s).unwrap();
    assert_eq!(v["sessionId"], "9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20");
    assert_eq!(v["waitingFor"], "input needed");
    assert_eq!(v["startedAt"], 1767232818776i64);
    let stopped = serde_json::to_value(by_name(&l.sessions, "say-ok")).unwrap();
    assert!(stopped.get("pid").is_none() && stopped.get("status").is_none());
}
