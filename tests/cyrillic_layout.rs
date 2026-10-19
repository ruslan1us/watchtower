//! Command keys under a Russian or Ukrainian layout (doc/0011): Cyrillic
//! letters act as the Latin key on the same physical key in command
//! contexts, and stay typed text in text fields.

mod common;

use common::{
    app_single, app_two_with, ch, ctrl, key, poll, project_list, select_name, status_text,
    type_text,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::{App, Confirm, Effect, Overlay};
use watchtower::sidecar::{AccountSource, Sidecar};

const SAY_OK: &str = "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81";

fn shifted(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

fn selected(app: &App) -> String {
    app.selected_session()
        .map(|m| m.session.display_name())
        .unwrap_or_default()
}

fn mixed() -> App {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    app
}

/// Two accounts where `main` owns `say-ok`, so `a` offers `alt`.
fn two_owned() -> App {
    let mut sc = Sidecar::default();
    sc.record_owner(SAY_OK, "main", AccountSource::Started);
    let mut app = app_two_with(sc);
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    select_name(&mut app, "say-ok");
    app
}

#[test]
fn list_keys_work_under_a_cyrillic_layout() {
    let mut app = mixed();
    // т is the n key: the new-session dialog.
    assert_eq!(app.handle_key(ch('т')), [Effect::OpenDialog]);
    // к is the r key: refresh; й is the q key: quit.
    assert_eq!(app.handle_key(ch('к')), [Effect::PollNow]);
    assert_eq!(app.handle_key(ch('й')), [Effect::Quit]);
    // Ctrl+с is Ctrl+C.
    assert_eq!(app.handle_key(ctrl('с')), [Effect::Quit]);
}

#[test]
fn movement_keys_and_uppercase() {
    let mut app = mixed();
    let first = selected(&app);
    app.handle_key(ch('о'));
    let second = selected(&app);
    assert_ne!(first, second, "о is j");
    app.handle_key(ch('л'));
    assert_eq!(selected(&app), first, "л is k");
    // П (Shift+п) is G and п is g: the same lines as with Latin keys.
    let mut latin = mixed();
    for (cyr, lat) in [(shifted('П'), shifted('G')), (ch('п'), ch('g'))] {
        app.handle_key(cyr);
        latin.handle_key(lat);
        assert_eq!(app.selected_entry(), latin.selected_entry(), "{cyr:?}");
    }
}

#[test]
fn remove_and_stop_confirmations() {
    let mut app = mixed();
    select_name(&mut app, "say-ok");
    // в is d: remove asks first; т (n) cancels.
    assert!(app.handle_key(ch('в')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Remove { .. })
    ));
    app.handle_key(ch('т'));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("remove cancelled"));
    // н is y: confirms.
    app.handle_key(ch('в'));
    let effects = app.handle_key(ch('н'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Remove(_))),
        "{effects:?}"
    );

    // ы is s: stop of a working row asks first.
    select_name(&mut app, "pick-colour");
    assert!(app.handle_key(ch('ы')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Stop { .. })
    ));
    let effects = app.handle_key(shifted('Н'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Stop(_))),
        "Н is Y: {effects:?}"
    );
}

#[test]
fn handoff_and_the_quit_question() {
    let mut app = two_owned();
    // ф is a: the hand-off asks first.
    assert!(app.handle_key(ch('ф')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Handoff { .. })
    ));
    let effects = app.handle_key(ch('н'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Handoff(_))),
        "{effects:?}"
    );
    // While it runs, й (q) asks; т (n) keeps tower running, н (y) quits.
    assert!(app.handle_key(ch('й')).is_empty());
    assert!(app.handle_key(ch('т')).is_empty());
    assert!(status_text(&app).contains("not quitting"));
    assert!(app.handle_key(ch('й')).is_empty());
    assert_eq!(app.handle_key(ch('н')), [Effect::Quit]);
}

#[test]
fn filter_help_and_resume_keys() {
    let mut app = mixed();
    // The RU layout types `.` on the `/` key and `,` on `?`.
    assert!(app.handle_key(ch('.')).is_empty());
    assert!(app.filter_editing());
    // The filter input keeps Cyrillic (and `.`, `,`) as text.
    type_text(&mut app, "тфв.,");
    assert_eq!(app.filter_text(), "тфв.,");
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.filter_editing());

    app.handle_key(ch(','));
    assert_eq!(app.overlay(), &Overlay::Help);
    app.handle_key(ch(','));
    assert_eq!(app.overlay(), &Overlay::None, ", (the ? key) closes help");

    // К (Shift+к) is R: the resume window; its project filter keeps text.
    let effects = app.handle_key(shifted('К'));
    assert!(
        matches!(effects.as_slice(), [Effect::ScanProjects(_)]),
        "{effects:?}"
    );
    assert!(app.resume_window().is_some());
    for c in ['т', 'й'] {
        app.handle_key(ch(c));
    }
    assert_eq!(app.resume_window().unwrap().filter.text(), "тй");
    assert!(app.resume_window().is_some(), "й did not quit");
}

#[test]
fn dialog_keeps_cyrillic_text_and_reads_shortcuts() {
    let mut app = app_single();
    let root = tempfile::tempdir().unwrap();
    assert_eq!(app.handle_key(ch('т')), [Effect::OpenDialog]);
    app.open_dialog(project_list(root.path(), &["web"]));
    // The project filter keeps typed Cyrillic.
    type_text(&mut app, "в");
    assert_eq!(app.dialog().unwrap().filter.text(), "в");
    app.handle_key(key(KeyCode::Backspace));
    while app.dialog().unwrap().field != watchtower::dialog::Field::Task {
        app.handle_key(key(KeyCode::Tab));
    }
    type_text(&mut app, "тфв почини тест");
    assert_eq!(app.dialog().unwrap().task.text(), "тфв почини тест");
    // Ctrl+ы is Ctrl+S: dispatch in the background.
    let effects = app.handle_key(ctrl('ы'));
    assert!(
        matches!(effects.as_slice(), [Effect::Dispatch(job)] if !job.attach),
        "{effects:?}"
    );
}

#[test]
fn dialog_questions_take_cyrillic_answers() {
    let mut app = app_single();
    let root = tempfile::tempdir().unwrap();
    app.handle_key(ch('n'));
    app.open_dialog(project_list(root.path(), &["web"]));
    while app.dialog().unwrap().field != watchtower::dialog::Field::Task {
        app.handle_key(key(KeyCode::Tab));
    }
    type_text(&mut app, "draft");
    // Esc asks to discard; т (n) keeps the dialog, н (y) discards.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('т'));
    assert!(app.dialog().is_some());
    assert_eq!(app.dialog().unwrap().task.text(), "draft");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('н'));
    assert!(app.dialog().is_none());
}
