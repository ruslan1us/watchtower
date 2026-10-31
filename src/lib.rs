//! Watchtower core library: everything `tower` knows about Claude Code
//! background sessions, without any UI.
//!
//! The pieces, in the order a poll uses them:
//!
//! - [`config`]: `config.toml` and the configured accounts (doc/0002, doc/0003).
//! - [`config_edit`]: the Settings window's edits of `config.toml`, its save,
//!   reload check and editor (doc/0022).
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
//! - [`ptysize`]: the real terminal size of a session, for that replay
//!   (doc/0025).
//! - [`logstream`]: one session's logs stitched across captures, so the
//!   screen keeps what the last full repaint drew (doc/0030).
//!
//! Actions (milestone 2, no UI):
//!
//! - [`projects`]: candidate projects for the new-session dialog (doc/0008).
//! - [`dispatch`]: `claude --bg` from the dialog's fields, the dialog's
//!   option lists and the trust flow command (doc/0008).
//! - [`actions`]: `stop` and `rm` of a session (doc/0009).
//! - [`handoff`]: continuing a session under another account (milestone 3,
//!   doc/0004).
//! - [`rename`]: renaming a stopped session with `/rename` without
//!   attaching (doc/0027).
//! - [`reply`]: the quick reply, a text sent to an idle or stopped session
//!   as its next prompt (doc/0040).
//!
//! The TUI (milestones 1–5, doc/0014):
//!
//! - [`app`]: the state machine (list, grouping, filter, selection,
//!   overlays, logs panel, stop, remove and hand-off, keys); no I/O.
//! - [`dialog`]: the new-session dialog's fields and keys (doc/0008); no I/O.
//! - [`settings`]: the Settings window's state and keys (doc/0024); its
//!   folder checks run as jobs.
//! - [`welcome`]: the steps of the welcome window on top of it (doc/0048).
//! - [`layout`]: command keys typed under a Cyrillic layout (doc/0011).
//! - [`ui`]: renders [`app::App`] with ratatui.
//! - [`poller`]: polls every account on worker threads (doc/0001).
//! - [`jobs`]: runs dispatch, stop, remove, hand-off, rename and the quick
//!   reply on worker threads.
//! - [`logfollow`]: fetches `claude logs` for the panel and follows working
//!   sessions in the background on its own thread (doc/0030).
//! - [`attach`]: hands the terminal to `claude attach` and back (doc/0005);
//!   also used for the editor and the trust flow.
//! - [`embed`]: the workspace style's `claude attach` in a pseudo-terminal,
//!   drawn inside tower (doc/0045).
//! - [`editor`]: the task in `$VISUAL` / `$EDITOR` (doc/0008).
//! - [`templates`]: prompt templates from `<config dir>/templates` and the
//!   reusable picker that inserts one (doc/0036).
//! - [`commands`]: the slash-command suggestions of the task field: the
//!   account's, the project's and the enabled plugins' skills and commands,
//!   read on a worker thread, and the menu that completes them (doc/0042).
//! - [`shell`]: `!`, a shell in the selected session's folder (doc/0037).
//! - [`gitinfo`]: the details panel's `git` line, read with `git status`
//!   on worker threads and cached per folder (doc/0037).
//! - [`usage`]: each account's plan usage, opt-in and informational: the
//!   `/usage` probe, its parser, hiding the probe's own row and the
//!   schedule (doc/0043).
//! - [`tui`]: the terminal runtime and event loop.
//! - [`args`]: command-line arguments.
//! - [`status`]: `tower status [--json]`, one merged snapshot without the
//!   TUI (doc/0010).
//! - [`control`]: `tower dispatch` and `tower wake`, starting and waking
//!   sessions from the command line (doc/0044).
//! - [`wake`]: finding, checking and waking a session without a live
//!   process for `tower wake` (doc/0044).
//! - [`subagents`]: the observe-only hooks passed to sessions tower starts,
//!   `tower hook`, and the per-session subagent files it keeps (doc/0029).

#[macro_use]
mod strenum;

pub mod actions;
pub mod activity;
pub mod app;
pub mod args;
pub mod attach;
pub mod cli;
pub mod commands;
pub mod config;
pub mod config_edit;
pub mod control;
pub mod dialog;
pub mod dispatch;
pub mod editor;
pub mod embed;
pub mod gitinfo;
pub mod groups;
pub mod handoff;
pub mod jobs;
pub mod layout;
pub mod listing;
pub mod logfollow;
pub mod logstream;
pub mod logview;
pub mod merge;
pub mod orchestrator;
pub mod paths;
pub mod poller;
pub mod projects;
pub mod ptysize;
pub mod rename;
pub mod reply;
pub mod settings;
pub mod shell;
pub mod sidecar;
pub mod status;
pub mod subagents;
pub mod tabtitle;
pub mod templates;
pub mod tui;
pub mod ui;
pub mod update;
pub mod usage;
pub mod wake;
pub mod welcome;
