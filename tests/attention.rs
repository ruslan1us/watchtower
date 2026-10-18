//! Milestone 5a: activity times tower tracks itself (doc/0006) and the
//! visual-only attention highlight (doc/0007), driven through the app and
//! rendered with ratatui's `TestBackend`.

mod common;

use std::path::Path;
use std::time::Instant;

use common::{HOME, NO_STATE_FILE, NoEnviron, ch, key, run_sidecar_check};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use watchtower::app::{App, AttachOutcome, Effect};
use watchtower::config::Config;
use watchtower::groups::Group;
use watchtower::listing::{State, Status, WaitingFor, parse_listing};
use watchtower::merge::MergedSession;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{self, Sidecar, StateTuple};

/// The `sessionId` of a test row.
fn sid(id: &str) -> String {
    format!("{id}-0000-4000-8000-000000000000")
}

/// A background row with a live process when it has a `status`.
fn bg(id: &str, name: &str, state: &str, status: Option<&str>, waiting: Option<&str>) -> Value {
    let mut row = json!({
        "id": id,
        "sessionId": sid(id),
        "name": name,
        "cwd": "/home/user/workspace/demo-app",
        "kind": "background",
        "state": state,
        "startedAt": 1_000,
    });
    if let Some(status) = status {
        row["status"] = json!(status);
        row["pid"] = json!(4_194_304);
    }
    if let Some(w) = waiting {
        row["waitingFor"] = json!(w);
    }
    row
}

fn working(id: &str, name: &str) -> Value {
    bg(id, name, "working", Some("busy"), None)
}

fn permission(id: &str, name: &str) -> Value {
    bg(
        id,
        name,
        "blocked",
        Some("waiting"),
        Some("permission prompt"),
    )
}

fn with_started(mut row: Value, started: i64) -> Value {
    row["startedAt"] = json!(started);
    row
}

fn update(cycle: u64, rows: &[Value]) -> PollUpdate {
    PollUpdate {
        account: "main".into(),
        cycle,
        result: Ok(parse_listing(&serde_json::to_string(rows).unwrap()).unwrap()),
    }
}

fn app_with(sidecar: Sidecar) -> App {
    let home = Path::new(HOME);
    App::new(
        Config::defaults(home),
        home.into(),
        sidecar,
        Box::new(NoEnviron),
    )
}

fn app() -> App {
    app_with(Sidecar::default())
}

fn row<'a>(app: &'a App, name: &str) -> &'a MergedSession {
    app.sessions()
        .iter()
        .find(|m| m.session.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no row {name}"))
}

fn lit(app: &App, name: &str) -> bool {
    app.is_highlighted(row(app, name))
}

fn names(app: &App) -> Vec<String> {
    app.sessions()
        .iter()
        .map(|m| m.session.display_name())
        .collect()
}

fn tuple(state: State, status: Option<Status>, waiting: Option<WaitingFor>) -> StateTuple {
    StateTuple {
        state: Some(state),
        status,
        waiting_for: waiting,
    }
}

fn permission_tuple() -> StateTuple {
    tuple(
        State::Blocked,
        Some(Status::Waiting),
        Some(WaitingFor::PermissionPrompt),
    )
}

/// Selects the row named `name` with the arrow keys.
fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

// ---- activity times (doc/0006) ------------------------------------------

#[test]
fn first_sighting_uses_started_at_and_a_change_the_poll_time() {
    let mut app = app();
    let effects = app.apply_poll(update(1, &[with_started(working("aaaa0001", "a"), 1_234)]));
    assert_eq!(effects, [Effect::SaveSidecar]);
    let rec = &app.sidecar.sessions[&sid("aaaa0001")];
    assert_eq!(
        rec.last_state,
        Some(tuple(State::Working, Some(Status::Busy), None))
    );
    assert_eq!(rec.last_change_ms, Some(1_234), "first sighting: startedAt");
    assert_eq!(app.last_change_at(row(&app, "a"), 99), Some(1_234));

    // No change: the time stays and nothing is saved.
    let effects = app.apply_poll(update(2, &[with_started(working("aaaa0001", "a"), 1_234)]));
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        app.sidecar.sessions[&sid("aaaa0001")].last_change_ms,
        Some(1_234)
    );

    // A changed tuple: the poll time.
    let before = sidecar::now_ms();
    let effects = app.apply_poll(update(
        3,
        &[with_started(
            bg("aaaa0001", "a", "done", Some("idle"), None),
            1_234,
        )],
    ));
    let after = sidecar::now_ms();
    assert_eq!(effects, [Effect::SaveSidecar]);
    let at = app.sidecar.sessions[&sid("aaaa0001")]
        .last_change_ms
        .unwrap();
    assert!(
        (before..=after).contains(&at),
        "{before} <= {at} <= {after}"
    );
    assert_eq!(app.last_change_at(row(&app, "a"), after + 5), Some(at));
}

#[test]
fn a_row_without_started_at_is_first_seen_now() {
    let mut app = app();
    let mut r = working("aaaa0001", "a");
    r.as_object_mut().unwrap().remove("startedAt");
    let before = sidecar::now_ms();
    app.apply_poll(update(1, &[r]));
    let at = app.sidecar.sessions[&sid("aaaa0001")]
        .last_change_ms
        .unwrap();
    assert!(at >= before);
}

#[test]
fn rows_sort_by_last_change_newest_first_then_name() {
    let mut app = app();
    let rows = |b_status: &str| {
        vec![
            with_started(
                bg("bbbb0001", "b-old", "working", Some(b_status), None),
                1_000,
            ),
            with_started(working("cccc0001", "c-new"), 3_000),
            with_started(working("aaaa0001", "a-new"), 3_000),
        ]
    };
    app.apply_poll(update(1, &rows("busy")));
    assert_eq!(names(&app), ["a-new", "c-new", "b-old"]);
    // b-old changes (busy -> idle, still Working): now the newest.
    app.apply_poll(update(2, &rows("idle")));
    assert_eq!(names(&app), ["b-old", "a-new", "c-new"]);
    // A restart of tower keeps the order from the sidecar, not startedAt.
    let mut again = app_with(app.sidecar.clone());
    again.apply_poll(update(1, &rows("idle")));
    assert_eq!(names(&again), ["b-old", "a-new", "c-new"]);
}

#[test]
fn activity_times_are_saved_at_most_once_per_poll_cycle() {
    let mut app = app();
    assert_eq!(
        app.apply_poll(update(1, &[working("aaaa0001", "a")])),
        [Effect::SaveSidecar]
    );
    // Another change in the same cycle waits.
    let done = bg("aaaa0001", "a", "done", Some("idle"), None);
    assert!(
        app.apply_poll(update(1, std::slice::from_ref(&done)))
            .is_empty()
    );
    assert!(app.sidecar.is_dirty());
    // The next cycle saves it, even without a new change.
    assert_eq!(
        app.apply_poll(update(2, std::slice::from_ref(&done))),
        [Effect::SaveSidecar]
    );
    assert!(app.apply_poll(update(3, &[done])).is_empty());
}

#[test]
fn a_reload_never_records_the_older_listing_over_another_towers_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut app = app();
    app.apply_poll(update(1, &[working("aaaa0001", "a")]));
    let Some(watchtower::app::SidecarJob::Save { seq, job }) = app.start_save() else {
        panic!("save expected");
    };
    let result = job.run(&path, sidecar::LOCK_WAIT);
    app.sidecar_saved(seq, &job, result);

    // Another tower saw the session go blocked and saved that.
    let mut other = Sidecar::load(&path).sidecar;
    other.observe_state_at(&sid("aaaa0001"), &permission_tuple(), Some(1_000), 50_000);
    other.save(&path).unwrap();

    // This tower reloads; its last listing still shows `working`, which is
    // not recorded again: the other tower's change stands.
    run_sidecar_check(&mut app, &path);
    let rec = &app.sidecar.sessions[&sid("aaaa0001")];
    assert_eq!(rec.last_state, Some(permission_tuple()));
    assert_eq!(rec.last_change_ms, Some(50_000));
}

// ---- attention (doc/0007) ------------------------------------------------

#[test]
fn every_transition_kind_highlights() {
    let cases = [
        (
            bg(
                "aaaa0001",
                "a",
                "blocked",
                Some("waiting"),
                Some("permission prompt"),
            ),
            Group::NeedsMe,
        ),
        (
            bg("aaaa0001", "a", "blocked", Some("idle"), None),
            Group::NeedsMe,
        ),
        (
            bg(
                "aaaa0001",
                "a",
                "working",
                Some("waiting"),
                Some("input needed"),
            ),
            Group::NeedsMe,
        ),
        (
            bg("aaaa0001", "a", "failed", Some("idle"), None),
            Group::Failed,
        ),
    ];
    for (attention, group) in cases {
        let mut app = app();
        app.apply_poll(update(1, &[working("aaaa0001", "a")]));
        assert!(!lit(&app, "a"));
        assert_eq!(app.attention_count(), 0);
        app.apply_poll(update(2, std::slice::from_ref(&attention)));
        assert!(lit(&app, "a"), "{attention}");
        assert_eq!(app.attention_count(), 1);
        assert_eq!(app.highlighted_in(group), 1);
    }
    // Done, working and stopped rows are not highlighted.
    let mut app = app();
    app.apply_poll(update(
        1,
        &[
            bg("aaaa0001", "a", "done", Some("idle"), None),
            working("bbbb0001", "b"),
            bg("cccc0001", "c", "failed", None, None),
        ],
    ));
    assert_eq!(app.attention_count(), 0);
}

#[test]
fn startup_highlights_unless_the_tuple_was_acknowledged() {
    let record = |last: StateTuple, ack: Option<StateTuple>| {
        let mut sc = Sidecar::default();
        let rec = sc.sessions.entry(sid("aaaa0001")).or_default();
        rec.last_state = Some(last);
        rec.last_change_ms = Some(500);
        rec.acknowledged = ack;
        sc
    };
    let input = tuple(
        State::Blocked,
        Some(Status::Waiting),
        Some(WaitingFor::InputNeeded),
    );
    let working_tuple = tuple(State::Working, Some(Status::Busy), None);
    let cases = [
        // No record at all.
        (Sidecar::default(), true),
        // The same tuple acknowledged: stays quiet.
        (record(permission_tuple(), Some(permission_tuple())), false),
        // Another tuple acknowledged.
        (record(permission_tuple(), Some(input)), true),
        // Not acknowledged.
        (record(permission_tuple(), None), true),
        // Acknowledged, but tower saw it leave that tuple since: a new
        // occurrence.
        (record(working_tuple, Some(permission_tuple())), true),
    ];
    for (i, (sc, expected)) in cases.into_iter().enumerate() {
        let mut app = app_with(sc);
        app.apply_poll(update(1, &[permission("aaaa0001", "a")]));
        assert_eq!(lit(&app, "a"), expected, "case {i}");
        assert_eq!(app.attention_count(), usize::from(expected), "case {i}");
    }
}

/// Presses Enter on the selected row and plays the runtime's refresh;
/// returns the attach effects.
fn enter_and_refresh(app: &mut App, rows: &[Value]) -> Vec<Effect> {
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.contains(&Effect::PollForAttach), "{effects:?}");
    app.attach_poll_started(1_000);
    let mut out = app.apply_poll(update(1_000, rows));
    out.extend(run_sidecar_check(app, Path::new(NO_STATE_FILE)));
    assert!(
        out.iter().any(|e| matches!(e, Effect::Attach(_))),
        "{out:?}"
    );
    out
}

#[test]
fn a_successful_attach_acknowledges() {
    let mut app = app();
    let rows = [permission("aaaa0001", "a"), working("bbbb0001", "b")];
    app.apply_poll(update(1, &rows));
    select(&mut app, "a");
    assert!(lit(&app, "a"));

    // A failed attach acknowledges nothing.
    enter_and_refresh(&mut app, &rows);
    let effects = app.attach_finished(AttachOutcome {
        name: "a".into(),
        account: "main".into(),
        result: Ok(Some(1)),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert!(lit(&app, "a"));

    enter_and_refresh(&mut app, &rows);
    let effects = app.attach_finished(AttachOutcome {
        name: "a".into(),
        account: "main".into(),
        result: Ok(Some(0)),
    });
    assert_eq!(effects, [Effect::SaveSidecar, Effect::PollNow]);
    assert!(!lit(&app, "a"));
    assert_eq!(app.attention_count(), 0);
    assert_eq!(
        app.sidecar.sessions[&sid("aaaa0001")].acknowledged,
        Some(permission_tuple())
    );
    // Back from a session that needs nothing: nothing to store.
    select(&mut app, "b");
    enter_and_refresh(&mut app, &rows);
    let effects = app.attach_finished(AttachOutcome {
        name: "b".into(),
        account: "main".into(),
        result: Ok(Some(0)),
    });
    assert_eq!(effects, [Effect::PollNow]);
    assert_eq!(app.sidecar.sessions[&sid("bbbb0001")].acknowledged, None);
}

#[test]
fn m_marks_seen_and_a_new_transition_highlights_again() {
    let mut app = app();
    app.apply_poll(update(1, &[permission("aaaa0001", "a")]));
    assert!(lit(&app, "a"));
    let effects = app.handle_key(ch('m'));
    assert_eq!(effects, [Effect::SaveSidecar]);
    assert!(!lit(&app, "a"));
    assert_eq!(
        app.sidecar.sessions[&sid("aaaa0001")].acknowledged,
        Some(permission_tuple())
    );
    // Again: nothing to do, and the status line says why.
    assert!(app.handle_key(ch('m')).is_empty());
    assert!(common::status_text(&app).contains("a is not highlighted"));

    // The same tuple on the next poll: still seen.
    app.apply_poll(update(2, &[permission("aaaa0001", "a")]));
    assert!(!lit(&app, "a"));

    // Answered, then a second permission prompt: highlighted again.
    app.apply_poll(update(3, &[working("aaaa0001", "a")]));
    assert!(!lit(&app, "a"));
    assert_eq!(app.sidecar.sessions[&sid("aaaa0001")].acknowledged, None);
    app.apply_poll(update(4, &[permission("aaaa0001", "a")]));
    assert!(lit(&app, "a"));

    // A different question while still in Needs me also counts as new.
    app.handle_key(ch('m'));
    assert!(!lit(&app, "a"));
    app.apply_poll(update(
        5,
        &[bg(
            "aaaa0001",
            "a",
            "blocked",
            Some("waiting"),
            Some("input needed"),
        )],
    ));
    assert!(lit(&app, "a"));
}

#[test]
fn interactive_rows_are_never_highlighted_or_counted() {
    let interactive = json!({
        "pid": 4_194_305,
        "sessionId": sid("eeee0001"),
        "name": "elsewhere",
        "cwd": "/home/user/workspace/demo-app",
        "kind": "interactive",
        "status": "waiting",
        "waitingFor": "permission prompt",
        "startedAt": 1_000,
    });
    let mut app = app();
    app.apply_poll(update(
        1,
        &[interactive.clone(), permission("aaaa0001", "a")],
    ));
    assert!(!lit(&app, "elsewhere"));
    assert!(lit(&app, "a"));
    assert_eq!(app.attention_count(), 1);
    assert_eq!(app.highlighted_in(Group::Interactive), 0);
    select(&mut app, "elsewhere");
    assert!(app.handle_key(ch('m')).is_empty());
    assert!(
        app.sidecar.sessions[&sid("eeee0001")]
            .acknowledged
            .is_none()
    );
}

#[test]
fn the_counter_counts_every_highlighted_row() {
    let mut app = app();
    app.apply_poll(update(
        1,
        &[
            permission("aaaa0001", "a"),
            bg("bbbb0001", "b", "blocked", Some("idle"), None),
            bg("cccc0001", "c", "failed", Some("idle"), None),
            bg("dddd0001", "d", "done", Some("idle"), None),
            working("ffff0001", "f"),
        ],
    ));
    assert_eq!(app.attention_count(), 3);
    assert_eq!(app.highlighted_in(Group::NeedsMe), 2);
    assert_eq!(app.highlighted_in(Group::Failed), 1);
    select(&mut app, "c");
    app.handle_key(ch('m'));
    assert_eq!(app.attention_count(), 2);
}

// ---- rendering --------------------------------------------------------

const NOW_MS: i64 = 1_800_000_000_000;

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

fn line_index(lines: &[String], needle: &str) -> u16 {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n"))) as u16
}

/// Column of the first cell holding `symbol` on row `y`.
fn column_of(buf: &Buffer, y: u16, symbol: &str) -> Option<u16> {
    (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == symbol)
}

/// An app with one highlighted row `lit` (changed 42 s before [`NOW_MS`])
/// and one quiet row `calm`, both of known ages.
fn rendered_app(color: bool) -> App {
    let mut app = app();
    app.color = color;
    app.apply_poll(update(
        1,
        &[permission("aaaa0001", "lit"), working("bbbb0001", "calm")],
    ));
    app.sidecar
        .sessions
        .get_mut(&sid("aaaa0001"))
        .unwrap()
        .last_change_ms = Some(NOW_MS - 42_000);
    app.sidecar
        .sessions
        .get_mut(&sid("bbbb0001"))
        .unwrap()
        .last_change_ms = Some(NOW_MS - 3 * 3_600_000);
    // Move the selection to the quiet row, so the marker row is plain.
    select(&mut app, "calm");
    app
}

#[test]
fn highlighted_rows_have_a_bold_marker_and_an_accent() {
    let mut app = rendered_app(true);
    let buf = render(&mut app, 120, 12);
    let text = lines(&buf);
    let y = line_index(&text, "lit");
    let row = &text[usize::from(y)];
    assert!(row.contains("● lit"), "{row}");
    let x = column_of(&buf, y, "●").expect("marker");
    assert_eq!(x, 2, "fixed leading column after the selection gutter");
    let marker = &buf[(x, y)];
    assert!(marker.modifier.contains(Modifier::BOLD));
    assert_eq!(marker.fg, Color::LightRed);
    // The name is bold too, without colour; the state column has the accent.
    let name_x = x + 2;
    assert!(buf[(name_x, y)].modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(name_x, y)].fg, Color::Reset);
    let detail_x = row.find("permission prompt").unwrap() as u16;
    assert_eq!(buf[(detail_x, y)].fg, Color::LightRed);

    // The quiet row: no marker, not bold.
    let y2 = line_index(&text, "calm");
    assert!(column_of(&buf, y2, "●").is_none());
    let calm_x = text[usize::from(y2)].find("calm").unwrap() as u16;
    assert!(!buf[(calm_x, y2)].modifier.contains(Modifier::BOLD));

    // Header and status bar.
    assert!(
        text.iter().any(|l| l.contains("Needs me (1)  ● 1")),
        "{text:#?}"
    );
    let status = &text[10];
    assert!(status.contains("● 1 needs you"), "{status}");
    let sx = column_of(&buf, 10, "●").unwrap();
    assert!(buf[(sx, 10)].modifier.contains(Modifier::BOLD));
}

#[test]
fn no_color_keeps_the_marker_and_bold_only() {
    let mut app = rendered_app(false);
    let buf = render(&mut app, 120, 12);
    let text = lines(&buf);
    let y = line_index(&text, "lit");
    let x = column_of(&buf, y, "●").expect("marker");
    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            assert_eq!(cell.fg, Color::Reset, "colour at {x},{y}");
            assert_eq!(cell.bg, Color::Reset, "colour at {x},{y}");
        }
    }
    let status = &text[10];
    assert!(status.contains("● 1 needs you"), "{status}");
    let sx = column_of(&buf, 10, "●").unwrap();
    assert!(buf[(sx, 10)].modifier.contains(Modifier::BOLD));
}

#[test]
fn the_age_column_shows_time_since_the_last_change() {
    let mut app = rendered_app(true);
    let text = lines(&render(&mut app, 120, 12));
    let lit_row = &text[usize::from(line_index(&text, "lit"))];
    assert!(lit_row.contains(" 42s  main"), "{lit_row}");
    let calm_row = &text[usize::from(line_index(&text, "calm"))];
    assert!(calm_row.contains("  3h  main"), "{calm_row}");

    for (ago, label) in [(0, "now"), (5 * 60_000, "5m"), (2 * 86_400_000, "2d")] {
        app.sidecar
            .sessions
            .get_mut(&sid("bbbb0001"))
            .unwrap()
            .last_change_ms = Some(NOW_MS - ago);
        let text = lines(&render(&mut app, 120, 12));
        let calm_row = &text[usize::from(line_index(&text, "calm"))];
        assert!(calm_row.contains(&format!(" {label}  main")), "{calm_row}");
    }
}

#[test]
fn help_lists_m_and_fits_24_rows() {
    let mut app = app();
    app.apply_poll(update(1, &[permission("aaaa0001", "a")]));
    app.handle_key(ch('?'));
    let text = lines(&render(&mut app, 80, 24));
    let all = text.join("\n");
    assert!(
        all.contains("mark the highlighted session as seen"),
        "{all}"
    );
    let top = line_index(&text, "Keys");
    let bottom = line_index(&text, "Esc or ? closes this help");
    assert!(top < bottom);
    assert!(!all.contains("Not available yet"), "{all}");
    assert!(all.contains("filter by name, project, account"), "{all}");
    assert!(all.contains("collapse / expand the group"), "{all}");
    // The whole box is on screen: every key line between the borders.
    assert!(all.contains("inside a session: return to tower"), "{all}");
    assert!(all.contains("Esc"));
}

#[test]
fn nothing_but_drawing_reaches_the_terminal() {
    // doc/0007: no BEL, no OSC, no title change. The renderer only fills
    // ratatui cells; none of them holds a control character.
    let mut app = rendered_app(true);
    let buf = render(&mut app, 120, 12);
    for cell in buf.content() {
        assert!(
            !cell.symbol().chars().any(|c| c.is_control()),
            "control character drawn: {:?}",
            cell.symbol()
        );
    }
}
