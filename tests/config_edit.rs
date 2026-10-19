//! Tower writing its own config for the Settings window (doc/0022): format
//! preserving edits, reset to default, validation, conflicts, the one-time
//! backup, atomic writes, creation, and the editor.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use watchtower::attach::{Handover, InheritSpawner, Termination};
use watchtower::config::{Config, ConfigError, UiStyle, UiTheme};
use watchtower::config_edit::{
    self, EditSession, HEADER, SaveError, SettingKey, SettingKind, SettingValue, ValueSource,
};

const HOME: &str = "/home/user";
const EXAMPLE: &str = include_str!("../config.example.toml");

fn home() -> &'static Path {
    Path::new(HOME)
}

fn text(s: &str) -> SettingValue {
    SettingValue::Text(s.to_owned())
}

/// A temp dir holding `config.toml` with `content`.
fn config_file(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, content).unwrap();
    (dir, path)
}

/// Replaces the file by a rename, as editors and towers do, so its stamp
/// surely changes.
fn replace(path: &Path, content: &str) {
    let tmp = path.with_extension("replace-tmp");
    fs::write(&tmp, content).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// The lines of `after` that are not in `before`, and the other way round.
fn line_diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let b: Vec<&str> = before.lines().collect();
    let a: Vec<&str> = after.lines().collect();
    let added = a
        .iter()
        .filter(|l| !b.contains(l))
        .map(|l| (*l).to_owned())
        .collect();
    let removed = b
        .iter()
        .filter(|l| !a.contains(l))
        .map(|l| (*l).to_owned())
        .collect();
    (added, removed)
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- format-preserving edits ------------------------------------------------

#[test]
fn a_change_keeps_comments_blank_lines_and_trailing_comments() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    session
        .set(SettingKey::UiTheme, text("warm"))
        .expect("warm is a theme");
    let outcome = session.save(false).unwrap();
    assert_eq!(outcome.written, vec![SettingKey::UiTheme]);
    let after = read(&path);
    let (added, removed) = line_diff(EXAMPLE, &after);
    assert_eq!(added, vec!["theme = \"warm\"".to_owned()]);
    assert_eq!(removed, vec!["theme = \"neutral\"".to_owned()]);
    assert_eq!(after.lines().count(), EXAMPLE.lines().count());
    assert_eq!(outcome.config.ui_theme, UiTheme::Warm);
    assert!(!session.has_changes());
    assert_eq!(session.saved(SettingKey::UiTheme), Some(text("warm")));

    // A trailing comment on the value's line stays.
    let (_dir, path) = config_file("[ui]\nstyle = \"dashboard\"   # dashboard | minimal\n");
    let mut session = EditSession::open(&path, home()).unwrap();
    session.set(SettingKey::UiStyle, text("minimal")).unwrap();
    session.save(false).unwrap();
    assert_eq!(
        read(&path),
        "[ui]\nstyle = \"minimal\"   # dashboard | minimal\n"
    );
}

#[test]
fn every_kind_of_setting_can_be_changed() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    let changes = [
        (SettingKey::UiStyle, text("minimal")),
        (SettingKey::DefaultAccount, text("alt")),
        (SettingKey::DefaultModel, text("my-model-1")),
        (SettingKey::DefaultEffort, text("high")),
        (SettingKey::DefaultPermissionMode, text("plan")),
        (
            SettingKey::ProjectRoots,
            SettingValue::Paths(vec!["~/src".into(), "~/work".into()]),
        ),
        (SettingKey::ExtraProjects, SettingValue::Paths(Vec::new())),
        (SettingKey::HandoffPrompt, text("please go on")),
        (SettingKey::AllowCrossGroupHandoff, SettingValue::Bool(true)),
        (SettingKey::PollIntervalSecs, SettingValue::Int(5)),
    ];
    for (key, value) in changes.clone() {
        session.set(key, value).unwrap();
    }
    assert_eq!(session.changes().len(), changes.len());
    let preview = session.validate().unwrap();
    session.save(false).unwrap();
    let after = read(&path);
    let c = Config::parse(&after, home()).unwrap();
    assert_eq!(c, preview);
    assert_eq!(c.ui_style, UiStyle::Minimal);
    assert_eq!(c.default_account, "alt");
    assert_eq!(c.default_model, "my-model-1");
    assert_eq!(c.default_effort, "high");
    assert_eq!(c.default_permission_mode, "plan");
    assert_eq!(
        c.project_roots,
        vec![
            PathBuf::from("/home/user/src"),
            PathBuf::from("/home/user/work")
        ]
    );
    assert!(c.extra_projects.is_empty());
    assert_eq!(c.handoff_prompt, "please go on");
    assert!(c.allow_cross_group_handoff);
    assert_eq!(c.poll_interval_secs, 5);
    // Only the changed values' lines differ; every comment is still there.
    let (added, removed) = line_diff(EXAMPLE, &after);
    assert_eq!(added.len(), changes.len(), "{added:?}");
    assert_eq!(removed.len(), changes.len(), "{removed:?}");
    assert!(added.iter().chain(&removed).all(|l| !l.starts_with('#')));
    for (key, value) in changes {
        assert_eq!(session.value(key), value);
        assert!(!session.is_modified(key));
    }
}

#[test]
fn a_list_spread_over_lines_stays_spread() {
    let (_dir, path) = config_file("project_roots = [\n    \"~/a\",\n]\n");
    let mut session = EditSession::open(&path, home()).unwrap();
    let paths = SettingValue::Paths(vec!["~/a".into(), "~/b".into()]);
    session.set(SettingKey::ProjectRoots, paths).unwrap();
    session.save(false).unwrap();
    assert_eq!(
        read(&path),
        "project_roots = [\n    \"~/a\",\n    \"~/b\",\n]\n"
    );
}

#[test]
fn new_keys_go_after_the_top_level_keys_and_into_ui() {
    let original = "\
# My accounts.
poll_interval_secs = 3

[[accounts]]
name = \"main\"
command = \"claude\"
";
    let (_dir, path) = config_file(original);
    let mut session = EditSession::open(&path, home()).unwrap();
    session.set(SettingKey::HandoffPrompt, text("go")).unwrap();
    session.set(SettingKey::UiTheme, text("terminal")).unwrap();
    session.save(false).unwrap();
    let after = read(&path);
    assert!(after.starts_with("# My accounts.\npoll_interval_secs = 3\nhandoff_prompt = \"go\"\n"));
    assert!(after.contains("\n[ui]\ntheme = \"terminal\"\n"), "{after}");
    let c = Config::parse(&after, home()).unwrap();
    assert_eq!(c.ui_theme, UiTheme::Terminal);
    assert_eq!(c.handoff_prompt, "go");
    assert_eq!(c.accounts.len(), 1);
}

// ---- reset to default -------------------------------------------------------

#[test]
fn reset_removes_the_key_and_keeps_the_comments_above_it() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    session.reset_to_default(SettingKey::PollIntervalSecs);
    let row = session.setting(SettingKey::PollIntervalSecs);
    assert!(row.modified);
    assert_eq!(row.source, ValueSource::Default);
    assert_eq!(row.value, SettingValue::Int(2));
    session.save(false).unwrap();
    let after = read(&path);
    assert!(!after.contains("poll_interval_secs ="), "{after}");
    // The comment stays where it was, now above the next key's comment.
    let comment = after.find("# Seconds between polls").expect("comment kept");
    let next = after.find("# Folders scanned").unwrap();
    assert!(comment < next);
    let (added, removed) = line_diff(EXAMPLE, &after);
    assert!(added.is_empty(), "{added:?}");
    assert_eq!(removed, vec!["poll_interval_secs = 2".to_owned()]);
    assert_eq!(session.saved(SettingKey::PollIntervalSecs), None);
    assert_eq!(Config::parse(&after, home()).unwrap().poll_interval_secs, 2);
}

#[test]
fn reset_of_a_tables_last_key_moves_its_comments_to_the_next_header() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    // The last key of [ui], and the last top-level key.
    session.reset_to_default(SettingKey::UiTheme);
    session.reset_to_default(SettingKey::AllowCrossGroupHandoff);
    session.save(false).unwrap();
    let after = read(&path);
    let (added, removed) = line_diff(EXAMPLE, &after);
    assert!(added.is_empty(), "{added:?}");
    assert_eq!(removed.len(), 2, "{removed:?}");
    let theme_comment = after.find("# Colour theme of the dashboard").unwrap();
    let accounts = after.find("\n[[accounts]]\n").unwrap();
    assert!(theme_comment < accounts);
    let guard_comment = after.find("# Whether `a` may hand a session off").unwrap();
    let ui = after.find("\n[ui]\n").unwrap();
    assert!(guard_comment < ui);
    assert!(ui < after.find("style = \"dashboard\"").unwrap());
    let c = Config::parse(&after, home()).unwrap();
    assert_eq!(c.ui_theme, UiTheme::Neutral);
    assert!(!c.allow_cross_group_handoff);
}

#[test]
fn reset_of_a_key_that_is_not_in_the_file_is_no_change() {
    let (_dir, path) = config_file("poll_interval_secs = 4\n");
    let mut session = EditSession::open(&path, home()).unwrap();
    session.reset_to_default(SettingKey::UiTheme);
    assert!(!session.has_changes());
    session.set(SettingKey::UiTheme, text("warm")).unwrap();
    session.reset_to_default(SettingKey::UiTheme);
    assert!(!session.has_changes());
    // Setting the file's value again is no change either.
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(9))
        .unwrap();
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(4))
        .unwrap();
    assert!(!session.has_changes());
}

// ---- validation -------------------------------------------------------------

#[test]
fn the_poll_interval_is_one_to_sixty_seconds() {
    let parse = |t: &str| Config::parse(t, home());
    assert_eq!(
        parse("poll_interval_secs = 1\n")
            .unwrap()
            .poll_interval_secs,
        1
    );
    assert_eq!(
        parse("poll_interval_secs = 60\n")
            .unwrap()
            .poll_interval_secs,
        60
    );
    for bad in ["poll_interval_secs = 0\n", "poll_interval_secs = 61\n"] {
        match parse(bad) {
            Err(ConfigError::Invalid(msg)) => {
                assert!(
                    msg.contains("poll_interval_secs must be 1 to 60 seconds"),
                    "{msg}"
                )
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
    let (_dir, path) = config_file("");
    let mut session = EditSession::open(&path, home()).unwrap();
    assert_eq!(
        session.kind(SettingKey::PollIntervalSecs),
        SettingKind::Int { min: 1, max: 60 }
    );
    for bad in [0, 61] {
        let err = session
            .set(SettingKey::PollIntervalSecs, SettingValue::Int(bad))
            .unwrap_err();
        assert_eq!(err, "must be 1 to 60 seconds");
    }
    assert!(!session.has_changes());
}

#[test]
fn bypass_permissions_is_never_offered_as_a_default() {
    let (_dir, path) = config_file("default_permission_mode = \"bypassPermissions\"\n");
    let mut session = EditSession::open(&path, home()).unwrap();
    // The parser still accepts it, and the window shows it.
    assert_eq!(
        session.value(SettingKey::DefaultPermissionMode),
        text("bypassPermissions")
    );
    let offered = session.offered(SettingKey::DefaultPermissionMode);
    assert!(
        !offered.iter().any(|m| m == "bypassPermissions"),
        "{offered:?}"
    );
    assert_eq!(
        offered,
        ["auto", "acceptEdits", "manual", "plan", "dontAsk"]
    );
    let err = session
        .set(SettingKey::DefaultPermissionMode, text("bypassPermissions"))
        .unwrap_err();
    assert!(err.contains("not offered"), "{err}");
    session
        .set(SettingKey::DefaultPermissionMode, text("auto"))
        .unwrap();
    assert!(session.has_changes());
}

#[test]
fn offered_values_and_field_checks() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    assert_eq!(
        session.offered(SettingKey::UiStyle),
        ["dashboard", "minimal"]
    );
    assert_eq!(
        session.offered(SettingKey::UiTheme),
        ["neutral", "warm", "terminal", "tokyo-night"]
    );
    assert_eq!(session.offered(SettingKey::DefaultAccount), ["main", "alt"]);
    assert_eq!(
        session.kind(SettingKey::DefaultModel),
        SettingKind::Choice {
            values: ["default", "fable", "opus", "sonnet", "haiku"]
                .map(String::from)
                .to_vec(),
            free_text: true,
        }
    );
    assert!(
        session
            .set(SettingKey::DefaultAccount, text("nope"))
            .is_err()
    );
    assert!(session.set(SettingKey::UiTheme, text("pink")).is_err());
    assert!(
        session
            .set(SettingKey::DefaultEffort, text("huge"))
            .is_err()
    );
    assert!(session.set(SettingKey::DefaultModel, text(" ")).is_err());
    assert!(
        session
            .set(SettingKey::DefaultModel, text("two words"))
            .is_err()
    );
    assert!(session.set(SettingKey::HandoffPrompt, text("  ")).is_err());
    let twice = SettingValue::Paths(vec!["~/a".into(), "~/a".into()]);
    assert!(session.set(SettingKey::ProjectRoots, twice).is_err());
    assert!(
        session
            .set(SettingKey::PollIntervalSecs, text("5"))
            .is_err()
    );
    assert!(!session.has_changes());
    // Accounts are shown read-only.
    let names: Vec<&str> = session.accounts().iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["main", "alt"]);
    // Rows: every key once, in order, with defaults marked.
    let rows = session.settings();
    assert_eq!(rows.len(), SettingKey::ALL.len());
    let style = &rows[0];
    assert_eq!(style.key, SettingKey::UiStyle);
    assert!(style.is_default());
    assert_eq!(style.source, ValueSource::File);
}

#[test]
fn an_invalid_result_writes_nothing() {
    let (dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    session
        .set(SettingKey::DefaultAccount, text("alt"))
        .unwrap();
    // Meanwhile the `alt` account was removed by hand.
    let edited = EXAMPLE.replace("name = \"alt\"", "name = \"other\"");
    replace(&path, &edited);
    let before = dir_names(dir.path());
    let err = session.save(false).unwrap_err();
    assert!(matches!(err, SaveError::Invalid(_)), "{err:?}");
    assert!(err.to_string().contains("alt"), "{err}");
    assert_eq!(read(&path), edited);
    let after: Vec<String> = dir_names(dir.path())
        .into_iter()
        .filter(|n| n != "config.toml.lock")
        .collect();
    assert_eq!(after, before, "no backup, no temp file");
    assert!(session.has_changes(), "the change stays unsaved");
}

// ---- conflicts --------------------------------------------------------------

#[test]
fn a_key_changed_on_disk_meanwhile_is_a_conflict_unless_forced() {
    let (_dir, path) = config_file(EXAMPLE);
    let mut session = EditSession::open(&path, home()).unwrap();
    session.set(SettingKey::UiTheme, text("warm")).unwrap();
    session.set(SettingKey::HandoffPrompt, text("go")).unwrap();
    // Another tower changed the theme and the poll interval.
    let other = EXAMPLE
        .replace("theme = \"neutral\"", "theme = \"terminal\"")
        .replace("poll_interval_secs = 2", "poll_interval_secs = 7");
    replace(&path, &other);
    let err = session.save(false).unwrap_err();
    let SaveError::Conflict(conflicts) = &err else {
        panic!("expected a conflict, got {err:?}");
    };
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c.key, SettingKey::UiTheme);
    assert_eq!(c.opened, Some(text("neutral")));
    assert_eq!(c.on_disk, Some(text("terminal")));
    assert_eq!(c.mine, Some(text("warm")));
    assert!(err.to_string().contains("[ui] theme"), "{err}");
    assert_eq!(read(&path), other, "nothing written");

    let outcome = session.save(true).unwrap();
    assert_eq!(outcome.written.len(), 2);
    let c = Config::parse(&read(&path), home()).unwrap();
    assert_eq!(c.ui_theme, UiTheme::Warm);
    assert_eq!(c.handoff_prompt, "go");
    // The other tower's unrelated change is kept.
    assert_eq!(c.poll_interval_secs, 7);
    assert_eq!(
        session.saved(SettingKey::PollIntervalSecs),
        Some(SettingValue::Int(7))
    );
}

#[test]
fn a_key_already_as_wanted_on_disk_is_no_conflict() {
    let (_dir, path) =
        config_file("poll_interval_secs = 4\nhandoff_prompt = \"x\"\ndefault_effort = \"low\"\n");
    let mut session = EditSession::open(&path, home()).unwrap();
    session.reset_to_default(SettingKey::PollIntervalSecs);
    session.reset_to_default(SettingKey::HandoffPrompt);
    session.set(SettingKey::DefaultEffort, text("max")).unwrap();
    // Removed on disk too, changed on disk, set on disk to the same value.
    replace(&path, "handoff_prompt = \"y\"\ndefault_effort = \"max\"\n");
    let Err(SaveError::Conflict(conflicts)) = session.save(false) else {
        panic!("expected a conflict");
    };
    let keys: Vec<SettingKey> = conflicts.iter().map(|c| c.key).collect();
    assert_eq!(keys, [SettingKey::HandoffPrompt]);
    assert_eq!(conflicts[0].on_disk, Some(text("y")));
    assert_eq!(conflicts[0].mine, None);
    session.save(true).unwrap();
    assert_eq!(read(&path), "default_effort = \"max\"\n");
    assert!(!session.has_changes());
}

// ---- backup, atomic write, permissions, symlinks, creation -----------------

#[test]
fn the_backup_is_made_once_before_the_first_write() {
    let (dir, path) = config_file(EXAMPLE);
    let bak = dir.path().join("config.toml.bak");
    let mut session = EditSession::open(&path, home()).unwrap();
    session.set(SettingKey::UiTheme, text("warm")).unwrap();
    let first = session.save(false).unwrap();
    assert_eq!(first.backup.as_deref(), Some(bak.as_path()));
    assert_eq!(read(&bak), EXAMPLE);
    session.set(SettingKey::UiTheme, text("terminal")).unwrap();
    let second = session.save(false).unwrap();
    assert_eq!(second.backup, None);
    assert_eq!(read(&bak), EXAMPLE, "never overwritten");
    // A save with nothing to save writes nothing.
    let none = session.save(false).unwrap();
    assert!(none.written.is_empty());
}

#[test]
fn the_write_is_atomic_and_keeps_the_permissions() {
    let (dir, path) = config_file(EXAMPLE);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut session = EditSession::open(&path, home()).unwrap();
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(9))
        .unwrap();
    let outcome = session.save(false).unwrap();
    assert_eq!(outcome.target, path);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let bak_mode = fs::metadata(dir.path().join("config.toml.bak"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(bak_mode, 0o600);
    assert_eq!(
        dir_names(dir.path()),
        ["config.toml", "config.toml.bak", "config.toml.lock"],
        "no temp file left"
    );
    assert_eq!(outcome.stamp, config_edit::stamp_of(&path));
}

#[test]
fn a_symlinked_config_writes_its_target_and_stays_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let dotfiles = dir.path().join("dotfiles");
    fs::create_dir(&dotfiles).unwrap();
    let target = dotfiles.join("tower.toml");
    fs::write(&target, "poll_interval_secs = 3\n").unwrap();
    let conf = dir.path().join("watchtower");
    fs::create_dir(&conf).unwrap();
    let link = conf.join("config.toml");
    std::os::unix::fs::symlink("../dotfiles/tower.toml", &link).unwrap();

    let mut session = EditSession::open(&link, home()).unwrap();
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(8))
        .unwrap();
    let outcome = session.save(false).unwrap();
    assert_eq!(outcome.target, conf.join("../dotfiles/tower.toml"));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(read(&target), "poll_interval_secs = 8\n");
    assert_eq!(dir_names(&dotfiles), ["tower.toml"], "no temp file left");
    assert_eq!(
        read(&conf.join("config.toml.bak")),
        "poll_interval_secs = 3\n"
    );
}

#[test]
fn a_missing_file_is_created_with_the_header_and_only_the_changed_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("xdg/watchtower/config.toml");
    let mut session = EditSession::open(&path, home()).unwrap();
    assert!(!session.exists());
    assert_eq!(session.accounts().len(), 1);
    assert_eq!(session.value(SettingKey::DefaultAccount), text("main"));
    assert_eq!(
        session.setting(SettingKey::UiTheme).source,
        ValueSource::Default
    );
    session.set(SettingKey::UiTheme, text("warm")).unwrap();
    session
        .set(SettingKey::PollIntervalSecs, SettingValue::Int(5))
        .unwrap();
    let outcome = session.save(false).unwrap();
    assert!(outcome.created);
    assert_eq!(outcome.backup, None);
    let written = read(&path);
    assert_eq!(
        written,
        format!("{HEADER}\npoll_interval_secs = 5\n\n[ui]\ntheme = \"warm\"\n")
    );
    assert!(HEADER.contains("config.example.toml"));
    let c = Config::parse(&written, home()).unwrap();
    assert_eq!(c.ui_theme, UiTheme::Warm);
    assert_eq!(c.poll_interval_secs, 5);
    assert!(session.exists());
    assert!(!dir.path().join("xdg/watchtower/config.toml.bak").exists());
}

#[test]
fn a_file_that_does_not_parse_cannot_be_opened() {
    let (_dir, path) = config_file("poll_interval_secs = [");
    assert!(matches!(
        EditSession::open(&path, home()),
        Err(ConfigError::Parse { .. })
    ));
}

// ---- the editor -------------------------------------------------------------

/// No terminal to hand over in a test.
struct NoTerminal;

impl Handover for NoTerminal {
    fn release(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn announce(&mut self, _line: &str) -> io::Result<()> {
        Ok(())
    }
    fn wait_for_key(&mut self, _prompt: &str) -> io::Result<()> {
        Ok(())
    }
    fn reclaim(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A fake editor: a shell script running `body` with the file as `$1`.
fn fake_editor(dir: &Path, name: &str, body: &str) -> Vec<String> {
    let script = dir.join(name);
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    vec![script.to_string_lossy().into_owned()]
}

fn edit(editor: &[String], path: &Path) -> Result<config_edit::ConfigSnapshot, String> {
    config_edit::edit_in_editor(
        &mut NoTerminal,
        &mut InheritSpawner,
        editor,
        path,
        home(),
        &Termination::default(),
    )
}

#[test]
fn the_editor_gets_a_created_file_and_its_result_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watchtower/config.toml");
    let append = fake_editor(
        dir.path(),
        "append",
        "printf 'poll_interval_secs = 7\\n' >> \"$1\"",
    );
    let snapshot = edit(&append, &path).unwrap();
    assert!(snapshot.found);
    assert_eq!(snapshot.stamp, config_edit::stamp_of(&path));
    assert_eq!(snapshot.result.unwrap().poll_interval_secs, 7);
    assert_eq!(read(&path), format!("{HEADER}poll_interval_secs = 7\n"));

    let break_it = fake_editor(
        dir.path(),
        "break",
        "printf 'poll_interval_secs = 0\\n' > \"$1\"",
    );
    let snapshot = edit(&break_it, &path).unwrap();
    assert!(matches!(snapshot.result, Err(ConfigError::Invalid(_))));

    let fail = fake_editor(dir.path(), "fail", "exit 3");
    let err = edit(&fail, &path).unwrap_err();
    assert!(err.contains("code 3"), "{err}");
}

/// `logs_follow_secs` (doc/0031): stepped through off and 5 to 30 s; any
/// value the parser accepts can be set, others are refused with the range.
#[test]
fn the_logs_follow_interval_is_off_or_five_to_thirty_seconds() {
    let (_dir, path) = config_file("");
    let mut session = EditSession::open(&path, home()).unwrap();
    let key = SettingKey::LogsFollowSecs;
    assert_eq!(key.toml_key(), "logs_follow_secs");
    assert_eq!(
        session.kind(key),
        SettingKind::Steps {
            values: vec![0, 5, 10, 15, 20, 30],
            min: 5,
            max: 30
        }
    );
    assert_eq!(session.offered(key), ["0", "5", "10", "15", "20", "30"]);
    assert_eq!(session.default_value(key), SettingValue::Int(10));
    for ok in [0, 5, 7, 30] {
        assert_eq!(session.check(key, &SettingValue::Int(ok)), Ok(()), "{ok}");
    }
    for bad in [-1, 4, 31] {
        assert_eq!(
            session.set(key, SettingValue::Int(bad)),
            Err("must be off (0) or 5 to 30 seconds".to_owned()),
            "{bad}"
        );
    }
    assert!(session.set(key, text("off")).is_err());
    session.set(key, SettingValue::Int(0)).unwrap();
    session.save(false).unwrap();
    assert_eq!(read(&path), "logs_follow_secs = 0\n");
    assert_eq!(
        Config::parse(&read(&path), home()).unwrap().logs_follow(),
        None
    );
}
