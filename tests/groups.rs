//! Group mapping and detail labels (doc/0006).

mod common;

use common::{by_name, listing};
use std::path::Path;

use watchtower::groups::{
    Group, GroupKey, Grouping, classify, detail, group_by_state, group_by_state_with, group_with,
    state_group,
};
use watchtower::listing::{Session, parse_listing};

fn row(json: &str) -> Session {
    let l = parse_listing(&format!("[{json}]")).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    l.sessions.into_iter().next().unwrap()
}

#[test]
fn fixture_rows_map_to_groups() {
    let l = listing("after-stop-all.json");
    let expect = [
        ("demo-app-3f", Group::Interactive, "busy"),
        ("bad-model", Group::Failed, "failed"),
        ("run-test-suite", Group::ReadyForReview, "done"),
        ("print-date", Group::ReadyForReview, "done"),
        ("pick-colour", Group::NeedsMe, "input needed"),
        ("say-ok", Group::Stopped, "stopped"),
        ("touch-config", Group::NeedsMe, "permission prompt"),
    ];
    for (name, group, label) in expect {
        let s = by_name(&l.sessions, name);
        assert_eq!(classify(s), group, "{name}");
        assert_eq!(detail(s), label, "{name}");
    }
    let mixed = listing("mixed-states.json");
    let s = by_name(&mixed.sessions, "run-test-suite");
    assert_eq!(
        (classify(s), detail(s).as_str()),
        (Group::Working, "working")
    );
}

#[test]
fn stopped_rule_comes_first() {
    // No pid and no status: Stopped & finished whatever the state.
    for state in ["done", "failed", "blocked", "working", "paused"] {
        let s = row(&format!(
            r#"{{ "kind": "background", "id": "1a2b3c4d", "state": "{state}" }}"#
        ));
        assert_eq!(classify(&s), Group::Stopped, "{state}");
        assert_eq!(detail(&s), "stopped");
    }
    // state: stopped with a live process also lands there.
    let s = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "idle", "state": "stopped" }"#,
    );
    assert_eq!(classify(&s), Group::Stopped);
}

#[test]
fn needs_me_variants() {
    let blocked_idle = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "idle", "state": "blocked" }"#,
    );
    assert_eq!(classify(&blocked_idle), Group::NeedsMe);
    // What Claude Code reports when a turn ended with a question.
    assert_eq!(detail(&blocked_idle), "waiting for your reply");

    let waiting_working = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "waiting", "waitingFor": "sandbox request", "state": "working" }"#,
    );
    assert_eq!(classify(&waiting_working), Group::NeedsMe);
    assert_eq!(detail(&waiting_working), "sandbox request");

    for w in ["worker request", "dialog open", "something new"] {
        let s = row(&format!(
            r#"{{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "waiting", "waitingFor": "{w}", "state": "blocked" }}"#
        ));
        assert_eq!((classify(&s), detail(&s)), (Group::NeedsMe, w.to_owned()));
    }
}

#[test]
fn working_detail_follows_status() {
    let woken = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "idle", "state": "working" }"#,
    );
    assert_eq!(
        (classify(&woken), detail(&woken).as_str()),
        (Group::Working, "idle")
    );
}

#[test]
fn unknown_state_goes_to_other_with_raw_value() {
    let s = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "busy", "state": "paused" }"#,
    );
    assert_eq!(
        (classify(&s), detail(&s).as_str()),
        (Group::Other, "paused")
    );
    let s = row(r#"{ "kind": "background", "id": "1a2b3c4d", "pid": 1, "status": "busy" }"#);
    assert_eq!(classify(&s), Group::Other);
}

#[test]
fn interactive_rows_even_without_process() {
    let s = row(r#"{ "kind": "interactive", "cwd": "/home/user/x" }"#);
    assert_eq!(classify(&s), Group::Interactive);
    assert_eq!(detail(&s), "interactive");
}

#[test]
fn groups_in_order_non_empty_sorted_by_started_at() {
    let l = listing("after-stop-all.json");
    let grouped = group_by_state(&l.sessions);
    let order: Vec<Group> = grouped.iter().map(|g| g.group).collect();
    assert_eq!(
        order,
        [
            Group::NeedsMe,
            Group::Failed,
            Group::ReadyForReview,
            Group::Stopped,
            Group::Interactive
        ]
    );
    let names = |g: Group| -> Vec<String> {
        grouped
            .iter()
            .find(|x| x.group == g)
            .unwrap()
            .rows
            .iter()
            .map(|s| s.display_name())
            .collect()
    };
    // Most recent startedAt first.
    assert_eq!(names(Group::NeedsMe), ["touch-config", "pick-colour"]);
    assert_eq!(
        names(Group::ReadyForReview),
        ["print-date", "run-test-suite"]
    );
    assert!(!Group::Stopped.default_expanded());
    assert!(Group::NeedsMe.default_expanded());
    assert_eq!(Group::Stopped.title(), "Stopped & finished");
}

#[test]
fn rows_sort_by_last_change_then_name() {
    let l = listing("after-stop-all.json");
    // Tracked change times: pick-colour changed last; the two done rows
    // tie and fall back to their names; a row without a time sorts last.
    let time = |s: &Session| match s.name.as_deref() {
        Some("pick-colour") => Some(9_000),
        Some("touch-config") => Some(5_000),
        Some("run-test-suite" | "print-date") => Some(7_000),
        _ => None,
    };
    let grouped = group_by_state_with(&l.sessions, |s| time(s));
    let names = |g: Group| -> Vec<String> {
        grouped
            .iter()
            .find(|x| x.group == g)
            .unwrap()
            .rows
            .iter()
            .map(|s| s.display_name())
            .collect()
    };
    assert_eq!(names(Group::NeedsMe), ["pick-colour", "touch-config"]);
    assert_eq!(
        names(Group::ReadyForReview),
        ["print-date", "run-test-suite"]
    );
}

/// An orchestrator row: `extra` holds its state fields.
fn orch(id: &str, extra: &str) -> Session {
    row(&format!(
        r#"{{ "kind": "background", "id": "{id}", "name": "orchestrator",
             "cwd": "/home/user/.config/watchtower/orchestrator", {extra} }}"#
    ))
}

#[test]
fn the_orchestrator_has_its_own_group_and_plain_details() {
    let cases = [
        // A turn that ended with a question: ready, not "Needs me".
        (
            r#""pid": 1, "status": "idle", "state": "blocked", "startedAt": 1"#,
            Group::NeedsMe,
            "ready",
        ),
        (
            r#""pid": 1, "status": "idle", "state": "done", "startedAt": 1"#,
            Group::ReadyForReview,
            "ready",
        ),
        (
            r#""pid": 1, "status": "busy", "state": "working", "startedAt": 1"#,
            Group::Working,
            "working",
        ),
        (
            r#""pid": 1, "status": "waiting", "waitingFor": "permission prompt", "state": "blocked", "startedAt": 1"#,
            Group::NeedsMe,
            "permission prompt",
        ),
        (
            r#""pid": 1, "status": "idle", "state": "failed", "startedAt": 1"#,
            Group::Failed,
            "failed",
        ),
        (
            r#""state": "done", "startedAt": 1"#,
            Group::Stopped,
            "stopped",
        ),
    ];
    for (fields, state, text) in cases {
        let s = orch("0c0c0c0c", fields);
        assert_eq!(classify(&s), Group::Orchestrator, "{fields}");
        assert_eq!(state_group(&s), state, "{fields}");
        assert_eq!(detail(&s), text, "{fields}");
    }
    assert_eq!(Group::Orchestrator.title(), "Orchestrator");
    // Only the exact name, and never an interactive row.
    let other = row(
        r#"{ "kind": "background", "id": "1a2b3c4d", "name": "orchestrator-old", "pid": 1, "status": "idle", "state": "blocked" }"#,
    );
    assert_eq!(classify(&other), Group::NeedsMe);
    let interactive = row(r#"{ "kind": "interactive", "name": "orchestrator", "status": "idle" }"#);
    assert_eq!(classify(&interactive), Group::Interactive);
}

#[test]
fn the_orchestrator_group_comes_first_in_both_modes() {
    let mut rows = listing("after-stop-all.json").sessions;
    // Three orchestrators: a stopped recent one, an older live one, the
    // most recent live one. `o` takes live first, then the most recent.
    rows.push(orch(
        "11111111",
        r#""state": "done", "startedAt": 9000000000000"#,
    ));
    rows.push(orch(
        "22222222",
        r#""pid": 7, "status": "idle", "state": "blocked", "startedAt": 10"#,
    ));
    rows.push(orch(
        "33333333",
        r#""pid": 8, "status": "busy", "state": "working", "startedAt": 20"#,
    ));
    let ids =
        |rows: &[&Session]| -> Vec<String> { rows.iter().map(|s| s.id.clone().unwrap()).collect() };
    let expected = ["33333333", "22222222", "11111111"];

    let by_state = group_by_state(&rows);
    assert_eq!(by_state[0].group, Group::Orchestrator);
    assert_eq!(ids(&by_state[0].rows), expected);
    assert_eq!(by_state[1].group, Group::NeedsMe);
    assert!(by_state.iter().skip(1).all(|g| {
        g.rows
            .iter()
            .all(|s| s.name.as_deref() != Some("orchestrator"))
    }));

    let by_project = group_with(Grouping::Project, &rows, Path::new("/home/user"), |s| {
        s.started_at
    });
    assert_eq!(by_project[0].key, GroupKey::State(Group::Orchestrator));
    assert_eq!(by_project[0].key.title(), "Orchestrator");
    assert_eq!(ids(&by_project[0].rows), expected);
    assert!(
        by_project[1..]
            .iter()
            .all(|g| g.key != GroupKey::State(Group::Orchestrator))
    );
    assert!(by_project[1..].iter().all(|g| {
        g.rows
            .iter()
            .all(|s| s.name.as_deref() != Some("orchestrator"))
    }));
}
