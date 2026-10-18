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
//! - [`activity`]: tracked activity times and the attention highlight
//!   (doc/0006, doc/0007).
//! - [`paths`]: home and XDG directories, `~` expansion.
//! - [`logview`]: `claude logs` fetched and replayed into a screen for the
//!   logs panel (doc/0009).
//!
//! Actions (milestone 2, no UI):
//!
//! - [`projects`]: candidate projects for the new-session dialog (doc/0008).
//! - [`dispatch`]: `claude --bg` from the dialog's fields, the dialog's
//!   option lists and the trust flow command (doc/0008).
//! - [`actions`]: `stop` and `rm` of a session (doc/0009).
//! - [`handoff`]: continuing a session under another account (milestone 3,
//!   doc/0004).
//!
//! The TUI (milestones 1–5, doc/0014):
//!
//! - [`app`]: the state machine (list, grouping, filter, selection,
//!   overlays, logs panel, stop, remove and hand-off, keys); no I/O.
//! - [`dialog`]: the new-session dialog's fields and keys (doc/0008); no I/O.
//! - [`ui`]: renders [`app::App`] with ratatui.
//! - [`poller`]: polls every account on worker threads (doc/0001).
//! - [`jobs`]: runs dispatch, stop, remove, hand-off and logs on worker
//!   threads.
//! - [`attach`]: hands the terminal to `claude attach` and back (doc/0005);
//!   also used for the editor and the trust flow.
//! - [`editor`]: the task in `$VISUAL` / `$EDITOR` (doc/0008).
//! - [`tui`]: the terminal runtime and event loop.
//! - [`args`]: command-line arguments.
//! - [`status`]: `tower status [--json]`, one merged snapshot without the
//!   TUI (doc/0010).

#[macro_use]
mod strenum;

pub mod actions;
pub mod activity;
pub mod app;
pub mod args;
pub mod attach;
pub mod cli;
pub mod config;
pub mod dialog;
pub mod dispatch;
pub mod editor;
pub mod groups;
pub mod handoff;
pub mod jobs;
pub mod listing;
pub mod logview;
pub mod merge;
pub mod orchestrator;
pub mod paths;
pub mod poller;
pub mod projects;
pub mod sidecar;
pub mod status;
pub mod tui;
pub mod ui;
