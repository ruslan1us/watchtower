//! The orchestrator session (doc/0010): an ordinary background session named
//! [`NAME`], started lazily on the first `o` in tower's own folder
//! `<config dir>/orchestrator/` and attached like any other session.
//!
//! This module holds the parts without UI state:
//!
//! - [`find`]: which listed row is the orchestrator, when several are named
//!   so (a live one first, then the most recent).
//! - [`request`]: the dispatch request: `default_account`, name
//!   `orchestrator`, permission mode `auto`, `default_model` unless it is a
//!   Haiku model (auto mode is unavailable there), `default_effort` and the
//!   short [`START_PROMPT`].
//! - [`prepare`]: creates the folder (`0700`) and writes the instruction file
//!   [`INSTRUCTIONS_FILE`] from [`TEMPLATE`] when it is missing; an existing
//!   file is never touched, so the user's edits stay.
//! - [`start`]: [`prepare`], then [`dispatch::run`]; the worker thread of
//!   [`crate::jobs`] calls it.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::cli::CliRunner;
use crate::config::Config;
use crate::dispatch::{self, DispatchError, DispatchRequest, Dispatched};
use crate::listing::Session;
use crate::merge::MergedSession;

/// The orchestrator's session name; `o` looks for a background row with
/// exactly this name.
pub const NAME: &str = "orchestrator";

/// The folder under tower's config dir the orchestrator runs in.
pub const DIR_NAME: &str = "orchestrator";

/// The instruction file in that folder.
pub const INSTRUCTIONS_FILE: &str = "CLAUDE.md";

/// The opening prompt of a new orchestrator: one short turn.
pub const START_PROMPT: &str = "You are the Watchtower orchestrator. Read CLAUDE.md in this folder, then wait for my questions.";

/// The permission mode: never bypass, which would hold incoming messages for
/// approval and is refused as root (doc/0010).
pub const PERMISSION_MODE: &str = "auto";

/// The instruction file's template; [`TOWER_PLACEHOLDER`] becomes the path
/// of the running `tower` binary.
pub const TEMPLATE: &str = include_str!("../assets/orchestrator/CLAUDE.md");

/// Where the template names the `tower` binary.
pub const TOWER_PLACEHOLDER: &str = "{{TOWER}}";

/// `<config dir>/orchestrator`.
pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join(DIR_NAME)
}

/// Whether the row is an orchestrator: a background row named exactly
/// [`NAME`]. The overview gives these rows their own group and never
/// highlights them (doc/0006, doc/0007).
pub fn is_orchestrator(s: &Session) -> bool {
    !s.is_interactive() && s.name.as_deref() == Some(NAME)
}

/// How `o` ranks several orchestrators, higher first: one with a live
/// process, then the most recently started. Rows with equal ranks keep
/// their display order.
pub fn rank(s: &Session) -> (bool, i64) {
    (s.has_live_process(), s.started_at.unwrap_or(i64::MIN))
}

/// The row `o` attaches to: background rows named exactly [`NAME`]
/// ([`is_orchestrator`]); with several, the first by [`rank`].
/// Returns its index into `sessions` and how many rows carry the name.
pub fn find(sessions: &[MergedSession]) -> Option<(usize, usize)> {
    let candidates: Vec<usize> = sessions
        .iter()
        .enumerate()
        .filter(|(_, m)| is_orchestrator(&m.session))
        .map(|(i, _)| i)
        .collect();
    let best = candidates.iter().copied().max_by_key(|&i| {
        // `max_by_key` keeps the last of equal keys; the reversed index
        // makes it keep the first in display order instead.
        (rank(&sessions[i].session), std::cmp::Reverse(i))
    })?;
    Some((best, candidates.len()))
}

/// True for any Haiku model name (alias or full name), case-insensitive:
/// the orchestrator needs auto mode, which Haiku 4.5 does not offer, so it
/// never runs on Haiku (doc/0010).
pub fn is_haiku(model: &str) -> bool {
    model.to_lowercase().contains("haiku")
}

/// The `--model` value for the orchestrator: `default_model`, or `None`
/// (no `--model`, Claude Code's default) when that is a Haiku model.
pub fn model(default_model: &str) -> Option<String> {
    (!is_haiku(default_model)).then(|| default_model.to_owned())
}

/// The dispatch request of a new orchestrator in `dir` (doc/0010).
pub fn request(config: &Config, dir: PathBuf) -> DispatchRequest {
    let mut request = DispatchRequest::new(dir, START_PROMPT, config.default_account().clone());
    request.name = Some(NAME.to_owned());
    request.model = model(&config.default_model);
    request.effort = Some(config.default_effort.clone());
    request.permission_mode = PERMISSION_MODE.to_owned();
    request
}

/// The instruction file's text with `tower` as the binary.
pub fn instructions(tower: &Path) -> String {
    TEMPLATE.replace(TOWER_PLACEHOLDER, &shell_word(&tower.to_string_lossy()))
}

/// `word` as one shell word: unchanged when it holds only safe characters,
/// else in single quotes.
fn shell_word(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c));
    if safe {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Creates `dir` with mode `0700` when it is missing (parents too) and
/// writes [`INSTRUCTIONS_FILE`] there (mode `0600`) when it is missing. An
/// existing file, including a symlink or one the user edited, is never
/// overwritten: the file is only ever created new. A write that fails
/// halfway removes the partial file, so the next start writes it again.
/// Returns true when the file was written.
pub fn prepare(dir: &Path, tower: &Path) -> io::Result<bool> {
    if !dir.is_dir() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let file = dir.join(INSTRUCTIONS_FILE);
    let mut f = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = f.write_all(instructions(tower).as_bytes()) {
        drop(f);
        let _ = fs::remove_file(&file);
        return Err(e);
    }
    Ok(true)
}

/// Why the orchestrator did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The folder or the instruction file could not be created.
    #[error("cannot prepare {}: {message}", dir.display())]
    Folder {
        /// The orchestrator folder.
        dir: PathBuf,
        /// The I/O error.
        message: String,
    },
    /// `claude --bg` failed; [`DispatchError::Untrusted`] offers the trust
    /// flow.
    #[error(transparent)]
    Dispatch(#[from] DispatchError),
}

/// Prepares the folder of `request` ([`prepare`], with `tower` as the
/// binary named in the instructions) and dispatches it. Blocks; the UI calls
/// it on a worker thread.
pub fn start(
    runner: &CliRunner,
    request: &DispatchRequest,
    tower: &Path,
) -> Result<Dispatched, StartError> {
    prepare(&request.project, tower).map_err(|e| StartError::Folder {
        dir: request.project.clone(),
        message: e.to_string(),
    })?;
    Ok(dispatch::run(runner, request)?)
}

/// The path of the running `tower` binary for the instructions, `tower`
/// when it cannot be determined.
pub fn current_tower() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tower"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words() {
        assert_eq!(shell_word("/usr/local/bin/tower"), "/usr/local/bin/tower");
        assert_eq!(shell_word("/opt/my tools/tower"), "'/opt/my tools/tower'");
        assert_eq!(shell_word("/a'b/tower"), r"'/a'\''b/tower'");
    }

    #[test]
    fn haiku_names() {
        assert!(is_haiku("haiku"));
        assert!(is_haiku("claude-HAIKU-4-5"));
        assert!(!is_haiku("sonnet"));
        assert_eq!(model("haiku"), None);
        assert_eq!(model("opus").as_deref(), Some("opus"));
    }
}
