//! The welcome window end to end (doc/0048): the real binary in a
//! pseudo-terminal this test plays, in a temp home, with a fake `claude`
//! found through `PATH`, since a first start's account runs a plain `claude`.
//! Commands other than `tower` and `tower setup` run without a terminal.
//!
//! The waits are for something to happen, with a long deadline; what must not
//! happen is checked after what must happen, so a slow machine cannot fail
//! them.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fake_claude::{Fake, lock};

const WAIT: Duration = Duration::from_secs(20);
const TOWER: &str = env!("CARGO_BIN_EXE_tower");

const LISTING: &str = r#"[{"pid": 4194304, "status": "idle", "state": "done", "id": "aaaa1111",
    "cwd": "/home/user/demo", "kind": "background", "startedAt": 1767232793428,
    "sessionId": "aaaa1111-5b2d-4f60-9a1e-7c3b2d4e5f60", "name": "alpha-task"}]"#;

/// A temp home, with `~/projects` holding two clones, and the fake `claude`.
struct Home {
    home: tempfile::TempDir,
    fake: Fake,
}

impl Home {
    fn new() -> Home {
        let home = tempfile::tempdir().unwrap();
        for repo in ["one", "two"] {
            std::fs::create_dir_all(home.path().join("projects").join(repo).join(".git")).unwrap();
        }
        let listing = home.path().join("agents.json");
        std::fs::write(&listing, LISTING).unwrap();
        let body = format!(
            "case \"$1\" in\n\
             --version) echo '2.1.316 (Claude Code)' ;;\n\
             agents) cat '{}' ;;\n\
             *) ;;\n\
             esac",
            listing.display()
        );
        let fake = {
            let _guard = lock();
            Fake::new("claude", &body)
        };
        Home { home, fake }
    }

    fn config(&self) -> PathBuf {
        self.home.path().join(".config/watchtower/config.toml")
    }

    fn write_config(&self, text: &str) {
        std::fs::create_dir_all(self.config().parent().unwrap()).unwrap();
        std::fs::write(self.config(), text).unwrap();
    }

    fn path_with_fake(&self) -> String {
        format!("{}:/usr/bin:/bin", self.fake.dir.path().display())
    }

    /// The environment every run gets, `PATH` from the caller.
    fn command(&self, args: &[&str], path: &str) -> Command {
        let mut cmd = Command::new(TOWER);
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env("PATH", path)
            .env("TERM", "xterm-256color");
        cmd
    }

    /// A command without a terminal.
    fn run(&self, args: &[&str]) -> Output {
        self.command(args, &self.path_with_fake())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

/// Tower in a pseudo-terminal.
struct Tower {
    child: Child,
    master: File,
    screen: Arc<Mutex<vt100::Parser>>,
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

impl Tower {
    fn start(home: &Home, args: &[&str], size: (u16, u16)) -> Tower {
        Tower::start_with_path(home, args, size, &home.path_with_fake())
    }

    fn start_with_path(home: &Home, args: &[&str], size: (u16, u16), path: &str) -> Tower {
        let (cols, rows) = size;
        let (mut master_fd, mut slave_fd) = (0, 0);
        // SAFETY: openpty fills two descriptors; the null pointers ask for no
        // name, default settings and no size.
        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(rc, 0, "openpty");
        // SAFETY: both were just opened and are owned here.
        let (master, slave) = unsafe {
            (
                OwnedFd::from_raw_fd(master_fd),
                OwnedFd::from_raw_fd(slave_fd),
            )
        };
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: a valid winsize on an open pty descriptor.
        assert_eq!(
            unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &winsize) },
            0
        );
        let mut cmd = home.command(args, path);
        cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        let master = File::from(master);
        let screen = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let mut reader = master.try_clone().unwrap();
        let mut answer = master.try_clone().unwrap();
        let parser = Arc::clone(&screen);
        thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n];
                if contains(chunk, b"\x1b[c") {
                    let _ = answer.write_all(b"\x1b[?62;22c");
                }
                if contains(chunk, b"\x1b[6n") {
                    let _ = answer.write_all(b"\x1b[1;1R");
                }
                parser.lock().unwrap().process(chunk);
            }
        });
        Tower {
            child,
            master,
            screen,
        }
    }

    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + WAIT;
        while !self.text().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "never showed {needle:?}:\n{}",
                self.text()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }

    /// Sends `Enter`, then waits for the step it leads to.
    fn next(&mut self, step: &str) {
        self.send(b"\r");
        self.wait_for(step);
    }

    fn wait_exit(&mut self) -> i32 {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            assert!(
                Instant::now() < deadline,
                "tower did not exit:\n{}",
                self.text()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Tower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const STEP1: &str = "step 1 of 5";
const STEP2: &str = "step 2 of 5";
const STEP3: &str = "step 3 of 5";
const STEP4: &str = "step 4 of 5";
const STEP5: &str = "step 5 of 5";
const ESC: &[u8] = b"\x1b";
const CTRL_C: &[u8] = b"\x03";
const DOWN: &[u8] = b"\x1b[B";
const RIGHT: &[u8] = b"\x1b[C";

fn toml_of(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_first_start_walks_the_steps_saves_and_carries_on_into_the_list() {
    let home = Home::new();
    assert!(!home.config().exists());
    let mut tower = Tower::start(&home, &[], (120, 40));
    tower.wait_for(STEP1);
    tower.wait_for("2.1.316");
    tower.wait_for("1 session");
    assert!(
        tower.text().contains("main runs claude"),
        "{}",
        tower.text()
    );
    tower.next(STEP2);
    tower.wait_for("‹ workspace ›");
    tower.next(STEP3);
    tower.wait_for("✓ 2 git clones");
    tower.next(STEP4);
    tower.next(STEP5);
    tower.wait_for("~/projects");
    assert!(!home.config().exists(), "nothing before the last Enter");
    tower.send(b"\r");
    // The list, with the session, in the workspace style.
    tower.wait_for("alpha-task");
    let text = toml_of(&home.config());
    assert!(text.contains("style = \"workspace\""), "{text}");
    assert!(text.contains("project_roots = [\"~/projects\"]"), "{text}");
    assert!(
        text.contains("# Colour theme of the dashboard style"),
        "{text}"
    );
    assert!(text.contains("\n[[accounts]]\nname = \"main\"\n"), "{text}");
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);

    // It never comes back.
    let before = toml_of(&home.config());
    let tower = Tower::start(&home, &[], (120, 40));
    tower.wait_for("alpha-task");
    assert!(!tower.text().contains(STEP1), "{}", tower.text());
    assert_eq!(toml_of(&home.config()), before);
}

#[test]
fn an_existing_config_does_not_open_it() {
    let home = Home::new();
    let text = "[ui]\nstyle = \"dashboard\"\n";
    home.write_config(text);
    let mut tower = Tower::start(&home, &[], (120, 40));
    tower.wait_for("alpha-task");
    assert!(!tower.text().contains(STEP1), "{}", tower.text());
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
    assert_eq!(toml_of(&home.config()), text);
}

#[test]
fn esc_saves_the_current_choices_and_goes_on() {
    let home = Home::new();
    let mut tower = Tower::start(&home, &[], (80, 24));
    tower.wait_for(STEP1);
    tower.next(STEP2);
    // The theme: previewed, and written with the rest.
    tower.send(DOWN);
    tower.send(RIGHT);
    tower.wait_for("‹ warm ›");
    tower.next(STEP3);
    tower.wait_for("✓ 2 git clones");
    tower.send(ESC);
    // Below 100 columns the workspace's fallback, the dashboard, shows it.
    tower.wait_for("alpha-task");
    let text = toml_of(&home.config());
    assert!(text.contains("style = \"workspace\""), "{text}");
    assert!(text.contains("theme = \"warm\""), "{text}");
    assert!(text.contains("project_roots = [\"~/projects\"]"), "{text}");
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
}

#[test]
fn ctrl_c_writes_nothing_and_the_window_shows_again() {
    let home = Home::new();
    let mut tower = Tower::start(&home, &[], (80, 24));
    tower.wait_for(STEP1);
    tower.next(STEP2);
    tower.send(CTRL_C);
    assert_eq!(tower.wait_exit(), 0);
    assert!(!home.config().exists());
    // Nor anything else of the config's: the hooks file waits for a choice.
    let dir = home.home.path().join(".config/watchtower");
    assert!(!dir.join("claude-settings.json").exists());
    assert!(!dir.join("config.toml.lock").exists());

    let tower = Tower::start(&home, &[], (80, 24));
    tower.wait_for(STEP1);
}

#[test]
fn claude_that_is_missing_is_said_and_the_window_goes_on() {
    let home = Home::new();
    // A folder with no `claude`, whatever this machine has installed.
    let empty = tempfile::tempdir().unwrap();
    let path = empty.path().display().to_string();
    let mut tower = Tower::start_with_path(&home, &[], (80, 24), &path);
    tower.wait_for("could not be run");
    tower.wait_for("install it later");
    tower.next(STEP2);
    tower.send(ESC);
    let deadline = Instant::now() + WAIT;
    while !home.config().exists() {
        assert!(Instant::now() < deadline, "no file:\n{}", tower.text());
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_small_terminal_goes_on_with_the_defaults_and_writes_nothing() {
    let home = Home::new();
    let mut tower = Tower::start(&home, &[], (70, 20));
    tower.wait_for("alpha-task");
    assert!(!tower.text().contains(STEP1), "{}", tower.text());
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
    assert!(!home.config().exists());
}

#[test]
fn a_write_that_fails_says_so_and_goes_on() {
    let home = Home::new();
    // The lock file as a folder: the save cannot take it.
    let dir = home.config().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(dir.join("config.toml.lock")).unwrap();
    let mut tower = Tower::start(&home, &[], (80, 24));
    tower.wait_for(STEP1);
    tower.send(ESC);
    tower.wait_for("could not be written");
    tower.send(b"\r");
    tower.wait_for("alpha-task");
    assert!(!home.config().exists());
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
}

const OWN_FILE: &str = "\
# My own comment.
unknown_key = \"kept\"

[ui]
theme = \"warm\" # my theme
";

#[test]
fn tower_setup_starts_from_the_file_and_changes_only_what_changed() {
    let home = Home::new();
    home.write_config(OWN_FILE);
    let mut tower = Tower::start(&home, &["setup"], (100, 30));
    tower.wait_for(STEP1);
    tower.next(STEP2);
    tower.wait_for("‹ dashboard ›");
    tower.wait_for("‹ warm ›");
    tower.send(RIGHT);
    tower.wait_for("‹ minimal ›");
    tower.next(STEP3);
    tower.next(STEP4);
    tower.next(STEP5);
    tower.wait_for("this 1 change");
    tower.send(b"\r");
    tower.wait_for("alpha-task");
    assert_eq!(
        toml_of(&home.config()),
        "# My own comment.\nunknown_key = \"kept\"\n\n[ui]\ntheme = \"warm\" # my theme\n\
         style = \"minimal\"\n"
    );
    assert_eq!(
        toml_of(&home.config().with_file_name("config.toml.bak")),
        OWN_FILE
    );
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
}

#[test]
fn tower_setup_esc_writes_nothing_and_goes_on() {
    let home = Home::new();
    home.write_config(OWN_FILE);
    let mut tower = Tower::start(&home, &["setup"], (100, 30));
    tower.wait_for(STEP1);
    tower.next(STEP2);
    tower.send(RIGHT);
    tower.wait_for("‹ minimal ›");
    tower.send(ESC);
    tower.wait_for("alpha-task");
    assert_eq!(toml_of(&home.config()), OWN_FILE);
    assert!(!home.config().with_file_name("config.toml.bak").exists());
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), 0);
}

/// `status`, `dispatch`, `wake`, `hook`, `--help` and `--version` have no
/// terminal, never open it and never write the config.
#[test]
fn the_commands_without_a_window_do_not_write_a_config() {
    let home = Home::new();
    for args in [
        &["status"][..],
        &["status", "--json"],
        &["--version"],
        &["--help"],
        &["hook"],
        &["dispatch", "--project", "/nonexistent", "--", "task"],
        &["wake", "aaaa1111"],
    ] {
        let out = home.run(args);
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(STEP1), "{args:?}: {all}");
        assert!(!all.contains('\x1b'), "{args:?} drew: {all}");
        assert!(!home.config().exists(), "{args:?} wrote the config");
    }
    assert_eq!(home.run(&["--version"]).status.code(), Some(0));
    assert_eq!(home.run(&["status"]).status.code(), Some(0));
}

/// `update` too, with a `PATH` that has no `curl`, so it uses no network.
#[test]
fn update_does_not_write_a_config() {
    let home = Home::new();
    let empty = tempfile::tempdir().unwrap();
    let out = home
        .command(&["update", "--check"], &empty.path().display().to_string())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!home.config().exists());
}

#[test]
fn tower_setup_needs_a_terminal_and_says_so_in_the_help() {
    let home = Home::new();
    let out = home.run(&["setup"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("needs an interactive terminal"), "{err}");
    assert!(!home.config().exists());

    let help = String::from_utf8(home.run(&["--help"]).stdout).unwrap();
    assert!(help.contains("tower setup"), "{help}");
    assert_eq!(home.run(&["setup", "--bogus"]).status.code(), Some(2));
    assert_eq!(home.run(&["setup", "--help"]).status.code(), Some(0));
}
