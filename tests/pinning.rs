//! Pinned sessions (doc/0047): the `P` key, the Pinned group, sorting within a
//! project, the sidecar's `pins` keyed by the short id, and their ageing.

mod common;

use std::path::Path;

use common::{HOME, NoEnviron, ch, key, status_text, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect, Entry};
use watchtower::config::{Config, UiStyle};
use watchtower::groups::{self, Group, GroupKey, Grouping};
use watchtower::listing::{Session, parse_listing};
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, PRUNE_UNSEEN_MS, SEEN_RESOLUTION_MS, Sidecar};
use watchtower::ui;

const NOW_MS: i64 = 1_800_000_000_000;

fn row(id: &str, session: &str, name: &str, folder: &str, process: &str, state: &str) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{session}",
            "cwd": "{HOME}/workspace/{folder}", "kind": "background", "name": "{name}",
            "state": "{state}", "startedAt": {NOW_MS} }}"#
    )
}

fn sid(id: &str) -> String {
    format!("{id}-0000-4000-8000-000000000000")
}

/// One row of each kind that matters here, in two projects.
fn listing() -> String {
    let idle = r#""pid": 4100, "status": "idle","#;
    let rows = [
        row(
            "aaaa1111",
            &sid("aaaa1111"),
            "alpha",
            "api",
            r#""pid": 4101, "status": "busy","#,
            "working",
        ),
        row(
            "bbbb2222",
            &sid("bbbb2222"),
            "beta",
            "api",
            r#""pid": 4102, "status": "waiting", "waitingFor": "input needed","#,
            "blocked",
        ),
        row("cccc3333", &sid("cccc3333"), "gamma", "api", idle, "done"),
        row("dddd4444", &sid("dddd4444"), "delta", "web", "", "done"),
        row(
            "eeee5555",
            &sid("eeee5555"),
            "orchestrator",
            "orch",
            idle,
            "done",
        ),
        format!(
            r#"{{ "pid": 4200, "cwd": "{HOME}/workspace/api", "kind": "interactive",
                "startedAt": {NOW_MS}, "sessionId": "ffff-0000", "name": "inter" }}"#
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

fn app_with(sidecar: Sidecar, text: &str) -> App {
    let config = Config::parse(
        "default_account = \"main\"\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n",
        Path::new(HOME),
    )
    .unwrap();
    let mut app = App::new(config, HOME.into(), sidecar, Box::new(NoEnviron));
    app.apply_poll(poll(text));
    app
}

fn app() -> App {
    app_with(Sidecar::default(), &listing())
}

/// `P` on the row named `name`.
fn toggle(app: &mut App, name: &str) -> Vec<Effect> {
    common::select_name(app, name);
    app.handle_key(ch('P'))
}

/// The list as text: `[Pinned]`, `alpha`, ….
fn outline(app: &App) -> Vec<String> {
    app.entries()
        .iter()
        .map(|e| match e {
            Entry::Header { group, .. } => format!("[{}]", group.title()),
            Entry::Row(i) => app.sessions()[*i].session.display_name(),
        })
        .collect()
}

/// Selects the header titled `title`.
fn select_header(app: &mut App, title: &str) {
    app.handle_key(key(KeyCode::Home));
    loop {
        if app.selected_header().is_some_and(|g| g.title() == title) {
            return;
        }
        let before = app.selected_entry();
        app.handle_key(ch('j'));
        assert_ne!(app.selected_entry(), before, "no header {title}");
    }
}

fn header_of<'a>(app: &'a App, title: &str) -> &'a Entry {
    app.entries()
        .iter()
        .find(|e| matches!(e, Entry::Header { group, .. } if group.title() == title))
        .unwrap_or_else(|| panic!("no header {title} in {:?}", outline(app)))
}

fn draw(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    // The workspace knows its pane from the first frame on.
    terminal
        .draw(|frame| ui::draw(frame, app, std::time::Instant::now()))
        .unwrap();
    terminal
        .draw(|frame| ui::draw(frame, app, std::time::Instant::now()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

// ---- the group ------------------------------------------------------------

#[test]
fn a_pinned_row_sits_in_its_own_group_right_after_the_orchestrator() {
    let mut app = app();
    assert!(!outline(&app).contains(&"[Pinned]".to_owned()));
    let effects = toggle(&mut app, "alpha");
    assert_eq!(effects, [Effect::SaveSidecar]);
    assert!(status_text(&app).contains("pinned alpha"));
    let list = outline(&app);
    assert_eq!(
        &list[..4],
        ["[Orchestrator]", "orchestrator", "[Pinned]", "alpha"]
    );
    // It left its state's group, and the selection stays on it.
    assert!(!list.contains(&"[Working]".to_owned()), "{list:?}");
    assert_eq!(
        app.selected_session().unwrap().session.display_name(),
        "alpha"
    );
}

#[test]
fn unpinning_puts_it_back_where_its_state_goes() {
    let mut app = app();
    let before = outline(&app);
    toggle(&mut app, "alpha");
    assert_ne!(outline(&app), before);
    assert_eq!(toggle(&mut app, "alpha"), [Effect::SaveSidecar]);
    assert!(status_text(&app).contains("unpinned alpha"));
    assert_eq!(outline(&app), before);
    assert!(app.sidecar.pins.is_empty());
}

#[test]
fn a_pinned_row_keeps_its_state_and_its_attention_counts() {
    let mut app = app();
    assert_eq!(app.attention_count(), 1);
    toggle(&mut app, "beta");
    let m = app.selected_session().unwrap();
    assert_eq!(groups::classify(&m.session), Group::NeedsMe);
    assert_eq!(groups::detail(&m.session), "input needed");
    assert!(app.is_highlighted(m), "still highlighted");
    assert_eq!(app.attention_count(), 1, "the counter still counts it");
    // The header of its group shows it, and the state's pill still counts it.
    let Entry::Header {
        count, highlighted, ..
    } = header_of(&app, "Pinned")
    else {
        unreachable!()
    };
    assert_eq!((*count, *highlighted), (1, 1));
    assert_eq!(app.highlighted_in(Group::NeedsMe), 1);
    assert!(!outline(&app).contains(&"[Needs me]".to_owned()));
}

#[test]
fn a_stopped_row_pinned_is_not_folded_away() {
    let mut app = app();
    assert!(!outline(&app).contains(&"delta".to_owned()), "folded");
    // Walking to it unfolds its group; once pinned that group has no rows.
    toggle(&mut app, "delta");
    let list = outline(&app);
    assert!(list.contains(&"delta".to_owned()));
    assert!(
        !list.contains(&"[Stopped & finished]".to_owned()),
        "{list:?}"
    );
    let m = app.selected_session().unwrap();
    assert_eq!(groups::detail(&m.session), "stopped");
}

#[test]
fn folding_filter_and_counts_work_on_the_pinned_group() {
    let mut app = app();
    toggle(&mut app, "alpha");
    toggle(&mut app, "gamma");
    let Entry::Header { count, .. } = header_of(&app, "Pinned") else {
        unreachable!()
    };
    assert_eq!(*count, 2);

    // `Tab` folds it; the count stays; the rows go.
    select_header(&mut app, "Pinned");
    app.handle_key(key(KeyCode::Tab));
    assert!(app.is_collapsed(&GroupKey::State(Group::Pinned)));
    assert!(!outline(&app).contains(&"alpha".to_owned()));
    let Entry::Header {
        count, collapsed, ..
    } = header_of(&app, "Pinned")
    else {
        unreachable!()
    };
    assert_eq!((*count, *collapsed), (2, true));
    app.handle_key(key(KeyCode::Tab));

    // The filter reaches into it, and hides it when nothing matches.
    app.handle_key(ch('/'));
    type_text(&mut app, "gam");
    assert_eq!(outline(&app), ["[Pinned]", "gamma"]);
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('/'));
    type_text(&mut app, "beta");
    assert_eq!(outline(&app), ["[Needs me]", "beta"]);
    app.handle_key(key(KeyCode::Esc));
}

#[test]
fn what_cannot_be_pinned_says_so() {
    let mut app = app();
    assert!(toggle(&mut app, "orchestrator").is_empty());
    assert!(status_text(&app).contains("own group"));
    assert!(toggle(&mut app, "inter").is_empty());
    assert!(status_text(&app).contains("only background sessions"));
    assert!(app.sidecar.pins.is_empty());
    select_header(&mut app, "Working");
    assert!(app.handle_key(ch('P')).is_empty());
    assert!(status_text(&app).contains("select its row"));
}

#[test]
fn a_cyrillic_layout_reaches_p_too() {
    let mut app = app();
    common::select_name(&mut app, "alpha");
    // `З` is on the `P` key of a QWERTY keyboard, shifted (doc/0011).
    assert_eq!(app.handle_key(ch('З')), [Effect::SaveSidecar]);
    assert!(app.sidecar.is_pinned("aaaa1111"));
}

// ---- project grouping -----------------------------------------------------

#[test]
fn by_project_a_pinned_row_sorts_first_within_its_project() {
    let mut app = app();
    app.handle_key(ch('p'));
    assert_eq!(app.grouping(), Grouping::Project);
    let list = outline(&app);
    let api = list.iter().position(|l| l == "[~/workspace/api]").unwrap();
    assert_eq!(&list[api + 1..api + 4], ["beta", "gamma", "alpha"]);

    toggle(&mut app, "alpha");
    let list = outline(&app);
    let api = list.iter().position(|l| l == "[~/workspace/api]").unwrap();
    assert_eq!(&list[api + 1..api + 4], ["alpha", "beta", "gamma"]);
    // No Pinned group here; the projects stay as they are.
    assert!(!list.contains(&"[Pinned]".to_owned()), "{list:?}");
    assert_eq!(list[0], "[Orchestrator]");
}

#[test]
fn back_by_state_the_pin_is_a_group_again() {
    let mut app = app();
    toggle(&mut app, "alpha");
    app.handle_key(ch('p'));
    app.handle_key(ch('p'));
    assert_eq!(&outline(&app)[2..4], ["[Pinned]", "alpha"]);
}

// ---- every style ----------------------------------------------------------

#[test]
fn every_style_draws_the_group_and_the_rows_state() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard, UiStyle::Workspace] {
        let mut app = app();
        app.style = style;
        draw(&mut app, 160, 40);
        // Past Home, in the workspace.
        app.handle_key(key(KeyCode::Down));
        toggle(&mut app, "beta");
        let screen = draw(&mut app, 160, 40);
        assert!(screen.contains("Pinned"), "{style:?}:\n{screen}");
        assert!(screen.contains("beta"), "{style:?}:\n{screen}");
        assert!(screen.contains("input needed"), "{style:?}:\n{screen}");
        assert!(screen.contains('●'), "{style:?}:\n{screen}");
        assert!(!screen.contains("▾ Needs me"), "{style:?}:\n{screen}");
    }
}

#[test]
fn a_cards_group_of_pinned_sessions_folds_like_any_other() {
    let mut app = app();
    app.style = UiStyle::Workspace;
    draw(&mut app, 160, 40);
    app.handle_key(key(KeyCode::Down));
    toggle(&mut app, "alpha");
    assert!(draw(&mut app, 160, 40).contains("alpha"));
    select_header(&mut app, "Pinned");
    app.handle_key(key(KeyCode::Tab));
    let screen = draw(&mut app, 160, 40);
    assert!(screen.contains("Pinned"), "{screen}");
    assert!(!screen.contains("▪ alpha"), "{screen}");
}

#[test]
fn the_details_say_it_is_pinned_and_a_group_counts_its_own_rows() {
    let mut app = app();
    app.style = UiStyle::Dashboard;
    toggle(&mut app, "alpha");
    let screen = draw(&mut app, 160, 40);
    assert!(screen.contains("Working · pinned"), "{screen}");
    select_header(&mut app, "Pinned");
    let screen = draw(&mut app, 160, 40);
    assert!(screen.contains("1 session"), "{screen}");
}

#[test]
fn home_still_counts_a_pinned_row_by_its_state() {
    let mut app = app();
    toggle(&mut app, "beta");
    app.style = UiStyle::Workspace;
    let screen = draw(&mut app, 160, 40);
    assert!(screen.contains("1 needs you"), "{screen}");
}

// ---- persistence ----------------------------------------------------------

#[test]
fn a_pin_survives_a_restart_of_tower() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut app = app();
    toggle(&mut app, "alpha");
    app.sidecar.save(&path).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(value["pins"]["aaaa1111"]["pinned_ms"].is_i64(), "{text}");

    let loaded = Sidecar::load(&path).sidecar;
    let app = app_with(loaded, &listing());
    assert_eq!(&outline(&app)[2..4], ["[Pinned]", "alpha"]);
}

#[test]
fn nothing_is_written_while_nothing_is_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut sidecar = Sidecar::default();
    sidecar.record_owner("x", "main", AccountSource::Started);
    sidecar.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("pins"), "{text}");
}

#[test]
fn the_session_id_changing_after_clear_does_not_lose_the_pin() {
    let mut app = app();
    toggle(&mut app, "alpha");
    let before = app.sidecar.clone();
    assert!(before.is_pinned("aaaa1111"));

    // `/clear`: the live row shows a new sessionId, the same short id
    // (reference doc, "`/clear` in a background session").
    let cleared = listing().replace(&sid("aaaa1111"), &sid("9999aaaa"));
    app.apply_poll(poll(&cleared));
    assert_eq!(&outline(&app)[2..4], ["[Pinned]", "alpha"]);
    assert_eq!(
        app.sessions()
            .iter()
            .find(|m| m.session.display_name() == "alpha")
            .and_then(|m| m.session.session_id.as_deref()),
        Some(sid("9999aaaa").as_str())
    );

    // Stopped, the row shows the original sessionId again.
    let stopped = listing().replace(r#""pid": 4101, "status": "busy","#, "");
    app.apply_poll(poll(&stopped));
    assert_eq!(&outline(&app)[2..4], ["[Pinned]", "alpha"]);
    assert_eq!(
        groups::detail(&app.sessions()[1].session),
        "stopped",
        "{:?}",
        outline(&app)
    );
}

// ---- ageing: the rules of the other records --------------------------------

fn seen<'a>(ids: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    ids.iter().copied()
}

#[test]
fn a_pin_not_listed_for_the_ageing_time_goes_only_on_a_complete_listing() {
    let mut sc = Sidecar::default();
    sc.toggle_pin("aaaa1111", NOW_MS);
    let later = NOW_MS + PRUNE_UNSEEN_MS;

    // A partial or failed listing proves nothing.
    assert!(!sc.observe_pins(seen(&[]), false, later));
    assert!(sc.is_pinned("aaaa1111"));
    // One moment before its time it stays.
    assert!(!sc.observe_pins(seen(&[]), true, later - 1));
    assert!(sc.is_pinned("aaaa1111"));
    // Listed, it is seen again, to the hour, and that restarts its age.
    assert!(sc.observe_pins(seen(&["aaaa1111"]), true, later - 1));
    assert!(!sc.observe_pins(seen(&[]), true, later));
    assert!(sc.is_pinned("aaaa1111"));
    // Gone for the whole time: dropped.
    assert!(sc.observe_pins(seen(&[]), true, later - 1 + PRUNE_UNSEEN_MS));
    assert!(!sc.is_pinned("aaaa1111"));
}

#[test]
fn the_seen_time_moves_only_to_the_hour() {
    let mut sc = Sidecar::default();
    sc.toggle_pin("aaaa1111", NOW_MS);
    assert!(!sc.observe_pins(seen(&["aaaa1111"]), true, NOW_MS + SEEN_RESOLUTION_MS - 1));
    assert!(sc.observe_pins(seen(&["aaaa1111"]), true, NOW_MS + SEEN_RESOLUTION_MS));
}

#[test]
fn an_app_keeps_a_listed_pin_and_drops_an_unlisted_one_by_age() {
    let mut sidecar = Sidecar::default();
    let long_ago = watchtower::sidecar::now_ms() - PRUNE_UNSEEN_MS - 1;
    sidecar.toggle_pin("aaaa1111", long_ago);
    sidecar.toggle_pin("00000000", long_ago);
    let app = app_with(sidecar, &listing());
    assert!(app.sidecar.is_pinned("aaaa1111"), "listed: kept");
    assert!(!app.sidecar.is_pinned("00000000"), "gone: dropped");
}

// ---- two towers -------------------------------------------------------------

/// Tower A and tower B after both read `path`.
fn two_towers(path: &Path, first: impl FnOnce(&mut Sidecar)) -> (Sidecar, Sidecar) {
    let mut a = Sidecar::default();
    first(&mut a);
    a.save(path).unwrap();
    (Sidecar::load(path).sidecar, Sidecar::load(path).sidecar)
}

#[test]
fn two_towers_pinning_keep_both_pins() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let (mut a, mut b) = two_towers(&path, |_| {});
    a.toggle_pin("aaaa1111", NOW_MS);
    b.toggle_pin("bbbb2222", NOW_MS);
    a.save(&path).unwrap();
    b.save(&path).unwrap();
    assert!(b.is_pinned("aaaa1111") && b.is_pinned("bbbb2222"));
}

#[test]
fn an_unpin_stands_against_a_tower_that_only_saw_the_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let (mut a, mut b) = two_towers(&path, |s| {
        s.toggle_pin("aaaa1111", NOW_MS);
    });
    assert!(!a.toggle_pin("aaaa1111", NOW_MS + 1), "unpinned");
    b.observe_pins(seen(&["aaaa1111"]), true, NOW_MS + SEEN_RESOLUTION_MS);
    b.save(&path).unwrap();
    a.save(&path).unwrap();
    assert!(!a.is_pinned("aaaa1111"));
    b.save(&path).unwrap();
    assert!(!b.is_pinned("aaaa1111"), "the unpin reached it");
    assert!(!Sidecar::load(&path).sidecar.is_pinned("aaaa1111"));
}

#[test]
fn a_new_pin_stands_against_an_old_unpin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let (mut a, mut b) = two_towers(&path, |s| {
        s.toggle_pin("aaaa1111", NOW_MS);
    });
    // B unpins and pins again, A unpins from the old pin.
    b.toggle_pin("aaaa1111", NOW_MS + 1);
    b.toggle_pin("aaaa1111", NOW_MS + 2);
    b.save(&path).unwrap();
    a.toggle_pin("aaaa1111", NOW_MS + 3);
    a.save(&path).unwrap();
    assert!(a.is_pinned("aaaa1111"), "the newer pin in the file stands");
}

#[test]
fn an_older_tower_keeps_the_pins_it_does_not_know() {
    // Written by a tower without this field: nothing lost, and the other way
    // round the field goes through `extra` on an older one (doc/0002).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(
        &path,
        r#"{ "version": 1, "sessions": {}, "pins": { "aaaa1111": { "pinned_ms": 7, "x": 1 } } }"#,
    )
    .unwrap();
    let mut sc = Sidecar::load(&path).sidecar;
    assert!(sc.is_pinned("aaaa1111"));
    assert_eq!(sc.pins["aaaa1111"].extra["x"], 1);
    sc.record_owner("k", "main", AccountSource::Started);
    sc.save(&path).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"x\": 1"));
}

// ---- the grouping functions -------------------------------------------------

fn rows() -> Vec<Session> {
    parse_listing(&listing()).unwrap().sessions
}

#[test]
fn the_plain_grouping_functions_pin_nothing() {
    let rows = rows();
    let groups = groups::group_by_state(&rows);
    assert!(groups.iter().all(|g| g.group != Group::Pinned));
}

#[test]
fn the_orchestrator_and_interactive_rows_never_land_in_pinned() {
    let rows = rows();
    let grouped = groups::group_by_state_pinned(&rows, |s| s.started_at, |_| true);
    let pinned = grouped.iter().find(|g| g.group == Group::Pinned).unwrap();
    assert_eq!(pinned.rows.len(), 4);
    assert!(grouped.iter().any(|g| g.group == Group::Orchestrator));
    assert!(grouped.iter().any(|g| g.group == Group::Interactive));
    // Their order is the rule of every group: last change, then name.
    let names: Vec<String> = pinned.rows.iter().map(|s| s.display_name()).collect();
    assert_eq!(names, ["alpha", "beta", "delta", "gamma"]);
}

#[test]
fn classify_never_says_pinned() {
    for s in rows() {
        assert_ne!(groups::classify(&s), Group::Pinned);
    }
}
