//! Scrolling the logs panel (doc/0028): the focus keys (Latin and
//! Cyrillic), paging and clamping, the paused snapshot, following again,
//! the reset on another row and after an attach, full screen in both
//! styles, and tiny terminals.

mod common;

use std::time::Instant;

use common::{app_single, ch, ctrl, key, poll, select_name};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::{
    App, AttachOutcome, Effect, LogsTarget, LogsView, NOTE_INTERACTIVE, Overlay,
};
use watchtower::config::UiStyle;
use watchtower::logview::{self, LogScreen};

/// `line 1` … `line n` on a 64-row screen: `n - 64` lines of scrollback.
fn numbered(n: usize) -> LogScreen {
    let lines: Vec<String> = (1..=n).map(|i| format!("line {i}")).collect();
    logview::render(format!("\x1b[2J\x1b[H{}", lines.join("\r\n")).as_bytes())
}

fn fetch_of(effects: &[Effect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        Effect::FetchLogs(t) => Some(t.id.clone()),
        _ => None,
    })
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

/// Hands the running fetch its result.
fn deliver(app: &mut App, screen: LogScreen) -> Vec<Effect> {
    let target: LogsTarget = app.logs().unwrap().in_flight().cloned().unwrap();
    app.logs_fetched(target, Ok(screen))
}

fn page(app: &App) -> Vec<String> {
    let panel = app.logs().unwrap();
    let LogsView::Screen(screen) = &panel.view else {
        panic!("{:?}", panel.view);
    };
    screen.page_plain(panel.height(), panel.scroll())
}

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

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

fn cyr(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

// ---- focus -----------------------------------------------------------------

#[test]
fn right_and_l_focus_the_panel_left_h_and_esc_return() {
    let mut app = open_with(numbered(300));
    assert!(!app.logs_focused());
    for (to_logs, back) in [
        (key(KeyCode::Right), key(KeyCode::Left)),
        (ch('l'), ch('h')),
        (key(KeyCode::Right), key(KeyCode::Esc)),
    ] {
        app.handle_key(to_logs);
        assert!(app.logs_focused());
        app.handle_key(back);
        assert!(!app.logs_focused());
        assert!(app.logs().is_some(), "returning keeps the panel open");
    }
    // From the list Esc closes the panel, as before.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.logs().is_none());
    // Without a panel → says how to open one and focuses nothing.
    app.handle_key(key(KeyCode::Right));
    assert!(!app.logs_focused());
    assert!(common::status_text(&app).contains("Space opens it"));
}

#[test]
fn cyrillic_keys_focus_scroll_and_return() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    // д is l, р is h, л / о are k / j, п / П are g / G, а is f.
    app.handle_key(cyr('д'));
    assert!(app.logs_focused());
    app.handle_key(cyr('л'));
    assert_eq!(app.logs().unwrap().scroll(), Some(289));
    app.handle_key(cyr('о'));
    // Back at the bottom: following again.
    assert_eq!(app.logs().unwrap().scroll(), None);
    app.handle_key(cyr('п'));
    assert_eq!(app.logs().unwrap().scroll(), Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Char('П'), KeyModifiers::SHIFT));
    assert_eq!(app.logs().unwrap().scroll(), None);
    app.handle_key(cyr('а'));
    assert!(app.logs().unwrap().full_screen());
    app.handle_key(cyr('р'));
    assert!(!app.logs_focused());
    assert!(!app.logs().unwrap().full_screen());
}

#[test]
fn list_keys_stay_with_the_list_until_focused() {
    let mut app = open_with(numbered(300));
    let before = app.selected_session().unwrap().session.display_name();
    app.handle_key(key(KeyCode::Right));
    // j scrolls the logs, not the list; s does nothing but say so.
    app.handle_key(ch('k'));
    app.handle_key(ch('s'));
    assert!(app.overlay() == &Overlay::None);
    assert!(common::status_text(&app).contains("← or Esc returns to the list"));
    assert_eq!(
        app.selected_session().unwrap().session.display_name(),
        before
    );
    // q and Ctrl+C still quit; ? opens the help, Esc closes only the help.
    assert_eq!(app.handle_key(ch('q')), [Effect::Quit]);
    assert_eq!(app.handle_key(ctrl('c')), [Effect::Quit]);
    app.handle_key(ch('?'));
    assert_eq!(app.overlay(), &Overlay::Help);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.overlay(), &Overlay::None);
    assert!(app.logs_focused());
    // Space closes the panel and gives the keys back to the list.
    app.handle_key(ch(' '));
    assert!(app.logs().is_none());
    assert!(!app.logs_focused());
}

#[test]
fn e_renames_from_the_list_but_not_from_the_focused_panel() {
    // doc/0027 with doc/0028: `e` is a list key, so the focused panel only
    // says how to get back; with the panel open and the list focused it
    // opens the rename window.
    let mut app = open_with(numbered(300));
    app.handle_key(key(KeyCode::Right));
    assert!(app.logs_focused());
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.rename_window().is_none());
    assert!(common::status_text(&app).contains("← or Esc returns to the list"));
    app.handle_key(key(KeyCode::Left));
    assert!(!app.logs_focused());
    assert!(app.handle_key(ch('e')).is_empty());
    let w = app.rename_window().expect("rename window from the list");
    assert_eq!(w.old_name, "print-date");
    assert!(app.logs().is_some(), "the panel stays open");
    // Every list key hint is also offered while the panel is open.
    for hint in watchtower::ui::FOOTER_KEYS {
        assert!(
            watchtower::ui::FOOTER_KEYS_LOGS_OPEN.contains(hint),
            "{hint} missing with the logs panel open"
        );
    }
}

// ---- scrolling -------------------------------------------------------------

#[test]
fn lines_pages_top_and_bottom() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(page(&app).first().unwrap(), "line 291");

    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.logs().unwrap().scroll(), Some(289));
    assert!(app.logs().unwrap().paused());
    assert_eq!(page(&app).first().unwrap(), "line 290");
    app.handle_key(key(KeyCode::PageUp));
    assert_eq!(app.logs().unwrap().scroll(), Some(279));
    app.handle_key(ctrl('u'));
    assert_eq!(app.logs().unwrap().scroll(), Some(269));
    app.handle_key(ctrl('d'));
    assert_eq!(app.logs().unwrap().scroll(), Some(279));
    app.handle_key(key(KeyCode::Home));
    assert_eq!(app.logs().unwrap().scroll(), Some(0));
    assert_eq!(page(&app).first().unwrap(), "line 1");
    // Nothing above the top.
    app.handle_key(ch('k'));
    assert_eq!(app.logs().unwrap().scroll(), Some(0));
    assert_eq!(app.logs().unwrap().position(), Some((1, 10, 300)));
    app.handle_key(key(KeyCode::PageDown));
    app.handle_key(ch('j'));
    assert_eq!(app.logs().unwrap().position(), Some((12, 21, 300)));
}

#[test]
fn the_offset_is_clamped_when_the_panel_resizes() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('k'));
    assert_eq!(app.logs().unwrap().scroll(), Some(289));
    // Taller: the last page starts earlier, the offset follows.
    app.set_logs_height(50);
    assert_eq!(app.logs().unwrap().scroll(), Some(250));
    assert_eq!(page(&app).last().unwrap(), "line 300");
    // Taller than the history: everything fits; still paused until End.
    app.set_logs_height(400);
    assert_eq!(app.logs().unwrap().scroll(), Some(0));
    assert_eq!(page(&app).len(), 300);
    assert!(app.logs().unwrap().paused());
    // Zero rows (a tiny terminal) does not panic.
    app.set_logs_height(0);
    app.handle_key(ch('k'));
    app.handle_key(key(KeyCode::PageDown));
}

#[test]
fn short_output_and_notes_have_nothing_to_scroll() {
    let mut app = open_with(logview::render(b"one\r\ntwo"));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    for k in [
        key(KeyCode::Up),
        key(KeyCode::PageUp),
        ch('g'),
        key(KeyCode::Down),
    ] {
        app.handle_key(k);
        assert_eq!(app.logs().unwrap().scroll(), None);
    }
    // Empty output.
    let mut app = open_with(logview::render(b""));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    assert_eq!(app.logs().unwrap().scroll(), None);
    // A note (interactive row): focus works, scrolling does nothing.
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    select_name(&mut app, "demo-app-3f");
    app.handle_key(ch(' '));
    assert_eq!(
        app.logs().unwrap().view,
        LogsView::Note(NOTE_INTERACTIVE.into())
    );
    app.handle_key(key(KeyCode::Right));
    assert!(app.logs_focused());
    app.handle_key(ch('k'));
    app.handle_key(ch('G'));
    assert_eq!(app.logs().unwrap().scroll(), None);
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("interactive session"), "{all}");
}

// ---- paused and following --------------------------------------------------

#[test]
fn a_paused_panel_keeps_its_snapshot() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    assert!(app.logs().unwrap().paused());

    // The owner's poll does not fetch while paused.
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(fetch_of(&effects), None);
    assert!(app.logs().unwrap().in_flight().is_none());

    // A fetch that was already running does not replace the snapshot.
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.apply_poll(poll("main", "mixed-states.json"));
    assert!(app.logs().unwrap().in_flight().is_some());
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('k'));
    let effects = deliver(&mut app, numbered(900));
    assert!(effects.is_empty());
    assert_eq!(page(&app).first().unwrap(), "line 290");
    assert_eq!(app.logs().unwrap().position(), Some((290, 299, 300)));
}

#[test]
fn end_follows_again_and_fetches_at_once() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    let effects = app.handle_key(key(KeyCode::End));
    assert_eq!(fetch_of(&effects).as_deref(), Some("7b4d9f02"));
    assert!(!app.logs().unwrap().paused());
    // The new output replaces the old as soon as it arrives.
    deliver(&mut app, numbered(900));
    assert_eq!(page(&app).last().unwrap(), "line 900");

    // Scrolling back down to the bottom follows again too.
    app.handle_key(ch('k'));
    app.handle_key(ch('k'));
    assert!(app.logs().unwrap().paused());
    app.handle_key(ch('j'));
    assert!(app.logs().unwrap().paused());
    let effects = app.handle_key(ch('j'));
    assert!(!app.logs().unwrap().paused());
    assert_eq!(fetch_of(&effects).as_deref(), Some("7b4d9f02"));
    // G while following still refreshes.
    deliver(&mut app, numbered(900));
    assert_eq!(
        fetch_of(&app.handle_key(ch('G'))).as_deref(),
        Some("7b4d9f02")
    );
}

#[test]
fn another_row_follows_from_the_bottom() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    app.handle_key(key(KeyCode::Left));
    // Still paused on the same row with the list focused.
    assert!(app.logs().unwrap().paused());
    select_name(&mut app, "say-ok");
    let panel = app.logs().unwrap();
    assert!(!panel.paused());
    assert_eq!(panel.target().unwrap().id, "b2f8d064");
    assert_eq!(panel.view, LogsView::Loading);
}

#[test]
fn returning_from_an_attach_follows_again() {
    let mut app = open_with(numbered(300));
    app.set_logs_height(10);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('g'));
    app.handle_key(ch('f'));
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.logs().unwrap().full_screen(), "Esc leaves full screen");
    let effects = app.attach_finished(AttachOutcome {
        name: "print-date".into(),
        account: "main".into(),
        result: Ok(Some(0)),
    });
    assert!(effects.contains(&Effect::PollNow));
    assert!(!app.logs().unwrap().paused());
    // The poll it asks for refreshes the panel.
    let effects = app.apply_poll(poll("main", "mixed-states.json"));
    assert_eq!(fetch_of(&effects).as_deref(), Some("7b4d9f02"));
    deliver(&mut app, numbered(300));
    let all = text(&render(&mut app, 160, 40));
    assert!(all.contains("line 300"), "{all}");
    assert!(!all.contains("paused"), "{all}");
}

// ---- drawing ---------------------------------------------------------------

#[test]
fn focus_title_and_footer_in_the_minimal_style() {
    let mut app = open_with(numbered(300));
    let lines = render(&mut app, 160, 40);
    let all = text(&lines);
    assert!(all.contains("→ scroll · Space/Esc close"), "{all}");
    assert!(all.contains("→ scroll logs"), "{all}");
    assert!(!all.contains('┏'), "not focused: plain border\n{all}");

    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::PageUp));
    let lines = render(&mut app, 160, 40);
    let all = text(&lines);
    assert!(all.contains('┏'), "focused: thick border\n{all}");
    // Side panel at 160 × 40: title, footer, status and borders take 5.
    let height = app.logs().unwrap().height();
    assert_eq!(height, 35);
    let first = 300 - 2 * height + 1;
    let want = format!(
        "paused · line {first}–{} of 300 · End follows",
        first + height - 1
    );
    assert!(all.contains(&want), "{want}\n{all}");
    assert!(all.contains(&format!("┃line {first}")), "{all}");
    // The footer has the panel's keys.
    let footer = lines.last().unwrap();
    assert!(
        footer.contains("↑↓ scroll") && footer.contains("← list"),
        "{footer}"
    );
    assert!(!footer.contains("n new"), "{footer}");
}

#[test]
fn full_screen_takes_the_body_in_both_styles() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        let mut app = open_with(numbered(300));
        app.style = style;
        let half = text(&render(&mut app, 160, 40));
        assert!(half.contains("print-date"), "{style:?}: list shown\n{half}");
        let half_height = app.logs().unwrap().height();

        app.handle_key(key(KeyCode::Right));
        app.handle_key(ch('f'));
        let lines = render(&mut app, 160, 40);
        let all = text(&lines);
        assert!(app.logs().unwrap().height() >= half_height, "{style:?}");
        // The list is gone; the panel spans the width.
        assert!(!all.contains("say-ok"), "{style:?}\n{all}");
        assert!(all.contains("f/← back"), "{style:?}\n{all}");
        let top = lines
            .iter()
            .find(|l| l.contains("logs · print-date"))
            .unwrap();
        assert!(top.chars().count() >= 159, "{style:?}: {top}");
        // Paused full screen shows the indicator; a resize clamps.
        app.handle_key(ch('g'));
        assert!(text(&render(&mut app, 160, 40)).contains("paused · line 1–"));
        let small = text(&render(&mut app, 80, 24));
        assert!(small.contains("line 1"), "{style:?}\n{small}");
        // f again: back to half the space, still focused.
        app.handle_key(ch('f'));
        assert!(app.logs_focused());
        assert!(text(&render(&mut app, 160, 40)).contains("say-ok"));
    }
}

#[test]
fn dashboard_marks_the_focused_panel() {
    let mut app = open_with(numbered(300));
    app.style = UiStyle::Dashboard;
    let all = text(&render(&mut app, 160, 40));
    assert!(all.contains("→ scroll · Space/Esc close"), "{all}");
    assert!(!all.contains('┏'), "{all}");
    app.handle_key(key(KeyCode::Right));
    let lines = render(&mut app, 160, 40);
    let all = text(&lines);
    assert!(all.contains('┏'), "{all}");
    assert!(lines.last().unwrap().contains("← list"), "{all}");
}

#[test]
fn help_lists_the_panel_keys_from_the_panel() {
    let mut app = open_with(numbered(300));
    app.handle_key(ch('?'));
    let all = text(&render(&mut app, 80, 24));
    assert!(all.contains("New-session dialog"), "{all}");
    assert!(all.contains("→ scrolls it"), "{all}");
    app.handle_key(key(KeyCode::Esc));

    app.handle_key(key(KeyCode::Right));
    app.handle_key(ch('?'));
    let lines = render(&mut app, 80, 24);
    let all = text(&lines);
    assert!(all.contains("Logs panel"), "{all}");
    assert!(!all.contains("New-session dialog"), "{all}");
    for want in [
        "scroll one page",
        "full screen on / off",
        "back to the list",
    ] {
        assert!(all.contains(want), "{want}\n{all}");
    }
    // It fits 24 rows: both borders are on screen.
    assert!(lines.iter().any(|l| l.contains("Esc or ? closes")), "{all}");
}

#[test]
fn tiny_terminals_do_not_panic() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        let mut app = open_with(numbered(300));
        app.style = style;
        for full in [false, true] {
            if full {
                app.handle_key(key(KeyCode::Right));
                app.handle_key(ch('f'));
                app.handle_key(ch('g'));
            }
            for (w, h) in [(1, 1), (2, 2), (10, 3), (20, 5), (40, 8), (80, 24)] {
                render(&mut app, w, h);
                app.handle_key(ch('k'));
                app.handle_key(key(KeyCode::PageDown));
            }
        }
    }
}
