//! The workspace style end to end (doc/0045): the real binary in a
//! pseudo-terminal this test plays, against a fake `claude` whose `attach`
//! writes what a test asks for and records every byte it reads. Nothing here
//! starts a real `claude`, so the mouse events are safe to send: a real attach
//! copies on a mouse release, which on WSL overwrites the Windows clipboard.
//!
//! The waits are for something to happen, with a long deadline; what must not
//! happen is checked after what must happen, or after a generous pause, so a
//! slow machine cannot fail them.

#[path = "support/fake_claude.rs"]
mod fake_claude;

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

const WAIT: Duration = Duration::from_secs(20);
/// Long enough for tower to have done what it must not do.
const PAUSE: Duration = Duration::from_millis(700);

const ALPHA: &str = "aaaa1111";
const BETA: &str = "bbbb2222";
const STOPPED: &str = "cccc3333";
const ORCHESTRATOR: &str = "dddd4444";
const STARTED: &str = "eeee5555";

fn row(id: &str, name: &str, cwd: &Path, live: bool) -> String {
    let process = if live {
        "\"pid\": 4194304, \"status\": \"idle\", \"state\": \"done\", "
    } else {
        "\"state\": \"done\", "
    };
    format!(
        "{{{process}\"id\": \"{id}\", \"cwd\": \"{}\", \"kind\": \"background\", \
         \"startedAt\": 1767232793428, \"sessionId\": \"{id}-5b2d-4f60-9a1e-7c3b2d4e5f60\", \
         \"name\": \"{name}\"}}",
        cwd.display()
    )
}

struct Options {
    rows: u16,
    cols: u16,
    style: &'static str,
    mouse: bool,
    /// What the fake's `attach` writes to its terminal.
    on_attach: Vec<u8>,
    /// Every row is live.
    live_only: bool,
    /// A listed row named `orchestrator`, with the id [`ORCHESTRATOR`].
    orchestrator: bool,
    /// The name a session the fake starts with `--bg` gets; it has the id
    /// [`STARTED`] and is listed from then on.
    starts: Option<&'static str>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            rows: 40,
            cols: 160,
            style: "workspace",
            mouse: true,
            on_attach: Vec::new(),
            live_only: true,
            orchestrator: false,
            starts: None,
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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

impl Tower {
    fn start(options: Options) -> Tower {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("demo");
        std::fs::create_dir_all(&project).unwrap();
        let mut rows = vec![
            row(ALPHA, "alpha-task", &project, true),
            row(BETA, "beta-task", &project, true),
        ];
        if !options.live_only {
            rows.push(row(STOPPED, "gamma-stopped", &project, false));
        }
        if options.orchestrator {
            rows.push(row(ORCHESTRATOR, "orchestrator", &project, true));
        }
        let listing = home.path().join("agents.json");
        std::fs::write(&listing, format!("[{}]", rows.join(","))).unwrap();
        // What the fake lists once it has started a session.
        let after = home.path().join("agents-after.json");
        if let Some(name) = options.starts {
            std::fs::create_dir_all(project.join(".git")).unwrap();
            rows.push(row(STARTED, name, &project, true));
            std::fs::write(&after, format!("[{}]", rows.join(","))).unwrap();
        }
        let on_attach = home.path().join("on_attach");
        std::fs::write(&on_attach, &options.on_attach).unwrap();
        let body = format!(
            "case \"$1\" in\n\
             agents) cat '{listing}' ;;\n\
             attach)\n\
               echo \"$2 $$\" >> \"$FAKE_DIR/attaches\"\n\
               stty raw -echo\n\
               printf '\\033[?1049h\\033[2J\\033[HATTACHED %s\\r\\n' \"$2\"\n\
               cat '{on_attach}'\n\
               if [ -e \"$FAKE_DIR/fail\" ]; then\n\
                 printf 'no such job\\r\\n'; exit 1\n\
               fi\n\
               exec cat >> \"$FAKE_DIR/keys.$2\"\n\
               ;;\n\
             --bg)\n\
               echo started >> \"$FAKE_DIR/starts\"\n\
               cp '{after}' '{listing}'\n\
               printf 'backgrounded · {STARTED} · {started}\\n'\n\
               ;;\n\
             *) ;;\n\
             esac",
            listing = listing.display(),
            after = after.display(),
            started = options.starts.unwrap_or("none"),
            on_attach = on_attach.display(),
        );
        let fake = {
            let _guard = lock();
            Fake::new("claude", &body)
        };
        let config_dir = home.path().join(".config/watchtower");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "default_account = \"main\"\n\
                 project_roots = [\"{root}\"]\n\
                 [ui]\nstyle = \"{style}\"\nmouse = {mouse}\n\
                 [[accounts]]\nname = \"main\"\ncommand = \"'{cmd}'\"\n",
                root = home.path().display(),
                style = options.style,
                mouse = options.mouse,
                cmd = fake.program().display(),
            ),
        )
        .unwrap();

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
        set_size(master.as_raw_fd(), options.rows, options.cols);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tower"));
        cmd.env_clear()
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .env("TERM_PROGRAM", "ghostty")
            .env("TMUX", "/tmp/x,1,0")
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
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            options.rows,
            options.cols,
            0,
        )));
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
        }
    }

    fn screen_text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn raw_len(&self) -> usize {
        self.raw.lock().unwrap().len()
    }

    /// Whether tower wrote `needle` after `from` bytes.
    fn wrote_since(&self, from: usize, needle: &[u8]) -> bool {
        contains(&self.raw.lock().unwrap()[from..], needle)
    }

    fn wrote(&self, needle: &[u8]) -> bool {
        self.wrote_since(0, needle)
    }

    fn wait_for(&self, what: &str, done: impl Fn(&Tower) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "never: {what}\nscreen:\n{}",
                self.screen_text()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_screen(&self, text: &str) {
        self.wait_for(&format!("the screen shows `{text}`"), |t| {
            t.screen_text().contains(text)
        });
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }

    /// The fake's `attach` calls so far: short id and pid.
    fn attaches(&self) -> Vec<(String, i32)> {
        std::fs::read_to_string(self.fake.dir.path().join("attaches"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (id, pid) = l.split_once(' ')?;
                Some((id.to_owned(), pid.parse().ok()?))
            })
            .collect()
    }

    /// Waits until `n` attaches happened and returns the last.
    fn wait_attaches(&self, n: usize) -> (String, i32) {
        self.wait_for(&format!("{n} attaches"), |t| t.attaches().len() >= n);
        self.attaches().pop().unwrap()
    }

    fn keys_of(&self, id: &str) -> Vec<u8> {
        std::fs::read(self.fake.dir.path().join(format!("keys.{id}"))).unwrap_or_default()
    }

    fn wait_keys(&self, id: &str, want: &[u8]) {
        self.wait_for(&format!("{id} reads {want:?}"), |t| {
            contains(&t.keys_of(id), want)
        });
    }

    /// This tower's children that are zombies.
    fn zombies(&self) -> Vec<i32> {
        children_of(self.child.id() as i32)
            .into_iter()
            .filter(|(_, state)| *state == 'Z')
            .map(|(pid, _)| pid)
            .collect()
    }

    fn tower_dir(&self) -> PathBuf {
        self.home.path().to_path_buf()
    }

    fn stop(mut self) -> Option<i32> {
        // SAFETY: plain kill of the child this test started.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        wait_child(&mut self.child)
    }
}

impl Drop for Tower {
    fn drop(&mut self) {
        // SAFETY: plain kill of the child this test started.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = wait_child(&mut self.child);
    }
}

fn wait_child(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code();
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    child.wait().ok().and_then(|s| s.code())
}

/// Every child of `parent`, with its state.
fn children_of(parent: i32) -> Vec<(i32, char)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, rest)) = stat.rsplit_once(") ") else {
            continue;
        };
        let mut fields = rest.split(' ');
        let state = fields.next().and_then(|s| s.chars().next()).unwrap_or('?');
        let ppid: i32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if ppid == parent {
            out.push((pid, state));
        }
    }
    out
}

/// Whether the process is there and not a zombie.
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next())
        })
        .is_some_and(|state| state != 'Z')
}

fn cpu_ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

fn other(id: &str) -> &'static str {
    if id == ALPHA { BETA } else { ALPHA }
}

/// The session the pane shows after `j` or `k` moved to the other row.
fn move_to_other(tower: &mut Tower, from: &str) {
    // The rows sit next to each other in one group; only one of the two keys
    // moves from `from` to the other row.
    let at_top = {
        let text = tower.screen_text();
        let a = text.find(&name_of(from)).unwrap();
        let b = text.find(&name_of(other(from))).unwrap();
        a < b
    };
    tower.send(if at_top { b"j" } else { b"k" });
}

fn name_of(id: &str) -> String {
    match id {
        ALPHA => "alpha-task".to_owned(),
        BETA => "beta-task".to_owned(),
        _ => "gamma-stopped".to_owned(),
    }
}

/// A tower with the selected row attached: its id and client pid.
fn attached(options: Options) -> (Tower, String, i32) {
    let mut tower = Tower::start(options);
    // Nothing attaches until a row is chosen (Home, doc/0045).
    tower.wait_screen("alpha-task");
    tower.send(b"j");
    let (id, pid) = tower.wait_attaches(1);
    tower.wait_screen(&format!("ATTACHED {id}"));
    (tower, id, pid)
}

#[test]
fn the_start_attaches_nothing_until_a_row_is_chosen() {
    let mut tower = Tower::start(Options::default());
    tower.wait_screen("Home");
    tower.wait_screen("alpha-task");
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty(), "{}", tower.screen_text());
    tower.wait_for("capture on", |t| t.wrote(b"\x1b[?1000h"));

    // A click on a card chooses it.
    let row = {
        let text = tower.screen_text();
        text.lines().position(|l| l.contains("alpha-task")).unwrap() as u16 + 1
    };
    tower.send(&mouse(0, 5, row + 1, 'M'));
    tower.send(&mouse(0, 5, row + 1, 'm'));
    let (id, pid) = tower.wait_attaches(1);
    assert_eq!(id, ALPHA);
    tower.wait_screen(&format!("ATTACHED {ALPHA}"));
    assert!(
        tower.keys_of(ALPHA).is_empty(),
        "a list click stays with tower"
    );

    // A click on `◆ tower` goes Home and leaves the client attached.
    tower.send(&mouse(0, 4, 1, 'M'));
    tower.send(&mouse(0, 4, 1, 'm'));
    tower.wait_screen("Home");
    thread::sleep(PAUSE);
    assert!(alive(pid), "Home closes nothing");
    assert_eq!(tower.attaches().len(), 1);
    assert!(!tower.screen_text().contains("ATTACHED"));

    // The wheel comes back to it, with the same client.
    tower.send(&mouse(65, 5, 10, 'M'));
    tower.wait_screen(&format!("ATTACHED {ALPHA}"));
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 1);
    assert!(alive(pid));
}

/// The foreground attach would leave the alternate screen; the pane never does.
const LEFT_THE_SCREEN: &[u8] = b"\x1b[?1049l";

#[test]
fn o_opens_the_orchestrator_in_the_pane_with_the_keys() {
    let mut tower = Tower::start(Options {
        orchestrator: true,
        ..Options::default()
    });
    // Its row: the footer's hint has the word too.
    tower.wait_screen("▪ orchestrator");
    tower.wait_screen("Home");
    tower.send(b"o");
    let (id, pid) = tower.wait_attaches(1);
    assert_eq!(id, ORCHESTRATOR);
    tower.wait_screen(&format!("ATTACHED {ORCHESTRATOR}"));
    tower.wait_screen("keyboard here");
    tower.wait_screen(" TERMINAL ");
    let text = tower.screen_text();
    assert!(text.contains('┏'), "its card is selected: {text}");
    assert!(text.contains("alpha-task"), "the list stays: {text}");
    assert!(
        !tower.wrote(LEFT_THE_SCREEN),
        "the terminal is not handed over"
    );
    tower.send(b"typed");
    tower.wait_keys(ORCHESTRATOR, b"typed");
    // Ctrl+Z returns to the list and the client stays.
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    assert!(alive(pid));
    // `o` again only gives it the keys.
    tower.send(b"o");
    tower.wait_screen(" TERMINAL ");
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 1);
    assert!(alive(pid));
    // Claude Code's Ctrl+C reaches it here; tower does not quit.
    tower.send(b"\x03");
    tower.wait_keys(ORCHESTRATOR, b"typed\x03");
}

#[test]
fn o_starts_a_missing_orchestrator_and_opens_it_when_it_appears() {
    let mut tower = Tower::start(Options {
        starts: Some("orchestrator"),
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.send(b"o");
    let (id, _pid) = tower.wait_attaches(1);
    assert_eq!(id, STARTED);
    tower.wait_screen(&format!("ATTACHED {STARTED}"));
    tower.wait_screen(" TERMINAL ");
    assert!(tower.screen_text().contains("▪ orchestrator"));
    assert!(
        !tower.wrote(LEFT_THE_SCREEN),
        "the terminal is not handed over"
    );
    let starts = std::fs::read_to_string(tower.fake.dir.path().join("starts")).unwrap();
    assert_eq!(starts.lines().count(), 1, "started once");
}

#[test]
fn start_and_attach_opens_the_new_session_in_the_pane_with_the_keys() {
    let mut tower = Tower::start(Options {
        starts: Some("started-task"),
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.send(b"n");
    tower.wait_screen("New session");
    tower.wait_screen("demo");
    tower.send(b"\t");
    tower.send(b"do the thing");
    tower.wait_screen("do the thing");
    tower.send(b"\x1b\r");
    let (id, pid) = tower.wait_attaches(1);
    assert_eq!(id, STARTED);
    tower.wait_screen(&format!("ATTACHED {STARTED}"));
    tower.wait_screen(" TERMINAL ");
    let text = tower.screen_text();
    assert!(text.contains("started-task"), "{text}");
    assert!(text.contains('┏'), "its card is selected: {text}");
    assert!(
        !tower.wrote(LEFT_THE_SCREEN),
        "the terminal is not handed over"
    );
    tower.send(b"hello");
    tower.wait_keys(STARTED, b"hello");
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    assert!(alive(pid));
}

#[test]
fn a_click_on_a_group_folds_it() {
    let mut tower = Tower::start(Options::default());
    tower.wait_screen("alpha-task");
    // The terminal's rows count from 1.
    let row = {
        let text = tower.screen_text();
        text.lines().position(|l| l.contains("Ready for")).unwrap() as u16 + 1
    };
    tower.send(&mouse(0, 5, row, 'M'));
    tower.send(&mouse(0, 5, row, 'm'));
    tower.wait_for("folded", |t| !t.screen_text().contains("alpha-task"));
    tower.send(&mouse(0, 5, row, 'M'));
    tower.send(&mouse(0, 5, row, 'm'));
    tower.wait_screen("alpha-task");
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty(), "a header attaches nothing");
}

#[test]
fn selection_attaches_once_with_a_clean_environment_and_the_sessions_folder() {
    let (tower, id, pid) = attached(Options::default());
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 1, "attached more than once");
    assert!(alive(pid));
    assert_eq!(
        tower.screen_text().matches("ATTACHED").count(),
        1,
        "{}",
        tower.screen_text()
    );
    // The fake records its environment and folder per call.
    let calls = tower.fake.all_argv();
    let n = 1 + calls
        .iter()
        .position(|argv| argv.first().is_some_and(|a| a == "attach"))
        .unwrap();
    assert_eq!(tower.fake.argv_of(n), ["attach", id.as_str()]);
    assert_eq!(
        tower.fake.cwd_of(n),
        tower.tower_dir().join("demo").canonicalize().unwrap()
    );
    assert_eq!(
        tower.fake.env_var_of(n, "TERM").as_deref(),
        Some("xterm-256color")
    );
    assert_eq!(
        tower.fake.env_var_of(n, "COLORTERM").as_deref(),
        Some("truecolor")
    );
    assert_eq!(tower.fake.env_var_of(n, "TERM_PROGRAM"), None);
    assert_eq!(tower.fake.env_var_of(n, "TMUX"), None);
}

#[test]
fn switching_replaces_the_client_and_leaves_no_zombie() {
    let (mut tower, first, first_pid) = attached(Options::default());
    move_to_other(&mut tower, &first);
    let (second, second_pid) = tower.wait_attaches(2);
    assert_eq!(second, other(&first));
    tower.wait_screen(&format!("ATTACHED {second}"));
    tower.wait_for("the first client is gone", |_| !alive(first_pid));
    tower.wait_for("no zombie", |t| t.zombies().is_empty());
    assert!(alive(second_pid));
    assert!(!tower.screen_text().contains(&format!("ATTACHED {first}")));
}

#[test]
fn ctrl_z_and_f12_give_the_list_the_keys_and_the_client_stays() {
    let (mut tower, id, pid) = attached(Options {
        on_attach: b"\x1b[?25h".to_vec(),
        ..Options::default()
    });
    for back in [&b"\x1a"[..], &b"\x1b[24~"[..]] {
        tower.send(b"\r");
        tower.wait_screen("TERMINAL");
        tower.send(b"mark");
        tower.wait_keys(&id, b"mark");
        tower.send(back);
        tower.wait_screen(" LIST ");
        assert!(alive(pid));
    }
    // Neither key went to the child, and back in the list letters do not.
    tower.send(b"?");
    tower.wait_screen("Terminal pane");
    tower.send(b"\x1b");
    thread::sleep(PAUSE);
    let keys = tower.keys_of(&id);
    assert_eq!(keys, b"markmark", "{keys:?}");
    assert_eq!(tower.attaches().len(), 1);
}

#[test]
fn keys_reach_the_terminal_as_a_terminal_sends_them() {
    let (mut tower, id, _pid) = attached(Options {
        on_attach: b"\x1b[?2004h".to_vec(),
        ..Options::default()
    });
    tower.send(b"\r");
    tower.wait_screen("TERMINAL");
    let keys: [(&[u8], &[u8]); 8] = [
        (b"hi", b"hi"),
        (b"\x1b[Z", b"\x1b[Z"),
        (b"\x1b\r", b"\x1b\r"),
        (b"\x1b[A", b"\x1b[A"),
        (b"\x03", b"\x03"),
        // `Ctrl+C` is Claude's here, not tower's quit; a lone Esc goes too.
        (b"\x1b[13;2u", b"\x1b[13;2u"),
        (b"\x1b[200~a\nb\x1b[201~", b"\x1b[200~a\nb\x1b[201~"),
        ("я".as_bytes(), "я".as_bytes()),
    ];
    for (typed, want) in keys {
        tower.send(typed);
        tower.wait_keys(&id, want);
    }
}

#[test]
fn nothing_raw_reaches_the_real_terminal() {
    let on_attach = b"visible\x07\x1b]0;a title\x07\x1b]9;a note\x07\x1b]777;notify;x;y\x07\
        \x1b]99;;z\x1b\\\x1b]52;c;?\x07more"
        .to_vec();
    let (tower, _id, _pid) = attached(Options {
        on_attach,
        ..Options::default()
    });
    tower.wait_screen("visiblemore");
    thread::sleep(PAUSE);
    for raw in [
        &b"\x07"[..],
        b"\x1b]0;",
        b"\x1b]9;",
        b"\x1b]777;",
        b"\x1b]99;",
        b"\x1b]52;",
    ] {
        assert!(!tower.wrote(raw), "{raw:?} reached the terminal");
    }
}

#[test]
fn a_clipboard_write_passes_and_a_read_does_not() {
    let on_attach = b"\x1b]52;c;?\x07\x1b]52;c;aGVsbG8=\x07done".to_vec();
    let (tower, _id, _pid) = attached(Options {
        on_attach,
        ..Options::default()
    });
    tower.wait_screen("done");
    tower.wait_for("the clipboard write", |t| {
        t.wrote(b"\x1b]52;c;aGVsbG8=\x1b\\")
    });
    assert!(!tower.wrote(b"\x1b]52;c;?"));
}

/// The cell at `col`, `row` of the screen, from 1, as tower's terminal gets it.
fn mouse(button: u8, col: u16, row: u16, last: char) -> Vec<u8> {
    format!("\x1b[<{button};{col};{row}{last}").into_bytes()
}

/// With the bar 28 columns wide on 160: the pane's inside starts at the
/// bar's width + 2 (terminal coordinates from 1) and the second row.
const PANE_X: u16 = 28 + 2;
const PANE_Y: u16 = 2;

#[test]
fn mouse_events_go_on_clamped_and_only_while_the_mode_is_on() {
    let (mut tower, id, _pid) = attached(Options {
        on_attach: b"\x1b[?1000;1002;1003;1006h".to_vec(),
        ..Options::default()
    });
    assert!(tower.wrote(b"\x1b[?1000h"), "capture is on in this style");
    // A click on the pane gives it the keys and goes on, in its coordinates.
    tower.send(&mouse(0, PANE_X + 2, PANE_Y + 1, 'M'));
    tower.wait_screen("TERMINAL");
    tower.wait_keys(&id, b"\x1b[<0;3;2M");
    // A drag past the pane's corners stays inside it, and the release too.
    tower.send(&mouse(32, 1, 1, 'M'));
    tower.wait_keys(&id, b"\x1b[<32;1;1M");
    tower.send(&mouse(32, 500, 200, 'M'));
    tower.send(&mouse(0, 500, 200, 'm'));
    let screen = tower.pane_size();
    let want = format!("\x1b[<0;{};{}m", screen.1, screen.0);
    tower.wait_keys(&id, want.as_bytes());
    // After the release a drag from outside is no gesture.
    let before = tower.keys_of(&id).len();
    tower.send(&mouse(32, 1, 1, 'M'));
    // A press outside the pane starts nothing there, and returns the keys.
    tower.send(&mouse(0, 1, 30, 'M'));
    tower.wait_screen(" LIST ");
    tower.send(&mouse(0, 1, 30, 'm'));
    thread::sleep(PAUSE);
    assert_eq!(tower.keys_of(&id).len(), before, "{:?}", tower.keys_of(&id));
}

impl Tower {
    /// The pane's inside as tower told the child, rows and columns, from the
    /// fixed layout at 40 × 160.
    fn pane_size(&self) -> (u16, u16) {
        (40 - 3, 160 - 30)
    }
}

#[test]
fn the_mouse_goes_nowhere_while_the_child_did_not_ask_for_it() {
    let (mut tower, id, _pid) = attached(Options::default());
    tower.send(&mouse(0, PANE_X + 2, PANE_Y + 1, 'M'));
    tower.wait_screen("TERMINAL");
    tower.send(&mouse(0, PANE_X + 2, PANE_Y + 1, 'm'));
    tower.send(&mouse(64, PANE_X + 2, PANE_Y + 1, 'M'));
    tower.send(b"end");
    tower.wait_keys(&id, b"end");
    assert_eq!(tower.keys_of(&id), b"end");
}

#[test]
fn mouse_false_turns_the_capture_off() {
    let (tower, _id, _pid) = attached(Options {
        mouse: false,
        ..Options::default()
    });
    thread::sleep(PAUSE);
    assert!(!tower.wrote(b"\x1b[?1000h"));
    assert!(tower.wrote(b"\x1b[?2004h"), "paste still goes to the pane");
}

#[test]
fn a_dialog_draws_over_the_pane_and_takes_the_keys() {
    let on_attach = b"first line of the pane\r\n\x1b[?25h".to_vec();
    let (mut tower, id, pid) = attached(Options {
        on_attach,
        ..Options::default()
    });
    tower.wait_screen("first line of the pane");
    tower.send(b"n");
    tower.wait_screen("New session");
    let text = tower.screen_text();
    assert!(text.contains("first line of the pane"), "{text}");
    assert!(text.contains("alpha-task"), "{text}");
    tower.send(b"typed-into-the-dialog");
    tower.wait_screen("typed-into-the-dialog");
    thread::sleep(PAUSE);
    assert!(tower.keys_of(&id).is_empty());
    assert!(alive(pid));
    // Esc asks first when the task has text; n keeps the dialog's question
    // closed; here it simply closes when nothing was dispatched.
    tower.send(b"\x1b");
    tower.wait_for("the question or the list", |t| {
        let s = t.screen_text();
        !s.contains("New session") || s.contains("Discard")
    });
    if tower.screen_text().contains("New session") {
        tower.send(b"y");
    }
    tower.wait_for("the dialog is gone", |t| {
        !t.screen_text().contains("New session")
    });
    assert!(tower.screen_text().contains("first line of the pane"));
}

#[test]
fn the_cursor_shows_only_while_the_terminal_has_the_keys() {
    let (mut tower, _id, _pid) = attached(Options {
        on_attach: b"\x1b[?25h".to_vec(),
        ..Options::default()
    });
    thread::sleep(PAUSE);
    let hidden = |t: &Tower| t.screen.lock().unwrap().screen().hide_cursor();
    assert!(hidden(&tower), "the list has the keys");
    tower.send(b"\r");
    tower.wait_for("the cursor", |t| !hidden(t));
    tower.send(b"\x1a");
    tower.wait_for("no cursor", hidden);
    // A window over the terminal hides Claude's cursor too.
    tower.send(b"\r");
    tower.wait_for("the cursor again", |t| !hidden(t));
}

#[test]
fn a_stopped_row_is_not_attached_on_selection_and_enter_wakes_it() {
    let (mut tower, _id, _pid) = attached(Options {
        live_only: false,
        ..Options::default()
    });
    // Stopped & finished starts folded: to its header, unfold, to its row.
    tower.send(b"G");
    tower.wait_for("the header is selected", |t| {
        t.screen_text().contains("Stopped")
    });
    thread::sleep(PAUSE);
    tower.send(b"\r");
    tower.wait_screen("gamma-stopped");
    tower.send(b"G");
    tower.wait_screen("Enter wakes it here");
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 1, "selection woke a stopped row");
    tower.send(b"\r");
    let (id, _) = tower.wait_attaches(2);
    assert_eq!(id, STOPPED);
    tower.wait_screen(&format!("ATTACHED {STOPPED}"));
    tower.wait_screen("TERMINAL");
}

#[test]
fn a_narrow_terminal_gets_the_dashboard_and_nothing_attaches() {
    let mut tower = Tower::start(Options {
        rows: 40,
        cols: 90,
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.wait_screen("Sessions");
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty());
    assert!(!tower.wrote(b"\x1b[?1000h"));
    // It grows: the workspace comes, on Home, and a row chosen attaches.
    set_size(tower.master.as_raw_fd(), 40, 160);
    tower.screen.lock().unwrap().screen_mut().set_size(40, 160);
    tower.wait_screen("Home");
    tower.send(b"j");
    let (_, pid) = tower.wait_attaches(1);
    assert!(alive(pid));
    tower.wait_for("capture on", |t| t.wrote(b"\x1b[?1000h"));
    // It shrinks: the client goes, and the capture.
    let mark = tower.raw_len();
    set_size(tower.master.as_raw_fd(), 40, 90);
    tower.screen.lock().unwrap().screen_mut().set_size(40, 90);
    tower.wait_for("the client ended", |_| !alive(pid));
    tower.wait_for("capture off", |t| t.wrote_since(mark, b"\x1b[?1000l"));
    tower.send(b"j");
    let _ = &mut tower;
}

#[test]
fn a_client_that_fails_leaves_its_message_and_the_keys_on_the_list() {
    let mut tower = Tower::start(Options {
        on_attach: b"\x1b[?25h".to_vec(),
        ..Options::default()
    });
    std::fs::write(tower.fake.dir.path().join("fail"), "").unwrap();
    tower.wait_screen("alpha-task");
    tower.send(b"j");
    let (_, pid) = tower.wait_attaches(1);
    tower.wait_screen("no such job");
    tower.wait_for("exit reported", |t| {
        t.screen_text().contains("exited with code 1")
    });
    tower.wait_for("reaped", |_| !alive(pid));
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 1, "a failed client is not retried");
    tower.wait_screen(" LIST ");
    tower.wait_for("no zombie", |t| t.zombies().is_empty());
}

#[test]
fn tower_uses_almost_no_cpu_while_idle() {
    let (tower, _id, pid) = attached(Options::default());
    thread::sleep(PAUSE);
    let before = cpu_ticks(tower.child.id());
    thread::sleep(Duration::from_secs(2));
    let used = cpu_ticks(tower.child.id()) - before;
    assert!(
        used < 40,
        "tower used {used} ticks of CPU in 2 s while idle"
    );
    assert!(alive(pid));
}

#[test]
fn quitting_ends_the_client_and_keeps_tower_clean() {
    let (mut tower, _id, pid) = attached(Options::default());
    tower.send(b"q");
    tower.wait_for("the client ended", |_| !alive(pid));
    let code = wait_child(&mut tower.child);
    assert_eq!(code, Some(0));
    assert!(tower.wrote(b"\x1b[?1000l"), "capture off on the way out");
    assert!(tower.wrote(b"\x1b[?2004l"), "paste off on the way out");
}

#[test]
fn a_termination_signal_ends_the_client_too() {
    let (tower, _id, pid) = attached(Options::default());
    assert_eq!(tower.stop(), Some(128 + libc::SIGTERM));
    assert!(!alive(pid));
}

#[test]
fn a_hang_up_of_the_terminal_ends_the_client_too() {
    let (tower, _id, pid) = attached(Options::default());
    // Closing the only master is a hang-up for tower's terminal; what reads it
    // in this test keeps a copy, so the signal stands in for the close.
    // SAFETY: plain kill of the child this test started.
    unsafe { libc::kill(tower.child.id() as i32, libc::SIGHUP) };
    let deadline = Instant::now() + WAIT;
    while alive(pid) {
        assert!(Instant::now() < deadline, "the client outlived tower");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_other_styles_never_attach_nor_capture_the_mouse() {
    for style in ["dashboard", "minimal"] {
        let mut tower = Tower::start(Options {
            style,
            ..Options::default()
        });
        tower.wait_screen("alpha-task");
        tower.send(b"j");
        thread::sleep(PAUSE);
        tower.send(b"k");
        thread::sleep(PAUSE);
        assert!(tower.attaches().is_empty(), "{style}");
        for mode in [&b"\x1b[?1000h"[..], b"\x1b[?1003h", b"\x1b[?2004h"] {
            assert!(!tower.wrote(mode), "{style} asked for {mode:?}");
        }
        // Not even on the way out.
        let code = tower.stop();
        assert_eq!(code, Some(128 + libc::SIGTERM), "{style}");
    }
}
