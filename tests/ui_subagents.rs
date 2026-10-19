//! The running subagents of a session drawn as a tree under its row
//! (doc/0029, "Display"), in both styles and several themes: the count
//! badge, up to four child lines and `… N more`, the selection covering
//! them, navigation skipping them, collapsed groups, project grouping, the
//! filter, the viewport, narrow terminals and the `N of M shown` counter.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use common::{HOME, NoEnviron, ch, key, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;
use watchtower::app::{App, Entry};
use watchtower::config::{Config, UiStyle, UiTheme};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;
use watchtower::subagents::Subagent;

const NOW_MS: i64 = 1_800_000_000_000;

/// A background row; live (with a pid) when it has a `status`.
fn bg(id: &str, name: &str, cwd: &str, state: &str, status: Option<&str>, ago_s: i64) -> Value {
    let mut row = json!({
        "id": id,
        "sessionId": sid(id),
        "name": name,
        "cwd": format!("/home/user/workspace/{cwd}"),
        "kind": "background",
        "state": state,
        "startedAt": NOW_MS - ago_s * 1000,
    });
    if let Some(status) = status {
        row["status"] = json!(status);
        row["pid"] = json!(4_194_000);
        if state == "blocked" {
            row["waitingFor"] = json!("input needed");
        }
    }
    row
}

fn sid(id: &str) -> String {
    format!("{id}-0000-4000-8000-000000000000")
}

fn rows() -> Vec<Value> {
    vec![
        bg(
            "a1a1a1a1",
            "schema-migration",
            "billing-ui",
            "working",
            Some("busy"),
            300,
        ),
        bg(
            "b2b2b2b2",
            "disk-analyze",
            "analysis",
            "working",
            Some("busy"),
            1740,
        ),
        bg("c3c3c3c3", "four-way", "api", "working", Some("busy"), 400),
        bg(
            "d4d4d4d4",
            "six-pack",
            "infra",
            "working",
            Some("busy"),
            500,
        ),
        bg(
            "e5e5e5e5",
            "pick-colour",
            "web",
            "blocked",
            Some("waiting"),
            42,
        ),
        bg("f6f6f6f6", "print-date", "api", "done", Some("idle"), 720),
        bg("a7a7a7a7", "say-ok", "api", "done", None, 7200),
    ]
}

fn update(rows: &[Value]) -> PollUpdate {
    PollUpdate {
        account: "main".into(),
        cycle: 0,
        result: Ok(parse_listing(&serde_json::to_string(rows).unwrap()).unwrap()),
    }
}

fn agent(id: &str, ty: &str, desc: Option<&str>, ago_s: i64) -> Subagent {
    Subagent {
        id: id.into(),
        agent_type: ty.into(),
        description: desc.map(Into::into),
        started_ms: NOW_MS - ago_s * 1000,
    }
}

fn many(prefix: &str, n: usize) -> Vec<Subagent> {
    (0..n)
        .map(|k| {
            agent(
                &format!("{prefix}{k}"),
                "general-purpose",
                Some(&format!("{prefix} task {k}")),
                600 - k as i64 * 60,
            )
        })
        .collect()
}

/// The subagents of the working rows: 3, 1, 4 and 6; also a stale file of
/// the blocked row, which is never shown (its state is not `working`).
fn agents() -> HashMap<String, Vec<Subagent>> {
    HashMap::from([
        (
            sid("a1a1a1a1"),
            vec![
                agent(
                    "aa01",
                    "general-purpose",
                    Some("Internet research Queue→Stream"),
                    130,
                ),
                agent("aa02", "general-purpose", Some("Related docs search"), 125),
                agent("aa03", "Explore", Some("Codebase queue audit"), 70),
            ],
        ),
        (
            sid("b2b2b2b2"),
            vec![agent(
                "bb01",
                "general-purpose",
                Some("Pipeline watcher"),
                240,
            )],
        ),
        (sid("c3c3c3c3"), many("four", 4)),
        (sid("d4d4d4d4"), many("six", 6)),
        (sid("e5e5e5e5"), many("stale", 2)),
    ])
}

fn app_in(style: UiStyle, theme: UiTheme) -> App {
    let config = Config::defaults(Path::new(HOME));
    assert!(config.track_subagents, "on by default");
    app_with(config, style, theme)
}

fn app_with(config: Config, style: UiStyle, theme: UiTheme) -> App {
    let mut app = App::new(config, HOME.into(), Sidecar::default(), Box::new(NoEnviron));
    app.style = style;
    app.set_theme(theme);
    app.apply_poll(update(&rows()));
    app
}

/// The dashboard in `neutral`, with the subagents read.
fn app() -> App {
    with_agents(app_in(UiStyle::Dashboard, UiTheme::Neutral))
}

fn with_agents(mut app: App) -> App {
    app.subagents_read(agents());
    app
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    lines(&render(app, w, h))
}

fn index_of(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
    &lines[index_of(lines, needle)]
}

/// The cell where `needle` starts on the first line containing it.
fn cell_at(lines: &[String], needle: &str) -> (u16, u16) {
    let y = index_of(lines, needle);
    let prefix = &lines[y][..lines[y].find(needle).unwrap()];
    (prefix.width() as u16, y as u16)
}

/// The column where `needle` starts on line `y`.
fn col(lines: &[String], y: usize, needle: &str) -> usize {
    let at = lines[y]
        .find(needle)
        .unwrap_or_else(|| panic!("no {needle:?} in {}", lines[y]));
    lines[y][..at].width()
}

fn select(app: &mut App, name: &str) {
    app.handle_key(key(KeyCode::Home));
    for _ in 0..app.entries().len() {
        if app
            .selected_session()
            .is_some_and(|m| m.session.display_name() == name)
        {
            return;
        }
        app.handle_key(ch('j'));
    }
    panic!("cannot select {name}");
}

/// The lines from the one containing `from`, `n` of them.
fn block(l: &[String], from: &str, n: usize) -> Vec<String> {
    let y = index_of(l, from);
    l[y..y + n].to_vec()
}

#[test]
fn dashboard_draws_badges_and_up_to_four_child_lines() {
    let mut app = app();
    select(&mut app, "pick-colour");
    let l = screen(&mut app, 160, 40);
    let text = l.join("\n");

    // 3 agents: three lines, the last with `└─`.
    let b = block(&l, "schema-migration", 4);
    assert!(b[0].contains("schema-migration  +3"), "{text}");
    assert!(b[1].contains("├─ Internet research Queue→Stream"), "{text}");
    assert!(b[2].contains("├─ Related docs search"), "{text}");
    assert!(b[3].contains("└─ Codebase queue audit"), "{text}");
    // 1 agent.
    let b = block(&l, "disk-analyze", 2);
    assert!(b[0].contains("+1"), "{text}");
    assert!(b[1].contains("└─ Pipeline watcher"), "{text}");
    // 4 agents: all four.
    let b = block(&l, "four-way", 6);
    assert!(b[0].contains("+4"), "{text}");
    for (k, line) in b[1..5].iter().enumerate() {
        let glyph = if k == 3 { "└─" } else { "├─" };
        assert!(line.contains(&format!("{glyph} four task {k}")), "{text}");
    }
    assert!(!b[5].contains("├─") && !b[5].contains("└─"), "{text}");
    // 6 agents: four, then `… 2 more`.
    let b = block(&l, "six-pack", 6);
    assert!(b[0].contains("+6"), "{text}");
    assert!(b[4].contains("├─ six task 3"), "{text}");
    assert!(b[5].contains("└─ … 2 more"), "{text}");
    assert!(!text.contains("six task 4"), "{text}");
    // A row that is not working shows no subagents, even with a file.
    assert!(!text.contains("stale task"), "{text}");
    assert!(!line_with(&l, "pick-colour").contains("+2"), "{text}");

    // Badges line up at the end of the name column; types line up in a
    // column of their own, ages in the age column.
    let y = index_of(&l, "schema-migration");
    let badge_x = col(&l, y, "+3");
    let y1 = index_of(&l, "disk-analyze");
    assert_eq!(col(&l, y1, "+1"), badge_x, "{text}");
    let type_x = col(&l, y + 1, "general-purpose");
    assert!(type_x > col(&l, y + 1, "Queue→Stream"), "{text}");
    assert_eq!(col(&l, y + 3, "Explore"), type_x, "{text}");
    assert_eq!(col(&l, y1 + 1, "general-purpose"), type_x, "{text}");
    let age_end = col(&l, y, "5m") + 2;
    assert_eq!(col(&l, y + 1, "2m") + 2, age_end, "{text}");
    assert_eq!(col(&l, y + 3, "1m") + 2, age_end, "{text}");
    let y4 = index_of(&l, "disk-analyze");
    assert_eq!(col(&l, y4 + 1, "4m") + 2, age_end, "{text}");
    // The tree hangs under the name.
    let name_x = col(&l, y, "schema-migration");
    assert_eq!(col(&l, y + 1, "├─"), name_x + 1, "{text}");
    // The counter counts sessions, not lines (Stopped & finished is
    // folded).
    assert!(
        line_with(&l, "╭ Sessions").contains("6 of 7 shown"),
        "{text}"
    );
}

#[test]
fn without_subagents_nothing_changes() {
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut plain = app_in(style, UiTheme::Neutral);
        // Only the blocked row has a file: nothing is shown.
        let mut stale = app_in(style, UiTheme::Neutral);
        stale.subagents_read(HashMap::from([(sid("e5e5e5e5"), many("stale", 2))]));
        // Tracking off: the files are ignored.
        let mut config = Config::defaults(Path::new(HOME));
        config.track_subagents = false;
        let mut off = app_with(config, style, UiTheme::Neutral);
        off.subagents_read(agents());
        for app in [&mut plain, &mut stale, &mut off] {
            select(app, "four-way");
        }
        for (w, h) in [(160, 40), (100, 30), (80, 24), (60, 20)] {
            let base = render(&mut plain, w, h);
            assert_eq!(base, render(&mut stale, w, h), "{style:?} {w}x{h}");
            assert_eq!(base, render(&mut off, w, h), "{style:?} {w}x{h} off");
        }
    }
}

#[test]
fn navigation_skips_the_child_lines() {
    let mut plain = app_in(UiStyle::Dashboard, UiTheme::Neutral);
    let mut app = app();
    assert_eq!(
        app.entries(),
        plain.entries(),
        "child lines are not entries"
    );
    select(&mut app, "schema-migration");
    let name = |app: &App| app.selected_session().map(|m| m.session.display_name());
    app.handle_key(ch('j'));
    assert_eq!(name(&app).as_deref(), Some("four-way"));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(name(&app).as_deref(), Some("six-pack"));
    app.handle_key(ch('k'));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(name(&app).as_deref(), Some("schema-migration"));
    // Up again reaches the Working header, never a child line.
    app.handle_key(key(KeyCode::Up));
    assert!(app.selected_header().is_some());
    // Paging, Home/End and g/G land on the same entries as without.
    for k in [
        key(KeyCode::PageDown),
        key(KeyCode::PageUp),
        key(KeyCode::End),
        ch('g'),
        ch('G'),
        key(KeyCode::Home),
    ] {
        render(&mut app, 100, 30);
        render(&mut plain, 100, 30);
        app.handle_key(k);
        plain.handle_key(k);
        assert_eq!(app.selected_entry(), plain.selected_entry(), "{k:?}");
    }
    // Actions act on the session: the selected session, not a subagent.
    select(&mut app, "six-pack");
    let l = screen(&mut app, 160, 40);
    assert!(line_with(&l, "╭ six-pack").contains("six-pack"));
}

#[test]
fn the_selection_covers_the_row_and_its_children() {
    for theme in [
        UiTheme::Neutral,
        UiTheme::Warm,
        UiTheme::TokyoNight,
        UiTheme::Terminal,
    ] {
        let mut app = with_agents(app_in(UiStyle::Dashboard, theme));
        select(&mut app, "schema-migration");
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        let (x, y) = cell_at(&l, "⠋ schema-migration");
        let x = x + 2;
        let row_bg = buf[(x, y)].bg;
        let row_rev = buf[(x, y)].modifier.contains(Modifier::REVERSED);
        assert!(row_bg != Color::Reset || row_rev, "{theme:?}: selected");
        for dy in 0..4 {
            let line = &l[usize::from(y + dy)];
            assert!(line.starts_with("│▌"), "{theme:?}: {line}");
            let cell = &buf[(x + 3, y + dy)];
            assert_eq!(cell.bg, row_bg, "{theme:?} line {dy}");
            assert_eq!(
                cell.modifier.contains(Modifier::REVERSED),
                row_rev,
                "{theme:?} line {dy}"
            );
        }
        // The next session's lines are not selected.
        let next = &buf[(x, y + 4)];
        assert_eq!(next.bg, Color::Reset, "{theme:?}");
        assert!(!l[usize::from(y + 4)].starts_with("│▌"), "{theme:?}");
    }
}

#[test]
fn children_are_dimmed_in_every_theme() {
    for theme in UiTheme::ALL {
        let mut app = with_agents(app_in(UiStyle::Dashboard, theme));
        select(&mut app, "pick-colour");
        let buf = render(&mut app, 160, 40);
        let l = lines(&buf);
        // The description is muted like a row's project, the tree glyph,
        // type and age faint like the column header.
        let (px, py) = cell_at(&l, "~/workspace/analysis");
        let muted = buf[(px, py)].fg;
        let (hx, hy) = cell_at(&l, "NAME");
        let faint = buf[(hx, hy)].fg;
        let (nx, ny) = cell_at(&l, "disk-analyze");
        let text = buf[(nx, ny)].fg;
        assert_ne!(muted, text, "{theme:?}");
        let (x, y) = cell_at(&l, "└─ Pipeline watcher");
        assert_eq!(buf[(x, y)].fg, faint, "{theme:?} tree");
        assert_eq!(buf[(x + 3, y)].fg, muted, "{theme:?} description");
        let (tx, ty) = cell_at(&l, "general-purpose");
        assert_eq!(buf[(tx, ty)].fg, faint, "{theme:?} type");
        assert_eq!(buf[(x, y)].bg, Color::Reset, "{theme:?}: no background");
        if theme == UiTheme::Terminal {
            assert!(
                matches!(muted, Color::Gray) && matches!(faint, Color::DarkGray),
                "named colours"
            );
        }
    }
    // Without colour: no colour at all, the tree dim.
    let mut app = app();
    app.color = false;
    select(&mut app, "pick-colour");
    let buf = render(&mut app, 160, 40);
    assert!(
        buf.content()
            .iter()
            .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
    );
    let l = lines(&buf);
    let (x, y) = cell_at(&l, "└─ Pipeline watcher");
    assert!(buf[(x, y)].modifier.contains(Modifier::DIM));
}

#[test]
fn collapsed_groups_hide_the_children() {
    let mut app = app();
    select(&mut app, "schema-migration");
    app.handle_key(key(KeyCode::Tab));
    assert!(app.selected_header().is_some(), "folded to its header");
    let l = screen(&mut app, 160, 40);
    let text = l.join("\n");
    assert!(line_with(&l, "▸ Working 4").contains('─'), "{text}");
    assert!(!text.contains("├─ ") && !text.contains("└─ "), "{text}");
    assert!(!text.contains("+3"), "{text}");
    // The counter still counts sessions.
    assert!(
        line_with(&l, "╭ Sessions").contains("2 of 7 shown"),
        "{text}"
    );
    app.handle_key(key(KeyCode::Tab));
    let l = screen(&mut app, 160, 40);
    assert!(l.iter().any(|x| x.contains("└─ Pipeline watcher")));
}

#[test]
fn children_follow_their_session_in_project_grouping() {
    let mut app = app();
    app.handle_key(ch('p'));
    let l = screen(&mut app, 160, 40);
    let text = l.join("\n");
    let header = index_of(&l, "▾ ~/workspace/billing-ui");
    assert!(l[header + 1].contains("schema-migration  +3"), "{text}");
    assert!(l[header + 2].contains("├─ Internet research"), "{text}");
    assert!(l[header + 4].contains("└─ Codebase queue audit"), "{text}");
    let infra = index_of(&l, "▾ ~/workspace/infra");
    assert!(l[infra + 1].contains("six-pack"), "{text}");
    assert!(l[infra + 6].contains("… 2 more"), "{text}");
}

#[test]
fn the_filter_matches_sessions_not_child_text() {
    let mut app = app();
    app.handle_key(ch('/'));
    type_text(&mut app, "Pipeline");
    app.handle_key(key(KeyCode::Enter));
    let l = screen(&mut app, 160, 40);
    let text = l.join("\n");
    assert!(!text.contains("Pipeline watcher"), "{text}");
    assert!(!text.contains("disk-analyze"), "{text}");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('/'));
    type_text(&mut app, "disk");
    app.handle_key(key(KeyCode::Enter));
    let l = screen(&mut app, 160, 40);
    let text = l.join("\n");
    assert!(text.contains("disk-analyze"), "{text}");
    assert!(text.contains("└─ Pipeline watcher"), "{text}");
    assert!(!text.contains("schema"), "{text}");
    assert!(
        line_with(&l, "╭ Sessions").contains("1 of 7 shown"),
        "{text}"
    );
}

#[test]
fn the_viewport_keeps_the_selection_and_its_children_in_view() {
    let mut list = rows();
    for k in 0..20 {
        list.push(bg(
            &format!("{k:08x}"),
            &format!("job-{k:02}"),
            "api",
            "done",
            Some("idle"),
            60 + k,
        ));
    }
    // Every working row comes before the 20 done ones; select the last
    // working row with six agents after scrolling to the bottom.
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = app_in(style, UiTheme::Neutral);
        app.apply_poll(update(&list));
        app.subagents_read(agents());
        app.handle_key(key(KeyCode::End));
        render(&mut app, 100, 30);
        select(&mut app, "six-pack");
        // Bottom up: from the end of the list back to six-pack.
        app.handle_key(key(KeyCode::End));
        render(&mut app, 100, 30);
        while app
            .selected_session()
            .is_none_or(|m| m.session.display_name() != "six-pack")
        {
            app.handle_key(ch('k'));
            let l = screen(&mut app, 100, 30);
            let name = app
                .selected_session()
                .map(|m| m.session.display_name())
                .unwrap_or_default();
            if !name.is_empty() {
                assert!(l.iter().any(|x| x.contains(&name)), "{style:?} {name}");
            }
        }
        let l = screen(&mut app, 100, 30);
        let text = l.join("\n");
        assert!(text.contains("six-pack"), "{style:?}\n{text}");
        assert!(text.contains("… 2 more"), "{style:?}\n{text}");
        // Walking down from the top keeps each selected block whole.
        app.handle_key(key(KeyCode::Home));
        for _ in 0..app.entries().len() {
            let l = screen(&mut app, 100, 30);
            if let Some(m) = app.selected_session() {
                let name = m.session.display_name();
                let y = index_of(&l, &name);
                let kids = app.subagents_of(&m.session).len().min(4)
                    + usize::from(app.subagents_of(&m.session).len() > 4);
                for dy in 1..=kids {
                    let line = &l[y + dy];
                    assert!(
                        line.contains("├─") || line.contains("└─"),
                        "{style:?} {name} +{dy}\n{}",
                        l.join("\n")
                    );
                }
            }
            app.handle_key(ch('j'));
        }
    }
}

#[test]
fn the_minimal_style_draws_the_tree_dimmed_under_the_highlight() {
    let mut app = with_agents(app_in(UiStyle::Minimal, UiTheme::Neutral));
    select(&mut app, "schema-migration");
    let buf = render(&mut app, 120, 30);
    let l = lines(&buf);
    let text = l.join("\n");
    assert!(l[0].contains("7 sessions"), "{text}");
    let y = index_of(&l, "schema-migration");
    assert!(l[y].starts_with("▶ "), "{text}");
    assert!(l[y].contains("schema-migration  +3"), "{text}");
    assert!(
        l[y + 1].contains("├─ Internet research Queue→Stream"),
        "{text}"
    );
    assert!(l[y + 3].contains("└─ Codebase queue audit"), "{text}");
    // Types in one column, age in the age column.
    assert_eq!(
        col(&l, y + 3, "Explore"),
        col(&l, y + 1, "general-purpose"),
        "{text}"
    );
    assert_eq!(col(&l, y + 1, "2m") + 2, col(&l, y, "5m") + 2, "{text}");
    // The highlight covers the children; they are dim.
    for dy in 1..4 {
        let x = col(&l, y + dy, "─") as u16;
        let cell = &buf[(x, (y + dy) as u16)];
        assert!(cell.modifier.contains(Modifier::REVERSED), "line {dy}");
        assert!(cell.modifier.contains(Modifier::DIM), "line {dy}");
    }
    let b = block(&l, "six-pack", 6);
    assert!(b[5].contains("└─ … 2 more"), "{text}");
    let b = block(&l, "four-way", 5);
    assert!(b[4].contains("└─ four task 3"), "{text}");
    assert!(block(&l, "disk-analyze", 2)[1].contains("└─ Pipeline watcher"));
    // Navigation skips them here as well.
    app.handle_key(ch('j'));
    assert_eq!(
        app.selected_session().unwrap().session.display_name(),
        "four-way"
    );
    // A collapsed group hides them.
    app.handle_key(key(KeyCode::Tab));
    let text = screen(&mut app, 120, 30).join("\n");
    assert!(!text.contains("├─") && !text.contains("+3"), "{text}");
}

#[test]
fn narrow_terminals_cut_descriptions_first_then_show_only_badges() {
    // The smallest dashboard keeps type and age and cuts descriptions.
    let mut app = app();
    select(&mut app, "pick-colour");
    let l = screen(&mut app, 80, 24);
    let text = l.join("\n");
    let y = index_of(&l, "├─ Internet");
    assert!(
        l[y].contains('…') || l[y].contains("Queue→Stream"),
        "{text}"
    );
    assert!(
        l[y].trim_end_matches('│').trim_end().ends_with("2m"),
        "{text}"
    );
    assert!(l.iter().all(|x| x.width() <= 80));
    // Without a column header the first separator is not joined to the
    // border, as without subagents.
    let top = index_of(&l, "╭ Sessions");
    assert!(l[top + 1].starts_with("│ ▾ Needs me"), "{text}");

    // A narrow minimal list: below the child lines' width only badges.
    let mut app = with_agents(app_in(UiStyle::Minimal, UiTheme::Neutral));
    let l = screen(&mut app, 18, 20);
    let text = l.join("\n");
    assert!(!text.contains("├─") && !text.contains("└─"), "{text}");
    let l = screen(&mut app, 60, 30);
    let text = l.join("\n");
    let y = index_of(&l, "├─ Internet");
    assert!(l[y].ends_with("2m"), "the age stays in view:\n{text}");
    assert!(l[y].contains('…'), "{text}");

    // Never a panic, at any size, in either style.
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = with_agents(app_in(style, UiTheme::Neutral));
        select(&mut app, "six-pack");
        for w in [1, 2, 5, 10, 20, 30, 40, 50, 79, 80, 81, 100, 129, 130, 200] {
            for h in [1, 3, 6, 10, 24, 50] {
                render(&mut app, w, h);
            }
        }
    }
}

#[test]
fn entries_keep_counting_sessions() {
    let app = app();
    let rows = app
        .entries()
        .iter()
        .filter(|e| matches!(e, Entry::Row(_)))
        .count();
    assert_eq!(rows, 6, "stopped & finished starts folded");
    let mut app = app;
    let l = screen(&mut app, 100, 30);
    assert!(line_with(&l, "╭ Sessions").contains("6 of 7 shown"));
}

#[test]
#[ignore = "prints the screens for a look"]
fn dump() {
    for style in [UiStyle::Dashboard, UiStyle::Minimal] {
        let mut app = with_agents(app_in(style, UiTheme::Neutral));
        select(&mut app, "schema-migration");
        for (w, h) in [(160, 40), (100, 30), (80, 24), (60, 20)] {
            println!("{style:?} {w}x{h}\n{}", screen(&mut app, w, h).join("\n"));
        }
    }
}
