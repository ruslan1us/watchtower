//! The pane's own text selection, the shell's history and zoom (doc/0045),
//! without a terminal or a child: [`App`] returns effects and the tests stand
//! in for the runtime, with a clock of their own. `tests/workspace.rs` runs the
//! real binary with a fake `claude` and a real `/bin/sh`.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, ctrl, key, status_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use watchtower::app::{ATTACH_DEBOUNCE, App, DOUBLE_CLICK, Effect, PaneFocus};
use watchtower::config::Config;
use watchtower::embed::{PaneScreen, SHELL_HISTORY, clipboard_write};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::projects::ProjectList;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::ui;

const NOW_MS: i64 = 1_800_000_000_000;

/// With 160 columns the bar is 28 wide, so the pane's inside starts at
/// column 29, row 1, and is 130 × 37 on 160 × 40.
const PANE: (u16, u16) = (29, 1);
const LEFT: MouseButton = MouseButton::Left;

fn row(id: &str, name: &str, process: &str) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
            "cwd": "{HOME}/workspace/demo", "kind": "background", "name": "{name}",
            "state": "done", "startedAt": {NOW_MS} }}"#
    )
}

fn listing() -> String {
    let live = r#""pid": 4100, "status": "idle","#;
    format!(
        "[{}]",
        [
            row("aaaa1111", "alpha", live),
            row("bbbb2222", "beta", live),
            row("cccc3333", "gamma", ""),
        ]
        .join(",")
    )
}

fn workspace() -> App {
    let text = listing();
    let config = Config::parse(
        "default_account = \"main\"\n[ui]\nstyle = \"workspace\"\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
        Path::new(HOME),
    )
    .unwrap();
    let mut sidecar = Sidecar::default();
    for s in &parse_listing(&text).unwrap().sessions {
        if let Some(id) = &s.session_id {
            sidecar.record_owner(id, "main", AccountSource::Started);
        }
    }
    let mut app = App::new(config.clone(), HOME.into(), sidecar, Box::new(NoEnviron));
    app.style = config.ui_style;
    app.apply_poll(PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing(&text).unwrap()),
    });
    draw(&mut app, 160, 40);
    common::select_name(&mut app, "alpha");
    app.pane_tick(Instant::now());
    app
}

fn draw_buffer(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text_of(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn draw(app: &mut App, w: u16, h: u16) -> String {
    text_of(&draw_buffer(app, w, h))
}

/// A shell on `alpha` with the keys, its history on, as the runtime opens it.
fn shell() -> (App, PaneScreen, u64) {
    let mut app = workspace();
    let open = app
        .handle_key(ch('!'))
        .iter()
        .find_map(|e| match e {
            Effect::PaneShellOpen(open) => Some(open.clone()),
            _ => None,
        })
        .expect("a shell opens");
    let screen = PaneScreen::with_history(open.rows, open.cols, SHELL_HISTORY);
    app.pane_opened(open.generation, screen.clone());
    draw(&mut app, 160, 40);
    (app, screen, open.generation)
}

/// `alpha`'s client with the keys, as the runtime opens it: no history.
fn client() -> (App, PaneScreen) {
    let mut app = workspace();
    let effects = app.pane_tick(Instant::now() + ATTACH_DEBOUNCE);
    let open = effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneOpen(open) => Some(open.clone()),
            _ => None,
        })
        .expect("attached after the debounce");
    let screen = PaneScreen::new(open.rows, open.cols);
    app.pane_opened(open.generation, screen.clone());
    app.handle_key(key(KeyCode::Enter));
    draw(&mut app, 160, 40);
    (app, screen)
}

fn at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn in_pane(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    at(kind, PANE.0 + x, PANE.1 + y)
}

/// A press, a drag and a release in the pane's cells; the release's effects.
fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) -> Vec<Effect> {
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), from.0, from.1));
    app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), to.0, to.1));
    app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), to.0, to.1))
}

fn copied(text: &str) -> Vec<Effect> {
    vec![Effect::PaneCopy(clipboard_write(text).unwrap())]
}

fn shift_page(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn lines(screen: &PaneScreen, range: std::ops::Range<usize>) {
    for k in range {
        screen.feed(format!("line{k}\r\n").as_bytes());
    }
}

fn pane_resize(effects: &[Effect]) -> Option<(u16, u16)> {
    effects.iter().find_map(|e| match e {
        Effect::PaneResize { rows, cols } | Effect::PaneShellResize { rows, cols } => {
            Some((*rows, *cols))
        }
        _ => None,
    })
}

// ---- the selection ---------------------------------------------------------------

#[test]
fn a_drag_selects_copies_on_release_and_says_how_much() {
    let (mut app, screen, _) = shell();
    screen.feed(b"hello wide world\r\nsecond line");
    let effects = drag(&mut app, (6, 0), (3, 1));
    assert_eq!(effects, copied("wide world\nseco"));
    assert!(status_text(&app).starts_with("copied 15 characters"));
    assert_eq!(
        app.pane_selection_spans(),
        vec![(0, 6, 15), (1, 0, 3)],
        "stays highlighted"
    );
    // The selection is painted in the theme's colours, nothing else is.
    let buf = draw_buffer(&mut app, 160, 40);
    let selected = buf[(PANE.0 + 6, PANE.1)].bg;
    assert_ne!(selected, buf[(PANE.0 + 5, PANE.1)].bg, "the cell before it");
    assert_eq!(buf[(PANE.0 + 15, PANE.1)].bg, selected);
    assert_ne!(buf[(PANE.0 + 16, PANE.1)].bg, selected, "the blank end");
    assert_eq!(buf[(PANE.0, PANE.1 + 1)].bg, selected);
    assert_ne!(buf[(PANE.0 + 4, PANE.1 + 1)].bg, selected, "after the end");
}

#[test]
fn nothing_goes_to_the_program_without_the_mouse() {
    let (mut app, screen, _) = shell();
    screen.feed(b"text");
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 0, 0));
    let drag = app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), 2, 0));
    assert!(drag.is_empty(), "{drag:?}");
    let release = app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), 2, 0));
    assert_eq!(release, copied("tex"));
}

#[test]
fn a_drag_stays_inside_the_pane() {
    let (mut app, screen, _) = shell();
    screen.feed(b"top\r\n");
    screen.feed(&b"\r\n".repeat(35));
    screen.feed(b"bottom row");
    // Past the bottom right corner, and on over the footer.
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 2, 36));
    app.handle_mouse(at(MouseEventKind::Drag(LEFT), 500, 200));
    let effects = app.handle_mouse(at(MouseEventKind::Up(LEFT), 500, 200));
    assert_eq!(effects, copied("ttom row"));
    // Past the top left corner, over the bar: from the pane's first cell.
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 2, 0));
    app.handle_mouse(at(MouseEventKind::Drag(LEFT), 0, 0));
    let effects = app.handle_mouse(at(MouseEventKind::Up(LEFT), 0, 0));
    assert_eq!(effects, copied("top"));
    assert_eq!(
        app.pane_focus(),
        PaneFocus::Terminal,
        "the bar took nothing"
    );
}

#[test]
fn a_drag_from_the_bar_starts_nothing() {
    let (mut app, screen, _) = shell();
    screen.feed(b"text in the pane");
    // A press on the bar gives the list the keys, as always.
    app.handle_mouse(at(MouseEventKind::Down(LEFT), 2, 0));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    let moved = app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), 5, 0));
    let up = app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), 5, 0));
    assert!(moved.is_empty() && up.is_empty(), "{moved:?} {up:?}");
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn a_click_clears_the_selection_and_focuses_the_pane() {
    let (mut app, screen, _) = shell();
    screen.feed(b"some text");
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 6, 0));
    assert!(app.pane_selection_spans().is_empty());
    let up = app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), 6, 0));
    assert!(up.is_empty(), "a click copies nothing: {up:?}");
    // From the list too: the keys come here, as today.
    app.handle_key(ctrl('z'));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 1, 0));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn a_drag_over_nothing_copies_nothing() {
    let (mut app, _screen, _) = shell();
    let effects = drag(&mut app, (3, 5), (9, 8));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(app.pane_selection_spans().is_empty());
    assert!(!status_text(&app).contains("copied"));
}

#[test]
fn a_double_click_takes_the_word_and_a_slow_one_does_not() {
    let (mut app, screen, _) = shell();
    screen.feed(b"open src/app.rs now");
    let t0 = Instant::now();
    let press = in_pane(MouseEventKind::Down(LEFT), 7, 0);
    let release = in_pane(MouseEventKind::Up(LEFT), 7, 0);
    assert!(app.handle_mouse_at(press, t0).is_empty());
    assert!(app.handle_mouse_at(release, t0).is_empty());
    let t1 = t0 + Duration::from_millis(100);
    assert!(app.handle_mouse_at(press, t1).is_empty());
    assert_eq!(app.handle_mouse_at(release, t1), copied("src/app.rs"));
    assert!(status_text(&app).starts_with("copied 10 characters"));
    // Too late for a double click: a click, which clears it.
    let t2 = t1 + DOUBLE_CLICK;
    app.handle_mouse_at(press, t2);
    assert!(app.pane_selection_spans().is_empty());
    assert!(app.handle_mouse_at(release, t2).is_empty());
    let t3 = t2 + DOUBLE_CLICK + Duration::from_millis(1);
    app.handle_mouse_at(press, t3);
    assert!(app.pane_selection_spans().is_empty());
    assert!(app.handle_mouse_at(release, t3).is_empty());
    // Another cell is no double click, and blank space selects nothing.
    let other = in_pane(MouseEventKind::Down(LEFT), 1, 0);
    let t4 = t3 + DOUBLE_CLICK + Duration::from_millis(1);
    app.handle_mouse_at(press, t4);
    app.handle_mouse_at(other, t4);
    assert!(app.pane_selection_spans().is_empty());
    let blank = in_pane(MouseEventKind::Down(LEFT), 60, 5);
    let t5 = t4 + DOUBLE_CLICK + Duration::from_millis(1);
    app.handle_mouse_at(blank, t5);
    app.handle_mouse_at(blank, t5);
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn wide_characters_and_wrapped_lines_copy_as_they_read() {
    let (mut app, screen, _) = shell();
    screen.feed("日本語 text\r\n".as_bytes());
    // From the second half of the first character to the first of the last.
    assert_eq!(drag(&mut app, (1, 0), (4, 0)), copied("日本語"));
    assert_eq!(app.pane_selection_spans(), vec![(0, 0, 5)]);
    // A line longer than the pane comes back as one line.
    let long = "x".repeat(130) + "yz";
    screen.feed(long.as_bytes());
    assert_eq!(drag(&mut app, (0, 1), (5, 2)), copied(&long));
}

#[test]
fn output_under_a_selection_ends_it_and_output_elsewhere_does_not() {
    let (mut app, screen, _) = shell();
    screen.feed(b"keep this\r\n\r\nprompt$ ");
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    // Below it: the selection stays.
    screen.feed(b"typed");
    app.pane_tick(Instant::now());
    assert!(!app.pane_selection_spans().is_empty());
    // Over it: it ends.
    screen.feed(b"\x1b[1;1Hcha");
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn output_during_a_drag_ends_it_and_nothing_is_copied() {
    let (mut app, screen, _) = shell();
    screen.feed(b"some text");
    app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 0, 0));
    app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), 3, 0));
    screen.feed(b"\x1b[1;1Hnew!");
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
    assert!(
        app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), 4, 0))
            .is_empty()
    );
    assert!(app.pane_selection_spans().is_empty());
    assert!(
        app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), 4, 0))
            .is_empty()
    );
}

#[test]
fn esc_ends_a_selection_and_only_then_goes_to_the_program() {
    let (mut app, screen, _) = shell();
    screen.feed(b"some text");
    drag(&mut app, (0, 0), (3, 0));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.pane_selection_spans().is_empty());
    assert_eq!(
        app.handle_key(key(KeyCode::Esc)),
        [Effect::PaneShellWrite(b"\x1b".to_vec())]
    );
}

#[test]
fn the_keys_leaving_or_the_program_changing_end_a_selection() {
    let (mut app, screen, generation) = shell();
    screen.feed(b"some text");
    // The keys go back to the list.
    drag(&mut app, (0, 0), (3, 0));
    app.handle_key(ctrl('z'));
    assert!(app.pane_selection_spans().is_empty());
    // `!` hides the shell.
    app.handle_key(key(KeyCode::Enter));
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    app.handle_key(key(KeyCode::F(12)));
    app.handle_key(ch('!'));
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
    // A window takes the keys.
    app.handle_key(ch('!'));
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    app.open_dialog(ProjectList::default());
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
    assert!(
        drag(&mut app, (0, 0), (3, 0)).is_empty(),
        "a window has the mouse"
    );
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('y'));
    // The program ends.
    assert!(app.dialog().is_none());
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    app.pane_exited(generation, Some(0));
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn a_resize_ends_a_selection() {
    let (mut app, screen, _) = shell();
    screen.feed(b"some text");
    drag(&mut app, (0, 0), (3, 0));
    draw(&mut app, 150, 40);
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn while_the_program_asks_for_the_mouse_everything_goes_to_it() {
    let (mut app, screen, _) = shell();
    screen.feed(b"some text");
    drag(&mut app, (0, 0), (3, 0));
    assert!(!app.pane_selection_spans().is_empty());
    screen.feed(b"\x1b[?1002;1006h");
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty(), "it ends");
    let sgr = |effects: Vec<Effect>| match effects.as_slice() {
        [Effect::PaneShellWrite(bytes)] => bytes.clone(),
        other => panic!("{other:?}"),
    };
    let press = in_pane(MouseEventKind::Down(LEFT), 0, 0);
    assert_eq!(sgr(app.handle_mouse(press)), b"\x1b[<0;1;1M");
    let moved = in_pane(MouseEventKind::Drag(LEFT), 3, 0);
    assert_eq!(sgr(app.handle_mouse(moved)), b"\x1b[<32;4;1M");
    assert!(app.pane_selection_spans().is_empty());
    let up = in_pane(MouseEventKind::Up(LEFT), 3, 0);
    assert_eq!(sgr(app.handle_mouse(up)), b"\x1b[<0;4;1m");
    // The wheel too, and a page key with `Shift`.
    let wheel = in_pane(MouseEventKind::ScrollUp, 3, 0);
    assert_eq!(sgr(app.handle_mouse(wheel)), b"\x1b[<64;4;1M");
    assert_eq!(
        sgr(app.handle_key(shift_page(KeyCode::PageUp))),
        b"\x1b[5;2~"
    );
    assert_eq!(app.pane_scrolled(), 0);
}

#[test]
fn claudes_terminal_keeps_every_mouse_event_when_it_asks() {
    let (mut app, screen) = client();
    screen.feed(b"\x1b[?1000;1002;1003;1006h");
    let sgr = |effects: Vec<Effect>| match effects.as_slice() {
        [Effect::PaneWrite(bytes)] => bytes.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        sgr(app.handle_mouse(in_pane(MouseEventKind::Down(LEFT), 1, 1))),
        b"\x1b[<0;2;2M"
    );
    assert_eq!(
        sgr(app.handle_mouse(in_pane(MouseEventKind::Drag(LEFT), 2, 1))),
        b"\x1b[<32;3;2M"
    );
    assert_eq!(
        sgr(app.handle_mouse(in_pane(MouseEventKind::Up(LEFT), 2, 1))),
        b"\x1b[<0;3;2m"
    );
    assert_eq!(
        sgr(app.handle_mouse(in_pane(MouseEventKind::ScrollDown, 2, 1))),
        b"\x1b[<65;3;2M"
    );
    assert!(app.pane_selection_spans().is_empty());
    assert!(!status_text(&app).contains("copied"));
}

#[test]
fn a_client_without_the_mouse_can_be_selected_but_has_no_history() {
    let (mut app, screen) = client();
    screen.feed(b"plain client");
    assert_eq!(drag(&mut app, (0, 0), (4, 0)), copied("plain"));
    // Its keys and wheel stay as they were.
    let wheel = in_pane(MouseEventKind::ScrollUp, 3, 0);
    assert!(app.handle_mouse(wheel).is_empty());
    assert_eq!(
        app.handle_key(shift_page(KeyCode::PageUp)),
        [Effect::PaneWrite(b"\x1b[5;2~".to_vec())]
    );
}

// ---- the history -----------------------------------------------------------------

#[test]
fn the_wheel_and_the_page_keys_scroll_and_the_title_says_so() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..200);
    let text = draw(&mut app, 160, 40);
    assert!(!text.contains('↑'), "{text}");
    let up = in_pane(MouseEventKind::ScrollUp, 3, 3);
    assert!(app.handle_mouse(up).is_empty());
    assert_eq!(app.pane_scrolled(), 3);
    let text = draw(&mut app, 160, 40);
    let title = text.lines().next().unwrap();
    assert!(title.contains("↑ 3 lines"), "{title}");
    assert!(title.contains("shell  "), "{title}");
    // A page is the pane's height less a row.
    assert!(app.handle_key(shift_page(KeyCode::PageUp)).is_empty());
    assert_eq!(app.pane_scrolled(), 3 + 36);
    assert!(app.handle_key(shift_page(KeyCode::PageDown)).is_empty());
    assert_eq!(app.pane_scrolled(), 3);
    let down = in_pane(MouseEventKind::ScrollDown, 3, 3);
    app.handle_mouse(down);
    assert_eq!(app.pane_scrolled(), 0);
    let title = draw(&mut app, 160, 40);
    assert!(!title.lines().next().unwrap().contains('↑'));
    // Plain page keys are the program's.
    assert_eq!(
        app.handle_key(key(KeyCode::PageUp)),
        [Effect::PaneShellWrite(b"\x1b[5~".to_vec())]
    );
}

#[test]
fn the_wheel_scrolls_with_the_list_holding_the_keys_and_the_view_is_kept() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    app.handle_key(ctrl('z'));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    app.handle_mouse(in_pane(MouseEventKind::ScrollUp, 3, 3));
    assert_eq!(app.pane_scrolled(), 3);
    assert_eq!(app.pane_focus(), PaneFocus::List, "the wheel moves no keys");
}

#[test]
fn any_key_or_paste_for_the_program_returns_to_the_bottom() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    screen.scroll(10);
    assert_eq!(
        app.handle_key(ch('x')),
        [Effect::PaneShellWrite(b"x".to_vec())]
    );
    assert_eq!(app.pane_scrolled(), 0);
    screen.scroll(10);
    assert_eq!(
        app.handle_paste("y"),
        [Effect::PaneShellWrite(b"y".to_vec())]
    );
    assert_eq!(app.pane_scrolled(), 0);
    // Tower's own keys do not.
    screen.scroll(10);
    app.handle_key(key(KeyCode::F(2)));
    assert_eq!(app.pane_scrolled(), 10);
    app.handle_key(key(KeyCode::F(2)));
    app.handle_key(ctrl('z'));
    assert_eq!(app.pane_scrolled(), 10);
}

#[test]
fn new_output_does_not_jump_the_view_and_the_marker_counts_it() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    app.handle_mouse(in_pane(MouseEventKind::ScrollUp, 3, 3));
    let before = draw(&mut app, 160, 40);
    lines(&screen, 100..105);
    app.pane_tick(Instant::now());
    let after = draw(&mut app, 160, 40);
    assert_eq!(app.pane_scrolled(), 8);
    assert!(
        after.lines().next().unwrap().contains("↑ 8 lines"),
        "{after}"
    );
    // The pane's rows are the same, the title's marker aside.
    let body = |text: &str| -> Vec<String> {
        text.lines()
            .skip(1)
            .take(36)
            .map(|l| l.chars().skip(29).take(100).collect())
            .collect()
    };
    assert_eq!(body(&before), body(&after));
}

#[test]
fn a_selection_works_while_scrolled_and_copies_what_is_highlighted() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..200);
    app.handle_key(shift_page(KeyCode::PageUp));
    app.handle_key(shift_page(KeyCode::PageUp));
    assert_eq!(app.pane_scrolled(), 72);
    let text = draw(&mut app, 160, 40);
    let shown: Vec<String> = text
        .lines()
        .skip(1)
        .take(2)
        .map(|l| l.chars().skip(29).take(7).collect::<String>())
        .map(|l| l.trim_end().to_owned())
        .collect();
    let want = shown.join("\n");
    assert!(want.starts_with("line"), "{text}");
    assert_eq!(drag(&mut app, (0, 0), (6, 1)), copied(&want));
    let said = format!("copied {} characters", want.len());
    assert!(
        status_text(&app).starts_with(&said),
        "{}",
        status_text(&app)
    );
    assert!(!app.pane_selection_spans().is_empty());
    // Output does not move the view, so it stays.
    lines(&screen, 200..205);
    app.pane_tick(Instant::now());
    assert!(!app.pane_selection_spans().is_empty());
    assert_eq!(app.pane_scrolled(), 77);
    // Moving the view changes what is under it: it ends.
    app.handle_mouse(in_pane(MouseEventKind::ScrollDown, 3, 3));
    assert!(app.pane_selection_spans().is_empty());
    // So does a key going back to the bottom.
    drag(&mut app, (0, 0), (6, 1));
    assert!(!app.pane_selection_spans().is_empty());
    app.handle_key(ch('x'));
    assert!(app.pane_selection_spans().is_empty());
    assert_eq!(app.pane_scrolled(), 0);
}

#[test]
fn the_alternate_screen_and_the_mouse_stop_the_history() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    // A full-screen program: the wheel does nothing, page keys are its own, a
    // drag selects.
    screen.feed(b"\x1b[?1049hless screen");
    let wheel = in_pane(MouseEventKind::ScrollUp, 3, 3);
    assert!(app.handle_mouse(wheel).is_empty());
    assert_eq!(app.pane_scrolled(), 0);
    assert_eq!(
        app.handle_key(shift_page(KeyCode::PageUp)),
        [Effect::PaneShellWrite(b"\x1b[5;2~".to_vec())]
    );
    assert_eq!(drag(&mut app, (0, 0), (3, 0)), copied("less"));
    app.handle_key(key(KeyCode::Esc));
    screen.feed(b"\x1b[?1049l");
    // A program asking for the mouse while the view is up brings it down.
    app.handle_mouse(wheel);
    assert_eq!(app.pane_scrolled(), 3);
    screen.feed(b"\x1b[?1000h");
    app.pane_tick(Instant::now());
    assert_eq!(app.pane_scrolled(), 0);
}

#[test]
fn up_in_the_history_there_is_no_cursor() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    screen.feed(b"$ \x1b[?25h");
    // A new backend's cursor stays at the corner unless the frame places it.
    let placed = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal
            .draw(|f| ui::draw_at(f, app, Instant::now(), NOW_MS))
            .unwrap();
        terminal.get_cursor_position().unwrap() != Position::new(0, 0)
    };
    assert!(placed(&mut app));
    app.handle_key(shift_page(KeyCode::PageUp));
    assert!(!placed(&mut app));
    app.handle_key(shift_page(KeyCode::PageDown));
    assert!(placed(&mut app));
}

#[test]
fn a_resize_keeps_the_view_sane() {
    let (mut app, screen, _) = shell();
    lines(&screen, 0..100);
    app.handle_key(shift_page(KeyCode::PageUp));
    let at = app.pane_scrolled();
    draw(&mut app, 110, 30);
    let effects = app.pane_tick(Instant::now());
    let (rows, cols) = pane_resize(&effects).expect("the shell follows");
    screen.set_size(rows, cols);
    assert_eq!(app.pane_scrolled(), at);
    let text = draw(&mut app, 110, 30);
    assert!(text.contains("line"), "{text}");
}

// ---- zoom ------------------------------------------------------------------------

fn top_left(text: &str) -> String {
    text.lines().next().unwrap().chars().take(12).collect()
}

#[test]
fn f2_and_z_fill_the_window_and_f2_leaves_keeping_the_keys() {
    for enter in [ch('z'), key(KeyCode::F(2)), ch('я')] {
        let (mut app, screen) = client();
        screen.feed(b"CLIENT");
        app.handle_key(ctrl('z'));
        // From the list, with the keys going to the terminal.
        assert!(!app.pane_wants_zoom());
        assert!(app.handle_key(enter).is_empty(), "{enter:?}");
        assert!(app.pane_wants_zoom());
        assert!(app.pane_has_keys());
        let text = draw(&mut app, 160, 40);
        assert!(top_left(&text).starts_with("CLIENT"), "{text}");
        assert!(text.contains(" ZOOM "), "{text}");
        assert!(!text.contains(" TERMINAL "), "{text}");
        assert!(!text.contains("alpha"), "the bar is gone: {text}");
        // The child gets the new size, once.
        let effects = app.pane_tick(Instant::now());
        assert_eq!(pane_resize(&effects), Some((39, 160)));
        screen.set_size(39, 160);
        assert_eq!(pane_resize(&app.pane_tick(Instant::now())), None);
        // `z` is typed now, and the same key leaves.
        assert_eq!(app.handle_key(ch('z')), [Effect::PaneWrite(b"z".to_vec())]);
        assert_eq!(app.handle_key(ch('q')), [Effect::PaneWrite(b"q".to_vec())]);
        assert!(app.handle_key(key(KeyCode::F(2))).is_empty());
        assert!(!app.pane_wants_zoom());
        assert_eq!(app.pane_focus(), PaneFocus::Terminal);
        let text = draw(&mut app, 160, 40);
        assert!(text.contains(" TERMINAL "), "{text}");
        assert!(text.contains("alpha"), "{text}");
        let effects = app.pane_tick(Instant::now());
        assert_eq!(pane_resize(&effects), Some((37, 130)));
    }
}

#[test]
fn f2_zooms_from_the_terminal_and_ctrl_z_and_f12_return_to_the_list() {
    for back in [ctrl('z'), key(KeyCode::F(12))] {
        let (mut app, _screen) = client();
        assert!(
            app.handle_key(key(KeyCode::F(2))).is_empty(),
            "never the child's"
        );
        assert!(app.pane_wants_zoom());
        draw(&mut app, 160, 40);
        app.pane_tick(Instant::now());
        app.handle_key(back);
        assert!(!app.pane_wants_zoom(), "{back:?}");
        assert_eq!(app.pane_focus(), PaneFocus::List);
        let text = draw(&mut app, 160, 40);
        assert!(text.contains(" LIST "), "{text}");
        let effects = app.pane_tick(Instant::now());
        assert_eq!(pane_resize(&effects), Some((37, 130)));
    }
}

#[test]
fn the_shell_zooms_too_with_its_history_and_selection() {
    let (mut app, screen, _) = shell();
    screen.feed(b"SHELL");
    lines(&screen, 0..100);
    app.handle_key(key(KeyCode::F(2)));
    let text = draw(&mut app, 160, 40);
    assert!(text.contains(" ZOOM "), "{text}");
    let effects = app.pane_tick(Instant::now());
    assert_eq!(pane_resize(&effects), Some((39, 160)));
    screen.set_size(39, 160);
    // The marker is on the footer: there is no title.
    app.handle_mouse(at(MouseEventKind::ScrollUp, 5, 5));
    assert_eq!(app.pane_scrolled(), 3);
    let text = draw(&mut app, 160, 40);
    let footer = text.lines().last().unwrap();
    assert!(footer.contains("↑ 3 lines"), "{footer}");
    assert!(footer.contains("F2 back"), "{footer}");
    // A drag in the window's own cells, now.
    app.handle_mouse(at(MouseEventKind::Down(LEFT), 0, 0));
    app.handle_mouse(at(MouseEventKind::Drag(LEFT), 4, 0));
    let effects = app.handle_mouse(at(MouseEventKind::Up(LEFT), 4, 0));
    assert!(
        matches!(effects.as_slice(), [Effect::PaneCopy(_)]),
        "{effects:?}"
    );
    assert_eq!(app.pane_selection_spans(), vec![(0, 0, 4)]);
    // A press on the footer takes nothing, and does not leave zoom.
    app.handle_mouse(at(MouseEventKind::Down(LEFT), 5, 39));
    assert!(app.pane_wants_zoom());
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    // Leaving ends the selection, as the pane's place changes.
    app.handle_key(key(KeyCode::F(2)));
    draw(&mut app, 160, 40);
    app.pane_tick(Instant::now());
    assert!(app.pane_selection_spans().is_empty());
}

#[test]
fn zoom_needs_a_terminal_and_says_so() {
    let mut app = workspace();
    // A stopped row: `z` wakes nothing, unlike Enter.
    common::select_name(&mut app, "gamma");
    app.pane_tick(Instant::now());
    let effects = app.handle_key(ch('z'));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(!app.pane_wants_zoom());
    assert!(
        status_text(&app).contains("no terminal to zoom"),
        "{}",
        status_text(&app)
    );
    // Home has no row.
    app.handle_key(ch('h'));
    assert!(app.handle_key(ch('z')).is_empty());
    assert!(!app.pane_wants_zoom());
    assert!(status_text(&app).contains("choose a session"));
}

#[test]
fn z_on_a_running_row_with_no_client_yet_attaches_at_once() {
    let mut app = workspace();
    let effects = app.handle_key(ch('z'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::PaneOpen(_))),
        "{effects:?}"
    );
    assert!(app.pane_wants_zoom());
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let text = draw(&mut app, 160, 40);
    assert!(text.contains(" ZOOM "), "{text}");
}

#[test]
fn the_end_of_the_program_leaves_zoom() {
    let (mut app, _screen, generation) = shell();
    app.handle_key(key(KeyCode::F(2)));
    draw(&mut app, 160, 40);
    app.pane_exited(generation, Some(0));
    app.pane_tick(Instant::now());
    assert!(!app.pane_wants_zoom());
    assert_eq!(app.pane_focus(), PaneFocus::List);
    let text = draw(&mut app, 160, 40);
    assert!(text.contains(" LIST "), "{text}");
}

#[test]
fn a_window_draws_over_zoom_and_takes_the_keys() {
    let (mut app, _screen) = client();
    app.handle_key(key(KeyCode::F(2)));
    draw(&mut app, 160, 40);
    app.pane_tick(Instant::now());
    app.open_dialog(ProjectList::default());
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("New session"), "{text}");
    assert!(text.contains(" ZOOM "), "{text}");
    assert!(!app.pane_has_keys());
    let typed = app.handle_key(ch('x'));
    assert!(
        typed.iter().all(|e| !matches!(e, Effect::PaneWrite(_))),
        "{typed:?}"
    );
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('y'));
    assert!(app.dialog().is_none());
    assert!(app.pane_has_keys());
    assert!(app.pane_wants_zoom(), "it was under the window all along");
}

#[test]
fn below_the_workspaces_size_zoom_goes_on_and_can_start() {
    // A zoomed terminal that shrinks stays zoomed.
    let (mut app, _screen) = client();
    app.handle_key(key(KeyCode::F(2)));
    draw(&mut app, 160, 40);
    app.pane_tick(Instant::now());
    let text = draw(&mut app, 80, 20);
    assert!(text.contains(" ZOOM "), "{text}");
    assert!(app.pane_active());
    let effects = app.pane_tick(Instant::now());
    assert_eq!(pane_resize(&effects), Some((19, 80)));
    // Leaving zoom there gives the dashboard, as the size says.
    app.handle_key(key(KeyCode::F(2)));
    let text = draw(&mut app, 80, 20);
    assert!(!text.contains(" ZOOM "), "{text}");
    assert!(!app.pane_active());
    let effects = app.pane_tick(Instant::now());
    assert!(effects.contains(&Effect::PaneClose), "{effects:?}");

    // From that dashboard, `z` on its selected row opens the terminal zoomed.
    common::select_name(&mut app, "beta");
    assert!(app.handle_key(ch('z')).is_empty());
    assert!(app.pane_wants_zoom());
    app.pane_tick(Instant::now());
    let text = draw(&mut app, 80, 20);
    assert!(text.contains(" ZOOM "), "{text}");
    let effects = app.pane_tick(Instant::now());
    let open = effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneOpen(open) => Some(open.clone()),
            _ => None,
        })
        .expect("it attaches");
    assert_eq!(open.id, "bbbb2222");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(app.pane_wants_zoom());

    // A stopped row there is refused without a frame of nothing.
    let mut app = workspace();
    draw(&mut app, 80, 20);
    app.pane_tick(Instant::now());
    common::select_name(&mut app, "gamma");
    assert!(app.handle_key(ch('z')).is_empty());
    assert!(!app.pane_wants_zoom());

    // Too small even for zoom: the fallback stays, and nothing waits.
    let mut app = workspace();
    draw(&mut app, 15, 3);
    app.pane_tick(Instant::now());
    common::select_name(&mut app, "beta");
    app.handle_key(ch('z'));
    assert!(app.pane_wants_zoom());
    draw(&mut app, 15, 3);
    assert!(!app.pane_wants_zoom());
    assert!(!app.pane_active());
}

#[test]
fn z_and_f2_do_nothing_in_the_other_styles() {
    let mut app = workspace();
    app.style = watchtower::config::UiStyle::Dashboard;
    draw(&mut app, 160, 40);
    app.pane_tick(Instant::now());
    for k in [ch('z'), key(KeyCode::F(2))] {
        assert!(app.handle_key(k).is_empty());
        assert!(!app.pane_wants_zoom());
    }
}

#[test]
fn a_size_change_is_due_at_once_after_the_frame_that_made_it() {
    let (mut app, _screen) = client();
    let now = Instant::now();
    app.pane_tick(now);
    assert_ne!(app.pane_wait(now), Some(Duration::ZERO));
    app.handle_key(key(KeyCode::F(2)));
    draw(&mut app, 160, 40);
    assert_eq!(app.pane_wait(now), Some(Duration::ZERO));
    app.pane_tick(now);
    assert_ne!(app.pane_wait(now), Some(Duration::ZERO));
}
