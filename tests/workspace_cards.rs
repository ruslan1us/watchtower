//! The workspace's Home panel, its clicks and wheel on the list, and the cards
//! (doc/0045), without a terminal or a child: [`App`] returns effects and the
//! tests stand in for the runtime, with a clock of their own.

mod common;
mod settings_common;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{HOME, NoEnviron, ch, ctrl, key, status_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::{Color, Modifier};
use watchtower::app::{
    ATTACH_CHECK_TIMEOUT, ATTACH_DEBOUNCE, App, Effect, Overlay, PaneFocus, PaneOpen,
    RELEASE_AFTER, WANTED_PATIENCE,
};
use watchtower::config::{CardSize, Config, ConfigError, UiStyle, UiTheme};
use watchtower::dispatch::Dispatched;
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
    attached_with("", name)
}

/// [`attached_to`] with `ui` added to `[ui]`.
fn attached_with(ui: &str, name: &str) -> (App, Instant) {
    let mut app = app_with(&listing(), ui, "");
    draw(&mut app, 160, 40);
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

fn the_open(effects: &[Effect]) -> PaneOpen {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneOpen(open) => Some(open.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no client in {effects:?}"))
}

fn no_foreground_attach(effects: &[Effect]) {
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Attach(_))),
        "{effects:?}"
    );
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
        // The gap below it is nobody's.
        app.handle_key(ch('h'));
        app.handle_mouse(mouse(DOWN, 5, top + rows));
        assert!(app.pane_home(), "{size:?}");
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
            let want = match (size, look) {
                ("small", "ansi16" | "no-color") => '▶',
                ("small", _) => '█',
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

/// What the selection looks like on a card, per look.
#[derive(Clone, Copy)]
enum Shade {
    Painted(Color),
    Reversed,
}

const SELECTION_LOOKS: [(&str, UiTheme, bool, Shade); 6] = [
    (
        "neutral",
        UiTheme::Neutral,
        true,
        Shade::Painted(NEUTRAL_BAR),
    ),
    (
        "neutral-light",
        UiTheme::NeutralLight,
        true,
        Shade::Painted(Color::Rgb(214, 214, 214)),
    ),
    (
        "tokyo-night",
        UiTheme::TokyoNight,
        true,
        Shade::Painted(Color::Rgb(45, 63, 118)),
    ),
    ("terminal", UiTheme::Terminal, true, Shade::Reversed),
    ("ansi16", UiTheme::Ansi16, true, Shade::Reversed),
    ("no-color", UiTheme::Neutral, false, Shade::Reversed),
];

fn shaded(cell: &ratatui::buffer::Cell, shade: Shade) -> bool {
    match shade {
        Shade::Painted(bar) => cell.bg == bar,
        Shade::Reversed => cell.modifier.contains(Modifier::REVERSED),
    }
}

#[test]
fn every_row_of_the_selected_card_has_the_selection_in_each_size_look_and_focus() {
    for (size, rows) in [("large", 4), ("medium", 3), ("small", 2)] {
        for (look, theme, color, shade) in SELECTION_LOOKS {
            for terminal in [false, true] {
                let (mut app, _t) = attached_with(&format!("card_size = \"{size}\""), "builds");
                app.set_theme(theme);
                app.color = color;
                if terminal {
                    app.handle_key(key(KeyCode::Enter));
                    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
                }
                let at = format!("{size} {look} terminal={terminal}");
                let buf = render(&mut app, 160, 40);
                let card = card_rows(&buf, "builds", rows);
                for (y, row) in &card {
                    for x in 0..28 {
                        let cell = &buf[(x, *y)];
                        // A painted account chip keeps its own background.
                        let chip = cell.bg != Color::Reset
                            && row.contains("main")
                            && matches!(shade, Shade::Painted(_));
                        assert!(shaded(cell, shade) || chip, "{at}: {x} of {row}");
                    }
                }
                // Other cards and the rows between have none.
                for name in ["asks", "reviews"] {
                    for (y, row) in card_rows(&buf, name, rows) {
                        assert!(!shaded(&buf[(14, y)], shade), "{at}: {row}");
                    }
                }
                let below = card[rows - 1].0 + 1;
                assert!(!shaded(&buf[(14, below)], shade), "{at}: the gap");
                assert!(
                    !shaded(&buf[(14, card[0].0 - 1)], shade),
                    "{at}: the header"
                );
                // Focus still shows on the title row and the footer.
                let text = text_of(&buf);
                let first = text.lines().next().unwrap();
                assert_eq!(first.starts_with('▌'), !terminal, "{at}: {first}");
                let footer = text.lines().last().unwrap();
                let chip = if terminal { " TERMINAL " } else { " LIST " };
                assert!(footer.contains(chip), "{at}: {footer}");
            }
        }
    }
}

#[test]
fn the_selected_card_keeps_a_coloured_heavy_outline_and_readable_text() {
    let (mut app, _t) = attached_with("", "builds");
    let buf = render(&mut app, 160, 40);
    let card = card_rows(&buf, "builds", 3);
    let top = card[0].0;
    assert!(card[0].1.starts_with("┏━"), "{}", card[0].1);
    assert!(card[1].1.starts_with('┃'), "{}", card[1].1);
    assert!(card[2].1.starts_with('┗'), "{}", card[2].1);
    for (x, y) in [
        (0, top),
        (1, top),
        (27, top),
        (0, top + 1),
        (27, top + 1),
        (0, top + 2),
    ] {
        assert_eq!(buf[(x, y)].fg, NEUTRAL_BLUE, "{x},{y}");
    }
    // The bar's own text colour, on every text of the card.
    const BAR_TEXT: Color = Color::Rgb(255, 255, 255);
    let middle = card[1].1.clone();
    let state = middle.chars().position(|c| c == 'w').unwrap() as u16;
    let folder = middle.chars().position(|c| c == '…').unwrap() as u16;
    assert_eq!(buf[(state, top + 1)].fg, BAR_TEXT);
    assert_eq!(buf[(folder, top + 1)].fg, BAR_TEXT);
    let name = card[0].1.chars().position(|c| c == 'b').unwrap() as u16;
    assert_eq!(buf[(name, top)].fg, BAR_TEXT);
    assert!(buf[(name, top)].modifier.contains(Modifier::BOLD));
    // The small card's edge is on the same fill, and shows.
    let (mut app, _t) = attached_with("card_size = \"small\"", "builds");
    let buf = render(&mut app, 160, 40);
    for (y, text) in card_rows(&buf, "builds", 2) {
        assert!(text.starts_with('█'), "{text}");
        assert_eq!(buf[(0, y)].fg, NEUTRAL_BLUE, "{text}");
        assert_eq!(buf[(5, y)].bg, NEUTRAL_BAR, "{text}");
    }
}

#[test]
fn a_cards_text_has_contrast_the_name_leads_the_state_is_coloured_the_folder_dim() {
    for (size, state_row) in [("large", 1), ("medium", 1), ("small", 1)] {
        let mut app = app_with(&listing(), &format!("card_size = \"{size}\""), "");
        let buf = render(&mut app, 160, 40);
        let rows = CardSize::parse(size).unwrap().rows();
        let card = card_rows(&buf, "builds", rows);
        let (top, title) = (&card[0].0, &card[0].1);
        let name = title
            .find("builds")
            .map(|b| title[..b].chars().count())
            .unwrap() as u16;
        assert!(
            buf[(name, *top)].modifier.contains(Modifier::BOLD),
            "{size}"
        );
        assert_eq!(buf[(name, *top)].fg, Color::Rgb(235, 235, 235), "{size}");
        let row = &card[state_row].1;
        let y = card[state_row].0;
        let state = row
            .find("working")
            .map(|b| row[..b].chars().count())
            .unwrap() as u16;
        assert_eq!(buf[(state, y)].fg, NEUTRAL_BLUE, "{size}: {row}");
        let (folder_y, folder_row) = if size == "large" {
            (card[2].0, &card[2].1)
        } else {
            (y, row)
        };
        let folder = folder_row.chars().position(|c| c == '/').unwrap() as u16;
        assert_eq!(
            buf[(folder, folder_y)].fg,
            Color::Rgb(138, 138, 138),
            "{size}"
        );
    }
    // An interactive row stays dim, its name too.
    let mut app = app_with(&listing(), "", "");
    let buf = render(&mut app, 160, 40);
    let card = card_rows(&buf, "other-shell", 3);
    let name = card[0].1.chars().position(|c| c == 'o').unwrap() as u16;
    assert!(!buf[(name, card[0].0)].modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(name, card[0].0)].fg, Color::Rgb(138, 138, 138));
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

#[test]
fn the_settings_window_saves_the_card_gap() {
    use settings_common::{app_for, env, file, open, press, select};
    use watchtower::config_edit::{SettingKey, SettingValue};

    let env = env(Some(
        "[ui]\nstyle = \"workspace\"\n\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
    ));
    let mut app = app_for(&env);
    open(&mut app);
    select(&mut app, "gap between cards");
    let value = |app: &App| app.settings().unwrap().session.value(SettingKey::UiCardGap);
    assert_eq!(value(&app), SettingValue::Int(1), "the default");
    press(&mut app, settings_common::key(KeyCode::Left));
    assert_eq!(value(&app), SettingValue::Int(0));
    let effects = app.handle_key(settings_common::ctrl('s'));
    let [Effect::SaveConfig(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    let result = job.run();
    app.config_saved(result);
    assert!(file(&env).contains("card_gap = 0"), "{}", file(&env));
    assert_eq!(app.config().ui_card_gap, 0, "applied at once");
}

#[test]
fn card_gap_parses_defaults_to_one_and_refuses_the_rest() {
    let parse = |text: &str| Config::parse(text, Path::new(HOME));
    assert_eq!(parse("").unwrap().ui_card_gap, 1);
    assert_eq!(parse("[ui]\ncard_gap = 0\n").unwrap().ui_card_gap, 0);
    assert_eq!(parse("[ui]\ncard_gap = 1\n").unwrap().ui_card_gap, 1);
    let err = parse("[ui]\ncard_gap = 2\n").unwrap_err();
    assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
    assert!(err.to_string().contains("[ui] card_gap"), "{err}");
}

// ---- the gap -----------------------------------------------------------------------

#[test]
fn the_gap_keeps_cards_and_groups_apart_and_can_be_traded_back() {
    let two = [
        row("cccc3333", "reviews", "web", IDLE, "done"),
        row("ffff6666", "another", "web", IDLE, "done"),
        row("bbbb2222", "builds", "billing-ui", BUSY, "working"),
    ];
    let text = format!("[{}]", two.join(","));
    for (size, rows) in [("large", 4usize), ("medium", 3), ("small", 2)] {
        // With it: a header stays with its first card, then a blank row
        // between cards and before the next header.
        let ui = format!("card_size = \"{size}\"");
        let mut app = app_with(&text, &ui, "");
        let lines = bar(&mut app, 40);
        let first = y_of(&lines, "Ready for");
        assert_eq!(first, 1, "no gap at the top of the list: {size}");
        let (a, b) = {
            let a = lines
                .iter()
                .position(|l| l.contains("reviews") || l.contains("another"));
            let a = a.unwrap();
            (a, a + rows + 1)
        };
        assert_eq!(a, first + 1, "{size}: {lines:#?}");
        assert!(lines[a + rows].trim().is_empty(), "{size}: {lines:#?}");
        assert!(
            lines[b].contains("reviews") || lines[b].contains("another"),
            "{size}: {lines:#?}"
        );
        assert!(lines[b + rows].trim().is_empty(), "{size}: {lines:#?}");
        assert!(
            lines[b + rows + 1].contains("Working"),
            "{size}: {lines:#?}"
        );
        // Without it: none.
        let ui = format!("card_size = \"{size}\"\ncard_gap = 0");
        let mut app = app_with(&text, &ui, "");
        let lines = bar(&mut app, 40);
        let a = y_of(&lines, "Ready for") + 1;
        assert!(
            lines[a + rows].contains("reviews") || lines[a + rows].contains("another"),
            "{size}: {lines:#?}"
        );
        assert!(
            lines[a + 2 * rows].contains("Working"),
            "{size}: {lines:#?}"
        );
    }
}

#[test]
fn a_click_on_a_gap_selects_nothing_and_clicks_around_it_do() {
    let mut app = at_home();
    let lines = bar(&mut app, 40);
    let top = y_of(&lines, "builds");
    let below = top + 3;
    assert!(lines[below].trim().is_empty(), "{lines:#?}");
    let before = app.handle_mouse(mouse(DOWN, 3, below as u16));
    assert!(before.is_empty());
    assert!(app.pane_home(), "still nothing chosen");
    app.handle_mouse(mouse(DOWN, 3, top as u16 + 2));
    assert_eq!(selected(&app).as_deref(), Some("builds"));
    let chosen = app.selected_entry();
    app.handle_mouse(mouse(DOWN, 3, below as u16));
    assert_eq!(app.selected_entry(), chosen, "a gap moves nothing");
    // The wheel over a gap still moves, as over the rest of the list.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 3, below as u16));
    assert_ne!(app.selected_entry(), chosen);
}

#[test]
fn the_viewport_and_the_page_keys_match_the_gap() {
    for gap in [0, 1] {
        for (size, rows) in [("large", 4usize), ("medium", 3), ("small", 2)] {
            let ui = format!("card_size = \"{size}\"\ncard_gap = {gap}");
            let mut app = app_with(&many(40), &ui, "");
            draw(&mut app, 160, 30);
            app.handle_key(key(KeyCode::Down));
            for _ in 0..39 {
                let lines = bar(&mut app, 30);
                let name = selected(&app).unwrap();
                let top = y_of(&lines, &name);
                assert!(top + rows <= 29, "{size} gap {gap}: {name} at {top}");
                app.handle_key(key(KeyCode::Down));
            }
            // A page down moves by the entries that show: the next page's
            // first card was not on the screen, and no card is skipped by more.
            let mut app = app_with(&many(40), &ui, "");
            let lines = bar(&mut app, 30);
            let shown = lines.iter().filter(|l| l.contains("row-")).count();
            app.handle_key(key(KeyCode::Down));
            let lines = bar(&mut app, 30);
            assert_eq!(lines.iter().filter(|l| l.contains("row-")).count(), shown);
            app.handle_key(key(KeyCode::PageDown));
            let name = selected(&app).unwrap();
            let n: usize = name.trim_start_matches("row-").parse().unwrap();
            // The header counts as a line; the selection stays near a page.
            assert!(
                (shown - 1..=shown + 1).contains(&n),
                "{size} gap {gap}: {n} of {shown}"
            );
            let lines = bar(&mut app, 30);
            let top = y_of(&lines, &name);
            assert!(top + rows <= 29, "{size} gap {gap}: {name} at {top}");
            app.handle_key(key(KeyCode::PageUp));
            assert!(selected(&app).is_some() || app.selected_header().is_some());
        }
    }
}

#[test]
fn a_selected_card_at_the_bottom_does_not_move_the_list_when_only_its_gap_is_cut() {
    // The footer, the title row and the two hints' rows, then a header and six
    // medium cards with their gaps: the sixth ends on the list's last row.
    let mut app = app_with(&many(40), "", "");
    let h = 4 + 1 + 6 * 3 + 5;
    let lines = bar(&mut app, h);
    let name = "row-05";
    let top = y_of(&lines, name);
    assert_eq!(top + 3, usize::from(h) - 2, "{lines:#?}");
    app.handle_mouse(mouse(DOWN, 3, top as u16));
    assert_eq!(selected(&app).as_deref(), Some(name));
    let after = bar(&mut app, h);
    assert_eq!(y_of(&after, name), top, "the list stays under the mouse");
}

/// The cards drawn on a `w` × `h` terminal in a list with six groups, by
/// project, where the rest of the cards are in the last: what doc/0045 counts.
fn cards_that_fit(size: &str, gap: u16, w: u16, h: u16) -> usize {
    let rows: Vec<String> = (0..60)
        .map(|k| {
            let folder = format!("p{}", k.min(5));
            row(
                &format!("b{k:07}"),
                &format!("card-{k:02}"),
                &folder,
                BUSY,
                "working",
            )
        })
        .collect();
    let ui = format!("card_size = \"{size}\"\ncard_gap = {gap}");
    let mut app = app_with(&format!("[{}]", rows.join(",")), &ui, "");
    app.handle_key(ch('p'));
    draw(&mut app, w, h)
        .lines()
        .filter(|l| l.contains("card-"))
        .count()
}

#[test]
fn the_fit_counts_of_the_decision_hold() {
    for (size, gap, wide, narrow) in [
        ("large", 1, 8, 5),
        ("medium", 1, 10, 6),
        ("small", 1, 13, 9),
        ("large", 0, 10, 6),
        ("medium", 0, 13, 8),
        ("small", 0, 20, 13),
    ] {
        assert_eq!(
            cards_that_fit(size, gap, 200, 50),
            wide,
            "{size} gap {gap} at 200x50"
        );
        assert_eq!(
            cards_that_fit(size, gap, 120, 36),
            narrow,
            "{size} gap {gap} at 120x36"
        );
    }
}

// ---- the footer ---------------------------------------------------------------------

#[test]
fn the_footer_on_a_group_header_says_what_enter_does_there() {
    let mut app = at_home();
    let lines = bar(&mut app, 40);
    let y = y_of(&lines, "Stopped") as u16;
    app.handle_mouse(mouse(DOWN, 3, y));
    assert!(app.selected_header().is_some());
    let text = draw(&mut app, 160, 40);
    let footer = text.lines().last().unwrap();
    assert!(footer.contains("Enter fold"), "{footer}");
    assert!(!footer.contains("Enter type here"), "{footer}");
    // And it does.
    let before = app.entries().len();
    app.handle_key(key(KeyCode::Enter));
    assert_ne!(app.entries().len(), before);
    // On a row the hint is the row's again.
    common::select_name(&mut app, "builds");
    let text = draw(&mut app, 160, 40);
    assert!(text.lines().last().unwrap().contains("Enter type here"));
    common::select_name(&mut app, "sleeps");
    let text = draw(&mut app, 160, 40);
    assert!(text.lines().last().unwrap().contains("Enter wake here"));
}

// ---- attaches open in the pane ------------------------------------------------------

const ORCHESTRATOR_DIR: &str = "orchestrator";

fn with_orchestrator() -> String {
    let rows = [
        row(
            "oooo0000",
            "orchestrator",
            ORCHESTRATOR_DIR,
            IDLE,
            "blocked",
        ),
        row("bbbb2222", "builds", "billing-ui", BUSY, "working"),
    ];
    format!("[{}]", rows.join(","))
}

/// Runs a checked attach to its end, as the clock would.
fn after_the_check(app: &mut App) -> Vec<Effect> {
    app.tick(Instant::now() + ATTACH_CHECK_TIMEOUT + Duration::from_secs(1))
}

#[test]
fn o_selects_the_orchestrators_row_and_opens_it_in_the_pane_with_the_keys() {
    let mut app = app_with(&with_orchestrator(), "", "");
    draw(&mut app, 160, 40);
    assert!(app.pane_home());
    let effects = app.handle_key(ch('o'));
    assert!(effects.contains(&Effect::PollForAttach), "{effects:?}");
    assert!(app.pane_home(), "not before the check");
    let effects = after_the_check(&mut app);
    no_foreground_attach(&effects);
    let open = the_open(&effects);
    assert_eq!(open.id, "oooo0000");
    assert_eq!(selected(&app).as_deref(), Some("orchestrator"));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(!app.pane_home());
    // The next turn of the loop leaves it so, and attaches nothing more.
    let t = Instant::now();
    let later = app.pane_tick(t);
    assert!(!opened(&later), "{later:?}");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    let later = app.pane_tick(t + ATTACH_DEBOUNCE);
    assert!(!opened(&later), "{later:?}");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert!(app.pane_has_keys());
    // Keys go to the child, and the way back is the usual one.
    let effects = app.handle_key(ch('x'));
    assert_eq!(effects, [Effect::PaneWrite(b"x".to_vec())]);
    app.handle_key(ctrl('z'));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(!opened(&app.pane_tick(t)));
    assert!(app.pane_client_key().is_some(), "the client stays");
    app.handle_key(ch('o'));
    let effects = after_the_check(&mut app);
    assert!(!opened(&effects), "the same client: {effects:?}");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    app.handle_key(key(KeyCode::F(12)));
    assert_eq!(app.pane_focus(), PaneFocus::List);
}

#[test]
fn o_replaces_another_rows_client_and_finds_a_row_in_a_folded_group_or_filtered_away() {
    let mut app = app_with(&with_orchestrator(), "", "");
    draw(&mut app, 160, 40);
    common::select_name(&mut app, "builds");
    let t = Instant::now();
    app.pane_tick(t);
    let open = the_open(&app.pane_tick(t + ATTACH_DEBOUNCE));
    assert_eq!(open.id, "bbbb2222");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));

    common::select_name(&mut app, "orchestrator");
    app.handle_key(key(KeyCode::Tab));
    assert!(selected(&app).is_none(), "folded");
    app.handle_key(ch('o'));
    let open = the_open(&after_the_check(&mut app));
    assert_eq!(open.id, "oooo0000");
    assert_eq!(selected(&app).as_deref(), Some("orchestrator"), "unfolded");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);

    let mut app = app_with(&with_orchestrator(), "", "");
    draw(&mut app, 160, 40);
    app.handle_key(ch('/'));
    app.handle_key(ch('b'));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ch('o'));
    let open = the_open(&after_the_check(&mut app));
    assert_eq!(open.id, "oooo0000");
    assert_eq!(
        selected(&app).as_deref(),
        Some("orchestrator"),
        "filter cleared"
    );
}

fn start_of(effects: &[Effect]) -> watchtower::app::OrchestratorJob {
    match effects {
        [Effect::StartOrchestrator(job)] => job.clone(),
        other => panic!("expected one orchestrator start, got {other:?}"),
    }
}

#[test]
fn o_without_an_orchestrator_starts_it_and_opens_it_when_it_appears() {
    let builds = row("bbbb2222", "builds", "billing-ui", BUSY, "working");
    let mut app = app_with(&format!("[{builds}]"), "", "");
    draw(&mut app, 160, 40);
    let job = start_of(&app.handle_key(ch('o')));
    let effects = app.orchestrator_finished(
        job,
        Ok(Dispatched {
            id: "oooo0000".into(),
            name: Some("orchestrator".into()),
        }),
    );
    no_foreground_attach(&effects);
    assert!(effects.contains(&Effect::PollNow), "{effects:?}");
    assert!(app.pane_home(), "not listed yet");
    let t = Instant::now();
    assert!(!opened(&app.pane_tick(t)), "still not listed");
    // It appears.
    app.apply_poll(poll("main", &with_orchestrator()));
    let effects = app.pane_tick(t);
    no_foreground_attach(&effects);
    assert_eq!(the_open(&effects).id, "oooo0000");
    assert_eq!(selected(&app).as_deref(), Some("orchestrator"));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(!opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
}

#[test]
fn a_start_that_fails_or_never_appears_keeps_working_as_before() {
    use watchtower::dispatch::DispatchError;
    use watchtower::orchestrator::StartError;
    let builds = row("bbbb2222", "builds", "billing-ui", BUSY, "working");
    let text = format!("[{builds}]");
    // Not trusted: its window, no client.
    let mut app = app_with(&text, "", "");
    draw(&mut app, 160, 40);
    let job = start_of(&app.handle_key(ch('o')));
    let effects = app.orchestrator_finished(
        job,
        Err(StartError::Dispatch(DispatchError::Untrusted {
            dir: "/home/user/x".into(),
            message: "Workspace not trusted".into(),
        })),
    );
    assert!(effects.is_empty(), "{effects:?}");
    assert!(draw(&mut app, 160, 40).contains("Ctrl+T"));
    assert!(!opened(&app.pane_tick(Instant::now())));

    // Never listed: it stops waiting and says so.
    let mut app = app_with(&text, "", "");
    draw(&mut app, 160, 40);
    let job = start_of(&app.handle_key(ch('o')));
    let started = Dispatched {
        id: "oooo0000".into(),
        name: None,
    };
    app.orchestrator_finished(job, Ok(started));
    let t = Instant::now();
    assert!(!opened(
        &app.pane_tick(t + WANTED_PATIENCE + Duration::from_secs(1))
    ));
    assert!(
        status_text(&app).contains("not listed yet"),
        "{}",
        status_text(&app)
    );
    app.apply_poll(poll("main", &with_orchestrator()));
    assert!(!opened(&app.pane_tick(t)), "too late to take the keys");
    assert!(app.pane_home());

    // The user moved on: the keys are not taken from them.
    let mut app = app_with(&text, "", "");
    draw(&mut app, 160, 40);
    let job = start_of(&app.handle_key(ch('o')));
    let started = Dispatched {
        id: "oooo0000".into(),
        name: None,
    };
    app.orchestrator_finished(job, Ok(started));
    app.handle_key(key(KeyCode::Down));
    let t = Instant::now();
    app.apply_poll(poll("main", &with_orchestrator()));
    app.pane_tick(t);
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(
        status_text(&app).contains("selection moved"),
        "{}",
        status_text(&app)
    );
}

/// The new-session dialog started with `Alt+Enter`: its job.
fn dialog_job(app: &mut App, root: &Path) -> watchtower::app::DispatchJob {
    app.open_dialog(common::project_list(root, &["demo"]));
    app.handle_key(key(KeyCode::Tab));
    for c in "do it".chars() {
        app.handle_key(ch(c));
    }
    let effects = app.handle_key(ratatui::crossterm::event::KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::ALT,
    ));
    let job = effects
        .iter()
        .find_map(|e| match e {
            Effect::Dispatch(job) => Some(job.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{effects:?}"));
    assert!(job.attach);
    job
}

#[test]
fn start_and_attach_opens_the_new_session_in_the_pane_with_the_keys() {
    let root = tempfile::tempdir().unwrap();
    let mut app = at_home();
    let job = dialog_job(&mut app, root.path());
    let effects = app.dispatch_finished(
        job,
        Ok(Dispatched {
            id: "nnnn5555".into(),
            name: Some("new-one".into()),
        }),
    );
    no_foreground_attach(&effects);
    assert!(effects.contains(&Effect::PollNow), "{effects:?}");
    assert!(app.dialog().is_none());
    let t = Instant::now();
    assert!(!opened(&app.pane_tick(t)));
    let rows = listing().trim_end_matches(']').to_owned()
        + ","
        + &row("nnnn5555", "new-one", "demo", BUSY, "working")
        + "]";
    app.apply_poll(poll("main", &rows));
    let effects = app.pane_tick(t);
    no_foreground_attach(&effects);
    let open = the_open(&effects);
    assert_eq!(open.id, "nnnn5555");
    assert!(open.cwd.ends_with("workspace/demo"), "{:?}", open.cwd);
    assert_eq!(selected(&app).as_deref(), Some("new-one"));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    // A plain start stays where it was.
    let mut app = at_home();
    app.open_dialog(common::project_list(root.path(), &["demo"]));
    app.handle_key(key(KeyCode::Tab));
    for c in "do it".chars() {
        app.handle_key(ch(c));
    }
    let effects = app.handle_key(ctrl('s'));
    let job = effects
        .iter()
        .find_map(|e| match e {
            Effect::Dispatch(job) => Some(job.clone()),
            _ => None,
        })
        .unwrap();
    let started = Dispatched {
        id: "nnnn5555".into(),
        name: None,
    };
    app.dispatch_finished(job, Ok(started));
    app.apply_poll(poll("main", &rows));
    assert!(!opened(&app.pane_tick(Instant::now())));
    assert!(app.pane_home());
}

#[test]
fn a_window_holds_the_opening_until_it_closes() {
    let root = tempfile::tempdir().unwrap();
    let mut app = at_home();
    let job = dialog_job(&mut app, root.path());
    let started = Dispatched {
        id: "nnnn5555".into(),
        name: None,
    };
    app.dispatch_finished(job, Ok(started));
    app.handle_key(ch('?'));
    let rows = listing().trim_end_matches(']').to_owned()
        + ","
        + &row("nnnn5555", "new-one", "demo", BUSY, "working")
        + "]";
    app.apply_poll(poll("main", &rows));
    let t = Instant::now();
    assert!(!opened(&app.pane_tick(t)));
    app.handle_key(key(KeyCode::Esc));
    assert!(opened(&app.pane_tick(t)));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
}

#[test]
fn below_the_width_the_foreground_attach_stays() {
    let mut app = app_with(&with_orchestrator(), "", "");
    draw(&mut app, 90, 30);
    assert!(!app.pane_active());
    app.handle_key(ch('o'));
    let effects = after_the_check(&mut app);
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Attach(_))),
        "{effects:?}"
    );
    assert!(!opened(&effects));
}

/// `e` on `builds`, with `new` typed and `Enter`: its client opens in the pane.
fn renamed_in_the_pane(new: &str) -> (App, Instant, PaneOpen) {
    let mut app = at_home();
    common::select_name(&mut app, "builds");
    app.handle_key(ch('e'));
    app.handle_key(ctrl('u'));
    for c in new.chars() {
        app.handle_key(ch(c));
    }
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.contains(&Effect::PollForAttach), "{effects:?}");
    let effects = after_the_check(&mut app);
    no_foreground_attach(&effects);
    let open = the_open(&effects);
    assert_eq!(open.id, "bbbb2222");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(
        status_text(&app).contains(&format!("/rename {new}")),
        "{}",
        status_text(&app)
    );
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    (app, Instant::now(), open)
}

fn listing_with_builds_named(name: &str) -> String {
    listing().replace("\"builds\"", &format!("\"{name}\""))
}

#[test]
fn renaming_a_running_session_opens_it_in_the_pane_and_checks_when_the_keys_return() {
    let (mut app, t, _open) = renamed_in_the_pane("fresh-name");
    // Nothing is checked while the terminal has the keys.
    assert!(!app.pane_tick(t).contains(&Effect::PollForRename));
    let effects = app.handle_key(ctrl('z'));
    assert!(effects.contains(&Effect::PollForRename), "{effects:?}");
    app.rename_poll_started(5_000);
    let mut update = poll("main", &listing_with_builds_named("fresh-name"));
    update.cycle = 5_000;
    app.apply_poll(update);
    assert!(
        status_text(&app).contains("renamed builds → fresh-name"),
        "{}",
        status_text(&app)
    );
    // Once.
    let effects = app.handle_key(ch('h'));
    assert!(!effects.contains(&Effect::PollForRename));
}

#[test]
fn a_rename_that_did_not_happen_gets_the_note_and_a_failed_client_drops_it() {
    let (mut app, ..) = renamed_in_the_pane("fresh-name");
    app.handle_key(key(KeyCode::F(12)));
    app.rename_poll_started(5_000);
    let mut update = poll("main", &listing());
    update.cycle = 5_000;
    app.apply_poll(update);
    assert!(
        status_text(&app).contains("name unchanged"),
        "{}",
        status_text(&app)
    );

    // A clean end of the client checks too.
    let (mut app, _t, open) = renamed_in_the_pane("fresh-name");
    let effects = app.pane_exited(open.generation, Some(0));
    assert!(effects.contains(&Effect::PollForRename), "{effects:?}");

    // A failed one does not.
    let (mut app, _t, open) = renamed_in_the_pane("fresh-name");
    let effects = app.pane_exited(open.generation, Some(1));
    assert_eq!(effects, [Effect::PollNow]);
    app.rename_poll_started(5_000);
    let mut update = poll("main", &listing());
    update.cycle = 5_000;
    app.apply_poll(update);
    assert!(
        !status_text(&app).contains("name unchanged"),
        "{}",
        status_text(&app)
    );
}

// ---- the usage block -----------------------------------------------------------------

const TWO_ACCOUNTS: &str = "[[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n\
                            config_dir = \"~/.claude-alt\"\n";

fn usage_app(text: &str, ui: &str) -> App {
    let mut app = app_with(text, ui, &format!("{TWO_ACCOUNTS}[usage]\nshow = true\n"));
    app.clock = Clock::UTC;
    app
}

fn usage_in(app: &mut App, account: &str, result: Result<Probed, String>, ago_s: i64) {
    app.usage_received(account, result, Instant::now(), NOW_MS - ago_s * 1000);
}

#[test]
fn the_usage_block_is_at_the_bars_bottom_while_it_is_on() {
    let mut app = usage_app(&listing(), "");
    usage_in(&mut app, "main", Ok(plan(24.0, 12.0)), 0);
    usage_in(&mut app, "alt", Ok(plan(5.0, 30.0)), 0);
    let lines = bar(&mut app, 40);
    // The footer is the last of the 40 rows.
    assert!(lines[36].starts_with("─ usage ─"), "{lines:#?}");
    assert!(
        lines[37].contains("main") && lines[37].contains("5h 24% · 7d 12%"),
        "{lines:#?}"
    );
    assert!(
        lines[38].contains("alt") && lines[38].contains("5h 5% · 7d 30%"),
        "{lines:#?}"
    );
    assert!(
        !lines[37].contains("resets"),
        "reset times stay on Home: {}",
        lines[37]
    );
    // Off: no space, no rows.
    let mut app = app_with(&listing(), "", TWO_ACCOUNTS);
    let lines = bar(&mut app, 40);
    assert!(!lines.iter().any(|l| l.contains("usage")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l.contains("5h")), "{lines:#?}");
}

#[test]
fn the_usage_blocks_rows_read_as_on_home_stale_dim_and_waiting_or_missing() {
    let mut app = usage_app(&listing(), "");
    // Before the first answer.
    let lines = bar(&mut app, 40);
    assert!(
        lines[37].contains("main") && lines[37].contains('…'),
        "{}",
        lines[37]
    );
    // A failed probe with nothing known, and a stale value dimmed.
    usage_in(&mut app, "main", Err("no answer".into()), 0);
    usage_in(&mut app, "alt", Ok(plan(5.0, 30.0)), 3 * 3600);
    let buf = render(&mut app, 160, 40);
    let lines = bar_of(&text_of(&buf), 28);
    assert!(lines[37].contains('—'), "{}", lines[37]);
    assert!(lines[38].contains("5h 5%"), "{}", lines[38]);
    let x = lines[38]
        .find("5h")
        .map(|b| lines[38][..b].chars().count())
        .unwrap();
    assert_eq!(
        buf[(x as u16, 38)].fg,
        Color::Rgb(138, 138, 138),
        "dim: {}",
        lines[38]
    );
}

#[test]
fn the_usage_block_drops_the_week_before_cutting_anything() {
    let long = "[[accounts]]\nname = \"a-very-long-account-name\"\ncommand = \"claude-alt\"\n\
                config_dir = \"~/.claude-alt\"\n[usage]\nshow = true\n";
    let mut app = app_with(&listing(), "list_width = 20", long);
    usage_in(&mut app, "main", Ok(plan(24.0, 12.0)), 0);
    usage_in(&mut app, "a-very-long-account-name", Ok(plan(5.0, 30.0)), 0);
    let lines = bar_of(&draw(&mut app, 160, 40), 20);
    let main = &lines[37];
    let other = &lines[38];
    assert_eq!(main.chars().count(), 20);
    // 20 cells hold the name column, so the week has gone from both; the long
    // name alone is cut, with `…`, and its number stays.
    assert!(main.contains("5h 24%") && !main.contains("7d"), "{main}");
    assert!(other.contains("5h 5%") && !other.contains("7d"), "{other}");
    assert!(other.contains('…'), "{other}");
    // Room for all of it: nothing goes.
    let mut app = app_with(&listing(), "list_width = 40", long);
    usage_in(&mut app, "main", Ok(plan(24.0, 12.0)), 0);
    usage_in(&mut app, "a-very-long-account-name", Ok(plan(5.0, 30.0)), 0);
    let lines = bar_of(&draw(&mut app, 160, 40), 40);
    assert!(lines[37].contains("5h 24% · 7d 12%"), "{}", lines[37]);
}

#[test]
fn cards_never_overlap_the_usage_block_and_clicks_and_pages_use_the_smaller_list() {
    let accounts = format!("{TWO_ACCOUNTS}[usage]\nshow = true\n");
    for (size, rows) in [("large", 4usize), ("medium", 3), ("small", 2)] {
        let mut app = app_with(&many(40), &format!("card_size = \"{size}\""), &accounts);
        usage_in(&mut app, "main", Ok(plan(24.0, 12.0)), 0);
        usage_in(&mut app, "alt", Ok(plan(5.0, 30.0)), 0);
        let lines = bar(&mut app, 30);
        // The block's 3 rows end on row 28; the hint is above it.
        assert!(lines[26].starts_with("─ usage"), "{size}: {lines:#?}");
        assert!(
            lines[25].trim().is_empty() || lines[25].contains('▼'),
            "{size}: {}",
            lines[25]
        );
        app.handle_key(key(KeyCode::Down));
        for _ in 0..39 {
            let lines = bar(&mut app, 30);
            let name = selected(&app).unwrap();
            let top = y_of(&lines, &name);
            assert!(top + rows <= 25, "{size}: {name} at {top}\n{lines:#?}");
            assert!(lines[26].starts_with("─ usage"), "{size}: {lines:#?}");
            app.handle_key(key(KeyCode::Down));
        }
        // A click on the block selects nothing.
        let before = app.selected_entry();
        for y in 26..29 {
            app.handle_mouse(mouse(DOWN, 3, y));
            assert_eq!(app.selected_entry(), before, "{size}: row {y}");
        }
    }
}

#[test]
fn a_short_list_hides_the_usage_block_rather_than_the_cards() {
    // Eight accounts need 9 rows: at 24 rows only 13 would be left for the
    // list... and at 21 rows it goes. The workspace's minimum is 24, so many
    // accounts stand in for few rows.
    let mut accounts = String::new();
    for k in 0..11 {
        accounts.push_str(&format!(
            "[[accounts]]\nname = \"acct{k}\"\ncommand = \"claude-{k}\"\n\
             config_dir = \"~/.claude-{k}\"\n"
        ));
    }
    accounts.push_str("[usage]\nshow = true\n");
    let mut app = app_with(&many(40), "", &accounts);
    let lines = bar(&mut app, 24);
    assert!(
        !lines.iter().any(|l| l.contains("─ usage")),
        "hidden: {lines:#?}"
    );
    assert!(lines.iter().any(|l| l.contains("row-")), "{lines:#?}");
    // With rows to spare it is back.
    let lines = bar(&mut app, 40);
    assert!(lines.iter().any(|l| l.contains("─ usage")), "{lines:#?}");
}

#[test]
fn home_keeps_the_reset_times() {
    let mut app = usage_app(&listing(), "");
    let in_two_hours = NOW_MS + 2 * 3_600_000;
    let mut probed = plan(24.0, 12.0);
    for limit in &mut probed.usage.as_mut().unwrap().limits {
        limit.resets_at_ms = Some(in_two_hours);
    }
    usage_in(&mut app, "main", Ok(probed), 0);
    let text = draw(&mut app, 160, 40);
    let home = text
        .lines()
        .find(|l| l.contains("5h 24% · resets"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(home.contains("7d 12% · resets"), "{home}");
    let bar = bar_of(&text, 28);
    assert!(!bar.iter().any(|l| l.contains("resets")), "{bar:#?}");
}

// ---- the list scrolls ----------------------------------------------------------------

#[test]
fn hints_say_what_is_hidden_on_rows_that_do_not_move() {
    let mut app = app_with(&many(40), "", "");
    let lines = bar(&mut app, 30);
    // 28 rows less the title: one header, then cards every 4 rows.
    assert!(
        lines[1].trim().is_empty(),
        "nothing above yet: {}",
        lines[1]
    );
    let below = &lines[28];
    assert!(below.contains('▼') && below.contains("more"), "{below}");
    let hidden: usize = below
        .split_whitespace()
        .find_map(|w| w.parse().ok())
        .unwrap();
    let shown = lines.iter().filter(|l| l.contains("row-")).count();
    assert_eq!(hidden + shown, 40, "{below}");
    app.handle_key(key(KeyCode::End));
    let lines = bar(&mut app, 30);
    let above = &lines[1];
    assert!(above.contains('▲') && above.contains("more"), "{above}");
    let hidden: usize = above
        .split_whitespace()
        .find_map(|w| w.parse().ok())
        .unwrap();
    let shown = lines.iter().filter(|l| l.contains("row-")).count();
    assert_eq!(hidden + shown, 40, "{above}");
    assert!(lines[28].trim().is_empty(), "nothing below: {}", lines[28]);
    // Dim, and the same rows in between.
    app.handle_key(key(KeyCode::Home));
    for _ in 0..20 {
        app.handle_key(key(KeyCode::Down));
    }
    let buf = render(&mut app, 160, 30);
    let lines = bar_of(&text_of(&buf), 28);
    assert!(
        lines[1].contains('▲') && lines[28].contains('▼'),
        "{lines:#?}"
    );
    assert_eq!(buf[(1, 1)].fg, Color::Rgb(138, 138, 138));
    assert_eq!(buf[(1, 28)].fg, Color::Rgb(138, 138, 138));
}

#[test]
fn a_hint_counts_groups_when_only_folded_ones_are_hidden() {
    let mut rows: Vec<String> = (0..12)
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
    rows.push(row("dddd4444", "sleeps", "infra", "", "done"));
    let text = format!("[{}]", rows.join(","));
    let found = (24..60).any(|h| {
        let mut app = app_with(&text, "", "");
        common::select_name(&mut app, "row-11");
        bar(&mut app, h)
            .iter()
            .any(|l| l.contains("▼ 1 more group"))
    });
    assert!(found, "the folded group below the last card");
}

#[test]
fn no_hints_and_no_reserved_rows_when_the_list_fits() {
    let mut app = app_with(&listing(), "", "");
    let lines = bar(&mut app, 40);
    assert!(!lines.iter().any(|l| l.contains('▲') || l.contains('▼')));
    assert_eq!(y_of(&lines, "Needs me"), 1, "{lines:#?}");
}

#[test]
fn a_click_on_a_hint_moves_a_page() {
    let mut app = app_with(&many(40), "", "");
    let lines = bar(&mut app, 30);
    let shown = lines.iter().filter(|l| l.contains("row-")).count();
    app.handle_key(key(KeyCode::Down));
    let first = app.selected_entry().unwrap();
    bar(&mut app, 30);
    app.handle_mouse(mouse(DOWN, 3, 28));
    let after = app.selected_entry().unwrap();
    assert!(after > first, "down a page");
    assert!(after - first <= shown + 1, "{after} from {first}");
    bar(&mut app, 30);
    app.handle_mouse(mouse(DOWN, 3, 1));
    assert!(app.selected_entry().unwrap() < after, "up a page");
    // A blank hint row is nobody's.
    let mut app = app_with(&many(40), "", "");
    bar(&mut app, 30);
    app.handle_mouse(mouse(DOWN, 3, 1));
    assert!(app.pane_home());
}

/// 60 sessions across the state groups, none of them folded.
fn sixty() -> String {
    let kinds = [
        (ASKS, "blocked"),
        (BUSY, "working"),
        (IDLE, "done"),
        (r#""pid": 4103, "status": "idle","#, "failed"),
    ];
    let rows: Vec<String> = (0..60)
        .map(|k| {
            let (process, state) = kinds[k % kinds.len()];
            row(
                &format!("c{k:07}"),
                &format!("many-{k:02}"),
                "demo",
                process,
                state,
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

#[test]
fn sixty_sessions_stay_usable_at_every_size() {
    for (size, rows) in [("large", 4usize), ("medium", 3), ("small", 2)] {
        let at = |what: &str| format!("{size}: {what}");
        let mut app = app_with(&sixty(), &format!("card_size = \"{size}\""), "");
        draw(&mut app, 200, 50);
        // Down the whole list: the selection is always whole on the screen.
        app.handle_key(key(KeyCode::Down));
        for _ in 0..70 {
            let lines = bar_of(&draw(&mut app, 200, 50), 36);
            if let Some(name) = selected(&app) {
                let top = y_of(&lines, &name);
                assert!(top + rows <= 48, "{} at {top}", at(&name));
                assert!(top >= 1, "{}", at(&name));
            } else {
                let title = app.selected_header().unwrap().title().to_owned();
                y_of(&lines, &title);
            }
            app.handle_key(key(KeyCode::Down));
        }
        // `G` and `g` reach the ends.
        app.handle_key(ch('G'));
        let lines = bar_of(&draw(&mut app, 200, 50), 36);
        assert!(!lines.iter().any(|l| l.contains('▼')), "{}", at("the end"));
        assert!(lines.iter().any(|l| l.contains('▲')), "{}", at("the end"));
        y_of(&lines, &selected(&app).unwrap());
        app.handle_key(ch('g'));
        assert_eq!(app.selected_entry(), Some(0));
        let lines = bar_of(&draw(&mut app, 200, 50), 36);
        assert!(!lines.iter().any(|l| l.contains('▲')), "{}", at("the top"));
        assert!(lines.iter().any(|l| l.contains('▼')), "{}", at("the top"));
        // Folding every group fits the list, and the hints go.
        loop {
            let header = app.selected_header().is_some();
            assert!(header);
            let open = app.entries().iter().any(|e| {
                matches!(
                    e,
                    watchtower::app::Entry::Header {
                        collapsed: false,
                        ..
                    }
                )
            });
            if !open {
                break;
            }
            app.handle_key(key(KeyCode::Tab));
            app.handle_key(key(KeyCode::Down));
            while app.selected_header().is_none() {
                app.handle_key(key(KeyCode::Down));
            }
            let collapsed = matches!(
                app.entries()[app.selected_entry().unwrap()],
                watchtower::app::Entry::Header {
                    collapsed: true,
                    ..
                }
            );
            if collapsed && app.selected_entry().unwrap() + 1 >= app.entries().len() {
                break;
            }
        }
        let lines = bar_of(&draw(&mut app, 200, 50), 36);
        assert!(
            !lines.iter().any(|l| l.contains('▼')),
            "{}\n{lines:#?}",
            at("folded")
        );
    }
}

#[test]
fn a_fast_wheel_attaches_once_after_it_settles() {
    for size in ["large", "medium", "small"] {
        let mut app = app_with(&sixty(), &format!("card_size = \"{size}\""), "");
        draw(&mut app, 200, 50);
        let mut t = Instant::now();
        let wheel = |app: &mut App| app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 10));
        let mut opens = 0;
        // Notches faster than the rest, across many rows.
        for _ in 0..40 {
            assert!(!opened(&wheel(&mut app)));
            let effects = app.pane_tick(t);
            opens += usize::from(opened(&effects));
            draw(&mut app, 200, 50);
            t += ATTACH_DEBOUNCE / 3;
        }
        assert_eq!(opens, 0, "{size}: nothing while it moves");
        let settled = t + ATTACH_DEBOUNCE;
        let effects = app.pane_tick(settled);
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::PaneOpen(_)))
                .count(),
            1,
            "{size}: {effects:?}"
        );
        let id = the_open(&effects).id;
        let row = app.selected_session().unwrap();
        assert_eq!(
            row.session.id.as_deref(),
            Some(id.as_str()),
            "{size}: the row it stopped on"
        );
        assert!(
            !opened(&app.pane_tick(settled + ATTACH_DEBOUNCE)),
            "{size}: once"
        );
    }
}

// ---- the shell in the pane -----------------------------------------------------------

fn shell_open(effects: &[Effect]) -> watchtower::app::PaneShellOpen {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::PaneShellOpen(open) => Some(open.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no shell in {effects:?}"))
}

/// `!` on `name`, with the shell's screen reported: its generation and screen.
fn shell_on(app: &mut App, name: &str) -> (u64, PaneScreen) {
    common::select_name(app, name);
    let open = shell_open(&app.handle_key(ch('!')));
    assert!(open.dir.ends_with(if name == "builds" {
        "billing-ui"
    } else {
        "web"
    }));
    let screen = PaneScreen::new(open.rows, open.cols);
    screen.feed(b"SHELL SCREEN");
    app.pane_opened(open.generation, screen.clone());
    (open.generation, screen)
}

#[test]
fn bang_opens_the_shell_in_the_pane_with_the_keys_beside_the_client() {
    let (mut app, t) = attached_to("builds");
    let (generation, screen) = shell_on(&mut app, "builds");
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(app.pane_has_keys());
    assert!(app.pane_client_key().is_some(), "the client stays");
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("shell  ~/workspace/billing-ui"), "{text}");
    assert!(text.contains("keyboard here"), "{text}");
    assert!(text.contains("SHELL SCREEN"), "{text}");
    // Keys go to the shell, not the client.
    assert_eq!(
        app.handle_key(ch('x')),
        [Effect::PaneShellWrite(b"x".to_vec())]
    );
    assert_eq!(
        app.handle_key(ctrl('c')),
        [Effect::PaneShellWrite(b"\x03".to_vec())]
    );
    assert_eq!(
        app.handle_paste("hi"),
        [Effect::PaneShellWrite(b"hi".to_vec())]
    );
    // Ctrl+Z and F12 are tower's, here too: never the shell's job control.
    let effects = app.handle_key(ctrl('z'));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PaneShellWrite(_))),
        "{effects:?}"
    );
    assert_eq!(app.pane_focus(), PaneFocus::List);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.pane_focus(),
        PaneFocus::Terminal,
        "Enter returns to the shell"
    );
    assert!(app.handle_key(key(KeyCode::F(12))).is_empty());
    assert_eq!(app.pane_focus(), PaneFocus::List);
    // A focus report and the size go to the shell.
    screen.feed(b"\x1b[?1004h");
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        [Effect::PaneShellWrite(b"\x1b[I".to_vec())]
    );
    assert_eq!(
        app.handle_key(ctrl('z')),
        [Effect::PaneShellWrite(b"\x1b[O".to_vec())]
    );
    draw(&mut app, 140, 40);
    let effects = app.pane_tick(t);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::PaneShellResize { .. })),
        "{effects:?}"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::PaneResize { .. })),
        "{effects:?}"
    );
    assert!(generation > 0);
}

#[test]
fn bang_toggles_between_the_shell_and_what_the_row_shows() {
    let (mut app, t) = attached_to("builds");
    shell_on(&mut app, "builds");
    app.handle_key(ctrl('z'));
    // To Claude: the list keeps the keys.
    assert!(app.handle_key(ch('!')).is_empty());
    assert!(!app.pane_shell_showing());
    assert_eq!(app.pane_focus(), PaneFocus::List);
    let text = draw(&mut app, 160, 40);
    assert!(!text.contains("SHELL SCREEN"), "{text}");
    assert!(text.contains("view only"), "{text}");
    assert!(!opened(&app.pane_tick(t)), "the client was there all along");
    // Back to the shell, with the keys, and no second shell.
    let effects = app.handle_key(ch('!'));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PaneShellOpen(_))),
        "{effects:?}"
    );
    assert!(app.pane_shell_showing());
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(draw(&mut app, 160, 40).contains("SHELL SCREEN"));
}

#[test]
fn the_shell_outlives_browsing_and_shows_again_on_its_row() {
    let mut app = at_home();
    let (_generation, _screen) = shell_on(&mut app, "builds");
    let t = Instant::now();
    app.pane_tick(t);
    // Its row needs no client while the shell shows.
    assert!(!opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
    assert_eq!(app.pane_wait(t), None, "no timer either");
    app.handle_key(ctrl('z'));
    // Another row: it attaches as usual, the shell stays.
    common::select_name(&mut app, "reviews");
    assert!(!app.pane_shell_showing());
    let effects = app.pane_tick(t);
    assert!(!effects.contains(&Effect::PaneShellClose), "{effects:?}");
    let effects = app.pane_tick(t + ATTACH_DEBOUNCE * 2);
    let open = the_open(&effects);
    assert_eq!(open.id, "cccc3333");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert!(!draw(&mut app, 160, 40).contains("SHELL SCREEN"));
    // Back: the shell shows, the keys stay with the list.
    common::select_name(&mut app, "builds");
    assert!(app.pane_shell_showing());
    assert!(
        app.pane_tick(t + ATTACH_DEBOUNCE * 2)
            .iter()
            .all(|e| !matches!(e, Effect::PaneShellOpen(_)))
    );
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(draw(&mut app, 160, 40).contains("SHELL SCREEN"));
    // Home hides it too, and comes back.
    app.handle_key(ch('h'));
    assert!(!app.pane_shell_showing());
    app.handle_key(key(KeyCode::Down));
    assert!(app.pane_shell_showing());
}

#[test]
fn bang_on_another_row_closes_the_old_shell_with_a_note() {
    let mut app = at_home();
    shell_on(&mut app, "builds");
    app.handle_key(ctrl('z'));
    common::select_name(&mut app, "reviews");
    let effects = app.handle_key(ch('!'));
    assert_eq!(
        effects.first(),
        Some(&Effect::PaneShellClose),
        "{effects:?}"
    );
    let open = shell_open(&effects);
    assert!(open.dir.ends_with("web"));
    assert!(status_text(&app).contains("closed the shell in ~/workspace/billing-ui"));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    // Its generation is the one that counts now: the old one's end says nothing.
    let screen = PaneScreen::new(open.rows, open.cols);
    app.pane_opened(open.generation, screen);
    assert!(app.pane_shell_showing());
    assert!(app.pane_exited(open.generation - 1, Some(0)).is_empty());
    assert!(app.pane_shell_showing());
}

#[test]
fn the_shell_ending_returns_the_pane_to_the_row_with_the_list_focused() {
    let (mut app, t) = attached_to("builds");
    let (generation, _screen) = shell_on(&mut app, "builds");
    let effects = app.pane_exited(generation, Some(0));
    assert_eq!(effects, [Effect::PollNow]);
    assert!(status_text(&app).contains("back from the shell in ~/workspace/billing-ui"));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(!app.pane_shell_showing());
    assert!(
        app.pane_client_key().is_some(),
        "the client was never touched"
    );
    let text = draw(&mut app, 160, 40);
    assert!(!text.contains("SHELL SCREEN"), "{text}");
    assert!(app.pane_tick(t).is_empty());
    // A failure says so; a shell hidden when it ends leaves the keys alone.
    let (generation, _screen) = shell_on(&mut app, "builds");
    app.pane_exited(generation, Some(3));
    assert!(status_text(&app).contains("exited with code 3"));
    let (generation, _screen) = shell_on(&mut app, "builds");
    app.handle_key(ctrl('z'));
    common::select_name(&mut app, "reviews");
    app.handle_key(key(KeyCode::Enter));
    app.pane_exited(generation, None);
    assert!(status_text(&app).contains("ended by a signal"));
}

#[test]
fn bang_on_a_stopped_row_wakes_nothing_and_a_gone_folder_starts_nothing() {
    let mut app = at_home();
    common::select_name(&mut app, "sleeps");
    let effects = app.handle_key(ch('!'));
    let open = shell_open(&effects);
    assert!(open.dir.ends_with("infra"));
    no_foreground_attach(&effects);
    assert!(!effects.contains(&Effect::PollForAttach), "{effects:?}");
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    let t = Instant::now();
    assert!(!opened(&app.pane_tick(t + ATTACH_DEBOUNCE)));
    // Enter types in the shell; only after `!` hides it does Enter wake.
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(!effects.contains(&Effect::PollForAttach), "{effects:?}");
    app.handle_key(ctrl('z'));
    app.handle_key(ch('!'));
    let text = draw(&mut app, 160, 40);
    assert!(text.contains("Enter wakes it here"), "{text}");
    assert!(
        app.handle_key(key(KeyCode::Enter))
            .contains(&Effect::PollForAttach)
    );

    let mut app = at_home();
    common::select_name(&mut app, "sleeps");
    let open = shell_open(&app.handle_key(ch('!')));
    app.pane_shell_folder_gone(open.generation);
    assert!(status_text(&app).contains("no longer exists; no shell started"));
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(!app.pane_shell_showing());
}

#[test]
fn a_shell_ends_with_its_row_and_with_the_style_and_a_failed_start_keeps_the_workspace() {
    let mut app = at_home();
    let (_generation, _screen) = shell_on(&mut app, "builds");
    let without = listing()
        .replace("\"builds\"", "\"x\"")
        .replace("bbbb2222", "zzzz9999");
    app.apply_poll(poll("main", &without));
    let effects = app.pane_tick(Instant::now());
    assert!(effects.contains(&Effect::PaneShellClose), "{effects:?}");
    assert!(
        status_text(&app).contains("its session is gone"),
        "{}",
        status_text(&app)
    );
    assert_eq!(app.pane_focus(), PaneFocus::List);

    let mut app = at_home();
    shell_on(&mut app, "builds");
    app.style = UiStyle::Dashboard;
    draw(&mut app, 160, 40);
    let effects = app.pane_tick(Instant::now());
    assert!(effects.contains(&Effect::PaneShellClose), "{effects:?}");

    // Too narrow for a while: it waits, and shows again.
    let mut app = at_home();
    shell_on(&mut app, "builds");
    draw(&mut app, 90, 30);
    assert!(
        !app.pane_tick(Instant::now())
            .contains(&Effect::PaneShellClose)
    );
    assert_eq!(app.pane_focus(), PaneFocus::List);
    assert!(draw(&mut app, 160, 40).contains("SHELL SCREEN"));

    // No PTY for it: a message, and the workspace goes on.
    let mut app = at_home();
    common::select_name(&mut app, "builds");
    let open = shell_open(&app.handle_key(ch('!')));
    app.pane_start_failed(open.generation, "no pty");
    assert!(
        status_text(&app).contains("no shell started"),
        "{}",
        status_text(&app)
    );
    assert!(!app.pane_failed());
    assert!(draw(&mut app, 160, 40).contains(" LIST "));
}

#[test]
fn below_the_width_bang_stays_the_foreground_hand_over() {
    let mut app = app_with(&listing(), "", "");
    draw(&mut app, 90, 30);
    common::select_name(&mut app, "builds");
    let effects = app.handle_key(ch('!'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Shell(_))),
        "{effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PaneShellOpen(_)))
    );
    // The other styles too.
    let mut app = app_with(&listing(), "", "");
    app.style = UiStyle::Dashboard;
    draw(&mut app, 160, 40);
    common::select_name(&mut app, "builds");
    let effects = app.handle_key(ch('!'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Shell(_))),
        "{effects:?}"
    );
}

#[test]
fn the_shell_clears_no_attention_and_keeps_the_client_from_counting_keys() {
    let (mut app, t) = attached_to("asks");
    assert_eq!(app.attention_count(), 1);
    common::select_name(&mut app, "asks");
    let open = shell_open(&app.handle_key(ch('!')));
    app.pane_opened(open.generation, PaneScreen::new(open.rows, open.cols));
    assert_eq!(app.pane_focus(), PaneFocus::Terminal);
    assert!(!app.pane_tick(t).contains(&Effect::SaveSidecar));
    assert_eq!(app.attention_count(), 1, "the shell answers nothing");
    // The client's clock for its release keeps running.
    let effects = app.pane_tick(t + RELEASE_AFTER);
    assert!(effects.contains(&Effect::PaneClose), "{effects:?}");
    assert!(!effects.contains(&Effect::PaneShellClose), "{effects:?}");
    assert!(app.pane_shell_showing());
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
