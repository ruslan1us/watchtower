//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::PathBuf;

use watchtower::listing::{Listing, Session, parse_listing};

/// Path of a file under `tests/fixtures/`.
pub fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// Reads a fixture as text.
pub fn fixture(rel: &str) -> String {
    std::fs::read_to_string(fixture_path(rel)).expect("fixture exists")
}

/// Parses an `agents-json/` fixture and asserts it produced no warnings.
pub fn listing(name: &str) -> Listing {
    let listing = parse_listing(&fixture(&format!("agents-json/{name}"))).expect("parses");
    assert!(
        listing.warnings.is_empty(),
        "{name}: {:?}",
        listing.warnings
    );
    listing
}

/// Finds a row by name.
pub fn by_name<'a>(sessions: &'a [Session], name: &str) -> &'a Session {
    sessions
        .iter()
        .find(|s| s.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no row named {name}"))
}

// ---- TUI helpers -------------------------------------------------------

use std::ffi::OsString;
use std::path::Path;

use watchtower::app::App;
use watchtower::config::Config;
use watchtower::merge::EnvironReader;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;

/// The fixture home: fixture rows live under `/home/user/…`.
pub const HOME: &str = "/home/user";

/// An environ reader that can never read a process (as for another user's
/// process), so ownership falls through to rules 2–4.
pub struct NoEnviron;

impl EnvironReader for NoEnviron {
    fn config_dir_var(&self, _pid: u32) -> std::io::Result<Option<OsString>> {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    }

    fn cmdline(&self, _pid: u32) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    }

    fn exe(&self, _pid: u32) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    }

    fn start_ticks(&self, _pid: u32) -> std::io::Result<u64> {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    }
}

/// An app with the built-in single account `main`.
pub fn app_single() -> App {
    let home = Path::new(HOME);
    App::new(
        Config::defaults(home),
        home.into(),
        Sidecar::default(),
        Box::new(NoEnviron),
    )
}

/// An app with accounts `main` (default) and `alt`.
pub fn app_two() -> App {
    app_two_with(Sidecar::default())
}

/// [`app_two`] with a given sidecar.
pub fn app_two_with(sidecar: Sidecar) -> App {
    let home = Path::new(HOME);
    let config = Config::parse(
        r#"
default_account = "main"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#,
        home,
    )
    .unwrap();
    App::new(config, home.into(), sidecar, Box::new(NoEnviron))
}

/// A successful poll of `account` returning an `agents-json/` fixture.
pub fn poll(account: &str, fixture_name: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(listing(fixture_name)),
    }
}

/// A failed poll of `account`.
pub fn poll_err(account: &str, err: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Err(err.into()),
    }
}

// ---- milestone 2 helpers ------------------------------------------------

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::projects::{self, ProjectList};

/// Creates git clones `names` (folders holding `.git`) under `root` and
/// discovers them like the dialog does (doc/0008).
pub fn project_list(root: &Path, names: &[&str]) -> ProjectList {
    for name in names {
        std::fs::create_dir_all(root.join(name).join(".git")).unwrap();
    }
    projects::discover(&[root.to_path_buf()], &[], &[], Path::new(HOME))
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

/// Types `text` key by key.
pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        let effects = app.handle_key(ch(c));
        assert!(effects.is_empty(), "typing {c:?} gave {effects:?}");
    }
}

/// The status line as one string.
pub fn status_text(app: &App) -> String {
    app.status_segments(std::time::Instant::now())
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join(" | ")
}

// ---- refresh before attach (doc/0005) -------------------------------------

use watchtower::app::{Effect, SidecarJob};

/// The cycle the fake runtime gives the poll an attach asks for.
pub const ATTACH_CYCLE: u64 = 1_000;

/// A sidecar path that does not exist: a check finds no file.
pub const NO_STATE_FILE: &str = "/nonexistent/watchtower-test/state.json";

/// Plays the runtime for an attach's refresh (doc/0005): when `effects`
/// ask for it, starts the poll (cycle [`ATTACH_CYCLE`]), applies the fresh
/// polls `(account, fixture)` and runs the sidecar check against
/// [`NO_STATE_FILE`]. Returns the other effects plus those that followed,
/// e.g. the attach. Without a refresh request, returns `effects`.
pub fn refresh_for_attach(
    app: &mut App,
    effects: Vec<Effect>,
    polls: &[(&str, &str)],
) -> Vec<Effect> {
    if !effects.contains(&Effect::PollForAttach) {
        return effects;
    }
    let mut out: Vec<Effect> = effects
        .into_iter()
        .filter(|e| !matches!(e, Effect::PollForAttach | Effect::CheckSidecar))
        .collect();
    app.attach_poll_started(ATTACH_CYCLE);
    for (account, fixture) in polls {
        let mut update = poll(account, fixture);
        update.cycle = ATTACH_CYCLE;
        out.extend(app.apply_poll(update));
    }
    out.extend(run_sidecar_check(app, Path::new(NO_STATE_FILE)));
    out
}

/// Runs a sidecar check against `path` on this thread, as the runtime's
/// worker would, and returns the app's effects.
pub fn run_sidecar_check(app: &mut App, path: &Path) -> Vec<Effect> {
    let Some(SidecarJob::Check { seq, job }) = app.start_sidecar_check() else {
        panic!("a sidecar job is already running");
    };
    app.sidecar_checked(seq, job.run(path))
}

// ---- list controls (doc/0006) ---------------------------------------------

use watchtower::app::Entry;
use watchtower::merge::MergedSession;

/// Selects the first row for which `pred` holds, walking the list from the
/// top with `j` and expanding collapsed groups (`Tab` on their header) on
/// the way, as a user would. Returns false at the bottom without a match.
pub fn select_where(app: &mut App, pred: impl Fn(&MergedSession) -> bool) -> bool {
    app.handle_key(key(KeyCode::Home));
    loop {
        if app.selected_session().is_some_and(&pred) {
            return true;
        }
        if let Some(e) = app.selected_entry()
            && let Entry::Header {
                collapsed: true, ..
            } = app.entries()[e]
        {
            app.handle_key(key(KeyCode::Tab));
        }
        let before = app.selected_entry();
        app.handle_key(ch('j'));
        if app.selected_entry() == before {
            return false;
        }
    }
}

/// Selects the row whose display name is `name` (see [`select_where`]).
pub fn select_name(app: &mut App, name: &str) {
    if !select_where(app, |m| m.session.display_name() == name) {
        panic!("no row {name}");
    }
}

/// The group headers as (title, count, collapsed).
pub fn headers(app: &App) -> Vec<(String, usize, bool)> {
    app.entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Header {
                group,
                count,
                collapsed,
                ..
            } => Some((group.title().to_owned(), *count, *collapsed)),
            Entry::Row(_) => None,
        })
        .collect()
}

/// The display names of the visible rows, in order.
pub fn visible_rows(app: &App) -> Vec<String> {
    app.entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Row(i) => Some(app.sessions()[*i].session.display_name()),
            Entry::Header { .. } => None,
        })
        .collect()
}
