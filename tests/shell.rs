//! `!`: a shell in the selected session's folder (doc/0037). The program
//! and its command line, the hand-over to a fake shell (the same steps as
//! an attach, doc/0005), a folder that is gone, the key in the list, the
//! report afterwards, and the footer and help. No real shell or Claude
//! Code runs.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{app_single, ch, key};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect};
use watchtower::attach::{Handover, InheritSpawner, Termination};
use watchtower::cli::CONFIG_DIR_VAR;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::shell::{
    self, FALLBACK_SHELL, ShellOutcome, ShellRequest, ShellResult, announcement, command, program,
};

// ---- the command -----------------------------------------------------------

#[test]
fn the_shell_is_shell_else_bin_sh() {
    assert_eq!(program(Some(OsStr::new("/usr/bin/zsh"))), "/usr/bin/zsh");
    assert_eq!(program(Some(OsStr::new(""))), FALLBACK_SHELL);
    assert_eq!(program(None), "/bin/sh");
}

#[test]
fn the_command_is_the_bare_shell_in_the_folder() {
    let dir = Path::new("/home/user/workspace/api");
    let line = command("/usr/bin/zsh", dir);
    assert_eq!(line.argv(), ["/usr/bin/zsh"]);
    assert_eq!(line.cwd.as_deref(), Some(dir));
    let cmd = line.to_command();
    assert_eq!(cmd.get_current_dir(), Some(dir));
    // No variables of its own; CLAUDE_CONFIG_DIR is removed, as from
    // every command tower runs (doc/0017).
    let envs: Vec<_> = cmd.get_envs().collect();
    assert_eq!(envs, [(OsStr::new(CONFIG_DIR_VAR), None)]);
}

#[test]
fn the_announcement() {
    assert_eq!(
        announcement("~/workspace/api"),
        "tower: shell in ~/workspace/api — type exit to return to tower"
    );
}

// ---- the hand-over ------------------------------------------------------------

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

fn program_of(fake: &fake_claude::Fake) -> String {
    fake.program().to_string_lossy().into_owned()
}

#[test]
fn the_shell_runs_in_the_folder_like_an_attach() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("fake-shell", "exit 0");
    let folder = tempfile::tempdir().unwrap();
    let req = ShellRequest {
        dir: folder.path().to_path_buf(),
        folder: "~/workspace/api".into(),
    };
    let mut term = FakeTerm::default();
    let outcome = shell::run(
        &mut term,
        &mut InheritSpawner,
        &req,
        &program_of(&fake),
        &Termination::default(),
    );
    assert_eq!(outcome.result, ShellResult::Ran(Ok(Some(0))));
    assert_eq!(outcome.folder, "~/workspace/api");
    // No arguments (the fake records none as one empty string).
    assert!(fake.argv().concat().is_empty(), "{:?}", fake.argv());
    assert_eq!(fake.calls(), 1);
    assert_eq!(
        fake.cwd().canonicalize().unwrap(),
        folder.path().canonicalize().unwrap()
    );
    assert_eq!(
        term.log,
        [
            "release",
            "announce tower: shell in ~/workspace/api — type exit to return to tower",
            "exited failed=false",
            "reclaim",
        ]
    );
}

#[test]
fn a_failing_shell_waits_for_a_key() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("fake-shell", "exit 3");
    let folder = tempfile::tempdir().unwrap();
    let req = ShellRequest {
        dir: folder.path().to_path_buf(),
        folder: "~/x".into(),
    };
    let mut term = FakeTerm::default();
    let outcome = shell::run(
        &mut term,
        &mut InheritSpawner,
        &req,
        &program_of(&fake),
        &Termination::default(),
    );
    assert_eq!(outcome.result, ShellResult::Ran(Ok(Some(3))));
    assert!(
        term.log.iter().any(
            |l| l == "wait tower: shell exited with code 3. Press any key to return to tower."
        ),
        "{:?}",
        term.log
    );
    assert_eq!(term.log.last().map(String::as_str), Some("reclaim"));
}

#[test]
fn a_folder_that_is_gone_runs_nothing_and_keeps_the_terminal() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("fake-shell", "exit 0");
    let tmp = tempfile::tempdir().unwrap();
    let req = ShellRequest {
        dir: tmp.path().join("removed"),
        folder: "~/workspace/removed".into(),
    };
    let mut term = FakeTerm::default();
    let outcome = shell::run(
        &mut term,
        &mut InheritSpawner,
        &req,
        &program_of(&fake),
        &Termination::default(),
    );
    assert_eq!(outcome.result, ShellResult::FolderGone);
    assert!(term.log.is_empty(), "{:?}", term.log);
    assert_eq!(fake.calls(), 0);
}

#[test]
fn the_announced_folder_is_sanitized() {
    let _l = fake_claude::lock();
    let fake = fake_claude::Fake::new("fake-shell", "exit 0");
    let folder = tempfile::tempdir().unwrap();
    let req = ShellRequest {
        dir: folder.path().to_path_buf(),
        folder: "~/evil\u{7}\u{1b}[31m".into(),
    };
    let mut term = FakeTerm::default();
    shell::run(
        &mut term,
        &mut InheritSpawner,
        &req,
        &program_of(&fake),
        &Termination::default(),
    );
    assert_eq!(
        term.log[1],
        "announce tower: shell in ~/evil??[31m — type exit to return to tower"
    );
}

// ---- the key ------------------------------------------------------------------

/// The app with a background row, an interactive row and a row without a
/// folder.
fn app() -> App {
    let mut app = app_single();
    let rows = r#"[
        {"id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000001",
         "cwd": "/home/user/workspace/api", "kind": "background", "name": "fix-login",
         "state": "done"},
        {"sessionId": "9c6e1a35-0000-4000-8000-000000000003", "pid": 5151,
         "cwd": "/home/user/workspace/web", "kind": "interactive", "name": "local-chat",
         "status": "idle"},
        {"id": "5e6f7a8b", "sessionId": "5e6f7a8b-0000-4000-8000-000000000002",
         "kind": "background", "name": "nowhere", "state": "done"}
    ]"#;
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 1,
        result: Ok(parse_listing(rows).unwrap()),
    });
    app
}

#[test]
fn bang_asks_for_a_shell_in_the_selected_rows_folder() {
    let mut app = app();
    common::select_name(&mut app, "fix-login");
    assert_eq!(
        app.handle_key(ch('!')),
        [Effect::Shell(ShellRequest {
            dir: PathBuf::from("/home/user/workspace/api"),
            folder: "~/workspace/api".into(),
        })]
    );
    // An interactive row has a folder too.
    common::select_name(&mut app, "local-chat");
    assert_eq!(
        app.handle_key(ch('!')),
        [Effect::Shell(ShellRequest {
            dir: PathBuf::from("/home/user/workspace/web"),
            folder: "~/workspace/web".into(),
        })]
    );
}

#[test]
fn bang_without_a_folder_or_on_a_header_does_nothing() {
    let mut app = app();
    common::select_name(&mut app, "nowhere");
    assert!(app.handle_key(ch('!')).is_empty());
    assert!(common::status_text(&app).contains("nowhere has no folder"));
    app.handle_key(key(KeyCode::Home));
    assert!(app.selected_header().is_some());
    assert!(app.handle_key(ch('!')).is_empty());
    assert!(common::status_text(&app).contains("select a session"));
}

#[test]
fn after_the_shell_tower_polls_and_reports() {
    let mut app = app();
    let effects = app.shell_finished(ShellOutcome {
        folder: "~/workspace/api".into(),
        result: ShellResult::Ran(Ok(Some(0))),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert!(common::status_text(&app).contains("back from the shell in ~/workspace/api"));

    let effects = app.shell_finished(ShellOutcome {
        folder: "~/workspace/api".into(),
        result: ShellResult::Ran(Ok(Some(2))),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert!(common::status_text(&app).contains("exited with code 2"));

    let effects = app.shell_finished(ShellOutcome {
        folder: "~/workspace/api".into(),
        result: ShellResult::Ran(Err("cannot run `/bin/nosh`: not found".into())),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert!(common::status_text(&app).contains("cannot run a shell in ~/workspace/api"));
}

#[test]
fn a_folder_that_is_gone_is_a_status_message() {
    let mut app = app();
    let effects = app.shell_finished(ShellOutcome {
        folder: "~/workspace/removed".into(),
        result: ShellResult::FolderGone,
    });
    assert!(effects.is_empty(), "{effects:?}");
    assert!(
        common::status_text(&app)
            .contains("~/workspace/removed no longer exists; no shell started"),
        "{}",
        common::status_text(&app)
    );
}

// ---- footer and help --------------------------------------------------------------

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_footer_and_the_help_show_the_key() {
    let mut app = app();
    let wide = render(&mut app, 300, 24);
    assert!(wide.contains("t reply · ! shell · c settings"), "{wide}");
    app.handle_key(ch('?'));
    let help = render(&mut app, 80, 24);
    assert!(
        help.contains("!, c, r, ?") && help.contains("shell in the folder"),
        "{help}"
    );
    // The help still fits a 24-row terminal (doc/0011).
    assert!(help.contains("Esc or ? closes this help"), "{help}");
}
