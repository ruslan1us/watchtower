//! The milestone 2 runtime against fake `claude` executables (doc/0015):
//! the app's effects carried out by [`Jobs`] on worker threads — dispatch
//! argv and working directory, stop and remove with the owning account,
//! logs fetched with the owning account — plus the task editor and the
//! trust flow's foreground hand-over.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::io;
use std::path::Path;
use std::time::Duration;

use common::{HOME, NoEnviron, ch, ctrl, key, type_text};
use fake_claude::{Fake, cat, cat_err, lock};
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect, LogsView, Overlay};
use watchtower::attach::{self, Foreground, Handover, InheritSpawner, Termination};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::dialog::DialogStatus;
use watchtower::dispatch;
use watchtower::editor;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};

/// An app whose accounts `main` and `alt` run the given fakes.
fn app_with(main: &Fake, alt: &Fake) -> App {
    let home = Path::new(HOME);
    let config = Config::parse(
        &format!(
            r#"
default_account = "main"

[[accounts]]
name = "main"
command = "'{}'"

[[accounts]]
name = "alt"
command = "'{}'"
config_dir = "~/.claude-alt"
"#,
            main.program().display(),
            alt.program().display()
        ),
        home,
    )
    .unwrap();
    App::new(config, home.into(), Sidecar::default(), Box::new(NoEnviron))
}

/// `alt` lists the fixture, `main` nothing, and every row has a `started`
/// record for `alt`, so `alt` owns it (rule 3; an empty listing is no
/// rule-2 evidence, doc/0017).
fn owned_by_alt(app: &mut App, fixture: &str) {
    for s in common::listing(fixture).sessions {
        if let Some(key) = s.key() {
            app.sidecar.record_started(key, "alt");
        }
    }
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing("[]").unwrap()),
    });
    app.apply_poll(common::poll("alt", fixture));
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

/// Carries out the background effects with `jobs` and feeds every result
/// back, until no job is outstanding. Returns the other effects.
fn drive(app: &mut App, jobs: &Jobs, effects: Vec<Effect>) -> Vec<Effect> {
    let mut rest = Vec::new();
    let mut outstanding = 0;
    let mut queue = effects;
    loop {
        for effect in queue.drain(..) {
            match effect {
                Effect::Dispatch(job) => {
                    jobs.dispatch(job);
                    outstanding += 1;
                }
                Effect::Stop(a) => {
                    jobs.stop(a);
                    outstanding += 1;
                }
                Effect::Remove(a) => {
                    jobs.remove(a);
                    outstanding += 1;
                }
                Effect::FetchLogs(t) => {
                    jobs.logs(t);
                    outstanding += 1;
                }
                other => rest.push(other),
            }
        }
        if outstanding == 0 {
            return rest;
        }
        let done: JobResult = jobs
            .recv_timeout(Duration::from_secs(20))
            .expect("job finished");
        outstanding -= 1;
        queue = done.apply(app);
    }
}

fn jobs() -> Jobs {
    Jobs::new(CliRunner::new().with_timeout(Duration::from_secs(20)))
}

// ---- dispatch ----------------------------------------------------------------

#[test]
fn dispatch_runs_the_chosen_account_in_the_project() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new(
        "claude-alt",
        &format!(
            "{}\n{}",
            cat_err("cli-output/bg-success.stderr"),
            cat("cli-output/bg-success.stdout")
        ),
    );
    let mut app = app_with(&main, &alt);
    let root = tempfile::tempdir().unwrap();
    app.open_dialog(common::project_list(root.path(), &["demo-app"]));
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "say OK\n");
    app.handle_key(key(KeyCode::Tab)); // name
    type_text(&mut app, "bad-model");
    app.handle_key(key(KeyCode::Tab)); // account
    app.handle_key(key(KeyCode::Right));
    let effects = app.handle_key(ctrl('s'));
    let rest = drive(&mut app, &jobs(), effects);

    assert_eq!(rest, [Effect::SaveSidecar, Effect::PollNow]);
    assert_eq!(
        alt.argv(),
        [
            "--bg",
            "--name",
            "bad-model",
            "--permission-mode",
            "auto",
            "say OK"
        ]
    );
    assert_eq!(
        alt.cwd(),
        root.path().join("demo-app").canonicalize().unwrap()
    );
    assert_eq!(main.calls(), 0, "only the chosen account's command");
    assert!(app.dialog().is_none());
    assert_eq!(
        app.sidecar
            .owner("3e9a1c07")
            .map(|(a, s)| (a.to_owned(), s.clone())),
        Some(("alt".into(), AccountSource::Started))
    );
    assert!(common::status_text(&app).contains("started bad-model (3e9a1c07) under alt"));
}

#[test]
fn untrusted_dispatch_keeps_the_dialog() {
    let _g = lock();
    let main = Fake::new(
        "claude",
        &format!("{}\nexit 1", cat_err("cli-output/bg-untrusted.stderr")),
    );
    let alt = Fake::new("claude-alt", "exit 99");
    let mut app = app_with(&main, &alt);
    let root = tempfile::tempdir().unwrap();
    app.open_dialog(common::project_list(root.path(), &["untrusted-repo"]));
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "fix the bug");
    let effects = app.handle_key(ctrl('s'));
    let rest = drive(&mut app, &jobs(), effects);
    assert!(rest.is_empty(), "{rest:?}");
    let d = app.dialog().expect("kept");
    assert!(matches!(d.status, DialogStatus::Untrusted { .. }));
    assert!(
        d.error
            .as_deref()
            .unwrap()
            .starts_with("Workspace not trusted")
    );
    assert_eq!(d.task.text(), "fix the bug");
    assert_eq!(main.calls(), 1);
}

// ---- stop and remove ---------------------------------------------------------

#[test]
fn stop_runs_the_owning_account() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new("claude-alt", &cat("cli-output/stop.stdout"));
    let mut app = app_with(&main, &alt);
    owned_by_alt(&mut app, "mixed-states.json");
    select(&mut app, "say-ok");
    let effects = app.handle_key(ch('s'));
    let rest = drive(&mut app, &jobs(), effects);
    assert_eq!(rest, [Effect::PollNow]);
    assert_eq!(alt.argv(), ["stop", "b2f8d064"]);
    assert_eq!(main.calls(), 0);
    assert!(common::status_text(&app).contains("stopped say-ok (b2f8d064)"));
}

#[test]
fn remove_runs_the_owning_account_after_confirmation() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new("claude-alt", &cat("cli-output/rm.stdout"));
    let mut app = app_with(&main, &alt);
    owned_by_alt(&mut app, "mixed-states.json");
    select(&mut app, "say-ok");
    assert!(app.handle_key(ch('d')).is_empty());
    assert_eq!(alt.calls(), 0, "nothing runs before y");
    let effects = app.handle_key(ch('y'));
    let rest = drive(&mut app, &jobs(), effects);
    assert_eq!(rest, [Effect::PollNow]);
    assert_eq!(alt.argv(), ["rm", "b2f8d064"]);
    assert!(common::status_text(&app).contains("removed say-ok"));
}

#[test]
fn remove_refusal_opens_the_window_and_runs_once() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new(
        "claude-alt",
        "echo 'Worktree has 1 unpushed commit; not removing.' >&2\n\
         echo 'To discard it: claude rm b2f8d064 --discard-unpushed 1234abc@/w/x' >&2\n\
         exit 1",
    );
    let mut app = app_with(&main, &alt);
    owned_by_alt(&mut app, "mixed-states.json");
    select(&mut app, "say-ok");
    app.handle_key(ch('d'));
    let effects = app.handle_key(ch('y'));
    let rest = drive(&mut app, &jobs(), effects);
    assert_eq!(rest, [Effect::PollNow]);
    assert_eq!(alt.calls(), 1, "never retried");
    assert_eq!(alt.argv(), ["rm", "b2f8d064"], "no flags");
    let text = app.popup().expect("refusal window").lines.join("\n");
    assert!(text.contains("1 unpushed commit"), "{text}");
    assert!(text.contains("--discard-unpushed 1234abc@/w/x"), "{text}");
    assert_eq!(app.overlay(), &Overlay::None);
}

// ---- logs ------------------------------------------------------------------------

#[test]
fn logs_are_fetched_with_the_owning_account() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new(
        "claude-alt",
        &format!(
            "echo 'Session 7b4d9f02 · print-date' >&2\n{}",
            cat("logs/done.ansi")
        ),
    );
    let mut app = app_with(&main, &alt);
    owned_by_alt(&mut app, "mixed-states.json");
    select(&mut app, "print-date");
    let effects = app.handle_key(ch(' '));
    let rest = drive(&mut app, &jobs(), effects);
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(alt.argv(), ["logs", "7b4d9f02"]);
    assert_eq!(main.calls(), 0);
    let LogsView::Screen(screen) = app.logs().unwrap().view.clone() else {
        panic!("{:?}", app.logs().unwrap().view)
    };
    let text = screen.plain_lines().join("\n");
    assert!(text.contains("reply with the single word OK"), "{text}");
    assert!(!text.contains("Session 7b4d9f02"), "stderr banner ignored");

    // The owner's next poll refreshes it: one more `logs` call.
    let effects = app.apply_poll(common::poll("alt", "mixed-states.json"));
    drive(&mut app, &jobs(), effects);
    assert_eq!(alt.calls(), 2);
}

#[test]
fn stopped_session_logs_become_the_note() {
    let _g = lock();
    let main = Fake::new("claude", "exit 99");
    let alt = Fake::new(
        "claude-alt",
        &format!("{}\nexit 1", cat_err("cli-output/logs-stopped.stderr")),
    );
    let mut app = app_with(&main, &alt);
    owned_by_alt(&mut app, "mixed-states.json");
    select(&mut app, "say-ok");
    let effects = app.handle_key(ch(' '));
    drive(&mut app, &jobs(), effects);
    assert_eq!(
        app.logs().unwrap().view,
        LogsView::Note(watchtower::app::NOTE_STOPPED.into())
    );
}

// ---- editor and trust flow ---------------------------------------------------

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
fn editor_command_from_visual_then_editor() {
    let os = |s: &str| Some(std::ffi::OsString::from(s));
    assert_eq!(
        editor::editor_command(os("code --wait"), os("vi")).unwrap(),
        ["code", "--wait"]
    );
    assert_eq!(editor::editor_command(os("  "), os("vi")).unwrap(), ["vi"]);
    assert_eq!(
        editor::editor_command(None, os("nano -w")).unwrap(),
        ["nano", "-w"]
    );
    assert!(
        editor::editor_command(None, None)
            .unwrap_err()
            .contains("$VISUAL")
    );
    assert!(editor::editor_command(os("'unclosed"), None).is_err());
    assert_eq!(editor::read_back("text\n"), "text");
    assert_eq!(editor::read_back("text\r\n"), "text");
    assert_eq!(editor::read_back("a\n\n"), "a\n");
}

#[test]
fn editor_round_trip_through_a_private_temp_file() {
    let _g = lock();
    let temp = tempfile::tempdir().unwrap();
    // The fake editor checks the mode, appends a line and records the path.
    let ed = Fake::new(
        "fake-editor",
        "f=\"$2\"\n\
         stat -c %a \"$f\" > \"$(dirname \"$0\")/mode.txt\"\n\
         echo \"$f\" > \"$(dirname \"$0\")/path.txt\"\n\
         printf '\\nedited in the editor\\n' >> \"$f\"",
    );
    let cmd = vec![
        ed.program().to_string_lossy().into_owned(),
        "--flag".to_owned(),
    ];
    let mut term = FakeTerm::default();
    let text = editor::edit(
        &mut term,
        &mut InheritSpawner,
        &cmd,
        "draft task",
        temp.path(),
        &Termination::default(),
    )
    .unwrap();
    assert_eq!(text, "draft task\nedited in the editor");
    assert_eq!(ed.argv()[0], "--flag");
    let mode = std::fs::read_to_string(ed.dir.path().join("mode.txt")).unwrap();
    assert_eq!(mode.trim(), "600");
    let path = std::fs::read_to_string(ed.dir.path().join("path.txt")).unwrap();
    assert!(!Path::new(path.trim()).exists(), "temp file removed");
    assert_eq!(term.log.first().map(String::as_str), Some("release"));
    assert_eq!(term.log.last().map(String::as_str), Some("reclaim"));
}

#[test]
fn failing_editor_leaves_the_task_unchanged() {
    let _g = lock();
    let temp = tempfile::tempdir().unwrap();
    let ed = Fake::new("fake-editor", "echo junk >> \"$1\"; exit 3");
    let cmd = vec![ed.program().to_string_lossy().into_owned()];
    let mut term = FakeTerm::default();
    let err = editor::edit(
        &mut term,
        &mut InheritSpawner,
        &cmd,
        "draft",
        temp.path(),
        &Termination::default(),
    )
    .unwrap_err();
    assert!(err.contains("exited with code 3"), "{err}");
    assert!(
        term.log
            .iter()
            .any(|l| l.starts_with("wait tower: the editor exited"))
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn trust_flow_runs_the_bare_account_command_in_the_folder() {
    let _g = lock();
    let fake = Fake::new("claude", "exit 1");
    let dir = tempfile::tempdir().unwrap();
    let line = dispatch::trust_command(&fake.account(), dir.path());
    let fg = Foreground {
        line: &line,
        announce: Some("tower: running claude".into()),
        label: "claude",
        pause_on_failure: false,
    };
    let mut term = FakeTerm::default();
    let result =
        attach::run_foreground(&mut term, &mut InheritSpawner, &fg, &Termination::default());
    assert_eq!(result, Ok(Some(1)));
    // No arguments (the fake's `printf '%s\0'` prints one empty field then).
    assert!(line.args.is_empty(), "{:?}", line.args);
    assert_eq!(fake.argv(), [""]);
    assert_eq!(fake.cwd(), dir.path().canonicalize().unwrap());
    // Ctrl+C twice may exit non-zero; no key pause for the trust flow.
    assert_eq!(
        term.log,
        ["release", "announce tower: running claude", "reclaim"]
    );
}
