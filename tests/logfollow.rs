//! The log follower (doc/0030): working sessions fetched at the cadence,
//! the bounds on parallel fetches and kept streams, requests during a
//! running fetch, the attached session, dropped streams, and the panel's
//! snapshot of a stream followed in the background; with an in-process
//! source and with a fake `claude`.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;
#[path = "support/synthetic.rs"]
mod synthetic;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_claude::{Fake, lock};
use synthetic::{Session, trimmed};
use watchtower::app::LogsTarget;
use watchtower::cli::CliRunner;
use watchtower::config::Account;
use watchtower::logfollow::{FollowSettings, FollowSpec, Follower, LogSource};
use watchtower::logview::LogsError;
use watchtower::merge::ProcEnviron;

/// Sessions by id, each advanced by `step` spinner frames per fetch; ids
/// not listed are stopped. Records every call and the most fetches that
/// ran at the same time.
struct Source {
    sessions: Mutex<HashMap<String, Session>>,
    step: usize,
    delay: Duration,
    calls: Mutex<Vec<String>>,
    running: AtomicUsize,
    most: AtomicUsize,
}

impl Source {
    fn new(ids: &[&str], step: usize, delay: Duration) -> Arc<Source> {
        let sessions = ids
            .iter()
            .map(|id| (id.to_string(), Session::start(30, 100, 40)))
            .collect();
        Arc::new(Source {
            sessions: Mutex::new(sessions),
            step,
            delay,
            calls: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        })
    }

    fn calls_of(&self, id: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| *c == id)
            .count()
    }

    fn total(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn reference(&self, id: &str) -> Vec<String> {
        trimmed(&self.sessions.lock().unwrap()[id].reference())
    }
}

impl LogSource for Source {
    fn logs(&self, account: &Account, id: &str) -> Result<Vec<u8>, LogsError> {
        assert_eq!(account.name, "main");
        self.calls.lock().unwrap().push(id.to_owned());
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let out = match self.sessions.lock().unwrap().get_mut(id) {
            Some(s) => {
                s.spin(self.step);
                Ok(s.capture())
            }
            None => Err(LogsError::NotRunning),
        };
        self.running.fetch_sub(1, Ordering::SeqCst);
        out
    }
}

fn account() -> Account {
    Account::new("main", "claude", None, Path::new("/home/user")).unwrap()
}

fn spec(id: &str, working: bool) -> FollowSpec {
    FollowSpec {
        target: LogsTarget {
            account: account(),
            id: id.to_owned(),
        },
        pid: Some(4000),
        probe: None,
        working,
    }
}

fn settings(cadence_ms: u64) -> FollowSettings {
    FollowSettings {
        cadence: Duration::from_millis(cadence_ms),
        ..FollowSettings::default()
    }
}

fn start(source: &Arc<Source>, settings: FollowSettings) -> Follower {
    Follower::start(source.clone(), Arc::new(ProcEnviron), settings)
}

/// Waits up to five seconds for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn working_sessions_are_fetched_at_the_cadence() {
    let source = Source::new(&["aaaa0001", "bbbb0002"], 5, Duration::ZERO);
    let follower = start(&source, settings(100));
    follower.set_plan(vec![spec("aaaa0001", true), spec("bbbb0002", false)]);
    wait_for("four fetches", || source.calls_of("aaaa0001") >= 4);
    let started = Instant::now();
    let before = source.calls_of("aaaa0001");
    std::thread::sleep(Duration::from_millis(350));
    let during = source.calls_of("aaaa0001") - before;
    assert!(
        (1..=5).contains(&during),
        "{during} fetches in {:?} at a 100 ms cadence",
        started.elapsed()
    );
    assert_eq!(source.calls_of("bbbb0002"), 0, "not working: not fetched");
    // A working row that stops working is no longer fetched.
    follower.set_plan(vec![spec("aaaa0001", false), spec("bbbb0002", false)]);
    std::thread::sleep(Duration::from_millis(150));
    let settled = source.calls_of("aaaa0001");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(source.calls_of("aaaa0001"), settled);
    // Its stream stays, with the base its start gave it.
    let streams = follower.streams();
    assert_eq!(streams.len(), 1, "{streams:?}");
    assert_eq!(streams[0].target.id, "aaaa0001");
    assert_eq!(streams[0].has_base, Some(true));
}

#[test]
fn background_fetches_stay_within_the_limit() {
    let ids: Vec<String> = (0..8).map(|i| format!("cccc000{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let source = Source::new(&refs, 1, Duration::from_millis(60));
    let follower = start(&source, settings(50));
    follower.set_plan(refs.iter().map(|id| spec(id, true)).collect());
    wait_for("every session fetched twice", || {
        refs.iter().all(|id| source.calls_of(id) >= 2)
    });
    assert!(source.most.load(Ordering::SeqCst) <= 3, "at most 3 at once");
    assert!(source.most.load(Ordering::SeqCst) >= 2, "they do overlap");
}

#[test]
fn a_request_during_a_fetch_runs_right_after_it() {
    let source = Source::new(&["dddd0001"], 5, Duration::from_millis(150));
    let follower = start(&source, settings(10_000));
    follower.set_plan(vec![spec("dddd0001", true)]);
    wait_for("the background fetch", || source.calls_of("dddd0001") == 1);
    follower.fetch(spec("dddd0001", true), true);
    let (target, result) = follower
        .recv_timeout(Duration::from_secs(5))
        .expect("the request reports back");
    assert_eq!(target.id, "dddd0001");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        source.calls_of("dddd0001"),
        2,
        "one more fetch, after the first"
    );
    assert_eq!(source.most.load(Ordering::SeqCst), 1, "never two at once");
    // Requests count for the cadence: nothing more for a while.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(source.calls_of("dddd0001"), 2);
}

#[test]
fn the_panel_gets_the_screen_followed_in_the_background() {
    // 20 frames per fetch, a ring of 40: the start's repaint is gone from
    // the third capture on.
    let source = Source::new(&["eeee0001"], 20, Duration::ZERO);
    let follower = start(&source, settings(30));
    follower.set_plan(vec![spec("eeee0001", true)]);
    wait_for("several fetches", || source.calls_of("eeee0001") >= 6);
    follower.set_plan(vec![spec("eeee0001", false)]);
    std::thread::sleep(Duration::from_millis(100));
    follower.fetch(spec("eeee0001", false), true);
    let (_, result) = follower.recv_timeout(Duration::from_secs(5)).unwrap();
    let screen = result.unwrap();
    assert!(!screen.partial());
    assert_eq!(trimmed(&screen.plain_lines()), source.reference("eeee0001"));
    let text = screen.plain_lines().join("\n");
    assert!(text.contains("Example Code v9.9.9"), "{text}");

    // A follower that starts late has no base: the screen says so.
    let late = start(&source, settings(10_000));
    late.fetch(spec("eeee0001", false), true);
    let (_, result) = late.recv_timeout(Duration::from_secs(5)).unwrap();
    let screen = result.unwrap();
    assert!(screen.partial());
    assert!(!screen.plain_lines().join("\n").contains("Example Code"));
}

#[test]
fn the_attached_session_is_followed_whatever_its_state() {
    let source = Source::new(&["ffff0001"], 1, Duration::ZERO);
    let follower = start(&source, settings(50));
    follower.set_plan(vec![spec("ffff0001", false)]);
    follower.attached(Some(spec("ffff0001", false)));
    wait_for("fetches while attached", || {
        source.calls_of("ffff0001") >= 3
    });
    follower.attached(None);
    std::thread::sleep(Duration::from_millis(100));
    let after = source.calls_of("ffff0001");
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(source.calls_of("ffff0001"), after);
}

#[test]
fn streams_are_bounded_and_dropped_when_their_rows_go() {
    let ids: Vec<String> = (0..6).map(|i| format!("abab000{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let source = Source::new(&refs, 1, Duration::ZERO);
    let follower = start(
        &source,
        FollowSettings {
            cadence: Duration::from_secs(60),
            max_streams: 4,
            grace: Duration::from_millis(200),
            ..FollowSettings::default()
        },
    );
    follower.set_plan(refs.iter().map(|id| spec(id, false)).collect());
    for id in &refs {
        follower.fetch(spec(id, false), false);
    }
    wait_for("six fetches", || source.total() == 6);
    wait_for("the bound", || {
        let s = follower.streams();
        s.len() == 4 && s.iter().all(|i| !i.busy)
    });
    // The last ones in the plan went first.
    let kept: Vec<String> = follower
        .streams()
        .into_iter()
        .map(|s| s.target.id)
        .collect();
    assert_eq!(kept, ["abab0000", "abab0001", "abab0002", "abab0003"]);

    // Rows that left the plan (gone, stopped, not listed) lose their
    // stream after the grace time.
    follower.set_plan(vec![spec("abab0001", false)]);
    wait_for("the drop", || follower.streams().len() == 1);
    assert_eq!(follower.streams()[0].target.id, "abab0001");
}

#[test]
fn a_session_without_a_live_process_loses_its_stream() {
    let source = Source::new(&["acac0001"], 1, Duration::ZERO);
    let follower = start(&source, settings(10_000));
    follower.set_plan(vec![spec("acac0001", false)]);
    follower.fetch(spec("acac0001", false), true);
    let (_, result) = follower.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(result.is_ok());
    assert_eq!(follower.streams()[0].has_base, Some(true));
    source.sessions.lock().unwrap().clear();
    follower.fetch(spec("acac0001", false), true);
    let (_, result) = follower.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(result, Err(LogsError::NotRunning)), "{result:?}");
    assert_eq!(follower.streams()[0].has_base, None);
}

#[test]
fn background_follows_run_the_owning_account_command() {
    let _g = lock();
    let mut s = Session::start(30, 100, 40);
    s.spin(3);
    let capture = std::env::temp_dir().join(format!("tower-follow-{}.bin", std::process::id()));
    std::fs::write(&capture, s.capture()).unwrap();
    let claude = Fake::new("claude", &format!("cat '{}'", capture.display()));
    let follower = Follower::start(
        Arc::new(CliRunner::new().with_timeout(Duration::from_secs(20))),
        Arc::new(ProcEnviron),
        settings(150),
    );
    let mut followed = spec("7b4d9f02", true);
    followed.target.account = claude.account();
    followed.pid = None;
    follower.set_plan(vec![followed.clone()]);
    wait_for("two background fetches", || claude.calls() >= 2);
    drop(follower);
    std::fs::remove_file(&capture).unwrap();
    for argv in claude.all_argv() {
        assert_eq!(argv, ["logs", "7b4d9f02"]);
    }
}
