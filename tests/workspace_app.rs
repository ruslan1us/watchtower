//! The workspace style's state machine and layout (doc/0045), without a
//! terminal or a child: [`App`] returns effects and the tests stand in for the
//! runtime, with a clock of their own. `tests/workspace.rs` runs the real
//! binary against a fake `claude`.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, ctrl, key, status_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use watchtower::app::{
    ATTACH_CHECK_TIMEOUT, ATTACH_DEBOUNCE, App, Effect, PaneFocus, PaneOpen, RELEASE_AFTER,
    coalesces,
};
use watchtower::config::{Config, ConfigError, UiStyle};
use watchtower::embed::PaneScreen;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::projects::ProjectList;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::ui;

const NOW_MS: i64 = 1_800_000_000_000;

fn row(id: &str, name: &str, process: &str, state: &str) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
            "cwd": "{HOME}/workspace/demo", "kind": "background", "name": "{name}",
            "state": "{state}", "startedAt": {NOW_MS} }}"#
    )
}

/// Two running rows, a stopped one in a group that starts folded, and an
/// interactive one.
fn listing() -> String {
    let live = r#""pid": 4100, "status": "idle","#;
    let rows = [
        row("aaaa1111", "alpha", live, "done"),
        row("bbbb2222", "beta", live, "done"),
        row("cccc3333", "gamma", "", "done"),
        format!(
            r#"{{ "pid": 4200, "cwd": "{HOME}/workspace/demo", "kind": "interactive",
                "startedAt": {NOW_MS}, "sessionId": "dddd-0000", "name": "delta" }}"#
        ),
    ];
    format!("[{}]", rows.join(","))
}

fn poll(text: &str) -> PollUpdate {
    PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing(text).unwrap()),
    }
}

fn app_from(text: &str) -> App {
    let config = Config::parse(
        "default_account = \"main\"\n[ui]\nstyle = \"workspace\"\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
        Path::new(HOME),
    )
    .unwrap();
    let mut sidecar = Sidecar::default();
    for s in &parse_listing(text).unwrap().sessions {
        if let Some(id) = &s.session_id {
            sidecar.record_owner(id, "main", AccountSource::Started);
        }
    }
    let mut app = App::new(config.clone(), HOME.into(), sidecar, Box::new(NoEnviron));
    app.style = config.ui_style;
    app.apply_poll(poll(text));
    app
}

fn draw(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

/// An app with the workspace drawn, the clock at `t0` and `alpha` selected.
fn workspace() -> (App, Instant) {
    let mut app = app_from(&listing());
    draw(&mut app, 160, 40);
    assert!(app.pane_active());
    common::select_name(&mut app, "alpha");
    let t0 = Instant::now();
    assert!(app.pane_tick(t0).is_empty());
    (app, t0)
}

fn opened(effects: &[Effect]) -> Option<&PaneOpen> {
    effects.iter().find_map(|e| match e {
        Effect::PaneOpen(open) => Some(open),
        _ => None,
    })
}

/// [`workspace`] with `alpha`'s client open: the clock after it opened, and
/// its generation and screen.
fn attached() -> (App, Instant, u64, PaneScreen) {
    let (mut app, t0) = workspace();
    let t = t0 + ATTACH_DEBOUNCE;
    let effects = app.pane_tick(t);
    let open = opened(&effects)
        .expect("attached after the debounce")
        .clone();
    assert_eq!(open.id, "aaaa1111");
    let screen = PaneScreen::new(open.rows, open.cols);
    app.pane_opened(open.generation, screen.clone());
    (app, t, open.generation, screen)
}

fn writes(effects: &[Effect]) -> Vec<u8> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::PaneWrite(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// With 160 columns the bar is 28 wide, so the pane's inside starts at
/// column 29, row 1.
const PANE: (u16, u16) = (29, 1);

fn in_pane(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    mouse(kind, PANE.0 + x, PANE.1 + y)
}

#[test]
fn the_arrows_attach_nothing_until_the_selection_rests() {
    let (mut app, t0) = workspace();
    // Moving on, again and again, within the debounce: the row left behind
    // never attaches, however long the whole run takes.
    let step = ATTACH_DEBOUNCE / 2;
    let mut t = t0;
    for code in [KeyCode::Down, KeyCode::Up, KeyCode::Down] {
        t += step;
        assert!(opened(&app.pane_tick(t)).is_none());
        app.handle_key(key(code));
        assert!(opened(&app.pane_tick(t)).is_none());
    }
    t += step;
    assert!(opened(&app.pane_tick(t)).is_none());
    t += step;
    let effects = app.pane_tick(t);
    assert_eq!(opened(&effects).unwrap().id, "bbbb2222");
    let open = opened(&effects).unwrap().clone();
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert!(opened(&app.pane_tick(t + ATTACH_DEBOUNCE * 4)).is_none());
}

#[test]
fn a_client_gets_the_owners_command_the_sessions_folder_and_the_panes_size() {
    let (_app, _t, _generation, screen) = attached();
    let (app_rows, app_cols) = screen.with(vt100::Screen::size);
    assert_eq!((app_rows, app_cols), (37, 130));
    let (mut app, t0) = workspace();
    let effects = app.pane_tick(t0 + ATTACH_DEBOUNCE);
    let open = opened(&effects).unwrap();
    assert_eq!(open.account.name, "main");
    assert_eq!(open.cwd, Path::new(HOME).join("workspace/demo"));
    assert_eq!((open.rows, open.cols), (37, 130));
}

#[test]
fn the_debounce_is_all_the_runtime_waits_for() {
    let (app, t0) = workspace();
    assert_eq!(app.pane_wait(t0), Some(ATTACH_DEBOUNCE));
    assert_eq!(
        app.pane_wait(t0 + ATTACH_DEBOUNCE * 2),
        Some(Duration::ZERO)
    );
    let (app, t, ..) = attached();
    assert_eq!(app.pane_wait(t), None);
}

#[test]
fn a_header_or_a_row_without_a_terminal_closes_the_client() {
    let (mut app, t, ..) = attached();
    common::select_name(&mut app, "delta");
    let t = t + Duration::from_millis(1);
    assert!(!app.pane_tick(t).contains(&Effect::PaneClose));
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE);
    assert!(effects.contains(&Effect::PaneClose), "{effects:?}");
    assert!(opened(&effects).is_none());
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("runs in another terminal"), "{text}");
    // `Enter` starts nothing here either, and says why.
    assert!(opened(&app.handle_key(key(KeyCode::Enter))).is_none());
    assert_eq!(app.pane_focus(), PaneFocus::List);
}

/// Unfolds the Stopped group and selects its row.
fn select_stopped(app: &mut App) {
    common::select_name(app, "alpha");
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Tab));
    common::select_name(app, "gamma");
}

#[test]
fn a_stopped_row_shows_a_panel_and_only_enter_wakes_it() {
    let (mut app, t0) = workspace();
    select_stopped(&mut app);
    let mut t = t0 + Duration::from_millis(1);
    assert!(opened(&app.pane_tick(t)).is_none());
    t += ATTACH_DEBOUNCE * 10;
    assert!(opened(&app.pane_tick(t)).is_none());
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("selecting it started nothing"), "{text}");
    assert!(text.contains("Enter wakes it here"), "{text}");
    assert!(
        text.contains("Enter wake here"),
        "the footer says so: {text}"
    );

    // Enter goes through the refresh of doc/0005, and opens in the pane.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.contains(&Effect::PollForAttach), "{effects:?}");
    assert!(opened(&effects).is_none());
    let effects = app.tick(Instant::now() + ATTACH_CHECK_TIMEOUT + Duration::from_secs(1));
    let open = opened(&effects).expect("the wake opens in the pane");
    assert_eq!(open.id, "cccc3333");
    assert_eq!(open.account.name, "main");
    assert!(!effects.iter().any(|e| matches!(e, Effect::Attach(_))));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
}

#[test]
fn enter_moves_the_keys_and_the_highlight_clears_there_not_on_selection() {
    let live = r#""pid": 4100, "status": "waiting", "waitingFor": "permission","#;
    let text = format!("[{}]", row("aaaa1111", "alpha", live, "blocked"));
    let mut app = app_from(&text);
    draw(&mut app, 160, 40);
    app.handle_key(key(KeyCode::Down));
    let t0 = Instant::now();
    app.pane_tick(t0);
    let effects = app.pane_tick(t0 + ATTACH_DEBOUNCE);
    let open = opened(&effects).unwrap().clone();
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert_eq!(app.attention_count(), 1, "selected and attached: still lit");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
    assert_eq!(app.attention_count(), 0);
}

#[test]
fn a_prompt_while_the_terminal_has_the_keys_is_already_seen() {
    let (mut app, t, ..) = attached();
    app.handle_key(key(KeyCode::Enter));
    let live = r#""pid": 4100, "status": "waiting", "waitingFor": "permission","#;
    let text = format!("[{}]", row("aaaa1111", "alpha", live, "blocked"));
    app.apply_poll(poll(&text));
    assert_eq!(app.attention_count(), 1);
    let effects = app.pane_tick(t);
    assert!(effects.contains(&Effect::SaveSidecar), "{effects:?}");
    assert_eq!(app.attention_count(), 0);
}

#[test]
fn enter_before_the_debounce_attaches_at_once() {
    let (mut app, _t0) = workspace();
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(opened(&effects).unwrap().id, "aaaa1111");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let open = opened(&effects).unwrap().clone();
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert_eq!(writes(&app.handle_key(ch('x'))), b"x");
}

#[test]
fn in_the_terminal_every_key_is_claudes_but_ctrl_z_and_f12() {
    let (mut app, _t, _generation, screen) = attached();
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pane_has_keys());
    assert_eq!(writes(&app.handle_key(ctrl('c'))), b"\x03");
    assert_eq!(writes(&app.handle_key(ch('q'))), b"q");
    assert_eq!(writes(&app.handle_key(key(KeyCode::Esc))), b"\x1b");
    assert_eq!(writes(&app.handle_key(key(KeyCode::Up))), b"\x1b[A");
    screen.feed(b"\x1b[?1h");
    assert_eq!(writes(&app.handle_key(key(KeyCode::Up))), b"\x1bOA");
    // Typed under a Cyrillic layout, the terminal keeps the letter.
    assert_eq!(writes(&app.handle_key(ch('я'))), "я".as_bytes());
    for back in [ctrl('z'), key(KeyCode::F(12)), ctrl('я')] {
        assert!(writes(&app.handle_key(back)).is_empty());
        assert_eq!(app.pane_focus(), PaneFocus::List, "{back:?}");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    }
    // Focus reports go only to a child that asked for them.
    screen.feed(b"\x1b[?1004h");
    assert_eq!(writes(&app.handle_key(ctrl('z'))), b"\x1b[O");
    assert_eq!(writes(&app.handle_key(key(KeyCode::Enter))), b"\x1b[I");
}

#[test]
fn a_window_takes_the_keys_from_the_terminal_and_gives_them_back() {
    let (mut app, ..) = attached();
    app.handle_key(key(KeyCode::Enter));
    app.open_dialog(ProjectList::default());
    assert!(!app.pane_has_keys());
    assert!(writes(&app.handle_key(ch('x'))).is_empty());
    assert!(
        writes(&app.handle_paste("y")).is_empty(),
        "paste goes to the window too"
    );
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("New session"), "{text}");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('y'));
    assert!(app.dialog().is_none());
    assert!(app.pane_has_keys());
    assert_eq!(writes(&app.handle_key(ch('z'))), b"z");
}

#[test]
fn paste_goes_where_the_keys_go_and_the_list_ignores_it() {
    let (mut app, _t, _generation, screen) = attached();
    assert!(
        app.handle_paste("quit").is_empty(),
        "no pasted letter runs a key"
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(writes(&app.handle_paste("a\nb")), b"a\rb");
    screen.feed(b"\x1b[?2004h");
    assert_eq!(
        writes(&app.handle_paste("a\nb")),
        b"\x1b[200~a\nb\x1b[201~".to_vec()
    );
}

#[test]
fn actions_on_the_row_close_its_client_and_it_stays_closed() {
    let (mut app, t, ..) = attached();
    // `s` on a row that is neither working nor blocked stops directly.
    let effects = app.handle_key(ch('s'));
    let stop = effects
        .iter()
        .find(|e| matches!(e, Effect::Stop(_)))
        .expect("stops")
        .clone();
    assert!(
        app.pane_before(&stop),
        "the runtime closes the client first"
    );
    assert_eq!(app.pane_client_key(), None);
    // Listed as running until the next poll: it must not attach, which would
    // wake what was just stopped.
    let mut later = t;
    for _ in 0..5 {
        later += ATTACH_DEBOUNCE;
        assert!(opened(&app.pane_tick(later)).is_none());
    }
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Not attached"), "{text}");
    assert!(text.contains("Enter attaches it here"), "{text}");
    // `Enter` asks for it again.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert_eq!(opened(&effects).unwrap().id, "aaaa1111");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
}

#[test]
fn moving_away_and_back_lifts_the_hold() {
    let (mut app, t, ..) = attached();
    let effects = app.handle_key(ch('s'));
    let stop = effects
        .iter()
        .find(|e| matches!(e, Effect::Stop(_)))
        .unwrap();
    assert!(app.pane_before(stop));
    let mut later = t + ATTACH_DEBOUNCE;
    assert!(opened(&app.pane_tick(later)).is_none());
    common::select_name(&mut app, "beta");
    later += Duration::from_millis(1);
    app.pane_tick(later);
    common::select_name(&mut app, "alpha");
    later += Duration::from_millis(1);
    app.pane_tick(later);
    later += ATTACH_DEBOUNCE;
    assert_eq!(opened(&app.pane_tick(later)).unwrap().id, "aaaa1111");
}

#[test]
fn an_action_on_another_session_leaves_the_client_alone() {
    let (mut app, ..) = attached();
    common::select_name(&mut app, "beta");
    let effects = app.handle_key(ch('s'));
    let stop = effects
        .iter()
        .find(|e| matches!(e, Effect::Stop(_)))
        .unwrap();
    assert!(!app.pane_before(stop));
    assert_eq!(
        app.pane_client_key(),
        Some("aaaa1111-0000-4000-8000-000000000000")
    );
}

#[test]
fn a_client_that_ends_returns_the_keys_and_is_not_reattached() {
    let (mut app, t, generation, _screen) = attached();
    app.handle_key(key(KeyCode::Enter));
    // Another client's end changes nothing.
    assert!(app.pane_exited(generation + 1, Some(0)).is_empty());
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let effects = app.pane_exited(generation, Some(0));
    assert_eq!(
        effects,
        [Effect::PollNow],
        "the row's state is asked at once"
    );
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(opened(&app.pane_tick(t + ATTACH_DEBOUNCE * 5)).is_none());
    assert!(status_text(&app).contains("the terminal of alpha ended"));
    assert!(draw(&mut app, 160, 40).contains("Not attached"));
}

#[test]
fn a_failed_clients_screen_stays_until_the_selection_moves() {
    let (mut app, t, generation, screen) = attached();
    screen.feed(b"No job matching 'aaaa1111'.");
    app.pane_exited(generation, Some(1));
    assert!(status_text(&app).contains("exited with code 1"));
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("No job matching"), "{text}");
    assert!(opened(&app.pane_tick(t + ATTACH_DEBOUNCE)).is_none());
    common::select_name(&mut app, "beta");
    app.pane_tick(t + ATTACH_DEBOUNCE * 2);
    assert!(!draw(&mut app, 160, 40).contains("No job matching"));
}

#[test]
fn the_client_is_released_after_ten_minutes_without_the_keys() {
    let (mut app, t, ..) = attached();
    assert!(app.pane_tick(t + RELEASE_AFTER / 2).is_empty());
    // With the keys, the time does not count.
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pane_tick(t + RELEASE_AFTER * 2).is_empty());
    app.handle_key(ctrl('z'));
    let start = Instant::now();
    assert!(app.pane_tick(start + RELEASE_AFTER / 2).is_empty());
    let effects = app.pane_tick(start + RELEASE_AFTER * 2);
    assert_eq!(effects, [Effect::PaneClose]);
    assert!(opened(&app.pane_tick(start + RELEASE_AFTER * 3)).is_none());
    assert!(draw(&mut app, 160, 40).contains("released"));
    assert!(opened(&app.handle_key(key(KeyCode::Enter))).is_some());
}

#[test]
fn the_panes_size_follows_the_terminal() {
    let (mut app, t, ..) = attached();
    draw(&mut app, 200, 50);
    let effects = app.pane_tick(t);
    assert_eq!(
        effects,
        [Effect::PaneResize {
            rows: 47,
            cols: 162
        }]
    );
    assert!(app.pane_tick(t).is_empty());
}

#[test]
fn a_narrow_terminal_closes_the_client_and_draws_the_dashboard() {
    let (mut app, t, ..) = attached();
    app.handle_key(key(KeyCode::Enter));
    let text = draw(&mut app, 99, 40);
    assert!(text.contains("Sessions"), "the dashboard's list: {text}");
    assert!(!app.pane_active());
    assert!(!app.wants_mouse());
    assert_eq!(app.pane_tick(t), [Effect::PaneClose]);
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(status_text(&app).contains("the workspace needs 100x24"));
    // Enter there is the dashboard's: the checked attach that takes the
    // terminal, never the pane.
    common::select_name(&mut app, "alpha");
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.contains(&Effect::PollForAttach), "{effects:?}");
    let effects = app.tick(Instant::now() + ATTACH_CHECK_TIMEOUT + Duration::from_secs(1));
    assert!(effects.iter().any(|e| matches!(e, Effect::Attach(_))));
    // At 100 columns the workspace is back.
    draw(&mut app, 100, 40);
    assert!(app.pane_active());
}

#[test]
fn a_terminal_that_cannot_start_falls_back_to_the_dashboard() {
    let (mut app, t0) = workspace();
    let effects = app.pane_tick(t0 + ATTACH_DEBOUNCE);
    let open = opened(&effects).unwrap().clone();
    app.pane_start_failed(open.generation, "no pty");
    assert!(status_text(&app).contains("no pty"));
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Sessions"), "{text}");
    assert!(!app.pane_active());
    assert!(opened(&app.pane_tick(t0 + ATTACH_DEBOUNCE * 3)).is_none());
}

#[test]
fn settings_over_the_pane_leave_the_client_alone() {
    let (mut app, t, ..) = attached();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ui]\nstyle = \"workspace\"\n").unwrap();
    let session = watchtower::config_edit::EditSession::open(&path, Path::new(HOME)).unwrap();
    app.settings_opened(Ok(session));
    assert!(app.settings().is_some());
    draw(&mut app, 160, 40);
    assert!(app.pane_tick(t + RELEASE_AFTER / 2).is_empty());
    assert!(app.pane_client_key().is_some());
}

#[test]
fn the_logs_panel_gives_way_to_the_pane() {
    let (mut app, t, ..) = attached();
    assert!(app.handle_key(ch(' ')).is_empty());
    assert!(app.logs().is_none());
    assert!(status_text(&app).contains("logs panel"));
    assert!(app.handle_key(key(KeyCode::Right)).is_empty());
    assert!(app.pane_tick(t).is_empty());
}

#[test]
fn a_press_on_the_pane_focuses_it_and_one_on_the_list_returns_the_keys() {
    let (mut app, _t, _generation, screen) = attached();
    screen.feed(b"\x1b[?1000;1002;1006h");
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    assert_eq!(
        writes(&app.handle_mouse(in_pane(down, 2, 3))),
        b"\x1b[<0;3;4M"
    );
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert_eq!(
        writes(&app.handle_mouse(in_pane(up, 2, 3))),
        b"\x1b[<0;3;4m"
    );
    // On the list: the keys come back, and nothing goes to the child.
    let on_list = app.handle_mouse(mouse(down, 3, 2));
    assert!(writes(&on_list).is_empty());
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(writes(&app.handle_mouse(mouse(up, 3, 2))).is_empty());
}

#[test]
fn a_click_on_a_row_selects_it() {
    let (mut app, ..) = attached();
    let text = draw(&mut app, 160, 40);
    let y = text.lines().position(|l| l.contains("beta")).unwrap() as u16;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, y));
    assert_eq!(
        app.selected_session().map(|m| m.session.display_name()),
        Some("beta".to_owned())
    );
}

#[test]
fn the_mouse_is_forwarded_only_in_the_modes_that_ask_for_it() {
    let (mut app, _t, _generation, screen) = attached();
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    let drag = MouseEventKind::Drag(MouseButton::Left);
    // Mode off: the press only focuses.
    assert!(writes(&app.handle_mouse(in_pane(down, 1, 1))).is_empty());
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(writes(&app.handle_mouse(in_pane(MouseEventKind::ScrollUp, 1, 1))).is_empty());
    // Press and release: no motion.
    screen.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(
        writes(&app.handle_mouse(in_pane(down, 1, 1))),
        b"\x1b[<0;2;2M"
    );
    assert!(writes(&app.handle_mouse(in_pane(drag, 2, 1))).is_empty());
    assert!(writes(&app.handle_mouse(in_pane(MouseEventKind::Moved, 2, 1))).is_empty());
    assert_eq!(
        writes(&app.handle_mouse(in_pane(up, 2, 1))),
        b"\x1b[<0;3;2m"
    );
    // Button motion: drags, no plain motion.
    screen.feed(b"\x1b[?1002h");
    assert_eq!(
        writes(&app.handle_mouse(in_pane(down, 1, 1))),
        b"\x1b[<0;2;2M"
    );
    assert_eq!(
        writes(&app.handle_mouse(in_pane(drag, 2, 1))),
        b"\x1b[<32;3;2M"
    );
    assert!(writes(&app.handle_mouse(in_pane(MouseEventKind::Moved, 2, 1))).is_empty());
    assert_eq!(
        writes(&app.handle_mouse(in_pane(up, 2, 1))),
        b"\x1b[<0;3;2m"
    );
    // Any motion, and the wheel, inside the pane only.
    screen.feed(b"\x1b[?1003h");
    assert_eq!(
        writes(&app.handle_mouse(in_pane(MouseEventKind::Moved, 2, 1))),
        b"\x1b[<35;3;2M"
    );
    assert_eq!(
        writes(&app.handle_mouse(in_pane(MouseEventKind::ScrollDown, 2, 1))),
        b"\x1b[<65;3;2M"
    );
    assert!(writes(&app.handle_mouse(mouse(MouseEventKind::Moved, 2, 2))).is_empty());
    assert!(writes(&app.handle_mouse(mouse(MouseEventKind::ScrollUp, 2, 2))).is_empty());
}

#[test]
fn a_gesture_goes_on_clamped_and_ends_when_the_keys_move() {
    let (mut app, _t, _generation, screen) = attached();
    screen.feed(b"\x1b[?1002;1006h");
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    let drag = MouseEventKind::Drag(MouseButton::Left);
    app.handle_mouse(in_pane(down, 1, 1));
    assert_eq!(
        writes(&app.handle_mouse(mouse(drag, 0, 0))),
        b"\x1b[<32;1;1M"
    );
    assert_eq!(
        writes(&app.handle_mouse(mouse(drag, 500, 200))),
        b"\x1b[<32;130;37M"
    );
    // `F12`: the gesture is over; its release goes nowhere.
    app.handle_key(key(KeyCode::F(12)));
    app.handle_key(key(KeyCode::Enter));
    assert!(writes(&app.handle_mouse(in_pane(drag, 2, 2))).is_empty());
    assert!(writes(&app.handle_mouse(in_pane(up, 2, 2))).is_empty());
    // A window: the mouse does nothing, and the gesture is over too.
    app.handle_mouse(in_pane(down, 1, 1));
    app.open_dialog(ProjectList::default());
    assert!(writes(&app.handle_mouse(in_pane(drag, 2, 2))).is_empty());
    assert!(writes(&app.handle_mouse(in_pane(down, 2, 2))).is_empty());
}

#[test]
fn only_motion_of_one_kind_coalesces() {
    let drag = MouseEventKind::Drag(MouseButton::Left);
    let down = MouseEventKind::Down(MouseButton::Left);
    let m = |kind| mouse(kind, 1, 1);
    assert!(coalesces(
        &m(MouseEventKind::Moved),
        &m(MouseEventKind::Moved)
    ));
    assert!(coalesces(&m(drag), &m(drag)));
    assert!(!coalesces(&m(drag), &m(MouseEventKind::Moved)));
    assert!(!coalesces(&m(down), &m(down)));
    assert!(!coalesces(
        &m(MouseEventKind::ScrollUp),
        &m(MouseEventKind::ScrollUp)
    ));
}

#[test]
fn the_layout_has_the_bar_the_pane_and_one_footer_row() {
    let (mut app, ..) = attached();
    let text = draw(&mut app, 200, 50);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 50);
    // 18 % of 200 is 36 columns; the pane's frame starts right after it.
    let chars: Vec<char> = lines[0].chars().collect();
    assert_eq!(chars[0], '▌', "the list has the keys: {}", lines[0]);
    assert_eq!(chars[36], '╭', "{}", lines[0]);
    assert!(lines[0].contains("alpha"), "{}", lines[0]);
    assert!(lines[0].contains("view only"), "{}", lines[0]);
    assert!(lines[49].contains(" LIST "), "{}", lines[49]);
    assert!(lines[49].contains("refreshed"), "{}", lines[49]);
    app.handle_key(key(KeyCode::Enter));
    let text = draw(&mut app, 200, 50);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains('┏'), "a thick frame: {}", lines[0]);
    assert!(lines[0].contains("keyboard here"), "{}", lines[0]);
    assert!(lines[49].contains(" TERMINAL "), "{}", lines[49]);
    assert!(lines[49].contains("Ctrl+Z"), "{}", lines[49]);
}

#[test]
fn the_pane_shows_the_childs_cells_with_their_colours_and_wide_characters() {
    let (mut app, _t, _generation, screen) = attached();
    screen.feed("\x1b[31mred\x1b[0m 你好 end".as_bytes());
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, &mut app, Instant::now(), NOW_MS))
        .unwrap();
    let buf = terminal.backend().buffer();
    let at = |x: u16| &buf[(PANE.0 + x, PANE.1)];
    assert_eq!(at(0).symbol(), "r");
    assert_eq!(at(0).fg, ratatui::style::Color::Indexed(1));
    assert_eq!(at(4).symbol(), "你");
    assert_eq!(at(6).symbol(), "好");
    assert_eq!(at(9).symbol(), "e");
    assert_eq!(at(9).fg, ratatui::style::Color::Reset);
    // A cursor only while the terminal has the keys.
    assert!(terminal.get_cursor_position().is_ok());
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("red"), "{text}");
}

#[test]
fn no_color_keeps_the_childs_text_and_drops_its_colours() {
    let (mut app, _t, _generation, screen) = attached();
    app.color = false;
    screen.feed(b"\x1b[31;1mred");
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, &mut app, Instant::now(), NOW_MS))
        .unwrap();
    let cell = &terminal.backend().buffer()[(PANE.0, PANE.1)];
    assert_eq!(cell.symbol(), "r");
    assert_eq!(cell.fg, ratatui::style::Color::Reset);
    assert!(cell.modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn the_help_lists_the_panes_keys_in_this_style_and_fits_24_rows() {
    let (mut app, ..) = attached();
    app.handle_key(ch('?'));
    let text = draw(&mut app, 100, 24);
    assert!(text.contains("Terminal pane"), "{text}");
    assert!(text.contains("Ctrl+Z, F12"), "{text}");
    assert!(text.contains("Esc or ? closes this help"), "{text}");
}

#[test]
fn the_bars_width_is_a_share_of_the_terminal_within_its_limits() {
    let (app, _t0) = workspace();
    for (total, want) in [
        (100, 28),
        (120, 28),
        (160, 28),
        (200, 36),
        (222, 39),
        (400, 40),
    ] {
        assert_eq!(ui::workspace_list_width(&app, total), want, "{total}");
    }
    let config = |extra: &str| Config::parse(&format!("[ui]\n{extra}\n"), Path::new(HOME));
    let with = config("list_width = 50").unwrap();
    assert_eq!(with.ui_list_width, Some(50));
    let app = App::new(with, HOME.into(), Sidecar::default(), Box::new(NoEnviron));
    assert_eq!(ui::workspace_list_width(&app, 200), 50);
    assert_eq!(ui::workspace_list_width(&app, 80), 40, "half at most");
    assert_eq!(config("list_width = 0").unwrap().ui_list_width, None);
    for bad in ["list_width = 5", "list_width = 200"] {
        assert!(matches!(config(bad), Err(ConfigError::Invalid(_))), "{bad}");
    }
}

#[test]
fn the_config_takes_the_style_and_the_mouse_switch() {
    let parse = |text: &str| Config::parse(text, Path::new(HOME)).unwrap();
    assert_eq!(
        parse("[ui]\nstyle = \"workspace\"\n").ui_style,
        UiStyle::Workspace
    );
    assert!(parse("").ui_mouse, "on by default");
    assert!(!parse("[ui]\nmouse = false\n").ui_mouse);
    assert_ne!(UiStyle::default(), UiStyle::Workspace, "opt-in");
    let mut app = app_from(&listing());
    draw(&mut app, 160, 40);
    assert!(app.wants_mouse());
    let config = Config::parse(
        "default_account = \"main\"\n[ui]\nstyle = \"workspace\"\nmouse = false\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
        Path::new(HOME),
    )
    .unwrap();
    app.apply_config(config);
    draw(&mut app, 160, 40);
    assert!(app.pane_active());
    assert!(!app.wants_mouse());
}

#[test]
fn another_style_never_opens_a_client() {
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = app_from(&listing());
        app.style = style;
        draw(&mut app, 200, 50);
        assert!(!app.pane_active());
        assert!(!app.wants_mouse());
        let t = Instant::now();
        assert!(app.pane_tick(t).is_empty());
        assert!(app.pane_tick(t + ATTACH_DEBOUNCE * 2).is_empty());
        assert!(app.handle_paste("x").is_empty());
        let down = MouseEventKind::Down(MouseButton::Left);
        assert!(app.handle_mouse(mouse(down, 100, 10)).is_empty());
    }
}
