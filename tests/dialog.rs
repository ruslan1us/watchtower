//! The new-session dialog as a state machine (doc/0008, doc/0011,
//! doc/0015): fields, filter, validation, the dispatch effect and every
//! outcome of a dispatch, without a terminal or processes.

mod common;

use std::path::{Path, PathBuf};

use common::{
    HOME, app_single, app_two, ch, ctrl, follow_off, key, project_list, status_text, type_text,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::{App, DispatchJob, Effect, LogsTarget};
use watchtower::cli::CliError;
use watchtower::config::Config;
use watchtower::dialog::{DialogPrompt, DialogStatus, Field};
use watchtower::dispatch::{DispatchError, Dispatched, HAIKU_AUTO_WARNING};
use watchtower::sidecar::{AccountSource, Sidecar};

/// An app with the dialog open over projects `api`, `demo-app`, `web` in a
/// temp root (so validation finds the folders).
fn open(mut app: App) -> (App, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let list = project_list(root.path(), &["web", "api", "demo-app"]);
    assert_eq!(app.handle_key(ch('n')), [Effect::OpenDialog]);
    app.open_dialog(list);
    (app, root)
}

fn field(app: &App) -> Field {
    app.dialog().expect("dialog open").field
}

fn tab(app: &mut App, n: usize) {
    for _ in 0..n {
        assert!(app.handle_key(key(KeyCode::Tab)).is_empty());
    }
}

fn to(app: &mut App, target: Field) {
    while field(app) != target {
        tab(app, 1);
    }
}

fn dispatch_job(effects: &[Effect]) -> DispatchJob {
    match effects {
        [Effect::Dispatch(job)] => job.clone(),
        other => panic!("expected one dispatch, got {other:?}"),
    }
}

fn error(app: &App) -> String {
    app.dialog()
        .and_then(|d| d.error.clone())
        .unwrap_or_default()
}

/// Fills in a valid task and starts the dispatch with `start`.
fn start(app: &mut App, start: KeyEvent) -> DispatchJob {
    to(app, Field::Task);
    type_text(app, "fix the flaky test");
    dispatch_job(&app.handle_key(start))
}

#[test]
fn fields_cycle_with_tab_and_shift_tab() {
    let (mut app, _root) = open(app_single());
    let order = [
        Field::Project,
        Field::Task,
        Field::Name,
        Field::Account,
        Field::Model,
        Field::Effort,
        Field::Mode,
    ];
    for f in order {
        assert_eq!(field(&app), f);
        tab(&mut app, 1);
    }
    assert_eq!(field(&app), Field::Project, "wraps around");
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(field(&app), Field::Mode);
    // Shift+Tab as reported with keyboard enhancement.
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
    assert_eq!(field(&app), Field::Effort);
    // Enter moves on outside the task field.
    to(&mut app, Field::Name);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(field(&app), Field::Account);
}

#[test]
fn projects_sorted_and_filtered_by_typing() {
    let (mut app, _root) = open(app_single());
    let names = |app: &App| -> Vec<String> {
        app.dialog()
            .unwrap()
            .filtered()
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(names(&app), ["api", "demo-app", "web"], "alphabetical");
    assert_eq!(field(&app), Field::Project, "starts on the project");
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    let selected = |app: &App| {
        app.dialog()
            .unwrap()
            .selected_project()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
    };
    assert_eq!(selected(&app).as_deref(), Some("web"));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected(&app).as_deref(), Some("web"), "clamps");

    // Typing filters and resets the selection to the first match.
    type_text(&mut app, "ap");
    assert_eq!(names(&app), ["api", "demo-app"]);
    assert_eq!(selected(&app).as_deref(), Some("api"));
    type_text(&mut app, "p");
    assert_eq!(names(&app), ["demo-app"]);
    // Letters are text here, never list keys ('q', 'j', 'n'…).
    type_text(&mut app, "qjn");
    assert!(names(&app).is_empty());
    assert_eq!(selected(&app), None);
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Backspace));
    }
    assert_eq!(app.dialog().unwrap().filter.text(), "app");
    assert_eq!(selected(&app).as_deref(), Some("demo-app"));
}

#[test]
fn recent_projects_come_first() {
    let root = tempfile::tempdir().unwrap();
    for n in ["api", "demo-app", "web"] {
        std::fs::create_dir_all(root.path().join(n).join(".git")).unwrap();
    }
    let web = root.path().join("web");
    let mut sidecar = Sidecar::default();
    sidecar.touch_recent_project(&web, Path::new(HOME), 1_790_000_000_000);
    let list = watchtower::projects::discover(
        &[root.path().to_path_buf()],
        &[],
        &sidecar.recent_projects,
        Path::new(HOME),
    );
    let mut app = common::app_two_with(sidecar);
    app.open_dialog(list);
    let first = app
        .dialog()
        .unwrap()
        .selected_project()
        .unwrap()
        .path
        .clone();
    assert_eq!(first, web);
}

#[test]
fn task_is_multi_line_and_editable() {
    let (mut app, _root) = open(app_single());
    to(&mut app, Field::Task);
    type_text(&mut app, "line one");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(field(&app), Field::Task, "Enter stays in the task");
    type_text(&mut app, "line two");
    // A line feed (Ctrl+J, as some terminals send for Ctrl+Enter) is a new
    // line too, never a dispatch.
    assert!(app.handle_key(ctrl('j')).is_empty());
    type_text(&mut app, "x");
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    // Backspace at the start of a line joins it with the previous one.
    assert_eq!(app.dialog().unwrap().task.text(), "line one\nline two");
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Home));
    type_text(&mut app, ">");
    assert_eq!(app.dialog().unwrap().task.text(), ">line one\nline two");
}

#[test]
fn ctrl_u_is_not_bound_in_the_dialog_fields() {
    // doc/0011 and doc/0036: the dialog's text fields take no Ctrl keys
    // (Ctrl+U empties only the list filter, the template picker's filter
    // and the rename field). The model preset is deleted with Backspace.
    let (mut app, _root) = open(app_single());
    to(&mut app, Field::Model);
    assert_eq!(app.dialog().unwrap().model.text(), "default");
    assert!(app.handle_key(ctrl('u')).is_empty());
    assert_eq!(app.dialog().unwrap().model.text(), "default", "no u typed");
    for _ in 0.."default".len() {
        app.handle_key(key(KeyCode::Backspace));
    }
    type_text(&mut app, "claude-opus-9");
    assert_eq!(app.dialog().unwrap().model.text(), "claude-opus-9");
    to(&mut app, Field::Task);
    type_text(&mut app, "keep");
    assert!(app.handle_key(ctrl('u')).is_empty());
    assert_eq!(app.dialog().unwrap().task.text(), "keep");
    assert!(app.dialog().is_some(), "the dialog stays open");
}

#[test]
fn config_defaults_and_choices() {
    let home = Path::new(HOME);
    let config = Config::parse(
        r#"
default_account = "alt"
default_model = "opus"
default_effort = "high"
default_permission_mode = "plan"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#,
        home,
    )
    .unwrap();
    let mut app = App::new(
        config,
        home.into(),
        Sidecar::default(),
        Box::new(common::NoEnviron),
    );
    let (mut app, _root) = {
        app.set_root(false);
        open(app)
    };
    let d = app.dialog().unwrap();
    assert_eq!(d.account.value(), "alt", "default_account preselected");
    assert_eq!(d.model.text(), "opus");
    assert_eq!(d.effort.value(), "high");
    assert_eq!(d.mode.value(), "plan");
    assert!(
        d.mode.items().iter().any(|m| m == "bypassPermissions"),
        "offered when not root"
    );

    to(&mut app, Field::Account);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.dialog().unwrap().account.value(), "main");
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.dialog().unwrap().account.value(), "alt");

    // Model: presets on ↑/↓, free text by typing.
    to(&mut app, Field::Model);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.dialog().unwrap().model.text(), "sonnet");
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.dialog().unwrap().model.text(), "fable");
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Backspace));
    }
    type_text(&mut app, "claude-sonnet-9");
    assert_eq!(app.dialog().unwrap().model.text(), "claude-sonnet-9");

    to(&mut app, Field::Effort);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.dialog().unwrap().effort.value(), "xhigh");
}

#[test]
fn root_hides_bypass_permissions() {
    let mut app = app_single();
    app.set_root(true);
    let (app, _root) = open(app);
    let modes = app.dialog().unwrap().mode.items().to_vec();
    assert!(!modes.iter().any(|m| m == "bypassPermissions"), "{modes:?}");
    assert_eq!(app.dialog().unwrap().mode.value(), "auto");
}

#[test]
fn haiku_with_auto_warns_inline() {
    let (mut app, _root) = open(app_single());
    assert_eq!(app.dialog().unwrap().haiku_warning(), None);
    to(&mut app, Field::Model);
    while app.dialog().unwrap().model.text() != "haiku" {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(
        app.dialog().unwrap().haiku_warning(),
        Some(HAIKU_AUTO_WARNING)
    );
    to(&mut app, Field::Mode);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.dialog().unwrap().mode.value(), "acceptEdits");
    assert_eq!(app.dialog().unwrap().haiku_warning(), None);
}

#[test]
fn validation_errors_stay_in_the_dialog() {
    let (mut app, _root) = open(app_single());
    // Too short a task: nothing runs, the task field gets the focus.
    to(&mut app, Field::Task);
    type_text(&mut app, "ab ");
    to(&mut app, Field::Name);
    assert!(app.handle_key(ctrl('s')).is_empty());
    assert_eq!(field(&app), Field::Task);
    assert!(
        error(&app).contains("at least 4 characters"),
        "{}",
        error(&app)
    );
    assert_eq!(app.dialog().unwrap().status, DialogStatus::Editing);

    // A name starting with '-' would be read as an option.
    type_text(&mut app, "cd");
    to(&mut app, Field::Name);
    type_text(&mut app, "-x");
    assert!(app.handle_key(ctrl('s')).is_empty());
    assert_eq!(field(&app), Field::Name);
    assert!(error(&app).contains("must not start with '-'"));

    // No project matches the filter.
    app.handle_key(key(KeyCode::Home));
    app.handle_key(key(KeyCode::Delete));
    to(&mut app, Field::Project);
    type_text(&mut app, "zzz");
    assert!(app.handle_key(ctrl('s')).is_empty());
    assert_eq!(field(&app), Field::Project);
    assert!(error(&app).contains("choose a project"));
    // The fields were kept throughout.
    assert_eq!(app.dialog().unwrap().task.text(), "ab cd");
    assert_eq!(app.dialog().unwrap().name.text(), "x");
}

#[test]
fn dispatch_effect_carries_the_request() {
    let (mut app, root) = open(app_two());
    to(&mut app, Field::Name);
    type_text(&mut app, "flaky");
    to(&mut app, Field::Account);
    app.handle_key(key(KeyCode::Right));
    to(&mut app, Field::Effort);
    app.handle_key(key(KeyCode::Right));
    let job = start(&mut app, ctrl('s'));
    assert!(!job.attach && !job.after_trust);
    let req = &job.request;
    assert_eq!(req.project, root.path().join("api"), "first project");
    assert_eq!(req.account.name, "alt");
    assert_eq!(
        req.command_line().argv(),
        [
            "claude-alt",
            "--bg",
            "--name",
            "flaky",
            "--effort",
            "low",
            "--permission-mode",
            "auto",
            "fix the flaky test"
        ]
    );
    assert_eq!(
        req.command_line().cwd.as_deref(),
        Some(req.project.as_path())
    );
    assert!(
        !req.args().iter().any(|a| a.contains("dangerously")),
        "never skips permissions"
    );

    // Starting…: the contents stay, keys do nothing.
    let d = app.dialog().unwrap();
    assert_eq!(d.status, DialogStatus::Starting { attach: false });
    assert!(app.handle_key(ch('x')).is_empty());
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.handle_key(ctrl('s')).is_empty(), "no second dispatch");
    assert!(app.dialog().is_some());
    assert_eq!(app.dialog().unwrap().task.text(), "fix the flaky test");
}

/// With background follows off (doc/0031) a new session is still fetched
/// once at once, so its start's full repaint is caught.
#[test]
fn with_follows_off_a_dispatch_still_fetches_its_session_once() {
    let mut app = app_two();
    follow_off(&mut app);
    let (mut app, _root) = open(app);
    let job = start(&mut app, ctrl('s'));
    let effects = app.dispatch_finished(
        job,
        Ok(Dispatched {
            id: "3e9a1c07".into(),
            name: Some("fix-flaky".into()),
        }),
    );
    let follow = Effect::FollowLogs(LogsTarget {
        account: app.config().accounts[0].clone(),
        id: "3e9a1c07".into(),
    });
    assert_eq!(effects, [Effect::SaveSidecar, follow, Effect::PollNow]);
}

#[test]
fn success_records_closes_and_polls() {
    let (mut app, root) = open(app_two());
    let job = start(&mut app, ctrl('s'));
    let effects = app.dispatch_finished(
        job,
        Ok(Dispatched {
            id: "3e9a1c07".into(),
            name: Some("fix-flaky".into()),
        }),
    );
    // The new session's logs are fetched at once, so its start's full
    // repaint is caught (doc/0030).
    let follow = Effect::FollowLogs(LogsTarget {
        account: app.config().accounts[0].clone(),
        id: "3e9a1c07".into(),
    });
    assert_eq!(effects, [Effect::SaveSidecar, follow, Effect::PollNow]);
    assert!(app.dialog().is_none());
    assert!(
        status_text(&app).contains("started fix-flaky (3e9a1c07) under main"),
        "{}",
        status_text(&app)
    );
    // Owner keyed by the short id until a listing shows the sessionId.
    let owner = app
        .sidecar
        .owner("3e9a1c07")
        .map(|(a, s)| (a.to_owned(), s.clone()));
    assert_eq!(owner, Some(("main".into(), AccountSource::Started)));
    let recent = &app.sidecar.recent_projects[0];
    assert!(
        PathBuf::from(&recent.path).ends_with("api"),
        "{}",
        recent.path
    );
    assert!(recent.path.starts_with(&*root.path().to_string_lossy()));
}

#[test]
fn started_record_moves_to_the_session_id_on_the_next_poll() {
    let (mut app, _root) = open(app_two());
    let job = start(&mut app, ctrl('s'));
    // bad-model is 3e9a1c07 in the fixtures.
    app.dispatch_finished(
        job,
        Ok(Dispatched {
            id: "3e9a1c07".into(),
            name: Some("bad-model".into()),
        }),
    );
    let effects = app.apply_poll(common::poll("main", "mixed-states.json"));
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
    assert!(app.sidecar.owner("3e9a1c07").is_none());
    let sid = "3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60";
    assert_eq!(
        app.sidecar.owner(sid).map(|(a, _)| a.to_owned()).as_deref(),
        Some("main")
    );
    // Rule 3 now resolves the owner even with shared, unreadable lists.
    app.apply_poll(common::poll("alt", "mixed-states.json"));
    let row = app
        .sessions()
        .iter()
        .find(|m| m.session.display_name() == "bad-model")
        .unwrap();
    assert_eq!(row.owner.label(), "main");
}

#[test]
fn dispatch_and_attach_with_ctrl_enter_or_alt_enter() {
    for k in [
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
    ] {
        let (mut app, _root) = open(app_two());
        to(&mut app, Field::Account);
        app.handle_key(key(KeyCode::Right));
        let job = start(&mut app, k);
        assert!(job.attach, "{k:?}");
        let effects = app.dispatch_finished(
            job,
            Ok(Dispatched {
                id: "5a7c2e91".into(),
                name: None,
            }),
        );
        let attach = effects
            .iter()
            .find_map(|e| match e {
                Effect::Attach(r) => Some(r.clone()),
                _ => None,
            })
            .expect("attach effect");
        assert_eq!(attach.account.name, "alt", "the request's account");
        assert_eq!(attach.id, "5a7c2e91");
        assert_eq!(attach.name, "5a7c2e91", "no name: the id");
        assert!(app.dialog().is_none());
    }
}

#[test]
fn untrusted_keeps_the_dialog_and_offers_ctrl_t() {
    let (mut app, _root) = open(app_two());
    // Ctrl+T does nothing before a trust error.
    to(&mut app, Field::Task);
    assert!(app.handle_key(ctrl('t')).is_empty());
    let job = start(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    let dir = job.request.project.clone();
    let effects = app.dispatch_finished(
        job.clone(),
        Err(DispatchError::Untrusted {
            dir: dir.clone(),
            message: "Workspace not trusted. Run `claude` in <dir> once and accept the trust \
                      prompt, then retry."
                .into(),
        }),
    );
    assert!(effects.is_empty());
    let d = app.dialog().expect("kept");
    assert_eq!(
        d.status,
        DialogStatus::Untrusted {
            dir: dir.clone(),
            attach: true
        }
    );
    assert!(error(&app).starts_with("Workspace not trusted"));
    assert_eq!(d.task.text(), "fix the flaky test");

    // Ctrl+T: suspend and run the account command there.
    let effects = app.handle_key(ctrl('t'));
    let trust = match effects.as_slice() {
        [Effect::Trust(j)] => j.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(trust.request, job.request, "the same dispatch");
    assert!(trust.attach);
    // Afterwards the dispatch is retried once, automatically.
    let effects = app.trust_finished(trust, Ok(Some(0)));
    let retry = dispatch_job(&effects);
    assert!(retry.after_trust && retry.attach);
    assert_eq!(retry.request, job.request);
    assert!(app.dialog().unwrap().is_starting());

    // Still untrusted after the retry: shown again, no loop.
    let effects = app.dispatch_finished(
        retry.clone(),
        Err(DispatchError::Untrusted {
            dir: dir.clone(),
            message: "Workspace not trusted.".into(),
        }),
    );
    assert!(effects.is_empty());
    assert!(error(&app).contains("still not trusted"), "{}", error(&app));

    // A second Ctrl+T, then success: attach follows.
    let effects = app.handle_key(ctrl('t'));
    let [Effect::Trust(trust)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    let retry = dispatch_job(&app.trust_finished(trust.clone(), Ok(Some(130))));
    let effects = app.dispatch_finished(
        retry,
        Ok(Dispatched {
            id: "1a2b3c4d".into(),
            name: Some("x".into()),
        }),
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::Attach(_))));
}

#[test]
fn trust_command_that_cannot_run_is_reported_without_retry() {
    let (mut app, _root) = open(app_single());
    let job = start(&mut app, ctrl('s'));
    app.dispatch_finished(
        job.clone(),
        Err(DispatchError::Untrusted {
            dir: job.request.project.clone(),
            message: "Workspace not trusted.".into(),
        }),
    );
    let effects = app.handle_key(ctrl('t'));
    let [Effect::Trust(trust)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    let effects = app.trust_finished(trust.clone(), Err("cannot run `claude`: not found".into()));
    assert!(effects.is_empty());
    assert!(error(&app).contains("cannot run"));
    assert!(matches!(
        app.dialog().unwrap().status,
        DialogStatus::Untrusted { .. }
    ));
}

#[test]
fn changing_project_or_account_withdraws_the_trust_offer() {
    let (mut app, _root) = open(app_two());
    let job = start(&mut app, ctrl('s'));
    app.dispatch_finished(
        job.clone(),
        Err(DispatchError::Untrusted {
            dir: job.request.project.clone(),
            message: "Workspace not trusted.".into(),
        }),
    );
    to(&mut app, Field::Account);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.dialog().unwrap().status, DialogStatus::Editing);
    assert!(app.handle_key(ctrl('t')).is_empty());
}

#[test]
fn other_failures_keep_contents_and_show_the_error() {
    let (mut app, _root) = open(app_single());
    let job = start(&mut app, ctrl('s'));
    let effects = app.dispatch_finished(
        job,
        Err(DispatchError::Failed {
            code: Some(2),
            stderr: "error: something broke".into(),
        }),
    );
    assert!(effects.is_empty());
    assert_eq!(
        error(&app),
        "claude --bg failed (exit 2): error: something broke"
    );
    assert_eq!(app.dialog().unwrap().status, DialogStatus::Editing);
    assert_eq!(app.dialog().unwrap().task.text(), "fix the flaky test");

    // A timeout is shown as well; the user can retry with Ctrl+S.
    let job = dispatch_job(&app.handle_key(ctrl('s')));
    app.dispatch_finished(
        job,
        Err(DispatchError::Cli(CliError::Timeout {
            program: "claude".into(),
            timeout: std::time::Duration::from_secs(30),
            stderr: String::new(),
        })),
    );
    assert!(error(&app).contains("timed out"), "{}", error(&app));
}

#[test]
fn escape_cancels_or_asks_to_discard() {
    // Empty task: Esc closes at once.
    let (mut app, _root) = open(app_single());
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.dialog().is_none());
    assert!(status_text(&app).contains("cancelled"));

    // With text: ask; n (or Esc) keeps editing; y discards.
    let (mut app, _root) = open(app_single());
    to(&mut app, Field::Task);
    type_text(&mut app, "keep me");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.dialog().unwrap().prompt, Some(DialogPrompt::Discard));
    app.handle_key(ch('n'));
    assert_eq!(app.dialog().unwrap().prompt, None);
    assert_eq!(app.dialog().unwrap().task.text(), "keep me");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.dialog().is_some(), "Esc answers no");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('y'));
    assert!(app.dialog().is_none());
}

#[test]
fn ctrl_c_asks_before_quitting() {
    let (mut app, _root) = open(app_single());
    assert!(app.handle_key(ctrl('c')).is_empty());
    assert_eq!(app.dialog().unwrap().prompt, Some(DialogPrompt::Quit));
    app.handle_key(ch('n'));
    assert!(app.dialog().is_some());
    app.handle_key(ctrl('c'));
    assert_eq!(app.handle_key(ch('y')), [Effect::Quit]);
}

#[test]
fn ctrl_g_edits_the_task_in_the_editor() {
    let (mut app, _root) = open(app_single());
    to(&mut app, Field::Task);
    type_text(&mut app, "draft");
    to(&mut app, Field::Model);
    assert_eq!(
        app.handle_key(ctrl('g')),
        [Effect::EditTask("draft".into())]
    );
    app.editor_finished(Ok("first line\nsecond line".into()));
    let d = app.dialog().unwrap();
    assert_eq!(d.task.text(), "first line\nsecond line");
    assert_eq!(d.field, Field::Task);
    // A failed edit leaves the task as it was.
    app.editor_finished(Err(
        "the editor exited with code 1; the task is unchanged".into()
    ));
    assert_eq!(app.dialog().unwrap().task.text(), "first line\nsecond line");
    assert!(error(&app).contains("exited with code 1"));
}

#[test]
fn list_keys_do_not_leak_into_the_dialog() {
    let mut app = app_single();
    app.apply_poll(common::poll("main", "mixed-states.json"));
    let (mut app, _root) = open(app);
    // 'q', 's', 'd', Space and '?' are text in the filter.
    for c in ['q', 's', 'd', ' ', '?', 'r'] {
        assert!(app.handle_key(ch(c)).is_empty(), "{c}");
    }
    assert_eq!(app.dialog().unwrap().filter.text(), "qsd ?r");
    assert_eq!(app.overlay(), &watchtower::app::Overlay::None);
}
