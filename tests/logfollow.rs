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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_claude::{Fake, lock};
use synthetic::{Session, trimmed};
use watchtower::app::LogsTarget;
use watchtower::cli::CliRunner;
use watchtower::config::Account;
use watchtower::logfollow::{FollowSettings, FollowSpec, Follower, LogSource};
use watchtower::logview::{LogScreen, LogsError};
use watchtower::merge::ProcEnviron;

/// Sessions by id, each advanced by `step` spinner frames per fetch; ids
/// not listed are stopped. Records every call with the time it started and
/// the most fetches that ran at the same time.
///
/// Fetches can be made to wait, so tests do not depend on how fast the
/// machine is: while `hold` is set every fetch waits for it to clear, and
/// with `overlap` the first fetch waits until a second one runs.
struct Source {
    sessions: Mutex<HashMap<String, Session>>,
    step: usize,
    delay: Duration,
    calls: Mutex<Vec<(String, Instant)>>,
    running: AtomicUsize,
    most: AtomicUsize,
    hold: AtomicBool,
    overlap: AtomicBool,
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
            hold: AtomicBool::new(false),
            overlap: AtomicBool::new(false),
        })
    }

    fn calls_of(&self, id: &str) -> usize {
        self.starts_of(id).len()
    }

    /// When each fetch of `id` started, in order.
    fn starts_of(&self, id: &str) -> Vec<Instant> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| c == id)
            .map(|(_, t)| *t)
            .collect()
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
        let first = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((id.to_owned(), Instant::now()));
            calls.len() == 1
        };
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        if first && self.overlap.load(Ordering::SeqCst) {
            wait_for("a second fetch at the same time", || {
                self.running.load(Ordering::SeqCst) >= 2
            });
        }
        wait_for("the hold to end", || !self.hold.load(Ordering::SeqCst));
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
        cadence: Some(Duration::from_millis(cadence_ms)),
        ..FollowSettings::default()
    }
}

fn start(source: &Arc<Source>, settings: FollowSettings) -> Follower {
    Follower::start(source.clone(), Arc::new(ProcEnviron), settings)
}

/// How long a test waits for something that should happen; only a broken
/// follower takes this long, so it can be generous for slow machines.
const PATIENCE: Duration = Duration::from_secs(30);

/// Waits up to [`PATIENCE`] for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let until = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until the follower has taken in everything sent to it so far and
/// no fetch runs: from then on, a fetch starts only when it decides on one.
/// (`streams` is answered in order with the other messages.)
fn settle(follower: &Follower) {
    wait_for("no fetch running", || {
        follower.streams().iter().all(|s| !s.busy)
    });
}

/// A request's result, waiting up to [`PATIENCE`].
fn answer(follower: &Follower) -> (LogsTarget, Result<LogScreen, LogsError>) {
    follower
        .recv_timeout(PATIENCE)
        .expect("the request reports back")
}

#[test]
fn working_sessions_are_fetched_at_the_cadence() {
    let cadence = Duration::from_millis(100);
    let source = Source::new(&["aaaa0001", "bbbb0002"], 5, Duration::ZERO);
    let follower = start(&source, settings(100));
    follower.set_plan(vec![spec("aaaa0001", true), spec("bbbb0002", false)]);
    // It keeps being fetched, never sooner than a cadence after the last
    // fetch (a fetch starts after the previous one ended, so the starts are
    // at least a cadence apart however slow the machine is).
    wait_for("four fetches", || source.calls_of("aaaa0001") >= 4);
    let starts = source.starts_of("aaaa0001");
    for pair in starts.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap >= cadence, "fetches {gap:?} apart at a 100 ms cadence");
    }
    assert_eq!(source.calls_of("bbbb0002"), 0, "not working: not fetched");
    // A working row that stops working is no longer fetched: once the
    // follower has the new plan and its last fetch is back, nothing more
    // starts for several cadences.
    follower.set_plan(vec![spec("aaaa0001", false), spec("bbbb0002", false)]);
    settle(&follower);
    let settled = source.calls_of("aaaa0001");
    std::thread::sleep(cadence * 3);
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
    let source = Source::new(&refs, 1, Duration::from_millis(20));
    // The first fetch waits for a second one, so they overlap however slow
    // the machine is: the follower must run them at the same time.
    source.overlap.store(true, Ordering::SeqCst);
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
    let source = Source::new(&["dddd0001"], 5, Duration::ZERO);
    // The background fetch is held until the request is in.
    source.hold.store(true, Ordering::SeqCst);
    let follower = start(&source, settings(60_000));
    follower.set_plan(vec![spec("dddd0001", true)]);
    wait_for("the background fetch", || source.calls_of("dddd0001") == 1);
    follower.fetch(spec("dddd0001", true), true);
    // Taken in (`streams` is answered after it) while the fetch still runs.
    let streams = follower.streams();
    assert!(streams[0].busy, "{streams:?}");
    assert_eq!(source.calls_of("dddd0001"), 1, "it waits for the first");
    source.hold.store(false, Ordering::SeqCst);
    let (target, result) = answer(&follower);
    assert_eq!(target.id, "dddd0001");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        source.calls_of("dddd0001"),
        2,
        "one more fetch, after the first"
    );
    assert_eq!(source.most.load(Ordering::SeqCst), 1, "never two at once");
    // Requests count for the cadence: nothing more for a while.
    settle(&follower);
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
    settle(&follower);
    follower.fetch(spec("eeee0001", false), true);
    let (_, result) = answer(&follower);
    let screen = result.unwrap();
    assert!(!screen.partial());
    assert_eq!(trimmed(&screen.plain_lines()), source.reference("eeee0001"));
    let text = screen.plain_lines().join("\n");
    assert!(text.contains("Example Code v9.9.9"), "{text}");

    // A follower that starts late has no base: the screen says so.
    let late = start(&source, settings(10_000));
    late.fetch(spec("eeee0001", false), true);
    let (_, result) = answer(&late);
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
    // Back from the attach: once the follower knows and its last fetch is
    // back, nothing more starts for several cadences.
    follower.attached(None);
    settle(&follower);
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
            cadence: Some(Duration::from_secs(60)),
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
    // A result is published only once the follower has its stream back
    // (doc/0030), so `streams` right after it sees that stream.
    follower.fetch(spec("acac0001", false), true);
    let (_, result) = answer(&follower);
    assert!(result.is_ok());
    let streams = follower.streams();
    assert!(!streams[0].busy, "{streams:?}");
    assert_eq!(streams[0].has_base, Some(true));
    source.sessions.lock().unwrap().clear();
    follower.fetch(spec("acac0001", false), true);
    let (_, result) = answer(&follower);
    assert!(matches!(result, Err(LogsError::NotRunning)), "{result:?}");
    let streams = follower.streams();
    assert!(!streams[0].busy, "{streams:?}");
    assert_eq!(streams[0].has_base, None);
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
    // The fake counts a call before it records the call's arguments, and
    // a third call may be running: wait until the second call's arguments
    // are written in full (its environment comes after them) and check the
    // first two.
    wait_for("two background fetches", || {
        claude.calls() >= 2 && claude.dir.path().join("env.2.txt").exists()
    });
    drop(follower);
    for n in 1..=2 {
        assert_eq!(claude.argv_of(n), ["logs", "7b4d9f02"], "call {n}");
    }
    std::fs::remove_file(&capture).unwrap();
}

/// `logs_follow_secs` applied while the follower runs (doc/0031): a new
/// cadence counts at once from each session's last fetch, off stops the
/// background fetches of working rows and of the attached session while
/// requests still run and feed the stream, and turning it on again resumes.
#[test]
fn the_cadence_changes_live_and_off_stops_background_fetches() {
    let source = Source::new(&["adad0001"], 1, Duration::ZERO);
    // Far longer than the test: only the first fetch, due at once.
    let follower = start(
        &source,
        FollowSettings {
            cadence: Some(Duration::from_secs(600)),
            ..FollowSettings::default()
        },
    );
    follower.set_plan(vec![spec("adad0001", true)]);
    wait_for("the first fetch", || source.calls_of("adad0001") == 1);
    settle(&follower);

    // A shorter cadence applies at once, counted from the last fetch.
    let short = Duration::from_millis(50);
    follower.set_cadence(Some(short));
    wait_for("fetches at the new cadence", || {
        source.calls_of("adad0001") >= 4
    });
    for pair in source.starts_of("adad0001").windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap >= short, "fetches {gap:?} apart at a 50 ms cadence");
    }

    // Off: once the follower has it and its last fetch is back, nothing
    // starts for many cadences, whether the row works or is attached.
    follower.set_cadence(None);
    settle(&follower);
    let off = source.calls_of("adad0001");
    std::thread::sleep(short * 4);
    follower.attached(Some(spec("adad0001", false)));
    settle(&follower);
    std::thread::sleep(short * 4);
    assert_eq!(source.calls_of("adad0001"), off, "no background fetch");

    // Requests still run and feed the stream.
    follower.fetch(spec("adad0001", true), true);
    let (_, result) = answer(&follower);
    assert!(!result.unwrap().partial(), "the stream kept its base");
    assert_eq!(source.calls_of("adad0001"), off + 1);
    follower.fetch(spec("adad0001", true), false);
    wait_for("the fetch without a snapshot", || {
        source.calls_of("adad0001") == off + 2
    });
    settle(&follower);
    std::thread::sleep(short * 4);
    assert_eq!(source.calls_of("adad0001"), off + 2, "still no background");
    assert_eq!(follower.streams()[0].has_base, Some(true));

    // On again: the background fetches resume.
    follower.set_cadence(Some(short));
    wait_for("fetches again", || source.calls_of("adad0001") >= off + 4);
}
