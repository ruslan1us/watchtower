//! `tower update` (doc/0023) against a fake `curl` on `PATH` that serves
//! files from a temp dir, and a fake release that is a shell script (the
//! ELF check is relaxed for it through the hidden `require_elf` hook).
//! Replacement only ever targets a temp copy, never `current_exe`.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use watchtower::args::{self, Command as Cli};
use watchtower::update::{
    ASSET, Check, EXIT_UPDATE_AVAILABLE, LATEST_API, Outcome, Request, Updater, Version,
    looks_like_dev_build, platform_supported, sha256_hex,
};

/// Writing an executable while another thread forks can make the exec fail
/// with ETXTBSY, so the tests of this binary run one at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn write_exe(path: &Path, text: &str, mode: u32) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A fake `curl`: records each call's arguments (NUL-separated) as
/// `args.<n>.bin`, then serves `served/<url with every other byte as _>` to
/// `-o <file>` or stdout, or fails like `curl -f` on a 404.
struct FakeCurl {
    dir: tempfile::TempDir,
}

impl FakeCurl {
    fn new() -> FakeCurl {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::create_dir(dir.path().join("served")).unwrap();
        let script = format!(
            r#"#!/bin/sh
D='{dir}'
echo call >> "$D/calls.txt"
n=$(wc -l < "$D/calls.txt" | tr -d ' ')
printf '%s\0' "$@" > "$D/args.$n.bin"
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    *) url="$1"; shift ;;
  esac
done
key=$(printf '%s' "$url" | tr -c 'A-Za-z0-9.-' '_')
f="$D/served/$key"
if [ ! -f "$f" ]; then
  echo "curl: (22) The requested URL returned error: 404" >&2
  exit 22
fi
if [ -n "$out" ]; then cp "$f" "$out"; else cat "$f"; fi
"#,
            dir = dir.path().display()
        );
        write_exe(&dir.path().join("bin/curl"), &script, 0o755);
        FakeCurl { dir }
    }

    fn key(url: &str) -> String {
        url.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
                    b as char
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn serve(&self, url: &str, body: &[u8]) {
        std::fs::write(self.dir.path().join("served").join(Self::key(url)), body).unwrap();
    }

    /// Serves the API answer naming `tag` as the latest release.
    fn serve_latest(&self, tag: &str) {
        let body = serde_json::json!({ "tag_name": tag, "name": tag, "draft": false });
        self.serve(LATEST_API, body.to_string().as_bytes());
    }

    /// Serves release `tag` whose binary prints `prints` for `--version`,
    /// with a checksum file that matches it (or `sum` when given).
    fn serve_release(&self, tag: &str, prints: &str, sum: Option<&str>) -> Vec<u8> {
        let binary = format!("#!/bin/sh\necho '{prints}'\n").into_bytes();
        let base = format!("https://github.com/ruslan1us/watchtower/releases/download/{tag}");
        self.serve(&format!("{base}/{ASSET}"), &binary);
        let hash = sum.map_or_else(|| sha256_hex(&binary), str::to_owned);
        self.serve(
            &format!("{base}/{ASSET}.sha256"),
            format!("{hash}  {ASSET}\n").as_bytes(),
        );
        binary
    }

    fn path(&self) -> OsString {
        format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display()).into()
    }

    /// The arguments of every call so far.
    fn calls(&self) -> Vec<Vec<String>> {
        let count = std::fs::read_to_string(self.dir.path().join("calls.txt"))
            .map_or(0, |t| t.lines().count());
        (1..=count)
            .map(|n| {
                let raw = std::fs::read(self.dir.path().join(format!("args.{n}.bin"))).unwrap();
                raw.split(|b| *b == 0)
                    .filter(|a| !a.is_empty())
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .collect()
            })
            .collect()
    }

    /// The URL (last argument) of every call so far.
    fn urls(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|c| c.last().cloned().unwrap_or_default())
            .collect()
    }

    fn updater(&self, current: &str) -> Updater {
        Updater {
            current: v(current),
            path: Some(self.path()),
            require_elf: false,
        }
    }
}

/// A temp install dir holding `bin/tower`, an old release printing
/// `tower <version>`, with `mode`.
struct Install {
    dir: tempfile::TempDir,
}

impl Install {
    fn new(version: &str, mode: u32) -> Install {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let install = Install { dir };
        write_exe(
            &install.target(),
            &format!("#!/bin/sh\necho 'tower {version}'\n"),
            mode,
        );
        install
    }

    fn target(&self) -> PathBuf {
        self.dir.path().join("bin/tower")
    }

    fn bin_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path().join("bin"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn mode(&self) -> u32 {
        std::fs::metadata(self.target())
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }
}

fn run(updater: &Updater, target: &Path, request: &Request) -> (Result<Outcome, String>, String) {
    let mut out = Vec::new();
    let result = updater
        .update(target, request, &mut out)
        .map_err(|e| e.to_string());
    (result, String::from_utf8(out).unwrap())
}

const RELEASE_BASE: &str = "https://github.com/ruslan1us/watchtower/releases/download";

// ---- versions and arguments ---------------------------------------------

#[test]
fn semver_comparison() {
    assert!(v("0.3.0") > v("0.2.1"));
    assert!(v("0.10.0") > v("0.9.9"));
    assert!(v("1.0.0") > v("0.99.99"));
    assert!(v("v0.2.1") == v("0.2.1"));
    assert!(v("1.0.0") > v("1.0.0-rc.1"));
    assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
    assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
    assert!(v("1.0.0-beta") > v("1.0.0-alpha.1"));
    assert!(v("1.0.0-alpha.1") > v("1.0.0-alpha"));
    assert!(v("1.0.0-alpha.beta") > v("1.0.0-alpha.1"));
    assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
    assert_eq!(v("1.2.3").tag(), "v1.2.3");
    assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
    for bad in [
        "", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "1.2.3-", "latest",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad}");
    }
    assert_eq!(
        Version::current().to_string(),
        env!("CARGO_PKG_VERSION"),
        "the running version parses"
    );
}

#[test]
fn update_alias_and_options_parse() {
    let plain = Cli::Update {
        check: false,
        version: None,
        force: false,
    };
    assert_eq!(args::parse(["update"]), Ok(plain.clone()));
    assert_eq!(args::parse(["--update"]), Ok(plain));
    assert_eq!(
        args::parse(["--update", "--check"]),
        Ok(Cli::Update {
            check: true,
            version: None,
            force: false,
        })
    );
    assert_eq!(
        args::parse(["--update", "--version", "v0.3.0", "--force"]),
        Ok(Cli::Update {
            check: false,
            version: Some(v("0.3.0")),
            force: true,
        })
    );
    assert_eq!(args::parse(["--update", "--help"]), Ok(Cli::Help));
    assert!(args::parse(["--update", "--check", "--version", "v1.0.0"]).is_err());
    assert!(args::parse(["update", "--version", "next"]).is_err());
    assert!(args::USAGE.contains("tower update [--check | --version vX.Y.Z] [--force]"));
    assert!(args::USAGE.contains("--update"));
}

#[test]
fn platforms_and_dev_build_paths() {
    assert!(platform_supported("linux", "x86_64"));
    assert!(!platform_supported("linux", "aarch64"));
    assert!(!platform_supported("macos", "x86_64"));
    for dev in [
        "/home/user/watchtower/target/debug/tower",
        "/home/user/watchtower/target/release/tower",
        "/home/user/watchtower/target/x86_64-unknown-linux-musl/release/tower",
        "/home/user/watchtower/target/x86_64-unknown-linux-gnu/debug/tower",
    ] {
        assert!(looks_like_dev_build(Path::new(dev)), "{dev}");
    }
    for installed in [
        "/usr/local/bin/tower",
        "/home/user/.local/bin/tower",
        "/opt/target/tower",
        "/home/user/target/bin/tower",
    ] {
        assert!(!looks_like_dev_build(Path::new(installed)), "{installed}");
    }
}

// ---- --check ---------------------------------------------------------------

#[test]
fn check_reports_up_to_date_and_available() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.2.1");
    let same = curl.updater("0.2.1").check().unwrap();
    assert!(!same.available());
    assert_eq!(same.line(), "up to date (v0.2.1)");
    assert_eq!(same.exit_code(), 0);

    curl.serve_latest("v0.3.0");
    let newer = curl.updater("0.2.1").check().unwrap();
    assert_eq!(
        newer,
        Check {
            current: v("0.2.1"),
            latest: v("0.3.0"),
        }
    );
    assert_eq!(newer.line(), "v0.3.0 available (running v0.2.1)");
    assert_eq!(newer.exit_code(), EXIT_UPDATE_AVAILABLE);

    let ahead = curl.updater("0.4.0").check().unwrap();
    assert_eq!(ahead.exit_code(), 0);
    assert!(ahead.line().starts_with("up to date (v0.4.0"));

    // Every call: HTTPS only, curlrc ignored, the exact API URL, to stdout.
    for call in curl.calls() {
        assert_eq!(call[0], "-q", "{call:?}");
        assert!(call.contains(&"-fsSL".to_owned()));
        assert!(call.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(call.windows(2).any(|w| w == ["--proto-redir", "=https"]));
        assert!(call.contains(&"--tlsv1.2".to_owned()));
        assert!(call.windows(2).any(|w| w == ["--max-time", "120"]));
        assert!(!call.contains(&"-o".to_owned()));
        assert_eq!(call.last().unwrap(), LATEST_API);
    }
}

#[test]
fn check_network_error_and_bad_answer() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let err = curl.updater("0.2.1").check().unwrap_err().to_string();
    assert!(err.contains("cannot look up the latest release"), "{err}");
    assert!(err.contains("404"), "{err}");

    curl.serve(LATEST_API, b"{\"message\": \"rate limited\"}");
    let err = curl.updater("0.2.1").check().unwrap_err().to_string();
    assert!(err.contains("no tag_name"), "{err}");

    let missing = Updater {
        current: v("0.2.1"),
        path: Some(OsString::from("/nonexistent-dir")),
        require_elf: false,
    };
    let err = missing.check().unwrap_err().to_string();
    assert!(err.contains("`curl` is not installed"), "{err}");
    assert!(
        err.contains("releases/latest/download/tower-x86_64-linux-musl"),
        "{err}"
    );
}

/// The real binary: `--check` exit codes, and a dev build refused before
/// any network call. It never runs an update that could replace itself.
#[test]
fn binary_check_exit_codes_and_dev_build_refusal() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let tower = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tower"))
            .args(args)
            .env_clear()
            .env("PATH", curl.path())
            .env("HOME", curl.dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let current = env!("CARGO_PKG_VERSION");

    let out = tower(&["update", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("latest release"));

    curl.serve_latest(&format!("v{current}"));
    let out = tower(&["--update", "--check"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("up to date (v{current})\n")
    );

    curl.serve_latest("v999.0.0");
    let out = tower(&["update", "--check"]);
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("v999.0.0 available (running v{current})\n")
    );

    // The test binary lives under target/…: refused, no network call.
    let calls = curl.calls().len();
    let out = tower(&["update"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("looks like a cargo build output"),
        "{stderr}"
    );
    assert!(stderr.contains("--force"), "{stderr}");
    assert_eq!(curl.calls().len(), calls);

    let out = tower(&["update", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = String::from_utf8_lossy(&out.stdout);
    for option in ["--check", "--version vX.Y.Z", "--force", "--update"] {
        assert!(help.contains(option), "{option}: {help}");
    }
}

// ---- update ----------------------------------------------------------------

#[test]
fn update_replaces_the_file_and_keeps_its_mode() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    let binary = curl.serve_release("v0.3.0", "tower 0.3.0", None);
    let install = Install::new("0.2.1", 0o750);

    let (result, out) = run(
        &curl.updater("0.2.1"),
        &install.target(),
        &Request::default(),
    );
    assert_eq!(
        result,
        Ok(Outcome::Installed {
            from: v("0.2.1"),
            to: v("0.3.0"),
            path: install.target(),
        })
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), binary);
    assert_eq!(install.mode(), 0o750);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    assert!(
        out.contains(&format!(
            "tower 0.2.1 → 0.3.0: downloaded, checksum OK, installed to {}",
            install.target().display()
        )),
        "{out}"
    );
    assert!(out.contains("Running tower instances keep the old version until restarted."));

    // The API, then the versioned checksum and asset, each written with -o
    // into a temp dir next to the target.
    let base = format!("{RELEASE_BASE}/v0.3.0");
    assert_eq!(
        curl.urls(),
        [
            LATEST_API.to_owned(),
            format!("{base}/{ASSET}.sha256"),
            format!("{base}/{ASSET}"),
        ]
    );
    for call in &curl.calls()[1..] {
        assert!(call.windows(2).any(|w| w == ["--proto", "=https"]));
        let dest = &call[call.iter().position(|a| a == "-o").unwrap() + 1];
        assert_eq!(
            Path::new(dest).parent().unwrap().parent().unwrap(),
            install.target().parent().unwrap(),
            "{dest}"
        );
    }
}

#[test]
fn update_when_up_to_date_downloads_nothing_unless_forced() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.2.1");
    curl.serve_release("v0.2.1", "tower 0.2.1", None);
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();

    let (result, out) = run(
        &curl.updater("0.2.1"),
        &install.target(),
        &Request::default(),
    );
    assert_eq!(result, Ok(Outcome::UpToDate(v("0.2.1"))));
    assert!(out.contains("up to date (v0.2.1)"), "{out}");
    assert_eq!(curl.urls(), [LATEST_API]);
    assert_eq!(std::fs::read(install.target()).unwrap(), before);

    let forced = Request {
        version: None,
        force: true,
    };
    let (result, out) = run(&curl.updater("0.2.1"), &install.target(), &forced);
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert!(out.contains("0.2.1 → 0.2.1 (reinstall)"), "{out}");
}

#[test]
fn checksum_mismatch_leaves_the_file_untouched() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    curl.serve_release("v0.3.0", "tower 0.3.0", Some(&"0".repeat(64)));
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();

    let (result, _) = run(
        &curl.updater("0.2.1"),
        &install.target(),
        &Request::default(),
    );
    let err = result.unwrap_err();
    assert!(err.contains("checksum mismatch"), "{err}");
    assert!(err.contains("nothing was changed"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.mode(), 0o755);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
}

#[test]
fn explicit_version_uses_its_url_and_checks_what_runs() {
    let _guard = lock();
    let curl = FakeCurl::new();
    // The v0.2.0 asset claims to be 0.1.9 when run.
    curl.serve_release("v0.2.0", "tower 0.1.9", None);
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let request = Request {
        version: Some(v("v0.2.0")),
        force: false,
    };

    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &request);
    let err = result.unwrap_err();
    assert!(
        err.contains("reports `tower 0.1.9`, expected `tower 0.2.0`"),
        "{err}"
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    // No API lookup: straight to the versioned URLs.
    let base = format!("{RELEASE_BASE}/v0.2.0");
    assert_eq!(
        curl.urls(),
        [format!("{base}/{ASSET}.sha256"), format!("{base}/{ASSET}")]
    );

    // A correct v0.2.0 installs, and says it is a downgrade.
    let binary = curl.serve_release("v0.2.0", "tower 0.2.0", None);
    let (result, out) = run(&curl.updater("0.2.1"), &install.target(), &request);
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert!(out.contains("0.2.1 → 0.2.0 (downgrade)"), "{out}");
    assert_eq!(std::fs::read(install.target()).unwrap(), binary);

    // A release that does not exist.
    let missing = Request {
        version: Some(v("v9.9.9")),
        force: false,
    };
    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &missing);
    let err = result.unwrap_err();
    assert!(err.contains("cannot download v9.9.9"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), binary);
    assert_eq!(install.bin_entries(), ["tower"]);
}

#[test]
fn dev_build_path_is_refused_unless_forced() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    let binary = curl.serve_release("v0.3.0", "tower 0.3.0", None);
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("target/release");
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("tower");
    write_exe(&target, "#!/bin/sh\necho 'tower 0.2.1'\n", 0o755);

    let (result, _) = run(&curl.updater("0.2.1"), &target, &Request::default());
    let err = result.unwrap_err();
    assert!(err.contains("looks like a cargo build output"), "{err}");
    assert!(curl.calls().is_empty(), "refused before any network call");

    let forced = Request {
        version: None,
        force: true,
    };
    let (result, _) = run(&curl.updater("0.2.1"), &target, &forced);
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), binary);
}

#[test]
fn unwritable_directory_is_a_clear_error() {
    let _guard = lock();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root can write any directory");
        return;
    }
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    curl.serve_release("v0.3.0", "tower 0.3.0", None);
    let install = Install::new("0.2.1", 0o755);
    let bin = install.target().parent().unwrap().to_owned();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();

    let (result, _) = run(
        &curl.updater("0.2.1"),
        &install.target(),
        &Request::default(),
    );
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert_eq!(
        err,
        format!(
            "cannot write {}: run as its owner (e.g. sudo tower update)",
            install.target().display()
        )
    );
    assert_eq!(curl.urls(), [LATEST_API], "nothing downloaded");
    assert_eq!(install.bin_entries(), ["tower"]);
}

#[test]
fn elf_check_is_enforced_by_default() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    curl.serve_release("v0.3.0", "tower 0.3.0", None);
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let strict = Updater {
        require_elf: true,
        ..curl.updater("0.2.1")
    };
    assert!(Updater::new().require_elf);

    let (result, _) = run(&strict, &install.target(), &Request::default());
    let err = result.unwrap_err();
    assert!(err.contains("not an x86-64 Linux executable"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
}
