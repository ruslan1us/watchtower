//! Windows opened by late results wait for the open one (doc/0011),
//! `Ctrl+C` inside a popup, the refresh before an attach (doc/0005), the
//! project scan off the UI thread (doc/0008) and another tower's sidecar
//! changes taken in before waking a session (doc/0017 section 8).

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{
    ATTACH_CYCLE, app_single, app_two_with, ch, ctrl, key, poll, refresh_for_attach,
    run_sidecar_check, status_text,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::actions::RemoveError;
use watchtower::app::{
    App, AttachOutcome, Confirm, Effect, HandoffJob, Overlay, SessionAction, SidecarJob,
};
use watchtower::dialog::DialogPrompt;
use watchtower::handoff::HandoffError;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::projects::ProjectList;
use watchtower::sidecar::{AccountSource, Sidecar};

const SAY_OK: &str = "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81";
const TOUCH: &str = "d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42";
const PRINT_DATE: &str = "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93";
const BOTH: [(&str, &str); 2] = [
    ("main", "after-stop-all.json"),
    ("alt", "after-stop-all.json"),
];

/// Two accounts with shared lists; `main` started every session.
fn app_owned() -> App {
    let mut sc = Sidecar::default();
    for sid in [SAY_OK, TOUCH, PRINT_DATE] {
        sc.record_owner(sid, "main", AccountSource::Started);
    }
    app_from(sc)
}

fn app_from(sc: Sidecar) -> App {
    let mut app = app_two_with(sc);
    for (account, fixture) in BOTH {
        app.apply_poll(poll(account, fixture));
    }
    app
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

fn handoff_of(effects: &[Effect]) -> HandoffJob {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Handoff(job) => Some(job.clone()),
            _ => None,
        })
        .expect("a hand-off starts")
}

/// `a` on the selected row, confirmed with `y` (every hand-off asks,
/// doc/0004).
fn start_handoff(app: &mut App) -> HandoffJob {
    assert!(app.handle_key(ch('a')).is_empty(), "asks first");
    handoff_of(&app.handle_key(ch('y')))
}

fn remove_of(effects: &[Effect]) -> SessionAction {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Remove(a) => Some(a.clone()),
            _ => None,
        })
        .expect("a remove runs")
}

fn attach_of(effects: &[Effect]) -> Option<(String, String)> {
    effects.iter().find_map(|e| match e {
        Effect::Attach(r) => Some((r.account.name.clone(), r.id.clone())),
        _ => None,
    })
}

fn refused() -> Result<(), RemoveError> {
    Err(RemoveError::Refused {
        code: Some(1),
        message: "has unpushed commits".into(),
    })
}

fn untrusted() -> Result<watchtower::handoff::Resumed, HandoffError> {
    Err(HandoffError::Untrusted {
        dir: PathBuf::from("/home/user/workspace/demo-app"),
        message: "Workspace not trusted".into(),
    })
}

// ---- late windows wait (doc/0011) -------------------------------------------

#[test]
fn a_late_trust_window_waits_for_the_open_confirmation() {
    let mut app = app_owned();
    select(&mut app, "say-ok");
    let job = start_handoff(&mut app);
    select(&mut app, "print-date");
    app.handle_key(ch('d'));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Remove { .. })
    ));

    // The hand-off fails with "not trusted" while "Remove print-date?" is
    // open: the question stays, the trust window waits.
    app.handoff_finished(job, untrusted());
    assert!(
        matches!(app.overlay(), Overlay::Confirm(Confirm::Remove { .. })),
        "{:?}",
        app.overlay()
    );
    assert_eq!(app.waiting_windows(), 1);
    assert!(
        status_text(&app).contains("1 window is waiting"),
        "{}",
        status_text(&app)
    );

    // The next key answers the question it was meant for.
    let action = remove_of(&app.handle_key(ch('y')));
    assert_eq!(action.name, "print-date");
    // Then the trust window opens.
    assert!(matches!(app.overlay(), Overlay::Untrusted(_)));
    assert_eq!(app.waiting_windows(), 0);
}

#[test]
fn late_windows_wait_for_help_and_pickers_in_order() {
    let mut app = app_owned();
    select(&mut app, "say-ok");
    let job = start_handoff(&mut app);
    select(&mut app, "print-date");
    let action = SessionAction {
        account: app.config().account("main").unwrap().clone(),
        id: "7b4d9f02".into(),
        name: "print-date".into(),
    };
    app.handle_key(ch('?'));
    assert_eq!(app.overlay(), &Overlay::Help);
    app.remove_finished(action, refused());
    app.handoff_finished(job, untrusted());
    assert_eq!(app.overlay(), &Overlay::Help, "help stays open");
    assert!(app.popup().is_none());
    assert_eq!(app.waiting_windows(), 2);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.popup().is_some(), "first come, first shown");
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay(), Overlay::Untrusted(_)));
}

#[test]
fn an_rm_refusal_waits_for_the_dialog_whose_ctrl_c_asks_first() {
    let mut app = app_owned();
    select(&mut app, "print-date");
    app.handle_key(ch('d'));
    let action = remove_of(&app.handle_key(ch('y')));
    app.open_dialog(ProjectList::default());

    app.remove_finished(action, refused());
    assert!(app.popup().is_none(), "not on top of the dialog");
    assert!(app.dialog().is_some());
    assert!(status_text(&app).contains("window is waiting"));

    // Ctrl+C is the dialog's: its own question, no quit.
    let effects = app.handle_key(ctrl('c'));
    assert!(!effects.contains(&Effect::Quit), "{effects:?}");
    assert_eq!(app.dialog().unwrap().prompt, Some(DialogPrompt::Quit));
    app.handle_key(ch('n'));
    assert!(app.dialog().is_some());
    assert!(app.popup().is_none());

    // Closing the dialog shows the refusal.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.dialog().is_none());
    assert!(
        app.popup()
            .unwrap()
            .title
            .contains("print-date was not removed")
    );
}

#[test]
fn ctrl_c_in_a_popup_asks_while_a_handoff_runs() {
    let mut app = app_owned();
    select(&mut app, "say-ok");
    let _job = start_handoff(&mut app);
    let action = SessionAction {
        account: app.config().account("main").unwrap().clone(),
        id: "7b4d9f02".into(),
        name: "print-date".into(),
    };
    app.remove_finished(action, refused());
    assert!(app.popup().is_some(), "nothing else was open");

    let effects = app.handle_key(ctrl('c'));
    assert!(!effects.contains(&Effect::Quit), "{effects:?}");
    assert!(app.quit_prompt().is_some(), "the hand-off question");
    app.handle_key(key(KeyCode::Esc));
    assert!(app.quit_prompt().is_none());
    assert!(app.popup().is_some(), "the popup is still there");
}

// ---- refresh before attach (doc/0005) ---------------------------------------

#[test]
fn a_session_gone_from_the_fresh_listing_is_not_attached() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(effects, [Effect::PollForAttach, Effect::CheckSidecar]);
    assert!(status_text(&app).contains("checking say-ok…"));

    app.attach_poll_started(ATTACH_CYCLE);
    let mut effects = app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: ATTACH_CYCLE,
        result: Ok(parse_listing("[]").unwrap()),
    });
    effects.extend(run_sidecar_check(
        &mut app,
        Path::new(common::NO_STATE_FILE),
    ));
    assert_eq!(attach_of(&effects), None, "{effects:?}");
    assert!(!app.attach_pending());
    assert!(
        status_text(&app).contains("session no longer listed"),
        "{}",
        status_text(&app)
    );
}

#[test]
fn a_listing_that_started_before_enter_does_not_count_and_the_wait_is_bounded() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    app.handle_key(key(KeyCode::Enter));
    app.attach_poll_started(ATTACH_CYCLE);
    // An older poll (cycle 0) reports: not the fresh listing.
    let effects = app.apply_poll(poll("main", "after-stop-all.json"));
    assert_eq!(attach_of(&effects), None);
    let effects = run_sidecar_check(&mut app, Path::new(common::NO_STATE_FILE));
    assert_eq!(attach_of(&effects), None, "still waits for the listing");
    assert!(app.tick(Instant::now()).is_empty());
    assert!(app.attach_pending());

    // After the bound, attach anyway and say so.
    let effects = app.tick(Instant::now() + Duration::from_secs(6));
    assert_eq!(
        attach_of(&effects),
        Some(("main".to_owned(), "b2f8d064".to_owned()))
    );
    app.attach_finished(AttachOutcome {
        name: "say-ok".into(),
        account: "main".into(),
        result: Ok(Some(0)),
    });
    let text = status_text(&app);
    assert!(text.contains("did not finish within 5s"), "{text}");
}

#[test]
fn esc_cancels_a_waiting_attach() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    let effects = app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.attach_pending());
    assert!(status_text(&app).contains("attach to say-ok cancelled"));
    let effects = refresh_for_attach(&mut app, effects, &[("main", "after-stop-all.json")]);
    assert_eq!(attach_of(&effects), None);
}

#[test]
fn a_window_opened_while_waiting_cancels_the_attach() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    select(&mut app, "say-ok");
    let effects = app.handle_key(key(KeyCode::Enter));
    app.handle_key(ch('?'));
    let effects = refresh_for_attach(&mut app, effects, &[("main", "after-stop-all.json")]);
    assert_eq!(attach_of(&effects), None);
    assert!(status_text(&app).contains("another window is open"));
}

// ---- another tower's records before waking (doc/0017 section 8) ----------------

/// A state file where `main` started every session, and this tower loaded
/// it.
fn shared_state(dir: &Path) -> (PathBuf, App) {
    let path = dir.join("state.json");
    let mut first = Sidecar::default();
    for sid in [SAY_OK, TOUCH, PRINT_DATE] {
        first.record_owner_at(sid, "main", AccountSource::Started, 100);
    }
    first.save(&path).unwrap();
    let app = app_from(Sidecar::load(&path).sidecar);
    (path, app)
}

#[test]
fn another_towers_handoff_is_honoured_before_attaching() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut app) = shared_state(dir.path());
    select(&mut app, "say-ok");
    assert_eq!(app.selected_session().unwrap().owner.label(), "main");
    // This tower has an unsaved change of its own.
    app.sidecar
        .record_owner_at(TOUCH, "alt", AccountSource::Asked, 500);

    // A second tower hands say-ok off to alt.
    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner_at(SAY_OK, "alt", AccountSource::Handoff, 200);
    other.save(&path).unwrap();

    let effects = app.handle_key(key(KeyCode::Enter));
    app.attach_poll_started(ATTACH_CYCLE);
    for (account, fixture) in BOTH {
        let mut update = poll(account, fixture);
        update.cycle = ATTACH_CYCLE;
        app.apply_poll(update);
    }
    assert!(effects.contains(&Effect::CheckSidecar));
    let effects = run_sidecar_check(&mut app, &path);
    assert_eq!(
        attach_of(&effects),
        Some(("alt".to_owned(), "b2f8d064".to_owned())),
        "{effects:?}"
    );
    assert_eq!(app.selected_session().unwrap().owner.label(), "alt");
    assert_eq!(
        app.sidecar.owner(TOUCH),
        Some(("alt", &AccountSource::Asked)),
        "the unsaved change is kept"
    );
    assert!(app.sidecar.is_dirty());
    app.attach_finished(AttachOutcome {
        name: "say-ok".into(),
        account: "alt".into(),
        result: Ok(Some(0)),
    });
    let text = status_text(&app);
    assert!(text.contains("turned out to run under alt"), "{text}");
}

#[test]
fn the_pickers_answer_waits_for_another_towers_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    Sidecar::default().save(&path).unwrap();
    let mut app = app_from(Sidecar::load(&path).sidecar);
    select(&mut app, "touch-config");
    assert_eq!(app.selected_session().unwrap().owner.label(), "?");
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.overlay(), Overlay::Picker(_)));

    // Meanwhile another tower records touch-config under alt.
    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner(TOUCH, "alt", AccountSource::Handoff);
    other.save(&path).unwrap();

    // Enter picks main, the default; the refresh finds alt.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.sidecar.owner(TOUCH), None, "not recorded yet");
    app.attach_poll_started(ATTACH_CYCLE);
    for (account, fixture) in BOTH {
        let mut update = poll(account, fixture);
        update.cycle = ATTACH_CYCLE;
        app.apply_poll(update);
    }
    assert!(effects.contains(&Effect::CheckSidecar));
    let effects = run_sidecar_check(&mut app, &path);
    assert_eq!(
        attach_of(&effects),
        Some(("alt".to_owned(), "d4a1f7c8".to_owned()))
    );
    assert_eq!(
        app.sidecar.owner(TOUCH),
        Some(("alt", &AccountSource::Handoff)),
        "nothing recorded as asked"
    );
}

#[test]
fn a_poll_time_check_takes_in_another_towers_change() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut app) = shared_state(dir.path());
    let mut other = Sidecar::load(&path).sidecar;
    other.record_owner_at(PRINT_DATE, "alt", AccountSource::Handoff, 300);
    other.save(&path).unwrap();
    select(&mut app, "print-date");
    assert_eq!(app.selected_session().unwrap().owner.label(), "main");
    run_sidecar_check(&mut app, &path);
    assert_eq!(app.selected_session().unwrap().owner.label(), "alt");
}

// ---- one sidecar job at a time (doc/0002) ------------------------------------

#[test]
fn sidecar_saves_run_one_at_a_time_with_one_queued() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut app) = shared_state(dir.path());
    app.sidecar.record_answer(TOUCH, "alt");
    let Some(SidecarJob::Save { seq, job }) = app.start_save() else {
        panic!("a save starts");
    };
    assert!(app.sidecar_busy());
    // More changes and save requests while it runs: one is queued.
    app.sidecar.record_answer(SAY_OK, "alt");
    assert!(app.start_save().is_none());
    assert!(app.start_save().is_none());
    assert!(app.start_sidecar_check().is_none());
    let result = job.run(&path, Duration::from_secs(2));
    let effects = app.sidecar_saved(seq, &job, result);
    assert_eq!(effects, [Effect::SaveSidecar], "the queued save, once");
    assert_eq!(
        app.sidecar.owner(SAY_OK),
        Some(("alt", &AccountSource::Asked))
    );
    assert!(app.sidecar.is_dirty());

    let Some(SidecarJob::Save { seq, job }) = app.start_save() else {
        panic!("the queued save starts");
    };
    let result = job.run(&path, Duration::from_secs(2));
    let effects = app.sidecar_saved(seq, &job, result);
    assert_eq!(effects, [Effect::CheckSidecar], "then the queued check");
    assert!(!app.sidecar.is_dirty());
    let disk = Sidecar::load(&path).sidecar;
    assert_eq!(disk.owner(SAY_OK), Some(("alt", &AccountSource::Asked)));
    assert_eq!(disk.owner(TOUCH), Some(("alt", &AccountSource::Asked)));
}

#[test]
fn saves_and_checks_run_on_job_workers() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut app) = shared_state(dir.path());
    let jobs = Jobs::new(watchtower::cli::CliRunner::new());
    app.sidecar.record_answer(TOUCH, "alt");
    jobs.sidecar(app.start_save().unwrap(), path.clone());
    let done = jobs.recv_timeout(Duration::from_secs(10)).expect("reports");
    assert!(matches!(done, JobResult::Saved(..)));
    done.apply(&mut app);
    assert!(!app.sidecar_busy());
    assert_eq!(
        Sidecar::load(&path).sidecar.owner(TOUCH),
        Some(("alt", &AccountSource::Asked))
    );
    jobs.sidecar(app.start_sidecar_check().unwrap(), path);
    let done = jobs.recv_timeout(Duration::from_secs(10)).expect("reports");
    assert!(
        matches!(&done, JobResult::Checked(_, Ok(r)) if !r.changed()),
        "{done:?}"
    );
}

// ---- the project scan (doc/0008) ----------------------------------------------

fn render(app: &mut App) -> String {
    let (w, h) = (100, 30);
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
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_dialog_opens_at_once_and_fills_when_the_scan_reports() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_single();
    assert_eq!(app.handle_key(ch('n')), [Effect::OpenDialog]);
    let scan = app.begin_project_scan();
    assert!(app.dialog().unwrap().scanning);
    assert!(render(&mut app).contains("scanning projects…"));
    type_demo(&mut app);

    // The scan runs on a job worker.
    std::fs::create_dir_all(root.path().join("demo-app/.git")).unwrap();
    std::fs::create_dir_all(root.path().join("other/.git")).unwrap();
    let scan = watchtower::app::ProjectScan {
        roots: vec![root.path().to_path_buf()],
        ..scan
    };
    let jobs = Jobs::new(watchtower::cli::CliRunner::new());
    jobs.projects(scan.clone());
    let done = jobs.recv_timeout(Duration::from_secs(10)).expect("reports");
    // A stale scan's result is dropped.
    app.projects_found(scan.id + 1, ProjectList::default());
    assert!(app.dialog().unwrap().scanning);
    done.apply(&mut app);
    let dialog = app.dialog().unwrap();
    assert!(!dialog.scanning);
    assert_eq!(dialog.projects.len(), 2);
    assert_eq!(
        dialog
            .selected_project()
            .unwrap()
            .display
            .rsplit('/')
            .next(),
        Some("demo-app"),
        "the filter typed meanwhile applies"
    );
    assert!(!render(&mut app).contains("scanning projects…"));
}

fn type_demo(app: &mut App) {
    for c in "demo".chars() {
        assert!(app.handle_key(ch(c)).is_empty());
    }
}

#[test]
fn exit_waits_for_the_save_in_flight_then_saves_the_rest() {
    use watchtower::tui::flush_sidecar;
    let dir = tempfile::tempdir().unwrap();
    let (path, mut app) = shared_state(dir.path());
    let jobs = Jobs::new(watchtower::cli::CliRunner::new());
    app.sidecar.record_answer(TOUCH, "alt");
    jobs.sidecar(app.start_save().unwrap(), path.clone());
    // A change after the save started.
    app.sidecar.record_answer(SAY_OK, "alt");
    let saved = flush_sidecar(
        &mut app,
        Duration::from_secs(10),
        |job| jobs.sidecar(job, path.clone()),
        |t| jobs.recv_timeout(t),
    );
    assert!(saved);
    assert!(!app.sidecar_busy());
    let disk = Sidecar::load(&path).sidecar;
    assert_eq!(disk.owner(TOUCH), Some(("alt", &AccountSource::Asked)));
    assert_eq!(disk.owner(SAY_OK), Some(("alt", &AccountSource::Asked)));
}
