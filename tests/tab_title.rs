//! The terminal's title (doc/0047), and a restart in the workspace's pane, end
//! to end: the real binary in a pseudo-terminal this test plays, against a fake
//! `claude`. Nothing here starts a real `claude`.
//!
//! The waits are for something to happen, with a long deadline. What must not
//! happen is checked after enough polls have gone by that it would have, so a
//! slow machine cannot fail them.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;
mod settings_common;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fake_claude::{Fake, lock};
use ratatui::crossterm::event::KeyCode;
use watchtower::app::Effect;
use watchtower::config_edit::{SettingKey, SettingValue};
use watchtower::tabtitle::{self, POP, PUSH};

const WAIT: Duration = Duration::from_secs(30);

const ALPHA: &str = "aaaa1111";
const BETA: &str = "bbbb2222";

const DOWN: &[u8] = b"\x1b[B";

fn set(text: &str) -> Vec<u8> {
    tabtitle::set_sequence(text).into_bytes()
}

fn row(id: &str, name: &str, cwd: &Path, needs_me: bool) -> String {
    let process = if needs_me {
        "\"pid\": 4194304, \"status\": \"waiting\", \"waitingFor\": \"input needed\", \
         \"state\": \"blocked\", "
    } else {
        "\"pid\": 4194304, \"status\": \"idle\", \"state\": \"done\", "
    };
    format!(
        "{{{process}\"id\": \"{id}\", \"cwd\": \"{}\", \"kind\": \"background\", \
         \"startedAt\": 1767232793428, \"sessionId\": \"{id}-5b2d-4f60-9a1e-7c3b2d4e5f60\", \
         \"name\": \"{name}\"}}",
        cwd.display()
    )
}

/// What a fake account does: lists `listing`, records `attach` and `respawn`
/// in one log, in order, and `attach` stays until a key arrives.
fn fake_body(name: &str, listing: &Path, log: &Path, on_attach: &Path) -> String {
    format!(
        "case \"$1\" in\n\
         agents) cat '{listing}' ;;\n\
         attach)\n\
           echo \"{name} attach $2\" >> '{log}'\n\
           echo \"$$\" >> \"$FAKE_DIR/attach.pids\"\n\
           stty raw -echo\n\
           printf '\\033[2J\\033[HATTACHED %s\\r\\n' \"$2\"\n\
           cat '{on_attach}'\n\
           dd bs=1 count=1 >/dev/null 2>&1\n\
           ;;\n\
         respawn)\n\
           echo \"{name} respawn $2\" >> '{log}'\n\
           if [ -f '{listing}.during' ]; then\n\
             cp '{listing}' '{listing}.live'\n\
             cp '{listing}.during' '{listing}.new'; mv '{listing}.new' '{listing}'\n\
             sleep 2.5\n\
             cp '{listing}.live' '{listing}.new'; mv '{listing}.new' '{listing}'\n\
           fi\n\
           echo \"respawned $2\"\n\
           ;;\n\
         *) ;;\n\
         esac",
        listing = listing.display(),
        log = log.display(),
        on_attach = on_attach.display(),
    )
}

struct Options {
    style: &'static str,
    /// The `[ui] tab_title` line, when there is one.
    tab_title: Option<bool>,
    /// The only row needs attention from the start.
    needs_me: bool,
    /// What the fake's `attach` writes to its terminal.
    on_attach: Vec<u8>,
    /// `alt` runs [`BETA`], with a command of its own.
    two_accounts: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            style: "dashboard",
            tab_title: None,
            needs_me: false,
            on_attach: Vec::new(),
            two_accounts: false,
        }
    }
}

/// Tower in a pseudo-terminal, and what this test sees of it.
struct Tower {
    child: Child,
    master: File,
    raw: Arc<Mutex<Vec<u8>>>,
    screen: Arc<Mutex<vt100::Parser>>,
    home: tempfile::TempDir,
    fake: Fake,
    _alt: Option<Fake>,
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

fn position(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Tower {
    fn start(options: Options) -> Tower {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("demo");
        std::fs::create_dir_all(&project).unwrap();
        let listing = home.path().join("agents.json");
        let alt_listing = home.path().join("agents-alt.json");
        let log = home.path().join("log");
        let on_attach = home.path().join("on_attach");
        std::fs::write(&on_attach, &options.on_attach).unwrap();
        let alpha = row(ALPHA, "alpha-task", &project, options.needs_me);
        let beta = row(BETA, "beta-task", &project, false);
        if options.two_accounts {
            std::fs::write(&listing, format!("[{alpha}]")).unwrap();
            std::fs::write(&alt_listing, format!("[{beta}]")).unwrap();
        } else {
            std::fs::write(&listing, format!("[{alpha},{beta}]")).unwrap();
        }
        let (fake, alt) = {
            let _guard = lock();
            let fake = Fake::new("claude", &fake_body("main", &listing, &log, &on_attach));
            let alt = options.two_accounts.then(|| {
                Fake::new(
                    "claude-alt",
                    &fake_body("alt", &alt_listing, &log, &on_attach),
                )
            });
            (fake, alt)
        };
        let mut config = format!(
            "default_account = \"main\"\npoll_interval_secs = 1\n[ui]\nstyle = \"{}\"\n",
            options.style
        );
        if let Some(on) = options.tab_title {
            config.push_str(&format!("tab_title = {on}\n"));
        }
        config.push_str(&format!(
            "[[accounts]]\nname = \"main\"\ncommand = \"'{}'\"\n",
            fake.program().display()
        ));
        if let Some(alt) = &alt {
            config.push_str(&format!(
                "[[accounts]]\nname = \"alt\"\ncommand = \"'{}'\"\nconfig_dir = \"~/.claude-alt\"\n",
                alt.program().display()
            ));
        }
        let config_dir = home.path().join(".config/watchtower");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), config).unwrap();

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
        let size = libc::winsize {
            ws_row: 40,
            ws_col: 160,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: a valid winsize on an open pty descriptor.
        assert_eq!(
            unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tower"));
        cmd.env_clear()
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
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
        let raw = Arc::new(Mutex::new(Vec::new()));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(40, 160, 0)));
        let mut reader = master.try_clone().unwrap();
        let mut answer = master.try_clone().unwrap();
        let (sink, parser) = (Arc::clone(&raw), Arc::clone(&screen));
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
                sink.lock().unwrap().extend_from_slice(chunk);
                parser.lock().unwrap().process(chunk);
            }
        });
        Tower {
            child,
            master,
            raw,
            screen,
            home,
            fake,
            _alt: alt,
        }
    }

    fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }

    fn wrote(&self, needle: &[u8]) -> bool {
        contains(&self.raw.lock().unwrap(), needle)
    }

    fn times(&self, needle: &[u8]) -> usize {
        count(&self.raw.lock().unwrap(), needle)
    }

    fn screen_text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn wait_for(&self, what: &str, done: impl Fn(&Tower) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "never: {what}\nscreen:\n{}\nwritten: {:?}",
                self.screen_text(),
                String::from_utf8_lossy(&self.raw()),
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_wrote(&self, needle: &[u8]) {
        self.wait_for(&format!("{:?}", String::from_utf8_lossy(needle)), |t| {
            t.wrote(needle)
        });
    }

    fn wait_screen(&self, text: &str) {
        self.wait_for(&format!("the screen shows `{text}`"), |t| {
            t.screen_text().contains(text)
        });
    }

    /// Waits until the fake has been asked for `n` more listings, so whatever
    /// tower does on a poll has had time to happen.
    fn wait_polls(&self, n: usize) {
        let target = self.fake.calls() + n;
        self.wait_for(&format!("{n} more polls"), |t| t.fake.calls() >= target);
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }

    fn listing(&self) -> PathBuf {
        self.home.path().join("agents.json")
    }

    fn set_rows(&self, needs_me: bool) {
        let project = self.home.path().join("demo");
        let alpha = row(ALPHA, "alpha-task", &project, needs_me);
        let beta = row(BETA, "beta-task", &project, false);
        let text = format!("[{alpha},{beta}]");
        // Atomically, so the fake never lists half a file.
        let tmp = self.home.path().join("agents.json.new");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, self.listing()).unwrap();
    }

    fn set_config_line(&self, on: bool) {
        let path = self.home.path().join(".config/watchtower/config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let text = text
            .lines()
            .filter(|l| !l.starts_with("tab_title"))
            .map(|l| {
                if l.starts_with("style") {
                    format!("{l}\ntab_title = {on}")
                } else {
                    l.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let tmp = path.with_extension("new");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.path().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn signal(&self, signal: i32) {
        // SAFETY: plain kill of the child this test started.
        unsafe { libc::kill(self.child.id() as i32, signal) };
    }

    /// Waits for tower's end and for what it wrote until then.
    fn wait_exit(&mut self) -> Option<i32> {
        let deadline = Instant::now() + WAIT;
        let code = loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                use std::os::unix::process::ExitStatusExt;
                break status.code().or(status.signal().map(|s| 128 + s));
            }
            assert!(Instant::now() < deadline, "tower did not end");
            thread::sleep(Duration::from_millis(20));
        };
        // The bytes are in the pty by now; give the reader its turn.
        thread::sleep(Duration::from_millis(100));
        code
    }
}

impl Drop for Tower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next())
        })
        .is_some_and(|state| state != 'Z')
}

// ---- the text ---------------------------------------------------------------

#[test]
fn the_title_says_tower_and_changes_only_when_its_text_does() {
    let mut tower = Tower::start(Options::default());
    let first = [PUSH.as_bytes(), set("tower").as_slice()].concat();
    tower.wait_wrote(&first);
    // Poll after poll, the same text is not written again.
    tower.wait_polls(4);
    assert_eq!(tower.times(&set("tower")), 1);

    // The status line's counter, in its words.
    tower.set_rows(true);
    let lit = set("tower · 1 needs you");
    tower.wait_wrote(&lit);
    tower.wait_polls(4);
    assert_eq!(tower.times(&lit), 1);
    assert_eq!(tower.times(&set("tower")), 1);

    // Attention over: back to the name, and the previous title stays saved.
    tower.set_rows(false);
    tower.wait_for("the name again", |t| t.times(&set("tower")) == 2);
    assert_eq!(tower.times(PUSH.as_bytes()), 1);
    assert_eq!(tower.times(POP.as_bytes()), 0);

    tower.send(b"q");
    assert_eq!(tower.wait_exit(), Some(0));
}

#[test]
fn it_uses_st_and_tower_never_writes_a_bel() {
    let mut tower = Tower::start(Options {
        needs_me: true,
        ..Options::default()
    });
    tower.wait_wrote(&set("tower · 1 needs you"));
    tower.send(b"q");
    tower.wait_exit();
    assert!(!tower.wrote(b"\x07"));
    assert!(!tower.wrote(b"\x1b]0;"), "only OSC 2");
}

#[test]
fn every_style_sets_it() {
    for style in ["dashboard", "minimal", "workspace"] {
        let tower = Tower::start(Options {
            style,
            needs_me: true,
            ..Options::default()
        });
        tower.wait_wrote(&set("tower · 1 needs you"));
    }
}

// ---- off ------------------------------------------------------------------

#[test]
fn off_tower_never_touches_the_title() {
    let mut tower = Tower::start(Options {
        tab_title: Some(false),
        needs_me: true,
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.wait_polls(3);
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), Some(0));
    for needle in [PUSH.as_bytes(), POP.as_bytes(), b"\x1b]2;".as_slice()] {
        assert!(!tower.wrote(needle), "{needle:?}");
    }
}

#[test]
fn turned_off_in_the_file_the_previous_title_comes_back() {
    let mut tower = Tower::start(Options::default());
    tower.wait_wrote(&set("tower"));
    tower.set_config_line(false);
    tower.wait_wrote(POP.as_bytes());
    // Quitting has nothing left to put back, and on again it is saved again.
    tower.set_config_line(true);
    tower.wait_for("a second save", |t| t.times(PUSH.as_bytes()) == 2);
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), Some(0));
    assert_eq!(tower.times(POP.as_bytes()), 2);
}

// ---- putting the previous title back ----------------------------------------

/// The last title step tower wrote: the neutral name, then the pop.
fn ends_with_a_restore(raw: &[u8]) -> bool {
    let restore = [set("tower").as_slice(), POP.as_bytes()].concat();
    let at = raw.windows(restore.len()).rposition(|w| w == restore);
    let Some(at) = at else {
        return false;
    };
    // Nothing that sets a title after it.
    !contains(&raw[at + restore.len()..], b"\x1b]2;")
}

#[test]
fn quitting_puts_the_previous_title_back_even_with_attention_shown() {
    let mut tower = Tower::start(Options {
        needs_me: true,
        ..Options::default()
    });
    tower.wait_wrote(&set("tower · 1 needs you"));
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), Some(0));
    assert!(ends_with_a_restore(&tower.raw()));
    assert_eq!(tower.times(POP.as_bytes()), 1);
}

#[test]
fn sigterm_puts_it_back_too() {
    let mut tower = Tower::start(Options::default());
    tower.wait_wrote(&set("tower"));
    tower.signal(libc::SIGTERM);
    assert_eq!(tower.wait_exit(), Some(128 + libc::SIGTERM));
    assert!(ends_with_a_restore(&tower.raw()));
}

#[test]
fn after_a_hang_up_nothing_is_written_at_all() {
    let mut tower = Tower::start(Options {
        needs_me: true,
        ..Options::default()
    });
    tower.wait_wrote(&set("tower · 1 needs you"));
    tower.signal(libc::SIGHUP);
    assert_eq!(tower.wait_exit(), Some(128 + libc::SIGHUP));
    // The terminal may be gone (doc/0005): the title stays with it.
    assert!(!tower.wrote(POP.as_bytes()));
}

#[test]
fn a_hand_over_shows_the_previous_title_and_takes_it_back_after() {
    let mut tower = Tower::start(Options {
        needs_me: true,
        on_attach: b"\x1b]2;a title of the child\x1b\\".to_vec(),
        ..Options::default()
    });
    let lit = set("tower · 1 needs you");
    tower.wait_wrote(&lit);
    tower.wait_screen("alpha-task");
    // The row with attention is the selected first row.
    tower.send(b"\r");
    tower.wait_screen("ATTACHED aaaa1111");

    let raw = tower.raw();
    let step = [set("tower").as_slice(), POP.as_bytes(), PUSH.as_bytes()].concat();
    let handed = position(&raw, &step).expect("the previous title before the child");
    let child = position(&raw, b"ATTACHED").unwrap();
    assert!(handed < child);
    // Once only, and the child's own title is the child's business.
    assert_eq!(count(&raw, &step), 1);
    assert_eq!(count(&raw, &lit), 1);

    // The attach ends with the key: tower's title is back, whatever the child
    // left, set again though only the terminal knows what it was, and the
    // saved title is not saved over.
    let after = raw.len();
    tower.send(b"x");
    tower.wait_for("tower's title after the attach", |t| {
        contains(&t.raw()[after..], &set("tower"))
    });
    assert_eq!(
        tower.times(PUSH.as_bytes()),
        2,
        "the first and the hand-over"
    );
    tower.send(b"q");
    assert_eq!(tower.wait_exit(), Some(0));
    assert!(ends_with_a_restore(&tower.raw()));
    assert_eq!(tower.times(POP.as_bytes()), 2, "the hand-over and the exit");
}

// ---- nothing from the pane --------------------------------------------------

#[test]
fn no_title_from_a_panes_program_reaches_the_terminal() {
    let on_attach = b"visible\x1b]2;evil-two\x07\x1b]0;evil-zero\x1b\\\x1b]1;evil-one\x07\
        \x1b[22;2t\x1b[23;2t\x07more"
        .to_vec();
    let mut tower = Tower::start(Options {
        style: "workspace",
        needs_me: true,
        on_attach,
        ..Options::default()
    });
    tower.wait_wrote(&set("tower · 1 needs you"));
    tower.wait_screen("alpha-task");
    tower.send(DOWN);
    tower.wait_screen("visiblemore");
    tower.wait_polls(2);
    assert!(!tower.wrote(b"evil"));
    assert!(!tower.wrote(b"\x07"));
    assert_eq!(tower.times(PUSH.as_bytes()), 1, "tower's own only");
    assert_eq!(tower.times(POP.as_bytes()), 0, "tower's own only");
    // The real title is still tower's: the count did not change for it.
    assert_eq!(tower.times(&set("tower · 1 needs you")), 1);
}

// ---- the Settings row -------------------------------------------------------

#[test]
fn the_settings_row_toggles_and_saves_it() {
    use settings_common::{app_for, env, file, key, open, press, select};
    let env = env(Some(
        "[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
    ));
    let mut app = app_for(&env);
    assert!(app.config().ui_tab_title, "on by default");
    assert_eq!(app.tab_title().as_deref(), Some("tower"));
    open(&mut app);
    select(&mut app, "terminal tab title");
    let value = |app: &watchtower::app::App| {
        app.settings()
            .unwrap()
            .session
            .value(SettingKey::UiTabTitle)
    };
    assert_eq!(value(&app), SettingValue::Bool(true));
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(value(&app), SettingValue::Bool(false));
    let effects = app.handle_key(settings_common::ctrl('s'));
    let [Effect::SaveConfig(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let result = job.run();
    app.config_saved(result);
    assert!(file(&env).contains("tab_title = false"), "{}", file(&env));
    assert!(!app.config().ui_tab_title, "applied at once");
    assert_eq!(app.tab_title(), None);
}

// ---- a restart in the pane, end to end ---------------------------------------

#[test]
fn a_restart_closes_the_panes_client_first_and_attaches_again() {
    let mut tower = Tower::start(Options {
        style: "workspace",
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.send(DOWN);
    tower.wait_screen("ATTACHED aaaa1111");
    tower.wait_for("the first attach", |t| t.log().len() == 1);
    let pids = tower.fake.dir.path().join("attach.pids");
    let pid = |n: usize| -> i32 {
        std::fs::read_to_string(&pids)
            .unwrap()
            .lines()
            .nth(n)
            .unwrap()
            .parse()
            .unwrap()
    };
    let first = pid(0);
    assert!(alive(first));

    tower.send(b"U");
    tower.wait_screen("Restart alpha-task");
    tower.send(b"y");
    tower.wait_for("the client attached again", |t| t.log().len() == 3);
    assert_eq!(
        tower.log(),
        [
            "main attach aaaa1111",
            "main respawn aaaa1111",
            "main attach aaaa1111",
        ]
    );
    tower.wait_for("the old client gone", |_| !alive(first));
    tower.wait_for("the new client running", |_| {
        std::fs::read_to_string(&pids).unwrap().lines().count() == 2
    });
    assert!(alive(pid(1)));
    tower.wait_screen("restarted alpha-task");
}

#[test]
fn a_row_listed_stopped_while_it_restarts_still_comes_back_in_the_pane() {
    // What the real CLI does: while `respawn` runs the row has no process, and
    // its group, Stopped & finished, starts folded.
    let mut tower = Tower::start(Options {
        style: "workspace",
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    let project = tower.home.path().join("demo");
    let stopped = format!(
        "[{{\"state\": \"done\", \"id\": \"{ALPHA}\", \"cwd\": \"{}\", \"kind\": \"background\", \
         \"startedAt\": 1767232793428, \"sessionId\": \"{ALPHA}-5b2d-4f60-9a1e-7c3b2d4e5f60\", \
         \"name\": \"alpha-task\"}},{}]",
        project.display(),
        row(BETA, "beta-task", &project, false),
    );
    std::fs::write(tower.listing().with_extension("json.during"), stopped).unwrap();
    tower.send(DOWN);
    tower.wait_screen("ATTACHED aaaa1111");
    tower.send(b"U");
    tower.wait_screen("Restart alpha-task");
    tower.send(b"y");
    tower.wait_for("attached again", |t| t.log().len() == 3);
    assert_eq!(
        tower.log(),
        [
            "main attach aaaa1111",
            "main respawn aaaa1111",
            "main attach aaaa1111",
        ]
    );
}

#[test]
fn restarting_every_session_uses_each_accounts_own_command() {
    let mut tower = Tower::start(Options {
        two_accounts: true,
        ..Options::default()
    });
    tower.wait_screen("beta-task");
    tower.wait_screen("alpha-task");
    tower.send(b"\x15"); // Ctrl+U
    tower.wait_screen("Restart 2 sessions");
    tower.wait_screen("main: 1 session");
    tower.wait_screen("alt: 1 session");
    // Not on a small `y`.
    tower.send(b"y");
    tower.wait_screen("press Y");
    assert!(tower.log().is_empty());
    tower.send(b"Y");
    tower.wait_for("both restarted", |t| t.log().len() == 2);
    assert_eq!(
        tower.log(),
        ["main respawn aaaa1111", "alt respawn bbbb2222"]
    );
    tower.wait_screen("restarted 2 sessions (main 1, alt 1)");
    let bins = [&tower.fake, tower._alt.as_ref().unwrap()];
    for fake in bins {
        assert!(
            fake.all_argv()
                .iter()
                .filter(|argv| argv[0] == "respawn")
                .all(|argv| argv.len() == 2 && !argv.contains(&"--all".to_owned())),
            "by id, never --all"
        );
    }
}
