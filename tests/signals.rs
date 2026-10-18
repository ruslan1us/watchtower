//! SIGTERM and SIGHUP end tower gracefully (doc/0005): the handlers only set
//! a flag, the event loop turns it into an exit with 128 + signal, and the
//! terminal is restored without writes after SIGHUP. A separate test binary,
//! so the process-wide handlers cannot affect other tests.

use std::process::Command;

use signal_hook::consts::{SIGHUP, SIGTERM};
use watchtower::attach::{TERMINATION_SIGNALS, Termination, signal_exit_status};
use watchtower::tui::{Exit, exit_for};

#[test]
fn signals_set_the_flag_instead_of_killing() {
    let termination = Termination::install().unwrap();
    assert_eq!(termination.pending(), None);
    signal_hook::low_level::raise(SIGTERM).unwrap();
    // Still running: the default action would have killed the test process.
    assert_eq!(termination.pending(), Some(SIGTERM));
    signal_hook::low_level::raise(SIGHUP).unwrap();
    assert_eq!(termination.pending(), Some(SIGHUP));
    // A second install shares the same flag.
    assert_eq!(Termination::install().unwrap().pending(), Some(SIGHUP));

    // A child started now has the default dispositions: handlers are reset
    // on exec, nothing is inherited as ignored (`SigIgn` bits clear).
    let out = Command::new("sh")
        .args(["-c", "grep '^SigIgn:' /proc/self/status"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let hex = text.split_whitespace().nth(1).expect("SigIgn line");
    let mask = u64::from_str_radix(hex, 16).unwrap();
    for sig in TERMINATION_SIGNALS {
        assert_eq!(mask & (1 << (sig - 1)), 0, "signal {sig} ignored in child");
    }
}

#[test]
fn pending_signal_decides_the_exit() {
    assert!(matches!(exit_for(Ok(()), None), Ok(Exit::Quit)));
    assert!(exit_for(Err(std::io::Error::other("draw failed")), None).is_err());
    assert!(matches!(
        exit_for(Ok(()), Some(SIGTERM)),
        Ok(Exit::Signal(SIGTERM))
    ));
    // After a hang-up the terminal fails first; the signal is the cause.
    assert!(matches!(
        exit_for(Err(std::io::Error::other("EIO")), Some(SIGHUP)),
        Ok(Exit::Signal(SIGHUP))
    ));
}

#[test]
fn exit_status_and_terminal_writes() {
    assert_eq!(Exit::Quit.status(), 0);
    assert_eq!(Exit::Signal(SIGTERM).status(), 143);
    assert_eq!(Exit::Signal(SIGHUP).status(), 129);
    assert_eq!(signal_exit_status(SIGTERM), 143);
    assert!(Exit::Quit.may_write_to_terminal());
    assert!(Exit::Signal(SIGTERM).may_write_to_terminal());
    assert!(!Exit::Signal(SIGHUP).may_write_to_terminal());
}

#[test]
fn request_records_like_the_handler() {
    let t = Termination::default();
    let shared = t.clone();
    shared.request(SIGTERM);
    assert_eq!(t.pending(), Some(SIGTERM));
}
