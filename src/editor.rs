//! Editing the new-session task in the user's editor (`Ctrl+G`, doc/0008).
//!
//! The editor is `$VISUAL`, else `$EDITOR`, split like a shell would (so
//! `code --wait` works) but never run through a shell. The task is written to
//! a fresh temporary file (mode 0600), the editor runs in the foreground
//! exactly like an attach ([`attach::run_foreground`]: terminal released,
//! no-op signal handlers, terminal reclaimed), and the saved text is read
//! back. The file is removed afterwards.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::attach::{self, Foreground, Handover, Spawner, Termination};
use crate::cli::CommandLine;

/// The editor command from the values of `VISUAL` and `EDITOR`: the first
/// one that is set and not blank, split into words. An error message when
/// neither is usable.
pub fn editor_command(
    visual: Option<OsString>,
    editor: Option<OsString>,
) -> Result<Vec<String>, String> {
    for (var, value) in [("VISUAL", visual), ("EDITOR", editor)] {
        let Some(value) = value else { continue };
        let text = value.to_string_lossy();
        if text.trim().is_empty() {
            continue;
        }
        return match shlex::split(&text) {
            Some(words) if !words.is_empty() => Ok(words),
            _ => Err(format!("${var} (`{text}`) is badly quoted")),
        };
    }
    Err("set $VISUAL or $EDITOR to edit the task in an editor".to_owned())
}

/// [`editor_command`] from tower's own environment.
pub fn editor_from_env() -> Result<Vec<String>, String> {
    editor_command(std::env::var_os("VISUAL"), std::env::var_os("EDITOR"))
}

/// The text read back from the editor: one trailing line break, which most
/// editors add on save, is dropped.
pub fn read_back(text: &str) -> String {
    let t = text.strip_suffix('\n').unwrap_or(text);
    t.strip_suffix('\r').unwrap_or(t).to_owned()
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Creates a new private temporary file in `dir` holding `text`.
fn temp_file(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    loop {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = dir.join(format!("tower-task-{}-{n}.md", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                file.sync_all()?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

/// Opens `text` in `editor` (program and leading arguments) and returns the
/// saved text. `temp_dir` holds the temporary file. An error message when
/// the file cannot be written or read, the editor cannot run, or it exits
/// non-zero (the task is then left unchanged).
pub fn edit(
    handover: &mut dyn Handover,
    spawner: &mut dyn Spawner,
    editor: &[String],
    text: &str,
    temp_dir: &Path,
    termination: &Termination,
) -> Result<String, String> {
    let Some((program, leading)) = editor.split_first() else {
        return Err("no editor command".to_owned());
    };
    let path = temp_file(temp_dir, text).map_err(|e| {
        format!(
            "cannot write a temporary file in {}: {e}",
            temp_dir.display()
        )
    })?;
    let mut args = leading.to_vec();
    args.push(path.to_string_lossy().into_owned());
    let line = CommandLine {
        program: program.clone(),
        args,
        cwd: None,
    };
    let fg = Foreground {
        line: &line,
        announce: Some(format!("tower: editing the task with {program}…")),
        label: "the editor",
        pause_on_failure: true,
    };
    let result = attach::run_foreground(handover, spawner, &fg, termination);
    let out = match result {
        Ok(Some(0)) => std::fs::read(&path)
            .map(|b| read_back(&String::from_utf8_lossy(&b)))
            .map_err(|e| format!("cannot read {}: {e}", path.display())),
        Ok(Some(code)) => Err(format!(
            "the editor exited with code {code}; the task is unchanged"
        )),
        Ok(None) => Err("the editor was ended by a signal; the task is unchanged".to_owned()),
        Err(err) => Err(err),
    };
    let _ = std::fs::remove_file(&path);
    out
}
