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
    /// So many more live rows, `row-00` and on.
    more_rows: usize,
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
            more_rows: 0,
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
        for k in 0..options.more_rows {
            rows.push(row(
                &format!("f{k:07}"),
                &format!("row-{k:02}"),
                &project,
                true,
            ));
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
        // In the bar: the pane's title names a row too.
        let text = tower.screen_text();
        let bar: Vec<String> = text.lines().map(|l| l.chars().take(28).collect()).collect();
        let at = |name: &str| bar.iter().position(|l| l.contains(name)).unwrap();
        at(&name_of(from)) < at(&name_of(other(from)))
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

/// The pid of the shell in the pane: it prints it, and this reads the screen.
fn shell_pid(tower: &mut Tower) -> i32 {
    tower.send(b"echo pid=$$-end\r");
    tower.wait_for("the shell's pid", |t| pid_on(&t.screen_text()).is_some());
    pid_on(&tower.screen_text()).unwrap()
}

fn pid_on(screen: &str) -> Option<i32> {
    screen
        .lines()
        .flat_map(|l| l.split("pid="))
        .skip(1)
        .find_map(|rest| rest.split("-end").next()?.parse().ok())
}

/// `!` on the selected row, in the pane: the shell's pid.
fn shell_terminal(tower: &mut Tower) -> i32 {
    tower.send(b"!");
    tower.wait_screen("shell  ~/demo");
    tower.wait_screen(" TERMINAL ");
    shell_pid(tower)
}

#[test]
fn the_shell_opens_toggles_survives_browsing_and_ends_with_exit() {
    let (mut tower, id, client) = attached(Options {
        on_attach: b"CLAUDE SCREEN\r\n".to_vec(),
        ..Options::default()
    });
    tower.wait_screen("CLAUDE SCREEN");
    let pid = shell_terminal(&mut tower);
    assert!(alive(pid));
    assert!(alive(client), "beside the client");
    assert!(
        !tower.wrote(LEFT_THE_SCREEN),
        "the terminal is not handed over"
    );
    assert!(!tower.screen_text().contains("CLAUDE SCREEN"));
    // The folder, and the same cleaning of the environment as the client's.
    tower.send(b"echo [$PWD][$TERM][$TERM_PROGRAM][$TMUX]\r");
    let want = format!(
        "[{}][xterm-256color][][]",
        tower.tower_dir().join("demo").display()
    );
    tower.wait_for("its environment", |t| {
        t.screen_text()
            .replace(['\n', ' ', '│', '┃'], "")
            .contains(&want.replace(' ', ""))
    });
    // Typed keys went nowhere else.
    tower.send(b"echo typed$((40+2))\r");
    tower.wait_screen("typed42");
    assert!(tower.keys_of(&id).is_empty());

    // Ctrl+Z returns to the list; the shell is not stopped.
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    assert!(alive(pid));
    // `!` shows Claude, `!` again the shell, with the keys.
    tower.send(b"!");
    tower.wait_screen("CLAUDE SCREEN");
    assert!(!tower.screen_text().contains("typed42"));
    tower.send(b"!");
    tower.wait_screen("typed42");
    tower.wait_screen(" TERMINAL ");
    assert_eq!(tower.attaches().len(), 1, "no second client");
    tower.send(b"\x1b[24~");
    tower.wait_screen(" LIST ");

    // Browse: the other row attaches, the shell stays; back, it shows.
    move_to_other(&mut tower, &id);
    let (other_id, _) = tower.wait_attaches(2);
    tower.wait_for("the other row", |t| !t.screen_text().contains("typed42"));
    assert!(alive(pid), "the shell outlives browsing");
    move_to_other(&mut tower, &other_id);
    tower.wait_screen("typed42");
    tower.wait_screen("shell  ~/demo");
    thread::sleep(PAUSE);
    assert_eq!(tower.attaches().len(), 2, "its row needs no client now");

    // `exit`: back to what the row shows, with the list focused.
    tower.send(b"\r");
    tower.wait_screen(" TERMINAL ");
    tower.send(b"exit\r");
    tower.wait_for("the shell is gone", |_| !alive(pid));
    tower.wait_screen(" LIST ");
    tower.wait_for("the shell's title goes", |t| {
        !t.screen_text().contains("shell  ~/demo")
    });
    tower.wait_for("no zombie", |t| t.zombies().is_empty());
    assert!(!tower.wrote(LEFT_THE_SCREEN));
}

#[test]
fn the_shell_on_another_row_replaces_the_old_one() {
    let (mut tower, id, _client) = attached(Options::default());
    let first = shell_terminal(&mut tower);
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    move_to_other(&mut tower, &id);
    tower.send(b"!");
    tower.wait_screen("closed the shell in ~/demo");
    tower.wait_for("the old shell is gone", |_| !alive(first));
    tower.wait_for("the old shell's screen goes", |t| {
        pid_on(&t.screen_text()).is_none()
    });
    let second = shell_pid(&mut tower);
    assert_ne!(first, second);
    assert!(alive(second));
    tower.wait_for("no zombie", |t| t.zombies().is_empty());
}

#[test]
fn quitting_ends_the_shell_with_no_zombie() {
    let (mut tower, _id, client) = attached(Options::default());
    let pid = shell_terminal(&mut tower);
    let code = tower.stop();
    assert_eq!(code, Some(128 + libc::SIGTERM));
    assert!(!alive(pid), "the shell ended with tower");
    assert!(!alive(client), "the client too");
    // `q` from the list as well.
    let (mut tower, _id, _client) = attached(Options::default());
    let pid = shell_terminal(&mut tower);
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    tower.send(b"q");
    let deadline = Instant::now() + WAIT;
    while alive(pid) {
        assert!(Instant::now() < deadline, "the shell outlived tower");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_shell_on_a_stopped_row_does_not_wake_it() {
    let mut tower = Tower::start(Options {
        live_only: false,
        ..Options::default()
    });
    tower.wait_screen("Stopped");
    // Its group starts folded: a click opens it, on the fake.
    let row = {
        let text = tower.screen_text();
        text.lines().position(|l| l.contains("Stopped")).unwrap() as u16 + 1
    };
    tower.send(&mouse(0, 5, row, 'M'));
    tower.send(&mouse(0, 5, row, 'm'));
    tower.wait_screen("gamma-stopped");
    let text = tower.screen_text();
    let row = text
        .lines()
        .position(|l| l.contains("gamma-stopped"))
        .unwrap() as u16
        + 1;
    tower.send(&mouse(0, 5, row, 'M'));
    tower.send(&mouse(0, 5, row, 'm'));
    tower.wait_screen("Enter wakes it here");
    tower.send(b"!");
    tower.wait_screen("shell  ~/demo");
    let pid = shell_pid(&mut tower);
    assert!(alive(pid));
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty(), "nothing wakes it");
    let calls = tower.fake.all_argv();
    assert!(
        !calls
            .iter()
            .any(|argv| argv.iter().any(|a| a == "--resume")),
        "{calls:?}"
    );
    tower.send(b"exit\r");
    tower.wait_for("the shell is gone", |_| !alive(pid));
    tower.wait_screen("Enter wakes it here");
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty());
}

#[test]
fn a_shell_in_a_gone_folder_starts_nothing_and_says_so() {
    let mut tower = Tower::start(Options::default());
    tower.wait_screen("alpha-task");
    std::fs::remove_dir_all(tower.tower_dir().join("demo")).unwrap();
    tower.send(b"j");
    tower.send(b"!");
    tower.wait_screen("no longer exists; no shell started");
    tower.wait_screen(" LIST ");
    assert!(!tower.screen_text().contains("shell  ~/demo"));
}

#[test]
fn below_the_width_the_shell_stays_the_foreground_hand_over() {
    let mut tower = Tower::start(Options {
        rows: 40,
        cols: 90,
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.send(b"!");
    tower.wait_for("handed over", |t| t.wrote(LEFT_THE_SCREEN));
    tower.wait_screen("tower: shell in ~/demo");
    tower.send(b"exit\r");
    tower.wait_screen("back from the shell");
}

#[test]
fn a_burst_of_the_wheel_ends_on_one_client_for_the_row_it_stopped_on() {
    let mut tower = Tower::start(Options {
        more_rows: 58,
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    tower.wait_screen("▼");
    let burst: Vec<u8> = (0..30).flat_map(|_| mouse(65, 5, 10, 'M')).collect();
    tower.send(&burst);
    let settled = |t: &Tower| t.attaches().last().is_some_and(|(_, pid)| alive(*pid));
    tower.wait_for("an attach", settled);
    thread::sleep(PAUSE);
    // Load decides how many starts the burst allows; the rest is the point:
    // far fewer than the notches, and one left, for the selected card.
    let attaches = tower.attaches();
    assert!(attaches.len() < 30, "{attaches:?}");
    let live: Vec<_> = attaches.iter().filter(|(_, pid)| alive(*pid)).collect();
    assert_eq!(live.len(), 1, "{attaches:?}");
    let (id, _) = live[0];
    let text = tower.screen_text();
    let k: usize = id.trim_start_matches('f').parse().unwrap();
    assert!(text.contains(&format!("┏━ ▪ row-{k:02}")), "{text}");
    tower.wait_for("no zombie", |t| t.zombies().is_empty());
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
    assert!(!tower.wrote(b"\x1b]52;"), "tower selects nothing here");
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

// ---- the pane's selection, the shell's history and zoom (doc/0045) ----------------

/// Where `text` shows on the screen, row and column from 0, in cells.
fn find(tower: &Tower, text: &str) -> Option<(u16, u16)> {
    let parser = tower.screen.lock().unwrap();
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    (0..rows).find_map(|r| {
        // The row's text, and the cell each byte of it came from.
        let (mut line, mut cell_of) = (String::new(), Vec::new());
        for c in 0..cols {
            let cell = screen.cell(r, c).unwrap();
            if cell.is_wide_continuation() {
                continue;
            }
            let shown = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            };
            line.push_str(shown);
            cell_of.extend(std::iter::repeat_n(c, shown.len()));
        }
        line.find(text).map(|byte| (r, cell_of[byte]))
    })
}

/// Waits for `text` on the screen, and where it shows.
fn wait_found(tower: &Tower, text: &str) -> (u16, u16) {
    tower.wait_for(&format!("`{text}` on the screen"), |t| {
        find(t, text).is_some()
    });
    find(tower, text).unwrap()
}

/// A press, a drag and a release on the screen's cells, from 0.
fn drag_cells(tower: &mut Tower, from: (u16, u16), to: (u16, u16)) {
    tower.send(&mouse(0, from.1 + 1, from.0 + 1, 'M'));
    tower.send(&mouse(32, to.1 + 1, to.0 + 1, 'M'));
    tower.send(&mouse(0, to.1 + 1, to.0 + 1, 'm'));
}

fn wait_wrote(tower: &Tower, what: &[u8]) {
    tower.wait_for(
        &format!("tower writes {:?}", String::from_utf8_lossy(what)),
        |t| t.wrote(what),
    );
}

fn background(tower: &Tower, row: u16, col: u16) -> vt100::Color {
    let parser = tower.screen.lock().unwrap();
    parser.screen().cell(row, col).unwrap().bgcolor()
}

/// The size of the terminal a process has as its stdin, rows and columns.
fn size_of(pid: i32) -> Option<(u16, u16)> {
    let tty = File::open(format!("/proc/{pid}/fd/0")).ok()?;
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: a valid winsize out-pointer on an open descriptor.
    let rc = unsafe { libc::ioctl(tty.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
    (rc == 0).then_some((size.ws_row, size.ws_col))
}

const F2: &[u8] = b"\x1bOQ";
const SHIFT_PAGE_UP: &[u8] = b"\x1b[5;2~";
const WHEEL_UP: u8 = 64;

/// The shell in the pane with the keys, with one line printed that a test can
/// find; the shell's pid.
fn shell_printing(tower: &mut Tower, line: &str) -> i32 {
    let pid = shell_terminal(tower);
    // The command line shows the octal form, so only the output has the text.
    let typed: String = line
        .chars()
        .map(|c| format!("\\{:03o}", c as u32))
        .collect();
    tower.send(format!("printf '{typed}\\n'\r").as_bytes());
    wait_found(tower, line);
    pid
}

#[test]
fn a_drag_in_the_shell_is_highlighted_and_copied_with_osc_52_only() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_printing(&mut tower, "hello selection");
    let (row, col) = find(&tower, "hello selection").unwrap();
    let plain = background(&tower, row, col);
    drag_cells(&mut tower, (row, col + 6), (row, col + 14));
    // base64 of `selection`, exactly.
    wait_wrote(&tower, b"\x1b]52;c;c2VsZWN0aW9u\x1b\\");
    tower.wait_screen("copied 9 characters");
    tower.wait_for("the highlight", |t| {
        background(t, row, col + 6) != plain && background(t, row, col + 14) != plain
    });
    assert_eq!(background(&tower, row, col + 5), plain, "before it");
    assert_eq!(background(&tower, row, col + 15), plain, "after it");
    // `Esc` ends it and the shell gets nothing.
    tower.send(b"\x1b");
    tower.wait_for("the highlight goes", |t| {
        background(t, row, col + 6) == plain
    });
    // A click copies nothing more.
    let writes = {
        let raw = tower.raw.lock().unwrap();
        raw.windows(5).filter(|w| *w == b"\x1b]52;").count()
    };
    tower.send(&mouse(0, col + 1, row + 1, 'M'));
    tower.send(&mouse(0, col + 1, row + 1, 'm'));
    tower.send(b"echo done$((1+1))\r");
    tower.wait_screen("done2");
    let raw = tower.raw.lock().unwrap();
    assert_eq!(raw.windows(5).filter(|w| *w == b"\x1b]52;").count(), writes);
}

#[test]
fn a_drag_from_the_bar_selects_nothing_and_a_drag_inside_stays_inside() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_printing(&mut tower, "inside text");
    let (row, col) = find(&tower, "inside text").unwrap();
    // Pressed inside and let go over the bar: from the pane's first column.
    drag_cells(&mut tower, (row, col + 5), (row, 2));
    wait_wrote(&tower, b"\x1b]52;c;aW5zaWRl\x1b\\");
    assert_eq!(col, PANE_X - 1, "the output starts the pane's row");
    tower.wait_screen(" TERMINAL ");
    // Pressed on the bar: the keys go to the list, and the rest does nothing.
    let from = tower.raw_len();
    drag_cells(&mut tower, (row, 2), (row, col + 5));
    tower.wait_screen(" LIST ");
    thread::sleep(PAUSE);
    assert!(!tower.wrote_since(from, b"\x1b]52;"));
}

#[test]
fn wide_characters_and_a_wrapped_line_copy_as_they_read() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_terminal(&mut tower);
    // `日本語` as UTF-8, and 140 `x` in a pane 130 wide.
    tower.send(
        b"printf 'A\\346\\227\\245\\346\\234\\254\\350\\252\\236B\\n'; \
          i=0; while [ $i -lt 140 ]; do printf x; i=$((i+1)); done; printf '\\n'\r",
    );
    let (row, col) = wait_found(&tower, "A日本語B");
    // The harness counts cells: from the second half of `日` to the first of `語`.
    drag_cells(&mut tower, (row, col + 2), (row, col + 5));
    // base64 of `日本語`.
    wait_wrote(&tower, b"\x1b]52;c;5pel5pys6Kqe\x1b\\");
    tower.wait_screen("copied 3 characters");

    let long = (row + 1, PANE_X - 1);
    drag_cells(&mut tower, long, (row + 2, PANE_X - 1 + 9));
    tower.wait_screen("copied 140 characters");
    assert!(!tower.wrote(b"\x1b]52;c;eA==\x1b\\"));
}

#[test]
fn a_program_asking_for_the_mouse_gets_it_and_tower_selects_nothing() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_printing(&mut tower, "text under a program");
    let mice = tower.tower_dir().join("mice");
    tower.send(
        format!(
            "stty raw -echo; printf '\\033[?1002;1006h\\033[1;1HMOUSE ON\\n'; exec cat > '{}'\r",
            mice.display()
        )
        .as_bytes(),
    );
    wait_found(&tower, "MOUSE ON");
    let (row, col) = find(&tower, "MOUSE ON").unwrap();
    drag_cells(&mut tower, (row, col), (row, col + 4));
    tower.send(&mouse(WHEEL_UP, col + 1, row + 1, 'M'));
    tower.send(b"!");
    tower.wait_for("the program reads every event", |_| {
        let got = std::fs::read(&mice).unwrap_or_default();
        contains(&got, b"\x1b[<0;1;1M")
            && contains(&got, b"\x1b[<32;5;1M")
            && contains(&got, b"\x1b[<0;5;1m")
            && contains(&got, b"\x1b[<64;1;1M")
            && contains(&got, b"!")
    });
    assert!(!tower.wrote(b"\x1b]52;"));
    assert!(!tower.screen_text().contains("copied"));
    assert!(!tower.screen_text().contains('↑'));
}

#[test]
fn the_shell_scrolls_by_wheel_and_keys_and_a_key_returns_to_the_bottom() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_terminal(&mut tower);
    tower.send(b"seq 1 300; printf 'th\\145 end\\n'\r");
    wait_found(&tower, "the end");
    assert!(find(&tower, "the end").is_some());
    // The wheel: three lines a notch, and the title says so.
    tower.send(&mouse(WHEEL_UP, PANE_X + 3, PANE_Y + 3, 'M'));
    tower.wait_screen("↑ 3 lines");
    tower.send(SHIFT_PAGE_UP);
    tower.wait_screen("↑ 39 lines");
    tower.send(SHIFT_PAGE_UP);
    tower.send(SHIFT_PAGE_UP);
    tower.wait_screen("↑ 111 lines");
    assert!(find(&tower, "the end").is_none(), "up in the history");
    // A selection there copies what shows: the top line of the view.
    let top = (PANE_Y - 1, PANE_X - 1);
    let shown: String = {
        let parser = tower.screen.lock().unwrap();
        (0..3)
            .filter_map(|c| {
                let cell = parser.screen().cell(top.0, top.1 + c).unwrap();
                cell.has_contents().then(|| cell.contents().to_owned())
            })
            .collect()
    };
    let at_top: usize = shown.trim().parse().unwrap();
    assert!((100..300).contains(&at_top), "{shown}");
    tower.send(&mouse(0, top.1 + 1, top.0 + 1, 'M'));
    tower.send(&mouse(32, top.1 + 3, top.0 + 1, 'M'));
    tower.send(&mouse(0, top.1 + 3, top.0 + 1, 'm'));
    let want = watchtower::embed::clipboard_write(&at_top.to_string()).unwrap();
    wait_wrote(&tower, &want);
    // `Shift+PgDn` comes back, and the key goes nowhere.
    tower.send(b"\x1b[6;2~");
    tower.wait_screen("↑ 75 lines");
    // A key the program gets: at the bottom, no marker.
    tower.send(b"echo typed$((40+2))\r");
    tower.wait_screen("typed42");
    tower.wait_for("the marker goes", |t| !t.screen_text().contains('↑'));
}

#[test]
fn new_output_does_not_move_the_view_while_scrolled_up() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_terminal(&mut tower);
    let flag = tower.tower_dir().join("flag");
    tower.send(
        format!(
            "(while [ ! -e '{}' ]; do sleep 0.05; done; seq 1000 1009) & seq 1 100; printf 'r\\145ady\\n'\r",
            flag.display()
        )
        .as_bytes(),
    );
    wait_found(&tower, "ready");
    tower.send(SHIFT_PAGE_UP);
    tower.wait_screen("↑ 36 lines");
    let row_text = |t: &Tower| -> Vec<String> {
        t.screen_text()
            .lines()
            .skip(1)
            .take(36)
            .map(|l| l.chars().skip(usize::from(PANE_X)).take(10).collect())
            .collect()
    };
    let before = row_text(&tower);
    std::fs::write(&flag, "").unwrap();
    tower.wait_screen("↑ 46 lines");
    assert_eq!(row_text(&tower), before, "the same lines at the same place");
    assert!(!tower.screen_text().contains("1009"));
}

#[test]
fn the_history_stops_at_its_limit_and_stays_small() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_terminal(&mut tower);
    tower.send(b"seq 1 15000; printf 'th\\145 end\\n'\r");
    wait_found(&tower, "the end");
    // More pages than the limit has.
    for _ in 0..300 {
        tower.send(SHIFT_PAGE_UP);
    }
    tower.wait_screen("↑ 10000 lines");
    tower.send(SHIFT_PAGE_UP);
    tower.send(b"\x1b[6;2~");
    tower.wait_screen("↑ 9964 lines");
    // At the pane's width a line is about 4 KiB: what the limit promises.
    let status = std::fs::read_to_string(format!("/proc/{}/status", tower.child.id())).unwrap();
    let rss_kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap();
    assert!(rss_kb < 150 * 1024, "{rss_kb} kB");
}

#[test]
fn zoom_gives_the_client_the_whole_window_and_returns() {
    let (mut tower, id, pid) = attached(Options {
        on_attach: b"CLAUDE SCREEN\r\n".to_vec(),
        ..Options::default()
    });
    tower.wait_screen("CLAUDE SCREEN");
    tower.wait_for("the pane's size", |_| size_of(pid) == Some((37, 130)));
    // `z` from the list: the whole window but the footer, with the keys.
    tower.send(b"z");
    tower.wait_screen(" ZOOM ");
    tower.wait_for("the first column", |t| {
        t.screen_text()
            .lines()
            .next()
            .is_some_and(|l| l.starts_with("ATTACHED"))
    });
    tower.wait_for("the new size", |_| size_of(pid) == Some((39, 160)));
    assert!(!tower.screen_text().contains("alpha-task"));
    // Keys are the child's now, `z` and list letters too; `F2` is not.
    tower.send(b"zqn");
    tower.wait_keys(&id, b"zqn");
    tower.send(F2);
    tower.wait_screen(" TERMINAL ");
    tower.wait_for("the pane's size again", |_| size_of(pid) == Some((37, 130)));
    tower.send(b"after");
    tower.wait_keys(&id, b"after");
    assert!(
        !contains(&tower.keys_of(&id), b"\x1bOQ"),
        "F2 stays tower's"
    );
    // From the terminal, and back to the list with `Ctrl+Z`.
    tower.send(F2);
    tower.wait_screen(" ZOOM ");
    tower.wait_for("the new size", |_| size_of(pid) == Some((39, 160)));
    tower.send(b"\x1a");
    tower.wait_screen(" LIST ");
    tower.wait_for("the pane's size again", |_| size_of(pid) == Some((37, 130)));
    // `F2` from the list, and back with `F12`.
    tower.send(F2);
    tower.wait_screen(" ZOOM ");
    tower.send(b"\x1b[24~");
    tower.wait_screen(" LIST ");
    assert_eq!(tower.attaches().len(), 1, "one client all along");
}

#[test]
fn zoom_works_for_the_shell_with_its_size_and_its_selection() {
    let (mut tower, _id, _client) = attached(Options::default());
    shell_terminal(&mut tower);
    tower.send(F2);
    tower.wait_screen(" ZOOM ");
    tower.send(b"stty size\r");
    tower.wait_screen("39 160");
    let (row, col) = find(&tower, "39 160").unwrap();
    assert_eq!(col, 0, "from the window's first column");
    drag_cells(&mut tower, (row, 0), (row, 1));
    wait_wrote(&tower, b"\x1b]52;c;Mzk=\x1b\\");
    tower.wait_screen("copied 2 characters");
    // The wheel, with the marker on the footer.
    tower.send(b"seq 1 100; printf 'th\\145 end\\n'\r");
    wait_found(&tower, "the end");
    tower.send(&mouse(WHEEL_UP, 5, 5, 'M'));
    tower.wait_for("the marker on the footer", |t| {
        t.screen_text()
            .lines()
            .last()
            .is_some_and(|l| l.contains("↑ 3 lines"))
    });
    tower.send(b"\x1b[24~");
    tower.wait_screen(" LIST ");
    tower.send(b"\r");
    tower.wait_screen(" TERMINAL ");
    tower.send(b"stty size\r");
    tower.wait_screen("37 130");
}

#[test]
fn zoom_ends_with_the_shell() {
    let (mut tower, _id, _client) = attached(Options::default());
    let pid = shell_terminal(&mut tower);
    tower.send(F2);
    tower.wait_screen(" ZOOM ");
    tower.send(b"exit\r");
    tower.wait_for("the shell is gone", |_| !alive(pid));
    tower.wait_screen(" LIST ");
    assert!(!tower.screen_text().contains(" ZOOM "));
}

#[test]
fn below_the_width_zoom_opens_the_terminal_in_the_whole_window() {
    let mut tower = Tower::start(Options {
        rows: 20,
        cols: 80,
        ..Options::default()
    });
    tower.wait_screen("alpha-task");
    thread::sleep(PAUSE);
    assert!(tower.attaches().is_empty());
    tower.send(b"j");
    tower.send(b"z");
    tower.wait_screen(" ZOOM ");
    let (_, pid) = tower.wait_attaches(1);
    tower.wait_for("the window's size", |_| size_of(pid) == Some((19, 80)));
    tower.send(F2);
    tower.wait_for("the dashboard", |t| !t.screen_text().contains(" ZOOM "));
    tower.wait_for("the client is closed", |_| !alive(pid));
}
