//! Sidecar round-trip, atomic write and corrupt-file handling (doc/0002).

use watchtower::listing::Session;
use watchtower::listing::{State, Status, WaitingFor};
use watchtower::merge::{MergedSession, Ownership};
use watchtower::sidecar::{AccountSource, Sidecar, StateTuple, corrupt_path};

const SID: &str = "1a2b3c4d-0000-4000-8000-000000000000";

const DOC_EXAMPLE: &str = r#"{
  "version": 1,
  "sessions": {
    "1a2b3c4d-0000-4000-8000-000000000000": {
      "account": "alt",
      "account_source": "handoff",
      "last_state": { "state": "blocked", "status": "waiting", "waitingFor": "permission prompt" },
      "last_change_ms": 1790000000000,
      "acknowledged": { "state": "blocked", "status": "waiting", "waitingFor": "permission prompt" },
      "pinned_note": "future field"
    }
  },
  "recent_projects": [
    { "path": "~/workspace/demo-app", "last_used_ms": 1790000000000 }
  ],
  "future_top_level": [1, 2, 3]
}"#;

#[test]
fn missing_file_starts_empty_without_warning() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = Sidecar::load(&dir.path().join("state.json"));
    assert_eq!(loaded.sidecar, Sidecar::default());
    assert!(loaded.warning.is_none());
}

#[test]
fn doc_example_round_trips_with_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, DOC_EXAMPLE).unwrap();

    let loaded = Sidecar::load(&path);
    assert!(loaded.warning.is_none());
    let sc = loaded.sidecar;
    assert_eq!(sc.version, 1);
    let rec = &sc.sessions[SID];
    assert_eq!(rec.account.as_deref(), Some("alt"));
    assert_eq!(rec.account_source, Some(AccountSource::Handoff));
    assert_eq!(
        rec.last_state,
        Some(StateTuple {
            state: Some(State::Blocked),
            status: Some(Status::Waiting),
            waiting_for: Some(WaitingFor::PermissionPrompt),
        })
    );
    assert_eq!(rec.last_change_ms, Some(1790000000000));
    assert_eq!(sc.owner(SID), Some(("alt", &AccountSource::Handoff)));
    assert_eq!(sc.recent_projects[0].path, "~/workspace/demo-app");

    sc.save(&path).unwrap();
    assert!(!dir.path().join("state.json.tmp").exists());
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(DOC_EXAMPLE).unwrap();
    assert_eq!(again, original, "unknown fields are kept");
}

#[test]
fn save_creates_directory_and_replaces_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watchtower").join("state.json");
    let mut sc = Sidecar::default();
    assert!(sc.record_owner(SID, "main", AccountSource::Started));
    sc.save(&path).unwrap();
    assert!(sc.record_answer(SID, "alt"));
    sc.save(&path).unwrap();
    let loaded = Sidecar::load(&path);
    assert_eq!(
        loaded.sidecar.owner(SID),
        Some(("alt", &AccountSource::Asked))
    );
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["state.json"]);
}

#[test]
fn record_owner_reports_changes_only() {
    let mut sc = Sidecar::default();
    assert!(sc.record_owner(SID, "main", AccountSource::Proc));
    assert!(!sc.record_owner(SID, "main", AccountSource::Proc));
    assert!(sc.record_owner(SID, "main", AccountSource::List));
    assert!(sc.record_owner(SID, "alt", AccountSource::List));
    assert!(sc.record_answer("5e6f7a8b-0000-4000-8000-000000000000", "main"));
    assert!(!sc.record_answer("5e6f7a8b-0000-4000-8000-000000000000", "main"));
}

#[test]
fn record_detections_stores_proc_and_list_only() {
    let row = |sid: &str| Session {
        session_id: Some(sid.into()),
        ..Default::default()
    };
    let merged = vec![
        MergedSession {
            session: row("a0000000-0000-4000-8000-000000000000"),
            seen_in: vec!["main".into()],
            owner: Ownership::Known {
                account: "main".into(),
                source: AccountSource::Proc,
            },
        },
        MergedSession {
            session: row("b0000000-0000-4000-8000-000000000000"),
            seen_in: vec!["alt".into()],
            owner: Ownership::Known {
                account: "alt".into(),
                source: AccountSource::List,
            },
        },
        MergedSession {
            session: row("c0000000-0000-4000-8000-000000000000"),
            seen_in: vec!["main".into(), "alt".into()],
            owner: Ownership::Known {
                account: "alt".into(),
                source: AccountSource::Handoff,
            },
        },
        MergedSession {
            session: row("d0000000-0000-4000-8000-000000000000"),
            seen_in: vec!["main".into(), "alt".into()],
            owner: Ownership::Unknown { config_dir: None },
        },
    ];
    let mut sc = Sidecar::default();
    assert!(sc.record_detections(&merged));
    assert!(!sc.record_detections(&merged));
    assert_eq!(sc.sessions.len(), 2);
    assert_eq!(
        sc.owner("a0000000-0000-4000-8000-000000000000"),
        Some(("main", &AccountSource::Proc))
    );
    assert_eq!(
        sc.owner("b0000000-0000-4000-8000-000000000000"),
        Some(("alt", &AccountSource::List))
    );
}

#[test]
fn corrupt_file_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "{ not json").unwrap();
    let loaded = Sidecar::load(&path);
    assert_eq!(loaded.sidecar, Sidecar::default());
    let warning = loaded.warning.expect("warning");
    assert!(warning.contains("corrupt"), "{warning}");
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(corrupt_path(&path)).unwrap(),
        "{ not json"
    );

    // A wrongly typed known field also counts as corrupt.
    std::fs::write(&path, r#"{ "version": "one" }"#).unwrap();
    assert!(Sidecar::load(&path).warning.is_some());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("state.json.corrupt")).unwrap(),
        r#"{ "version": "one" }"#
    );
}

#[test]
fn unreadable_file_starts_fresh_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be cannot be read as a file.
    let path = dir.path().join("state.json");
    std::fs::create_dir(&path).unwrap();
    let loaded = Sidecar::load(&path);
    assert_eq!(loaded.sidecar, Sidecar::default());
    assert!(loaded.warning.unwrap().contains("unreadable"));
}

#[test]
fn unknown_account_source_is_kept() {
    let sc: Sidecar = serde_json::from_str(
        r#"{ "version": 1, "sessions": { "x": { "account": "main", "account_source": "guessed" } } }"#,
    )
    .unwrap();
    assert_eq!(
        sc.owner("x"),
        Some(("main", &AccountSource::Unknown("guessed".into())))
    );
}
