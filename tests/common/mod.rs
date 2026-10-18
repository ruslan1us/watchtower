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
        result: Ok(listing(fixture_name)),
    }
}

/// A failed poll of `account`.
pub fn poll_err(account: &str, err: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        result: Err(err.into()),
    }
}
