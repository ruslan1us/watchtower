//! `R`: resuming a past conversation into the background through Claude
//! Code's own picker, `<account command> --resume` (doc/0019). The window
//! (account and project folder), its defaults and keys, the hand-over to a
//! fake account command, and the report afterwards. No real `claude` runs.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{app_single, app_two, ch, ctrl, key, poll};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect, ResumeOutcome, ResumeRequest};
use watchtower::attach::{
    Handover, InheritSpawner, RESUME_MIN_GAP, Spawner, Termination, resume_delay, resume_past,
};
use watchtower::dialog::ResumeField;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::projects::{Project, ProjectList};

/// Projects under `/home/user/workspace`, most recently used first.
fn projects(names: &[&str]) -> ProjectList {
    ProjectList {
        projects: names
            .iter()
            .map(|n| {
                let path = PathBuf::from(format!("/home/user/workspace/{n}"));
                Project {
                    canonical: path.clone(),
                    display: format!("~/workspace/{n}"),
                    path,
                    last_used_ms: None,
                    extra: false,
                }
            })
            .collect(),
        warnings: Vec::new(),
    }
}

/// A listing of `account` with one stopped background row named `name` in
/// `cwd`.
fn one_row(account: &str, name: &str, cwd: &str) -> PollUpdate {
    let rows = format!(
        r#"[{{"id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000000",
            "cwd": "{cwd}", "kind": "background", "name": "{name}", "state": "done"}}]"#
    );
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(&rows).unwrap()),
    }
}

/// Presses `R` and fills the window with `list` from the scan it asked for.
fn open(app: &mut App, list: ProjectList) {
    let effects = app.handle_key(ch('R'));
    let [Effect::ScanProjects(scan)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert!(app.resume_window().unwrap().scanning);
    app.projects_found(scan.id, list);
}

fn chosen(app: &App) -> (String, String) {
    let w = app.resume_window().expect("the resume window is open");
    (
        w.account.value().to_owned(),
        w.selected_project()
            .map(|p| p.display.clone())
            .unwrap_or_default(),
    )
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

#[test]
fn without_a_row_the_default_account_and_most_recent_project() {
    let mut app = app_two();
    open(&mut app, projects(&["web", "api", "demo-app"]));
    assert_eq!(chosen(&app), ("main".into(), "~/workspace/web".into()));
    let w = app.resume_window().unwrap();
    assert_eq!(w.field, ResumeField::Project);
    assert!(!w.scanning);
}

#[test]
fn the_selected_rows_owner_and_folder_are_preselected() {
    use watchtower::sidecar::{AccountSource, Sidecar};
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(
        "1a2b3c4d-0000-4000-8000-000000000000",
        "alt",
        AccountSource::Started,
    );
    let mut app = common::app_two_with(sidecar);
    // Shared lists; the sidecar says alt (doc/0003). It runs in a worktree
    // inside its project.
    for account in ["main", "alt"] {
        app.apply_poll(one_row(
            account,
            "fix-api",
            "/home/user/workspace/api/.claude/worktrees/fix-api",
        ));
    }
    common::select_name(&mut app, "fix-api");
    assert_eq!(app.selected_session().unwrap().owner.label(), "alt");
    open(&mut app, projects(&["web", "api", "demo-app"]));
    assert_eq!(chosen(&app), ("alt".into(), "~/workspace/api".into()));
}

#[test]
fn an_unknown_owner_or_unlisted_folder_falls_back() {
    let mut app = app_two();
    // Both accounts list it and nothing tells them apart: owner unknown.
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    common::select_name(&mut app, "say-ok");
    assert_eq!(app.selected_session().unwrap().owner.label(), "?");
    // Its folder (demo-app) is not a listed project.
    open(&mut app, projects(&["web", "api"]));
    assert_eq!(chosen(&app), ("main".into(), "~/workspace/web".into()));
}

#[test]
fn keys_choose_and_enter_runs_the_picker() {
    let mut app = app_two();
    open(&mut app, projects(&["web", "api", "demo-app"]));
    // Typing filters, ↓ moves.
    for c in "ap".chars() {
        app.handle_key(ch(c));
    }
    assert_eq!(chosen(&app).1, "~/workspace/api");
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(chosen(&app).1, "~/workspace/api");
    // Tab to the account, → changes it.
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.resume_window().unwrap().field, ResumeField::Account);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(chosen(&app).0, "alt");
    let effects = app.handle_key(key(KeyCode::Enter));
    let [Effect::ResumePast(req)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(req.account.name, "alt");
    assert_eq!(req.account.command, "claude-alt");
    assert_eq!(req.folder, Path::new("/home/user/workspace/api"));
    // Subagent tracking was never enabled (as in the other tests).
    assert_eq!(req.settings, None);
    assert!(app.resume_window().is_none());
}

/// Presses `R`, then `Enter` on the first project, and returns the request.
fn confirmed_request(app: &mut App) -> ResumeRequest {
    open(app, projects(&["web"]));
    let effects = app.handle_key(key(KeyCode::Enter));
    let [Effect::ResumePast(req)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    req.clone()
}

#[test]
fn the_request_carries_the_settings_file_while_tracking() {
    // doc/0039: `R` passes the same `--settings` file as a dispatch.
    let config_dir = Path::new("/home/user/.config/watchtower");
    let mut on = app_single();
    on.enable_subagents(config_dir);
    assert_eq!(
        confirmed_request(&mut on).settings,
        Some(config_dir.join("claude-settings.json"))
    );

    let home = Path::new(common::HOME);
    let mut config = watchtower::config::Config::defaults(home);
    config.track_subagents = false;
    let mut off = App::new(
        config,
        home.into(),
        watchtower::sidecar::Sidecar::default(),
        Box::new(common::NoEnviron),
    );
    off.enable_subagents(config_dir);
    assert_eq!(confirmed_request(&mut off).settings, None);
}

#[test]
fn esc_cancels_and_ctrl_c_quits() {
    let mut app = app_two();
    open(&mut app, projects(&["web"]));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.resume_window().is_none());

    open(&mut app, projects(&["web"]));
    assert_eq!(app.handle_key(ctrl('c')), vec![Effect::Quit]);
}

#[test]
fn enter_needs_a_project() {
    let mut app = app_single();
    let effects = app.handle_key(ch('R'));
    assert!(matches!(effects.as_slice(), [Effect::ScanProjects(_)]));
    // Still scanning.
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(
        app.resume_window().unwrap().error.as_deref(),
        Some("still scanning projects…")
    );
    let Effect::ScanProjects(scan) = &effects[0] else {
        unreachable!()
    };
    app.projects_found(scan.id, ProjectList::default());
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(
        app.resume_window().unwrap().error.as_deref(),
        Some("choose a project folder first")
    );
    let all = render(&mut app, 120, 30);
    assert!(all.contains("no projects found"), "{all}");
}

#[test]
fn the_window_shows_fields_and_how_to_come_back() {
    let mut app = app_two();
    open(&mut app, projects(&["web", "api"]));
    let all = render(&mut app, 120, 30);
    assert!(all.contains("Resume a past conversation"), "{all}");
    assert!(all.contains("▶ ~/workspace/web"), "{all}");
    assert!(all.contains("‹ main ›"), "{all}");
    assert!(
        all.contains("Enter runs `claude --resume` in the folder"),
        "{all}"
    );
    assert!(all.contains("(Ctrl+A"), "{all}");
    assert!(all.contains("there shows all projects"), "{all}");
    // The note wraps at spaces; its sentences are searched across rows.
    let flat = all
        .lines()
        .map(|l| l.trim_matches(|c: char| " │┃".contains(c)))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        flat.contains("press ← on an empty prompt and Ctrl+C twice to come back to tower"),
        "{all}"
    );
    assert!(
        all.contains("(Esc there reopens the conversation)"),
        "{all}"
    );
    assert!(all.contains("as a copy"), "{all}");
    assert!(!all.contains("/bg"), "{all}");
    // `q` is typed into the filter; it does not quit.
    assert_eq!(app.handle_key(ch('q')), Vec::new());
    assert_eq!(app.resume_window().unwrap().filter.text(), "q");
}

#[test]
fn the_keys_drop_whole_hints_on_one_row() {
    // doc/0019: Enter and Esc stay longest, `type to filter` goes first.
    let mut app = app_two();
    open(&mut app, projects(&["web", "api"]));
    let full = "Tab/Shift+Tab field · ↑↓ choose · type to filter · Enter open the picker · Esc \
                cancel";
    for (w, h, want) in [
        (
            80,
            24,
            "Tab/Shift+Tab field · ↑↓ choose · Enter open the picker · Esc cancel",
        ),
        (100, 30, full),
        (104, 32, full),
    ] {
        let all = render(&mut app, w, h);
        let row = all
            .lines()
            .find(|l| l.contains("Tab/Shift+Tab field"))
            .unwrap_or_else(|| panic!("{w}x{h}\n{all}"))
            .trim_matches(|c: char| " │╭╮╰╯├┤─".contains(c));
        assert_eq!(row, want, "{w}x{h}");
    }
}

#[test]
fn back_from_the_picker_polls_at_once() {
    let mut app = app_two();
    let effects = app.resume_finished(ResumeOutcome {
        account: "alt".into(),
        folder: "~/workspace/api".into(),
        result: Ok(Some(0)),
        settings_error: None,
    });
    assert_eq!(effects, vec![Effect::PollNow]);
    let status = common::status_text(&app);
    assert!(
        status.contains("back from claude --resume in ~/workspace/api (alt)"),
        "{status}"
    );
    let effects = app.resume_finished(ResumeOutcome {
        account: "alt".into(),
        folder: "~/workspace/api".into(),
        result: Ok(Some(1)),
        settings_error: None,
    });
    assert_eq!(effects, vec![Effect::PollNow]);
    assert!(common::status_text(&app).contains("exited with code 1"));
}

#[test]
fn r_is_in_the_help() {
    let mut app = app_single();
    app.handle_key(ch('?'));
    // The help still fits a 24-row terminal (doc/0011).
    let all = render(&mut app, 100, 24);
    assert!(
        all.contains("resume a past conversation (claude --resume)"),
        "{all}"
    );
    assert!(all.contains("Esc or ? closes this help"), "{all}");
}

// ---- the hand-over ----------------------------------------------------------

/// Records the order of terminal operations.
#[derive(Default)]
struct FakeTerm {
    log: Vec<String>,
}

impl Handover for FakeTerm {
    fn release(&mut self) -> io::Result<()> {
        self.log.push("release".into());
        Ok(())
    }
    fn announce(&mut self, line: &str) -> io::Result<()> {
        self.log.push(format!("announce {line}"));
        Ok(())
    }
    fn child_exited(&mut self, failed: bool) {
        self.log.push(format!("exited failed={failed}"));
    }
    fn wait_for_key(&mut self, prompt: &str) -> io::Result<()> {
        self.log.push(format!("wait {prompt}"));
        Ok(())
    }
    fn reclaim(&mut self) -> io::Result<()> {
        self.log.push("reclaim".into());
        Ok(())
    }
}

#[test]
fn the_picker_runs_in_the_folder_with_no_other_argument() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("claude-alt", "exit 0");
    let folder = tempfile::tempdir().unwrap();
    let req = ResumeRequest {
        account: fake.account_named("alt", Some("~/.claude-alt")),
        folder: folder.path().to_path_buf(),
        settings: None,
    };
    let mut term = FakeTerm::default();
    let outcome = resume_past(
        &mut term,
        &mut InheritSpawner,
        &req,
        "~/workspace/api",
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(0)));
    assert_eq!(outcome.account, "alt");
    assert_eq!(fake.argv(), ["--resume"]);
    assert_eq!(
        fake.cwd().canonicalize().unwrap(),
        folder.path().canonicalize().unwrap()
    );
    assert_eq!(term.log.len(), 4, "{:?}", term.log);
    assert_eq!(term.log[0], "release");
    let announce = &term.log[1];
    assert!(announce.starts_with("announce tower: `"), "{announce}");
    assert!(
        announce.contains(
            " --resume` in ~/workspace/api (alt): pick a conversation (Ctrl+A shows all \
             projects); then press ← on an empty prompt and Ctrl+C twice to come back to tower \
             (Esc there reopens the conversation) — it keeps running in the background as a \
             copy and appears in tower"
        ),
        "{announce}"
    );
    assert_eq!(term.log[2], "exited failed=false");
    assert_eq!(term.log[3], "reclaim");
}

/// The announcement's text after the account command, the same with and
/// without `--settings` (doc/0039).
const ANNOUNCED: &str = " --resume` in ~/workspace/api (alt): pick a conversation";

#[test]
fn the_picker_gets_the_settings_file_before_resume() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("claude-alt", "exit 0");
    let folder = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let settings = config_dir.path().join("claude-settings.json");
    let req = ResumeRequest {
        account: fake.account_named("alt", Some("~/.claude-alt")),
        folder: folder.path().to_path_buf(),
        settings: Some(settings.clone()),
    };
    let mut term = FakeTerm::default();
    let outcome = resume_past(
        &mut term,
        &mut InheritSpawner,
        &req,
        "~/workspace/api",
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(0)));
    assert_eq!(outcome.settings_error, None);
    assert_eq!(
        fake.argv(),
        ["--settings", settings.to_str().unwrap(), "--resume"]
    );
    // Written first, as for a dispatch: only the three hooks.
    let text = std::fs::read_to_string(&settings).unwrap();
    for hook in [
        "SubagentStart",
        "SubagentStop",
        "PostToolUse",
        " hook --dir ",
    ] {
        assert!(text.contains(hook), "{text}");
    }
    // The printed line is unchanged.
    assert!(term.log[1].contains(ANNOUNCED), "{:?}", term.log);
    assert!(!term.log[1].contains("--settings"), "{:?}", term.log);
}

#[test]
fn an_unusable_settings_file_runs_without_it_and_warns_once() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("claude-alt", "exit 0");
    let folder = tempfile::tempdir().unwrap();
    // A relative config dir is refused for the hook command (doc/0029).
    let req = ResumeRequest {
        account: fake.account_named("alt", Some("~/.claude-alt")),
        folder: folder.path().to_path_buf(),
        settings: Some(PathBuf::from("relative-config/claude-settings.json")),
    };
    let mut term = FakeTerm::default();
    let outcome = resume_past(
        &mut term,
        &mut InheritSpawner,
        &req,
        "~/workspace/api",
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(0)));
    assert_eq!(fake.argv(), ["--resume"]);
    assert!(term.log[1].contains(ANNOUNCED), "{:?}", term.log);
    let err = outcome
        .settings_error
        .clone()
        .expect("the reason is reported");
    assert!(err.contains("is not absolute"), "{err}");

    let mut app = app_two();
    assert_eq!(app.resume_finished(outcome.clone()), vec![Effect::PollNow]);
    assert_eq!(app.resume_finished(outcome), vec![Effect::PollNow]);
    let status = common::status_text(&app);
    let warning = "sessions start without the subagent hooks";
    assert_eq!(status.matches(warning).count(), 1, "{status}");
    assert!(status.contains("subagents: "), "{status}");
    assert!(status.contains("back from claude --resume"), "{status}");
}

#[test]
fn a_failing_picker_waits_for_a_key() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("claude", "exit 3");
    let req = ResumeRequest {
        account: fake.account(),
        folder: std::env::temp_dir(),
        settings: None,
    };
    let mut term = FakeTerm::default();
    let outcome = resume_past(
        &mut term,
        &mut InheritSpawner,
        &req,
        "/tmp",
        &Termination::default(),
    );
    assert_eq!(outcome.result, Ok(Some(3)));
    assert!(
        term.log.iter().any(|l| l
            == "wait tower: claude --resume exited with code 3. Press any key to return to tower."),
        "{:?}",
        term.log
    );
}

#[test]
fn a_child_that_keeps_stopping_is_resumed_without_spinning() {
    // A plain interactive `claude` may suspend itself on Ctrl+Z; tower
    // resumes it, at most once per RESUME_MIN_GAP, and never hangs. SIGSTOP
    // here, since SIGTSTP may be discarded in an orphaned process group.
    assert_eq!(resume_delay(None), Duration::ZERO);
    assert_eq!(
        resume_delay(Some(Duration::from_millis(30))),
        RESUME_MIN_GAP - Duration::from_millis(30)
    );
    assert_eq!(resume_delay(Some(RESUME_MIN_GAP * 2)), Duration::ZERO);

    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new(
        "claude",
        "kill -STOP $$; kill -STOP $$; kill -STOP $$; exit 5",
    );
    let start = Instant::now();
    // InheritSpawner spawns and then waits with `wait_resuming`.
    let status = InheritSpawner
        .run(std::process::Command::new(fake.program()))
        .unwrap();
    assert_eq!(status.code(), Some(5));
    // Three stops in a row: the second and third resumes were spaced out.
    assert!(start.elapsed() >= RESUME_MIN_GAP, "{:?}", start.elapsed());
}
