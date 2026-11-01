//! `tower update` (doc/0023) against a fake `curl` on `PATH` that serves
//! files from a temp dir, and a fake release that is a shell script (the
//! format check is relaxed for it through the hidden `check_format` hook).
//! Replacement only ever targets a temp copy, never `current_exe`. Signed
//! releases use minisign keys made at run time, trusted through the hidden
//! `keys` field.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

use ct_codecs::{Base64, Encoder};
use minisign_verify::Signature;
use watchtower::args::{self, Command as Cli};
use watchtower::update::{
    Check, EXIT_UPDATE_AVAILABLE, LATEST_API, LATEST_PAGE, MACOS_SINCE, Outcome, PINNED_KEYS,
    RELEASE_TAG_PREFIX, RELEASES_PAGE, Request, SIGNED_SINCE, Updater, Version, asset_for,
    elf_machine, first_release, is_elf_executable, is_macho_executable, is_signed,
    looks_like_dev_build, macho_cpu_type, parse_latest_redirect, platform_supported, sha256_hex,
};

/// What the fake releases stand in for, whatever system runs the tests.
const LINUX: (&str, &str) = ("linux", "x86_64");
const MAC_ARM: (&str, &str) = ("macos", "aarch64");
const MAC_X86: (&str, &str) = ("macos", "x86_64");

/// The release binaries of [`LINUX`], of Linux on ARM, and of macOS.
const ASSET: &str = "tower-x86_64-linux-musl";
const ARM_ASSET: &str = "tower-aarch64-linux-musl";
const MAC_ARM_ASSET: &str = "tower-arm64-macos";
const MAC_X86_ASSET: &str = "tower-x86_64-macos";

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
/// `args.<n>.bin`. For `<key>` (the URL with every other byte as `_`) it
/// fails like `curl -f` with the status in `status/<key>`; with `-w` it
/// prints `redirect/<key>` (the redirect target, like `%{redirect_url}`);
/// otherwise it serves `served/<key>` to `-o <file>` or stdout. Anything
/// missing fails like `curl -f` on a 404.
struct FakeCurl {
    dir: tempfile::TempDir,
}

impl FakeCurl {
    fn new() -> FakeCurl {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        for sub in ["served", "redirect", "status"] {
            std::fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let script = format!(
            r#"#!/bin/sh
D='{dir}'
echo call >> "$D/calls.txt"
n=$(wc -l < "$D/calls.txt" | tr -d ' ')
printf '%s\0' "$@" > "$D/args.$n.bin"
out=""; url=""; w=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -w) w="$2"; shift 2 ;;
    *) url="$1"; shift ;;
  esac
done
key=$(printf '%s' "$url" | tr -c 'A-Za-z0-9.-' '_')
if [ -f "$D/status/$key" ]; then
  echo "curl: (22) The requested URL returned error: $(cat "$D/status/$key")" >&2
  exit 22
fi
if [ -n "$w" ]; then
  if [ -f "$D/redirect/$key" ]; then cat "$D/redirect/$key"; exit 0; fi
  echo "curl: (22) The requested URL returned error: 404" >&2
  exit 22
fi
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

    /// Makes `url` answer with a redirect to `to`.
    fn redirect(&self, url: &str, to: &str) {
        std::fs::write(self.dir.path().join("redirect").join(Self::key(url)), to).unwrap();
    }

    /// Makes `url` fail with HTTP `status`.
    fn fail(&self, url: &str, status: u16) {
        let file = self.dir.path().join("status").join(Self::key(url));
        std::fs::write(file, status.to_string()).unwrap();
    }

    /// Makes the releases page redirect to `tag` as the latest release.
    fn serve_latest(&self, tag: &str) {
        self.redirect(LATEST_PAGE, &format!("{RELEASE_TAG_PREFIX}{tag}"));
    }

    /// Serves the API answer naming `tag` as the latest release.
    fn serve_latest_api(&self, tag: &str) {
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
            check_format: false,
            platform: LINUX,
            keys: Vec::new(),
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

/// The latest release, on the old unsigned way.
fn unsigned() -> Request {
    Request {
        allow_unsigned: true,
        ..Request::default()
    }
}

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
        allow_unsigned: false,
    };
    assert_eq!(args::parse(["update"]), Ok(plain.clone()));
    assert_eq!(args::parse(["--update"]), Ok(plain));
    assert_eq!(
        args::parse(["--update", "--check"]),
        Ok(Cli::Update {
            check: true,
            version: None,
            force: false,
            allow_unsigned: false,
        })
    );
    assert_eq!(
        args::parse(["--update", "--version", "v0.3.0", "--force"]),
        Ok(Cli::Update {
            check: false,
            version: Some(v("0.3.0")),
            force: true,
            allow_unsigned: false,
        })
    );
    assert_eq!(
        args::parse(["update", "--version", "v0.3.0", "--allow-unsigned"]),
        Ok(Cli::Update {
            check: false,
            version: Some(v("0.3.0")),
            force: false,
            allow_unsigned: true,
        })
    );
    assert!(args::parse(["update", "--check", "--allow-unsigned"]).is_err());
    assert_eq!(args::parse(["--update", "--help"]), Ok(Cli::Help));
    assert!(args::parse(["--update", "--check", "--version", "v1.0.0"]).is_err());
    assert!(args::parse(["update", "--version", "next"]).is_err());
    assert!(
        args::USAGE
            .contains("tower update [--check | --version vX.Y.Z] [--force] [--allow-unsigned]")
    );
    assert!(args::USAGE.contains(&format!("older than v{SIGNED_SINCE}")));
    assert!(args::USAGE.contains("--update"));
}

/// A platform without a binary keeps its refusal, and `--check` still looks.
#[test]
fn an_unsupported_platform_is_refused() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    let install = Install::new("0.2.1", 0o755);
    let riscv = Updater {
        platform: ("linux", "riscv64"),
        ..curl.updater("0.2.1")
    };
    let (result, _) = run(&riscv, &install.target(), &Request::default());
    let err = result.unwrap_err();
    assert_eq!(
        err,
        "releases exist only for Linux and macOS on x86_64 and aarch64, not linux riscv64; \
         build from source instead"
    );
    assert!(riscv.check().is_ok());
}

#[test]
fn platforms_and_dev_build_paths() {
    assert!(platform_supported("linux", "x86_64"));
    assert!(platform_supported("linux", "aarch64"));
    assert!(platform_supported("macos", "x86_64"));
    assert!(platform_supported("macos", "aarch64"));
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

    // Every call reads only the redirect of the releases page: curlrc
    // ignored, HTTPS only, no -L, the body dropped, the API never asked.
    let lookup = [
        "-q",
        "-fsS",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--max-time",
        "120",
        "-o",
        "/dev/null",
        "-w",
        "%{redirect_url}",
        LATEST_PAGE,
    ];
    let calls = curl.calls();
    assert_eq!(calls.len(), 3);
    for call in calls {
        assert_eq!(call, lookup);
    }
}

#[test]
fn check_network_error_and_bad_answer() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let err = curl.updater("0.2.1").check().unwrap_err().to_string();
    assert!(
        err.starts_with("cannot look up the latest release: releases page: "),
        "{err}"
    );
    assert!(err.contains("; GitHub API: "), "{err}");
    assert!(err.contains("404"), "{err}");
    assert!(
        err.ends_with("; `tower update --version vX.Y.Z` skips the lookup"),
        "{err}"
    );
    assert!(!err.contains('\n'), "one line: {err}");
    assert_eq!(curl.urls(), [LATEST_PAGE, LATEST_API]);

    curl.serve(LATEST_API, b"{\"message\": \"Not Found\"}");
    let err = curl.updater("0.2.1").check().unwrap_err().to_string();
    assert!(
        err.contains("GitHub API: the answer has no tag_name"),
        "{err}"
    );

    let missing = Updater {
        path: Some(OsString::from("/nonexistent-dir")),
        ..curl.updater("0.2.1")
    };
    let err = missing.check().unwrap_err().to_string();
    assert!(err.contains("`curl` is not installed"), "{err}");
    assert_eq!(err.matches("not installed").count(), 1, "{err}");
    assert!(
        err.contains("releases/latest/download/tower-x86_64-linux-musl"),
        "{err}"
    );
}

// ---- finding the latest release -------------------------------------------

#[test]
fn latest_redirect_is_parsed_strictly() {
    let ok = |url: &str| parse_latest_redirect(url);
    assert_eq!(ok(&format!("{RELEASE_TAG_PREFIX}v0.3.1")), Ok(v("0.3.1")));
    assert_eq!(ok(&format!("{RELEASE_TAG_PREFIX}v0.3.1\n")), Ok(v("0.3.1")));
    assert_eq!(
        ok(&format!("{RELEASE_TAG_PREFIX}v1.0.0-rc.1")),
        Ok(v("1.0.0-rc.1"))
    );

    // No release yet: GitHub redirects to the releases list.
    for url in [RELEASES_PAGE.to_owned(), format!("{RELEASES_PAGE}/")] {
        assert_eq!(ok(&url), Err("no release found".to_owned()), "{url}");
    }
    // No redirect at all.
    let err = ok("").unwrap_err();
    assert!(err.contains("did not redirect to a release"), "{err}");

    // Another repository, scheme or host, or a near miss of the prefix.
    for url in [
        "https://github.com/someone/watchtower/releases/tag/v0.3.1",
        "http://github.com/ruslan1us/watchtower/releases/tag/v0.3.1",
        "https://github.com.example/ruslan1us/watchtower/releases/tag/v0.3.1",
        "https://github.com/ruslan1us/watchtower/releases/tags/v0.3.1",
        "https://github.com/ruslan1us/watchtower-x/releases/tag/v0.3.1",
        "https://github.com/login",
    ] {
        let err = ok(url).unwrap_err();
        assert!(err.contains("unexpected URL"), "{url}: {err}");
    }
    // The right prefix, but not a `vX.Y.Z` tag.
    for tag in [
        "",
        "latest",
        "0.3.1",
        "v0.3",
        "v0.3.1/extra",
        "v0.3.1?x=1",
        "v0.3.1+build",
        "v0.3.1%0a",
        "v0.3.1 v0.3.2",
    ] {
        let err = ok(&format!("{RELEASE_TAG_PREFIX}{tag}")).unwrap_err();
        assert!(err.contains("is not a version"), "{tag}: {err}");
    }
}

#[test]
fn latest_uses_the_releases_page_before_the_api() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.1");
    curl.serve_latest_api("v0.2.0");
    assert_eq!(curl.updater("0.3.2").latest(), Ok(v("0.3.1")));
    assert_eq!(curl.urls(), [LATEST_PAGE], "the API is not asked");
}

#[test]
fn latest_falls_back_to_the_api() {
    let _guard = lock();

    // The page fails (rate limit or outage): the API answers.
    let curl = FakeCurl::new();
    curl.fail(LATEST_PAGE, 429);
    curl.serve_latest_api("v0.3.1");
    assert_eq!(curl.updater("0.3.2").latest(), Ok(v("0.3.1")));
    assert_eq!(curl.urls(), [LATEST_PAGE, LATEST_API]);
    let api_call = &curl.calls()[1];
    assert!(api_call.contains(&"-fsSL".to_owned()), "{api_call:?}");
    assert!(!api_call.contains(&"-w".to_owned()), "{api_call:?}");

    // A redirect somewhere unexpected is not trusted: the API answers.
    let curl = FakeCurl::new();
    curl.redirect(
        LATEST_PAGE,
        "https://github.com/someone/else/releases/tag/v9.9.9",
    );
    curl.serve_latest_api("v0.3.1");
    assert_eq!(curl.updater("0.3.2").latest(), Ok(v("0.3.1")));
    assert_eq!(curl.urls(), [LATEST_PAGE, LATEST_API]);

    // `--check` takes the same way.
    let check = curl.updater("0.3.0").check().unwrap();
    assert_eq!(check.line(), "v0.3.1 available (running v0.3.0)");
}

#[test]
fn latest_reports_both_failures_and_the_rate_limit() {
    let _guard = lock();

    // No release yet, and the API has none either.
    let curl = FakeCurl::new();
    curl.redirect(LATEST_PAGE, RELEASES_PAGE);
    curl.fail(LATEST_API, 404);
    let err = curl.updater("0.3.2").check().unwrap_err().to_string();
    assert!(
        err.starts_with(
            "cannot look up the latest release: releases page: no release found; GitHub API: "
        ),
        "{err}"
    );
    assert!(!err.contains("rate limit"), "{err}");
    assert!(
        err.ends_with("`tower update --version vX.Y.Z` skips the lookup"),
        "{err}"
    );

    // The API refuses with 403 or 429: named as the rate limit, curl's
    // text kept.
    for status in [403, 429] {
        let curl = FakeCurl::new();
        curl.fail(LATEST_PAGE, 503);
        curl.fail(LATEST_API, status);
        let err = curl.updater("0.3.2").check().unwrap_err().to_string();
        assert!(err.contains("releases page: curl: (22) "), "{err}");
        assert!(err.contains("returned error: 503"), "{err}");
        assert!(
            err.contains("GitHub API: GitHub's API rate limit for this network address"),
            "{err}"
        );
        assert!(
            err.contains(&format!(
                "returned error: {status} ({LATEST_API}, curl exit 22)"
            )),
            "{err}"
        );
        assert!(
            err.ends_with("`tower update --version vX.Y.Z` skips the lookup"),
            "{err}"
        );
        assert!(!err.contains('\n'), "one line: {err}");
    }

    // `tower update` fails the same way and changes nothing.
    let curl = FakeCurl::new();
    curl.fail(LATEST_PAGE, 403);
    curl.fail(LATEST_API, 403);
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let (result, _) = run(
        &curl.updater("0.2.1"),
        &install.target(),
        &Request::default(),
    );
    let err = result.unwrap_err();
    assert!(err.contains("rate limit"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
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
    for option in [
        "--check",
        "--version vX.Y.Z",
        "--force",
        "--allow-unsigned",
        "--update",
    ] {
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

    let (result, out) = run(&curl.updater("0.2.1"), &install.target(), &unsigned());
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
            "tower 0.2.1 → 0.3.0: downloaded, no signature, checksum OK, installed to {}",
            install.target().display()
        )),
        "{out}"
    );
    assert!(out.contains("Running tower instances keep the old version until restarted."));

    // The releases page, then the versioned checksum and asset, each written with -o
    // into a temp dir next to the target.
    let base = format!("{RELEASE_BASE}/v0.3.0");
    assert_eq!(
        curl.urls(),
        [
            LATEST_PAGE.to_owned(),
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
    assert_eq!(curl.urls(), [LATEST_PAGE]);
    assert_eq!(std::fs::read(install.target()).unwrap(), before);

    let forced = Request {
        force: true,
        ..unsigned()
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

    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &unsigned());
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
        ..unsigned()
    };

    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &request);
    let err = result.unwrap_err();
    assert!(
        err.contains("reports `tower 0.1.9`, expected `tower 0.2.0`"),
        "{err}"
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    // No lookup: straight to the versioned URLs.
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
        version: Some(v("v0.2.9")),
        ..unsigned()
    };
    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &missing);
    let err = result.unwrap_err();
    assert!(err.contains("cannot download v0.2.9"), "{err}");
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
        force: true,
        ..unsigned()
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

    let (result, _) = run(&curl.updater("0.2.1"), &install.target(), &unsigned());
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert_eq!(
        err,
        format!(
            "cannot write {}: run as its owner (e.g. sudo tower update)",
            install.target().display()
        )
    );
    assert_eq!(curl.urls(), [LATEST_PAGE], "nothing downloaded");
    assert_eq!(install.bin_entries(), ["tower"]);
}

#[test]
fn format_check_is_enforced_by_default() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_latest("v0.3.0");
    curl.serve_release("v0.3.0", "tower 0.3.0", None);
    let install = Install::new("0.2.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let strict = Updater {
        check_format: true,
        ..curl.updater("0.2.1")
    };
    assert!(Updater::new().check_format);

    let (result, _) = run(&strict, &install.target(), &unsigned());
    let err = result.unwrap_err();
    assert!(err.contains("not an x86-64 Linux executable"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
}

// ---- signed releases -------------------------------------------------------

/// A throwaway minisign key pair, made at test run time. No private key,
/// even a test one, is ever written to the repository.
struct Signer(minisign::KeyPair);

impl Signer {
    fn new() -> Signer {
        Signer(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
    }

    fn public(&self) -> String {
        self.0.pk.to_base64()
    }

    /// A `.minisig` over `data`, as `minisign -S -t <comment>` writes it.
    fn sign(&self, data: &[u8], comment: &str) -> String {
        minisign::sign(
            None,
            &self.0.sk,
            data,
            Some(comment),
            Some("throwaway test key"),
        )
        .unwrap()
        .to_string()
    }

    /// A valid legacy `.minisig` over `data`: Ed25519 over the data itself,
    /// not over its hash. The minisign crate only writes the prehashed form,
    /// so this signs with the same secret key directly.
    fn sign_legacy(&self, data: &[u8], comment: &str) -> String {
        // Algorithms (6), salt (32), limits (16) and key id (8) come first.
        let secret = &self.0.sk.to_bytes()[62..126];
        let secret = ed25519_compact::SecretKey::from_slice(secret).unwrap();
        let signature = secret.sign(data, None);
        let global = secret.sign([&signature[..], comment.as_bytes()].concat(), None);
        let line = [b"Ed", self.0.pk.keynum(), &signature[..]].concat();
        let text = format!(
            "untrusted comment: throwaway test key\n{}\ntrusted comment: {comment}\n{}\n",
            Base64::encode_to_string(line).unwrap(),
            Base64::encode_to_string(&global[..]).unwrap(),
        );
        // Genuine: only the strict mode can be what refuses it.
        minisign_verify::PublicKey::from_base64(&self.public())
            .unwrap()
            .verify(data, &Signature::decode(&text).unwrap(), true)
            .expect("a valid legacy signature");
        text
    }
}

/// A served release of `version`, signed by `signer`.
struct SignedRelease {
    version: String,
    base: String,
    asset: &'static str,
    binary: Vec<u8>,
    manifest: Vec<u8>,
}

impl SignedRelease {
    /// The trusted comment this release's signature must have.
    fn comment(&self) -> String {
        format!("watchtower v{}", self.version)
    }

    /// A release binary of this version that does something extra.
    fn evil_binary(&self, curl: &FakeCurl) -> Vec<u8> {
        curl.marking_binary(&format!("tower {}\necho evil", self.version))
    }

    fn manifest_url(&self) -> String {
        format!("{}/SHA256SUMS", self.base)
    }

    fn signature_url(&self) -> String {
        format!("{}/SHA256SUMS.minisig", self.base)
    }

    fn binary_url(&self) -> String {
        format!("{}/{}", self.base, self.asset)
    }

    fn sha256_url(&self) -> String {
        format!("{}/{}.sha256", self.base, self.asset)
    }
}

impl FakeCurl {
    /// A release binary that leaves a mark when run, so a test can tell that
    /// nothing downloaded ran.
    fn marking_binary(&self, prints: &str) -> Vec<u8> {
        format!(
            "#!/bin/sh\necho ran >> '{}'\necho '{prints}'\n",
            self.ran_mark().display()
        )
        .into_bytes()
    }

    fn ran_mark(&self) -> PathBuf {
        self.dir.path().join("ran")
    }

    fn ran(&self) -> bool {
        self.ran_mark().exists()
    }

    /// `sha256sum` lines for all the release binaries, this one's last.
    fn manifest_for(asset: &str, binary: &[u8]) -> Vec<u8> {
        let mut manifest = String::new();
        for other in [ASSET, ARM_ASSET, MAC_ARM_ASSET, MAC_X86_ASSET] {
            if other != asset {
                let hash = sha256_hex(other.as_bytes());
                manifest.push_str(&format!("{hash}  {other}\n"));
            }
        }
        manifest.push_str(&format!("{}  {asset}\n", sha256_hex(binary)));
        manifest.into_bytes()
    }

    /// Serves a whole release as the release workflow publishes it, including
    /// the old `.sha256` beside the binary, which a signed update must ignore.
    fn serve_signed(&self, version: &str, signer: &Signer, asset: &'static str) -> SignedRelease {
        let base = format!("{RELEASE_BASE}/v{version}");
        let binary = self.marking_binary(&format!("tower {version}"));
        let manifest = Self::manifest_for(asset, &binary);
        let release = SignedRelease {
            version: version.to_owned(),
            base,
            asset,
            binary,
            manifest,
        };
        self.serve(&release.binary_url(), &release.binary);
        self.serve(
            &release.sha256_url(),
            format!("{}  {asset}\n", sha256_hex(&release.binary)).as_bytes(),
        );
        self.serve(&release.manifest_url(), &release.manifest);
        let signature = signer.sign(&release.manifest, &format!("watchtower v{version}"));
        self.serve(&release.signature_url(), signature.as_bytes());
        release
    }

    /// Makes `release` hold `binary` instead, with a manifest and signature
    /// that vouch for it.
    fn replace_binary(&self, release: &SignedRelease, signer: &Signer, binary: &[u8]) {
        self.serve(&release.binary_url(), binary);
        let manifest = Self::manifest_for(release.asset, binary);
        self.serve(&release.manifest_url(), &manifest);
        let signature = signer.sign(&manifest, &release.comment());
        self.serve(&release.signature_url(), signature.as_bytes());
    }

    fn trusting(&self, current: &str, signers: &[&Signer]) -> Updater {
        Updater {
            keys: signers.iter().map(|s| s.public()).collect(),
            ..self.updater(current)
        }
    }
}

fn exactly(version: &str) -> Request {
    Request {
        version: Some(v(version)),
        ..Request::default()
    }
}

#[test]
fn a_good_signature_installs() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    curl.serve_latest("v0.7.0");
    let release = curl.serve_signed("0.7.0", &signer, ASSET);
    let install = Install::new("0.6.1", 0o750);

    let (result, out) = run(
        &curl.trusting("0.6.1", &[&signer]),
        &install.target(),
        &Request::default(),
    );
    assert_eq!(
        result,
        Ok(Outcome::Installed {
            from: v("0.6.1"),
            to: v("0.7.0"),
            path: install.target(),
        })
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
    assert_eq!(install.mode(), 0o750);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    assert!(curl.ran(), "the run check ran it, so `ran` can tell");
    assert!(out.contains("Signature OK for v0.7.0."), "{out}");
    assert!(
        out.contains(&format!(
            "tower 0.6.1 → 0.7.0: downloaded, signature and checksum OK, installed to {}",
            install.target().display()
        )),
        "{out}"
    );
    // The binary comes last, after the signature, and the old `.sha256` is
    // never asked for. Every call keeps the safe curl options.
    assert_eq!(
        curl.urls(),
        [
            LATEST_PAGE.to_owned(),
            release.manifest_url(),
            release.signature_url(),
            release.binary_url(),
        ]
    );
    for call in &curl.calls()[1..] {
        assert_eq!(call[0], "-q", "{call:?}");
        assert!(call.windows(2).any(|w| w == ["--proto", "=https"]));
    }
}

/// How a release can be wrong, what the error must say, and whether the
/// binary may have been downloaded by then. The text is written for
/// `tower-x86_64-linux-musl` v0.7.0, and adapted when run for another release.
type Fault = (
    &'static str,
    fn(&FakeCurl, &SignedRelease, &Signer),
    &'static str,
    bool,
);

const FAULTS: &[Fault] = &[
    (
        "tampered manifest",
        |curl, release, _| {
            let mut manifest = release.manifest.clone();
            manifest[0] = if manifest[0] == b'0' { b'1' } else { b'0' };
            curl.serve(&release.manifest_url(), &manifest);
        },
        "does not match",
        false,
    ),
    (
        "manifest and binary swapped together",
        |curl, release, _| {
            let evil = release.evil_binary(curl);
            curl.serve(&release.binary_url(), &evil);
            curl.serve(
                &release.manifest_url(),
                &FakeCurl::manifest_for(release.asset, &evil),
            );
        },
        "does not match",
        false,
    ),
    (
        "tampered binary",
        |curl, release, _| {
            curl.serve(&release.binary_url(), &release.evil_binary(curl));
        },
        "checksum mismatch",
        true,
    ),
    (
        "signature from an unknown key",
        |curl, release, _| {
            let stranger = Signer::new();
            let forged = stranger.sign(&release.manifest, &release.comment());
            curl.serve(&release.signature_url(), forged.as_bytes());
        },
        "unknown key",
        false,
    ),
    (
        "legacy signature",
        |curl, release, signer| {
            let legacy = signer.sign_legacy(&release.manifest, &release.comment());
            curl.serve(&release.signature_url(), legacy.as_bytes());
        },
        "legacy format",
        false,
    ),
    (
        "an older release's signature, replayed",
        |curl, release, signer| {
            let replay = signer.sign(&release.manifest, "watchtower v0.6.9");
            curl.serve(&release.signature_url(), replay.as_bytes());
        },
        "expected `watchtower v0.7.0`, found `watchtower v0.6.9`",
        false,
    ),
    (
        "trusted comment edited after signing",
        |curl, release, signer| {
            let good = signer.sign(&release.manifest, "watchtower v0.6.9");
            let edited = good.replace("watchtower v0.6.9", &release.comment());
            curl.serve(&release.signature_url(), edited.as_bytes());
        },
        "does not match",
        false,
    ),
    (
        "missing signature",
        |curl, release, _| {
            std::fs::remove_file(
                curl.dir
                    .path()
                    .join("served")
                    .join(FakeCurl::key(&release.signature_url())),
            )
            .unwrap();
        },
        "cannot download v0.7.0 SHA256SUMS.minisig",
        false,
    ),
    (
        "missing manifest",
        |curl, release, _| {
            curl.fail(&release.manifest_url(), 404);
        },
        "cannot download v0.7.0 SHA256SUMS",
        false,
    ),
    (
        "signature that is not one",
        |curl, release, _| {
            curl.serve(
                &release.signature_url(),
                b"<html>not a signature\x1b[31m</html>",
            );
        },
        "SHA256SUMS.minisig of v0.7.0 is not a minisign signature",
        false,
    ),
    (
        "signature that is not text",
        |curl, release, _| {
            curl.serve(&release.signature_url(), &[0xff, 0xfe, 0x00]);
        },
        "is not a minisign signature",
        false,
    ),
    (
        "far too long a manifest",
        |curl, release, signer| {
            let long = vec![b'\n'; 64 * 1024 + 1];
            curl.serve(&release.manifest_url(), &long);
            let signature = signer.sign(&long, &release.comment());
            curl.serve(&release.signature_url(), signature.as_bytes());
        },
        "SHA256SUMS is too long",
        false,
    ),
    (
        "signed manifest without this binary",
        |curl, release, signer| {
            let other = format!("{}  {ARM_ASSET}\n", sha256_hex(b"other")).into_bytes();
            curl.serve(&release.manifest_url(), &other);
            let signature = signer.sign(&other, &release.comment());
            curl.serve(&release.signature_url(), signature.as_bytes());
        },
        "SHA256SUMS has no line for tower-x86_64-linux-musl",
        false,
    ),
    (
        "signed manifest naming the binary only in part",
        |curl, release, signer| {
            let (hash, asset) = (sha256_hex(&release.binary), release.asset);
            let near = format!("{hash}  {asset}.sha256\n{hash}  x{asset}\n").into_bytes();
            curl.serve(&release.manifest_url(), &near);
            let signature = signer.sign(&near, &release.comment());
            curl.serve(&release.signature_url(), signature.as_bytes());
        },
        "SHA256SUMS has no line for tower-x86_64-linux-musl",
        false,
    ),
];

/// Every fault is refused with any flags: the binary stays byte-identical,
/// nothing downloaded runs, and no temp dir stays behind. `--force` and
/// `--allow-unsigned` skip none of it, and nothing falls back to `.sha256`.
/// macOS takes the same way.
#[test]
fn a_bad_release_is_refused_whatever_the_flags() {
    let _guard = lock();
    for (version, platform, asset) in [
        ("0.7.0", LINUX, ASSET),
        ("0.7.1", MAC_ARM, MAC_ARM_ASSET),
        ("0.7.1", MAC_X86, MAC_X86_ASSET),
    ] {
        let tag = format!("v{version}");
        for (name, fault, says, binary_fetched) in FAULTS {
            let says = says.replace("v0.7.0", &tag).replace(ASSET, asset);
            for (force, allow_unsigned) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let context = format!(
                    "{name} on {platform:?} (force {force}, allow-unsigned {allow_unsigned})"
                );
                let curl = FakeCurl::new();
                let signer = Signer::new();
                let release = curl.serve_signed(version, &signer, asset);
                fault(&curl, &release, &signer);
                let install = Install::new("0.6.1", 0o755);
                let before = std::fs::read(install.target()).unwrap();
                let request = Request {
                    force,
                    allow_unsigned,
                    ..exactly(&tag)
                };
                let updater = Updater {
                    platform,
                    ..curl.trusting("0.6.1", &[&signer])
                };

                let (result, _) = run(&updater, &install.target(), &request);
                let err = result.unwrap_err();
                assert!(err.contains(&says), "{context}: {err}");
                assert!(err.ends_with("nothing was changed"), "{context}: {err}");
                assert!(
                    err.chars().all(|c| !c.is_control()),
                    "{context}: no raw control characters: {err:?}"
                );
                assert_eq!(
                    std::fs::read(install.target()).unwrap(),
                    before,
                    "{context}"
                );
                assert_eq!(install.mode(), 0o755, "{context}");
                assert_eq!(install.bin_entries(), ["tower"], "{context}: no leftovers");
                assert!(!curl.ran(), "{context}: nothing downloaded may run");
                let urls = curl.urls();
                assert_eq!(
                    urls.contains(&release.binary_url()),
                    *binary_fetched,
                    "{context}: {urls:?}"
                );
                assert!(
                    !urls.contains(&release.sha256_url()),
                    "{context}: no fallback to the .sha256: {urls:?}"
                );
            }
        }
    }
}

/// The trusted comment must be the exact text: nothing around it, and the
/// raw bytes of a wrong one never reach the terminal.
#[test]
fn the_trusted_comment_must_match_exactly_and_is_not_echoed() {
    let _guard = lock();
    for comment in [
        "",
        "v0.7.0",
        "watchtower 0.7.0",
        "watchtower v0.7.0 ",
        " watchtower v0.7.0",
        "watchtower v0.7.0-rc.1",
        "watchtower v0.7.00",
        "watchtower v0.7.0\tprehashed",
        "Watchtower v0.7.0",
        "not-watchtower v0.7.0",
        "\u{1b}]0;owned\u{7}\u{1b}[2J\u{202e}watchtower v0.7.0",
    ] {
        let curl = FakeCurl::new();
        let signer = Signer::new();
        let release = curl.serve_signed("0.7.0", &signer, ASSET);
        let signature = signer.sign(&release.manifest, comment);
        curl.serve(&release.signature_url(), signature.as_bytes());
        let install = Install::new("0.6.1", 0o755);
        let before = std::fs::read(install.target()).unwrap();

        let (result, out) = run(
            &curl.trusting("0.6.1", &[&signer]),
            &install.target(),
            &exactly("v0.7.0"),
        );
        let err = result.unwrap_err();
        assert!(err.contains("expected `watchtower v0.7.0`"), "{err}");
        assert!(
            err.chars()
                .all(|c| c == '…' || c.is_ascii_graphic() || c == ' '),
            "{comment:?}: {err:?}"
        );
        assert!(!out.contains("Signature OK"), "{out}");
        assert_eq!(std::fs::read(install.target()).unwrap(), before);
        assert!(!curl.ran());
        assert!(!curl.urls().contains(&release.binary_url()));
    }
}

/// The checks after the signature still hold: the binary must report the
/// right version, and be an executable of this system's format for this
/// architecture before it may run.
#[test]
fn a_signed_release_still_gets_the_format_and_run_checks() {
    let _guard = lock();
    let signer = Signer::new();

    // Signed and matching, but it says it is something else when run.
    let curl = FakeCurl::new();
    let release = curl.serve_signed("0.7.0", &signer, ASSET);
    let liar = curl.marking_binary("tower 0.6.9");
    curl.serve(&release.binary_url(), &liar);
    let manifest = FakeCurl::manifest_for(ASSET, &liar);
    curl.serve(&release.manifest_url(), &manifest);
    let signature = signer.sign(&manifest, "watchtower v0.7.0");
    curl.serve(&release.signature_url(), signature.as_bytes());
    let install = Install::new("0.6.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let (result, _) = run(
        &curl.trusting("0.6.1", &[&signer]),
        &install.target(),
        &exactly("v0.7.0"),
    );
    let err = result.unwrap_err();
    assert!(err.contains("reports `tower 0.6.9`"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);

    // A shell script is no executable of either format: refused before it can
    // run. On macOS this is all that stops it, as macOS would run it with `sh`.
    for (platform, asset, says) in [
        (LINUX, ASSET, "not an x86-64 Linux executable"),
        (
            ("linux", "aarch64"),
            ARM_ASSET,
            "not an aarch64 Linux executable",
        ),
        (MAC_ARM, MAC_ARM_ASSET, "not an arm64 macOS executable"),
        (MAC_X86, MAC_X86_ASSET, "not an x86-64 macOS executable"),
    ] {
        let curl = FakeCurl::new();
        curl.serve_signed("0.7.1", &signer, asset);
        let strict = Updater {
            check_format: true,
            platform,
            ..curl.trusting("0.6.1", &[&signer])
        };
        let (result, _) = run(&strict, &install.target(), &exactly("v0.7.1"));
        let err = result.unwrap_err();
        assert!(err.contains(says), "{err}");
        assert!(!curl.ran(), "not run");
        assert_eq!(std::fs::read(install.target()).unwrap(), before);
    }
}

/// A bare ELF header for `machine`.
fn elf(machine: u16) -> [u8; 20] {
    let mut header = [0u8; 20];
    header[..4].copy_from_slice(b"\x7fELF");
    (header[4], header[5], header[16]) = (2, 1, 3);
    header[18..20].copy_from_slice(&machine.to_le_bytes());
    header
}

/// The start of a `mach_header_64`: magic, cputype, cpusubtype, filetype, ncmds.
fn macho(magic: [u8; 4], cpu_type: u32, filetype: u32) -> [u8; 20] {
    let mut header = [0u8; 20];
    header[..4].copy_from_slice(&magic);
    header[4..8].copy_from_slice(&cpu_type.to_le_bytes());
    header[12..16].copy_from_slice(&filetype.to_le_bytes());
    header
}

const MH_MAGIC_64: [u8; 4] = [0xCF, 0xFA, 0xED, 0xFE];
const CPU_TYPE_ARM64: u32 = 0x0100_000C;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const MH_EXECUTE: u32 = 2;

/// Why a signed v0.7.1 whose binary is `binary` is not installed on `platform`,
/// with the format check on. The checksum and signature are right, so only the
/// checks after them can refuse it.
fn refusal_of_binary(platform: (&'static str, &'static str), binary: &[u8]) -> String {
    let asset = asset_for(platform.0, platform.1).unwrap();
    let signer = Signer::new();
    let curl = FakeCurl::new();
    let release = curl.serve_signed("0.7.1", &signer, asset);
    curl.replace_binary(&release, &signer, binary);
    let strict = Updater {
        check_format: true,
        platform,
        ..curl.trusting("0.6.1", &[&signer])
    };
    let install = Install::new("0.6.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let (result, _) = run(&strict, &install.target(), &exactly("v0.7.1"));
    let err = result.unwrap_err();
    assert!(err.ends_with("nothing was changed"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    err
}

/// An ELF for the other architecture, with its checksum right, is refused;
/// the right one passes that check (and then cannot run, being a header).
#[test]
fn the_elf_machine_must_be_the_running_arch() {
    let _guard = lock();
    assert_eq!(elf_machine("x86_64"), Some(62));
    assert_eq!(elf_machine("aarch64"), Some(183));
    assert_eq!(elf_machine("riscv64"), None);

    assert!(is_elf_executable(&elf(62), 62));
    assert!(is_elf_executable(&elf(183), 183));
    assert!(!is_elf_executable(&elf(183), 62));
    assert!(!is_elf_executable(&elf(62), 183));

    for (arch, machine, wrong, says) in [
        ("x86_64", 62, 183, "not an x86-64 Linux executable"),
        ("aarch64", 183, 62, "not an aarch64 Linux executable"),
    ] {
        let err = refusal_of_binary(("linux", arch), &elf(wrong));
        assert!(err.contains(says), "{arch}: {err}");
        let err = refusal_of_binary(("linux", arch), &elf(machine));
        // Linux refuses to exec the bare header; macOS runs it through `sh`, which prints nothing.
        assert!(err.contains("the downloaded binary"), "{arch}: {err}");
        assert!(!err.contains("Linux executable"), "{arch}: {err}");
    }
}

/// macOS wants a 64-bit thin Mach-O executable for this architecture and nothing
/// else. The right one passes that check and then cannot run, being a header:
/// which error that gives depends on the system running the test, so the
/// assertion is loose.
#[test]
fn the_macho_check_wants_a_thin_executable_for_this_arch() {
    let _guard = lock();
    assert_eq!(macho_cpu_type("aarch64"), Some(CPU_TYPE_ARM64));
    assert_eq!(macho_cpu_type("x86_64"), Some(CPU_TYPE_X86_64));
    assert_eq!(macho_cpu_type("riscv64"), None);

    assert!(is_macho_executable(
        &macho(MH_MAGIC_64, CPU_TYPE_ARM64, MH_EXECUTE),
        CPU_TYPE_ARM64
    ));
    assert!(!is_macho_executable(
        &macho(MH_MAGIC_64, CPU_TYPE_ARM64, MH_EXECUTE),
        CPU_TYPE_X86_64
    ));

    for (platform, cpu, wrong, says) in [
        (
            MAC_ARM,
            CPU_TYPE_ARM64,
            CPU_TYPE_X86_64,
            "the download is not an arm64 macOS executable",
        ),
        (
            MAC_X86,
            CPU_TYPE_X86_64,
            CPU_TYPE_ARM64,
            "the download is not an x86-64 macOS executable",
        ),
    ] {
        let err = refusal_of_binary(platform, &macho(MH_MAGIC_64, cpu, MH_EXECUTE));
        assert!(err.contains("the downloaded binary"), "{platform:?}: {err}");
        assert!(!err.contains("macOS executable"), "{platform:?}: {err}");

        let fat = |magic: [u8; 4]| {
            let mut header = [0u8; 20];
            header[..4].copy_from_slice(&magic);
            header[7] = 2; // Two architectures, big-endian.
            header[11] = 0x0C;
            header
        };
        let refused: [(&str, Vec<u8>); 12] = [
            (
                "the other architecture",
                macho(MH_MAGIC_64, wrong, MH_EXECUTE).to_vec(),
            ),
            ("a fat binary", fat([0xCA, 0xFE, 0xBA, 0xBE]).to_vec()),
            (
                "a 64-bit fat binary",
                fat([0xCA, 0xFE, 0xBA, 0xBF]).to_vec(),
            ),
            ("an object file", macho(MH_MAGIC_64, cpu, 1).to_vec()),
            ("a library", macho(MH_MAGIC_64, cpu, 6).to_vec()),
            ("a bundle", macho(MH_MAGIC_64, cpu, 8).to_vec()),
            (
                "a 32-bit Mach-O",
                macho([0xCE, 0xFA, 0xED, 0xFE], cpu, MH_EXECUTE).to_vec(),
            ),
            (
                "a big-endian Mach-O",
                macho([0xFE, 0xED, 0xFA, 0xCF], cpu, MH_EXECUTE).to_vec(),
            ),
            ("an ELF", elf(62).to_vec()),
            ("a script", b"#!/bin/sh\necho 'tower 0.7.1'\n".to_vec()),
            ("too short", MH_MAGIC_64.to_vec()),
            ("empty", Vec::new()),
        ];
        for (name, binary) in refused {
            let err = refusal_of_binary(platform, &binary);
            assert!(err.contains(says), "{platform:?} {name}: {err}");
        }
    }

    // A Mach-O on Linux is refused like anything else that is no ELF.
    for (arch, cpu, says) in [
        ("x86_64", CPU_TYPE_X86_64, "not an x86-64 Linux executable"),
        ("aarch64", CPU_TYPE_ARM64, "not an aarch64 Linux executable"),
    ] {
        let err = refusal_of_binary(("linux", arch), &macho(MH_MAGIC_64, cpu, MH_EXECUTE));
        assert!(err.contains(says), "{arch}: {err}");
    }
}

/// This build itself, whatever system runs the test, passes that system's format
/// check, and through a signed release it installs and runs from its new place.
/// On macOS CI this is what shows a real Mach-O passing, the rename leaving it
/// runnable. It needs a version with a build for this platform, so until the
/// version reaches [`MACOS_SINCE`] macOS only checks the header.
#[test]
fn this_build_passes_its_own_format_check_and_installs() {
    let _guard = lock();
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let Some(asset) = asset_for(os, arch) else {
        eprintln!("skipped: no release binary for {os} {arch}");
        return;
    };
    let binary = std::fs::read(env!("CARGO_BIN_EXE_tower")).unwrap();
    match os {
        "linux" => assert!(is_elf_executable(&binary, elf_machine(arch).unwrap())),
        "macos" => assert!(is_macho_executable(&binary, macho_cpu_type(arch).unwrap())),
        _ => panic!("no format check for {os}"),
    }

    let current = Version::current();
    if first_release(os, arch).is_some_and(|first| current < v(first)) {
        eprintln!("skipped the install: {current} is older than the first {os} {arch} release");
        return;
    }
    let curl = FakeCurl::new();
    let signer = Signer::new();
    let release = curl.serve_signed(&current.to_string(), &signer, asset);
    curl.replace_binary(&release, &signer, &binary);
    let install = Install::new("0.6.1", 0o755);
    let real = Updater {
        check_format: true,
        platform: (os, arch),
        ..curl.trusting("0.6.1", &[&signer])
    };
    let (result, _) = run(&real, &install.target(), &exactly(&current.tag()));
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), binary);
    assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
    let out = Command::new(install.target())
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("tower {current}")
    );
}

// ---- macOS -----------------------------------------------------------------

/// A signed macOS release installs like a Linux one, with its own binary for
/// each architecture and nothing of the other platforms' asked for.
#[test]
fn a_signed_macos_update_installs() {
    let _guard = lock();
    for (platform, asset) in [(MAC_ARM, MAC_ARM_ASSET), (MAC_X86, MAC_X86_ASSET)] {
        let curl = FakeCurl::new();
        let signer = Signer::new();
        curl.serve_latest("v0.7.1");
        let release = curl.serve_signed("0.7.1", &signer, asset);
        let install = Install::new("0.7.0", 0o750);
        let mac = Updater {
            platform,
            ..curl.trusting("0.7.0", &[&signer])
        };

        let check = mac.check().unwrap();
        assert_eq!(check.line(), "v0.7.1 available (running v0.7.0)");
        assert_eq!(check.exit_code(), EXIT_UPDATE_AVAILABLE);

        let (result, out) = run(&mac, &install.target(), &Request::default());
        assert_eq!(
            result,
            Ok(Outcome::Installed {
                from: v("0.7.0"),
                to: v("0.7.1"),
                path: install.target(),
            })
        );
        assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
        assert_eq!(install.mode(), 0o750);
        assert_eq!(install.bin_entries(), ["tower"], "no temp leftovers");
        assert!(out.contains("Signature OK for v0.7.1."), "{out}");
        assert_eq!(
            curl.urls(),
            [
                LATEST_PAGE.to_owned(),
                LATEST_PAGE.to_owned(),
                release.manifest_url(),
                release.signature_url(),
                format!("{RELEASE_BASE}/v0.7.1/{asset}"),
            ]
        );
    }
}

/// The first macOS release, a pre-release, installs by its full tag.
#[test]
fn the_first_macos_release_installs() {
    let _guard = lock();
    assert_eq!(MACOS_SINCE, "0.7.1-rc.1");
    assert!(is_signed(&v(MACOS_SINCE)), "macOS has no unsigned release");
    for (platform, asset) in [(MAC_ARM, MAC_ARM_ASSET), (MAC_X86, MAC_X86_ASSET)] {
        let curl = FakeCurl::new();
        let signer = Signer::new();
        let release = curl.serve_signed(MACOS_SINCE, &signer, asset);
        let install = Install::new("0.7.0", 0o755);
        let mac = Updater {
            platform,
            ..curl.trusting("0.7.0", &[&signer])
        };
        let (result, _) = run(&mac, &install.target(), &exactly("v0.7.1-rc.1"));
        assert!(
            matches!(result, Ok(Outcome::Installed { .. })),
            "{result:?}"
        );
        assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
    }
}

#[test]
fn the_first_release_per_platform() {
    assert_eq!(first_release("linux", "x86_64"), None);
    assert_eq!(first_release("linux", "aarch64"), Some(SIGNED_SINCE));
    assert_eq!(first_release("macos", "aarch64"), Some(MACOS_SINCE));
    assert_eq!(first_release("macos", "x86_64"), Some(MACOS_SINCE));
    assert_eq!(first_release("linux", "riscv64"), None);
    assert_eq!(first_release("windows", "x86_64"), None);
}

/// A release older than the platform's first has no binary to fetch: refused
/// by name before any network call, and no flag helps or is suggested. It comes
/// before the question of keys.
#[test]
fn a_target_before_the_platforms_first_release_is_refused() {
    let _guard = lock();
    for (platform, name, first, targets) in [
        (
            MAC_ARM,
            "macOS",
            "v0.7.1-rc.1",
            &[
                "v0.7.1-rc.0",
                "v0.7.1-beta.1",
                "v0.7.0",
                "v0.7.0-rc.1",
                "v0.6.1",
            ][..],
        ),
        (
            MAC_X86,
            "macOS",
            "v0.7.1-rc.1",
            &["v0.7.1-rc.0", "v0.7.0", "v0.6.1"][..],
        ),
        (
            ("linux", "aarch64"),
            "aarch64",
            "v0.7.0-rc.1",
            &["v0.7.0-beta.1", "v0.6.1"][..],
        ),
    ] {
        for target in targets {
            let want =
                format!("{target} has no {name} build, the first is {first}; nothing was changed");
            for (force, allow_unsigned) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let curl = FakeCurl::new();
                curl.serve_release(target, "tower 0.0.0", None);
                let install = Install::new("0.7.1", 0o755);
                let before = std::fs::read(install.target()).unwrap();
                for keys in [Vec::new(), vec![Signer::new().public()]] {
                    let updater = Updater {
                        platform,
                        keys,
                        ..curl.updater("0.7.1")
                    };
                    let request = Request {
                        force,
                        allow_unsigned,
                        ..exactly(target)
                    };
                    let (result, out) = run(&updater, &install.target(), &request);
                    assert_eq!(result.unwrap_err(), want);
                    assert_eq!(out, "", "not even a first step");
                }
                assert!(curl.calls().is_empty(), "no network: {:?}", curl.calls());
                assert_eq!(std::fs::read(install.target()).unwrap(), before);
                assert_eq!(install.bin_entries(), ["tower"]);
            }
        }
    }
}

/// `tower update` must not suggest a `--version` this platform would refuse. Pre-
/// releases are never the latest, so a Mac on the first one really meets this.
#[test]
fn the_downgrade_hint_names_only_what_this_platform_can_install() {
    let _guard = lock();
    let attempt = |platform: (&'static str, &'static str), running: &str, latest: &str| {
        let curl = FakeCurl::new();
        curl.serve_latest(latest);
        let install = Install::new(running, 0o755);
        let before = std::fs::read(install.target()).unwrap();
        let updater = Updater {
            platform,
            keys: vec![Signer::new().public()],
            ..curl.updater(running)
        };
        let (result, _) = run(&updater, &install.target(), &Request::default());
        assert_eq!(curl.urls(), [LATEST_PAGE], "nothing downloaded");
        assert_eq!(std::fs::read(install.target()).unwrap(), before);
        result.unwrap_err()
    };

    for platform in [MAC_ARM, MAC_X86] {
        assert_eq!(
            attempt(platform, "0.7.1-rc.1", "v0.7.0"),
            "running v0.7.1-rc.1 is newer than the latest release v0.7.0; nothing was changed"
        );
        assert_eq!(
            attempt(platform, "0.7.2", "v0.7.1"),
            "running v0.7.2 is newer than the latest release v0.7.1; nothing was changed \
             (use --version v0.7.1 to downgrade)"
        );
    }
    assert_eq!(
        attempt(("linux", "aarch64"), "0.7.0", "v0.6.1"),
        "running v0.7.0 is newer than the latest release v0.6.1; nothing was changed"
    );
    assert_eq!(
        attempt(LINUX, "0.7.0", "v0.6.1"),
        "running v0.7.0 is newer than the latest release v0.6.1; nothing was changed \
         (use --version v0.6.1 to downgrade)"
    );

    // `--force` gets past that question, and meets the refusal it would have
    // met with the hint.
    let curl = FakeCurl::new();
    curl.serve_latest("v0.7.0");
    let install = Install::new("0.7.1-rc.1", 0o755);
    let mac = Updater {
        platform: MAC_ARM,
        ..curl.updater("0.7.1-rc.1")
    };
    let forced = Request {
        force: true,
        ..Request::default()
    };
    let (result, _) = run(&mac, &install.target(), &forced);
    assert_eq!(
        result.unwrap_err(),
        "v0.7.0 has no macOS build, the first is v0.7.1-rc.1; nothing was changed"
    );
    assert_eq!(curl.urls(), [LATEST_PAGE]);

    // `--check` on the same Mac is calm about it.
    let check = mac.check().unwrap();
    assert_eq!(
        check.line(),
        "up to date (v0.7.1-rc.1; latest release v0.7.0)"
    );
    assert_eq!(check.exit_code(), 0);
}

/// The real binary, copied out of `target/` so it is no cargo build output: on a
/// platform with a first release, an older target is refused before any network
/// call, whatever the flags. The copy is removed first because the fixture's
/// file must not be overwritten in place, which macOS can punish on a signed
/// binary.
#[test]
fn binary_refuses_a_target_before_its_platforms_first_release() {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let Some(first) = first_release(os, arch) else {
        eprintln!("skipped: every release has a binary for {os} {arch}");
        return;
    };
    let _guard = lock();
    let curl = FakeCurl::new();
    let install = Install::new("0.6.1", 0o755);
    std::fs::remove_file(install.target()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_tower"), install.target()).unwrap();
    let before = std::fs::read(install.target()).unwrap();
    let out = Command::new(install.target())
        .args([
            "update",
            "--version",
            "v0.6.1",
            "--force",
            "--allow-unsigned",
        ])
        .env_clear()
        .env("PATH", curl.path())
        .env("HOME", curl.dir.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let name = if os == "macos" { "macOS" } else { arch };
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "v0.6.1 has no {name} build, the first is v{first}; nothing was changed"
        )),
        "{stderr}"
    );
    assert!(curl.calls().is_empty(), "no network: {:?}", curl.calls());
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
}

#[test]
fn either_of_two_keys_is_accepted() {
    let _guard = lock();
    let (first, second) = (Signer::new(), Signer::new());
    for signer in [&first, &second] {
        for keys in [[&first, &second], [&second, &first]] {
            let curl = FakeCurl::new();
            let release = curl.serve_signed("0.7.0", signer, ASSET);
            let install = Install::new("0.6.1", 0o755);
            let (result, out) = run(
                &curl.trusting("0.6.1", &keys),
                &install.target(),
                &exactly("v0.7.0"),
            );
            assert!(
                matches!(result, Ok(Outcome::Installed { .. })),
                "{result:?}"
            );
            assert!(out.contains("Signature OK"), "{out}");
            assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
        }
    }
}

/// No keys, or a key that does not parse, verifies nothing at all.
#[test]
fn empty_or_broken_keys_fail_closed() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    let release = curl.serve_signed("0.7.0", &signer, ASSET);
    let install = Install::new("0.6.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();

    for request in [
        exactly("v0.7.0"),
        Request {
            force: true,
            allow_unsigned: true,
            ..exactly("v0.7.0")
        },
    ] {
        let (result, _) = run(&curl.trusting("0.6.1", &[]), &install.target(), &request);
        let err = result.unwrap_err();
        assert!(err.contains("no release signing keys"), "{err}");
        assert!(err.contains("cannot verify v0.7.0"), "{err}");
        assert!(err.ends_with("nothing was changed"), "{err}");
        assert!(curl.calls().is_empty(), "refused before any download");
    }

    for keys in [
        vec![signer.public(), "not a key".to_owned()],
        vec!["not a key".to_owned(), signer.public()],
    ] {
        let broken = Updater {
            keys,
            ..curl.updater("0.6.1")
        };
        let (result, _) = run(&broken, &install.target(), &exactly("v0.7.0"));
        let err = result.unwrap_err();
        assert!(
            err.contains("signing key of this build does not parse"),
            "{err}"
        );
    }
    assert!(!curl.urls().contains(&release.binary_url()));
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
    assert!(!curl.ran());
}

/// [`SIGNED_SINCE`] splits the versions where the contract says it does.
#[test]
fn signed_since_orders_pre_releases() {
    assert_eq!(SIGNED_SINCE, "0.7.0-rc.1");
    for signed in [
        "0.7.0-rc.1",
        "0.7.0-rc.2",
        "0.7.0-rc.10",
        "0.7.0",
        "0.7.1-alpha.1",
        "0.7.1",
        "0.8.0",
        "1.0.0-rc.1",
        "1.0.0",
    ] {
        assert!(is_signed(&v(signed)), "{signed}");
    }
    for unsigned in [
        "0.6.1",
        "0.6.99",
        "0.7.0-alpha.1",
        "0.7.0-beta.9",
        "0.7.0-rc",
        "0.7.0-rc.0",
        "0.1.0",
    ] {
        assert!(!is_signed(&v(unsigned)), "{unsigned}");
    }
    assert_eq!(v(SIGNED_SINCE).tag(), "v0.7.0-rc.1");
}

/// The first signed version takes the signed way with its full tag.
#[test]
fn the_first_signed_version_is_signed_under_its_pre_release_tag() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    let release = curl.serve_signed("0.7.0-rc.1", &signer, ASSET);
    let install = Install::new("0.6.1", 0o755);
    let (result, out) = run(
        &curl.trusting("0.6.1", &[&signer]),
        &install.target(),
        &exactly("v0.7.0-rc.1"),
    );
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert!(out.contains("Signature OK for v0.7.0-rc.1."), "{out}");
    assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
    assert!(!curl.urls().contains(&release.sha256_url()));
}

/// Older than `SIGNED_SINCE`: refused, by name of the flag, before any network
/// call, `--force` or not. An old release through the lookup is no way round.
#[test]
fn an_old_target_needs_allow_unsigned() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    curl.serve_release("v0.6.1", "tower 0.6.1", None);
    let install = Install::new("0.7.0", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let updater = curl.trusting("0.7.0", &[&signer]);

    for request in [
        exactly("v0.6.1"),
        Request {
            force: true,
            ..exactly("v0.6.1")
        },
    ] {
        let (result, _) = run(&updater, &install.target(), &request);
        let err = result.unwrap_err();
        assert_eq!(
            err,
            "v0.6.1 has no signature, signed releases start at v0.7.0-rc.1; pass \
             --allow-unsigned to install it on its checksum alone; nothing was changed"
        );
    }
    assert!(curl.calls().is_empty(), "no network: {:?}", curl.calls());

    // `--force` with the lookup naming an old release, on a newer binary.
    curl.serve_latest("v0.6.1");
    let forced = Request {
        force: true,
        ..Request::default()
    };
    let (result, _) = run(&updater, &install.target(), &forced);
    assert!(result.unwrap_err().contains("--allow-unsigned"));
    assert_eq!(curl.urls(), [LATEST_PAGE]);
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
}

/// With the flag an old release takes the old `.sha256` way, and says so.
#[test]
fn allow_unsigned_uses_the_sha256_and_says_so() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let binary = curl.serve_release("v0.6.1", "tower 0.6.1", None);
    let install = Install::new("0.7.0", 0o755);
    let request = Request {
        allow_unsigned: true,
        ..exactly("v0.6.1")
    };

    // The keys do not matter here: there is nothing for them to verify.
    let (result, out) = run(&curl.trusting("0.7.0", &[]), &install.target(), &request);
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert!(
        out.contains("v0.6.1 has no signature: checking only its checksum"),
        "{out}"
    );
    assert!(
        out.contains("0.7.0 → 0.6.1 (downgrade): downloaded, no signature, checksum OK"),
        "{out}"
    );
    assert!(!out.contains("Signature OK"), "{out}");
    assert_eq!(std::fs::read(install.target()).unwrap(), binary);
    let base = format!("{RELEASE_BASE}/v0.6.1");
    assert_eq!(
        curl.urls(),
        [format!("{base}/{ASSET}.sha256"), format!("{base}/{ASSET}")]
    );

    // The checksum still counts.
    let curl = FakeCurl::new();
    curl.serve_release("v0.6.1", "tower 0.6.1", Some(&"0".repeat(64)));
    let install = Install::new("0.7.0", 0o755);
    let (result, _) = run(&curl.updater("0.7.0"), &install.target(), &request);
    assert!(result.unwrap_err().contains("checksum mismatch"));
}

/// `--allow-unsigned` does nothing for a release that must be signed: a good
/// one is verified as ever, and [`FAULTS`] covers a bad one with the flag.
#[test]
fn allow_unsigned_has_no_effect_on_a_signed_release() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    let release = curl.serve_signed("0.7.0", &signer, ASSET);
    let install = Install::new("0.6.1", 0o755);
    let request = Request {
        allow_unsigned: true,
        ..exactly("v0.7.0")
    };
    let (result, out) = run(
        &curl.trusting("0.6.1", &[&signer]),
        &install.target(),
        &request,
    );
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert!(out.contains("Signature OK"), "{out}");
    assert!(!out.contains("no signature"), "{out}");
    assert!(!curl.urls().contains(&release.sha256_url()));
}

#[test]
fn assets_per_platform() {
    assert_eq!(
        asset_for("linux", "x86_64"),
        Some("tower-x86_64-linux-musl")
    );
    assert_eq!(
        asset_for("linux", "aarch64"),
        Some("tower-aarch64-linux-musl")
    );
    assert_eq!(asset_for("macos", "aarch64"), Some("tower-arm64-macos"));
    assert_eq!(asset_for("macos", "x86_64"), Some("tower-x86_64-macos"));
    for (os, arch) in [
        ("macos", "arm64"),
        ("macos", "riscv64"),
        ("linux", "arm64"),
        ("linux", "riscv64"),
        ("linux", "arm"),
        ("windows", "x86_64"),
    ] {
        assert_eq!(asset_for(os, arch), None, "{os} {arch}");
        assert!(!platform_supported(os, arch), "{os} {arch}");
    }
}

/// The aarch64 binary is what an aarch64 tower asks for and looks up.
#[test]
fn the_aarch64_asset_is_requested_on_aarch64() {
    let _guard = lock();
    let curl = FakeCurl::new();
    let signer = Signer::new();
    curl.serve_latest("v0.7.0");
    let release = curl.serve_signed("0.7.0", &signer, ARM_ASSET);
    let install = Install::new("0.6.1", 0o755);
    let arm = Updater {
        platform: ("linux", "aarch64"),
        ..curl.trusting("0.6.1", &[&signer])
    };
    let (result, _) = run(&arm, &install.target(), &Request::default());
    assert!(
        matches!(result, Ok(Outcome::Installed { .. })),
        "{result:?}"
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), release.binary);
    let urls = curl.urls();
    assert_eq!(
        urls.last().unwrap(),
        &format!("{RELEASE_BASE}/v0.7.0/{ARM_ASSET}")
    );
    assert!(!urls.iter().any(|u| u.ends_with(ASSET)), "{urls:?}");
}

/// No release before `SIGNED_SINCE` has an aarch64 binary, so naming the flag
/// there would only lead to a 404.
#[test]
fn an_old_target_has_no_aarch64_build() {
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_release("v0.6.1", "tower 0.6.1", None);
    let install = Install::new("0.7.0", 0o755);
    let arm = Updater {
        platform: ("linux", "aarch64"),
        ..curl.updater("0.7.0")
    };
    let request = Request {
        allow_unsigned: true,
        ..exactly("v0.6.1")
    };
    for request in [exactly("v0.6.1"), request] {
        let (result, _) = run(&arm, &install.target(), &request);
        let err = result.unwrap_err();
        assert_eq!(
            err,
            "v0.6.1 has no aarch64 build, the first is v0.7.0-rc.1; nothing was changed"
        );
    }
    assert!(curl.calls().is_empty());
}

/// Without curl the way around it must not need curl, and names this
/// platform's binary.
#[test]
fn the_missing_curl_message_does_not_suggest_curl() {
    let _guard = lock();
    let no_curl = |arch: &'static str| Updater {
        path: Some(OsString::from("/nonexistent-dir")),
        platform: ("linux", arch),
        keys: vec![Signer::new().public()],
        current: v("0.6.1"),
        check_format: false,
    };

    let err = no_curl("aarch64").check().unwrap_err().to_string();
    assert_eq!(
        err,
        "cannot look up the latest release: `curl` is not installed or not on PATH; install it \
         and run this again, or download \
         https://github.com/ruslan1us/watchtower/releases/latest/download/tower-aarch64-linux-musl \
         some other way, save it as ~/.local/bin/tower and `chmod +x` it"
    );

    // No binary to name on a platform without one.
    let err = no_curl("riscv64").check().unwrap_err().to_string();
    assert!(err.ends_with("install it and run this again"), "{err}");

    // `tower update --version` meets it at the first download.
    let install = Install::new("0.6.1", 0o755);
    let before = std::fs::read(install.target()).unwrap();
    let (result, _) = run(&no_curl("x86_64"), &install.target(), &exactly("v0.7.0"));
    let err = result.unwrap_err();
    assert!(
        err.contains("cannot download v0.7.0 SHA256SUMS: `curl` is not"),
        "{err}"
    );
    assert!(
        err.contains("download/tower-x86_64-linux-musl some other way"),
        "{err}"
    );
    assert!(!err.contains("curl -"), "{err}");
    assert!(err.ends_with("nothing was changed"), "{err}");
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);

    // macOS always has it, so only `PATH` can be wrong. No download to suggest:
    // a browser's copy would be quarantined.
    for platform in [MAC_ARM, MAC_X86] {
        let mac = Updater {
            platform,
            ..no_curl("x86_64")
        };
        let err = mac.check().unwrap_err().to_string();
        assert_eq!(
            err,
            "cannot look up the latest release: `curl` is not installed or not on PATH; macOS \
             includes it as /usr/bin/curl, so add /usr/bin to PATH and run this again"
        );
        let (result, _) = run(&mac, &install.target(), &exactly("v0.7.1"));
        let err = result.unwrap_err();
        assert!(
            err.contains("cannot download v0.7.1 SHA256SUMS: `curl` is not"),
            "{err}"
        );
        assert!(!err.contains("some other way"), "{err}");
        assert!(err.ends_with("nothing was changed"), "{err}");
    }
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
}

/// The real binary, copied out of `target/` so it is no cargo build output: an
/// old release is refused by flag name, and the flag reaches the update. Its
/// real ELF check then stops the fake release, so nothing is replaced.
#[test]
fn binary_passes_allow_unsigned_through() {
    // Elsewhere every release is signed: the test above covers what is older.
    if !(cfg!(target_os = "linux") && cfg!(target_arch = "x86_64")) {
        eprintln!("skipped: no release older than the signed ones for this platform");
        return;
    }
    let _guard = lock();
    let curl = FakeCurl::new();
    curl.serve_release("v0.6.1", "tower 0.6.1", None);
    let install = Install::new("0.6.1", 0o755);
    std::fs::copy(env!("CARGO_BIN_EXE_tower"), install.target()).unwrap();
    let before = std::fs::read(install.target()).unwrap();
    let tower = |args: &[&str]| {
        Command::new(install.target())
            .args(args)
            .env_clear()
            .env("PATH", curl.path())
            .env("HOME", curl.dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };

    let out = tower(&["update", "--version", "v0.6.1"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--allow-unsigned"), "{stderr}");
    assert!(curl.calls().is_empty(), "no network: {:?}", curl.calls());

    let out = tower(&["update", "--version", "v0.6.1", "--allow-unsigned"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("v0.6.1 has no signature"), "{stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not an x86-64 Linux executable"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(install.target()).unwrap(), before);
    assert_eq!(install.bin_entries(), ["tower"]);
}

// ---- the pinned keys -------------------------------------------------------

/// The keys a real build trusts are the pinned ones.
#[test]
fn the_default_updater_trusts_the_pinned_keys() {
    assert_eq!(Updater::new().keys, PINNED_KEYS);
}

/// The lines the release workflow's grep finds in the source (a quoted literal
/// alone on its line) are exactly the pinned keys.
fn key_lines_in_the_source() -> Vec<String> {
    include_str!("../src/update.rs")
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("\"RW"))
        .map(|line| line.trim_end_matches(',').trim_matches('"').to_owned())
        .collect()
}

#[test]
fn pinned_keys_keep_the_form_the_release_workflow_reads() {
    assert_eq!(key_lines_in_the_source(), PINNED_KEYS);
}

/// The release gate: the release workflow runs this by name, so no release
/// ships without both keys.
#[test]
#[ignore = "the release workflow runs it by name"]
fn pinned_keys_are_ready_for_release() {
    assert_eq!(
        PINNED_KEYS.len(),
        2,
        "src/update.rs must pin exactly two release signing keys"
    );
    let mut ids = Vec::new();
    for key in PINNED_KEYS {
        minisign_verify::PublicKey::from_base64(key).expect("a pinned key parses");
        let parsed = minisign::PublicKey::from_base64(key).expect("a pinned key parses");
        ids.push(parsed.keynum().to_vec());
    }
    assert_ne!(ids[0], ids[1], "the two pinned keys need distinct key ids");
    assert_eq!(key_lines_in_the_source(), PINNED_KEYS);
}
