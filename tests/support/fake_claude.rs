//! A fake `claude` executable for action tests (doc/0015).
//!
//! The script records its arguments (NUL-separated, so multi-line arguments
//! survive), its physical working directory and one line per invocation,
//! then runs a shell snippet that prints fixture output.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use watchtower::config::Account;

/// Serializes the tests of one test binary: writing an executable while
/// another thread forks can make the exec fail with ETXTBSY.
static LOCK: Mutex<()> = Mutex::new(());

/// Takes the per-binary lock.
pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A temp dir holding the fake executable and its records.
pub struct Fake {
    pub dir: tempfile::TempDir,
    name: String,
}

impl Fake {
    /// Writes an executable `name` that records its call and runs `body`.
    pub fn new(name: &str, body: &str) -> Fake {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(name);
        let rec = |f: &str| dir.path().join(f).display().to_string();
        let text = format!(
            "#!/bin/sh\n\
             printf '%s\\0' \"$@\" > '{argv}'\n\
             pwd -P > '{cwd}'\n\
             echo call >> '{calls}'\n\
             {body}\n",
            argv = rec("argv.bin"),
            cwd = rec("cwd.txt"),
            calls = rec("calls.txt"),
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fake {
            dir,
            name: name.to_owned(),
        }
    }

    /// Absolute path of the executable.
    pub fn program(&self) -> PathBuf {
        self.dir.path().join(&self.name)
    }

    /// An account whose command is the fake's absolute path.
    pub fn account(&self) -> Account {
        let cmd = self.program().to_string_lossy().into_owned();
        Account::new("main", format!("'{cmd}'"), None, Path::new("/home/user")).unwrap()
    }

    /// The arguments of the last call.
    pub fn argv(&self) -> Vec<String> {
        let bytes = std::fs::read(self.dir.path().join("argv.bin")).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let mut args: Vec<String> = text.split('\0').map(str::to_owned).collect();
        assert_eq!(args.pop().as_deref(), Some(""), "NUL-terminated");
        args
    }

    /// The physical working directory of the last call.
    pub fn cwd(&self) -> PathBuf {
        let text = std::fs::read_to_string(self.dir.path().join("cwd.txt")).unwrap();
        PathBuf::from(text.trim_end_matches('\n'))
    }

    /// How many times the fake was run.
    pub fn calls(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("calls.txt"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }
}

/// Path of a file under `tests/fixtures/`.
pub fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

/// A shell snippet that prints a fixture on stdout.
pub fn cat(rel: &str) -> String {
    format!("cat '{}'", fixture_path(rel).display())
}

/// A shell snippet that prints a fixture on stderr.
pub fn cat_err(rel: &str) -> String {
    format!("cat '{}' >&2", fixture_path(rel).display())
}
