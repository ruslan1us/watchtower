//! Restarting sessions onto the installed Claude Code (doc/0047): `respawn`
//! through the owner's command, its confirmations, the flow over several
//! sessions and accounts, what it reports, and the workspace pane's client.
//! `--all` is never run, here or by tower: the fake only shows the argv.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, ctrl, key, status_text};
use fake_claude::{Fake, lock};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::actions::{self, RespawnError};
use watchtower::app::{
    ATTACH_DEBOUNCE, App, Confirm, Effect, Overlay, PaneFocus, Purpose, RestartCost, RestartJob,
    SessionAction,
};
use watchtower::cli::CliRunner;
use watchtower::config::{Config, UiStyle};
use watchtower::embed::PaneScreen;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::ui;

const NOW_MS: i64 = 1_800_000_000_000;

const CONFIG: &str = r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#;

fn row(id: &str, name: &str, process: &str, state: &str) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
            "cwd": "{HOME}/workspace/demo", "kind": "background", "name": "{name}",
            "state": "{state}", "startedAt": {NOW_MS} }}"#
    )
}

const IDLE: &str = r#""pid": 4100, "status": "idle","#;
const BUSY: &str = r#""pid": 4101, "status": "busy","#;
const WAITING: &str = r#""pid": 4102, "status": "waiting", "waitingFor": "permission prompt","#;

/// Running rows of both accounts, one of no known account, a stopped one and
/// an interactive one; see [`app`] for who owns what.
fn listing() -> String {
    let rows = [
        row("aaaa1111", "idle-main", IDLE, "done"),
        row("bbbb2222", "busy-main", BUSY, "working"),
        row("cccc3333", "asking-alt", WAITING, "blocked"),
        row("dddd4444", "task-alt", BUSY, "done"),
        row("eeee5555", "orchestrator", IDLE, "done"),
        row("ffff6666", "whose", IDLE, "done"),
        row("99990000", "stopped-main", "", "done"),
        format!(
            r#"{{ "pid": 4200, "cwd": "{HOME}/workspace/demo", "kind": "interactive",
                "startedAt": {NOW_MS}, "sessionId": "abcd-0000", "name": "inter" }}"#
        ),
    ];
    format!("[{}]", rows.join(","))
}

fn poll(account: &str, text: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(text).unwrap()),
    }
}

fn session_id(id: &str) -> String {
    format!("{id}-0000-4000-8000-000000000000")
}

fn app() -> App {
    let config = Config::parse(CONFIG, Path::new(HOME)).unwrap();
    let mut sidecar = Sidecar::default();
    for (id, owner) in [
        ("aaaa1111", "main"),
        ("bbbb2222", "main"),
        ("cccc3333", "alt"),
        ("dddd4444", "alt"),
        ("eeee5555", "main"),
        ("99990000", "main"),
    ] {
        sidecar.record_owner(&session_id(id), owner, AccountSource::Started);
    }
    let mut app = App::new(config, HOME.into(), sidecar, Box::new(NoEnviron));
    app.apply_poll(poll("main", &listing()));
    app.apply_poll(poll("alt", &listing()));
    app
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

fn select_header(app: &mut App, title: &str) {
    app.handle_key(key(KeyCode::Home));
    loop {
        if app.selected_header().is_some_and(|g| g.title() == title) {
            return;
        }
        let before = app.selected_entry();
        app.handle_key(ch('j'));
        assert_ne!(app.selected_entry(), before, "no header {title}");
    }
}

fn draw(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// [`listing`] with `name`'s row turned into `to`: `(process, state)`.
fn with_row(id: &str, name: &str, process: &str, state: &str) -> String {
    let rows: Vec<serde_json::Value> = serde_json::from_str(&listing()).unwrap();
    let new: serde_json::Value = serde_json::from_str(&row(id, name, process, state)).unwrap();
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| if r["id"] == id { new.clone() } else { r })
        .collect();
    serde_json::to_string(&rows).unwrap()
}

fn poll_both(app: &mut App, text: &str) {
    app.apply_poll(poll("main", text));
    app.apply_poll(poll("alt", text));
}

/// The window's words with its borders and line breaks gone.
fn window_text(app: &mut App) -> String {
    let screen = draw(app, 100, 40);
    let mut words: Vec<&str> = Vec::new();
    for line in screen.lines() {
        words.extend(
            line.split_whitespace()
                .filter(|w| !w.chars().all(|c| "│╭╮╰╯─".contains(c))),
        );
    }
    words.join(" ")
}

fn job_of(effects: &[Effect]) -> &RestartJob {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Restart(job) => Some(job),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no restart in {effects:?}"))
}

fn items(job: &RestartJob) -> Vec<(&str, &str)> {
    job.items
        .iter()
        .map(|a| (a.account.name.as_str(), a.id.as_str()))
        .collect()
}

// ---- the command ----------------------------------------------------------

#[test]
fn respawn_runs_the_owners_command_with_the_short_id() {
    let _guard = lock();
    let fake = Fake::new("claude", "echo 'respawned a1b2c3d4'");
    actions::respawn(&CliRunner::new(), &fake.account(), "a1b2c3d4").unwrap();
    assert_eq!(fake.argv(), ["respawn", "a1b2c3d4"]);
}

#[test]
fn only_the_cli_saying_so_counts_as_restarted() {
    let _guard = lock();
    let fake = Fake::new("claude", "echo something else");
    let err = actions::respawn(&CliRunner::new(), &fake.account(), "a1b2c3d4").unwrap_err();
    assert!(
        matches!(err, RespawnError::Failed { code: Some(0), .. }),
        "{err:?}"
    );
}

#[test]
fn an_unknown_id_and_a_failure_keep_the_clis_words() {
    let _guard = lock();
    // The CLI's own output for an unknown id (reference doc).
    let fake = Fake::new("claude", "echo \"No job matching 'a1b2c3d4'\" >&2; exit 1");
    let err = actions::respawn(&CliRunner::new(), &fake.account(), "a1b2c3d4").unwrap_err();
    assert!(matches!(err, RespawnError::UnknownId { .. }), "{err:?}");
    assert_eq!(err.to_string(), "No job matching 'a1b2c3d4'");

    let fake = Fake::new("claude", "echo 'the service is busy' >&2; exit 3");
    let err = actions::respawn(&CliRunner::new(), &fake.account(), "a1b2c3d4").unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("exit 3") && text.contains("the service is busy"),
        "{text}"
    );
}

#[test]
fn nothing_but_a_short_id_ever_reaches_the_command() {
    let _guard = lock();
    let fake = Fake::new("claude", "echo 'respawned --all'");
    for id in ["--all", "", "-x", "two words"] {
        let err = actions::respawn(&CliRunner::new(), &fake.account(), id).unwrap_err();
        assert!(matches!(err, RespawnError::InvalidId(_)), "{id}: {err:?}");
    }
    assert_eq!(fake.calls(), 0);
}

#[test]
fn the_worker_runs_each_session_with_its_own_accounts_command() {
    let _guard = lock();
    let main = Fake::new("claude", "echo \"respawned $2\"");
    // The second of alt's fails; the rest still run.
    let alt = Fake::new(
        "claude-alt",
        "if [ \"$2\" = bad00000 ]; then echo 'no' >&2; exit 2; fi; echo \"respawned $2\"",
    );
    let action = |fake: &Fake, account: &str, id: &str| SessionAction {
        account: fake.account_named(account, None),
        id: id.to_owned(),
        name: format!("name-{id}"),
    };
    let job = RestartJob {
        run: 7,
        items: vec![
            action(&main, "main", "aaaa1111"),
            action(&alt, "alt", "bad00000"),
            action(&alt, "alt", "cccc3333"),
            action(&main, "main", "bbbb2222"),
        ],
    };
    let jobs = Jobs::new(CliRunner::new());
    jobs.restart(job);
    let mut steps = Vec::new();
    let results = loop {
        match jobs
            .recv_timeout(Duration::from_secs(30))
            .expect("reports back")
        {
            JobResult::RestartStep { run, done } => {
                assert_eq!(run, 7);
                steps.push(done);
            }
            JobResult::Restart(job, results) => {
                assert_eq!(job.run, 7);
                break results;
            }
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(steps, [0, 1, 2, 3]);
    let ok: Vec<bool> = results.iter().map(Result::is_ok).collect();
    assert_eq!(ok, [true, false, true, true]);
    assert_eq!(
        main.all_argv(),
        [["respawn", "aaaa1111"], ["respawn", "bbbb2222"]]
    );
    assert_eq!(
        alt.all_argv(),
        [["respawn", "bad00000"], ["respawn", "cccc3333"]]
    );
}

// ---- one session ----------------------------------------------------------

#[test]
fn u_asks_first_naming_the_session_and_y_restarts_it_under_its_owner() {
    let mut app = app();
    select(&mut app, "idle-main");
    assert!(app.handle_key(ch('U')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Restart {
            cost: RestartCost::Nothing,
            ..
        })
    ));
    let text = window_text(&mut app);
    assert!(
        text.contains("Restart idle-main (aaaa1111, account main)"),
        "{text}"
    );
    assert!(
        text.contains("The conversation, id, name, account"),
        "{text}"
    );
    assert!(!text.contains("interrupted"), "{text}");

    let effects = app.handle_key(ch('y'));
    assert_eq!(items(job_of(&effects)), [("main", "aaaa1111")]);
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("restarting idle-main"));
}

#[test]
fn it_goes_by_the_rows_owner() {
    let mut app = app();
    select(&mut app, "asking-alt");
    app.handle_key(ch('U'));
    let effects = app.handle_key(ch('y'));
    assert_eq!(items(job_of(&effects)), [("alt", "cccc3333")]);
    assert_eq!(job_of(&effects).items[0].account.command, "claude-alt");
}

#[test]
fn a_running_turn_is_warned_about() {
    let mut app = app();
    select(&mut app, "busy-main");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("It is working"), "{text}");
    assert!(
        text.contains("the running turn is interrupted and does not continue"),
        "{text}"
    );
}

#[test]
fn a_pending_prompt_is_warned_about() {
    let mut app = app();
    select(&mut app, "asking-alt");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("It waits on a permission prompt"), "{text}");
    assert!(text.contains("the prompt is dropped"), "{text}");
}

#[test]
fn a_background_task_is_named_as_not_verified() {
    let mut app = app();
    select(&mut app, "task-alt");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("A background task"), "{text}");
    assert!(text.contains("not verified"), "{text}");
}

#[test]
fn n_esc_and_enter_cancel() {
    for answer in [ch('n'), ch('N'), key(KeyCode::Esc), key(KeyCode::Enter)] {
        let mut app = app();
        select(&mut app, "idle-main");
        app.handle_key(ch('U'));
        assert!(app.handle_key(answer).is_empty());
        assert_eq!(app.overlay(), &Overlay::None);
        assert!(status_text(&app).contains("restart cancelled"));
    }
}

#[test]
fn a_stopped_row_has_nothing_to_restart() {
    let mut app = app();
    select(&mut app, "stopped-main");
    assert!(app.handle_key(ch('U')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    let status = status_text(&app);
    assert!(status.contains("stopped-main is stopped"), "{status}");
}

#[test]
fn an_interactive_row_is_not_ours_to_restart() {
    let mut app = app();
    select(&mut app, "inter");
    assert!(app.handle_key(ch('U')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("runs in another terminal"));
}

#[test]
fn an_unknown_owner_is_asked_for_first() {
    let mut app = app();
    select(&mut app, "whose");
    app.handle_key(ch('U'));
    let Overlay::Picker(picker) = app.overlay() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(picker.purpose, Purpose::Restart);
    app.handle_key(ch('j'));
    app.handle_key(key(KeyCode::Enter));
    let Overlay::Confirm(Confirm::Restart { action, .. }) = app.overlay() else {
        panic!("{:?}", app.overlay());
    };
    assert_eq!(action.account.name, "alt");
    let effects = app.handle_key(ch('y'));
    assert_eq!(items(job_of(&effects)), [("alt", "ffff6666")]);
}

#[test]
fn a_cyrillic_layout_reaches_the_keys() {
    let mut app = app();
    select(&mut app, "idle-main");
    // `Г` is on the `U` key, shifted; `н` answers yes (doc/0011).
    app.handle_key(ch('Г'));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Restart { .. })
    ));
    assert!(!app.handle_key(ch('н')).is_empty());

    let mut app = self::app();
    app.handle_key(ctrl('г'));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::RestartMany(_))
    ));
    // Only `Н`, a capital, confirms it.
    assert!(app.handle_key(ch('н')).is_empty());
    assert!(!app.handle_key(ch('Н')).is_empty());
}

// ---- several sessions ------------------------------------------------------

#[test]
fn every_session_lists_how_many_on_each_account_and_only_a_capital_y_goes() {
    let mut app = app();
    select(&mut app, "idle-main");
    assert!(app.handle_key(ctrl('u')).is_empty());
    let text = window_text(&mut app);
    assert!(text.contains("Restart 5 sessions"), "{text}");
    assert!(text.contains("every running session"), "{text}");
    assert!(text.contains("main: 3 sessions"), "{text}");
    assert!(text.contains("alt: 2 sessions"), "{text}");
    assert!(text.contains("1 is working"), "{text}");
    assert!(text.contains("1 is waiting on a prompt"), "{text}");
    assert!(text.contains("1 is running a background task"), "{text}");
    assert!(
        text.contains("Left out: 1 stopped, 1 with an unknown account"),
        "{text}"
    );
    assert!(text.contains("Y (with Shift) restart"), "{text}");

    // A slip of `y`, or any other key, runs nothing and closes nothing.
    assert!(app.handle_key(ch('y')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::RestartMany(_))
    ));
    assert!(status_text(&app).contains("press Y"));
    assert!(app.handle_key(ch('x')).is_empty());
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::RestartMany(_))
    ));

    let effects = app.handle_key(ch('Y'));
    assert_eq!(
        items(job_of(&effects)),
        [
            ("main", "eeee5555"),
            ("main", "aaaa1111"),
            ("main", "bbbb2222"),
            ("alt", "cccc3333"),
            ("alt", "dddd4444"),
        ]
    );
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("restarting 5 sessions"));
}

#[test]
fn the_stronger_window_cancels_too() {
    for answer in [ch('n'), key(KeyCode::Esc), key(KeyCode::Enter)] {
        let mut app = app();
        app.handle_key(ctrl('u'));
        assert!(app.handle_key(answer).is_empty());
        assert_eq!(app.overlay(), &Overlay::None);
        assert!(status_text(&app).contains("restart cancelled"));
    }
}

#[test]
fn every_session_ignores_the_filter_and_says_so_by_its_counts() {
    let mut app = app();
    app.handle_key(ch('/'));
    common::type_text(&mut app, "idle-main");
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('u'));
    assert!(window_text(&mut app).contains("Restart 5 sessions"));
}

#[test]
fn u_on_a_group_header_restarts_that_group() {
    let mut app = app();
    select_header(&mut app, "Working");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("Restart 1 session"), "{text}");
    assert!(text.contains("the Working group"), "{text}");
    assert!(text.contains("main: 1 session"), "{text}");
    let effects = app.handle_key(ch('Y'));
    assert_eq!(items(job_of(&effects)), [("main", "bbbb2222")]);
}

#[test]
fn u_on_the_pinned_groups_header_goes_by_its_rows() {
    let mut app = app();
    select(&mut app, "idle-main");
    app.handle_key(ch('P'));
    select(&mut app, "asking-alt");
    app.handle_key(ch('P'));
    select_header(&mut app, "Pinned");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("Restart 2 sessions"), "{text}");
    assert!(text.contains("the Pinned group"), "{text}");
    let effects = app.handle_key(ch('Y'));
    assert_eq!(
        items(job_of(&effects)),
        [("main", "aaaa1111"), ("alt", "cccc3333")]
    );
}

#[test]
fn u_on_a_project_header_names_the_project() {
    let mut app = app();
    app.handle_key(ch('p'));
    select_header(&mut app, "~/workspace/demo");
    app.handle_key(ch('U'));
    let text = window_text(&mut app);
    assert!(text.contains("the project ~/workspace/demo"), "{text}");
    assert!(text.contains("Restart 4 sessions"), "{text}");
}

#[test]
fn a_group_with_nothing_running_says_so() {
    let mut app = app();
    select_header(&mut app, "Stopped & finished");
    assert!(app.handle_key(ch('U')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    let status = status_text(&app);
    assert!(
        status.contains("nothing to restart in the Stopped & finished group"),
        "{status}"
    );
    assert!(status.contains("1 stopped"), "{status}");
}

#[test]
fn no_sessions_at_all_says_so() {
    let config = Config::parse(CONFIG, Path::new(HOME)).unwrap();
    let mut app = App::new(config, HOME.into(), Sidecar::default(), Box::new(NoEnviron));
    app.handle_key(ctrl('u'));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("no running sessions"));
}

// ---- while it runs, and what it reports ------------------------------------

/// `idle-main` is being restarted.
fn running(app: &mut App) -> RestartJob {
    select(app, "idle-main");
    app.handle_key(ch('U'));
    job_of(&app.handle_key(ch('y'))).clone()
}

#[test]
fn while_it_restarts_the_row_refuses_every_other_action() {
    let mut app = app();
    running(&mut app);
    for k in ['s', 'd', 'a', 'e', 't', 'U'] {
        let effects = app.handle_key(ch(k));
        assert!(effects.is_empty(), "{k}: {effects:?}");
        assert_eq!(app.overlay(), &Overlay::None, "{k}");
        assert!(
            status_text(&app).contains("tower is restarting idle-main"),
            "{k}: {}",
            status_text(&app)
        );
    }
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Attach(_))),
        "{effects:?}"
    );
}

#[test]
fn a_second_restart_leaves_out_what_is_on_its_way() {
    let mut app = app();
    running(&mut app);
    app.handle_key(ctrl('u'));
    let text = window_text(&mut app);
    assert!(text.contains("Restart 4 sessions"), "{text}");
    assert!(text.contains("1 tower is working on"), "{text}");
}

#[test]
fn it_says_how_far_it_got() {
    let mut app = app();
    app.handle_key(ctrl('u'));
    let job = job_of(&app.handle_key(ch('Y'))).clone();
    assert!(status_text(&app).contains("restarting 5 sessions (0 done)"));
    app.restart_step(job.run, 3);
    assert!(status_text(&app).contains("restarting 5 sessions (3 done)"));
    let results = job.items.iter().map(|_| Ok(())).collect();
    assert_eq!(app.restart_finished(job, results), [Effect::PollNow]);
    let status = status_text(&app);
    assert!(
        status.contains("restarted 5 sessions (main 3, alt 2)"),
        "{status}"
    );
    assert!(!status.contains("restarting"), "{status}");
}

#[test]
fn one_restarted_and_one_that_failed_on_the_status_line() {
    let mut app = app();
    let job = running(&mut app);
    assert_eq!(app.restart_finished(job, vec![Ok(())]), [Effect::PollNow]);
    assert!(status_text(&app).contains("restarted idle-main (aaaa1111)"));
    assert!(app.popup().is_none());

    let job = running(&mut app);
    let err = RespawnError::UnknownId {
        message: "No job matching 'aaaa1111'".to_owned(),
    };
    assert_eq!(app.restart_finished(job, vec![Err(err)]), [Effect::PollNow]);
    let status = status_text(&app);
    assert!(
        status.contains("cannot restart idle-main: No job matching"),
        "{status}"
    );
    assert!(app.popup().is_none());
    // The row can be tried again.
    app.handle_key(ch('U'));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::Restart { .. })
    ));
}

#[test]
fn several_failures_open_a_window_naming_each_account() {
    let mut app = app();
    app.handle_key(ctrl('u'));
    let job = job_of(&app.handle_key(ch('Y'))).clone();
    let results = job
        .items
        .iter()
        .map(|a| {
            if a.account.name == "alt" {
                Err(RespawnError::Failed {
                    code: Some(1),
                    message: format!("alt refused {}", a.id),
                })
            } else {
                Ok(())
            }
        })
        .collect();
    assert_eq!(app.restart_finished(job, results), [Effect::PollNow]);
    let status = status_text(&app);
    assert!(
        status.contains("restarted 3 of 5 sessions; 2 failed"),
        "{status}"
    );
    let popup = app.popup().expect("a window with the answers");
    let text = popup.lines.join("\n");
    assert!(
        text.contains("asking-alt (cccc3333, account alt)"),
        "{text}"
    );
    assert!(text.contains("alt refused cccc3333"), "{text}");
    assert!(text.contains("task-alt (dddd4444, account alt)"), "{text}");
    assert!(!text.contains("idle-main"), "{text}");
}

#[test]
fn the_window_of_failures_waits_behind_an_open_one() {
    let mut app = app();
    app.handle_key(ctrl('u'));
    let job = job_of(&app.handle_key(ch('Y'))).clone();
    app.handle_key(ch('?'));
    let results = job
        .items
        .iter()
        .map(|_| {
            Err(RespawnError::Failed {
                code: Some(1),
                message: "no".to_owned(),
            })
        })
        .collect();
    app.restart_finished(job, results);
    assert_eq!(app.overlay(), &Overlay::Help);
    assert!(app.popup().is_none());
    assert_eq!(app.waiting_windows(), 1);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.popup().is_some());
}

#[test]
fn the_selection_comes_back_when_the_row_was_listed_stopped_meanwhile() {
    let mut app = app();
    let job = running(&mut app);
    // While `respawn` runs the row has no process: it goes to the folded
    // Stopped & finished, whose header stands in for it.
    poll_both(&mut app, &with_row("aaaa1111", "idle-main", "", "done"));
    assert!(app.selected_session().is_none());
    assert_eq!(
        app.selected_header()
            .map(|g| g.title().to_owned())
            .as_deref(),
        Some("Stopped & finished")
    );
    app.restart_finished(job, vec![Ok(())]);
    // A listing from before the end may still say so.
    poll_both(&mut app, &with_row("aaaa1111", "idle-main", "", "done"));
    assert!(app.selected_session().is_none());
    // Back with a new process.
    let live = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "idle","#,
        "done",
    );
    poll_both(&mut app, &live);
    assert_eq!(
        app.selected_session().unwrap().session.display_name(),
        "idle-main"
    );
}

#[test]
fn moving_on_during_the_restart_is_not_undone() {
    let mut app = app();
    let job = running(&mut app);
    poll_both(&mut app, &with_row("aaaa1111", "idle-main", "", "done"));
    select(&mut app, "busy-main");
    let live = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "idle","#,
        "done",
    );
    app.restart_finished(job, vec![Ok(())]);
    poll_both(&mut app, &live);
    assert_eq!(
        app.selected_session().unwrap().session.display_name(),
        "busy-main"
    );
}

/// The row's recorded change time.
fn change_of(app: &App, id: &str) -> Option<i64> {
    app.sidecar.sessions[&session_id(id)].last_change_ms
}

#[test]
fn a_restart_that_changes_nothing_keeps_the_rows_time() {
    let mut app = app();
    let before = change_of(&app, "aaaa1111");
    assert!(before.is_some());
    let job = running(&mut app);
    poll_both(&mut app, &with_row("aaaa1111", "idle-main", "", "done"));
    assert_eq!(change_of(&app, "aaaa1111"), before, "while it restarts");
    app.restart_finished(job, vec![Ok(())]);
    let live = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "idle","#,
        "done",
    );
    poll_both(&mut app, &live);
    assert_eq!(
        change_of(&app, "aaaa1111"),
        before,
        "the same tuple is back"
    );
    // Quiet no more: a later change counts.
    let busy = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "busy","#,
        "working",
    );
    poll_both(&mut app, &busy);
    assert_ne!(change_of(&app, "aaaa1111"), before);
}

#[test]
fn an_interrupted_turn_is_a_change_and_asks_for_attention() {
    let mut app = app();
    select(&mut app, "busy-main");
    assert!(!app.is_highlighted(app.selected_session().unwrap()));
    app.handle_key(ch('U'));
    let job = job_of(&app.handle_key(ch('y'))).clone();
    app.restart_finished(job, vec![Ok(())]);
    // As the real CLI lists it after a running turn was cut (reference doc).
    let cut = with_row(
        "bbbb2222",
        "busy-main",
        r#""pid": 7000, "status": "idle","#,
        "blocked",
    );
    poll_both(&mut app, &cut);
    let row = app
        .sessions()
        .iter()
        .find(|m| m.session.display_name() == "busy-main")
        .unwrap();
    assert!(app.is_highlighted(row), "it waits for you now");
}

#[test]
fn a_failed_restart_leaves_the_row_as_it_was() {
    let mut app = app();
    let before = change_of(&app, "aaaa1111");
    let job = running(&mut app);
    let err = RespawnError::Failed {
        code: Some(1),
        message: "no".to_owned(),
    };
    app.restart_finished(job, vec![Err(err)]);
    poll_both(&mut app, &listing());
    assert_eq!(change_of(&app, "aaaa1111"), before);
    let busy = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "busy","#,
        "working",
    );
    poll_both(&mut app, &busy);
    assert_ne!(change_of(&app, "aaaa1111"), before, "tracked as ever");
}

#[test]
fn the_help_lists_the_keys_and_still_fits_24_rows() {
    let mut app = app();
    app.handle_key(ch('?'));
    let screen = draw(&mut app, 80, 24);
    assert!(screen.contains("P, U"), "{screen}");
    assert!(screen.contains("Ctrl+U: all"), "{screen}");
    assert!(screen.contains("Esc or ? closes this help"), "{screen}");
}

// ---- the workspace --------------------------------------------------------

fn workspace() -> App {
    let mut app = app();
    app.style = UiStyle::Workspace;
    draw(&mut app, 160, 40);
    assert!(app.pane_active());
    app
}

/// `name`'s client open in the pane; the clock after it opened.
fn attached(app: &mut App, name: &str) -> Instant {
    select(app, name);
    let t0 = Instant::now();
    app.pane_tick(t0);
    let t = t0 + ATTACH_DEBOUNCE;
    let effects = app.pane_tick(t);
    let open = effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneOpen(open) => Some(open.clone()),
            _ => None,
        })
        .expect("attached");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    t
}

fn opens(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::PaneOpen(open) => Some(format!("{} {}", open.account.name, open.id)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_pane_closes_its_client_first_and_attaches_again_when_the_row_is_back() {
    let mut app = workspace();
    let t = attached(&mut app, "idle-main");
    let job = running_in_pane(&mut app);
    let effect = Effect::Restart(job.clone());

    // The runtime asks this right before it runs the effect: the client goes.
    assert!(app.pane_before(&effect), "the runtime closes the client");
    assert_eq!(app.pane_client_key(), None);
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(!app.pane_before(&effect), "only once");

    // Nothing attaches while it restarts, however long that takes.
    let t = t + Duration::from_secs(1);
    assert!(opens(&app.pane_tick(t)).is_empty());
    let m = app.selected_session().unwrap();
    assert!(app.pane_row_busy(m));

    assert_eq!(app.restart_finished(job, vec![Ok(())]), [Effect::PollNow]);
    let effects = app.pane_tick(t + Duration::from_secs(1));
    assert_eq!(opens(&effects), ["main aaaa1111"]);
}

/// `U` and `y` on the selected row, in the pane.
fn running_in_pane(app: &mut App) -> RestartJob {
    app.handle_key(ch('U'));
    job_of(&app.handle_key(ch('y'))).clone()
}

#[test]
fn the_pane_attaches_again_only_once_the_row_is_listed_live() {
    let mut app = workspace();
    let t = attached(&mut app, "idle-main");
    let job = running_in_pane(&mut app);
    assert!(app.pane_before(&Effect::Restart(job.clone())));
    // The real CLI lists it without a process while `respawn` runs.
    poll_both(&mut app, &with_row("aaaa1111", "idle-main", "", "done"));
    let t = t + Duration::from_secs(1);
    assert!(opens(&app.pane_tick(t)).is_empty());
    app.restart_finished(job, vec![Ok(())]);
    let t = t + Duration::from_secs(1);
    assert!(opens(&app.pane_tick(t)).is_empty(), "no process yet");
    let live = with_row(
        "aaaa1111",
        "idle-main",
        r#""pid": 7000, "status": "idle","#,
        "done",
    );
    poll_both(&mut app, &live);
    assert!(opens(&app.pane_tick(t)).is_empty(), "the usual rest");
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE);
    assert_eq!(opens(&effects), ["main aaaa1111"]);
}

#[test]
fn a_failed_restart_does_not_attach_again_until_asked() {
    let mut app = workspace();
    let t = attached(&mut app, "idle-main");
    let job = running_in_pane(&mut app);
    assert!(app.pane_before(&Effect::Restart(job.clone())));
    let err = RespawnError::Failed {
        code: Some(1),
        message: "no".to_owned(),
    };
    app.restart_finished(job, vec![Err(err)]);
    let t = t + Duration::from_secs(1);
    assert!(opens(&app.pane_tick(t)).is_empty());
    assert!(app.pane_held().is_some(), "the panel says why");
    assert!(status_text(&app).contains("cannot restart idle-main"));
    // `Enter` asks.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(opens(&effects), ["main aaaa1111"]);
}

#[test]
fn a_restart_of_other_sessions_leaves_the_client_alone() {
    let mut app = workspace();
    attached(&mut app, "idle-main");
    let other = Effect::Restart(RestartJob {
        run: 9,
        items: vec![SessionAction {
            account: app.config().accounts[1].clone(),
            id: "cccc3333".to_owned(),
            name: "asking-alt".to_owned(),
        }],
    });
    assert!(!app.pane_before(&other));
    assert_eq!(
        app.pane_client_key().map(str::to_owned),
        Some(session_id("aaaa1111"))
    );
}

#[test]
fn a_restart_of_every_session_closes_the_client_among_them() {
    let mut app = workspace();
    let t = attached(&mut app, "idle-main");
    app.handle_key(ctrl('u'));
    let job = job_of(&app.handle_key(ch('Y'))).clone();
    assert!(app.pane_before(&Effect::Restart(job.clone())));
    let results = job.items.iter().map(|_| Ok(())).collect();
    app.restart_finished(job, results);
    let effects = app.pane_tick(t + Duration::from_secs(1));
    assert_eq!(opens(&effects), ["main aaaa1111"]);
}

#[test]
fn moving_on_during_the_restart_attaches_where_the_selection_is() {
    let mut app = workspace();
    let t = attached(&mut app, "idle-main");
    let job = running_in_pane(&mut app);
    assert!(app.pane_before(&Effect::Restart(job.clone())));
    select(&mut app, "busy-main");
    let t = t + Duration::from_secs(1);
    app.pane_tick(t);
    app.restart_finished(job, vec![Ok(())]);
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE);
    assert_eq!(opens(&effects), ["main bbbb2222"]);
}

#[test]
fn other_styles_have_no_client_to_close() {
    let mut app = app();
    let job = running(&mut app);
    assert!(!app.pane_before(&Effect::Restart(job)));
}

#[test]
fn on_home_u_says_to_choose_and_ctrl_u_still_works() {
    let mut app = workspace();
    assert!(app.pane_home());
    assert!(app.handle_key(ch('U')).is_empty());
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(status_text(&app).contains("choose a session first"));
    assert!(app.handle_key(ch('P')).is_empty());
    assert!(app.sidecar.pins.is_empty());
    app.handle_key(ctrl('u'));
    assert!(matches!(
        app.overlay(),
        Overlay::Confirm(Confirm::RestartMany(_))
    ));
}
