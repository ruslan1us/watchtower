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
//!   [`INSTRUCTIONS_FILE`] from [`TEMPLATE`] when it is missing or still an
//!   unedited version tower shipped (doc/0044); a file the user edited and a
//!   symlink are never touched. [`refresh`] does the same at tower's start
//!   for a folder that exists.
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
pub fn shell_word(word: &str) -> String {
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

/// SHA-256 of every template tower has shipped in
/// `assets/orchestrator/CLAUDE.md`, as written in the repository (with
/// [`TOWER_PLACEHOLDER`]), oldest first. An instruction file that is one of
/// them once the tower path is put back as the placeholder was written by
/// tower and never edited, so it may be replaced by the current template
/// (doc/0044). A change to the template must add its hash here; a test
/// checks that [`TEMPLATE`] is listed.
pub const SHIPPED_TEMPLATES: [&str; 4] = [
    // 2026-10-18, the first template.
    "099754bfc1f4e682e462f8a791299f13b726875a2c2f0c0b101195b7ce91d87b",
    // 2026-10-19, the orchestrator group.
    "9b3f1ea087731c3704a26e5c1edb4d4f75f02046fe0279a2a00d5f5da67ebfd1",
    // 2026-10-19, subagents in `status --json`.
    "604411db52edc5f2bb06d7fb0cda1f81996b4b6410358d1e198e7e0cc6a5ca4b",
    // 2026-10-28, `tower dispatch` and `tower wake` (doc/0044).
    "85f1d5abe584df6b08132956144566c22e77947f765ca261c748d5a4f308545c",
];

/// What [`prepare`] or [`refresh`] did with the instruction file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instructions {
    /// Written: it was missing, or an unedited older version tower wrote.
    Written,
    /// Already the current text; nothing written. Also a missing folder
    /// for [`refresh`].
    Current,
    /// The user edited it (it matches no version tower shipped): left as
    /// it is (doc/0010). The TUI says so once (doc/0044).
    Edited,
    /// A symlink in its place: left alone.
    Linked,
}

/// SHA-256 of `text`, lowercase hex.
fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `text` is a file tower wrote from one of [`SHIPPED_TEMPLATES`],
/// unedited. The tower path is the word in its first ``Run `<tower> status
/// --json` ``, the line every version starts its instructions with; it is
/// put back as the placeholder everywhere it follows a backtick (the
/// current form), or only in that line (the older versions, which name the
/// binary only there; this also covers an older file written with the bare
/// fallback `tower`, a word the template uses elsewhere too).
pub fn is_shipped(text: &str) -> bool {
    const BEFORE: &str = "Run `";
    const AFTER: &str = " status --json`";
    let Some(start) = text.find(BEFORE).map(|i| i + BEFORE.len()) else {
        return false;
    };
    let Some(len) = text[start..].find(AFTER) else {
        return false;
    };
    let word = &text[start..start + len];
    if word.is_empty() || word.contains('\n') {
        return false;
    }
    let everywhere = text.replace(&format!("`{word} "), &format!("`{TOWER_PLACEHOLDER} "));
    let once = format!(
        "{}{TOWER_PLACEHOLDER}{}",
        &text[..start],
        &text[start + len..]
    );
    [everywhere, once]
        .iter()
        .any(|t| SHIPPED_TEMPLATES.contains(&sha256_hex(t).as_str()))
}

/// Creates `dir` with mode `0700` when it is missing (parents too) and
/// writes [`INSTRUCTIONS_FILE`] there (mode `0600`) from [`instructions`]
/// for `tower` when it is missing, or when it is an unedited version tower
/// wrote ([`is_shipped`]) that differs from the current one, so an older
/// file gets the current rules (doc/0044). A file the user edited is never
/// overwritten ([`Instructions::Edited`], doc/0010), nor is a symlink in its
/// place. The write is atomic (a temp file in the folder, fsync, rename),
/// so a failure leaves the old file.
pub fn prepare(dir: &Path, tower: &Path) -> io::Result<Instructions> {
    if !dir.is_dir() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    write_instructions(dir, tower)
}

/// [`prepare`] for a folder that already exists, run at tower's start so an
/// orchestrator that is listed (or stopped) reads the current rules on its
/// next start or when told to re-read the file (doc/0044). A missing folder
/// is left missing: the orchestrator is created only on `o` (doc/0010).
pub fn refresh(dir: &Path, tower: &Path) -> io::Result<Instructions> {
    if !dir.is_dir() {
        return Ok(Instructions::Current);
    }
    write_instructions(dir, tower)
}

/// The status-line note for an edited instruction file at `file`
/// (doc/0044).
pub fn edited_note(file: &Path) -> String {
    format!(
        "orchestrator instructions were edited; the new dispatch and wake rules aren't in \
         them; delete {} to get tower's version",
        file.display()
    )
}

fn write_instructions(dir: &Path, tower: &Path) -> io::Result<Instructions> {
    let file = dir.join(INSTRUCTIONS_FILE);
    let text = instructions(tower);
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.file_type().is_symlink() => return Ok(Instructions::Linked),
        Ok(meta) if !meta.is_file() => {
            return Err(io::Error::other(format!(
                "{} is not a file",
                file.display()
            )));
        }
        Ok(_) => {
            let old = fs::read(&file)?;
            if old == text.as_bytes() {
                return Ok(Instructions::Current);
            }
            let shipped = std::str::from_utf8(&old).is_ok_and(is_shipped);
            if !shipped {
                return Ok(Instructions::Edited);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let tmp = dir.join(format!(".{INSTRUCTIONS_FILE}.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let written = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &file)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(Instructions::Written)
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

/// The path of the running `tower` binary for the instructions (a
/// ` (deleted)` suffix after `tower update` dropped, as for the hooks,
/// doc/0029), `tower` when it cannot be determined.
pub fn current_tower() -> PathBuf {
    crate::subagents::running_exe().unwrap_or_else(|_| PathBuf::from("tower"))
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
