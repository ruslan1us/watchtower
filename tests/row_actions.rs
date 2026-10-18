//! Stop, remove and the logs panel as a state machine (doc/0009, doc/0011,
//! doc/0015): confirmations, the effects with the owning account, the
//! refusal window, and when the logs panel fetches.

mod common;

use common::{app_single, app_two, ch, ctrl, fixture_path, key, poll, poll_err, status_text};
use ratatui::crossterm::event::KeyCode;
use watchtower::actions::{RemoveError, StopError};
use watchtower::app::{
    App, Confirm, Effect, LogsTarget, LogsView, NOTE_INTERACTIVE, NOTE_STOPPED, NOTE_UNKNOWN_OWNER,
    Overlay, Purpose, SessionAction,
};
use watchtower::listing::{Listing, parse_listing};
use watchtower::logview::{self, LogsError};
use watchtower::poller::PollUpdate;

fn select(app: &mut App, name: &str) {
    app.handle_key(key(KeyCode::Home));
    for _ in 0..app.sessions().len() {
        if app
            .selected_session()
            .map(|m| m.session.display_name())
            .as_deref()
            == Some(name)
        {
            return;
        }
        app.handle_key(ch('j'));
    }
    panic!("no row {name}");
}

fn stop_of(effects: &[Effect]) -> Option<(&str, &str)> {
    effects.iter().find_map(|e| match e {
        Effect::Stop(a) => Some((a.account.name.as_str(), a.id.as_str())),
        _ => None,
    })
}

fn remove_of(effects: &[Effect]) -> Option<(&str, &str)> {
    effects.iter().find_map(|e| match e {
        Effect::Remove(a) => Some((a.account.name.as_str(), a.id.as_str())),
        _ => None,
    })
}

fn fetch_of(effects: &[Effect]) -> Option<(String, String)> {
    effects.iter().find_map(|e| match e {
        Effect::FetchLogs(t) => Some((t.account.name.clone(), t.id.clone())),
        _ => None,
    })
}

fn action(app: &App, name: &str, id: &str) -> SessionAction {
    SessionAction {
        account: app.config().accounts[0].clone(),
        id: id.into(),
        name: name.into(),
    }
}

/// Two accounts with separate lists: `main` lists everything, `alt`
/// nothing, so every row's owner is `main` by rule 2.
fn app_two_separate(fixture: &str) -> App {
    let mut app = app_two();
    app.apply_poll(PollUpdate {
        account: "alt".into(),
        result: Ok(parse_listing("[]").unwrap()),
    });
    app.apply_poll(poll("main", fixture));
    app
}

// ---- stop ------------------------------------------------------------------

#[test]
fn stop_done_or_stopped_rows_directly() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select(&mut app, "say-ok");
    let effects = app.handle_key(ch('s'));
    assert_eq!(stop_of(&effects), Some(("main", "b2f8d064")));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("stopping say-ok"));

    // A row without a live process (state kept as `done`) also stops directly.
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    assert!(app.selected_session().unwrap().session.pid.is_none());
    assert_eq!(
        stop_of(&app.handle_key(ch('s'))),
        Some(("main", "b2f8d064"))
    );
}

#[test]
fn stop_working_or_blocked_rows_asks_first() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    for (name, id) in [("pick-colour", "9c6e1a35"), ("run-test-suite", "5a7c2e91")] {
        select(&mut app, name);
        assert!(app.handle_key(ch('s')).is_empty(), "{name}: asks first");
        let Overlay::Confirm(Confirm::Stop { action, detail }) = app.overlay().clone() else {
            panic!("{name}: {:?}", app.overlay())
        };
        assert_eq!(action.id, id);
        assert!(!detail.is_empty());
        // n, Esc and Enter (the default) cancel; other keys are ignored.
        for cancel in [ch('n'), key(KeyCode::Esc), key(KeyCode::Enter)] {
            assert!(app.handle_key(cancel).is_empty());
            assert_eq!(app.overlay(), &Overlay::None);
            assert!(status_text(&app).contains("stop cancelled"));
            app.handle_key(ch('s'));
        }
        assert!(app.handle_key(ch('x')).is_empty());
        assert!(matches!(app.overlay(), Overlay::Confirm(_)));
        let effects = app.handle_key(ch('y'));
        assert_eq!(stop_of(&effects), Some(("main", id)));
        assert_eq!(app.overlay(), &Overlay::None);
    }
}

#[test]
fn interactive_rows_are_never_stopped_or_removed() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select(&mut app, "demo-app-3f");
    assert!(app.handle_key(ch('s')).is_empty());
    assert!(status_text(&app).contains("tower cannot stop it"));
    assert!(app.handle_key(ch('d')).is_empty());
    assert!(status_text(&app).contains("tower cannot remove it"));
    assert_eq!(app.overlay(), &Overlay::None);
}

#[test]
fn unknown_owner_is_asked_before_stop() {
    let mut app = app_two();
    // Shared lists, unreadable environ: owners unknown.
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    select(&mut app, "touch-config");
    assert!(app.handle_key(ch('s')).is_empty());
    let Overlay::Picker(p) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    assert_eq!(p.purpose, Purpose::Stop);
    app.handle_key(ch('j'));
    // The answer is stored, then the usual confirmation for a blocked row.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(effects, [Effect::SaveSidecar]);
    let Overlay::Confirm(Confirm::Stop { action, .. }) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    assert_eq!(action.account.name, "alt");
    assert_eq!(stop_of(&app.handle_key(ch('y'))), Some(("alt", "d4a1f7c8")));
    assert_eq!(app.selected_session().unwrap().owner.label(), "alt");
}

#[test]
fn stop_outcomes_are_reported_and_poll() {
    let mut app = app_single();
    let a = action(&app, "say-ok", "b2f8d064");
    assert_eq!(app.stop_finished(a.clone(), Ok(())), [Effect::PollNow]);
    assert!(status_text(&app).contains("stopped say-ok (b2f8d064)"));
    let effects = app.stop_finished(
        a,
        Err(StopError::UnknownId {
            message: "No job matching 'b2f8d064'. Run 'claude agents' to list running sessions."
                .into(),
        }),
    );
    assert_eq!(effects, [Effect::PollNow]);
    assert!(status_text(&app).contains("cannot stop say-ok: No job matching"));
}

// ---- remove ----------------------------------------------------------------

#[test]
fn remove_always_confirms_with_the_owning_account() {
    let mut app = app_two_separate("mixed-states.json");
    select(&mut app, "say-ok");
    assert!(
        app.handle_key(ch('d')).is_empty(),
        "asks first, even when done"
    );
    let Overlay::Confirm(Confirm::Remove { action }) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    assert_eq!(
        (action.account.name.as_str(), action.id.as_str()),
        ("main", "b2f8d064")
    );
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("remove cancelled"));
    app.handle_key(ch('d'));
    let effects = app.handle_key(ch('Y'));
    assert_eq!(remove_of(&effects), Some(("main", "b2f8d064")));
    assert!(status_text(&app).contains("removing say-ok"));
    assert_eq!(app.remove_finished(action, Ok(())), [Effect::PollNow]);
    assert!(status_text(&app).contains("removed say-ok (b2f8d064)"));
}

#[test]
fn unknown_owner_is_asked_before_remove() {
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    select(&mut app, "say-ok");
    app.handle_key(ch('d'));
    let Overlay::Picker(p) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    assert_eq!(p.purpose, Purpose::Remove);
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Remove { .. })
    ));
    assert_eq!(
        remove_of(&app.handle_key(ch('y'))),
        Some(("main", "b2f8d064"))
    );
}

#[test]
fn refusal_shows_the_full_text_and_is_never_retried() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    let a = action(&app, "say-ok", "b2f8d064");
    let text = "Refusing to remove the worktree of b2f8d064: 2 unpushed commits.\n\
                Run again with --discard-unpushed abc1234@/home/user/workspace/demo-app/.claude/worktrees/b2f8d064";
    let effects = app.remove_finished(
        a,
        Err(RemoveError::Refused {
            code: Some(1),
            message: text.into(),
        }),
    );
    assert_eq!(effects, [Effect::PollNow], "no retry, no flags");
    let popup = app.popup().expect("refusal window").clone();
    let all = popup.lines.join("\n");
    assert!(all.contains("2 unpushed commits"), "{all}");
    assert!(all.contains("--discard-unpushed abc1234@"), "{all}");
    assert!(all.contains("exit code 1"), "{all}");
    assert!(all.contains("never passes"), "{all}");

    // It is on top of everything: list keys only scroll it.
    assert!(app.handle_key(ch('d')).is_empty());
    assert!(app.handle_key(ch('s')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(ch('j'));
    assert_eq!(app.popup().unwrap().scroll, 2);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.popup().unwrap().scroll, 1);
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.popup().unwrap().scroll, 0);
    assert_eq!(app.handle_key(ctrl('c')), [Effect::Quit]);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.popup().is_none());
}

#[test]
fn other_remove_errors_go_to_the_status_line() {
    let mut app = app_single();
    let a = action(&app, "gone", "0000aaaa");
    let effects = app.remove_finished(
        a,
        Err(RemoveError::UnknownId {
            message: "No job matching '0000aaaa'.".into(),
        }),
    );
    assert_eq!(effects, [Effect::PollNow]);
    assert!(app.popup().is_none());
    assert!(status_text(&app).contains("cannot remove gone: No job matching"));
}

// ---- logs panel ------------------------------------------------------------

fn screen(name: &str) -> logview::LogScreen {
    logview::render(&std::fs::read(fixture_path(&format!("logs/{name}.ansi"))).unwrap())
}

fn view(app: &App) -> LogsView {
    app.logs().expect("panel open").view.clone()
}

#[test]
fn space_fetches_the_selected_live_row_with_its_owner() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select(&mut app, "print-date");
    let effects = app.handle_key(ch(' '));
    assert_eq!(fetch_of(&effects), Some(("main".into(), "7b4d9f02".into())));
    assert_eq!(view(&app), LogsView::Loading);
    assert!(app.logs().unwrap().title.contains("print-date (main)"));

    let target = LogsTarget {
        account: app.config().accounts[0].clone(),
        id: "7b4d9f02".into(),
    };
    let effects = app.logs_fetched(target, Ok(screen("done")));
    assert!(effects.is_empty());
    assert!(matches!(view(&app), LogsView::Screen(_)));

    // Space again closes it; so does Esc.
    assert!(app.handle_key(ch(' ')).is_empty());
    assert!(app.logs().is_none());
    app.handle_key(ch(' '));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.logs().is_none());
}

#[test]
fn notes_instead_of_fetches() {
    // Stopped row: no `logs` call (it would only fail).
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch(' ')).is_empty());
    assert_eq!(view(&app), LogsView::Note(NOTE_STOPPED.into()));

    // Interactive row.
    select(&mut app, "demo-app-3f");
    assert_eq!(view(&app), LogsView::Note(NOTE_INTERACTIVE.into()));

    // Unknown owner.
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    select(&mut app, "touch-config");
    assert!(app.handle_key(ch(' ')).is_empty());
    assert_eq!(view(&app), LogsView::Note(NOTE_UNKNOWN_OWNER.into()));
}

#[test]
fn selection_change_fetches_the_new_row_one_at_a_time() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select(&mut app, "pick-colour");
    let effects = app.handle_key(ch(' '));
    assert_eq!(fetch_of(&effects).unwrap().1, "9c6e1a35");
    let first = app.logs().unwrap().in_flight().cloned().unwrap();

    // Moving while the first fetch runs: no second process yet.
    assert!(app.handle_key(ch('j')).is_empty());
    assert_eq!(app.logs().unwrap().target().unwrap().id, "3e9a1c07");
    assert_eq!(view(&app), LogsView::Loading);

    // The stale result is dropped and the current row is fetched.
    let effects = app.logs_fetched(first, Ok(screen("permission-prompt")));
    assert_eq!(fetch_of(&effects).unwrap().1, "3e9a1c07");
    assert_eq!(view(&app), LogsView::Loading);

    let second = app.logs().unwrap().in_flight().cloned().unwrap();
    let effects = app.logs_fetched(second, Ok(screen("failed")));
    assert!(effects.is_empty());
    assert!(matches!(view(&app), LogsView::Screen(_)));

    // Moving onto a stopped-style note row and back fetches again.
    select(&mut app, "demo-app-3f");
    assert_eq!(view(&app), LogsView::Note(NOTE_INTERACTIVE.into()));
    app.handle_key(key(KeyCode::Up));
    assert!(app.logs().unwrap().in_flight().is_some());
}

#[test]
fn every_poll_of_the_owner_refreshes_the_panel() {
    let mut app = app_two_separate("mixed-states.json");
    select(&mut app, "print-date");
    let effects = app.handle_key(ch(' '));
    let target = app.logs().unwrap().in_flight().cloned().unwrap();
    assert_eq!(fetch_of(&effects), Some(("main".into(), "7b4d9f02".into())));
    app.logs_fetched(target.clone(), Ok(screen("done")));

    // The owner's poll refreshes; another account's poll does not.
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(fetch_of(&effects), Some(("main".into(), "7b4d9f02".into())));
    app.logs_fetched(target.clone(), Ok(screen("done")));
    let effects = app.apply_poll(PollUpdate {
        account: "alt".into(),
        result: Ok(Listing::default()),
    });
    assert_eq!(fetch_of(&effects), None);
    // A failed poll of the owner still counts as a tick.
    let effects = app.apply_poll(poll_err("main", "timed out"));
    assert!(fetch_of(&effects).is_some());

    // A tick while a fetch runs is remembered, not run in parallel.
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(fetch_of(&effects), None);
    let effects = app.logs_fetched(target.clone(), Ok(screen("done")));
    assert!(fetch_of(&effects).is_some(), "pending refresh issued");

    // Nothing is fetched while the panel is closed.
    app.logs_fetched(target, Ok(screen("done")));
    app.handle_key(ch(' '));
    assert!(fetch_of(&app.apply_poll(poll("main", "mixed-states.json"))).is_none());
}

#[test]
fn fetch_errors_become_notes() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select(&mut app, "say-ok");
    app.handle_key(ch(' '));
    let t = app.logs().unwrap().in_flight().cloned().unwrap();
    app.logs_fetched(t.clone(), Err(LogsError::NotRunning));
    assert_eq!(view(&app), LogsView::Note(NOTE_STOPPED.into()));
    app.apply_poll(poll("main", "mixed-states.json"));
    app.logs_fetched(t.clone(), Err(LogsError::UnknownSession));
    assert!(matches!(view(&app), LogsView::Note(n) if n.contains("does not host")));
    app.apply_poll(poll("main", "mixed-states.json"));
    app.logs_fetched(
        t,
        Err(LogsError::Failed {
            code: Some(2),
            stderr: "boom".into(),
        }),
    );
    assert!(matches!(view(&app), LogsView::Error(e) if e.contains("boom")));
}
