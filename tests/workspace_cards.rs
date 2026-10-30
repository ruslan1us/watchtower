//! The workspace's Home panel, its clicks and wheel on the list, and the cards
//! (doc/0045), without a terminal or a child: [`App`] returns effects and the
//! tests stand in for the runtime, with a clock of their own.

mod common;
mod settings_common;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, key, status_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::{Color, Modifier};
use watchtower::app::{ATTACH_DEBOUNCE, App, Effect, Overlay, PaneFocus, RELEASE_AFTER};
use watchtower::config::{CardSize, Config, ConfigError, UiStyle, UiTheme};
use watchtower::embed::PaneScreen;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::subagents::Subagent;
use watchtower::ui;
use watchtower::usage::{Clock, Limit, LimitKind, PlanUsage, Probed};

const NOW_MS: i64 = 1_800_000_000_000;

const NEUTRAL_AMBER: Color = Color::Rgb(255, 196, 0);
const NEUTRAL_BLUE: Color = Color::Rgb(100, 170, 255);
const NEUTRAL_RED: Color = Color::Rgb(255, 100, 100);
const LIGHT_AMBER: Color = Color::Rgb(138, 90, 0);
const LIGHT_BLUE: Color = Color::Rgb(31, 95, 196);
const NEUTRAL_BAR: Color = Color::Rgb(58, 58, 58);

fn row(id: &str, name: &str, cwd: &str, process: &str, state: &str) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{id}-0000-4000-8000-000000000000",
            "cwd": "{HOME}/workspace/{cwd}", "kind": "background", "name": "{name}",
            "state": "{state}", "startedAt": {NOW_MS} }}"#
    )
}

const IDLE: &str = r#""pid": 4100, "status": "idle","#;
const BUSY: &str = r#""pid": 4101, "status": "busy","#;
const ASKS: &str = r#""pid": 4102, "status": "waiting", "waitingFor": "permission prompt","#;

/// In list order: `asks` in Needs me, highlighted; `reviews` in Ready for
/// review; `builds` in Working; `sleeps` in the folded Stopped & finished;
/// `other-shell` in Interactive.
fn listing() -> String {
    let rows = [
        row("cccc3333", "reviews", "web", IDLE, "done"),
        row("bbbb2222", "builds", "billing-ui", BUSY, "working"),
        row("aaaa1111", "asks", "api", ASKS, "blocked"),
        row("dddd4444", "sleeps", "infra", "", "done"),
        format!(
            r#"{{ "pid": 4200, "cwd": "{HOME}/workspace/demo", "kind": "interactive",
                "startedAt": {NOW_MS}, "sessionId": "eeee-0000", "name": "other-shell" }}"#
        ),
    ];
    format!("[{}]", rows.join(","))
}

fn poll(account: &str, text: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(text).unwrap()),
    }
}

/// An app with `text` listed, its sessions owned by `main`, in the workspace
/// style with `ui` added to `[ui]` and `rest` to the config's end.
fn app_with(text: &str, ui: &str, rest: &str) -> App {
    let config = Config::parse(
        &format!(
            "default_account = \"main\"\n[ui]\nstyle = \"workspace\"\n{ui}\n\
             [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n{rest}\n"
        ),
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
    app.apply_poll(poll("main", text));
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text_of(buf: &Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn draw(app: &mut App, w: u16, h: u16) -> String {
    text_of(&render(app, w, h))
}

/// The first `w` columns of each row: the list bar when it is `w` wide.
fn bar_of(text: &str, w: usize) -> Vec<String> {
    text.lines()
        .map(|line| line.chars().take(w).collect())
        .collect()
}

/// The rows of the bar, on a terminal 160 wide, where it has 28 columns.
fn bar(app: &mut App, h: u16) -> Vec<String> {
    bar_of(&draw(app, 160, h), 28)
}

fn y_of(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in\n{}", lines.join("\n")))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);

fn selected(app: &App) -> Option<String> {
    app.selected_session().map(|m| m.session.display_name())
}

fn opened(effects: &[Effect]) -> bool {
    effects.iter().any(|e| matches!(e, Effect::PaneOpen(_)))
}

/// The workspace drawn on a 160 × 40 terminal, on Home.
fn at_home() -> App {
    let mut app = app_with(&listing(), "", "");
    draw(&mut app, 160, 40);
    assert!(app.pane_home());
    app
}

/// [`at_home`] with `name` chosen and its client open, reported to the app.
fn attached_to(name: &str) -> (App, Instant) {
    let mut app = at_home();
    common::select_name(&mut app, name);
    let t = Instant::now();
    app.pane_tick(t);
    let t = t + ATTACH_DEBOUNCE;
    let effects = app.pane_tick(t);
    let open = effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneOpen(open) => Some(open.clone()),
            _ => None,
        })
        .expect("attached after the debounce");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    (app, t)
}

// ---- Home -------------------------------------------------------------------

#[test]
fn the_start_attaches_nothing_and_shows_home_with_no_row_selected() {
    let mut app = at_home();
    assert_eq!(app.selected_entry(), None);
    assert_eq!(selected(&app), None);
    let t = Instant::now();
    for later in [
        Duration::ZERO,
        ATTACH_DEBOUNCE,
        Duration::from_secs(5),
        RELEASE_AFTER * 2,
    ] {
        assert!(app.pane_tick(t + later).is_empty(), "{later:?}");
        assert_eq!(app.pane_wait(t + later), None, "no timer on Home");
    }
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Home"), "{text}");
    assert!(
        text.contains(&format!("◆ tower {}", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    assert!(!text.contains('┏'), "no card is selected: {text}");
    assert!(!text.contains("view only"), "{text}");
    assert!(text.contains("↑↓/click select"), "the footer: {text}");
    assert!(!text.contains("Enter type here"), "{text}");
}

#[test]
fn the_first_arrow_selects_the_first_row_and_it_attaches_after_the_rest() {
    for first in [
        key(KeyCode::Down),
        key(KeyCode::Up),
        ch('j'),
        ch('k'),
        key(KeyCode::PageDown),
    ] {
        let mut app = at_home();
        app.handle_key(first);
        assert!(!app.pane_home(), "{first:?}");
        assert_eq!(selected(&app).as_deref(), Some("asks"), "{first:?}");
        assert!(draw(&mut app, 160, 40).contains('┏'), "{first:?}");
        let t = Instant::now();
        assert!(!opened(&app.pane_tick(t)), "not before the rest");
        assert!(opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
    }
    // The next arrow moves on, as always.
    let mut app = at_home();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_ne!(selected(&app).as_deref(), Some("asks"));
    // The absolute keys go where they say.
    let mut app = at_home();
    app.handle_key(key(KeyCode::Home));
    assert!(!app.pane_home());
    assert_eq!(app.selected_entry(), Some(0));
    let mut app = at_home();
    app.handle_key(key(KeyCode::End));
    assert_eq!(selected(&app).as_deref(), Some("other-shell"));
}

#[test]
fn the_first_click_on_a_card_selects_it() {
    let mut app = at_home();
    let lines = bar(&mut app, 40);
    let y = y_of(&lines, "builds") as u16;
    app.handle_mouse(mouse(DOWN, 4, y + 1));
    assert!(!app.pane_home());
    assert_eq!(selected(&app).as_deref(), Some("builds"));
}

#[test]
fn row_keys_do_nothing_on_home() {
    for code in [
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Char('s'),
        KeyCode::Char('d'),
        KeyCode::Char('a'),
        KeyCode::Char('e'),
        KeyCode::Char('t'),
        KeyCode::Char('m'),
        KeyCode::Char('!'),
    ] {
        let mut app = at_home();
        let before = app.entries().len();
        let effects = app.handle_key(key(code));
        assert!(effects.is_empty(), "{code:?}: {effects:?}");
        assert_eq!(*app.overlay(), Overlay::None, "{code:?}");
        assert_eq!(app.entries().len(), before, "{code:?} folded a group");
        assert!(app.pane_home(), "{code:?}");
    }
    let mut app = at_home();
    app.handle_key(key(KeyCode::Enter));
    assert!(status_text(&app).contains("choose a session first"));
}

#[test]
fn h_goes_home_closes_nothing_and_the_next_arrow_comes_back() {
    let (mut app, t) = attached_to("builds");
    app.handle_key(ch('h'));
    assert!(app.pane_home());
    assert_eq!(selected(&app), None);
    let effects = app.pane_tick(t);
    assert!(effects.is_empty(), "{effects:?}");
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE * 4);
    assert!(effects.is_empty(), "the client stays: {effects:?}");
    assert_eq!(app.pane_wait(t), None);
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Home"), "{text}");
    assert!(!text.contains('┏'), "{text}");

    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected(&app).as_deref(), Some("builds"), "where it was");
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE * 8);
    assert!(effects.is_empty(), "the same client: {effects:?}");
    assert!(draw(&mut app, 160, 40).contains("view only"));
}

#[test]
fn another_row_chosen_from_home_replaces_the_client() {
    let (mut app, t) = attached_to("builds");
    app.handle_key(ch('h'));
    app.pane_tick(t);
    // Back on `builds`, then over its group's header to the row above.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(selected(&app).as_deref(), Some("reviews"));
    app.pane_tick(t);
    assert!(opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
}

#[test]
fn the_release_rule_still_runs_on_home() {
    let (mut app, t) = attached_to("builds");
    app.handle_key(ch('h'));
    app.pane_tick(t);
    let effects = app.pane_tick(t + RELEASE_AFTER);
    assert!(effects.contains(&Effect::PaneClose), "{effects:?}");
    assert!(app.pane_home());
}

#[test]
fn a_click_on_tower_goes_home_from_the_terminal_too() {
    let (mut app, t) = attached_to("builds");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    app.handle_mouse(mouse(DOWN, 3, 0));
    assert!(app.pane_home());
    assert_eq!(app.pane_focus(), PaneFocus::List);
    // The pane shows Home, so a press there reaches nothing.
    let effects = app.handle_mouse(mouse(DOWN, 60, 5));
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(app.pane_tick(t).is_empty());
}

#[test]
fn h_works_on_the_russian_layout_and_only_in_the_workspace() {
    let mut app = at_home();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(ch('р'));
    assert!(app.pane_home());
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = app_with(&listing(), "", "");
        app.style = style;
        draw(&mut app, 200, 50);
        let before = selected(&app);
        assert!(before.is_some(), "{style:?} starts on a row");
        assert!(app.handle_key(ch('h')).is_empty());
        assert_eq!(selected(&app), before, "{style:?}");
    }
}

#[test]
fn a_narrow_terminal_has_the_selection_and_home_comes_back_with_the_width() {
    let mut app = app_with(&listing(), "", "");
    let text = draw(&mut app, 90, 30);
    assert!(text.contains("Sessions"), "the dashboard: {text}");
    assert!(selected(&app).is_some());
    assert!(!app.pane_home());
    assert!(app.pane_tick(Instant::now()).is_empty());
    assert!(draw(&mut app, 160, 40).contains("Home"));
    assert!(app.pane_home());
}

#[test]
fn switching_to_the_style_starts_on_home() {
    let mut app = app_with(&listing(), "", "");
    app.style = UiStyle::Dashboard;
    draw(&mut app, 160, 40);
    app.handle_key(key(KeyCode::Down));
    let t = Instant::now();
    app.pane_tick(t);
    app.style = UiStyle::Workspace;
    assert!(draw(&mut app, 160, 40).contains("Home"));
    assert!(app.pane_home());
    assert!(!opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
}

#[test]
fn home_lists_the_counts_and_the_sessions_that_need_attention() {
    let mut app = at_home();
    let text = draw(&mut app, 160, 40);
    let line = text
        .lines()
        .find(|l| l.contains("needs you ·"))
        .unwrap_or_else(|| panic!("{text}"));
    for piece in [
        "1 needs you",
        "1 ready",
        "1 working",
        "1 stopped",
        "1 interactive",
    ] {
        assert!(line.contains(piece), "{line}");
    }
    let attention = text
        .lines()
        .find(|l| l.contains("● asks"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(attention.contains("main"), "{attention}");
    assert!(!text.contains("Plan usage"), "off by default: {text}");
    for key in ["↑↓ or click", "Enter", "Ctrl+Z, F12", "new session", "help"] {
        assert!(text.contains(key), "{key}: {text}");
    }
    // Seen: not listed, and the counts stay.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(ch('m'));
    app.handle_key(ch('h'));
    let text = draw(&mut app, 160, 40);
    assert!(!text.contains("● asks"), "{text}");
    assert!(text.contains("Nothing needs you."), "{text}");
    assert!(text.contains("1 needs you"), "{text}");
}

#[test]
fn home_has_the_state_colours_and_says_when_nothing_is_listed() {
    let mut app = at_home();
    let buf = render(&mut app, 160, 40);
    let lines: Vec<String> = text_of(&buf).lines().map(str::to_owned).collect();
    let y = y_of(&lines, "1 needs you ·");
    let x = lines[y]
        .split("1 needs you")
        .next()
        .unwrap()
        .chars()
        .count();
    assert_eq!(buf[(x as u16, y as u16)].symbol(), "1");
    assert_eq!(buf[(x as u16, y as u16)].fg, NEUTRAL_AMBER);

    let mut app = app_with("[]", "", "");
    assert!(draw(&mut app, 160, 40).contains("No sessions yet"));
}

#[test]
fn a_long_attention_list_gives_way_to_the_keys() {
    let rows: Vec<String> = (0..40)
        .map(|k| {
            row(
                &format!("a{k:07}"),
                &format!("ask-{k}"),
                "api",
                ASKS,
                "blocked",
            )
        })
        .collect();
    let mut app = app_with(&format!("[{}]", rows.join(",")), "", "");
    let text = draw(&mut app, 120, 24);
    assert!(text.contains("● ask-0"), "{text}");
    assert!(text.contains(" more"), "{text}");
    assert!(text.contains("Ctrl+Z, F12"), "{text}");
    assert!(text.contains("help"), "{text}");
}

fn plan(session: f64, weekly: f64) -> Probed {
    let limit = |kind: &str, percent: f64| Limit {
        kind: LimitKind::from(kind),
        group: None,
        percent: Some(percent),
        resets_at_ms: None,
        model: None,
        surface: None,
        severity: Some("normal".to_owned()),
        active: None,
    };
    Probed {
        usage: Some(PlanUsage {
            limits: vec![limit("session", session), limit("weekly_all", weekly)],
            extra: None,
        }),
        ..Probed::default()
    }
}

#[test]
fn home_shows_each_accounts_plan_usage_while_it_is_on() {
    let alt = "[[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n\
               config_dir = \"~/.claude-alt\"\n";
    let mut app = app_with(&listing(), "", &format!("{alt}[usage]\nshow = true\n"));
    app.clock = Clock::UTC;
    let t = Instant::now();
    app.usage_received("main", Ok(plan(24.0, 12.0)), t, NOW_MS);
    app.usage_received("alt", Ok(plan(5.0, 30.0)), t, NOW_MS);
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Plan usage"), "{text}");
    let at = |name: &str| {
        text.lines()
            .find(|l| l.contains(&format!(" {name} ")) && l.contains("5h"))
            .unwrap_or_else(|| panic!("{name}: {text}"))
            .to_owned()
    };
    assert!(at("main").contains("5h 24% · 7d 12%"), "{}", at("main"));
    assert!(at("alt").contains("5h 5% · 7d 30%"), "{}", at("alt"));
    // Off: the same config without it.
    let mut app = app_with(&listing(), "", alt);
    let text = draw(&mut app, 160, 40);
    assert!(!text.contains("Plan usage"), "{text}");
    assert!(!text.contains("5h"), "{text}");
}

// ---- clicks and the wheel on the list ------------------------------------------

#[test]
fn a_click_on_a_group_header_selects_it_and_folds_and_unfolds_it() {
    let mut app = at_home();
    let lines = bar(&mut app, 40);
    let y = y_of(&lines, "Working") as u16;
    assert!(lines[y as usize].contains('▾'), "{}", lines[y as usize]);
    assert!(lines.iter().any(|l| l.contains("builds")));

    assert!(app.handle_mouse(mouse(DOWN, 10, y)).is_empty());
    assert!(!app.pane_home());
    assert_eq!(app.selected_header().map(|g| g.title()), Some("Working"));
    let lines = bar(&mut app, 40);
    assert!(lines[y as usize].contains('▸'), "{}", lines[y as usize]);
    assert!(!lines.iter().any(|l| l.contains("builds")), "folded");

    app.handle_mouse(mouse(DOWN, 10, y));
    assert_eq!(app.selected_header().map(|g| g.title()), Some("Working"));
    let lines = bar(&mut app, 40);
    assert!(lines[y as usize].contains('▾'), "{}", lines[y as usize]);
    assert!(lines.iter().any(|l| l.contains("builds")), "open again");

    // A group that starts folded opens the same way.
    let y = y_of(&lines, "Stopped") as u16;
    app.handle_mouse(mouse(DOWN, 2, y));
    assert!(bar(&mut app, 40).iter().any(|l| l.contains("sleeps")));
}

#[test]
fn a_click_on_a_group_says_why_not_while_a_filter_keeps_it_open() {
    let mut app = at_home();
    app.handle_key(ch('/'));
    app.handle_key(ch('w'));
    app.handle_key(key(KeyCode::Enter));
    let lines = bar(&mut app, 40);
    let y = y_of(&lines, "Working") as u16;
    app.handle_mouse(mouse(DOWN, 10, y));
    assert!(status_text(&app).contains("groups stay open"));
    assert!(bar(&mut app, 40).iter().any(|l| l.contains("builds")));
}

#[test]
fn the_wheel_over_the_list_moves_through_it() {
    let mut app = at_home();
    let wheel = |app: &mut App, kind| app.handle_mouse(mouse(kind, 5, 10));
    // On Home the first notch selects, as the first arrow does.
    wheel(&mut app, MouseEventKind::ScrollDown);
    assert_eq!(selected(&app).as_deref(), Some("asks"));
    let at = app.selected_entry().unwrap();
    wheel(&mut app, MouseEventKind::ScrollDown);
    assert_eq!(app.selected_entry(), Some(at + 1));
    wheel(&mut app, MouseEventKind::ScrollUp);
    assert_eq!(app.selected_entry(), Some(at));
    // At the ends it stops.
    for _ in 0..50 {
        wheel(&mut app, MouseEventKind::ScrollDown);
    }
    assert_eq!(selected(&app).as_deref(), Some("other-shell"));
    // Sideways and over the pane it moves nothing here.
    let before = app.selected_entry();
    wheel(&mut app, MouseEventKind::ScrollLeft);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 60, 10));
    assert_eq!(app.selected_entry(), before);
}

#[test]
fn the_wheel_over_the_list_gives_it_the_keys_and_sends_nothing_to_the_child() {
    let (mut app, _t) = attached_to("builds");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let effects = app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 10));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PaneWrite(w) if !w.starts_with(b"\x1b[O"))),
        "{effects:?}"
    );
}

#[test]
fn a_window_takes_clicks_and_the_wheel_away_from_the_list() {
    let mut app = at_home();
    app.handle_key(ch('?'));
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 10));
    app.handle_mouse(mouse(DOWN, 5, 0));
    app.handle_mouse(mouse(DOWN, 5, 3));
    assert!(app.pane_home());
    assert_eq!(selected(&app), None);
}

#[test]
fn a_click_on_any_row_of_a_card_selects_it_at_every_size() {
    for (size, rows) in [
        (CardSize::Large, 4),
        (CardSize::Medium, 3),
        (CardSize::Small, 2),
    ] {
        let ui = format!("card_size = \"{}\"", size.as_str());
        let mut app = app_with(&listing(), &ui, "");
        let lines = bar(&mut app, 40);
        let top = y_of(&lines, "builds") as u16;
        for y in top..top + rows {
            for x in [0, 13, 27] {
                app.handle_key(ch('h'));
                app.handle_mouse(mouse(DOWN, x, y));
                assert_eq!(
                    selected(&app).as_deref(),
                    Some("builds"),
                    "{size:?} at {x},{y}"
                );
            }
        }
        // The rows around it are not its own.
        app.handle_key(ch('h'));
        app.handle_mouse(mouse(DOWN, 5, top - 1));
        assert_eq!(app.selected_header().map(|g| g.title()), Some("Working"));
        app.handle_mouse(mouse(DOWN, 5, top - 1));
        let lines = bar(&mut app, 40);
        assert_eq!(y_of(&lines, "builds") as u16, top);
        app.handle_mouse(mouse(DOWN, 5, top + rows));
        assert_ne!(selected(&app).as_deref(), Some("builds"), "{size:?}");
    }
}

#[test]
fn a_selected_card_does_not_grow_or_move_the_list() {
    for size in ["large", "medium", "small"] {
        let mut app = app_with(&listing(), &format!("card_size = \"{size}\""), "");
        let names = ["asks", "reviews", "builds", "other-shell"];
        // Selecting by name opens the folded group on the way.
        common::select_name(&mut app, "other-shell");
        let lines = bar(&mut app, 40);
        let at: Vec<usize> = names.iter().map(|n| y_of(&lines, n)).collect();
        for name in names {
            common::select_name(&mut app, name);
            let lines = bar(&mut app, 40);
            let now: Vec<usize> = names.iter().map(|n| y_of(&lines, n)).collect();
            assert_eq!(now, at, "{size} with {name} selected");
        }
    }
}

// ---- cards ----------------------------------------------------------------------

/// The cells of the card whose title holds `name`, on a terminal 160 wide.
fn card_rows(buf: &Buffer, name: &str, rows: usize) -> Vec<(u16, String)> {
    let lines: Vec<String> = bar_of(&text_of(buf), 28);
    let top = y_of(&lines, name);
    (top..top + rows)
        .map(|y| (y as u16, lines[y].clone()))
        .collect()
}

#[test]
fn medium_cards_have_the_name_on_top_state_and_folder_then_account_and_age() {
    let mut app = app_with(&listing(), "", "");
    let lines = bar(&mut app, 40);
    let top = y_of(&lines, "builds");
    let (a, b, c) = (&lines[top], &lines[top + 1], &lines[top + 2]);
    assert!(a.starts_with("╭─ ") && a.ends_with('╮'), "{a}");
    assert!(a.contains(" builds ─"), "{a}");
    assert!(
        b.starts_with("│ working") && b.ends_with("…/billing-ui │"),
        "{b}"
    );
    assert!(c.starts_with("╰─ main ─") && c.ends_with(" ─╯"), "{c}");
    assert!(c.contains(" now "), "{c}");
    assert!(!lines.join("\n").contains("git"), "cards show no git");
}

#[test]
fn large_cards_give_the_state_and_the_folder_their_own_rows() {
    let mut app = app_with(&listing(), "card_size = \"large\"", "");
    let lines = bar(&mut app, 40);
    let top = y_of(&lines, "builds");
    assert!(lines[top].starts_with("╭─"), "{}", lines[top]);
    let (b, c, d) = (&lines[top + 1], &lines[top + 2], &lines[top + 3]);
    assert!(b.starts_with("│ working") && b.ends_with(" now │"), "{b}");
    assert!(c.starts_with("│ ~/workspace/billing-ui"), "{c}");
    assert!(d.starts_with("╰─ main ─") && d.ends_with("─╯"), "{d}");
    assert!(!d.contains("now"), "{d}");
}

#[test]
fn small_cards_have_two_rows_a_coloured_edge_and_no_outline() {
    let mut app = app_with(&listing(), "card_size = \"small\"", "");
    let buf = render(&mut app, 160, 40);
    let rows = card_rows(&buf, "builds", 2);
    let (y, top) = &rows[0];
    assert!(top.starts_with("▌ "), "{top}");
    assert!(top.trim_end().ends_with("now"), "{top}");
    assert_eq!(buf[(0, *y)].fg, NEUTRAL_BLUE);
    let (_, below) = &rows[1];
    assert!(below.starts_with("▌ working"), "{below}");
    assert!(below.ends_with("main"), "{below}");
    let bar = bar_of(&text_of(&buf), 28).join("\n");
    assert!(!bar.contains('╭') && !bar.contains('┏'), "{bar}");
}

#[test]
fn every_size_is_drawn_in_dark_light_and_without_colour() {
    let looks = [
        ("dark", UiTheme::Neutral, true, Some(NEUTRAL_BLUE)),
        ("light", UiTheme::NeutralLight, true, Some(LIGHT_BLUE)),
        ("ansi16", UiTheme::Ansi16, true, Some(Color::Blue)),
        ("no-color", UiTheme::Neutral, false, None),
    ];
    for (size, rows) in [("large", 4), ("medium", 3), ("small", 2)] {
        for (look, theme, color, blue) in looks {
            let mut app = app_with(&listing(), &format!("card_size = \"{size}\""), "");
            app.set_theme(theme);
            app.color = color;
            common::select_name(&mut app, "reviews");
            let buf = render(&mut app, 160, 40);
            let at = format!("{size} {look}");
            let card = card_rows(&buf, "builds", rows);
            let edge = &buf[(0, card[0].0)];
            match blue {
                Some(blue) => assert_eq!(edge.fg, blue, "{at}"),
                None => assert_eq!(edge.fg, Color::Reset, "{at}"),
            }
            // The selected card shows without colour: heavy, or a marker.
            let sel = card_rows(&buf, "reviews", rows);
            let first = sel[0].1.chars().next().unwrap();
            let want = match (size, color) {
                ("small", true) => '█',
                ("small", false) => '▶',
                _ => '┏',
            };
            assert_eq!(first, want, "{at}: {}", sel[0].1);
            if size != "small" {
                let last = &sel[rows - 1].1;
                assert!(last.starts_with('┗') && last.ends_with('┛'), "{at}: {last}");
                assert!(sel[1].1.starts_with('┃'), "{at}: {}", sel[1].1);
            }
            // The attention marker and bold need no colour (doc/0007).
            let lines = bar_of(&text_of(&buf), 28);
            let asks = y_of(&lines, "asks");
            assert!(lines[asks].contains("● ▪ asks"), "{at}: {}", lines[asks]);
            let x = lines[asks].chars().position(|c| c == 'a').unwrap() as u16;
            assert!(
                buf[(x, asks as u16)].modifier.contains(Modifier::BOLD),
                "{at}"
            );
            // Interactive rows are dim, with a dashed outline where there is one.
            let shell = y_of(&lines, "other-shell");
            let dashed = lines[shell].contains('┄') || lines[shell].starts_with('┆');
            assert!(dashed, "{at}: {}", lines[shell]);
        }
    }
}

#[test]
fn the_outline_has_the_states_colour_in_the_theme() {
    let mut app = app_with(&listing(), "", "");
    let buf = render(&mut app, 160, 40);
    for (name, colour) in [("builds", NEUTRAL_BLUE), ("asks", NEUTRAL_AMBER)] {
        let (y, _) = card_rows(&buf, name, 3)[0];
        for (x, dy) in [(0, 0), (27, 0), (1, 0), (0, 1), (27, 1), (0, 2), (27, 2)] {
            assert_eq!(buf[(x, y + dy)].fg, colour, "{name} at {x},{dy}");
        }
    }
    // The marker stays red on the amber card.
    let (y, top) = &card_rows(&buf, "asks", 3)[0];
    let x = top.chars().position(|c| c == '●').unwrap();
    assert_eq!(buf[(x as u16, *y)].fg, NEUTRAL_RED);
    let mut app = app_with(&listing(), "", "");
    app.set_theme(UiTheme::NeutralLight);
    let buf = render(&mut app, 160, 40);
    let (y, _) = card_rows(&buf, "asks", 3)[0];
    assert_eq!(buf[(0, y)].fg, LIGHT_AMBER);
}

#[test]
fn the_selection_bar_is_on_the_title_only_while_the_list_has_the_keys() {
    let (mut app, _t) = attached_to("builds");
    let on_bar = |buf: &Buffer, y: u16, x: u16| buf[(x, y)].bg == NEUTRAL_BAR;
    let buf = render(&mut app, 160, 40);
    let rows = card_rows(&buf, "builds", 3);
    let y = rows[0].0;
    assert!(rows[0].1.starts_with("┏━"), "{}", rows[0].1);
    assert!(on_bar(&buf, y, 5), "the name is on the bar");
    assert!(on_bar(&buf, y, 20), "the run after it too");
    assert!(!on_bar(&buf, y + 1, 5), "only the title");
    assert_eq!(buf[(1, y)].fg, NEUTRAL_BLUE, "the run keeps its colour");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let buf = render(&mut app, 160, 40);
    let rows = card_rows(&buf, "builds", 3);
    assert!(rows[0].1.starts_with("┏━"), "still heavy: {}", rows[0].1);
    assert!(!on_bar(&buf, y, 5), "the terminal has the keys");
    assert!(
        text_of(&buf).starts_with(" ◆ tower"),
        "no ▌ on the title row"
    );

    // Without colour it is reverse video.
    let (mut app, _t) = attached_to("builds");
    app.color = false;
    let buf = render(&mut app, 160, 40);
    assert!(buf[(5, y)].modifier.contains(Modifier::REVERSED));
    assert!(!buf[(5, y + 1)].modifier.contains(Modifier::REVERSED));
}

#[test]
fn the_small_cards_bar_covers_both_rows_and_its_edge_stays_visible() {
    let mut app = app_with(&listing(), "card_size = \"small\"", "");
    common::select_name(&mut app, "builds");
    let buf = render(&mut app, 160, 40);
    let rows = card_rows(&buf, "builds", 2);
    for (y, text) in &rows {
        assert!(text.starts_with('█'), "{text}");
        assert_eq!(buf[(5, *y)].bg, NEUTRAL_BAR, "{text}");
        assert_ne!(buf[(0, *y)].bg, NEUTRAL_BAR, "{text}");
    }
    app.color = false;
    let buf = render(&mut app, 160, 40);
    for (y, text) in card_rows(&buf, "builds", 2) {
        assert!(text.starts_with('▶'), "{text}");
        assert!(buf[(5, y)].modifier.contains(Modifier::REVERSED), "{text}");
        assert!(!buf[(0, y)].modifier.contains(Modifier::REVERSED), "{text}");
    }
}

#[test]
fn the_subagent_badge_sits_on_the_title_and_child_lines_are_left_out() {
    let id = "bbbb2222-0000-4000-8000-000000000000";
    let agents = (0..3)
        .map(|k| Subagent {
            id: format!("agent{k}"),
            agent_type: "general-purpose".into(),
            description: Some("a task".into()),
            started_ms: NOW_MS,
        })
        .collect::<Vec<_>>();
    for (size, top_end) in [("large", " +3 ─╮"), ("medium", " +3 ─╮")] {
        let mut app = app_with(&listing(), &format!("card_size = \"{size}\""), "");
        app.subagents_read(HashMap::from([(id.to_owned(), agents.clone())]));
        let lines = bar(&mut app, 40);
        let top = &lines[y_of(&lines, "builds")];
        assert!(top.ends_with(top_end), "{size}: {top}");
    }
    let mut app = app_with(&listing(), "card_size = \"small\"", "");
    app.subagents_read(HashMap::from([(id.to_owned(), agents.clone())]));
    let lines = bar(&mut app, 40);
    let top = &lines[y_of(&lines, "builds")];
    assert!(top.ends_with("+3 now"), "{top}");
    // No child lines: a card keeps its rows.
    assert!(!lines.iter().any(|l| l.contains("a task")));
    // Narrow, with a long name: both stay, the name cut.
    let long = row(
        "bbbb2222",
        "a-long-name-here",
        "billing-ui",
        BUSY,
        "working",
    );
    let mut app = app_with(&format!("[{long}]"), "list_width = 20", "");
    app.subagents_read(HashMap::from([(id.to_owned(), agents)]));
    let text = draw(&mut app, 160, 40);
    let lines = bar_of(&text, 20);
    let top = &lines[y_of(&lines, "a-long")];
    assert_eq!(top.chars().count(), 20, "{top}");
    assert!(top.contains('…') && top.contains(" +3 "), "{top}");
}

/// Every word on the row is in `whole`, or is cut and ends in `…`.
fn cut_only_with_an_ellipsis(row: &str, whole: &str) {
    for word in row.split(['│', '┃', '┆', ' ', '─', '━', '┄', '╮', '┓', '╯', '┛'])
    {
        let word = word.trim_start_matches(['╭', '┏', '╰', '┗', '▌', '█', '▶', '┆']);
        if word.is_empty() || word.ends_with('…') {
            continue;
        }
        let word = word.trim_start_matches("…/");
        assert!(whole.contains(word), "{word:?} cut without …: {row}");
    }
}

#[test]
fn long_names_states_and_folders_are_fitted_at_every_width() {
    let name = "a-very-long-session-name-that-goes-on-for-a-while";
    let folder = "billing-platform-frontend-with-a-very-long-folder-name";
    let live = r#""pid": 4100, "status": "waiting", "waitingFor": "permission prompt","#;
    let text = format!("[{}]", row("aaaa1111", name, folder, live, "blocked"));
    let whole = format!("{name} permission prompt ~/workspace/{folder} main now ● ▪ Needs me 1");
    for size in ["large", "medium", "small"] {
        for width in [20, 28, 40, 60] {
            let ui = format!("card_size = \"{size}\"\nlist_width = {width}");
            let mut app = app_with(&text, &ui, "");
            common::select_name(&mut app, name);
            let screen = draw(&mut app, 200, 40);
            let lines = bar_of(&screen, width);
            let top = y_of(&lines, "a-very");
            let rows = CardSize::parse(size).unwrap().rows();
            for r in &lines[top..top + rows] {
                assert_eq!(r.chars().count(), width, "{size} {width}: {r}");
                cut_only_with_an_ellipsis(r, &whole);
                // The outline closes on its last cell.
                if size != "small" {
                    let last = r.chars().last().unwrap();
                    assert!("┓┃┛".contains(last), "{size} {width}: {r}");
                }
            }
        }
    }
    // Short forms before cuts: the folder, then the state.
    let mut app = app_with(&text, "", "");
    let lines = bar(&mut app, 40);
    let top = y_of(&lines, "a-very");
    assert!(lines[top].contains('…'), "{}", lines[top]);
    assert!(lines[top + 1].contains("permission"), "{}", lines[top + 1]);
    assert!(lines[top + 1].contains("…/billing"), "{}", lines[top + 1]);
}

#[test]
fn the_bars_title_row_says_what_is_hidden_and_who_has_the_keys() {
    let mut app = app_with(&listing(), "", "");
    let lines = bar(&mut app, 40);
    assert!(lines[0].starts_with("▌◆ tower"), "{}", lines[0]);
    assert!(lines[0].contains("4 of 5"), "{}", lines[0]);
    let mut app = app_with(&listing(), "list_width = 60", "");
    let text = draw(&mut app, 160, 40);
    let top = &bar_of(&text, 60)[0];
    assert!(top.contains("by state"), "{top}");
    assert!(top.contains("4 of 5 shown"), "{top}");
    app.handle_key(ch('p'));
    let text = draw(&mut app, 160, 40);
    assert!(bar_of(&text, 60)[0].contains("by project"));
}

// ---- the viewport -------------------------------------------------------------------

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|k| {
            row(
                &format!("b{k:07}"),
                &format!("row-{k:02}"),
                "demo",
                BUSY,
                "working",
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

#[test]
fn the_viewport_keeps_the_selected_card_whole_and_scrolls_by_cards() {
    for (size, rows) in [("large", 4usize), ("medium", 3), ("small", 2)] {
        let mut app = app_with(&many(40), &format!("card_size = \"{size}\""), "");
        draw(&mut app, 160, 30);
        app.handle_key(key(KeyCode::Down));
        for _ in 0..39 {
            let lines = bar(&mut app, 30);
            let name = selected(&app).unwrap();
            let top = y_of(&lines, &name);
            assert!(top + rows <= 29, "{size}: {name} at {top}");
            let bottom = &lines[top + rows - 1];
            if size != "small" {
                assert!(lines[top].starts_with('┏'), "{size}: {}", lines[top]);
                assert!(bottom.starts_with('┗'), "{size}: {bottom}");
            }
            // Below the title row the list starts with a whole entry and
            // ends with one: no card without its top or bottom edge.
            if size != "small" {
                let opens = lines.iter().filter(|l| l.starts_with(['╭', '┏'])).count();
                let closes = lines.iter().filter(|l| l.starts_with(['╰', '┗'])).count();
                assert_eq!(opens, closes, "{size}\n{}", lines.join("\n"));
            }
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(selected(&app).as_deref(), Some("row-39"), "{size}");
        // A page moves about what shows, and up again stays whole.
        for _ in 0..8 {
            app.handle_key(key(KeyCode::PageUp));
            let lines = bar(&mut app, 30);
            if let Some(name) = selected(&app) {
                let top = y_of(&lines, &name);
                assert!(top + rows <= 29, "{size}: {name} at {top}");
            }
        }
    }
}

#[test]
fn clicks_follow_the_scrolled_list() {
    let mut app = app_with(&many(40), "", "");
    draw(&mut app, 160, 30);
    app.handle_key(key(KeyCode::End));
    let lines = bar(&mut app, 30);
    let y = y_of(&lines, "row-36");
    for dy in 0..3 {
        app.handle_key(key(KeyCode::End));
        app.handle_mouse(mouse(DOWN, 3, (y + dy) as u16));
        assert_eq!(selected(&app).as_deref(), Some("row-36"), "row {dy}");
    }
    // Rows no card fills select nothing.
    let mut app = app_with(&listing(), "", "");
    draw(&mut app, 160, 40);
    app.handle_mouse(mouse(DOWN, 3, 38));
    assert!(app.pane_home());
}

// ---- `[ui] card_size` -------------------------------------------------------------------

#[test]
fn card_size_parses_defaults_to_medium_and_refuses_the_rest() {
    let parse = |text: &str| Config::parse(text, Path::new(HOME));
    assert_eq!(parse("").unwrap().ui_card_size, CardSize::Medium);
    assert_eq!(CardSize::default(), CardSize::Medium);
    for size in CardSize::ALL {
        let text = format!("[ui]\ncard_size = \"{}\"\n", size.as_str());
        assert_eq!(parse(&text).unwrap().ui_card_size, size);
    }
    assert_eq!(CardSize::Large.rows(), 4);
    assert_eq!(CardSize::Medium.rows(), 3);
    assert_eq!(CardSize::Small.rows(), 2);
    let err = parse("[ui]\ncard_size = \"huge\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
    let message = err.to_string();
    assert!(message.contains("[ui] card_size `huge`"), "{message}");
    assert!(message.contains("\"medium\""), "{message}");
}

#[test]
fn the_settings_window_cycles_and_saves_the_card_size() {
    use settings_common::{app_for, env, file, open, press, select};
    use watchtower::config_edit::{SettingKey, SettingValue};

    let env = env(Some(
        "[ui]\nstyle = \"workspace\"\n\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
    ));
    let mut app = app_for(&env);
    open(&mut app);
    select(&mut app, "card size");
    let value = |app: &App| {
        app.settings()
            .unwrap()
            .session
            .value(SettingKey::UiCardSize)
    };
    let text = |s: &str| SettingValue::Text(s.to_owned());
    assert_eq!(value(&app), text("medium"));
    press(&mut app, settings_common::key(KeyCode::Right));
    assert_eq!(value(&app), text("small"));
    press(&mut app, settings_common::key(KeyCode::Left));
    press(&mut app, settings_common::key(KeyCode::Left));
    assert_eq!(value(&app), text("large"));
    let effects = app.handle_key(settings_common::ctrl('s'));
    let [Effect::SaveConfig(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let result = job.run();
    app.config_saved(result);
    assert!(
        file(&env).contains("card_size = \"large\""),
        "{}",
        file(&env)
    );
    assert_eq!(
        app.config().ui_card_size,
        CardSize::Large,
        "applied at once"
    );
    // The default removes the key.
    press(&mut app, settings_common::key(KeyCode::Delete));
    assert_eq!(value(&app), text("medium"));
    let effects = app.handle_key(settings_common::ctrl('s'));
    let [Effect::SaveConfig(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let result = job.run();
    app.config_saved(result);
    assert!(!file(&env).contains("card_size"), "{}", file(&env));
    assert_eq!(app.config().ui_card_size, CardSize::Medium);
}

#[test]
fn a_saved_size_redraws_the_cards() {
    let mut app = app_with(&listing(), "", "");
    let lines = bar(&mut app, 40);
    assert!(lines.iter().any(|l| l.starts_with('╭')));
    let config = Config::parse(
        "default_account = \"main\"\n[ui]\nstyle = \"workspace\"\ncard_size = \"small\"\n\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
        Path::new(HOME),
    )
    .unwrap();
    app.apply_config(config);
    let lines = bar(&mut app, 40);
    assert!(!lines.iter().any(|l| l.starts_with('╭')), "{lines:?}");
}

// ---- the other styles ---------------------------------------------------------------------

#[test]
fn the_other_styles_ignore_the_card_size_and_home() {
    let draw_style = |style, ui: &str| {
        let mut app = app_with(&listing(), ui, "");
        app.style = style;
        assert!(!app.pane_home());
        draw(&mut app, 160, 40)
    };
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let plain = draw_style(style, "");
        let small = draw_style(style, "card_size = \"small\"");
        assert_eq!(plain, small, "{style:?}");
        assert!(!plain.contains("Home"), "{style:?}");
    }
}
