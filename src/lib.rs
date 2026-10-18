//! Watchtower core library: everything `tower` knows about Claude Code
//! background sessions, without any UI.
//!
//! The pieces, in the order a poll uses them:
//!
//! - [`config`]: `config.toml` and the configured accounts (doc/0002, doc/0003).
//! - [`cli`]: runs `<account command> …` with a timeout; `agents --json --all`
//!   and the `attach` command line (doc/0001, doc/0005).
//! - [`listing`]: the lenient parser and model of the JSON listing (doc/0001,
//!   reference/claude-code-cli.md). The only module that knows the JSON shape.
//! - [`merge`]: merges the listings of all accounts and resolves which account
//!   owns each session (doc/0003).
//! - [`sidecar`]: tower's own `state.json` (doc/0002).
//! - [`groups`]: maps sessions to the overview groups and detail labels
//!   (doc/0006).
//! - [`paths`]: home and XDG directories, `~` expansion.
//!
//! The TUI (milestone 1, doc/0014):
//!
//! - [`app`]: the state machine (list, selection, overlays, keys); no I/O.
//! - [`ui`]: renders [`app::App`] with ratatui.
//! - [`poller`]: polls every account on worker threads (doc/0001).
//! - [`attach`]: hands the terminal to `claude attach` and back (doc/0005).
//! - [`tui`]: the terminal runtime and event loop.
//! - [`args`]: command-line arguments.

#[macro_use]
mod strenum;

pub mod app;
pub mod args;
pub mod attach;
pub mod cli;
pub mod config;
pub mod groups;
pub mod listing;
pub mod merge;
pub mod paths;
pub mod poller;
pub mod sidecar;
pub mod tui;
pub mod ui;
