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
    let mut sc = loaded.sidecar;
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
    assert_eq!(dir_names(dir.path()), ["state.json", "state.json.lock"]);
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
    // No temp file is left behind; the lock file stays (it is never removed).
    assert_eq!(
        dir_names(path.parent().unwrap()),
        ["state.json", "state.json.lock"]
    );
}

fn dir_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
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

// ---- recent projects (doc/0008) ----------------------------------------

use std::path::Path;
use watchtower::sidecar::{MAX_RECENT_PROJECTS, RecentProject};

fn recent_paths(sc: &Sidecar) -> Vec<&str> {
    sc.recent_projects.iter().map(|r| r.path.as_str()).collect()
}

#[test]
fn touch_recent_project_moves_to_front_in_tilde_form() {
    let home = Path::new("/home/user");
    let mut sc = Sidecar::default();
    assert!(sc.touch_recent_project(Path::new("/home/user/workspace/a"), home, 1));
    assert!(sc.touch_recent_project(Path::new("/home/user/workspace/b"), home, 2));
    assert!(sc.touch_recent_project(Path::new("/srv/c"), home, 3));
    assert_eq!(
        recent_paths(&sc),
        ["/srv/c", "~/workspace/b", "~/workspace/a"]
    );

    assert!(sc.touch_recent_project(Path::new("/home/user/workspace/a"), home, 4));
    assert_eq!(
        recent_paths(&sc),
        ["~/workspace/a", "/srv/c", "~/workspace/b"]
    );
    assert_eq!(sc.recent_projects[0].last_used_ms, 4);
    // Same project, same time: unchanged, no save due.
    assert!(!sc.touch_recent_project(Path::new("/home/user/workspace/a"), home, 4));
}

#[test]
fn touch_recent_project_keeps_unknown_fields_and_drops_duplicates() {
    let home = Path::new("/home/user");
    let mut sc: Sidecar = serde_json::from_str(
        r#"{"recent_projects": [
            {"path": "~/workspace/b", "last_used_ms": 5},
            {"path": "/home/user/workspace/a", "last_used_ms": 4, "note": "kept"},
            {"path": "~/workspace/a", "last_used_ms": 3}
        ]}"#,
    )
    .unwrap();
    assert!(sc.touch_recent_project(Path::new("/home/user/workspace/a"), home, 9));
    assert_eq!(recent_paths(&sc), ["~/workspace/a", "~/workspace/b"]);
    assert_eq!(sc.recent_projects[0].extra["note"], "kept");
}

#[test]
fn touch_recent_project_caps_the_list() {
    let home = Path::new("/home/user");
    let mut sc = Sidecar::default();
    sc.recent_projects = (0..MAX_RECENT_PROJECTS + 5)
        .map(|i| RecentProject {
            path: format!("~/workspace/p{i}"),
            last_used_ms: 0,
            extra: Default::default(),
        })
        .collect();
    assert!(sc.touch_recent_project(Path::new("/home/user/workspace/new"), home, 1));
    assert_eq!(sc.recent_projects.len(), MAX_RECENT_PROJECTS);
    assert_eq!(sc.recent_projects[0].path, "~/workspace/new");
    assert_eq!(
        sc.recent_projects.last().unwrap().path,
        format!("~/workspace/p{}", MAX_RECENT_PROJECTS - 2)
    );
}

#[test]
fn touch_recent_project_matches_through_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    std::fs::create_dir_all(home.join("workspace/a")).unwrap();
    std::os::unix::fs::symlink(home.join("workspace"), home.join("ws")).unwrap();
    let mut sc = Sidecar::default();
    sc.touch_recent_project(&home.join("ws/a"), home, 1);
    sc.touch_recent_project(&home.join("workspace/a"), home, 2);
    assert_eq!(recent_paths(&sc), ["~/workspace/a"]);
}

#[test]
fn started_record_and_short_id_adoption() {
    let mut sc = Sidecar::default();
    assert!(sc.record_started("1a2b3c4d", "alt"));
    assert_eq!(sc.owner("1a2b3c4d"), Some(("alt", &AccountSource::Started)));

    // A row without sessionId keeps the short-id key.
    let bare = Session {
        id: Some("1a2b3c4d".into()),
        ..Session::default()
    };
    assert!(!sc.adopt_short_id_records([&bare]));

    // A newer record under the sessionId wins over the short-id one.
    let row = Session {
        id: Some("1a2b3c4d".into()),
        session_id: Some(SID.into()),
        ..Session::default()
    };
    sc.record_owner(SID, "main", AccountSource::Proc);
    assert!(sc.adopt_short_id_records([&row]));
    assert_eq!(sc.owner(SID), Some(("main", &AccountSource::Proc)));
    assert!(!sc.sessions.contains_key("1a2b3c4d"));
}

#[test]
fn now_ms_is_plausible() {
    // After 2020-01-01.
    assert!(watchtower::sidecar::now_ms() > 1_577_836_800_000);
}

// ---- several instances: merge on save, lock (doc/0017 section 8) --------

use std::time::Duration;
use watchtower::sidecar::{
    PRUNE_UNSEEN_MS, PRUNE_UNSTARTED_MS, SEEN_RESOLUTION_MS, lock_path, tmp_path,
};

const SID2: &str = "5e6f7a8b-0000-4000-8000-000000000000";
const SID3: &str = "9c8d7e6f-0000-4000-8000-000000000000";

/// A state file where `main` owns `SID` by `proc`, and two instances that
/// loaded it.
fn two_instances(dir: &Path) -> (std::path::PathBuf, Sidecar, Sidecar) {
    let path = dir.join("state.json");
    let mut first = Sidecar::default();
    first.record_owner_at(SID, "main", AccountSource::Proc, 100);
    first.save(&path).unwrap();
    let a = Sidecar::load(&path).sidecar;
    let b = Sidecar::load(&path).sidecar;
    (path, a, b)
}

fn on_disk(path: &Path) -> Sidecar {
    let loaded = Sidecar::load(path);
    assert!(loaded.warning.is_none(), "{:?}", loaded.warning);
    loaded.sidecar
}

#[test]
fn two_instances_do_not_revert_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut a, mut b) = two_instances(dir.path());

    // Instance 1 hands SID off; instance 2, still holding `proc main` in
    // memory, records something else and saves.
    assert!(a.record_owner_at(SID, "alt", AccountSource::Handoff, 200));
    a.save(&path).unwrap();
    assert!(b.record_owner_at(SID2, "main", AccountSource::Asked, 300));
    b.save(&path).unwrap();

    let disk = on_disk(&path);
    assert_eq!(disk.owner(SID), Some(("alt", &AccountSource::Handoff)));
    assert_eq!(disk.owner(SID2), Some(("main", &AccountSource::Asked)));
    // Instance 2's memory now has instance 1's change too.
    assert_eq!(b.owner(SID), Some(("alt", &AccountSource::Handoff)));
    assert!(!b.is_dirty());

    // And back the other way: instance 1 saves again without reverting 2.
    assert!(a.record_owner_at(SID3, "alt", AccountSource::Started, 400));
    a.save(&path).unwrap();
    let disk = on_disk(&path);
    assert_eq!(disk.owner(SID2), Some(("main", &AccountSource::Asked)));
    assert_eq!(disk.owner(SID3), Some(("alt", &AccountSource::Started)));
    assert_eq!(disk.owner(SID), Some(("alt", &AccountSource::Handoff)));
    assert_eq!(a, disk);
}

#[test]
fn newer_updated_ms_wins_whichever_saves_first() {
    for a_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (path, mut a, mut b) = two_instances(dir.path());
        a.record_owner_at(SID, "main", AccountSource::Asked, 1_000);
        b.record_owner_at(SID, "alt", AccountSource::Handoff, 2_000);
        let (x, y) = if a_first {
            (&mut a, &mut b)
        } else {
            (&mut b, &mut a)
        };
        x.save(&path).unwrap();
        y.save(&path).unwrap();
        let disk = on_disk(&path);
        assert_eq!(
            disk.owner(SID),
            Some(("alt", &AccountSource::Handoff)),
            "a_first={a_first}"
        );
        assert_eq!(disk.sessions[SID].updated_ms, Some(2_000));
    }
}

#[test]
fn list_never_beats_proc_in_a_merge() {
    for a_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut a = Sidecar::default();
        let mut b = Sidecar::default();
        // `proc` older than the `list` detection, still kept.
        a.record_owner_at(SID, "main", AccountSource::Proc, 1_000);
        b.record_owner_at(SID, "alt", AccountSource::List, 2_000);
        let (x, y) = if a_first {
            (&mut a, &mut b)
        } else {
            (&mut b, &mut a)
        };
        x.save(&path).unwrap();
        y.save(&path).unwrap();
        assert_eq!(
            on_disk(&path).owner(SID),
            Some(("main", &AccountSource::Proc)),
            "a_first={a_first}"
        );
    }
}

#[test]
fn concurrent_saves_never_corrupt_or_lose_records() {
    const PER_THREAD: usize = 100;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let threads: Vec<_> = (0..3)
        .map(|t| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut sc = Sidecar::load(&path).sidecar;
                for i in 0..PER_THREAD {
                    let key = format!("{t:x}{i:07x}-0000-4000-8000-000000000000");
                    sc.record_owner(&key, "main", AccountSource::Asked);
                    sc.save_with_wait(&path, Duration::from_secs(30)).unwrap();
                    // Every save leaves a complete, parseable file.
                    let text = std::fs::read(&path).unwrap();
                    serde_json::from_slice::<serde_json::Value>(&text).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let disk = on_disk(&path);
    assert_eq!(disk.sessions.len(), 3 * PER_THREAD);
    assert!(!corrupt_path(&path).exists());
    assert_eq!(dir_names(dir.path()), ["state.json", "state.json.lock"]);
}

#[test]
fn temp_file_names_are_unique() {
    let path = Path::new("/home/user/.config/watchtower/state.json");
    let one = tmp_path(path);
    let two = tmp_path(path);
    assert_ne!(one, two);
    let name = one.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with(&format!("state.json.tmp.{}.", std::process::id())),
        "{name}"
    );
    assert_eq!(one.parent(), path.parent());
}

#[test]
fn lock_timeout_keeps_the_change_for_a_later_save() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut sc = Sidecar::default();
    sc.record_owner(SID, "alt", AccountSource::Handoff);
    sc.save(&path).unwrap();

    // Another instance holds the lock.
    let holder = std::fs::File::open(lock_path(&path)).unwrap();
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    sc.record_answer(SID2, "main");
    let err = sc
        .save_with_wait(&path, Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    assert!(err.to_string().contains("another tower"), "{err}");
    assert!(sc.is_dirty());
    assert!(on_disk(&path).owner(SID2).is_none(), "nothing written");
    assert!(!sc.retry_due(), "not before the retry delay");

    drop(holder);
    sc.save(&path).unwrap();
    assert!(!sc.is_dirty());
    assert_eq!(
        on_disk(&path).owner(SID2),
        Some(("main", &AccountSource::Asked))
    );
}

#[test]
fn merge_keeps_unknown_fields_and_the_file_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, DOC_EXAMPLE).unwrap();
    let mut a = Sidecar::load(&path).sidecar;

    // Another (newer) tower rewrites the file meanwhile.
    let mut other: serde_json::Value = serde_json::from_str(DOC_EXAMPLE).unwrap();
    other["version"] = 2.into();
    other["added_by_other"] = "x".into();
    other["sessions"][SID]["pinned_note"] = "changed".into();
    std::fs::write(&path, other.to_string()).unwrap();

    a.record_owner_at(SID2, "main", AccountSource::Asked, 5);
    a.save(&path).unwrap();
    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(disk["version"], 2);
    assert_eq!(disk["future_top_level"], serde_json::json!([1, 2, 3]));
    assert_eq!(disk["added_by_other"], "x");
    assert_eq!(disk["sessions"][SID]["pinned_note"], "changed");
    assert_eq!(disk["sessions"][SID2]["account"], "main");
    assert_eq!(disk["sessions"][SID2]["updated_ms"], 5);
}

#[test]
fn recent_projects_of_both_instances_are_kept() {
    let home = Path::new("/home/user");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut a = Sidecar::default();
    let mut b = Sidecar::default();
    a.touch_recent_project(Path::new("/home/user/workspace/a"), home, 10);
    b.touch_recent_project(Path::new("/home/user/workspace/b"), home, 20);
    a.save(&path).unwrap();
    b.save(&path).unwrap();
    assert_eq!(
        recent_paths(&on_disk(&path)),
        ["~/workspace/b", "~/workspace/a"]
    );
}

// ---- pruning (doc/0017 section 9) ---------------------------------------

const T0: i64 = 1_790_000_000_000;

#[test]
fn records_unseen_for_fourteen_days_are_pruned() {
    let mut sc = Sidecar::default();
    sc.record_owner_at(SID, "main", AccountSource::Proc, T0);
    sc.record_owner_at(SID2, "alt", AccountSource::Handoff, T0);
    assert!(sc.observe_listing([SID, SID2], true, T0));
    assert_eq!(sc.sessions[SID].last_seen_ms, Some(T0));
    // Seen again within the resolution: nothing to write.
    assert!(!sc.observe_listing([SID, SID2], true, T0 + SEEN_RESOLUTION_MS - 1));

    // SID2 keeps being listed; SID is gone.
    let later = T0 + PRUNE_UNSEEN_MS - 1;
    sc.observe_listing([SID2], true, later);
    assert!(sc.sessions.contains_key(SID), "not yet 14 days");
    let later = T0 + PRUNE_UNSEEN_MS;
    assert!(!sc.observe_listing([SID2], false, later));
    assert!(sc.sessions.contains_key(SID), "a poll failed: no pruning");
    assert!(sc.observe_listing([SID2], true, later));
    assert!(!sc.sessions.contains_key(SID));
    assert!(sc.sessions.contains_key(SID2));
}

#[test]
fn a_recent_owner_change_counts_as_activity() {
    let mut sc = Sidecar::default();
    sc.record_owner_at(SID, "main", AccountSource::Proc, T0);
    sc.observe_listing([SID], true, T0);
    sc.record_owner_at(SID, "alt", AccountSource::Asked, T0 + PRUNE_UNSEEN_MS);
    sc.observe_listing([], true, T0 + PRUNE_UNSEEN_MS + 1);
    assert!(sc.sessions.contains_key(SID));
}

#[test]
fn short_id_started_record_without_a_row_is_pruned_after_an_hour() {
    let mut sc = Sidecar::default();
    sc.record_owner_at("1a2b3c4d", "alt", AccountSource::Started, T0);
    // A dispatch whose row did appear, without a sessionId.
    sc.record_owner_at("5e6f7a8b", "main", AccountSource::Started, T0);
    sc.observe_listing(["5e6f7a8b"], true, T0 + 1);

    sc.observe_listing([], true, T0 + PRUNE_UNSTARTED_MS - 1);
    assert!(sc.sessions.contains_key("1a2b3c4d"));
    sc.observe_listing([], false, T0 + PRUNE_UNSTARTED_MS);
    assert!(sc.sessions.contains_key("1a2b3c4d"), "a poll failed");
    assert!(sc.observe_listing([], true, T0 + PRUNE_UNSTARTED_MS));
    assert!(!sc.sessions.contains_key("1a2b3c4d"));
    assert!(sc.sessions.contains_key("5e6f7a8b"), "its row appeared");
}

#[test]
fn records_of_an_older_tower_get_an_age_first() {
    let mut sc: Sidecar = serde_json::from_str(
        r#"{ "sessions": { "1a2b3c4d": { "account": "alt", "account_source": "started" } } }"#,
    )
    .unwrap();
    assert!(sc.observe_listing([], true, T0));
    assert_eq!(sc.sessions["1a2b3c4d"].updated_ms, Some(T0));
    assert!(!sc.observe_listing([], true, T0 + PRUNE_UNSTARTED_MS - 1));
    assert!(sc.observe_listing([], true, T0 + PRUNE_UNSTARTED_MS));
    assert!(sc.sessions.is_empty());
}

#[test]
fn pruning_survives_the_merge_but_yields_to_a_newer_sighting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut first = Sidecar::default();
    first.record_owner_at(SID, "main", AccountSource::Proc, T0);
    first.record_owner_at(SID2, "main", AccountSource::Proc, T0);
    first.save(&path).unwrap();
    let mut a = Sidecar::load(&path).sidecar;
    let mut b = Sidecar::load(&path).sidecar;

    // Instance 1 prunes both. Instance 2 saw SID2 again and saved first.
    b.observe_listing([SID2], false, T0 + PRUNE_UNSEEN_MS);
    b.save(&path).unwrap();
    assert!(a.observe_listing([], true, T0 + PRUNE_UNSEEN_MS));
    assert!(a.sessions.is_empty());
    a.save(&path).unwrap();
    let disk = on_disk(&path);
    assert!(!disk.sessions.contains_key(SID), "pruned");
    assert!(
        disk.sessions.contains_key(SID2),
        "seen by the other instance"
    );

    // Instance 2's next save does not bring the pruned record back.
    b.record_answer(SID3, "alt");
    b.save(&path).unwrap();
    let disk = on_disk(&path);
    assert!(!disk.sessions.contains_key(SID));
    assert!(disk.sessions.contains_key(SID3));
}

mod common;

#[test]
fn app_prunes_only_when_every_account_polled_fine() {
    use watchtower::app::Effect;
    let mut sc = Sidecar::default();
    // Not seen since 1970: well past 14 days.
    sc.record_owner_at(SID, "main", AccountSource::Asked, 0);
    let mut app = common::app_two_with(sc);
    app.apply_poll(common::poll("main", "mixed-states.json"));
    app.apply_poll(common::poll_err("alt", "timed out"));
    assert!(app.sidecar.sessions.contains_key(SID), "alt failed");
    let effects = app.apply_poll(common::poll("alt", "mixed-states.json"));
    assert!(!app.sidecar.sessions.contains_key(SID));
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
}
