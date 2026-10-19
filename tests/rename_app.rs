//! Renaming as a state machine (doc/0027, doc/0011, doc/0015): `e` and its
//! Cyrillic key open the window with the current name; the name is checked;
//! rows that cannot be renamed say why; a stopped row is renamed from tower
//! (the job, its progress, a copy, a name that never shows, the trust
//! retry, an unknown owner) without an attention highlight or a new
//! activity time; a running row is renamed through an attach, and the name
//! is compared once tower is back.

mod common;

use std::time::Instant;

use common::{
    app_two, app_two_with, ch, ctrl, fixture, key, poll, refresh_for_attach, status_text, type_text,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, AttachOutcome, Effect, Overlay, Purpose, RenameJob};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::rename::{RenameError, Renamed};
use watchtower::sidecar::{AccountSource, Sidecar};

const SAY_OK: &str = "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81";
const TOUCH: &str = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";
const PRINT_DATE: &str = "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93";

/// The `say-ok` row of `after-stop-all.json` (stopped), as the fixture has it.
const SAY_OK_STOPPED: &str = "\"name\": \"say-ok\",\n    \"state\": \"done\"";

/// `after-stop-all.json` with `from` replaced by `to`.
fn listing_with(from: &str, to: &str) -> String {
    let text = fixture("agents-json/after-stop-all.json");
    assert!(text.contains(from), "{from}");
    text.replace(from, to)
}

fn poll_text(account: &str, text: &str, cycle: u64) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle,
        result: Ok(parse_listing(text).unwrap()),
    }
}

fn poll_both_text(app: &mut App, text: &str) -> Vec<Effect> {
    let mut effects = app.apply_poll(poll_text("main", text, 0));
    effects.extend(app.apply_poll(poll_text("alt", text, 0)));
    effects
}

/// Two accounts with shared lists; `main` started `say-ok`, `touch-config`
/// and `print-date`.
fn app_owned() -> App {
    let mut sc = Sidecar::default();
    for sid in [SAY_OK, TOUCH, PRINT_DATE] {
        sc.record_owner(sid, "main", AccountSource::Started);
    }
    let mut app = app_two_with(sc);
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    app
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

/// `e` on `name`, the name field emptied and `new` typed.
fn type_new_name(app: &mut App, name: &str, new: &str) {
    select(app, name);
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_some(), "{}", status_text(app));
    app.handle_key(ctrl('u'));
    type_text(app, new);
}

/// `say-ok` renamed to `greet`: the job the window's `Enter` starts.
fn start_stopped_rename(app: &mut App) -> RenameJob {
    type_new_name(app, "say-ok", "greet");
    let effects = app.handle_key(key(KeyCode::Enter));
    match effects.as_slice() {
        [Effect::Rename(job)] => job.clone(),
        other => panic!("expected the rename job, got {other:?}"),
    }
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- the window --------------------------------------------------------------

#[test]
fn e_opens_the_window_with_the_current_name() {
    let mut app = app_owned();
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('e')).is_empty());
    let w = app.rename_window().expect("open");
    assert_eq!(w.name.text(), "say-ok");
    assert_eq!(w.name.cursor(), 6, "cursor at the end");
    assert_eq!(w.old_name, "say-ok");
    assert!(!w.running);
    assert_eq!(w.owner.as_deref(), Some("main"));
    // Esc closes it.
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.rename_window().is_none());
    assert!(status_text(&app).contains("rename cancelled"));
    // `у` is `e` on the RU/UA layout; inside, typed Cyrillic stays as is.
    app.handle_key(ch('у'));
    assert!(app.rename_window().is_some());
    type_text(&mut app, "ы");
    assert_eq!(app.rename_window().unwrap().name.text(), "say-okы");
    // The usual editing keys.
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Delete));
    assert_eq!(app.rename_window().unwrap().name.text(), "ay-okы");
    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("Rename session"), "{screen}");
    assert!(screen.contains("Enter rename"), "{screen}");
}

#[test]
fn the_name_is_checked() {
    let mut app = app_owned();
    type_new_name(&mut app, "say-ok", "   ");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let w = app.rename_window().expect("still open");
    assert!(w.error.as_deref().unwrap().contains("empty"), "{w:?}");

    app.handle_key(ctrl('u'));
    type_text(&mut app, "orchestrator");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let err = app.rename_window().unwrap().error.clone().unwrap();
    assert!(err.contains("orchestrator"), "{err}");

    app.handle_key(ctrl('u'));
    type_text(&mut app, &"x".repeat(81));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let err = app.rename_window().unwrap().error.clone().unwrap();
    assert!(err.contains("at most 80"), "{err}");

    // An unchanged name (after trimming) closes the window; nothing runs.
    app.handle_key(ctrl('u'));
    type_text(&mut app, " say-ok ");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(app.rename_window().is_none());
    assert!(status_text(&app).contains("say-ok keeps its name"));
}

// ---- rows that cannot be renamed -----------------------------------------------

#[test]
fn interactive_rows_and_the_orchestrator_are_not_renamed() {
    let mut app = app_owned();
    select(&mut app, "demo-app-3f");
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_none());
    assert!(
        status_text(&app).contains("rename it there with /rename"),
        "{}",
        status_text(&app)
    );

    let mut app = app_two();
    let text = listing_with("\"name\": \"print-date\"", "\"name\": \"orchestrator\"");
    poll_both_text(&mut app, &text);
    select(&mut app, "orchestrator");
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_none());
    let status = status_text(&app);
    assert!(
        status.contains("knows the orchestrator by its name"),
        "{status}"
    );
}

#[test]
fn a_row_in_a_hand_off_or_a_second_rename_is_refused() {
    let mut app = app_owned();
    select(&mut app, "print-date");
    app.handle_key(ch('a'));
    let effects = app.handle_key(ch('y'));
    assert!(matches!(effects.as_slice(), [Effect::Handoff(_)]));
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_none());
    assert!(status_text(&app).contains("being handed off"));

    let mut app = app_owned();
    start_stopped_rename(&mut app);
    // The row being renamed refuses every row action meanwhile.
    select(&mut app, "say-ok");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(status_text(&app).contains("say-ok is being renamed"));
    assert!(app.handle_key(ch('a')).is_empty());
    assert!(app.handle_key(ch('e')).is_empty());
    // And a second rename waits.
    select(&mut app, "print-date");
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_none());
    assert!(status_text(&app).contains("a rename is already running (say-ok)"));
}

// ---- a stopped row ------------------------------------------------------------

#[test]
fn a_stopped_row_is_renamed_with_its_owner() {
    let mut app = app_owned();
    let job = start_stopped_rename(&mut app);
    let r = &job.request;
    assert_eq!(r.id, "b2f8d064");
    assert_eq!(r.session_id, SAY_OK);
    assert_eq!(
        (r.old_name.as_str(), r.new_name.as_str()),
        ("say-ok", "greet")
    );
    assert_eq!(r.cwd.to_str(), Some("/home/user/workspace/demo-app"));
    assert_eq!(r.owner.name, "main");
    assert_eq!(r.owner.command, "claude");
    assert!(!job.after_trust);
    assert!(app.rename_window().is_none());
    assert!(
        status_text(&app).contains("renaming say-ok → greet: waking it under main"),
        "{}",
        status_text(&app)
    );
    app.rename_step(watchtower::rename::RenameStep::Confirming);
    assert!(status_text(&app).contains("renaming say-ok → greet: waiting for the new name"));

    let effects = app.rename_finished(job, Ok(Renamed { notes: Vec::new() }));
    assert!(effects.contains(&Effect::PollNow));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Stop(_) | Effect::Remove(_)))
    );
    let status = status_text(&app);
    assert!(status.contains("renamed say-ok → greet"), "{status}");
    assert!(!status.contains("renaming"), "{status}");
}

/// The wake and the stop for a rename are not activity (doc/0006,
/// doc/0007): no highlight while it runs, and once the row is stopped
/// again its time since the last change is what it was.
#[test]
fn the_rename_raises_no_highlight_and_keeps_the_activity_time() {
    let mut app = app_owned();
    let record = |app: &App| app.sidecar.sessions.get(SAY_OK).cloned().unwrap();
    let before = record(&app);
    let attention = app.attention_count();
    let job = start_stopped_rename(&mut app);

    // While it runs the row is live; even a blocked tuple is not
    // highlighted or recorded.
    let woken = listing_with(
        SAY_OK_STOPPED,
        "\"name\": \"greet\",\n    \"status\": \"waiting\",\n    \"waitingFor\": \"permission prompt\",\n    \"state\": \"blocked\"",
    );
    poll_both_text(&mut app, &woken);
    let row = app
        .sessions()
        .iter()
        .find(|m| m.session.key() == Some(SAY_OK))
        .unwrap();
    assert!(row.session.has_live_process());
    assert!(!app.is_highlighted(row));
    // The age column keeps the time from before the wake.
    assert_eq!(app.last_change_at(row, i64::MAX), before.last_change_ms);
    assert_eq!(app.attention_count(), attention);
    assert_eq!(record(&app).last_state, before.last_state);
    assert_eq!(record(&app).last_change_ms, before.last_change_ms);

    app.rename_finished(job, Ok(Renamed { notes: Vec::new() }));
    // Stopped again, listed `state: stopped` as observed after such a
    // stop: taken as the last tuple, without a new change time.
    let stopped = listing_with(
        SAY_OK_STOPPED,
        "\"name\": \"greet\",\n    \"state\": \"stopped\"",
    );
    poll_both_text(&mut app, &stopped);
    let after = record(&app);
    assert_eq!(
        after.last_state.as_ref().and_then(|t| t.state.clone()),
        Some(watchtower::listing::State::Stopped)
    );
    assert_eq!(after.last_change_ms, before.last_change_ms);
    assert_eq!(app.attention_count(), attention);
    // From now on its changes count again.
    let later = listing_with(
        SAY_OK_STOPPED,
        "\"name\": \"greet\",\n    \"status\": \"waiting\",\n    \"waitingFor\": \"permission prompt\",\n    \"state\": \"blocked\"",
    );
    poll_both_text(&mut app, &later);
    assert_eq!(app.attention_count(), attention + 1);
    assert_ne!(record(&app).last_state, after.last_state);
}

#[test]
fn a_copy_is_reported_and_left_alone() {
    let mut app = app_owned();
    let job = start_stopped_rename(&mut app);
    let effects = app.rename_finished(
        job,
        Err(RenameError::Copied {
            new_id: "0c0d0e0f".into(),
            notes: vec!["note: started a copy of that conversation as 0c0d0e0f.".into()],
        }),
    );
    assert!(effects.contains(&Effect::SaveSidecar));
    assert!(effects.contains(&Effect::PollNow));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Stop(_) | Effect::Remove(_)))
    );
    let status = status_text(&app);
    assert!(
        status.contains("started a copy 0c0d0e0f of say-ok"),
        "{status}"
    );
    assert!(
        status.contains("nothing was stopped or removed"),
        "{status}"
    );
    // The copy runs under the owner's command.
    assert_eq!(
        app.sidecar.owner("0c0d0e0f"),
        Some(("main", &AccountSource::Started))
    );
}

#[test]
fn a_name_that_never_shows_leaves_the_session_running() {
    let mut app = app_owned();
    let job = start_stopped_rename(&mut app);
    app.rename_finished(
        job,
        Err(RenameError::NotConfirmed {
            waited: std::time::Duration::from_secs(10),
            last_name: Some("say-ok".into()),
            last_error: None,
        }),
    );
    let status = status_text(&app);
    assert!(
        status.contains("rename of say-ok to greet not confirmed"),
        "{status}"
    );
    assert!(status.contains("left running"), "{status}");
    // It is an ordinary running session again: its states count.
    let blocked = listing_with(
        SAY_OK_STOPPED,
        "\"name\": \"say-ok\",\n    \"status\": \"waiting\",\n    \"waitingFor\": \"permission prompt\",\n    \"state\": \"blocked\"",
    );
    let attention = app.attention_count();
    poll_both_text(&mut app, &blocked);
    assert_eq!(app.attention_count(), attention + 1);
}

#[test]
fn an_untrusted_folder_offers_the_trust_flow_and_retries_once() {
    let mut app = app_owned();
    let job = start_stopped_rename(&mut app);
    let untrusted = || RenameError::Untrusted {
        dir: "/home/user/workspace/demo-app".into(),
        message: "Workspace not trusted. Run `claude` there once and accept the trust prompt, \
                  then retry."
            .into(),
    };
    app.rename_finished(job.clone(), Err(untrusted()));
    assert!(matches!(app.overlay(), Overlay::RenameUntrusted(_)));
    assert!(status_text(&app).contains("main has not trusted say-ok's folder; Ctrl+T"));
    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("Not trusted by main"), "{screen}");
    assert!(screen.contains("stays stopped"), "{screen}");

    let effects = app.handle_key(ctrl('t'));
    let [Effect::RenameTrust(trust)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(trust.request, job.request);
    let effects = app.rename_trust_finished(trust.clone(), Ok(Some(0)));
    let [Effect::Rename(retry)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert!(retry.after_trust);
    assert!(status_text(&app).contains("renaming say-ok → greet"));

    // Still untrusted: the window says so; Esc leaves it as it was.
    app.rename_finished(retry.clone(), Err(untrusted()));
    let Overlay::RenameUntrusted(u) = app.overlay().clone() else {
        panic!("trust window")
    };
    assert!(u.message.contains("still not trusted"), "{}", u.message);
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(status_text(&app).contains("say-ok stays stopped as it was"));
}

#[test]
fn an_unknown_owner_is_asked_for_and_recorded() {
    // Shared lists and no records: every owner is unknown (doc/0017).
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    type_new_name(&mut app, "say-ok", "greet");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let Overlay::Picker(picker) = app.overlay().clone() else {
        panic!("the account picker: {:?}", app.overlay())
    };
    assert_eq!(picker.purpose, Purpose::Rename);
    assert_eq!(picker.rename_to.as_deref(), Some("greet"));
    let screen = render(&mut app, 120, 40);
    assert!(
        screen.contains("Enter rename with this account"),
        "{screen}"
    );
    assert!(screen.contains("moves the session to it"), "{screen}");

    app.handle_key(key(KeyCode::Down));
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
    let job = effects
        .iter()
        .find_map(|e| match e {
            Effect::Rename(job) => Some(job.clone()),
            _ => None,
        })
        .expect("the rename starts");
    assert_eq!(job.request.owner.name, "alt");
    assert_eq!(job.request.new_name, "greet");
    assert_eq!(
        app.sidecar.owner(SAY_OK),
        Some(("alt", &AccountSource::Asked))
    );
}

// ---- a running row ------------------------------------------------------------

/// `e` on the running `print-date`, `new-date` typed, `Enter`: the attach
/// after its refresh, and the attach's return.
fn rename_running(app: &mut App, exit: i32) -> Vec<Effect> {
    type_new_name(app, "print-date", "new-date");
    let w = app.rename_window().unwrap();
    assert!(w.running);
    let screen = render(app, 120, 40);
    assert!(screen.contains("/rename new-date"), "{screen}");
    assert!(screen.contains("Ctrl+Z to come back"), "{screen}");
    assert!(screen.contains("Enter attach"), "{screen}");

    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(app.rename_window().is_none());
    let effects = refresh_for_attach(
        app,
        effects,
        &[
            ("main", "after-stop-all.json"),
            ("alt", "after-stop-all.json"),
        ],
    );
    let request = effects
        .iter()
        .find_map(|e| match e {
            Effect::Attach(r) => Some(r.clone()),
            _ => None,
        })
        .expect("attaches as Enter on the row would");
    assert_eq!(request.id, "7b4d9f02");
    assert_eq!(request.account.name, "main");
    app.attach_finished(AttachOutcome {
        name: request.name,
        account: "main".into(),
        result: Ok(Some(exit)),
    })
}

#[test]
fn a_running_row_is_renamed_inside_and_checked_after_the_attach() {
    let mut app = app_owned();
    let effects = rename_running(&mut app, 0);
    assert!(effects.contains(&Effect::PollForRename), "{effects:?}");
    assert!(!effects.contains(&Effect::PollNow), "{effects:?}");
    app.rename_poll_started(5_000);
    let renamed = listing_with("\"name\": \"print-date\"", "\"name\": \"new-date\"");
    // A listing from before the attach's poll does not count.
    app.apply_poll(poll_text("main", &renamed, 4_999));
    assert!(
        !status_text(&app).contains("renamed"),
        "{}",
        status_text(&app)
    );
    app.apply_poll(poll_text("main", &renamed, 5_000));
    assert!(
        status_text(&app).contains("renamed print-date → new-date"),
        "{}",
        status_text(&app)
    );
}

#[test]
fn a_running_row_whose_name_did_not_change_gets_a_neutral_note() {
    let mut app = app_owned();
    rename_running(&mut app, 0);
    app.rename_poll_started(5_000);
    let mut update = poll("main", "after-stop-all.json");
    update.cycle = 5_000;
    app.apply_poll(update);
    let status = status_text(&app);
    assert!(
        status.contains("name unchanged: print-date (to rename it, type /rename new-date"),
        "{status}"
    );
}

#[test]
fn a_failed_attach_checks_no_name() {
    let mut app = app_owned();
    let effects = rename_running(&mut app, 1);
    assert!(effects.contains(&Effect::PollNow), "{effects:?}");
    assert!(!effects.contains(&Effect::PollForRename));
    let mut update = poll("main", "after-stop-all.json");
    update.cycle = 9_000;
    app.apply_poll(update);
    assert!(!status_text(&app).contains("name unchanged"));
}

// ---- keys in the help and the footer -----------------------------------------

#[test]
fn help_and_footer_offer_e() {
    let mut app = app_owned();
    app.handle_key(ch('?'));
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("a, e"), "{screen}");
    assert!(screen.contains("(hand-off); rename"), "{screen}");
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 220, 20);
    let footer = screen.lines().last().unwrap();
    assert!(footer.contains("e rename"), "{footer}");
}
