//! The welcome window (doc/0048) on the app and with `TestBackend`: a temp
//! home, and the window's effects run on this thread. The real binary in a
//! pseudo-terminal is in `welcome_pty.rs`.

mod common;
mod settings_common;

use std::fs;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use settings_common::{
    Env, app_for, ch, ctrl, env, file, key, press, render, run, screen, type_in,
};
use watchtower::app::{App, Effect};
use watchtower::config::{CardSize, Config, UiStyle, UiTheme};
use watchtower::config_edit::{EXAMPLE, EditSession, first_run_template};
use watchtower::settings::Mode;
use watchtower::welcome::{self, COMMON_FOLDERS, ClaudeProbe, Origin, ProjectChoice, Step};

const ALL_STEPS: usize = 5;

fn enter() -> KeyEvent {
    key(KeyCode::Enter)
}

fn esc() -> KeyEvent {
    key(KeyCode::Esc)
}

/// The app with the welcome window open and what it asks for run; the Claude
/// Code probe is left for the test to answer.
fn opened(env: &Env, origin: Origin) -> App {
    let mut app = app_for(env);
    let effects = app.open_welcome(origin, (100, 30));
    let left = run(&mut app, effects);
    assert!(
        matches!(left.as_slice(), [Effect::ProbeClaude(_)]),
        "{left:?}"
    );
    assert!(app.settings().is_some(), "the window did not open");
    app
}

fn state(app: &App) -> &welcome::Welcome {
    app.settings().unwrap().welcome.as_ref().unwrap()
}

fn step(app: &App) -> Step {
    state(app).step
}

/// Presses `Enter` until the window is on `to`.
fn go_to(app: &mut App, to: Step) {
    while step(app) != to {
        press(app, enter());
    }
}

fn projects(app: &App) -> Vec<(String, bool)> {
    state(app)
        .projects
        .iter()
        .map(|c| (c.path.clone(), c.ticked))
        .collect()
}

fn loaded(env: &Env) -> Config {
    Config::load_from(&env.path, &env.home).unwrap()
}

/// `~/name` under the temp home, with `clones` git clones in it.
fn folder(env: &Env, name: &str, clones: usize) {
    fs::create_dir_all(env.home.join(name)).unwrap();
    for i in 0..clones {
        fs::create_dir_all(env.home.join(name).join(format!("repo{i}/.git"))).unwrap();
    }
}

/// Every comment line of the example that the file must keep.
fn assert_keeps_comments(text: &str) {
    let rewritten = [
        "# name = \"work\"",
        "# command = \"claude-work\"",
        "# config_dir = \"~/.claude-work\"",
        "# group = \"work\"",
    ];
    for line in EXAMPLE.lines().filter(|l| l.trim_start().starts_with('#')) {
        assert!(
            text.lines().any(|l| l == line) || rewritten.contains(&line),
            "lost the comment {line:?}"
        );
    }
}

// ---- when it opens ----------------------------------------------------------

#[test]
fn only_a_missing_file_or_tower_setup_opens_it() {
    assert_eq!(Origin::for_start(false, false), Some(Origin::FirstRun));
    assert_eq!(Origin::for_start(false, true), None);
    assert_eq!(Origin::for_start(true, false), Some(Origin::Setup));
    assert_eq!(Origin::for_start(true, true), Some(Origin::Setup));
}

#[test]
fn a_small_terminal_gets_a_notice_and_defaults_and_nothing_is_written() {
    let env = env(None);
    let mut app = app_for(&env);
    for size in [(79, 24), (80, 23), (40, 10)] {
        assert!(app.open_welcome(Origin::FirstRun, size).is_empty());
        assert!(app.settings().is_none());
    }
    assert_eq!(app.style, UiStyle::Dashboard);
    assert!(
        common::status_text(&app).contains("the welcome window needs a 80x24 terminal"),
        "{}",
        common::status_text(&app)
    );
    assert!(!env.path.exists());
}

#[test]
fn a_terminal_that_shrinks_shows_a_plain_notice_and_esc_writes_nothing() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Look);
    let shown = screen(&mut app, 60, 12).join("\n");
    assert!(
        shown.contains("This window needs a 80x24 terminal"),
        "{shown}"
    );
    assert!(state(&app).too_small);
    // Other keys wait for the terminal to grow.
    press(&mut app, enter());
    assert_eq!(step(&app), Step::Look);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("step 2 of 5"), "{shown}");
    assert!(!state(&app).too_small);
    screen(&mut app, 60, 12);
    press(&mut app, esc());
    assert!(app.settings().is_none());
    assert_eq!(app.style, UiStyle::Dashboard, "the defaults");
    assert!(!env.path.exists());
}

// ---- the template --------------------------------------------------------------

#[test]
fn the_template_is_the_commented_example_with_defaults_and_one_account() {
    let env = env(None);
    let text = first_run_template();
    let config = Config::parse(&text, &env.home).unwrap();
    // Nothing but the chosen values differs from what no file gives, so
    // saving it needs no restart for the accounts.
    assert_eq!(config, Config::defaults(&env.home));
    assert_eq!(config.accounts.len(), 1);
    assert_eq!(config.accounts[0].name, "main");
    assert_eq!(config.accounts[0].command, "claude");
    assert!(text.contains("\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n"));
    assert!(text.contains("\n# [[accounts]]\n# name = \"alt\"\n"));
    assert!(text.contains("project_roots = []\n"));
    assert!(text.contains("extra_projects = []\n"));
    assert_keeps_comments(&text);
}

// ---- Esc: skips the rest and saves ----------------------------------------------

#[test]
fn esc_at_every_step_writes_a_valid_config_with_the_current_choices() {
    for at in 0..ALL_STEPS {
        let env = env(None);
        let mut app = opened(&env, Origin::FirstRun);
        for _ in 0..at {
            press(&mut app, enter());
        }
        press(&mut app, esc());
        assert!(app.settings().is_none(), "step {at}: still open");
        let text = file(&env);
        let config = loaded(&env);
        assert_eq!(config.ui_style, UiStyle::Workspace, "step {at}");
        assert!(text.contains("style = \"workspace\""), "step {at}");
        assert_keeps_comments(&text);
        // The temp home's ~/workspace holds three clones.
        assert_eq!(
            config.project_roots,
            [env.home.join("workspace")],
            "step {at}"
        );
        assert!(config.extra_projects.is_empty(), "step {at}");
        assert!(text.contains("project_roots = [\"~/workspace\"]"), "{text}");
        // Carries on with those settings, without a restart.
        assert_eq!(app.style, UiStyle::Workspace, "step {at}");
        assert_eq!(app.config().ui_style, UiStyle::Workspace, "step {at}");
        assert_eq!(app.config().project_roots, config.project_roots);
        assert!(
            app.restart_note().is_none(),
            "accounts did not change: {:?}",
            app.restart_note()
        );
        assert!(!common::status_text(&app).contains("no config at"));
        // And it never comes back.
        assert!(app.config_source().unwrap().found);
    }
}

#[test]
fn a_new_file_gets_no_backup_and_the_write_leaves_no_temp_file() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    press(&mut app, esc());
    assert!(env.path.exists());
    let names: Vec<String> = fs::read_dir(env.path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut names = names;
    names.sort();
    assert_eq!(names, ["config.toml", "config.toml.lock"]);
}

#[test]
fn the_folders_are_created_when_missing() {
    let env = env(None);
    fs::remove_dir_all(env.path.parent().unwrap()).unwrap();
    fs::remove_dir_all(env.home.join(".config")).unwrap();
    let mut app = opened(&env, Origin::FirstRun);
    press(&mut app, esc());
    assert!(env.path.exists());
    assert_eq!(loaded(&env).ui_style, UiStyle::Workspace);
}

// ---- Ctrl+C: quits, writes nothing ----------------------------------------------

#[test]
fn ctrl_c_at_every_step_quits_and_writes_nothing() {
    for at in 0..ALL_STEPS {
        let env = env(None);
        let mut app = opened(&env, Origin::FirstRun);
        for _ in 0..at {
            press(&mut app, enter());
        }
        let left = press(&mut app, ctrl('c'));
        assert_eq!(left, [Effect::Quit], "step {at}");
        assert!(!env.path.exists(), "step {at}");
    }
}

// ---- the steps -------------------------------------------------------------------

#[test]
fn enter_goes_on_and_backspace_and_shift_tab_go_back() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    for want in [Step::Look, Step::Projects, Step::Options, Step::Done] {
        press(&mut app, enter());
        assert_eq!(step(&app), want);
    }
    press(&mut app, key(KeyCode::Backspace));
    assert_eq!(step(&app), Step::Options);
    press(&mut app, key(KeyCode::BackTab));
    assert_eq!(step(&app), Step::Projects);
    press(&mut app, key(KeyCode::BackTab));
    press(&mut app, key(KeyCode::BackTab));
    press(&mut app, key(KeyCode::BackTab));
    assert_eq!(step(&app), Step::Welcome);
}

#[test]
fn look_previews_as_it_changes_and_the_card_size_only_in_the_workspace() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Look);
    assert_eq!(app.style, UiStyle::Workspace, "preselected, and previewed");
    let window = app.settings().unwrap();
    assert_eq!(window.look_rows().len(), 3);

    press(&mut app, key(KeyCode::Left));
    assert_eq!(app.style, UiStyle::Minimal);
    assert_eq!(app.settings().unwrap().look_rows().len(), 2);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(!shown.contains("card size"), "{shown}");

    press(&mut app, key(KeyCode::Left));
    assert_eq!(app.style, UiStyle::Dashboard);
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.style, UiStyle::Workspace);

    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.theme, UiTheme::Warm);

    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Right));
    press(&mut app, esc());
    let config = loaded(&env);
    assert_eq!(config.ui_theme, UiTheme::Warm);
    assert_eq!(config.ui_card_size, CardSize::Small);
    assert_eq!(app.theme, UiTheme::Warm);
    assert_eq!(app.config().ui_card_size, CardSize::Small);
}

#[test]
fn the_written_choices_can_all_be_the_defaults() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Look);
    press(&mut app, key(KeyCode::Left));
    press(&mut app, key(KeyCode::Left));
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    let text = file(&env);
    assert!(text.contains("style = \"dashboard\""));
    assert_eq!(loaded(&env).ui_style, UiStyle::Dashboard);
    assert_eq!(app.style, UiStyle::Dashboard);
}

#[test]
fn options_switch_and_are_written() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Options);
    press(&mut app, ch(' '));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Left));
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    let config = loaded(&env);
    assert!(config.usage_show);
    assert!(!config.track_subagents);
    assert!(!config.ui_tab_title);
    assert_keeps_comments(&file(&env));
}

// ---- the projects ----------------------------------------------------------------

#[test]
fn the_usual_folders_are_suggested_with_their_counts_and_those_with_repos_ticked() {
    let env = env(None);
    folder(&env, "projects", 2);
    folder(&env, "code", 0);
    fs::write(env.home.join("repos"), "a file, not a folder").unwrap();
    let mut app = opened(&env, Origin::FirstRun);
    assert_eq!(
        projects(&app),
        [
            ("~/projects".to_owned(), true),
            ("~/workspace".to_owned(), true),
            ("~/code".to_owned(), false),
        ],
        "in the order of the list, without what is missing or no folder"
    );
    go_to(&mut app, Step::Projects);
    let shown = screen(&mut app, 80, 24);
    let row = |needle: &str| settings_common::line_with(&shown, needle).to_owned();
    assert!(row("~/projects").contains("✓ 2 git clones"), "{shown:?}");
    assert!(row("~/workspace").contains("✓ 3 git clones"), "{shown:?}");
    assert!(row("~/code").contains("no git clones in it"), "{shown:?}");
    assert!(row("~/projects").contains("[x]"));
    assert!(row("~/code").contains("[ ]"));
    assert_eq!(COMMON_FOLDERS.len(), 8);

    // Space ticks and unticks, and the draft follows.
    press(&mut app, ch(' '));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, ch(' '));
    press(&mut app, esc());
    let config = loaded(&env);
    assert_eq!(
        config.project_roots,
        [env.home.join("workspace"), env.home.join("code")]
    );
    assert!(file(&env).contains("project_roots = [\"~/workspace\", \"~/code\"]"));
}

#[test]
fn no_usual_folder_says_so_and_enter_still_goes_on() {
    let env = env(None);
    fs::remove_dir_all(env.home.join("workspace")).unwrap();
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Projects);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(
        shown.contains("None of the usual folders is here"),
        "{shown}"
    );
    press(&mut app, enter());
    assert_eq!(step(&app), Step::Options);
    press(&mut app, esc());
    assert!(loaded(&env).project_roots.is_empty());
    assert!(file(&env).contains("project_roots = []\n"));
}

/// To the field for another folder.
fn to_field(app: &mut App) {
    go_to(app, Step::Projects);
    while state(app).project_row < state(app).projects.len() {
        press(app, key(KeyCode::Down));
    }
}

#[test]
fn a_typed_folder_must_exist_keeps_its_tilde_and_lands_on_the_right_list() {
    let env = env(None);
    folder(&env, "elsewhere/stuff", 1);
    fs::create_dir_all(env.home.join("one-clone/.git")).unwrap();
    let mut app = opened(&env, Origin::FirstRun);
    to_field(&mut app);

    type_in(&mut app, "~/nowhere");
    press(&mut app, enter());
    assert_eq!(state(&app).field_error.as_deref(), Some("not found"));
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("✗ not found"), "{shown}");
    assert_eq!(step(&app), Step::Projects, "stays on the step");

    press(&mut app, ctrl('u'));
    assert_eq!(state(&app).field_error, None);
    type_in(&mut app, "~/elsewhere/stuff/");
    press(&mut app, enter());
    assert_eq!(state(&app).field.text(), "", "the field is empty again");
    press(&mut app, ctrl('u'));
    // Not a folder that holds clones: offered itself in the dialog.
    type_in(&mut app, "~/one-clone");
    press(&mut app, enter());
    let last = state(&app).projects.last().unwrap().clone();
    assert_eq!(
        last,
        ProjectChoice {
            path: "~/one-clone".to_owned(),
            list: watchtower::config_edit::SettingKey::ExtraProjects,
            ticked: true,
        }
    );
    // A folder already listed is only ticked.
    let n = state(&app).projects.len();
    type_in(&mut app, "~/workspace");
    press(&mut app, enter());
    assert_eq!(state(&app).projects.len(), n);

    // With nothing typed, Enter goes on.
    press(&mut app, enter());
    assert_eq!(step(&app), Step::Options);
    press(&mut app, esc());
    let text = file(&env);
    assert!(
        text.contains("project_roots = [\"~/workspace\", \"~/elsewhere/stuff\"]"),
        "{text}"
    );
    assert!(
        text.contains("extra_projects = [\"~/one-clone\"]"),
        "{text}"
    );
    let config = loaded(&env);
    assert_eq!(config.extra_projects, [env.home.join("one-clone")]);
}

#[test]
fn an_absolute_path_a_file_and_tab_completion_work_in_the_field() {
    let env = env(None);
    folder(&env, "unique-name", 1);
    fs::write(env.home.join("a-file"), "x").unwrap();
    let mut app = opened(&env, Origin::FirstRun);
    to_field(&mut app);
    type_in(&mut app, "~/a-file");
    press(&mut app, enter());
    assert_eq!(state(&app).field_error.as_deref(), Some("not a folder"));
    press(&mut app, ctrl('u'));

    type_in(&mut app, "~/unique");
    press(&mut app, key(KeyCode::Tab));
    assert_eq!(state(&app).field.text(), "~/unique-name/");
    press(&mut app, enter());
    assert_eq!(state(&app).projects.last().unwrap().path, "~/unique-name");

    let absolute = env.home.join("unique-name").display().to_string();
    type_in(&mut app, &absolute);
    press(&mut app, enter());
    assert_eq!(
        state(&app)
            .projects
            .iter()
            .filter(|c| c.path.contains("unique-name"))
            .count(),
        1,
        "the same folder written differently"
    );
}

#[test]
fn the_field_takes_text_and_esc_clears_it_before_it_skips() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    to_field(&mut app);
    // Letters that are command keys elsewhere, and Cyrillic as typed.
    type_in(&mut app, "jk д");
    assert_eq!(state(&app).field.text(), "jk д");
    press(&mut app, key(KeyCode::Backspace));
    assert_eq!(state(&app).field.text(), "jk ");
    assert_eq!(step(&app), Step::Projects);
    press(&mut app, esc());
    assert_eq!(state(&app).field.text(), "");
    assert!(app.settings().is_some(), "Esc only cleared the field");
    // Empty, Backspace goes back; then Esc skips.
    press(&mut app, key(KeyCode::Backspace));
    assert_eq!(step(&app), Step::Look);
    press(&mut app, esc());
    assert!(app.settings().is_none());
    assert!(env.path.exists());
}

#[test]
fn on_a_list_row_letters_are_commands_and_on_the_field_they_are_text() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    go_to(&mut app, Step::Projects);
    assert_eq!(state(&app).project_row, 0);
    press(&mut app, ch('j'));
    assert_eq!(state(&app).project_row, 1, "j moves");
    press(&mut app, ch('о'));
    assert_eq!(
        state(&app).project_row,
        1,
        "at the field the Cyrillic о types"
    );
    assert_eq!(state(&app).field.text(), "о");
}

// ---- Claude Code --------------------------------------------------------------------

#[test]
fn the_welcome_step_says_what_it_found() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("checking…"), "{shown}");
    assert!(shown.contains("looking…"), "{shown}");
    assert!(shown.contains("github.com/ruslan1us/watchtower"), "{shown}");
    app.claude_version_found(Ok("2.1.316".to_owned()));
    assert_eq!(state(&app).claude, ClaudeProbe::Found("2.1.316".to_owned()));
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("2.1.316"), "{shown}");
    assert_eq!(welcome::version_text("2.1.316 (Claude Code)\n"), "2.1.316");
    assert_eq!(welcome::version_text("\n weird\x07 text\n"), "weird? text");
    assert_eq!(welcome::version_text(""), "");
}

#[test]
fn claude_that_cannot_run_is_said_plainly_and_the_window_goes_on() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    app.claude_version_found(Err(
        "cannot run `claude`: No such file or directory (os error 2)".to_owned(),
    ));
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("could not be run"), "{shown}");
    assert!(shown.contains("install it later"), "{shown}");
    press(&mut app, enter());
    assert_eq!(step(&app), Step::Look);
    press(&mut app, esc());
    assert_eq!(loaded(&env).ui_style, UiStyle::Workspace);
}

// ---- a write that fails --------------------------------------------------------------

/// `config.toml.lock` as a folder: the save cannot take its lock.
fn break_the_lock(env: &Env) {
    fs::create_dir_all(env.path.with_file_name("config.toml.lock")).unwrap();
}

#[test]
fn a_failed_write_says_so_and_goes_on_with_the_defaults() {
    let env = env(None);
    break_the_lock(&env);
    let mut app = opened(&env, Origin::FirstRun);
    press(&mut app, esc());
    assert!(app.settings().is_some(), "the window says why first");
    assert!(state(&app).failed.is_some());
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("could not be written"), "{shown}");
    assert!(shown.contains("built-in defaults for this run"), "{shown}");
    // Other keys wait.
    press(&mut app, ch(' '));
    assert!(app.settings().is_some());
    press(&mut app, enter());
    assert!(app.settings().is_none());
    assert!(!env.path.exists());
    assert_eq!(app.style, UiStyle::Dashboard, "the defaults");
    assert!(app.config().project_roots.is_empty());
    let status = common::status_text(&app);
    assert!(status.contains("not saved"), "{status}");
    assert!(status.contains("comes back next time"), "{status}");
}

#[test]
fn a_failed_write_in_setup_keeps_the_settings() {
    let env = env(Some(settings_common::CONFIG));
    break_the_lock(&env);
    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Look);
    press(&mut app, key(KeyCode::Right));
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("The file stays as it was"), "{shown}");
    press(&mut app, enter());
    assert!(app.settings().is_none());
    assert_eq!(app.style, UiStyle::Dashboard);
    assert_eq!(file(&env), settings_common::CONFIG);
}

// ---- a file that appears meanwhile ----------------------------------------------------

#[test]
fn a_file_another_tower_wrote_meanwhile_is_asked_about() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    fs::write(&env.path, "[ui]\nstyle = \"minimal\"\n").unwrap();
    press(&mut app, esc());
    assert!(matches!(app.settings().unwrap().mode, Mode::Conflict(_)));
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("Changed in the file"), "{shown}");
    // No: the other file stays, and the next reload applies it.
    press(&mut app, ch('n'));
    assert!(app.settings().is_none());
    assert_eq!(file(&env), "[ui]\nstyle = \"minimal\"\n");

    let env2 = settings_common::env(None);
    let mut app = opened(&env2, Origin::FirstRun);
    fs::write(&env2.path, "[ui]\nstyle = \"minimal\"\n").unwrap();
    press(&mut app, esc());
    press(&mut app, ch('y'));
    assert!(app.settings().is_none());
    assert_eq!(loaded(&env2).ui_style, UiStyle::Workspace);
}

// ---- tower setup ----------------------------------------------------------------------

const SETUP_FILE: &str = r#"# My own header comment.
unknown_top = "kept"

project_roots = ["~/workspace"]   # my trailing comment
extra_projects = ["~/notes"]

[ui]
# My comment on the theme.
theme = "warm"
unknown_ui = 1

[future]
anything = "stays"

[[accounts]]
name = "main"
command = "claude"
"#;

#[test]
fn setup_starts_from_the_file_and_does_not_preselect() {
    let env = env(Some(SETUP_FILE));
    folder(&env, "projects", 2);
    let mut app = opened(&env, Origin::Setup);
    assert_eq!(
        projects(&app),
        [
            ("~/workspace".to_owned(), true),
            ("~/notes".to_owned(), true),
            ("~/projects".to_owned(), false),
        ]
    );
    assert_eq!(app.style, UiStyle::Dashboard, "as the file says");
    assert_eq!(app.theme, UiTheme::Warm);
    assert!(!app.settings().unwrap().session.has_changes());
    go_to(&mut app, Step::Done);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("Nothing changed"), "{shown}");
    press(&mut app, enter());
    assert!(app.settings().is_none());
    assert_eq!(file(&env), SETUP_FILE, "nothing to write");
    assert!(!env.path.with_file_name("config.toml.bak").exists());
}

#[test]
fn setup_esc_writes_nothing_after_any_change() {
    for at in 0..ALL_STEPS {
        let env = env(Some(SETUP_FILE));
        let mut app = opened(&env, Origin::Setup);
        go_to(&mut app, Step::Look);
        press(&mut app, key(KeyCode::Right));
        press(&mut app, key(KeyCode::BackTab));
        for _ in 0..at {
            press(&mut app, enter());
        }
        assert_eq!(app.style, UiStyle::Minimal, "previewed");
        press(&mut app, esc());
        assert!(app.settings().is_none(), "step {at}");
        assert_eq!(file(&env), SETUP_FILE, "step {at}");
        assert_eq!(
            app.style,
            UiStyle::Dashboard,
            "step {at}: back to the file's"
        );
    }
}

#[test]
fn setup_with_no_file_starts_as_a_first_start_but_esc_still_cancels() {
    let env = env(None);
    let mut app = opened(&env, Origin::Setup);
    assert_eq!(app.style, UiStyle::Workspace);
    press(&mut app, esc());
    assert!(!env.path.exists());
    assert_eq!(app.style, UiStyle::Dashboard);

    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    assert_eq!(loaded(&env).ui_style, UiStyle::Workspace);
    assert_keeps_comments(&file(&env));
}

#[test]
fn setup_changes_only_what_changed_and_keeps_comments_and_unknown_keys() {
    let env = env(Some(SETUP_FILE));
    folder(&env, "projects", 2);
    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Look);
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Left));
    assert_eq!(app.theme, UiTheme::Neutral);
    go_to(&mut app, Step::Projects);
    // Unticks ~/notes, ticks ~/projects.
    press(&mut app, key(KeyCode::Down));
    press(&mut app, ch(' '));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, ch(' '));
    go_to(&mut app, Step::Done);
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("Enter saves these 3 changes"), "{shown}");
    assert!(shown.contains("~/workspace · ~/notes") || shown.contains("~/workspace · ~/projects"));
    press(&mut app, enter());
    assert!(app.settings().is_none());
    let expected = SETUP_FILE
        .replace(
            "project_roots = [\"~/workspace\"]   # my trailing comment",
            "project_roots = [\"~/workspace\", \"~/projects\"]   # my trailing comment",
        )
        .replace("extra_projects = [\"~/notes\"]", "extra_projects = []")
        .replace("theme = \"warm\"", "theme = \"neutral\"");
    assert_eq!(file(&env), expected);
    // The one-time backup of 0022.
    let bak = fs::read_to_string(env.path.with_file_name("config.toml.bak")).unwrap();
    assert_eq!(bak, SETUP_FILE);
    assert_eq!(app.theme, UiTheme::Neutral);
}

#[test]
fn setup_going_there_and_back_changes_nothing_even_for_a_key_not_in_the_file() {
    let env = env(Some(SETUP_FILE));
    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Look);
    // `style` is not in the file: there and back is no change.
    press(&mut app, key(KeyCode::Right));
    press(&mut app, key(KeyCode::Left));
    go_to(&mut app, Step::Options);
    press(&mut app, ch(' '));
    press(&mut app, ch(' '));
    assert!(!app.settings().unwrap().session.has_changes());
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    assert_eq!(file(&env), SETUP_FILE);
}

#[test]
fn setup_shows_the_accounts_but_never_changes_them() {
    let env = env(Some(settings_common::CONFIG));
    let mut app = opened(&env, Origin::Setup);
    let shown = screen(&mut app, 100, 30).join("\n");
    assert!(
        shown.contains("main runs claude, alt runs claude-alt. More accounts:"),
        "{shown}"
    );
    go_to(&mut app, Step::Look);
    press(&mut app, key(KeyCode::Right));
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    let text = file(&env);
    assert!(
        text.contains("name = \"alt\"\ncommand = \"claude-alt\""),
        "{text}"
    );
    assert!(text.contains("group = \"personal\""), "{text}");
}

#[test]
fn setup_keeps_a_symlink_a_symlink() {
    let env = env(None);
    let target = env.home.join("dotfiles-config.toml");
    fs::write(&target, SETUP_FILE).unwrap();
    std::os::unix::fs::symlink(&target, &env.path).unwrap();
    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Look);
    press(&mut app, key(KeyCode::Right));
    go_to(&mut app, Step::Done);
    press(&mut app, enter());
    assert!(
        fs::symlink_metadata(&env.path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_to_string(&target)
            .unwrap()
            .contains("style = \"minimal\"")
    );
}

#[test]
fn no_session_can_open_the_new_session_dialog_behind_it() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    press(&mut app, ch('n'));
    assert!(app.dialog().is_none());
    assert_eq!(step(&app), Step::Welcome);
}

// ---- drawing -----------------------------------------------------------------------

/// The app on `at` of a walk with long values.
fn at_step(env: &Env, at: usize, theme: UiTheme, color: bool) -> App {
    let mut app = opened(env, Origin::FirstRun);
    app.claude_version_found(Ok("2.1.316".to_owned()));
    app.color = color;
    app.set_theme(theme);
    for _ in 0..at {
        press(&mut app, enter());
    }
    app
}

fn assert_fits(shown: &[String], w: u16) {
    for line in shown {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= usize::from(w),
            "too wide: {line}"
        );
    }
}

#[test]
fn every_step_draws_at_both_sizes_in_every_look() {
    let looks = [
        ("dark", UiTheme::Neutral, true),
        ("light", UiTheme::NeutralLight, true),
        ("no-colour", UiTheme::Neutral, false),
    ];
    let needs = [
        ("Welcome", "Tower keeps your Claude Code"),
        ("Look", "‹ workspace ›"),
        ("Projects", "~/workspace"),
        ("Options", "track subagents"),
        ("Done", "Enter saves to"),
    ];
    for (at, (title, needle)) in needs.iter().enumerate() {
        for (name, theme, color) in looks {
            for (w, h) in [(80, 24), (160, 45)] {
                let env = env(None);
                let mut app = at_step(&env, at, theme, color);
                let buf = render(&mut app, w, h);
                let shown = settings_common::lines(&buf);
                let all = shown.join("\n");
                assert_fits(&shown, w);
                assert!(all.contains(*title), "{name} {w}x{h}\n{all}");
                assert!(all.contains(*needle), "{name} {w}x{h}\n{all}");
                assert!(all.contains("Ctrl+C quit"), "{name} {w}x{h}\n{all}");
                let colours = (0..buf.area.height)
                    .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                    .any(|(x, y)| {
                        let cell = &buf[(x, y)];
                        cell.fg != ratatui::style::Color::Reset
                            || cell.bg != ratatui::style::Color::Reset
                    });
                assert_eq!(colours, color, "{name} {w}x{h}");
            }
        }
    }
}

#[test]
fn a_long_list_of_folders_keeps_the_row_and_the_field_in_view() {
    let mut text = String::from("project_roots = [");
    for i in 0..40 {
        text.push_str(&format!("\"~/root-{i:02}\", "));
    }
    text.push_str("]\n");
    let env = env(Some(&text));
    let mut app = opened(&env, Origin::Setup);
    go_to(&mut app, Step::Projects);
    for _ in 0..39 {
        press(&mut app, key(KeyCode::Down));
    }
    let shown = screen(&mut app, 80, 24).join("\n");
    assert!(shown.contains("~/root-39"), "{shown}");
    assert!(shown.contains("another folder"), "{shown}");
    assert!(!shown.contains("~/root-00"), "{shown}");
}

#[test]
fn the_done_step_lists_the_summary_and_the_file() {
    let env = env(None);
    let mut app = at_step(&env, 4, UiTheme::Neutral, true);
    let shown = screen(&mut app, 80, 24).join("\n");
    for want in [
        "workspace (experimental)",
        "neutral",
        "medium",
        "~/workspace",
        "plan usage",
        "track subagents",
        "tab title",
        "main runs claude",
        "~/.config/watchtower/config.toml",
    ] {
        assert!(shown.contains(want), "{want}\n{shown}");
    }
}

#[test]
fn settings_and_its_reset_are_untouched_by_the_template() {
    // A plain Settings window on no file still creates the short file (0022).
    let env = env(None);
    let mut session = EditSession::open(&env.path, &env.home).unwrap();
    session
        .set(
            watchtower::config_edit::SettingKey::UiTheme,
            watchtower::config_edit::SettingValue::Text("warm".to_owned()),
        )
        .unwrap();
    session.save(false).unwrap();
    let text = file(&env);
    assert!(text.starts_with(watchtower::config_edit::HEADER), "{text}");
    assert!(!text.contains("[[accounts]]"), "{text}");
}

#[test]
fn the_ctrl_modifier_on_other_keys_does_nothing() {
    let env = env(None);
    let mut app = opened(&env, Origin::FirstRun);
    press(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert_eq!(step(&app), Step::Welcome);
    assert!(!env.path.exists());
}
