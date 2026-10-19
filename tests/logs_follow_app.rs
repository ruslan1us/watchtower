//! The app's side of following logs (doc/0030): which sessions it hands
//! the follower, the partial-output note in both styles, and a paused
//! panel whose stream keeps being fed in the background until `End`.

mod common;
#[path = "support/synthetic.rs"]
mod synthetic;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{app_single, app_two, ch, key, poll, select_name};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use synthetic::{Session, trimmed};
use watchtower::app::{App, Effect, LogsTarget, LogsView};
use watchtower::config::{Account, UiStyle};
use watchtower::listing::parse_listing;
use watchtower::logfollow::{FollowSettings, Follower, LogSource, MAX_STREAMS};
use watchtower::logview::{self, LogScreen, LogsError};
use watchtower::merge::ProcEnviron;
use watchtower::poller::PollUpdate;
use watchtower::ui::PARTIAL_NOTE;

fn ids(app: &App) -> Vec<(String, bool)> {
    app.follow_plan()
        .into_iter()
        .map(|s| (s.target.id, s.working))
        .collect()
}

#[test]
fn the_plan_holds_live_owned_rows_working_first() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    // Interactive and stopped rows are not followed; nothing works here.
    let plan = ids(&app);
    assert!(plan.iter().all(|(_, working)| !working), "{plan:?}");
    let listed: Vec<&str> = plan.iter().map(|(id, _)| id.as_str()).collect();
    for id in ["3e9a1c07", "5a7c2e91", "7b4d9f02", "9c6e1a35", "d4a1f7c8"] {
        assert!(listed.contains(&id), "{id} in {listed:?}");
    }
    assert!(!listed.contains(&"b2f8d064"), "stopped: {listed:?}");

    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    let plan = ids(&app);
    assert_eq!(plan[0], ("5a7c2e91".to_owned(), true), "{plan:?}");
    assert_eq!(plan.iter().filter(|(_, w)| *w).count(), 1);
    let spec = &app.follow_plan()[0];
    assert_eq!(spec.pid, Some(4240));
    assert_eq!(spec.target.account.name, "main");

    // The panel's row comes first.
    select_name(&mut app, "say-ok");
    app.handle_key(ch(' '));
    assert_eq!(ids(&app)[0], ("b2f8d064".to_owned(), false));
    assert_eq!(ids(&app)[1], ("5a7c2e91".to_owned(), true));
}

#[test]
fn rows_of_an_unknown_owner_are_not_followed() {
    let mut app = app_two();
    app.apply_poll(poll("main", "after-stop-all.json"));
    app.apply_poll(poll("alt", "after-stop-all.json"));
    let listed: Vec<String> = ids(&app).into_iter().map(|(id, _)| id).collect();
    assert!(!listed.contains(&"d4a1f7c8".to_owned()), "{listed:?}");
}

#[test]
fn the_plan_is_bounded() {
    let rows: Vec<String> = (0..MAX_STREAMS + 4)
        .map(|i| {
            format!(
                r#"{{"pid": {pid}, "id": "f{i:07}", "cwd": "/home/user/workspace/demo",
                   "kind": "background", "sessionId": "f{i:07}-0000-4000-8000-000000000000",
                   "name": "job-{i}", "status": "{status}", "state": "{state}"}}"#,
                pid = 5000 + i,
                status = if i % 2 == 0 { "busy" } else { "idle" },
                state = if i % 2 == 0 { "working" } else { "done" },
            )
        })
        .collect();
    let mut app = app_single();
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing(&format!("[{}]", rows.join(","))).unwrap()),
    });
    let plan = ids(&app);
    assert_eq!(plan.len(), MAX_STREAMS);
    let working = plan.iter().take_while(|(_, w)| *w).count();
    assert_eq!(working, 10, "every working row first: {plan:?}");
}

// ---- the partial note -------------------------------------------------------

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The app with the logs panel open on print-date, showing `screen`.
fn open_with(screen: LogScreen) -> App {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select_name(&mut app, "print-date");
    app.handle_key(ch(' '));
    deliver(&mut app, screen);
    app
}

fn deliver(app: &mut App, screen: LogScreen) -> Vec<Effect> {
    let target: LogsTarget = app.logs().unwrap().in_flight().cloned().unwrap();
    app.logs_fetched(target, Ok(screen))
}

/// Differential frames only: no full clear.
fn fragments() -> LogScreen {
    let lines: Vec<String> = (1..=40)
        .map(|i| format!("\x1b[{i};3Hfragment {i}"))
        .collect();
    logview::render(lines.concat().as_bytes())
}

#[test]
fn a_partial_screen_says_so_in_both_styles() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        let mut app = open_with(fragments());
        app.style = style;
        assert!(app.logs().unwrap().partial());
        let lines = render(&mut app, 200, 50);
        let title = lines
            .iter()
            .find(|l| l.contains("logs · print-date"))
            .unwrap_or_else(|| panic!("{style:?}: {lines:#?}"));
        assert!(title.contains(PARTIAL_NOTE), "{style:?}: {title}");
        let notes = lines.iter().filter(|l| l.contains(PARTIAL_NOTE)).count();
        assert_eq!(notes, 2, "{style:?}: title and bottom border\n{lines:#?}");

        // Paused: the note stays next to where the panel is.
        render(&mut app, 200, 20);
        app.handle_key(key(KeyCode::Right));
        app.handle_key(ch('g'));
        assert!(app.logs().unwrap().paused(), "{style:?}");
        let all = render(&mut app, 200, 20).join("\n");
        assert!(all.contains("paused · line 1–"), "{style:?}\n{all}");
        assert!(all.contains("partial output"), "{style:?}\n{all}");
        app.handle_key(key(KeyCode::Left));

        // Narrow: the name goes, then the note gets short; the hint keeps
        // only the keys.
        let all = render(&mut app, 60, 30).join("\n");
        assert!(all.contains("partial output"), "{style:?}\n{all}");
        assert!(!all.contains(PARTIAL_NOTE), "{style:?}\n{all}");

        // A screen with a base: no note.
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Left));
        deliver(&mut app, logview::render(b"\x1b[2J\x1b[Hwhole screen"));
        assert!(!app.logs().unwrap().partial(), "{style:?}");
        let all = render(&mut app, 200, 50).join("\n");
        assert!(all.contains("whole screen"), "{style:?}\n{all}");
        assert!(!all.contains("partial output"), "{style:?}\n{all}");
    }
}

// ---- paused while the stream is fed -----------------------------------------

/// One session that moves on by `step` frames per fetch.
struct OneSession {
    session: Mutex<Session>,
    step: usize,
    calls: Mutex<usize>,
}

impl LogSource for OneSession {
    fn logs(&self, _account: &Account, id: &str) -> Result<Vec<u8>, LogsError> {
        assert_eq!(id, "5a7c2e91");
        *self.calls.lock().unwrap() += 1;
        let mut s = self.session.lock().unwrap();
        s.spin(self.step);
        Ok(s.capture())
    }
}

fn screen_text(app: &App) -> String {
    match &app.logs().unwrap().view {
        LogsView::Screen(screen) => screen.plain_lines().join("\n"),
        other => panic!("{other:?}"),
    }
}

/// Runs the panel's fetches through `follower` until none is left.
fn run_fetches(app: &mut App, follower: &Follower, effects: Vec<Effect>) {
    let mut queue = effects;
    while let Some(effect) = queue.pop() {
        if let Effect::FetchLogs(t) = effect {
            follower.fetch(app.follow_spec(&t), true);
            let (t, result) = follower.recv_timeout(Duration::from_secs(5)).unwrap();
            queue.extend(app.logs_fetched(t, result));
        }
    }
}

#[test]
fn a_paused_panel_stays_while_its_stream_is_fed_and_end_shows_it() {
    let source = Arc::new(OneSession {
        session: Mutex::new(Session::start(30, 100, 40)),
        step: 20,
        calls: Mutex::new(0),
    });
    let follower = Follower::start(
        source.clone(),
        Arc::new(ProcEnviron),
        FollowSettings {
            cadence: Duration::from_millis(30),
            ..FollowSettings::default()
        },
    );
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select_name(&mut app, "run-test-suite");
    let effects = app.handle_key(ch(' '));
    run_fetches(&mut app, &follower, effects);
    follower.set_plan(app.follow_plan());
    assert!(screen_text(&app).contains("Example Code"));

    // Pause at the top.
    app.set_logs_height(5);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    assert!(app.logs().unwrap().paused());
    let frozen = screen_text(&app);

    // The session goes on; the follower keeps feeding its stream, and the
    // owner's polls fetch nothing for the paused panel.
    source.session.lock().unwrap().set(12, "a later message");
    let calls = *source.calls.lock().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while *source.calls.lock().unwrap() < calls + 4 {
        assert!(Instant::now() < until, "background fetches");
        std::thread::sleep(Duration::from_millis(10));
    }
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::FetchLogs(_))),
        "{effects:?}"
    );
    assert_eq!(screen_text(&app), frozen, "the snapshot stays");

    // End follows again: the stream as fed meanwhile, with its base, though
    // no capture holds the start's repaint any more.
    let effects = app.handle_key(key(KeyCode::End));
    run_fetches(&mut app, &follower, effects);
    assert!(!app.logs().unwrap().paused());
    let text = screen_text(&app);
    assert!(text.contains("a later message"), "{text}");
    assert!(text.contains("Example Code"), "{text}");
    assert!(!app.logs().unwrap().partial());
    let LogsView::Screen(screen) = &app.logs().unwrap().view else {
        unreachable!()
    };
    let reference = trimmed(&source.session.lock().unwrap().reference());
    assert_eq!(trimmed(&screen.plain_lines()), reference);
}
