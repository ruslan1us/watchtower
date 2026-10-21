//! Rendering of the milestone 2 screens with ratatui's `TestBackend`
//! (doc/0015): the new-session dialog, the logs panel fed with the
//! `tests/fixtures/logs/*.ansi` streams, confirmations, the refusal window
//! and the footer.

mod common;

use std::time::Instant;

use common::{app_single, app_two, ch, ctrl, fixture_path, key, poll, project_list, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::actions::RemoveError;
use watchtower::app::{App, LogsTarget, SessionAction};
use watchtower::dispatch::DispatchError;
use watchtower::logview;

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    render_with_cursor(app, w, h).0
}

fn render_with_cursor(app: &mut App, w: u16, h: u16) -> (Vec<String>, (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer();
    let lines = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    (lines, (pos.x, pos.y))
}

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

fn open_dialog(app: &mut App) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let list = project_list(root.path(), &["api", "demo-app", "web"]);
    app.open_dialog(list);
    root
}

// ---- dialog ----------------------------------------------------------------

#[test]
fn dialog_shows_fields_defaults_and_keys() {
    let mut app = app_two();
    let _root = open_dialog(&mut app);
    let lines = render(&mut app, 120, 40);
    let all = text(&lines);
    assert!(all.contains("New session"), "{all}");
    for label in [
        "Project", "Task", "Name", "Account", "Model", "Effort", "Mode",
    ] {
        assert!(all.contains(label), "{label} missing\n{all}");
    }
    assert!(line_with(&lines, "▶ Project").contains("3 of 3"));
    assert!(
        line_with(&lines, "api").contains("▶"),
        "first project chosen"
    );
    assert!(all.contains("demo-app") && all.contains("web"));
    assert!(line_with(&lines, "Account").contains("‹ main ›"));
    assert!(line_with(&lines, "Account").contains("claude"));
    assert!(line_with(&lines, "Model").contains("default"));
    assert!(line_with(&lines, "Effort").contains("‹ default ›"));
    assert!(line_with(&lines, "  Mode ").contains("‹ auto ›"));
    assert!(line_with(&lines, "Name").contains("optional"));
    // Without keyboard enhancement the footer offers Alt+Enter.
    assert!(all.contains("Ctrl+S start"), "{all}");
    assert!(all.contains("Alt+Enter start & attach"), "{all}");
    assert!(!all.contains("Ctrl+Enter"), "{all}");
    assert!(all.contains("Ctrl+G editor"));

    app.keyboard_enhanced = true;
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("Ctrl+Enter (or Alt+Enter)"), "{all}");
}

#[test]
fn dialog_task_filter_cursor_and_messages() {
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    type_text(&mut app, "dem");
    let (lines, (cx, cy)) = render_with_cursor(&mut app, 120, 40);
    assert!(line_with(&lines, "▶ Project").contains("dem"));
    assert!(line_with(&lines, "▶ Project").contains("1 of 3"));
    assert!(!text(&lines).contains("~/") || !text(&lines).contains(" web"));
    // The cursor sits after the filter text.
    assert_eq!(&lines[usize::from(cy)][..], line_with(&lines, "▶ Project"));
    let row: String = lines[usize::from(cy)]
        .chars()
        .take(usize::from(cx))
        .collect();
    assert!(row.ends_with("dem"), "{row:?}");

    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "first line");
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "second");
    let (lines, (cx, cy)) = render_with_cursor(&mut app, 120, 40);
    assert!(line_with(&lines, "┊ first line").contains("first line"));
    let row = &lines[usize::from(cy)];
    assert!(row.contains("┊ second"), "{row}");
    let before: String = row.chars().take(usize::from(cx)).collect();
    assert!(before.ends_with("second"), "{before:?}");

    // Validation error and the Haiku warning are shown inline.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    while app.dialog().unwrap().model.text() != "haiku" {
        app.handle_key(key(KeyCode::Down));
    }
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("auto mode is unavailable on Haiku"), "{all}");

    // Esc with text asks first.
    app.handle_key(key(KeyCode::Esc));
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("Discard this task?"), "{all}");
}

#[test]
fn dialog_starting_untrusted_and_errors() {
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "do the thing");
    let effects = app.handle_key(ctrl('s'));
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("starting…"), "{all}");
    assert!(all.contains("do the thing"), "contents kept");

    let [watchtower::app::Effect::Dispatch(job)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    let dir = job.request.project.clone();
    app.dispatch_finished(
        job.clone(),
        Err(DispatchError::Untrusted {
            dir,
            message: "Workspace not trusted. Run `claude` in /home/user/workspace/api once and \
                      accept the trust prompt, then retry."
                .into(),
        }),
    );
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("Workspace not trusted"), "{all}");
    assert!(all.contains("Ctrl+T: run `claude` in"), "{all}");

    app.dispatch_finished(
        job.clone(),
        Err(DispatchError::Failed {
            code: Some(1),
            stderr: "error: boom".into(),
        }),
    );
    let all = text(&render(&mut app, 120, 40));
    assert!(
        all.contains("claude --bg failed (exit 1): error: boom"),
        "{all}"
    );
    assert!(!all.contains("Ctrl+T: run"), "{all}");
}

#[test]
fn dialog_without_projects_explains_the_config() {
    let mut app = app_single();
    app.open_dialog(watchtower::projects::ProjectList {
        projects: vec![],
        warnings: vec!["project root /home/user/workspace: No such file or directory".into()],
    });
    let all = text(&render(&mut app, 120, 40));
    assert!(all.contains("no projects found"), "{all}");
    assert!(all.contains("project_roots"), "{all}");
    assert!(all.contains("No such file or directory"), "{all}");
}

#[test]
fn dialog_without_projects_names_the_config_file() {
    use watchtower::config::ConfigSource;
    let file = std::path::PathBuf::from("/home/user/.config/watchtower/config.toml");
    let mut app = app_single();
    app.set_config_source(ConfigSource {
        path: file.clone(),
        found: true,
    });
    app.open_dialog(watchtower::projects::ProjectList::default());
    let all = text(&render(&mut app, 120, 40));
    // The note wraps inside the dialog rather than being cut off.
    assert!(
        all.contains("no projects found — set project_roots or extra_projects in"),
        "{all}"
    );
    assert!(
        all.contains("~/.config/watchtower/config.toml (loaded)"),
        "{all}"
    );
    // Found: no warning in the status line.
    assert!(!all.contains("no config at"), "{all}");

    let mut app = app_single();
    app.set_config_source(ConfigSource {
        path: file,
        found: false,
    });
    assert!(
        common::status_text(&app).contains(
            "no config at ~/.config/watchtower/config.toml; using defaults — see \
             config.example.toml"
        ),
        "{}",
        common::status_text(&app)
    );
    app.open_dialog(watchtower::projects::ProjectList::default());
    let all = text(&render(&mut app, 80, 40));
    // The note wraps at spaces; search it across rows.
    let flat = all
        .lines()
        .map(|l| l.trim_matches(|c: char| " │".contains(c)))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(flat.contains("no projects found — no config at"), "{all}");
    assert!(flat.contains("(using defaults); create it with"), "{all}");
    assert!(flat.contains("(see config.example.toml)"), "{all}");
}

/// The text of the row holding `needle`, without the borders and spaces of
/// the dialog and the screen behind it.
fn inner_text(lines: &[String], needle: &str) -> String {
    line_with(lines, needle)
        .trim_matches(|c: char| " │╭╮╰╯├┤─".contains(c))
        .to_owned()
}

#[test]
fn dialog_keys_drop_whole_hints_on_one_row() {
    // doc/0008: Ctrl+S and Ctrl+P stay longest, then the attach key, Tab,
    // Ctrl+G and Enter; the row never wraps.
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    let wide = "Tab/Shift+Tab field · Ctrl+S start · Alt+Enter start & attach · Ctrl+G editor · \
                Ctrl+P template";
    for (w, h, want) in [
        (
            80,
            24,
            "Ctrl+S start · Alt+Enter start & attach · Ctrl+P template",
        ),
        // One column of padding on each side: at 100 columns `Ctrl+G` goes.
        (
            100,
            30,
            "Tab/Shift+Tab field · Ctrl+S start · Alt+Enter start & attach · Ctrl+P template",
        ),
        (104, 32, wide),
    ] {
        let lines = render(&mut app, w, h);
        assert_eq!(inner_text(&lines, "Ctrl+S start"), want, "{w}x{h}");
        let all = text(&lines);
        assert!(!all.contains("Enter next field"), "{w}x{h}\n{all}");
        assert_eq!(all.matches("Ctrl+S start").count(), 1, "{w}x{h}\n{all}");
    }
    // The attach key that works here, in its longer form.
    app.keyboard_enhanced = true;
    for (w, h, want) in [
        (
            80,
            24,
            "Ctrl+S start · Ctrl+Enter (or Alt+Enter) start & attach · Ctrl+P template",
        ),
        (
            104,
            32,
            "Tab/Shift+Tab field · Ctrl+S start · Ctrl+Enter (or Alt+Enter) start & attach · \
             Ctrl+P template",
        ),
    ] {
        let lines = render(&mut app, w, h);
        assert_eq!(inner_text(&lines, "Ctrl+S start"), want, "{w}x{h}");
    }
}

#[test]
fn dialog_task_placeholder_takes_the_longest_variant_that_fits() {
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    app.handle_key(key(KeyCode::Tab));
    for (w, h, want) in [
        (
            80,
            24,
            "describe the task · Ctrl+G editor · Ctrl+P template",
        ),
        (
            100,
            30,
            "describe the task · Enter new line · Ctrl+G editor · Ctrl+P template",
        ),
        (
            104,
            32,
            "describe the task · Enter new line · Ctrl+G editor · Ctrl+P template",
        ),
    ] {
        let lines = render(&mut app, w, h);
        assert_eq!(
            inner_text(&lines, "▶ Task"),
            format!("▶ Task    {want}"),
            "{w}x{h}"
        );
    }
}

#[test]
fn dialog_model_hint_shows_whole_or_not_at_all() {
    let full = "↑↓ default · fable · opus · sonnet · haiku · or type a model name";
    let short = "↑↓ presets · or type a model name";
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    for (w, h, want) in [(80, 24, short), (100, 30, full), (104, 32, full)] {
        let lines = render(&mut app, w, h);
        let model = inner_text(&lines, "  Model ");
        assert!(model.starts_with("Model   default "), "{w}x{h}: {model}");
        assert!(model.ends_with(want), "{w}x{h}: {model}");
    }
    // A long model name keeps its room; the hint goes first.
    let long = "claude-opus-4-1-20250805-experimental-long-context";
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Tab));
    }
    while !app.dialog().unwrap().model.text().is_empty() {
        app.handle_key(key(KeyCode::Backspace));
    }
    type_text(&mut app, long);
    let lines = render(&mut app, 80, 24);
    assert_eq!(inner_text(&lines, "▶ Model"), format!("▶ Model   {long}"));
    for (w, h) in [(100, 30), (104, 32)] {
        let lines = render(&mut app, w, h);
        let model = inner_text(&lines, "▶ Model");
        assert!(model.ends_with(short), "{w}x{h}: {model}");
        assert!(
            model.contains("experimental-long-context"),
            "{w}x{h}: {model}"
        );
    }
}

#[test]
fn dialog_starting_message_wraps() {
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "do the thing");
    app.handle_key(ctrl('s'));
    let lines = render(&mut app, 80, 24);
    assert_eq!(
        inner_text(&lines, "starting…"),
        "starting… (a supervisor may have to start first; this can take a few"
    );
    assert_eq!(inner_text(&lines, "seconds)"), "seconds)");
}

#[test]
fn dialog_survives_small_terminals() {
    let mut app = app_single();
    let _root = open_dialog(&mut app);
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, &"long task text ".repeat(40));
    for (w, h) in [(1, 1), (10, 4), (30, 8), (50, 15), (80, 24)] {
        render(&mut app, w, h);
    }
    let lines = render(&mut app, 80, 24);
    assert!(text(&lines).contains("New session"));
}

// ---- logs panel ------------------------------------------------------------

fn fetched(app: &mut App, name: &str) {
    let target: LogsTarget = app.logs().unwrap().in_flight().cloned().unwrap();
    let bytes = std::fs::read(fixture_path(&format!("logs/{name}.ansi"))).unwrap();
    app.logs_fetched(target, Ok(logview::render(&bytes)));
}

fn snapshot(name: &str) -> Vec<String> {
    std::fs::read_to_string(fixture_path(&format!("logs/{name}.screen.txt")))
        .unwrap()
        .lines()
        .map(|l| l.trim_end().to_owned())
        .collect()
}

#[test]
fn bottom_panel_shows_the_tail_of_the_screen() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    // Selected: pick-colour (input needed).
    app.handle_key(ch(' '));
    let lines = render(&mut app, 120, 40);
    assert!(text(&lines).contains("loading…"));
    fetched(&mut app, "permission-prompt");
    let lines = render(&mut app, 120, 40);
    let top = lines
        .iter()
        .position(|l| l.contains("logs · pick-colour (main)"))
        .unwrap();
    assert!(top > 5, "below the list: {top}");
    // The last non-blank rows of the snapshot are the panel's last rows.
    let snap: Vec<String> = snapshot("permission-prompt")
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let all = text(&lines);
    for want in snap.iter().rev().take(3) {
        let want: String = want.chars().take(100).collect();
        assert!(all.contains(want.trim()), "{want:?} missing in\n{all}");
    }
    assert!(line_with(&lines, "Space/Esc close").contains('└'));
}

#[test]
fn side_panel_on_wide_terminals_clips_without_wrapping() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    common::select_name(&mut app, "say-ok");
    app.handle_key(ch(' '));
    fetched(&mut app, "done");
    let lines = render(&mut app, 160, 30);
    // Side by side: the list row and the panel share screen lines.
    let header = line_with(&lines, "logs · say-ok (main)");
    assert!(header.find("logs ·").unwrap() >= 75, "{header}");
    assert!(
        line_with(&lines, "reply with the single word OK").contains("say-ok") || {
            let l = line_with(&lines, "reply with the single word OK");
            l.find("reply").unwrap() > 75
        }
    );
    // Never wrapped: no line is wider than the terminal and the banner row
    // keeps its text on one line.
    assert!(
        lines
            .iter()
            .all(|l| unicode_width::UnicodeWidthStr::width(l.as_str()) <= 160)
    );
    let all = text(&lines);
    assert!(all.contains("Claude Code v2.1.302"), "{all}");
}

#[test]
fn panel_notes_render() {
    let mut app = app_single();
    app.apply_poll(poll("main", "after-stop-all.json"));
    common::select_name(&mut app, "say-ok");
    app.handle_key(ch(' '));
    let all = text(&render(&mut app, 120, 30));
    assert!(
        all.contains("no live output — session stopped; attach to view it"),
        "{all}"
    );
}

// ---- confirmations, refusal window, footer ----------------------------------

#[test]
fn confirmations_render() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    // pick-colour is blocked: stop asks.
    app.handle_key(ch('s'));
    let all = text(&render(&mut app, 120, 30));
    assert!(all.contains("Stop session?"), "{all}");
    assert!(
        all.contains("Stop pick-colour (9c6e1a35, account main)?"),
        "{all}"
    );
    assert!(all.contains("no (default)"), "{all}");
    app.handle_key(ch('n'));
    app.handle_key(ch('d'));
    let all = text(&render(&mut app, 120, 30));
    assert!(all.contains("Remove session?"), "{all}");
    assert!(all.contains("transcript stays on disk"), "{all}");
    assert!(all.contains("worktree"), "{all}");
}

#[test]
fn refusal_window_scrolls() {
    let mut app = app_single();
    let action = SessionAction {
        account: app.config().accounts[0].clone(),
        id: "b2f8d064".into(),
        name: "say-ok".into(),
    };
    let message = (1..=30)
        .map(|i| format!("refusal line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.remove_finished(
        action.into(),
        Err(RemoveError::Refused {
            code: Some(1),
            message,
        })
        .into(),
    );
    let lines = render(&mut app, 100, 20);
    let all = text(&lines);
    assert!(all.contains("say-ok was not removed"), "{all}");
    assert!(all.contains("refusal line 1"), "{all}");
    assert!(!all.contains("refusal line 30"), "{all}");
    assert!(all.contains("scroll"), "{all}");
    app.handle_key(key(KeyCode::End));
    let all = text(&render(&mut app, 100, 20));
    assert!(all.contains("refusal line 30"), "{all}");
    assert!(all.contains("never passes"), "{all}");
}

#[test]
fn footer_keeps_the_detach_hint_on_narrow_terminals() {
    let mut app = app_single();
    app.apply_poll(poll("main", "mixed-states.json"));
    let lines = render(&mut app, 120, 10);
    let footer = &lines[9];
    assert!(footer.contains("n new"), "{footer}");
    assert!(footer.contains("Space logs"), "{footer}");
    assert!(
        footer.ends_with("in a session: Ctrl+Z returns to tower"),
        "{footer}"
    );
    let lines = render(&mut app, 60, 10);
    assert!(
        lines[9].ends_with("Ctrl+Z returns to tower"),
        "{}",
        lines[9]
    );
}

#[test]
fn help_lists_milestone_two_keys() {
    let mut app = app_single();
    app.handle_key(ch('?'));
    let all = text(&render(&mut app, 100, 30));
    for needle in [
        "new session",
        "stop (confirm when working or blocked); remove",
        "logs panel",
        "New-session dialog",
        "Ctrl+S",
        "Ctrl+G",
        "Ctrl+T",
        "filter by name, project, account",
        "group by state / project",
        "attach to the orchestrator",
    ] {
        assert!(all.contains(needle), "{needle} missing\n{all}");
    }
}

#[test]
fn wrap_breaks_at_spaces_and_splits_long_words() {
    use watchtower::ui::wrap;
    assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    assert_eq!(wrap("a\nb c", 10), ["a", "b c"]);
    assert_eq!(wrap("abcdefghij xy", 4), ["abcd", "efgh", "ij", "xy"]);
    assert_eq!(wrap("", 5), [""]);
    assert!(
        wrap("日本語の文", 4)
            .iter()
            .all(|r| unicode_width::UnicodeWidthStr::width(r.as_str()) <= 4)
    );
}
