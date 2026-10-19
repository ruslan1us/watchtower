//! The Settings window's state machine (doc/0024): opening and closing,
//! moving, every kind of edit, saving with the window kept open, the
//! questions (unsaved changes, the editor, a conflict, the cross-group
//! hand-off), the live preview of the style and theme, the list editor,
//! and keys typed under a Cyrillic layout (doc/0011).

mod common;
mod settings_common;

use std::fs;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use settings_common::*;
use watchtower::app::{App, Effect};
use watchtower::config::{UiStyle, UiTheme};
use watchtower::config_edit::{self, Section, SettingKey, SettingValue};
use watchtower::settings::{FolderInfo, Mode, NoticeKind, Row, SettingsWindow};

fn win(app: &App) -> &SettingsWindow {
    app.settings().expect("the Settings window is open")
}

fn value(app: &App, key: SettingKey) -> SettingValue {
    win(app).session.value(key)
}

fn text(s: &str) -> SettingValue {
    SettingValue::Text(s.to_owned())
}

fn notice(app: &App) -> String {
    win(app)
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

fn opened() -> (Env, App) {
    let env = env(Some(CONFIG));
    let mut app = app_for(&env);
    open(&mut app);
    (env, app)
}

/// Saves with `Ctrl+S` and runs the save on this thread.
fn save(app: &mut App) -> Vec<Effect> {
    press(app, ctrl('s'))
}

// ---- opening and closing --------------------------------------------------

#[test]
fn c_opens_the_window_and_esc_closes_it() {
    let env = env(Some(CONFIG));
    let mut app = app_for(&env);
    let effects = app.handle_key(ch('c'));
    assert_eq!(effects, [Effect::OpenSettings(env.path.clone())]);
    run(&mut app, effects);
    let w = win(&app);
    assert!(w.session.exists());
    assert_eq!(w.shown_path, "~/.config/watchtower/config.toml");
    assert_eq!(w.selected_row(), Row::Setting(SettingKey::UiStyle));
    assert_eq!(w.mode, Mode::Browse);
    assert!(press(&mut app, key(KeyCode::Esc)).is_empty());
    assert!(app.settings().is_none());
    assert_eq!(file(&env), CONFIG, "nothing was written");
}

#[test]
fn cyrillic_c_opens_the_window() {
    let env = env(Some(CONFIG));
    let mut app = app_for(&env);
    run_key_opens(&mut app, 'с');
    assert!(app.settings().is_some());
}

fn run_key_opens(app: &mut App, c: char) {
    let effects = app.handle_key(ch(c));
    assert!(
        matches!(effects.as_slice(), [Effect::OpenSettings(_)]),
        "{effects:?}"
    );
    run(app, effects);
}

#[test]
fn an_invalid_file_is_not_opened() {
    let env = env(Some("poll_interval_secs = 0\n"));
    // The app runs with the defaults; the file is broken meanwhile.
    let mut app = {
        let ok = settings_common::env(None);
        let mut app = app_for(&ok);
        app.set_config_source(watchtower::config::ConfigSource::probe(&env.path));
        app
    };
    let effects = press(&mut app, ch('c'));
    assert!(effects.is_empty());
    assert!(app.settings().is_none());
    let status = common::status_text(&app);
    assert!(status.contains("settings not opened"), "{status}");
}

#[test]
fn a_missing_file_opens_with_the_defaults() {
    let env = env(None);
    let mut app = app_for(&env);
    open(&mut app);
    let w = win(&app);
    assert!(!w.session.exists());
    assert_eq!(w.session.accounts().len(), 1);
    assert_eq!(value(&app, SettingKey::UiTheme), text("neutral"));
}

// ---- moving ----------------------------------------------------------------

#[test]
fn arrows_move_and_tab_jumps_between_sections() {
    let (_env, mut app) = opened();
    press(&mut app, key(KeyCode::Down));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiTheme));
    press(&mut app, ch('k'));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiStyle));
    press(&mut app, ch('j'));
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::DefaultAccount));
    for (want, section) in [
        (SettingKey::ProjectRoots, Section::Projects),
        (SettingKey::HandoffPrompt, Section::Handoff),
        (SettingKey::PollIntervalSecs, Section::General),
    ] {
        press(&mut app, key(KeyCode::Tab));
        assert_eq!(win(&app).selected_key(), Some(want));
        assert_eq!(win(&app).selected_row().section(), section);
    }
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(win(&app).selected_row(), Row::Account(0));
    // Wraps around, and Shift+Tab goes back.
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiStyle));
    press(&mut app, key(KeyCode::BackTab));
    assert_eq!(win(&app).selected_row(), Row::Account(0));
    press(&mut app, key(KeyCode::BackTab));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::PollIntervalSecs));
    press(&mut app, key(KeyCode::End));
    assert_eq!(win(&app).selected_row(), Row::Account(1));
    press(&mut app, ch('g'));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiStyle));
}

#[test]
fn accounts_are_read_only() {
    let (_env, mut app) = opened();
    press(&mut app, key(KeyCode::End));
    press(&mut app, key(KeyCode::Enter));
    assert!(notice(&app).contains("read-only"), "{}", notice(&app));
    press(&mut app, key(KeyCode::Delete));
    assert!(!win(&app).session.has_changes());
}

// ---- edits ------------------------------------------------------------------

#[test]
fn arrows_cycle_a_choice() {
    let (_env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    assert_eq!(value(&app, SettingKey::DefaultEffort), text("low"));
    press(&mut app, key(KeyCode::Left));
    press(&mut app, key(KeyCode::Left));
    assert_eq!(value(&app, SettingKey::DefaultEffort), text("max"));
    assert!(win(&app).session.is_modified(SettingKey::DefaultEffort));
    assert_eq!(win(&app).change_count(), 1);
}

#[test]
fn bypass_permissions_is_never_offered() {
    let (_env, mut app) = opened();
    select(&mut app, "default permission mode");
    for _ in 0..10 {
        press(&mut app, key(KeyCode::Right));
        assert_ne!(
            value(&app, SettingKey::DefaultPermissionMode),
            text("bypassPermissions")
        );
    }
    press(&mut app, key(KeyCode::Enter));
    let Mode::Pick(pick) = &win(&app).mode else {
        panic!("no pick list");
    };
    assert!(!pick.items.iter().any(|i| i == "bypassPermissions"));
}

#[test]
fn the_pick_list_keeps_with_enter_and_cancels_with_esc() {
    let (_env, mut app) = opened();
    select(&mut app, "default account");
    press(&mut app, key(KeyCode::Enter));
    assert!(matches!(win(&app).mode, Mode::Pick(_)));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(value(&app, SettingKey::DefaultAccount), text("main"));
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(value(&app, SettingKey::DefaultAccount), text("alt"));
    assert_eq!(win(&app).mode, Mode::Browse);
}

#[test]
fn a_full_model_name_is_typed_after_other() {
    let (_env, mut app) = opened();
    select(&mut app, "default model");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, key(KeyCode::End));
    press(&mut app, key(KeyCode::Enter));
    assert!(matches!(win(&app).mode, Mode::Input(_)));
    type_in(&mut app, "two words");
    let Mode::Input(input) = &win(&app).mode else {
        panic!("no input");
    };
    assert_eq!(
        input.error.as_deref(),
        Some("default model must be one word")
    );
    press(&mut app, key(KeyCode::Enter));
    assert!(
        matches!(win(&app).mode, Mode::Input(_)),
        "an invalid value stays"
    );
    press(&mut app, ctrl('u'));
    type_in(&mut app, "claude-test-model");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        value(&app, SettingKey::DefaultModel),
        text("claude-test-model")
    );
}

#[test]
fn a_number_is_checked_as_typed() {
    let (_env, mut app) = opened();
    select(&mut app, "poll interval");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "0");
    let Mode::Input(input) = &win(&app).mode else {
        panic!("no input");
    };
    assert_eq!(input.error.as_deref(), Some("must be 1 to 60 seconds"));
    // Enter refuses an invalid value; so does Ctrl+S.
    press(&mut app, key(KeyCode::Enter));
    assert!(matches!(win(&app).mode, Mode::Input(_)));
    assert!(press(&mut app, ctrl('s')).is_empty());
    assert!(notice(&app).contains("must be 1 to 60 seconds"));
    // Esc undoes the edit.
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(win(&app).mode, Mode::Browse);
    assert_eq!(
        value(&app, SettingKey::PollIntervalSecs),
        SettingValue::Int(2)
    );
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "abc");
    press(&mut app, key(KeyCode::Enter));
    assert!(
        matches!(&win(&app).mode, Mode::Input(i) if i.error.as_deref() == Some("must be 1 to 60 seconds"))
    );
    press(&mut app, ctrl('u'));
    type_in(&mut app, "15");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        value(&app, SettingKey::PollIntervalSecs),
        SettingValue::Int(15)
    );
}

#[test]
fn text_fields_keep_cyrillic_as_typed() {
    let (_env, mut app) = opened();
    select(&mut app, "hand-off prompt");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    let Mode::Input(input) = &win(&app).mode else {
        panic!("no input");
    };
    assert_eq!(
        input.error.as_deref(),
        Some("hand-off prompt must not be empty")
    );
    // `с`, `ы`, `н` and `е` are command keys elsewhere; here they are text.
    type_in(&mut app, "продолжай сейчас");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        value(&app, SettingKey::HandoffPrompt),
        text("продолжай сейчас")
    );
}

#[test]
fn the_cross_group_hand_off_asks_before_turning_on() {
    let (_env, mut app) = opened();
    select(&mut app, "allow cross-group hand-off");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(win(&app).mode, Mode::CrossGroup);
    press(&mut app, ch('n'));
    assert_eq!(win(&app).mode, Mode::Browse);
    assert_eq!(
        value(&app, SettingKey::AllowCrossGroupHandoff),
        SettingValue::Bool(false)
    );
    press(&mut app, key(KeyCode::Right));
    assert_eq!(win(&app).mode, Mode::CrossGroup);
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(
        value(&app, SettingKey::AllowCrossGroupHandoff),
        SettingValue::Bool(false)
    );
    // `н` is `y` on the RU layout.
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ch('н'));
    assert_eq!(
        value(&app, SettingKey::AllowCrossGroupHandoff),
        SettingValue::Bool(true)
    );
    // Turning it off does not ask.
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(win(&app).mode, Mode::Browse);
    assert_eq!(
        value(&app, SettingKey::AllowCrossGroupHandoff),
        SettingValue::Bool(false)
    );
}

#[test]
fn del_goes_back_to_the_default() {
    let (_env, mut app) = opened();
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    assert_eq!(value(&app, SettingKey::UiTheme), text("warm"));
    press(&mut app, key(KeyCode::Delete));
    // The file has the theme: the reset removes it on save.
    assert!(win(&app).session.is_modified(SettingKey::UiTheme));
    assert_eq!(value(&app, SettingKey::UiTheme), text("neutral"));
    assert!(win(&app).session.changes()[0].reset);
    // A setting not in the file is already the default.
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Delete));
    assert!(notice(&app).contains("already the default"));
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Delete));
    assert!(!win(&app).session.is_modified(SettingKey::DefaultEffort));
}

// ---- saving ----------------------------------------------------------------

#[test]
fn ctrl_s_saves_off_the_ui_thread_and_keeps_the_window_open() {
    let (env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    let effects = app.handle_key(ctrl('s'));
    let [Effect::SaveConfig(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(win(&app).saving);
    assert_eq!(notice(&app), "saving…");
    // A second Ctrl+S while it runs starts nothing.
    assert!(app.handle_key(ctrl('s')).is_empty());
    let result = job.run();
    app.config_saved(result);
    let w = win(&app);
    assert!(!w.saving);
    assert!(!w.session.has_changes());
    assert!(w.just_saved);
    let n = w.notice.as_ref().unwrap();
    assert_eq!(n.kind, NoticeKind::Success);
    assert_eq!(
        n.text,
        "✓ Saved 1 change to ~/.config/watchtower/config.toml · comments and formatting kept · \
         backup config.toml.bak · applied now"
    );
    assert!(file(&env).contains("default_effort = \"low\""));
    assert_eq!(app.config().default_effort, "low");
    // Nothing more to save.
    assert!(save(&mut app).is_empty());
    assert_eq!(notice(&app), "nothing to save");
}

#[test]
fn a_missing_file_is_created_by_ctrl_s() {
    let env = env(None);
    let mut app = app_for(&env);
    open(&mut app);
    select(&mut app, "poll interval");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "5");
    press(&mut app, key(KeyCode::Enter));
    save(&mut app);
    assert!(notice(&app).starts_with("✓ Created ~/.config/watchtower/config.toml with 1 change"));
    let text = file(&env);
    assert!(text.starts_with(config_edit::HEADER), "{text}");
    assert!(text.contains("poll_interval_secs = 5"));
    assert!(win(&app).session.exists());
}

#[test]
fn esc_with_changes_asks_y_saves_and_closes() {
    let (env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(win(&app).mode, Mode::Unsaved);
    press(&mut app, ch('y'));
    assert!(app.settings().is_none(), "closed after the save");
    assert!(file(&env).contains("default_effort = \"low\""));
}

#[test]
fn esc_with_changes_asks_n_discards() {
    let (env, mut app) = opened();
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.theme, UiTheme::Warm, "previewed");
    press(&mut app, key(KeyCode::Esc));
    // `т` is `n` on the RU layout.
    press(&mut app, ch('т'));
    assert!(app.settings().is_none());
    assert_eq!(app.theme, UiTheme::Neutral, "the preview reverted");
    assert_eq!(file(&env), CONFIG);
}

#[test]
fn esc_or_enter_in_the_question_keeps_editing() {
    let (_env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    for k in [KeyCode::Esc, KeyCode::Enter] {
        press(&mut app, key(KeyCode::Esc));
        assert_eq!(win(&app).mode, Mode::Unsaved);
        press(&mut app, key(k));
        assert_eq!(win(&app).mode, Mode::Browse);
        assert!(win(&app).session.has_changes());
    }
    // Other keys are ignored while it asks.
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ch('x'));
    assert_eq!(win(&app).mode, Mode::Unsaved);
}

#[test]
fn ctrl_c_with_changes_asks_before_quitting() {
    let (_env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    assert!(press(&mut app, ctrl('c')).is_empty());
    assert_eq!(win(&app).mode, Mode::QuitUnsaved);
    press(&mut app, ch('n'));
    assert_eq!(win(&app).mode, Mode::Browse);
    press(&mut app, ctrl('c'));
    assert_eq!(press(&mut app, ch('y')), [Effect::Quit]);
}

#[test]
fn a_conflict_asks_y_overwrites_n_cancels() {
    let (env, mut app) = opened();
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    // Another tower sets the theme meanwhile.
    fs::write(&env.path, CONFIG.replace("\"neutral\"", "\"terminal\"")).unwrap();
    save(&mut app);
    let Mode::Conflict(conflicts) = &win(&app).mode else {
        panic!("no conflict question: {:?}", win(&app).mode);
    };
    assert_eq!(conflicts[0].key, SettingKey::UiTheme);
    assert_eq!(conflicts[0].on_disk, Some(text("terminal")));
    assert_eq!(conflicts[0].mine, Some(text("warm")));
    press(&mut app, ch('n'));
    assert_eq!(win(&app).mode, Mode::Browse);
    assert!(notice(&app).contains("not saved"));
    assert!(file(&env).contains("theme = \"terminal\""));
    assert!(win(&app).session.has_changes());
    save(&mut app);
    assert!(matches!(win(&app).mode, Mode::Conflict(_)));
    press(&mut app, ch('y'));
    assert!(file(&env).contains("theme = \"warm\""));
    assert!(!win(&app).session.has_changes());
    assert!(app.settings().is_some(), "the window stays open");
}

// ---- the editor ------------------------------------------------------------

#[test]
fn e_without_changes_opens_the_editor_and_comes_back() {
    let (env, mut app) = opened();
    let effects = press(&mut app, ch('e'));
    assert_eq!(effects, [Effect::EditConfig(env.path.clone())]);
    assert!(app.settings().is_none(), "the editor takes the terminal");
    // The editor saved a valid file: the window opens again.
    fs::write(&env.path, CONFIG.replace("\"neutral\"", "\"warm\"")).unwrap();
    let snapshot = config_edit::read_config(&env.path, &env.home);
    let effects = app.config_editor_finished(Ok(snapshot));
    assert!(effects.contains(&Effect::OpenSettings(env.path.clone())));
    run(&mut app, effects);
    assert_eq!(value(&app, SettingKey::UiTheme), text("warm"));
}

#[test]
fn e_after_a_failed_editor_stays_in_the_list() {
    let (_env, mut app) = opened();
    press(&mut app, ch('e'));
    let effects = app.config_editor_finished(Err("set $VISUAL or $EDITOR".into()));
    assert!(effects.is_empty());
    assert!(app.settings().is_none());
}

#[test]
fn e_with_changes_asks_save_discard_or_cancel() {
    // Esc cancels.
    let (env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    assert!(press(&mut app, ch('e')).is_empty());
    assert_eq!(win(&app).mode, Mode::EditPrompt);
    assert!(press(&mut app, key(KeyCode::Esc)).is_empty());
    assert_eq!(win(&app).mode, Mode::Browse);
    // `у` is `e` on the RU layout; `n` discards and opens the editor.
    press(&mut app, ch('у'));
    assert_eq!(win(&app).mode, Mode::EditPrompt);
    let effects = press(&mut app, ch('n'));
    assert_eq!(effects, [Effect::EditConfig(env.path.clone())]);
    assert_eq!(file(&env), CONFIG);
    // `y` saves first.
    let (env, mut app) = opened();
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    press(&mut app, ch('e'));
    let effects = press(&mut app, ch('y'));
    assert_eq!(effects, [Effect::EditConfig(env.path.clone())]);
    assert!(file(&env).contains("default_effort = \"low\""));
}

// ---- live preview ------------------------------------------------------------

#[test]
fn the_theme_and_style_preview_live_and_revert() {
    let (_env, mut app) = opened();
    assert_eq!(app.theme, UiTheme::Neutral);
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.theme, UiTheme::Warm, "the cursor previews");
    assert_eq!(win(&app).previewing().as_deref(), Some("theme warm"));
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.theme, UiTheme::Terminal);
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(app.theme, UiTheme::Neutral, "Esc reverts");
    assert!(!win(&app).session.has_changes());
    select(&mut app, "style");
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.style, UiStyle::Minimal);
    // Closing without saving reverts.
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ch('n'));
    assert_eq!(app.style, UiStyle::Dashboard);
}

#[test]
fn a_reload_while_open_keeps_the_preview() {
    let (env, mut app) = opened();
    // The reload check knows the file as it is.
    let job = app.start_config_check().unwrap();
    app.config_checked(job.epoch, job.check.run());
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.theme, UiTheme::Warm);
    // A hand edit changes the theme and the style meanwhile.
    let edited = CONFIG
        .replace("\"neutral\"", "\"tokyo-night\"")
        .replace("\"dashboard\"", "\"minimal\"");
    let tmp = env.path.with_extension("tmp");
    fs::write(&tmp, &edited).unwrap();
    fs::rename(&tmp, &env.path).unwrap();
    let job = app.start_config_check().unwrap();
    app.config_checked(job.epoch, job.check.run());
    assert_eq!(app.config().ui_theme, UiTheme::TokyoNight, "reloaded");
    assert_eq!(app.theme, UiTheme::Warm, "the preview survives the reload");
    assert_eq!(
        app.style,
        UiStyle::Minimal,
        "an unchanged setting follows the file"
    );
    // Discarding shows the reloaded look.
    press(&mut app, key(KeyCode::Esc));
    press(&mut app, ch('n'));
    assert_eq!(app.theme, UiTheme::TokyoNight);
    assert_eq!(app.style, UiStyle::Minimal);
}

// ---- the list editor -----------------------------------------------------------

fn list(app: &App) -> &watchtower::settings::ListEditor {
    match &win(app).mode {
        Mode::List(list) => list,
        other => panic!("no list editor: {other:?}"),
    }
}

#[test]
fn the_list_editor_adds_edits_and_removes() {
    let (_env, mut app) = opened();
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(list(&app).items, ["~/workspace", "~/old-projects"]);
    // The folders were looked at on a worker (here, this thread).
    let w = win(&app);
    assert_eq!(
        w.folders.get("~/workspace"),
        Some(&FolderInfo::Folder {
            git: false,
            clones: 3
        })
    );
    assert_eq!(
        w.folder_note(SettingKey::ProjectRoots, "~/workspace").0,
        "✓ 3 git clones"
    );
    assert_eq!(
        w.folder_note(SettingKey::ProjectRoots, "~/old-projects").0,
        "not found; skipped when scanning"
    );
    // Add.
    press(&mut app, ch('a'));
    type_in(&mut app, "~/notes");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(
        list(&app).items,
        ["~/workspace", "~/old-projects", "~/notes"]
    );
    assert!(list(&app).input.is_none());
    // A duplicate is refused.
    press(&mut app, ch('a'));
    type_in(&mut app, "~/workspace/");
    press(&mut app, key(KeyCode::Enter));
    let input = list(&app).input.as_ref().expect("the input stays");
    assert_eq!(input.error.as_deref(), Some("~/workspace is listed twice"));
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(list(&app).items.len(), 3);
    // Edit the selected one (the added one).
    assert_eq!(list(&app).selected, 2);
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "~/проекты");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(list(&app).items[2], "~/проекты", "Cyrillic stays as typed");
    // Remove the missing one.
    press(&mut app, ch('k'));
    press(&mut app, ch('d'));
    assert_eq!(list(&app).items, ["~/workspace", "~/проекты"]);
    // Back to Settings: the changes are unsaved.
    press(&mut app, key(KeyCode::Esc));
    assert_eq!(win(&app).mode, Mode::Browse);
    assert_eq!(
        value(&app, SettingKey::ProjectRoots),
        SettingValue::Paths(vec!["~/workspace".into(), "~/проекты".into()])
    );
    assert!(win(&app).session.is_modified(SettingKey::ProjectRoots));
}

#[test]
fn tab_completes_a_folder_name() {
    let (_env, mut app) = opened();
    select(&mut app, "extra projects");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ch('a'));
    let text = |app: &App| list(app).input.as_ref().unwrap().input.text();
    type_in(&mut app, "~/wor");
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(text(&app), "~/workspace/");
    // Several matches with no longer common start: the names are listed.
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(text(&app), "~/workspace/");
    assert_eq!(notice(&app), "4 folders: api  docs  scratch  web");
    type_in(&mut app, "a");
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(text(&app), "~/workspace/api/");
    press(&mut app, key(KeyCode::Enter));
    assert_eq!(list(&app).items, ["~/notes", "~/workspace/api"]);
    assert_eq!(
        win(&app)
            .folder_note(SettingKey::ExtraProjects, "~/workspace/api")
            .0,
        "✓ git clone"
    );
    // Nothing matches.
    press(&mut app, ch('a'));
    type_in(&mut app, "~/zz");
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(text(&app), "~/zz");
    assert_eq!(notice(&app), "no folder matches ~/zz");
}

#[test]
fn one_folder_job_runs_at_a_time() {
    let (_env, mut app) = opened();
    select(&mut app, "project roots");
    // Open the list without running the folder job.
    let effects = app.handle_key(key(KeyCode::Enter));
    let [Effect::CheckFolders(job)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(job.probe, ["~/old-projects", "~/workspace"]);
    // Typing while it runs asks for nothing more yet.
    assert!(app.handle_key(ch('a')).is_empty());
    for c in "~/notes".chars() {
        assert!(app.handle_key(ch(c)).is_empty(), "a job is still running");
    }
    let next = app.folders_checked(job.run());
    let [Effect::CheckFolders(next)] = next.as_slice() else {
        panic!("{next:?}");
    };
    assert!(next.probe.contains(&"~/notes".to_owned()));
}

#[test]
fn keys_typed_under_a_cyrillic_layout_are_commands_outside_text_fields() {
    let (_env, mut app) = opened();
    // `о` is `j`, `л` is `k`.
    press(&mut app, ch('о'));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiTheme));
    press(&mut app, ch('л'));
    assert_eq!(win(&app).selected_key(), Some(SettingKey::UiStyle));
    // `Ctrl+ы` is `Ctrl+S`.
    select(&mut app, "default effort");
    press(&mut app, key(KeyCode::Right));
    let effects = app.handle_key(KeyEvent::new(KeyCode::Char('ы'), KeyModifiers::CONTROL));
    assert!(
        matches!(effects.as_slice(), [Effect::SaveConfig(_)]),
        "{effects:?}"
    );
}

// ---- through the real backend --------------------------------------------------

#[test]
fn edits_are_saved_to_the_file_keeping_its_comments() {
    let (env, mut app) = opened();
    select(&mut app, "theme");
    press(&mut app, key(KeyCode::Right));
    select(&mut app, "poll interval");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('u'));
    type_in(&mut app, "7");
    press(&mut app, key(KeyCode::Enter));
    select(&mut app, "project roots");
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ch('j'));
    press(&mut app, ch('d'));
    press(&mut app, key(KeyCode::Esc));
    select(&mut app, "extra projects");
    press(&mut app, key(KeyCode::Delete));
    select(&mut app, "style");
    press(&mut app, key(KeyCode::Delete));
    assert_eq!(win(&app).change_count(), 5);
    save(&mut app);
    assert!(app.settings().is_some());
    assert!(
        notice(&app).starts_with("✓ Saved 5 changes"),
        "{}",
        notice(&app)
    );

    // Read back from disk.
    let text = file(&env);
    for kept in [
        "# Watchtower config for the Settings tests.",
        "# Folders scanned for git clones.",
        "default_account = \"main\"",
        "[[accounts]]",
        "config_dir = \"~/.claude-alt\"",
    ] {
        assert!(text.contains(kept), "{kept:?} lost:\n{text}");
    }
    assert!(text.contains("theme = \"warm\""), "{text}");
    assert!(text.contains("poll_interval_secs = 7"), "{text}");
    assert!(text.contains("project_roots = [\"~/workspace\"]"), "{text}");
    assert!(
        !text.contains("extra_projects"),
        "reset key removed:\n{text}"
    );
    assert!(!text.contains("style ="), "reset key removed:\n{text}");
    let config = watchtower::config::Config::load_from(&env.path, &env.home).unwrap();
    assert_eq!(config.ui_theme, UiTheme::Warm);
    assert_eq!(config.ui_style, UiStyle::Dashboard);
    assert_eq!(config.poll_interval_secs, 7);
    assert!(config.extra_projects.is_empty());
    // The original is kept once, and the running tower took the new file.
    let backup = fs::read_to_string(config_edit::backup_path(&env.path)).unwrap();
    assert_eq!(backup, CONFIG);
    assert_eq!(app.config().poll_interval_secs, 7);
    assert_eq!(app.theme, UiTheme::Warm);
    // Reopening shows the saved values with no changes.
    press(&mut app, key(KeyCode::Esc));
    open(&mut app);
    assert!(!win(&app).session.has_changes());
    assert_eq!(
        value(&app, SettingKey::PollIntervalSecs),
        SettingValue::Int(7)
    );
}
