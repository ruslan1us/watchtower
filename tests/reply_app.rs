//! The quick reply as a state machine (doc/0040, doc/0011, doc/0015): which
//! rows `t` (and its Cyrillic key) offers and why the others are refused,
//! the window with its texts in both styles, the template picker in it,
//! the job `Ctrl+S` starts for a live and a stopped row, the progress and
//! result messages, a copy, the trust retry, an unknown owner, other jobs
//! on the row, and the help and footer.

mod common;

use std::path::Path;
use std::time::Instant;

use common::{NoEnviron, app_two_with, ch, ctrl, key, status_text, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect, Overlay, Purpose, ReplyJob};
use watchtower::config::{Config, UiStyle};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::reply::{Replied, ReplyError, ReplyStep};
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::templates::{Template, TemplateList};

/// One background row as `claude agents --json` lists it. `process` is
/// the live part (`pid`, `status`, `waitingFor`), empty for a stopped row.
fn row(n: u32, name: &str, state: &str, process: &str) -> String {
    let id = format!("{n:08x}");
    format!(
        r#"{{ {process} "id": "{id}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767232822505,
            "sessionId": "{id}-6a3c-4e1f-9d58-0c7a2b4e6f81", "name": "{name}",
            "state": "{state}" }}"#
    )
}

const IDLE: &str = r#""pid": 4342, "status": "idle","#;
const BUSY: &str = r#""pid": 4342, "status": "busy","#;

/// Every state combination the eligibility rules name, with whether `t`
/// opens the window, and a part of the refusal when it does not.
fn cases() -> Vec<(String, &'static str, Option<&'static str>)> {
    let c = |n, name, state, process: &str, refusal| (row(n, name, state, process), name, refusal);
    vec![
        // Live and idle at the end of a turn.
        c(1, "ready-done", "done", IDLE, None),
        c(2, "asked-question", "blocked", IDLE, None),
        c(3, "failed-live", "failed", IDLE, None),
        // No live process: stopped and finished, failed, mid-turn, stopped.
        c(4, "stopped-done", "done", "", None),
        c(5, "stopped-failed", "failed", "", None),
        c(6, "stopped-blocked", "blocked", "", None),
        c(7, "stopped-midturn", "working", "", None),
        c(8, "stopped-state", "stopped", "", None),
        // Not offered.
        c(
            9,
            "working-busy",
            "working",
            BUSY,
            Some("would interrupt its turn"),
        ),
        c(
            10,
            "working-idle",
            "working",
            IDLE,
            Some("is working; wait until its turn ends"),
        ),
        c(
            11,
            "done-bg-task",
            "done",
            BUSY,
            Some("still runs a background task"),
        ),
        c(
            12,
            "permission",
            "blocked",
            r#""pid": 4342, "status": "waiting", "waitingFor": "permission prompt","#,
            Some("waiting in Claude Code (permission prompt); attach to answer it"),
        ),
        c(
            13,
            "question-dialog",
            "blocked",
            r#""pid": 4342, "status": "waiting", "waitingFor": "input needed","#,
            Some("waiting in Claude Code (input needed); attach to answer it"),
        ),
        c(
            14,
            "idle-but-waiting-for",
            "blocked",
            r#""pid": 4342, "status": "idle", "waitingFor": "dialog open","#,
            Some("attach to answer it"),
        ),
        c(
            15,
            "pid-no-status",
            "done",
            r#""pid": 4342,"#,
            Some("cannot tell whether"),
        ),
        // The orchestrator, by the same rules.
        c(16, "orchestrator", "blocked", IDLE, None),
    ]
}

fn listing_text(rows: &[String]) -> String {
    format!("[{}]", rows.join(",\n"))
}

fn poll_text(account: &str, text: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(text).unwrap()),
    }
}

/// Both accounts list `text`; every row there is recorded as `main`'s.
fn app_with(text: &str) -> App {
    let listing = parse_listing(text).unwrap();
    let mut sc = Sidecar::default();
    for s in &listing.sessions {
        if let Some(sid) = &s.session_id {
            sc.record_owner(sid, "main", AccountSource::Started);
        }
    }
    let mut app = app_two_with(sc);
    app.apply_poll(poll_text("main", text));
    app.apply_poll(poll_text("alt", text));
    app
}

/// The usual pair: `ready-done` (live, idle) and `stopped-done`.
fn app_pair() -> App {
    app_with(&listing_text(&[
        row(1, "ready-done", "done", IDLE),
        row(4, "stopped-done", "done", ""),
    ]))
}

fn sid(n: u32) -> String {
    format!("{n:08x}-6a3c-4e1f-9d58-0c7a2b4e6f81")
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

/// `t` on `name` and `text` typed (`\n` as `Enter`).
fn open_and_type(app: &mut App, name: &str, text: &str) {
    select(app, name);
    assert!(app.handle_key(ch('t')).is_empty());
    assert!(app.reply_window().is_some(), "{}", status_text(app));
    for c in text.chars() {
        let effects = if c == '\n' {
            app.handle_key(key(KeyCode::Enter))
        } else {
            app.handle_key(ch(c))
        };
        assert!(effects.is_empty(), "{c:?}: {effects:?}");
    }
}

/// `Ctrl+S`, which must start the reply job.
fn send(app: &mut App) -> ReplyJob {
    let effects = app.handle_key(ctrl('s'));
    let job = effects.iter().find_map(|e| match e {
        Effect::Reply(job) => Some(job.clone()),
        _ => None,
    });
    job.unwrap_or_else(|| panic!("expected the reply job, got {effects:?}"))
}

fn render(app: &mut App, w: u16, h: u16) -> String {
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
        .collect::<Vec<_>>()
        .join("\n")
}

/// A screen with the window borders and padding removed, joined into one
/// line, so wrapped sentences can be searched.
fn flat(screen: &str) -> String {
    screen
        .lines()
        .map(|l| l.trim_matches(|c: char| " │┃║|".contains(c)))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- which rows are offered ---------------------------------------------------

#[test]
fn t_is_offered_only_for_idle_and_stopped_rows() {
    let cases = cases();
    let rows: Vec<String> = cases.iter().map(|(r, _, _)| r.clone()).collect();
    let mut app = app_with(&listing_text(&rows));
    for (_, name, refusal) in &cases {
        select(&mut app, name);
        assert!(app.handle_key(ch('t')).is_empty(), "{name}");
        match refusal {
            None => {
                let w = app
                    .reply_window()
                    .unwrap_or_else(|| panic!("{name}: {}", status_text(&app)));
                assert_eq!(w.name, *name);
                assert_eq!(w.account, "main");
                let live = app.selected_session().unwrap().session.has_live_process();
                assert_eq!(w.live, live, "{name}");
                assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
                assert!(app.reply_window().is_none());
            }
            Some(why) => {
                assert!(app.reply_window().is_none(), "{name} must not be offered");
                let status = status_text(&app);
                assert!(status.contains(why), "{name}: {status}");
                assert!(status.contains(name), "{name}: {status}");
            }
        }
    }
}

#[test]
fn interactive_rows_and_rows_without_ids_are_refused() {
    let text = r#"[
      { "pid": 1201, "cwd": "/home/user/workspace/demo-app", "kind": "interactive",
        "startedAt": 1767225600000, "sessionId": "e6c3b5d9-4f7a-4c2e-b1d8-9a0f2c4e6b13",
        "name": "demo-app-3f", "status": "idle" },
      { "id": "0000abcd", "cwd": "/home/user/workspace/demo-app", "kind": "background",
        "startedAt": 1767225600000, "name": "no-session-id", "state": "done" }
    ]"#;
    let mut app = app_with(text);
    select(&mut app, "demo-app-3f");
    app.handle_key(ch('t'));
    assert!(app.reply_window().is_none());
    assert!(status_text(&app).contains("runs in another terminal"));
    select(&mut app, "no-session-id");
    app.handle_key(ch('t'));
    assert!(app.reply_window().is_none());
    assert!(status_text(&app).contains("has no sessionId"));
}

#[test]
fn the_cyrillic_key_opens_the_window_and_text_stays_as_typed() {
    let mut app = app_pair();
    select(&mut app, "ready-done");
    // `е` is `t` on the RU/UA layout (doc/0011).
    assert!(app.handle_key(ch('е')).is_empty());
    assert!(app.reply_window().is_some());
    type_text(&mut app, "проверь");
    assert_eq!(app.reply_window().unwrap().text.text(), "проверь");
}

// ---- the window ---------------------------------------------------------------

#[test]
fn the_window_says_what_will_happen_in_both_styles() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        let mut app = app_pair();
        app.style = style;
        select(&mut app, "ready-done");
        app.handle_key(ch('t'));
        let screen = flat(&render(&mut app, 120, 40));
        assert!(
            screen.contains("Reply to ready-done"),
            "{style:?}: {screen}"
        );
        assert!(
            screen.contains(
                "Stops the idle session and wakes it under main with your text as the next \
                 prompt."
            ),
            "{style:?}: {screen}"
        );
        assert!(
            screen.contains("Enter new line · Ctrl+S send · Ctrl+P template · Esc cancel"),
            "{style:?}: {screen}"
        );
        app.handle_key(key(KeyCode::Esc));

        select(&mut app, "stopped-done");
        app.handle_key(ch('t'));
        type_text(&mut app, "looks good");
        let screen = flat(&render(&mut app, 120, 40));
        assert!(
            screen.contains("Reply to stopped-done"),
            "{style:?}: {screen}"
        );
        assert!(
            screen.contains("Wakes the session under main with your text as the next prompt."),
            "{style:?}: {screen}"
        );
        assert!(screen.contains("looks good"), "{style:?}: {screen}");
        assert!(!screen.contains("Stops the idle"), "{style:?}: {screen}");
    }
}

#[test]
fn enter_is_a_new_line_and_esc_asks_before_dropping_text() {
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "one\ntwo");
    app.handle_key(ctrl('j'));
    type_text(&mut app, "three");
    assert_eq!(app.reply_window().unwrap().text.text(), "one\ntwo\nthree");
    // Esc with text asks; n keeps editing, y drops it.
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.reply_window().unwrap().prompt.is_some());
    let screen = flat(&render(&mut app, 120, 40));
    assert!(screen.contains("Discard this text?"), "{screen}");
    app.handle_key(ch('n'));
    assert!(app.reply_window().unwrap().prompt.is_none());
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('y'));
    assert!(app.reply_window().is_none());
    assert!(status_text(&app).contains("reply cancelled; nothing was sent"));
    // A blank window closes at once, and Ctrl+C there quits.
    select(&mut app, "ready-done");
    app.handle_key(ch('t'));
    assert_eq!(app.handle_key(ctrl('c')), [Effect::Quit]);
}

#[test]
fn an_empty_text_sends_nothing() {
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "  \n  ");
    assert!(app.handle_key(ctrl('s')).is_empty());
    let w = app.reply_window().expect("still open");
    assert!(
        w.error.as_deref().unwrap().contains("nothing to send"),
        "{w:?}"
    );
}

#[test]
fn ctrl_p_inserts_a_template_at_the_cursor() {
    let mut app = app_pair();
    app.set_config_dir(Path::new("/xdg/watchtower"));
    open_and_type(&mut app, "ready-done", "Thanks.\n");
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(load)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(load.folder, Path::new("/xdg/watchtower/templates"));
    let list = TemplateList {
        templates: vec![
            Template {
                name: "review".into(),
                text: "Review the diff.\n- be strict".into(),
            },
            Template {
                name: "tests".into(),
                text: "Add tests.".into(),
            },
        ],
        ..TemplateList::default()
    };
    assert!(app.templates_loaded(load.id, list).is_empty());
    let screen = render(&mut app, 120, 40);
    assert!(screen.contains("Templates"), "{screen}");
    assert!(screen.contains("review"), "{screen}");
    // The picker has the keys: typed text filters it.
    type_text(&mut app, "rev");
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let w = app.reply_window().unwrap();
    assert!(w.templates.is_none());
    assert_eq!(w.text.text(), "Thanks.\nReview the diff.\n- be strict");
    // A blank text is replaced; Esc in the picker leaves the text as it was.
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "  ");
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(load)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    let one = TemplateList {
        templates: vec![Template {
            name: "tests".into(),
            text: "Add tests.".into(),
        }],
        ..TemplateList::default()
    };
    app.templates_loaded(load.id, one.clone());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.reply_window().unwrap().text.text(), "Add tests.");
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(load)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    app.templates_loaded(load.id, one);
    app.handle_key(key(KeyCode::Esc));
    let w = app.reply_window().unwrap();
    assert!(w.templates.is_none());
    assert_eq!(w.text.text(), "Add tests.");
}

// ---- the job --------------------------------------------------------------------

#[test]
fn ctrl_s_on_a_live_row_starts_a_stop_and_wake_with_the_text() {
    let mut app = app_pair();
    open_and_type(
        &mut app,
        "ready-done",
        "  - check the tests\nthen report  \n",
    );
    let job = send(&mut app);
    assert!(app.reply_window().is_none(), "the window closes");
    let r = &job.request;
    assert_eq!(r.id, "00000001");
    assert_eq!(r.session_id, sid(1));
    assert_eq!(r.owner.name, "main");
    assert!(r.live);
    assert_eq!(r.text, "- check the tests\nthen report");
    assert_eq!(r.cwd, Path::new("/home/user/workspace/demo-app"));
    assert!(!job.after_trust);
    assert_eq!(job.recorded, app.sidecar.base_owner(&sid(1)));
    assert!(status_text(&app).contains("stopping ready-done…"));
    app.reply_step(
        &sid(1),
        ReplyStep::Waking {
            account: "main".into(),
        },
    );
    assert!(status_text(&app).contains("waking ready-done with your text…"));
    // The row's other actions wait for it.
    for k in ['t', 'a', 'e', 's', 'd'] {
        app.handle_key(ch(k));
        assert!(
            status_text(&app).contains("tower is sending your reply to ready-done"),
            "{k}: {}",
            status_text(&app)
        );
        assert!(app.reply_window().is_none());
        assert_eq!(*app.overlay(), Overlay::None, "{k}");
    }
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let effects = app.reply_finished(job, Ok(Replied { notes: Vec::new() }));
    assert!(effects.contains(&Effect::PollNow));
    let status = status_text(&app);
    assert!(status.contains("sent to ready-done"), "{status}");
    assert!(!status.contains("waking"), "{status}");
}

#[test]
fn ctrl_s_on_a_stopped_row_only_wakes_it() {
    let mut app = app_pair();
    open_and_type(&mut app, "stopped-done", "merge it");
    let job = send(&mut app);
    assert!(!job.request.live);
    assert_eq!(job.request.session_id, sid(4));
    assert!(status_text(&app).contains("waking stopped-done with your text…"));
    // Another row can get its reply meanwhile.
    open_and_type(&mut app, "ready-done", "and you too");
    let second = send(&mut app);
    let status = status_text(&app);
    assert!(status.contains("waking stopped-done"), "{status}");
    assert!(status.contains("stopping ready-done"), "{status}");
    app.reply_finished(second, Ok(Replied { notes: Vec::new() }));
    assert!(status_text(&app).contains("waking stopped-done"));
}

#[test]
fn a_row_that_changed_under_the_window_says_what_ctrl_s_does_now() {
    let stopped = listing_text(&[
        row(1, "ready-done", "done", ""),
        row(4, "stopped-done", "done", ""),
    ]);
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "go");
    app.apply_poll(poll_text("main", &stopped));
    app.apply_poll(poll_text("alt", &stopped));
    assert!(app.handle_key(ctrl('s')).is_empty());
    let w = app.reply_window().unwrap();
    assert!(!w.live);
    assert!(w.error.as_deref().unwrap().contains("stopped meanwhile"));
    let job = send(&mut app);
    assert!(!job.request.live);

    // A row that started working is refused, and the text is kept.
    let busy = listing_text(&[
        row(1, "ready-done", "working", BUSY),
        row(4, "stopped-done", "done", ""),
    ]);
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "go");
    app.apply_poll(poll_text("main", &busy));
    app.apply_poll(poll_text("alt", &busy));
    assert!(app.handle_key(ctrl('s')).is_empty());
    let w = app.reply_window().unwrap();
    assert!(w.error.as_deref().unwrap().contains("is working"), "{w:?}");
    assert_eq!(w.text.text(), "go");
}

#[test]
fn a_copy_is_reported_with_its_id_and_recorded_as_started() {
    let mut app = app_pair();
    open_and_type(&mut app, "stopped-done", "go");
    let job = send(&mut app);
    let effects = app.reply_finished(
        job,
        Err(ReplyError::Copied {
            new_id: "0c0d0e0f".into(),
            notes: vec!["note: … started a copy as 0c0d0e0f.".into()],
        }),
    );
    assert!(effects.contains(&Effect::SaveSidecar));
    let status = status_text(&app);
    assert!(
        status.contains("started a copy 0c0d0e0f of stopped-done under main"),
        "{status}"
    );
    assert!(
        status.contains("nothing else was stopped or removed"),
        "{status}"
    );
    assert_eq!(
        app.sidecar.owner("0c0d0e0f"),
        Some(("main", &AccountSource::Started))
    );
}

#[test]
fn an_untrusted_folder_offers_the_trust_flow_and_retries_without_a_stop() {
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "go");
    let job = send(&mut app);
    assert!(job.request.live);
    app.reply_finished(
        job,
        Err(ReplyError::Untrusted {
            dir: "/home/user/workspace/demo-app".into(),
            message: "Workspace not trusted. Run `claude` in /home/user/workspace/demo-app \
                      once and accept the trust prompt, then retry."
                .into(),
        }),
    );
    let Overlay::ReplyUntrusted(u) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    let screen = flat(&render(&mut app, 120, 40));
    assert!(screen.contains("Not trusted by main"), "{screen}");
    assert!(
        screen.contains("Your text was not sent to ready-done"),
        "{screen}"
    );
    let effects = app.handle_key(ctrl('t'));
    let [Effect::ReplyTrust(trust)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(*trust, u.job);
    let effects = app.reply_trust_finished(trust.clone(), Ok(Some(0)));
    let [Effect::Reply(retry)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert!(retry.after_trust);
    assert!(!retry.request.live, "not stopped again");
    assert_eq!(retry.request.text, "go");
    // Esc instead leaves it.
    app.reply_finished(
        retry.clone(),
        Err(ReplyError::Untrusted {
            dir: "/home/user/workspace/demo-app".into(),
            message: "Workspace not trusted.".into(),
        }),
    );
    let Overlay::ReplyUntrusted(u) = app.overlay().clone() else {
        panic!("{:?}", app.overlay())
    };
    assert!(u.message.contains("still not trusted after running claude"));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(*app.overlay(), Overlay::None);
    assert!(status_text(&app).contains("stays stopped and your text was not sent"));
}

#[test]
fn another_towers_record_and_other_failures_are_reported() {
    let mut app = app_pair();
    open_and_type(&mut app, "ready-done", "go");
    let job = send(&mut app);
    let effects = app.reply_finished(
        job.clone(),
        Err(ReplyError::OwnerChanged {
            account: "alt".into(),
            stopped: false,
        }),
    );
    assert!(effects.contains(&Effect::CheckSidecar));
    let status = status_text(&app);
    assert!(
        status.contains("another tower recorded ready-done under alt meanwhile; nothing was run"),
        "{status}"
    );
    app.reply_finished(
        job,
        Err(ReplyError::StillRunning {
            waited: std::time::Duration::from_secs(10),
            last_error: None,
        }),
    );
    let status = status_text(&app);
    assert!(status.contains("cannot send to ready-done"), "{status}");
    assert!(status.contains("your text was not sent"), "{status}");
}

// ---- owners and other jobs ------------------------------------------------------

#[test]
fn an_unknown_owner_asks_for_the_account_first_in_one_group() {
    let text = listing_text(&[row(1, "ready-done", "done", IDLE)]);
    let mut app = app_two_with(Sidecar::default());
    app.apply_poll(poll_text("main", &text));
    app.apply_poll(poll_text("alt", &text));
    select(&mut app, "ready-done");
    app.handle_key(ch('t'));
    let Overlay::Picker(picker) = app.overlay().clone() else {
        panic!("{:?} {}", app.overlay(), status_text(&app))
    };
    assert_eq!(picker.purpose, Purpose::Reply);
    let screen = flat(&render(&mut app, 120, 40));
    assert!(screen.contains("to send your text"), "{screen}");
    app.handle_key(ch('j'));
    app.handle_key(key(KeyCode::Enter));
    let w = app.reply_window().expect("opened after the pick");
    assert_eq!(w.account, "alt");
    assert_eq!(
        app.sidecar.owner(&sid(1)),
        Some(("alt", &AccountSource::Asked))
    );
    type_text(&mut app, "go");
    let job = send(&mut app);
    assert_eq!(job.request.owner.name, "alt");
}

#[test]
fn an_unknown_owner_is_refused_when_the_accounts_span_groups() {
    let home = Path::new(common::HOME);
    let config = Config::parse(
        r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal"
"#,
        home,
    )
    .unwrap();
    let text = listing_text(&[row(1, "ready-done", "done", IDLE)]);
    let mut app = App::new(config, home.into(), Sidecar::default(), Box::new(NoEnviron));
    app.apply_poll(poll_text("main", &text));
    app.apply_poll(poll_text("alt", &text));
    select(&mut app, "ready-done");
    app.handle_key(ch('t'));
    assert!(app.reply_window().is_none());
    assert_eq!(*app.overlay(), Overlay::None);
    let status = status_text(&app);
    assert!(status.contains("account is unknown (?)"), "{status}");
    assert!(status.contains("attach first (Enter)"), "{status}");
}

#[test]
fn a_row_in_a_hand_off_or_a_rename_is_refused() {
    let mut app = app_pair();
    select(&mut app, "ready-done");
    app.handle_key(ch('a'));
    let effects = app.handle_key(ch('y'));
    assert!(
        matches!(effects.as_slice(), [Effect::Handoff(_)]),
        "{effects:?}"
    );
    app.handle_key(ch('t'));
    assert!(app.reply_window().is_none());
    assert!(status_text(&app).contains("is being handed off"));

    let mut app = app_pair();
    select(&mut app, "stopped-done");
    app.handle_key(ch('e'));
    app.handle_key(ctrl('u'));
    type_text(&mut app, "renamed");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(effects.as_slice(), [Effect::Rename(_)]),
        "{effects:?}"
    );
    app.handle_key(ch('t'));
    assert!(app.reply_window().is_none());
    assert!(status_text(&app).contains("is being renamed"));
}

// ---- help and footer ----------------------------------------------------------

#[test]
fn help_and_footer_offer_t() {
    let mut app = app_pair();
    app.handle_key(ch('?'));
    let screen = render(&mut app, 100, 24);
    assert!(screen.contains("m, t"), "{screen}");
    assert!(screen.contains("quick reply"), "{screen}");
    app.handle_key(key(KeyCode::Esc));
    let screen = render(&mut app, 240, 20);
    let footer = screen.lines().last().unwrap();
    assert!(footer.contains("t reply"), "{footer}");
}
