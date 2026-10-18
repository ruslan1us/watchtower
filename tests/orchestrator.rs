//! The orchestrator key `o` and its instruction file (doc/0010): the state
//! machine (attach to an existing row with its owner, lazy start with the
//! exact request, duplicates, the trust flow, attach after the start, the
//! Haiku fallback), the folder and `CLAUDE.md`, and the start run by
//! [`Jobs`] against a fake `claude`.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, ctrl, key, status_text};
use fake_claude::{Fake, cat, cat_err, lock};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, AttachRequest, Effect, OrchestratorJob, Overlay};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::dispatch::{DispatchError, Dispatched};
use watchtower::jobs::Jobs;
use watchtower::listing::parse_listing;
use watchtower::orchestrator::{self, StartError};
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};

// ---- helpers -------------------------------------------------------------

/// A config with accounts `main` (default) and `alt`, plus `extra` lines.
fn config(main: &str, alt: &str, extra: &str) -> Config {
    Config::parse(
        &format!(
            r#"
default_account = "main"
{extra}

[[accounts]]
name = "main"
command = "{main}"

[[accounts]]
name = "alt"
command = "{alt}"
config_dir = "~/.claude-alt"
"#
        ),
        Path::new(HOME),
    )
    .unwrap()
}

fn app_with(config: Config) -> App {
    App::new(config, HOME.into(), Sidecar::default(), Box::new(NoEnviron))
}

fn app_two() -> App {
    app_with(config("claude", "claude-alt", ""))
}

/// A row: short id, name, live (`status: idle` and a pid) or stopped, and
/// `startedAt`.
struct Row<'a> {
    id: &'a str,
    name: &'a str,
    live: bool,
    started: i64,
}

fn listing_json(rows: &[Row]) -> String {
    let items: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let live = if r.live {
                format!(r#""pid": {}, "status": "idle", "#, 5000 + i)
            } else {
                String::new()
            };
            format!(
                r#"{{ {live}"id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
                   "cwd": "/home/user/.config/watchtower/orchestrator", "kind": "background",
                   "startedAt": {started}, "name": "{name}", "state": "done" }}"#,
                id = r.id,
                name = r.name,
                started = r.started,
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn update(account: &str, cycle: u64, rows: &[Row]) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle,
        result: Ok(parse_listing(&listing_json(rows)).unwrap()),
    }
}

/// Every account lists nothing but `rows` under `owner` (recorded as
/// `started`, rule 3); the others list nothing.
fn listed_under(app: &mut App, owner: &str, rows: &[Row]) {
    for r in rows {
        app.sidecar
            .record_started(&format!("{}-0000-4000-8000-000000000000", r.id), owner);
    }
    for account in ["main", "alt"] {
        let rows: &[Row] = if account == owner { rows } else { &[] };
        app.apply_poll(update(account, 0, rows));
    }
}

/// Plays the refresh an attach waits for (doc/0005): fresh polls of both
/// accounts with the same rows, and a sidecar check that finds no file.
fn refresh(app: &mut App, effects: Vec<Effect>, owner: &str, rows: &[Row]) -> Vec<Effect> {
    assert!(
        effects.contains(&Effect::PollForAttach),
        "an attach refresh: {effects:?}"
    );
    app.attach_poll_started(common::ATTACH_CYCLE);
    let mut out = Vec::new();
    for account in ["main", "alt"] {
        let rows: &[Row] = if account == owner { rows } else { &[] };
        out.extend(app.apply_poll(update(account, common::ATTACH_CYCLE, rows)));
    }
    out.extend(common::run_sidecar_check(
        app,
        Path::new(common::NO_STATE_FILE),
    ));
    out
}

fn attach_of(effects: &[Effect]) -> AttachRequest {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Attach(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no attach in {effects:?}"))
}

fn start_of(effects: &[Effect]) -> OrchestratorJob {
    match effects {
        [Effect::StartOrchestrator(job)] => job.clone(),
        other => panic!("expected one orchestrator start, got {other:?}"),
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
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn untrusted() -> StartError {
    StartError::Dispatch(DispatchError::Untrusted {
        dir: "/x".into(),
        message: "Workspace not trusted. Run claude there first.".into(),
    })
}

// ---- an existing orchestrator ---------------------------------------------

#[test]
fn o_attaches_to_the_listed_orchestrator_with_its_owner() {
    let mut app = app_two();
    let rows = [
        Row {
            id: "aa11bb22",
            name: "fix-flaky-test",
            live: true,
            started: 10,
        },
        Row {
            id: "0c0c0c0c",
            name: "orchestrator",
            live: true,
            started: 20,
        },
    ];
    listed_under(&mut app, "alt", &rows);
    // The selection stays where it is: `o` does not need the row selected.
    let effects = app.handle_key(ch('o'));
    assert_eq!(effects, [Effect::PollForAttach, Effect::CheckSidecar]);
    assert!(status_text(&app).contains("checking orchestrator"));
    let effects = refresh(&mut app, effects, "alt", &rows);
    let attach = attach_of(&effects);
    assert_eq!(attach.account.name, "alt");
    assert_eq!(attach.id, "0c0c0c0c");
    assert_eq!(attach.name, "orchestrator");
}

#[test]
fn a_name_that_only_contains_orchestrator_does_not_count() {
    let mut app = app_two();
    let dir = tempfile::tempdir().unwrap();
    app.set_config_dir(dir.path());
    let rows = [Row {
        id: "0c0c0c0c",
        name: "orchestrator-old",
        live: true,
        started: 20,
    }];
    listed_under(&mut app, "main", &rows);
    start_of(&app.handle_key(ch('o')));
}

#[test]
fn an_unknown_owner_opens_the_account_picker() {
    let mut app = app_two();
    let rows = [Row {
        id: "0c0c0c0c",
        name: "orchestrator",
        live: false,
        started: 20,
    }];
    // Both accounts list it (shared lists) and nothing records an owner.
    app.apply_poll(update("main", 0, &rows));
    app.apply_poll(update("alt", 0, &rows));
    assert!(app.handle_key(ch('o')).is_empty());
    let Overlay::Picker(p) = app.overlay() else {
        panic!("expected the picker, got {:?}", app.overlay());
    };
    assert_eq!(p.id, "0c0c0c0c");
    assert_eq!(p.name, "orchestrator");
}

#[test]
fn duplicates_prefer_a_live_row_then_the_most_recent() {
    let mut app = app_two();
    let rows = [
        Row {
            id: "11111111",
            name: "orchestrator",
            live: false,
            started: 30,
        },
        Row {
            id: "22222222",
            name: "orchestrator",
            live: true,
            started: 10,
        },
        Row {
            id: "33333333",
            name: "orchestrator",
            live: true,
            started: 20,
        },
    ];
    listed_under(&mut app, "main", &rows);
    let effects = app.handle_key(ch('o'));
    let text = status_text(&app);
    assert!(
        text.contains(
            "3 sessions are named orchestrator; using the most recent live one (33333333)"
        ),
        "{text}"
    );
    let effects = refresh(&mut app, effects, "main", &rows);
    assert_eq!(attach_of(&effects).id, "33333333");

    // None live: the most recently started.
    let mut app = app_two();
    let rows = [
        Row {
            id: "11111111",
            name: "orchestrator",
            live: false,
            started: 10,
        },
        Row {
            id: "22222222",
            name: "orchestrator",
            live: false,
            started: 30,
        },
    ];
    listed_under(&mut app, "main", &rows);
    let effects = app.handle_key(ch('o'));
    assert!(status_text(&app).contains("using the most recent (22222222)"));
    let effects = refresh(&mut app, effects, "main", &rows);
    assert_eq!(attach_of(&effects).id, "22222222");
}

#[test]
fn nothing_happens_before_every_account_was_listed() {
    let mut app = app_two();
    assert!(app.handle_key(ch('o')).is_empty());
    app.apply_poll(update("main", 0, &[]));
    assert!(app.handle_key(ch('o')).is_empty());
    assert!(
        status_text(&app).contains("cannot look for the orchestrator yet: alt has not been listed")
    );
}

// ---- starting one -------------------------------------------------------------

#[test]
fn a_missing_orchestrator_is_started_with_the_exact_request() {
    let mut app = app_with(config(
        "claude",
        "claude-alt",
        "default_model = \"opus\"\ndefault_effort = \"high\"\ndefault_permission_mode = \"plan\"",
    ));
    let dir = tempfile::tempdir().unwrap();
    app.set_config_dir(dir.path());
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    assert!(!job.after_trust);
    let r = &job.request;
    assert_eq!(r.project, dir.path().join("orchestrator"));
    assert_eq!(r.account.name, "main");
    assert_eq!(r.name.as_deref(), Some("orchestrator"));
    assert_eq!(r.model_arg(), Some("opus"));
    assert_eq!(r.effort_arg(), Some("high"));
    // Never the dialog's default mode, and never bypass.
    assert_eq!(r.permission_mode_arg(), "auto");
    assert_eq!(r.task_arg(), orchestrator::START_PROMPT);
    assert!(r.task_arg().chars().count() >= watchtower::dispatch::MIN_TASK_CHARS);
    assert_eq!(
        r.args(),
        [
            "--bg",
            "--name",
            "orchestrator",
            "--model",
            "opus",
            "--effort",
            "high",
            "--permission-mode",
            "auto",
            orchestrator::START_PROMPT,
        ]
    );
    assert!(status_text(&app).contains("starting the orchestrator under main"));

    // A second `o` while it starts does not start another.
    assert!(app.handle_key(ch('o')).is_empty());
    assert!(status_text(&app).contains("the orchestrator is starting"));
}

#[test]
fn a_haiku_default_model_passes_no_model() {
    for model in ["haiku", "claude-haiku-4-5-20251001"] {
        let mut app = app_with(config(
            "claude",
            "claude-alt",
            &format!("default_model = \"{model}\""),
        ));
        listed_under(&mut app, "main", &[]);
        let job = start_of(&app.handle_key(ch('o')));
        assert_eq!(job.request.model_arg(), None, "{model}");
        assert!(!job.request.args().contains(&"--model".to_owned()));
    }
    // `default` passes none either.
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    assert_eq!(job.request.model_arg(), None);
    assert_eq!(job.request.effort_arg(), None);
}

#[test]
fn a_successful_start_records_the_owner_and_attaches() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    let started = Dispatched {
        id: "0d0d0d0d".into(),
        name: Some("orchestrator".into()),
    };
    let effects = app.orchestrator_finished(job, Ok(started));
    assert_eq!(effects.len(), 2, "{effects:?}");
    assert_eq!(effects[0], Effect::SaveSidecar);
    let attach = attach_of(&effects);
    assert_eq!(
        (attach.account.name.as_str(), attach.id.as_str()),
        ("main", "0d0d0d0d")
    );
    assert_eq!(
        app.sidecar.owner("0d0d0d0d"),
        Some(("main", &AccountSource::Started))
    );
    // Back from it before a poll lists it: `o` attaches again without
    // starting a second one (and without a refresh, it is not listed yet).
    let again = app.handle_key(ch('o'));
    assert_eq!(attach_of(&again).id, "0d0d0d0d");
    assert!(!again.contains(&Effect::PollForAttach));

    // Once listed, `o` goes through the normal attach.
    let rows = [Row {
        id: "0d0d0d0d",
        name: "orchestrator",
        live: true,
        started: 1,
    }];
    app.apply_poll(update("main", 1, &rows));
    let effects = app.handle_key(ch('o'));
    assert_eq!(effects, [Effect::PollForAttach, Effect::CheckSidecar]);
}

#[test]
fn a_just_started_orchestrator_that_never_shows_is_forgotten() {
    let mut app = app_two();
    let dir = tempfile::tempdir().unwrap();
    app.set_config_dir(dir.path());
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    let started = Dispatched {
        id: "0d0d0d0d".into(),
        name: None,
    };
    app.orchestrator_finished(job, Ok(started));
    for cycle in 1..=3 {
        app.apply_poll(update("main", cycle, &[]));
    }
    start_of(&app.handle_key(ch('o')));
}

#[test]
fn a_start_that_ends_while_a_window_is_open_does_not_attach() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    app.handle_key(ch('?'));
    let started = Dispatched {
        id: "0d0d0d0d".into(),
        name: None,
    };
    let effects = app.orchestrator_finished(job, Ok(started));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Attach(_))),
        "{effects:?}"
    );
    assert!(status_text(&app).contains("press o to attach"));
    assert_eq!(app.overlay(), &Overlay::Help);
}

#[test]
fn a_failed_start_is_reported_and_o_can_try_again() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    let err = StartError::Dispatch(DispatchError::Failed {
        code: Some(2),
        stderr: "boom".into(),
    });
    assert!(app.orchestrator_finished(job, Err(err)).is_empty());
    assert!(status_text(&app).contains("cannot start the orchestrator"));
    start_of(&app.handle_key(ch('o')));
}

#[test]
fn an_untrusted_folder_offers_the_trust_flow_then_retries_once() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    assert!(
        app.orchestrator_finished(job.clone(), Err(untrusted()))
            .is_empty()
    );
    let Overlay::OrchestratorUntrusted(u) = app.overlay().clone() else {
        panic!("expected the trust window, got {:?}", app.overlay());
    };
    assert_eq!(u.job, job);
    assert!(u.message.starts_with("Workspace not trusted"));
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("Not trusted by main"), "{screen}");
    assert!(screen.contains("Ctrl+T"), "{screen}");

    // Other keys leave it open; Ctrl+T runs the trust flow.
    assert!(app.handle_key(ch('o')).is_empty());
    let effects = app.handle_key(ctrl('t'));
    assert_eq!(effects, [Effect::OrchestratorTrust(job.clone())]);
    assert_eq!(app.overlay(), &Overlay::None);

    // The interactive command exited: one retry, marked as such.
    let retry = start_of(&app.orchestrator_trust_finished(job.clone(), Ok(Some(0))));
    assert!(retry.after_trust);
    assert_eq!(retry.request, job.request);

    // Still untrusted: the window comes back and says so; Esc gives up.
    app.orchestrator_finished(retry, Err(untrusted()));
    let Overlay::OrchestratorUntrusted(u) = app.overlay().clone() else {
        panic!("expected the trust window again");
    };
    assert!(u.message.contains("still not trusted"), "{}", u.message);
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("the orchestrator was not started"));
}

#[test]
fn a_trust_command_that_cannot_run_reopens_the_window() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    app.orchestrator_finished(job.clone(), Err(untrusted()));
    app.handle_key(ctrl('t'));
    let effects = app.orchestrator_trust_finished(job, Err("cannot run claude".into()));
    assert!(effects.is_empty());
    let Overlay::OrchestratorUntrusted(u) = app.overlay().clone() else {
        panic!("expected the trust window");
    };
    assert_eq!(u.message, "cannot run claude");
    // Not counted as starting any more: Esc, then `o` starts again.
    app.handle_key(key(KeyCode::Esc));
    start_of(&app.handle_key(ch('o')));
}

#[test]
fn help_and_footer_offer_o() {
    let mut app = app_two();
    listed_under(&mut app, "main", &[]);
    app.handle_key(ch('?'));
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("attach to the orchestrator"), "{screen}");
    assert!(screen.contains("Not available yet: / p i Tab"), "{screen}");
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 160, 20);
    let footer = screen.lines().last().unwrap();
    assert!(footer.contains("o orchestrator"), "{footer}");
}

// ---- the folder and CLAUDE.md -------------------------------------------------

#[test]
fn the_folder_is_private_and_the_instructions_are_written_once() {
    let root = tempfile::tempdir().unwrap();
    let dir = orchestrator::dir(&root.path().join("watchtower"));
    let tower = Path::new("/opt/tools/bin/tower");
    assert!(orchestrator::prepare(&dir, tower).unwrap());
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let file = dir.join("CLAUDE.md");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("`/opt/tools/bin/tower status --json`"),
        "{text}"
    );
    assert!(text.contains("`tower status --json`"), "{text}");
    assert!(!text.contains(orchestrator::TOWER_PLACEHOLDER));

    // The user's edits stay.
    std::fs::write(&file, "my own notes\n").unwrap();
    assert!(!orchestrator::prepare(&dir, Path::new("/other/tower")).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "my own notes\n");
}

#[test]
fn an_existing_folder_keeps_its_mode_and_a_symlinked_file_is_left_alone() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("orchestrator");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
    let target = root.path().join("shared.md");
    std::os::unix::fs::symlink(&target, dir.join("CLAUDE.md")).unwrap();
    assert!(!orchestrator::prepare(&dir, Path::new("/t/tower")).unwrap());
    assert!(!target.exists(), "a dangling link is not followed");
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o750);
}

#[test]
fn the_template_covers_the_role_tools_and_rules() {
    let t = orchestrator::TEMPLATE;
    assert!(t.contains(orchestrator::TOWER_PLACEHOLDER));
    for needle in [
        "report and relay",
        "status --json",
        "<accountCommand> logs <id>",
        "Never use another account's command",
        "ListAgents",
        "SendMessage",
        "notify_when_idle",
        "permission prompt",
        "`stop`, `rm`, `--bg`, `--resume`, `attach`, `respawn` or `daemon`",
        "Never change Claude Code settings",
        "unsure",
        "Keep answers short",
    ] {
        assert!(t.contains(needle), "{needle} missing");
    }
    // A path with a space stays one shell word.
    let text = orchestrator::instructions(Path::new("/opt/my tools/tower"));
    assert!(
        text.contains("`'/opt/my tools/tower' status --json`"),
        "{text}"
    );
}

// ---- the runtime ---------------------------------------------------------------

#[test]
fn the_start_runs_the_default_account_in_the_orchestrator_folder() {
    let _g = lock();
    let main = Fake::new(
        "claude",
        &format!(
            "{}\n{}",
            cat_err("cli-output/bg-success.stderr"),
            cat("cli-output/bg-success.stdout")
        ),
    );
    let alt = Fake::new("claude-alt", "exit 99");
    let quoted = |f: &Fake| format!("'{}'", f.program().display());
    let mut app = app_with(config(&quoted(&main), &quoted(&alt), ""));
    let root = tempfile::tempdir().unwrap();
    let config_dir = root.path().join("watchtower");
    app.set_config_dir(&config_dir);
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));

    let jobs = Jobs::new(CliRunner::new().with_timeout(Duration::from_secs(20)));
    jobs.orchestrator(job);
    let done = jobs
        .recv_timeout(Duration::from_secs(20))
        .expect("the start reports back");
    let effects = done.apply(&mut app);

    let dir = config_dir.join("orchestrator");
    assert_eq!(
        main.argv(),
        [
            "--bg",
            "--name",
            "orchestrator",
            "--permission-mode",
            "auto",
            orchestrator::START_PROMPT,
        ]
    );
    assert_eq!(main.cwd(), dir.canonicalize().unwrap());
    assert_eq!(alt.calls(), 0, "only the default account's command");
    let text = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    let exe = std::env::current_exe().unwrap();
    assert!(text.contains(&exe.display().to_string()), "{text}");
    // bg-success names the session `bad-model` with id 3e9a1c07.
    let attach = attach_of(&effects);
    assert_eq!(
        (attach.account.name.as_str(), attach.id.as_str()),
        ("main", "3e9a1c07")
    );
    assert_eq!(
        app.sidecar.owner("3e9a1c07"),
        Some(("main", &AccountSource::Started))
    );
}

#[test]
fn an_untrusted_start_comes_back_as_the_trust_window() {
    let _g = lock();
    let main = Fake::new(
        "claude",
        &format!("{}\nexit 1", cat_err("cli-output/bg-untrusted.stderr")),
    );
    let mut app = app_with(config(&format!("'{}'", main.program().display()), "x", ""));
    let root = tempfile::tempdir().unwrap();
    app.set_config_dir(root.path());
    listed_under(&mut app, "main", &[]);
    let job = start_of(&app.handle_key(ch('o')));
    let jobs = Jobs::new(CliRunner::new().with_timeout(Duration::from_secs(20)));
    jobs.orchestrator(job);
    let done = jobs.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(done.apply(&mut app).is_empty());
    assert!(matches!(app.overlay(), Overlay::OrchestratorUntrusted(_)));
    assert_eq!(main.calls(), 1);
}
