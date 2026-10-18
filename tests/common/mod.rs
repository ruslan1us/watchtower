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
