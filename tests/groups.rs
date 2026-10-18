//! Group mapping and detail labels (doc/0006).

mod common;

use common::{by_name, listing};
use watchtower::groups::{Group, classify, detail, group_by_state};
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
    assert_eq!(detail(&blocked_idle), "blocked (no reason given)");

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
