//! A key pressed after a terminal resize is handled and drawn at once
//! (doc/0005): the real binary in a pseudo-terminal, against a fake
//! `claude` that lists one session.
//!
//! crossterm's reader drops the readiness of terminal input that one wait
//! reports together with a `SIGWINCH`. Tower waits on the input alone, so a
//! resize while it is idle (terminals resize when a full-screen program
//! starts, or after a hand-over) used to be reported together with the next
//! key, and that key was handled only when the key after it arrived.
//!
//! The test plays the terminal: it answers tower's device-attributes and
//! cursor-position queries, resizes the window (which sends `SIGWINCH` to
//! tower) and types. A background thread reads everything tower writes.

#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fake_claude::{Fake, lock};

/// How long tower gets to draw what a key changed. Far above one loop turn
/// (200 ms); before the fix the dialog never came without another key.
const DRAW_WAIT: Duration = Duration::from_secs(3);

/// The title of the new-session dialog `n` opens (doc/0008).
const DIALOG: &str = "New session";

/// Tower running in a pseudo-terminal whose other end is this test.
struct Pty {
    child: Child,
    master: File,
    out: Arc<Mutex<Vec<u8>>>,
    _home: tempfile::TempDir,
    _fake: Fake,
}

fn set_size(fd: i32, rows: u16, cols: u16) {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: a valid winsize on an open pty descriptor.
    assert_eq!(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) }, 0);
}

impl Pty {
    /// Starts tower with one account whose fake lists one session in a
    /// temporary folder. `$SHELL`, used by `!`, exits at once.
    fn start() -> Pty {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("demo");
        std::fs::create_dir_all(&project).unwrap();
        let listing = home.path().join("agents.json");
        std::fs::write(
            &listing,
            format!(
                "[{{\"pid\": 4194304, \"id\": \"3e9a1c07\", \"cwd\": \"{}\", \
                 \"kind\": \"background\", \"startedAt\": 1767232793428, \
                 \"sessionId\": \"3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60\", \
                 \"name\": \"demo-task\", \"status\": \"idle\", \"state\": \"blocked\"}}]",
                project.display()
            ),
        )
        .unwrap();
        let fake = {
            let _guard = lock();
            Fake::new("claude", &format!("cat '{}'", listing.display()))
        };
        let config_dir = home.path().join(".config/watchtower");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "default_account = \"main\"\n\
                 project_roots = [\"{}\"]\n\
                 [[accounts]]\nname = \"main\"\ncommand = \"'{}'\"\n",
                home.path().display(),
                fake.program().display()
            ),
        )
        .unwrap();
        let shell = home.path().join("shell");
        std::fs::write(&shell, "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let (mut master_fd, mut slave_fd) = (0, 0);
        // SAFETY: openpty fills two descriptors; the null pointers ask for
        // no name, default settings and no size.
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
        // SAFETY: both descriptors were just opened and are owned here.
        let (master, slave) = unsafe {
            (
                OwnedFd::from_raw_fd(master_fd),
                OwnedFd::from_raw_fd(slave_fd),
            )
        };
        set_size(master.as_raw_fd(), 40, 120);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tower"));
        cmd.env_clear()
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .env("SHELL", &shell)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec. The
        // pty becomes the controlling terminal of a new session, so tower
        // is its foreground process group and gets its SIGWINCH.
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
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut reader = master.try_clone().unwrap();
        let mut answer = master.try_clone().unwrap();
        let sink = Arc::clone(&out);
        thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n];
                // A terminal without the keyboard-enhancement protocol:
                // only the device attributes are answered.
                if contains(chunk, b"\x1b[c") {
                    let _ = answer.write_all(b"\x1b[?62;22c");
                }
                if contains(chunk, b"\x1b[6n") {
                    let _ = answer.write_all(b"\x1b[1;1R");
                }
                sink.lock().unwrap().extend_from_slice(chunk);
            }
        });
        Pty {
            child,
            master,
            out,
            _home: home,
            _fake: fake,
        }
    }

    /// How many bytes tower has written so far.
    fn mark(&self) -> usize {
        self.out.lock().unwrap().len()
    }

    /// Waits up to `timeout` for `needle` in what tower wrote after `from`.
    fn wait_for(&self, from: usize, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if contains(&self.out.lock().unwrap()[from..], needle.as_bytes()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// How many times `needle` occurs in what tower wrote.
    fn count(&self, needle: &str) -> usize {
        let out = self.out.lock().unwrap();
        out.windows(needle.len())
            .filter(|w| *w == needle.as_bytes())
            .count()
    }

    fn type_keys(&mut self, keys: &str) {
        self.master.write_all(keys.as_bytes()).unwrap();
    }

    /// Resizes the window, which sends SIGWINCH to tower, and lets tower
    /// sit idle afterwards, as when nobody types.
    fn resize_and_idle(&self, rows: u16, cols: u16) {
        set_size(self.master.as_raw_fd(), rows, cols);
        thread::sleep(Duration::from_millis(600));
    }

    /// Waits for the list with the fake's session, then lets tower idle.
    fn wait_listed(&self) {
        assert!(
            self.wait_for(0, "demo-task", Duration::from_secs(10)),
            "tower never listed the session"
        );
        thread::sleep(Duration::from_millis(800));
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: plain kill of the child this test started.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A process's CPU time in clock ticks (user + system), from `/proc`.
fn cpu_ticks(pid: u32) -> u64 {
    let stat =
        std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).unwrap();
    let fields: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

#[test]
fn an_idle_key_is_drawn_at_once() {
    let mut pty = Pty::start();
    pty.wait_listed();
    let from = pty.mark();
    pty.type_keys("n");
    assert!(pty.wait_for(from, DIALOG, DRAW_WAIT), "n opened no dialog");
}

#[test]
fn the_first_key_after_a_resize_at_start_is_drawn_at_once() {
    let mut pty = Pty::start();
    pty.wait_listed();
    pty.resize_and_idle(36, 110);
    let from = pty.mark();
    pty.type_keys("n");
    assert!(
        pty.wait_for(from, DIALOG, DRAW_WAIT),
        "n after a resize was not drawn until another key"
    );
    // And tower does not spin while it waits for the next key.
    let before = cpu_ticks(pty.child.id());
    thread::sleep(Duration::from_secs(1));
    let used = cpu_ticks(pty.child.id()) - before;
    assert!(
        used < 30,
        "tower used {used} ticks of CPU in 1 s while idle"
    );
}

#[test]
fn the_first_key_after_a_hand_over_and_a_resize_is_drawn_at_once() {
    let mut pty = Pty::start();
    pty.wait_listed();
    let entered = pty.count("\x1b[?1049h");
    // `!` hands the terminal to `$SHELL`, which exits at once; tower then
    // takes it back and enters the alternate screen again.
    pty.type_keys("!");
    let deadline = Instant::now() + Duration::from_secs(10);
    while pty.count("\x1b[?1049h") <= entered {
        assert!(
            Instant::now() < deadline,
            "tower never took the terminal back"
        );
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(800));
    pty.resize_and_idle(36, 110);
    let from = pty.mark();
    pty.type_keys("n");
    assert!(
        pty.wait_for(from, DIALOG, DRAW_WAIT),
        "n after the hand-over and a resize was not drawn until another key"
    );
}
