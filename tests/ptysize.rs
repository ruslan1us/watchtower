//! The session terminal's real size for the logs replay (doc/0025):
//! `tty_nr` decoding and the `/dev/pts` mapping, which size source wins,
//! the app's process evidence for it, and the logs job using it.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{HOME, NoEnviron, poll};
use fake_claude::{Fake, lock};
use watchtower::app::{App, LogsTarget};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::logview::{self, DEFAULT_COLS, DEFAULT_ROWS, MAX_COLS, MAX_ROWS};
use watchtower::merge::EnvironReader;
use watchtower::ptysize::{
    TerminalReader, TtyProbe, decode_dev, parse_tty_nr, pts_number, session_size,
};
use watchtower::sidecar::Sidecar;

/// The kernel's 32-bit encoding of `major:minor` (`new_encode_dev`).
fn encode(major: u32, minor: u32) -> u32 {
    (minor & 0xff) | (major << 8) | ((minor & !0xff) << 12)
}

/// A `/proc/<pid>/stat` line with `tty_nr` (field 7) and start ticks
/// (field 22); the name holds spaces and parentheses, as it may.
fn stat_line(pid: u32, tty_nr: i64, ticks: u64) -> String {
    let mut fields: Vec<String> = vec!["0".to_owned(); 50];
    fields[0] = "S".to_owned();
    fields[4] = tty_nr.to_string();
    fields[19] = ticks.to_string();
    format!("{pid} (claude (x) y) {}\n", fields.join(" "))
}

// ---- decoding ----------------------------------------------------------------

#[test]
fn tty_nr_is_field_seven_after_the_name() {
    assert_eq!(parse_tty_nr(&stat_line(42, 34819, 9)), Some(34819));
    assert_eq!(parse_tty_nr(&stat_line(42, 0, 9)), Some(0));
    // Printed as a signed int: a set top bit comes out negative.
    assert_eq!(
        parse_tty_nr(&stat_line(42, -2_147_000_000, 9)),
        Some(2_147_967_296)
    );
    assert_eq!(parse_tty_nr(&stat_line(42, 1 << 33, 9)), None);
    assert_eq!(parse_tty_nr("42 (claude) S 1 2 3"), None);
    assert_eq!(parse_tty_nr("no parenthesis"), None);
    assert_eq!(parse_tty_nr("42 (claude) S 1 2 3 pts 5"), None);
}

#[test]
fn device_numbers_decode_to_major_and_minor() {
    // /dev/pts/3, as read from a real session's stat.
    assert_eq!(decode_dev(34819), (136, 3));
    for (major, minor) in [
        (136, 0),
        (136, 255),
        (136, 256),
        (136, 300),
        (136, 1_048_575),
        (4, 1),
    ] {
        assert_eq!(decode_dev(encode(major, minor)), (major, minor));
    }
    assert_eq!(decode_dev(0), (0, 0));
}

#[test]
fn only_unix98_pty_slaves_map_to_dev_pts() {
    assert_eq!(pts_number(136, 3), Some(3));
    // A current kernel keeps the whole index in the minor of major 136.
    assert_eq!(pts_number(136, 256), Some(256));
    assert_eq!(pts_number(136, 300), Some(300));
    assert_eq!(pts_number(136, 1_048_575), Some(1_048_575));
    // The old 8-bit-minor layout spread indices over majors 137–143.
    assert_eq!(pts_number(137, 44), Some(300));
    assert_eq!(pts_number(143, 255), Some(7 * 256 + 255));
    // ... with minors below 256 only.
    assert_eq!(pts_number(137, 256), None);
    // No terminal, virtual consoles, /dev/tty, BSD ptys, PTY masters.
    for (major, minor) in [(0, 0), (4, 1), (5, 0), (3, 0), (128, 3), (135, 0), (144, 0)] {
        assert_eq!(pts_number(major, minor), None, "{major}:{minor}");
    }
}

// ---- which size wins -----------------------------------------------------------

/// A [`TerminalReader`] with canned answers that records which device was
/// asked for.
struct FakeTerminal {
    stat: Option<String>,
    size: io::Result<(u16, u16)>,
    asked: Mutex<Vec<(u32, u32, u32)>>,
}

impl FakeTerminal {
    fn new(stat: Option<String>, size: io::Result<(u16, u16)>) -> FakeTerminal {
        FakeTerminal {
            stat,
            size,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<(u32, u32, u32)> {
        self.asked.lock().unwrap().clone()
    }
}

impl TerminalReader for FakeTerminal {
    fn stat(&self, _pid: u32) -> io::Result<String> {
        self.stat
            .clone()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn pts_size(&self, pts: u32, major: u32, minor: u32) -> io::Result<(u16, u16)> {
        self.asked.lock().unwrap().push((pts, major, minor));
        match &self.size {
            Ok(size) => Ok(*size),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        }
    }
}

const PROBE: TtyProbe = TtyProbe {
    pid: 4290,
    start_ticks: 123_456,
};

#[test]
fn the_real_size_is_read_from_the_session_pts() {
    let t = FakeTerminal::new(Some(stat_line(4290, 34819, 123_456)), Ok((57, 231)));
    assert_eq!(session_size(&t, PROBE), Some((57, 231)));
    assert_eq!(t.asked(), [(3, 136, 3)]);

    let t = FakeTerminal::new(
        Some(stat_line(4290, i64::from(encode(136, 300)), 123_456)),
        Ok((50, 200)),
    );
    assert_eq!(session_size(&t, PROBE), Some((50, 200)));
    assert_eq!(t.asked(), [(300, 136, 300)]);
}

#[test]
fn no_size_without_solid_evidence() {
    let good = || Some(stat_line(4290, 34819, 123_456));
    let cases = [
        ("unreadable stat", FakeTerminal::new(None, Ok((57, 231)))),
        (
            "guard fails: other start ticks (reused pid)",
            FakeTerminal::new(Some(stat_line(4290, 34819, 999)), Ok((57, 231))),
        ),
        (
            "no terminal",
            FakeTerminal::new(Some(stat_line(4290, 0, 123_456)), Ok((57, 231))),
        ),
        (
            "a virtual console, not a pts",
            FakeTerminal::new(
                Some(stat_line(4290, i64::from(encode(4, 1)), 123_456)),
                Ok((57, 231)),
            ),
        ),
        (
            "device unreadable or failing the checks",
            FakeTerminal::new(good(), Err(io::Error::from(io::ErrorKind::InvalidData))),
        ),
        ("zero rows", FakeTerminal::new(good(), Ok((0, 231)))),
        ("zero columns", FakeTerminal::new(good(), Ok((57, 0)))),
        (
            "garbled stat",
            FakeTerminal::new(Some("4290 (claude) S".to_owned()), Ok((57, 231))),
        ),
    ];
    for (what, t) in cases {
        assert_eq!(session_size(&t, PROBE), None, "{what}");
    }
}

#[test]
fn the_real_size_beats_the_inference_and_is_capped() {
    // The stream addresses at most row 57, column 24 (as in the bug).
    let bytes = b"\x1b[2J\x1b[H\x1b[57;24Hx";
    assert_eq!(logview::infer_size(bytes), (DEFAULT_ROWS, DEFAULT_COLS));
    assert_eq!(logview::emulator_size(bytes, Some((57, 231))), (57, 231));
    assert_eq!(logview::emulator_size(bytes, Some((20, 80))), (20, 80));
    assert_eq!(
        logview::emulator_size(bytes, Some((1000, 1000))),
        (MAX_ROWS, MAX_COLS)
    );
    // Unknown or zero: the inference, unchanged.
    for real in [None, Some((0, 231)), Some((57, 0))] {
        assert_eq!(
            logview::emulator_size(bytes, real),
            logview::infer_size(bytes)
        );
    }
    assert_eq!(
        logview::render_sized(bytes, Some((57, 231))).size(),
        (57, 231)
    );
    assert_eq!(logview::render_sized(bytes, None), logview::render(bytes));
}

// ---- the app's process evidence ------------------------------------------------

/// Every process looks like a Claude Code worker of `main` (`~/.claude`),
/// started at `10_000 + pid` ticks.
struct ClaudeProcesses;

impl EnvironReader for ClaudeProcesses {
    fn config_dir_var(&self, _pid: u32) -> io::Result<Option<OsString>> {
        Ok(None)
    }

    fn cmdline(&self, _pid: u32) -> io::Result<Vec<u8>> {
        Ok(b"claude\0--bg-worker\0".to_vec())
    }

    fn exe(&self, _pid: u32) -> io::Result<PathBuf> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }

    fn start_ticks(&self, pid: u32) -> io::Result<u64> {
        Ok(10_000 + u64::from(pid))
    }
}

fn app_with(environ: Box<dyn EnvironReader>) -> App {
    let home = Path::new(HOME);
    App::new(
        Config::defaults(home),
        home.into(),
        Sidecar::default(),
        environ,
    )
}

fn target(app: &App, id: &str) -> LogsTarget {
    LogsTarget {
        account: app.config().accounts[0].clone(),
        id: id.into(),
    }
}

#[test]
fn the_probe_carries_the_pid_the_guard_accepted() {
    let mut app = app_with(Box::new(ClaudeProcesses));
    app.apply_poll(poll("main", "after-stop-all.json"));
    assert_eq!(
        app.logs_probe(&target(&app, "7b4d9f02")),
        Some(TtyProbe {
            pid: 4290,
            start_ticks: 14_290
        })
    );
    // A stopped row has no pid; an unknown id has no row.
    assert_eq!(app.logs_probe(&target(&app, "b2f8d064")), None);
    assert_eq!(app.logs_probe(&target(&app, "00000000")), None);
    // Another account's target is not this row's owner.
    let mut other = target(&app, "7b4d9f02");
    other.account.name = "alt".into();
    assert_eq!(app.logs_probe(&other), None);
}

#[test]
fn no_probe_when_the_guard_did_not_accept_the_process() {
    // Unreadable processes: the guard never accepts them.
    let mut app = app_with(Box::new(NoEnviron));
    app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(app.logs_probe(&target(&app, "7b4d9f02")), None);
}

// ---- the logs job --------------------------------------------------------------

fn fetch_with(terminal: FakeTerminal, probe: Option<TtyProbe>) -> (u16, u16) {
    let _g = lock();
    // Addresses only row 1, column 1: the inference gives the default.
    let claude = Fake::new("claude", "printf '\\033[2J\\033[Hhello'");
    let jobs = Jobs::new(CliRunner::new().with_timeout(Duration::from_secs(20)))
        .with_terminal(Arc::new(terminal));
    let target = LogsTarget {
        account: claude.account(),
        id: "7b4d9f02".into(),
    };
    jobs.logs(target, probe);
    let Some(JobResult::Logs(_, Ok(screen))) = jobs.recv_timeout(Duration::from_secs(20)) else {
        panic!("expected a replayed screen");
    };
    assert_eq!(screen.plain_lines(), ["hello"]);
    screen.size()
}

#[test]
fn the_logs_job_replays_at_the_real_size() {
    let t = FakeTerminal::new(Some(stat_line(4290, 34819, 123_456)), Ok((57, 231)));
    assert_eq!(fetch_with(t, Some(PROBE)), (57, 231));
}

#[test]
fn the_logs_job_falls_back_to_the_inference() {
    let size = (DEFAULT_ROWS, DEFAULT_COLS);
    let unreadable = || FakeTerminal::new(None, Ok((57, 231)));
    assert_eq!(fetch_with(unreadable(), Some(PROBE)), size);
    let readable = || FakeTerminal::new(Some(stat_line(4290, 34819, 123_456)), Ok((57, 231)));
    assert_eq!(fetch_with(readable(), None), size, "no probe, no read");
}
