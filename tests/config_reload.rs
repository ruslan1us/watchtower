//! A running tower takes in changes to `config.toml` (doc/0022): the live
//! settings apply, account changes wait for a restart, an invalid file
//! keeps the current config, and tower's own saves are not reloaded.

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::{NoEnviron, status_text};
use watchtower::app::{App, Effect, RESTART_NOTE};
use watchtower::config::{Config, ConfigSource, UiStyle, UiTheme};
use watchtower::config_edit::{self, EditSession, SettingKey, SettingValue};
use watchtower::sidecar::Sidecar;

const TWO: &str = r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#;

fn replace(path: &Path, content: &str) {
    let tmp = path.with_extension("replace-tmp");
    fs::write(&tmp, content).unwrap();
    fs::rename(&tmp, path).unwrap();
}

/// An app started from the config in `path` (written with `content`).
fn app_with(dir: &Path, content: &str) -> (App, std::path::PathBuf) {
    let path = dir.join("config.toml");
    fs::write(&path, content).unwrap();
    let home = dir.to_path_buf();
    let config = Config::load_from(&path, &home).unwrap();
    let mut app = App::new(
        config.clone(),
        home,
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    app.style = config.ui_style;
    app.set_config_source(ConfigSource::probe(&path));
    (app, path)
}

/// Runs the reload check on this thread, as the runtime's worker would.
fn check(app: &mut App) -> Vec<Effect> {
    let job = app.start_config_check().expect("no check running");
    let found = job.check.run();
    app.config_checked(job.epoch, found)
}

#[test]
fn the_first_check_of_an_unchanged_file_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _path) = app_with(dir.path(), TWO);
    let before = app.config().clone();
    assert!(check(&mut app).is_empty());
    assert_eq!(app.config(), &before);
    assert!(!status_text(&app).contains("reloaded"));
    // Now the version is known: nothing to read.
    let job = app.start_config_check().unwrap();
    assert!(job.check.run().is_none());
}

#[test]
fn live_settings_apply_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    check(&mut app);
    let changed = format!(
        "poll_interval_secs = 5\nhandoff_prompt = \"go on\"\nallow_cross_group_handoff = true\n\
         default_model = \"opus\"\ndefault_effort = \"high\"\ndefault_permission_mode = \"plan\"\n\
         default_account = \"alt\"\nproject_roots = [\"~/src\"]\nextra_projects = [\"~/notes\"]\n\
         {TWO_BODY}\n[ui]\nstyle = \"minimal\"\ntheme = \"warm\"\n",
        TWO_BODY = TWO.replace("default_account = \"main\"\n", "")
    );
    replace(&path, &changed);
    let effects = check(&mut app);
    assert_eq!(
        effects,
        vec![Effect::SetPollInterval(Duration::from_secs(5))]
    );
    let c = app.config();
    assert_eq!(c.poll_interval_secs, 5);
    assert_eq!(c.handoff_prompt, "go on");
    assert!(c.allow_cross_group_handoff);
    assert_eq!(c.default_model, "opus");
    assert_eq!(c.default_effort, "high");
    assert_eq!(c.default_permission_mode, "plan");
    assert_eq!(c.default_account, "alt");
    assert_eq!(c.project_roots, vec![dir.path().join("src")]);
    assert_eq!(c.extra_projects, vec![dir.path().join("notes")]);
    assert_eq!(app.style, UiStyle::Minimal);
    assert_eq!(app.theme, UiTheme::Warm);
    assert!(app.restart_note().is_none());
    assert!(
        status_text(&app).contains("reloaded"),
        "{}",
        status_text(&app)
    );
    // The next new-session dialog scans the new roots.
    let scan = app.begin_project_scan();
    assert_eq!(scan.roots, vec![dir.path().join("src")]);
}

#[test]
fn account_changes_wait_for_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    check(&mut app);
    let accounts = app.config().accounts.clone();
    let more = format!(
        "{}\n[[accounts]]\nname = \"third\"\ncommand = \"claude-third\"\n\
         config_dir = \"~/.claude-third\"\n",
        TWO.replace(
            "default_account = \"main\"",
            "default_account = \"third\"\npoll_interval_secs = 4"
        )
    );
    replace(&path, &more);
    let effects = check(&mut app);
    assert_eq!(
        effects,
        vec![Effect::SetPollInterval(Duration::from_secs(4))]
    );
    assert_eq!(
        app.config().accounts,
        accounts,
        "accounts are not applied live"
    );
    assert_eq!(
        app.config().default_account,
        "main",
        "the new default is not running"
    );
    assert_eq!(app.config().poll_interval_secs, 4, "the rest applies");
    assert_eq!(app.restart_note(), Some(RESTART_NOTE));
    assert!(status_text(&app).contains(RESTART_NOTE));
    // Back to the running accounts: the note goes.
    replace(&path, TWO);
    check(&mut app);
    assert_eq!(app.restart_note(), None);
}

#[test]
fn an_invalid_file_keeps_the_current_config_and_shows_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    check(&mut app);
    let before = app.config().clone();
    replace(&path, &format!("poll_interval_secs = 0\n{TWO}"));
    assert!(check(&mut app).is_empty());
    assert_eq!(app.config(), &before);
    let err = app.config_error().expect("the error is kept").to_owned();
    assert!(err.contains("poll_interval_secs must be 1 to 60"), "{err}");
    // Not read again while unchanged, and still shown.
    let job = app.start_config_check().unwrap();
    assert!(job.check.run().is_none());
    app.config_checked(job.epoch, None);
    assert!(status_text(&app).contains("config not reloaded"));
    // Fixed: applied, the error goes.
    replace(&path, &format!("poll_interval_secs = 3\n{TWO}"));
    check(&mut app);
    assert_eq!(app.config().poll_interval_secs, 3);
    assert!(app.config_error().is_none());
}

#[test]
fn a_saved_file_is_applied_and_not_reloaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    let mut session = EditSession::open(&path, dir.path()).unwrap();
    session
        .set(SettingKey::UiTheme, SettingValue::Text("terminal".into()))
        .unwrap();
    // A check started before the save reports after it: dropped.
    let stale = app.start_config_check().unwrap();
    app.config_saved(session.save_job(false).run());
    assert_eq!(app.theme, UiTheme::Terminal);
    assert!(status_text(&app).contains("saved"), "{}", status_text(&app));
    replace(&path, &format!("poll_interval_secs = 9\n{TWO}"));
    let found = stale.check.run();
    assert!(found.is_some());
    assert!(app.config_checked(stale.epoch, found).is_empty());
    assert_eq!(
        app.config().poll_interval_secs,
        2,
        "the stale check is dropped"
    );
    // The window takes the result and is up to date.
    let outcome = app.take_config_save_result().unwrap().unwrap();
    session.finish_save(&outcome);
    assert!(!session.has_changes());
    assert!(app.take_config_save_result().is_none());
}

#[test]
fn tower_does_not_reload_its_own_save() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    check(&mut app);
    let mut session = EditSession::open(&path, dir.path()).unwrap();
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(6))
        .unwrap();
    let effects = app.config_saved(session.save_job(false).run());
    assert_eq!(
        effects,
        vec![Effect::SetPollInterval(Duration::from_secs(6))]
    );
    let job = app.start_config_check().unwrap();
    assert!(job.check.run().is_none(), "the saved version is known");
}

#[test]
fn a_created_file_clears_the_missing_config_warning() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watchtower/config.toml");
    let home = dir.path().to_path_buf();
    let mut app = App::new(
        Config::defaults(&home),
        home.clone(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    app.set_config_source(ConfigSource::probe(&path));
    assert!(status_text(&app).contains("no config at"));
    let mut session = EditSession::open(&path, &home).unwrap();
    session
        .set(SettingKey::UiStyle, SettingValue::Text("minimal".into()))
        .unwrap();
    app.config_saved(session.save_job(false).run());
    assert!(
        !status_text(&app).contains("no config at"),
        "{}",
        status_text(&app)
    );
    assert!(app.config_source().unwrap().found);
    assert_eq!(app.style, UiStyle::Minimal);
}

#[test]
fn the_editor_result_is_applied_or_its_error_shown() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = app_with(dir.path(), TWO);
    assert_eq!(app.edit_config(), vec![Effect::EditConfig(path.clone())]);
    replace(&path, &format!("[ui]\ntheme = \"warm\"\n{TWO}"));
    let snapshot = config_edit::read_config(&path, dir.path());
    app.config_editor_finished(Ok(snapshot));
    assert_eq!(app.theme, UiTheme::Warm);
    replace(&path, &format!("[ui]\ntheme = \"pink\"\n{TWO}"));
    let snapshot = config_edit::read_config(&path, dir.path());
    app.config_editor_finished(Ok(snapshot));
    assert_eq!(app.theme, UiTheme::Warm, "the current config is kept");
    assert!(app.config_error().is_some());
    app.config_editor_finished(Err("the editor exited with code 1".into()));
    assert!(status_text(&app).contains("code 1"));
}
