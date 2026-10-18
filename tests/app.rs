//! The TUI state machine without a terminal (doc/0015): selection over
//! groups, keys, the account picker and attach requests.

mod common;

use common::{HOME, app_single, app_two, app_two_with, poll, poll_err};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use watchtower::app::{App, AttachOutcome, Effect, Entry, Overlay, Severity};
use watchtower::groups::Group;
use watchtower::sidecar::{AccountSource, Sidecar};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn press(app: &mut App, k: KeyEvent) -> Vec<Effect> {
    app.handle_key(k)
}

fn selected(app: &App) -> String {
    app.selected_session()
        .map(|m| m.session.display_name())
        .unwrap_or_default()
}

fn message(app: &App) -> String {
    let now = std::time::Instant::now();
    app.status_segments(now)
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn attach_of(effects: &[Effect]) -> Option<(&str, &str, &str)> {
    effects.iter().find_map(|e| match e {
        Effect::Attach(r) => Some((r.account.name.as_str(), r.id.as_str(), r.name.as_str())),
        _ => None,
    })
}

#[test]
fn groups_in_doc_order_with_counts() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    let headers: Vec<(Group, usize)> = app
        .entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Header { group, count } => Some((*group, *count)),
            Entry::Row(_) => None,
        })
        .collect();
    assert_eq!(
        headers,
        [
            (Group::NeedsMe, 2),
            (Group::Failed, 1),
            (Group::ReadyForReview, 2),
            (Group::Stopped, 1),
            (Group::Interactive, 1),
        ]
    );
}

#[test]
fn selection_skips_headers_and_clamps() {
    let mut app = app_single();
    assert!(app.selected_session().is_none());
    assert!(press(&mut app, key(KeyCode::Down)).is_empty(), "empty list");

    app.apply_poll(poll("main", "mixed-states.json"));
    // Needs me: pick-colour; Failed: bad-model; Ready: say-ok, print-date;
    // Working: run-test-suite; Interactive: demo-app-3f.
    assert_eq!(selected(&app), "pick-colour");
    press(&mut app, key(KeyCode::Up));
    assert_eq!(selected(&app), "pick-colour", "top clamps");
    press(&mut app, ch('j'));
    assert_eq!(selected(&app), "bad-model", "header skipped");
    press(&mut app, key(KeyCode::Down));
    assert_eq!(selected(&app), "say-ok");
    press(&mut app, ch('j'));
    assert_eq!(selected(&app), "print-date");
    press(&mut app, ch('k'));
    assert_eq!(selected(&app), "say-ok");
    press(&mut app, key(KeyCode::End));
    assert_eq!(selected(&app), "demo-app-3f");
    press(&mut app, key(KeyCode::Down));
    assert_eq!(selected(&app), "demo-app-3f", "bottom clamps");
    press(&mut app, ch('g'));
    assert_eq!(selected(&app), "pick-colour");
    press(
        &mut app,
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
    );
    assert_eq!(selected(&app), "demo-app-3f");
    press(&mut app, key(KeyCode::Home));
    assert_eq!(selected(&app), "pick-colour");

    app.page_size = 3;
    press(&mut app, key(KeyCode::PageDown));
    assert_eq!(selected(&app), "print-date");
    press(&mut app, key(KeyCode::PageDown));
    assert_eq!(selected(&app), "demo-app-3f");
    press(&mut app, key(KeyCode::PageUp));
    assert_eq!(selected(&app), "say-ok");

    // The selected entry is always a row.
    let e = app.selected_entry().unwrap();
    assert!(matches!(app.entries()[e], Entry::Row(_)));
}

#[test]
fn selection_follows_the_session_across_polls() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    press(&mut app, ch('j'));
    press(&mut app, ch('j'));
    press(&mut app, ch('j'));
    assert_eq!(selected(&app), "print-date");
    // touch-config appears in Needs me and say-ok moves to Stopped.
    app.apply_poll(poll("main", "after-stop-all.json"));
    assert_eq!(selected(&app), "print-date");
}

#[test]
fn enter_attaches_with_owning_account() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    let effects = press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        attach_of(&effects),
        Some(("main", "9c6e1a35", "pick-colour"))
    );
}

#[test]
fn stopped_row_attaches_by_id() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    while selected(&app) != "say-ok" {
        press(&mut app, ch('j'));
    }
    let effects = press(&mut app, key(KeyCode::Enter));
    assert_eq!(attach_of(&effects), Some(("main", "b2f8d064", "say-ok")));
}

#[test]
fn interactive_row_is_not_attached() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    press(&mut app, key(KeyCode::End));
    assert_eq!(selected(&app), "demo-app-3f");
    let effects = press(&mut app, key(KeyCode::Enter));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(message(&app).contains("cannot be attached"));
    assert_eq!(app.overlay(), &Overlay::None);
}

#[test]
fn unknown_owner_asks_records_and_attaches() {
    let mut app = app_two();
    // Shared lists, unreadable environ, empty sidecar: owners unknown.
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    assert_eq!(selected(&app), "touch-config");
    assert_eq!(app.selected_session().unwrap().owner.label(), "?");

    let effects = press(&mut app, key(KeyCode::Enter));
    assert!(effects.is_empty(), "nothing runs before the answer");
    let Overlay::Picker(p) = app.overlay().clone() else {
        panic!("picker expected, got {:?}", app.overlay());
    };
    assert_eq!(p.selected, 0, "default account preselected");
    assert_eq!(p.id, "d4a1f7c8");

    // Keys go to the picker, not the list.
    press(&mut app, ch('j'));
    press(&mut app, ch('j'));
    assert_eq!(selected(&app), "touch-config");
    let effects = press(&mut app, key(KeyCode::Enter));
    assert_eq!(effects[0], Effect::SaveSidecar, "saved before attaching");
    assert_eq!(
        attach_of(&effects),
        Some(("alt", "d4a1f7c8", "touch-config"))
    );
    assert_eq!(app.overlay(), &Overlay::None);
    let sid = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";
    let owner = app
        .sidecar
        .owner(sid)
        .map(|(a, s)| (a.to_owned(), s.clone()));
    assert_eq!(owner, Some(("alt".to_owned(), AccountSource::Asked)));
    assert_eq!(app.selected_session().unwrap().owner.label(), "alt");

    // The next poll keeps the answer (sidecar, rule 3); no second question.
    app.apply_poll(poll("main", "after-stop-all.json"));
    let effects = press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        attach_of(&effects),
        Some(("alt", "d4a1f7c8", "touch-config"))
    );
}

#[test]
fn picker_escape_cancels() {
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    press(&mut app, key(KeyCode::Enter));
    assert!(matches!(app.overlay(), Overlay::Picker(_)));
    press(&mut app, key(KeyCode::Down));
    let effects = press(&mut app, key(KeyCode::Esc));
    assert!(effects.is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(app.sidecar.sessions.values().all(|r| r.account.is_none()));
    assert!(message(&app).contains("cancelled"));
    // Ctrl+C still quits from the picker.
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(press(&mut app, ctrl('c')), [Effect::Quit]);
}

#[test]
fn sidecar_owner_attaches_without_asking() {
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "alt",
        AccountSource::Started,
    );
    let mut app = app_two_with(sidecar);
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    while selected(&app) != "say-ok" {
        press(&mut app, ch('j'));
    }
    let effects = press(&mut app, key(KeyCode::Enter));
    assert_eq!(attach_of(&effects), Some(("alt", "b2f8d064", "say-ok")));
}

#[test]
fn quit_help_refresh_and_reserved_keys() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(press(&mut app, ch('q')), [Effect::Quit]);
    assert_eq!(press(&mut app, ctrl('c')), [Effect::Quit]);
    assert_eq!(press(&mut app, ch('r')), [Effect::PollNow]);

    // Ctrl+Z is a no-op in tower (never suspends it) with a hint.
    assert!(press(&mut app, ctrl('z')).is_empty());
    assert!(message(&app).contains("Ctrl+Z"));

    // ← is Claude Code's key; tower leaves it unbound.
    let before = selected(&app);
    assert!(press(&mut app, key(KeyCode::Left)).is_empty());
    assert_eq!(selected(&app), before);

    // Keys of later milestones (doc/0014) only say so.
    for c in ['o', '/', 'p', 'i', 'm'] {
        assert!(press(&mut app, ch(c)).is_empty(), "{c}");
        assert!(message(&app).contains("not available yet"), "{c}");
    }

    // Help overlay: opens with ?, swallows list keys, closes with Esc.
    press(&mut app, ch('?'));
    assert_eq!(app.overlay(), &Overlay::Help);
    assert!(press(&mut app, key(KeyCode::Enter)).is_empty());
    assert_eq!(app.overlay(), &Overlay::None, "Enter closes help");
    press(&mut app, ch('?'));
    press(&mut app, ch('j'));
    assert_eq!(app.overlay(), &Overlay::Help);
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(app.overlay(), &Overlay::None);
    press(&mut app, ch('?'));
    assert_eq!(press(&mut app, ch('q')), [Effect::Quit]);

    // Key releases are ignored.
    let release = KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    assert!(press(&mut app, release).is_empty());
}

#[test]
fn failed_poll_keeps_last_rows_and_shows_error() {
    let mut app = app_single();
    let status = |app: &App| message(app);
    assert!(status(&app).contains("loading"));
    app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(app.sessions().len(), 6);
    app.apply_poll(poll_err(
        "main",
        "listing failed (exit 1):\nsomething broke",
    ));
    assert_eq!(app.sessions().len(), 6, "last good rows kept");
    let s = status(&app);
    assert!(
        s.contains("main: listing failed (exit 1): something broke"),
        "{s}"
    );
    assert!(s.contains("showing last good list"), "{s}");
    let errors: Vec<_> = app
        .status_segments(std::time::Instant::now())
        .into_iter()
        .filter(|s| s.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1);
    // A later success clears the error.
    app.apply_poll(poll("main", "mixed-states.json"));
    assert!(!status(&app).contains("listing failed"));
}

#[test]
fn failed_account_is_not_rule_two_evidence() {
    let mut app = app_two();
    let all = common::listing("after-stop-all.json").sessions;
    let (main_rows, alt_rows): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|s| s.name.as_deref() == Some("say-ok"));
    let update = |account: &str, rows| watchtower::poller::PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(watchtower::listing::Listing {
            sessions: rows,
            warnings: vec![],
        }),
    };
    app.apply_poll(update("main", main_rows));
    app.apply_poll(update("alt", alt_rows));
    let owner = |app: &App, name: &str| {
        app.sessions()
            .iter()
            .find(|m| m.session.display_name() == name)
            .unwrap()
            .owner
            .label()
            .to_owned()
    };
    assert_eq!(owner(&app, "say-ok"), "main", "separate lists: rule 2");
    assert_eq!(owner(&app, "touch-config"), "alt");
    // alt fails: its rows stay listed and say-ok keeps its recorded owner.
    app.apply_poll(poll_err("alt", "timed out"));
    assert_eq!(app.sessions().len(), 7);
    assert_eq!(owner(&app, "say-ok"), "main", "from the sidecar now");
}

#[test]
fn detections_are_saved_only_on_change() {
    let mut app = app_two();
    // Separate, non-empty lists from one poll cycle: rows resolve by rule 2.
    let all = common::listing("mixed-states.json").sessions;
    let (main_rows, alt_rows): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|s| s.name.as_deref() != Some("print-date"));
    let update = |account: &str, rows: &Vec<_>| watchtower::poller::PollUpdate {
        account: account.into(),
        cycle: 1,
        result: Ok(watchtower::listing::Listing {
            sessions: rows.clone(),
            warnings: vec![],
        }),
    };
    assert!(app.apply_poll(update("alt", &alt_rows)).is_empty());
    let effects = app.apply_poll(update("main", &main_rows));
    assert_eq!(effects, [Effect::SaveSidecar]);
    assert!(app.apply_poll(update("main", &main_rows)).is_empty());
}

#[test]
fn single_account_writes_no_implicit_owners() {
    let mut app = app_single();
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(app.sidecar.sessions.is_empty());
    // Every row still attaches with the only account.
    assert!(app.sessions().iter().all(|m| m.owner.label() == "main"));
}

#[test]
fn attach_outcome_is_reported_and_polls() {
    let mut app = app_single();
    let effects = app.attach_finished(AttachOutcome {
        name: "say-ok".into(),
        account: "main".into(),
        result: Ok(Some(1)),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert!(message(&app).contains("exited with code 1"));
    app.attach_finished(AttachOutcome {
        name: "say-ok".into(),
        account: "main".into(),
        result: Ok(Some(0)),
    });
    assert!(message(&app).contains("back from say-ok (main)"));
    app.attach_finished(AttachOutcome {
        name: "say-ok".into(),
        account: "main".into(),
        result: Err("cannot run `claude`: not found".into()),
    });
    assert!(message(&app).contains("cannot attach"));
}

#[test]
fn listing_warnings_and_fixture_home_shortening() {
    let mut app = app_single();
    app.apply_poll(watchtower::poller::PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(watchtower::listing::parse_listing(
            r#"[{"kind":"background","id":"1a2b3c4d","cwd":"/home/user/workspace/api","state":"done"}, 7]"#,
        )
        .unwrap()),
    });
    assert!(message(&app).contains("main: skipped listing row 1"));
    assert_eq!(app.home(), std::path::Path::new(HOME));
}
