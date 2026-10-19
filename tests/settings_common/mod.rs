//! Helpers for the Settings window's tests (doc/0024): a temp home with a
//! config under `~/.config/watchtower/`, an app over it, and a stand-in
//! for the runtime that runs the window's effects on this thread.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;
use watchtower::app::{App, Effect};
use watchtower::config::{Config, ConfigSource};
use watchtower::config_edit::EditSession;
use watchtower::sidecar::Sidecar;

use super::common::NoEnviron;

/// A config like the mock's: two accounts in two groups, two project
/// roots (one missing), one extra project, the style and theme set.
pub const CONFIG: &str = r#"# Watchtower config for the Settings tests.
default_account = "main"

# Folders scanned for git clones.
project_roots = ["~/workspace", "~/old-projects"]
extra_projects = ["~/notes"]   # listed as they are

[ui]
style = "dashboard"   # dashboard | minimal
theme = "neutral"

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal"
"#;

/// A temp home and the config path tower uses in it.
pub struct Env {
    pub dir: TempDir,
    pub home: PathBuf,
    pub path: PathBuf,
}

/// A temp home with `content` as its config (`None`: no file), and the
/// folders the config names: `~/workspace` with three git clones and
/// `~/notes`; `~/old-projects` does not exist.
pub fn env(content: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let path = home.join(".config/watchtower/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if let Some(content) = content {
        fs::write(&path, content).unwrap();
    }
    for name in ["api", "web", "docs"] {
        fs::create_dir_all(home.join("workspace").join(name).join(".git")).unwrap();
    }
    fs::create_dir_all(home.join("workspace/scratch")).unwrap();
    fs::create_dir_all(home.join("notes")).unwrap();
    Env { dir, home, path }
}

/// An app started from the config of `env`, as the runtime starts it.
pub fn app_for(env: &Env) -> App {
    let config = if env.path.exists() {
        Config::load_from(&env.path, &env.home).unwrap()
    } else {
        Config::defaults(&env.home)
    };
    let mut app = App::new(
        config.clone(),
        env.home.clone(),
        Sidecar::default(),
        Box::new(NoEnviron),
    );
    app.style = config.ui_style;
    app.set_config_source(ConfigSource::probe(&env.path));
    app
}

/// A key without modifiers.
pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A character key.
pub fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

/// `Ctrl+<c>`.
pub fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Plays the runtime for the window's effects on this thread: opens the
/// window, runs saves and folder jobs, and feeds their results back.
/// Returns the effects it did not run.
pub fn run(app: &mut App, effects: Vec<Effect>) -> Vec<Effect> {
    let mut left = Vec::new();
    let mut queue: std::collections::VecDeque<Effect> = effects.into();
    while let Some(effect) = queue.pop_front() {
        match effect {
            Effect::OpenSettings(path) => {
                let result = EditSession::open(&path, app.home());
                queue.extend(app.settings_opened(result));
            }
            Effect::SaveConfig(job) => queue.extend(app.config_saved(job.run())),
            Effect::CheckFolders(job) => queue.extend(app.folders_checked(job.run())),
            other => left.push(other),
        }
    }
    left
}

/// Presses `key` and runs what follows ([`run`]).
pub fn press(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let effects = app.handle_key(key);
    run(app, effects)
}

/// Types `text` key by key.
pub fn type_in(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, ch(c));
    }
}

/// Opens the Settings window with `c`.
pub fn open(app: &mut App) {
    let effects = press(app, ch('c'));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(app.settings().is_some(), "the window did not open");
}

/// Renders the screen at `w` × `h`.
pub fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), 1_800_000_000_000))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// The rendered rows as text, right-trimmed.
pub fn lines(buf: &Buffer) -> Vec<String> {
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

/// The screen as text.
pub fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    lines(&render(app, w, h))
}

/// The row holding `needle`.
pub fn line_with<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in\n{}", lines.join("\n")))
}

/// Moves the selection to the row labelled `label`.
pub fn select(app: &mut App, label: &str) {
    press(app, key(KeyCode::Home));
    for _ in 0..30 {
        let window = app.settings().unwrap();
        if let Some(key) = window.selected_key()
            && key.label() == label
        {
            return;
        }
        press(app, key(KeyCode::Down));
    }
    panic!("no row {label}");
}

/// The file's text now.
pub fn file(env: &Env) -> String {
    fs::read_to_string(&env.path).unwrap()
}

/// `path` relative to the temp home, for messages.
pub fn rel(env: &Env, path: &Path) -> String {
    path.strip_prefix(&env.home)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
