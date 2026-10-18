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
use watchtower::tui::{HungUp, Input, MODE_RESETS, polls_after, take_input, wait_input, wait_key};

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
