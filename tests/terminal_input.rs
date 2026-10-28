//! Terminal input without a TTY (doc/0005): a terminal whose input hangs up,
//! fails or reaches EOF ends tower as SIGHUP would, and every input-waiting
//! loop checks the termination flag and never spins. Pipes stand in for the
//! terminal; `poll(2)` reports a pipe whose writer is gone with `POLLHUP`,
//! as it does for a hung-up terminal.

use std::cell::Cell;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use signal_hook::consts::{SIGHUP, SIGTERM};
use watchtower::app::DispatchJob;
use watchtower::attach::Termination;
use watchtower::cli::CliError;
use watchtower::config::Account;
use watchtower::dispatch::{DispatchError, DispatchRequest};
use watchtower::jobs::JobResult;
use watchtower::tui::{
    HungUp, Input, MODE_RESETS, REARM_LIMIT, ReadyEvents, ReadyInput, polls_after, read_ready,
    take_input, wait_input, wait_key,
};

fn pipe() -> (OwnedFd, std::fs::File) {
    let (read, write) = std::io::pipe().unwrap();
    (
        OwnedFd::from(read),
        std::fs::File::from(OwnedFd::from(write)),
    )
}

#[test]
fn wait_input_sees_idle_ready_and_hang_up() {
    use std::io::Write;
    let (read, mut write) = pipe();
    let fd = read.as_raw_fd();

    let start = Instant::now();
    assert_eq!(wait_input(fd, Duration::from_millis(50)), Input::Idle);
    assert!(start.elapsed() >= Duration::from_millis(40), "it waited");

    write.write_all(b"x").unwrap();
    assert_eq!(wait_input(fd, Duration::from_secs(1)), Input::Ready);

    // Writer gone: a hang-up, reported at once rather than after the timeout.
    drop(write);
    let start = Instant::now();
    assert_eq!(wait_input(fd, Duration::from_secs(5)), Input::HangUp);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn wait_input_on_an_invalid_descriptor_is_a_hang_up() {
    assert_eq!(wait_input(1_000_000, Duration::ZERO), Input::HangUp);
}

#[test]
fn a_hang_up_is_recorded_as_sighup() {
    let termination = Termination::default();
    assert_eq!(take_input(&termination, Ok(Vec::new())), Some(Vec::new()));
    assert_eq!(termination.pending(), None);
    assert_eq!(take_input(&termination, Err(HungUp)), None);
    assert_eq!(termination.pending(), Some(SIGHUP));
}

fn press(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

#[test]
fn key_wait_ends_on_a_press_but_not_on_a_release() {
    let termination = Termination::default();
    let mut release = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    let calls = Cell::new(0);
    let result = wait_key(&termination, |_| {
        calls.set(calls.get() + 1);
        Ok(match calls.get() {
            1 => vec![Event::Key(release), Event::Resize(80, 24)],
            _ => vec![press('x')],
        })
    });
    assert_eq!(result, Ok(()));
    assert_eq!(calls.get(), 2);
    assert_eq!(termination.pending(), None);
}

#[test]
fn key_wait_ends_on_termination() {
    let termination = Termination::default();
    let calls = Cell::new(0);
    let result = wait_key(&termination, |timeout| {
        assert!(timeout > Duration::ZERO, "each turn waits");
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            termination.request(SIGTERM);
        }
        Ok(Vec::new())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(calls.get(), 3);
}

#[test]
fn key_wait_ends_on_a_hang_up_without_spinning() {
    // The real input source over a pipe whose writer is gone, as a terminal
    // after `tmux kill-server`: the wait must end at the first turn.
    let (read, write) = pipe();
    drop(write);
    let fd = read.as_raw_fd();
    let termination = Termination::default();
    let calls = Cell::new(0);
    let start = Instant::now();
    let result = wait_key(&termination, |timeout| {
        calls.set(calls.get() + 1);
        match wait_input(fd, timeout) {
            Input::HangUp => Err(HungUp),
            _ => Ok(Vec::new()),
        }
    });
    assert_eq!(result, Err(HungUp));
    assert_eq!(calls.get(), 1);
    assert_eq!(termination.pending(), Some(SIGHUP));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn mode_resets_switch_off_mouse_paste_and_keyboard_modes() {
    for seq in [
        "\x1b[?1000l",
        "\x1b[?1002l",
        "\x1b[?1003l",
        "\x1b[?1006l",
        "\x1b[?2004l",
        "\x1b[<u",
    ] {
        assert!(MODE_RESETS.contains(seq), "{seq:?}");
    }
    assert!(!MODE_RESETS.contains('\x07'), "no BEL (doc/0007)");
}

fn dispatch_job() -> DispatchJob {
    let account = Account::new("main", "claude", None, Path::new("/home/user")).unwrap();
    DispatchJob {
        request: DispatchRequest {
            project: "/home/user/workspace".into(),
            task: "reply OK".into(),
            name: None,
            account,
            model: None,
            effort: None,
            permission_mode: "auto".into(),
            settings: None,
            end_options: false,
        },
        attach: false,
        after_trust: false,
    }
}

#[test]
fn a_dispatch_timeout_polls_at_once() {
    let timeout = JobResult::Dispatch(
        dispatch_job(),
        Err(DispatchError::Cli(CliError::Timeout {
            program: "claude".into(),
            timeout: Duration::from_secs(30),
            stderr: String::new(),
        })),
    );
    assert!(polls_after(&timeout));
    let failed = JobResult::Dispatch(
        dispatch_job(),
        Err(DispatchError::Failed {
            code: Some(2),
            stderr: "error".into(),
        }),
    );
    assert!(!polls_after(&failed));
}

/// crossterm's edge-triggered reader as `read_ready` sees it: input waiting
/// on the terminal, whether crossterm has been told about it (an edge),
/// and a pending resize. A read with both an edge and a resize returns the
/// resize only and uses the edge up, leaving the input unread, as
/// crossterm's `epoll` source does.
#[derive(Default)]
struct EdgeReader {
    waiting: Vec<Event>,
    edge: bool,
    resize: bool,
    reads: usize,
    rearms: usize,
    /// A re-arm gives no new edge (input crossterm never reads).
    deaf: bool,
}

impl EdgeReader {
    fn typed(&mut self, keys: &str) {
        self.waiting.extend(keys.chars().map(press));
        self.edge = true;
    }
}

impl ReadyInput for EdgeReader {
    fn waiting(&mut self) -> bool {
        !self.waiting.is_empty()
    }

    fn read(&mut self) -> Result<Vec<Event>, HungUp> {
        self.reads += 1;
        if !self.edge {
            return Ok(Vec::new());
        }
        self.edge = false;
        if std::mem::take(&mut self.resize) {
            return Ok(vec![Event::Resize(110, 36)]);
        }
        Ok(std::mem::take(&mut self.waiting))
    }

    fn rearm(&mut self) {
        self.rearms += 1;
        if !self.deaf && !self.waiting.is_empty() {
            self.edge = true;
        }
    }
}

#[test]
fn a_key_waiting_behind_a_resize_is_read_in_the_same_turn() {
    // A resize while tower was idle, then `n`: crossterm reports the
    // resize and loses the key's readiness; the key must still come now,
    // not after the next key press.
    let mut input = EdgeReader {
        resize: true,
        ..EdgeReader::default()
    };
    input.typed("n");
    let read = read_ready(&mut input).unwrap();
    assert_eq!(
        read,
        ReadyEvents {
            events: vec![Event::Resize(110, 36), press('n')],
            stalled: false,
        }
    );
    assert_eq!(input.rearms, 1);
    assert!(!input.waiting());
}

#[test]
fn an_idle_key_is_read_without_a_rearm() {
    let mut input = EdgeReader::default();
    input.typed("jk");
    let read = read_ready(&mut input).unwrap();
    assert_eq!(read.events, vec![press('j'), press('k')]);
    assert!(!read.stalled);
    assert_eq!((input.reads, input.rearms), (1, 0));
}

#[test]
fn input_crossterm_never_reads_is_reported_as_stalled_not_retried() {
    let mut input = EdgeReader {
        resize: true,
        deaf: true,
        ..EdgeReader::default()
    };
    input.typed("n");
    let read = read_ready(&mut input).unwrap();
    assert_eq!(read.events, vec![Event::Resize(110, 36)]);
    assert!(read.stalled);
    assert_eq!((input.reads, input.rearms), (2, 1));
}

/// Input that keeps arriving: every read takes one key and more is
/// waiting, as during a long paste.
struct Stream {
    reads: usize,
}

impl ReadyInput for Stream {
    fn waiting(&mut self) -> bool {
        true
    }
    fn read(&mut self) -> Result<Vec<Event>, HungUp> {
        self.reads += 1;
        Ok(vec![press('x')])
    }
    fn rearm(&mut self) {}
}

#[test]
fn a_long_paste_is_read_in_bounded_turns() {
    let mut input = Stream { reads: 0 };
    let read = read_ready(&mut input).unwrap();
    assert!(!read.stalled);
    assert_eq!(input.reads, REARM_LIMIT + 1);
    assert_eq!(read.events.len(), REARM_LIMIT + 1);
}

#[test]
fn a_hang_up_while_reading_ready_input_is_returned() {
    struct Gone;
    impl ReadyInput for Gone {
        fn waiting(&mut self) -> bool {
            true
        }
        fn read(&mut self) -> Result<Vec<Event>, HungUp> {
            Err(HungUp)
        }
        fn rearm(&mut self) {}
    }
    assert_eq!(read_ready(&mut Gone), Err(HungUp));
}
