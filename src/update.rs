//! `tower update` (doc/0023): replaces the running binary with a release
//! from GitHub, and `tower update --check`, which only compares versions.
//!
//! This is the only place tower uses the network, and only when one of
//! these commands is typed: nothing checks for updates on its own. The
//! latest version comes from the redirect of the `releases/latest` web page,
//! with the GitHub API as the fallback (doc/0026). Every
//! request goes through the system `curl`, run directly (no shell) with
//! HTTPS-only, TLS 1.2+ options and `-q`, so a `~/.curlrc` cannot weaken
//! them. Tower links no HTTP or TLS code.
//!
//! An update downloads the release asset and its `.sha256` into a fresh
//! temp dir next to the target binary, verifies the SHA-256, checks that the
//! file is an x86-64 ELF executable and that it runs and prints the expected
//! `tower X.Y.Z`, and only then gives it the old binary's mode and owner and
//! renames it over the target, so the swap is atomic. On any failure the
//! old binary is untouched and the temp dir is removed.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// The release asset: the static musl binary (doc/0012).
pub const ASSET: &str = "tower-x86_64-linux-musl";

/// The web page of the newest (non-draft, non-pre-release) release. GitHub
/// answers it with a redirect to `<RELEASE_TAG_PREFIX><tag>`, or to
/// [`RELEASES_PAGE`] while the repository has no release. Unlike the REST
/// API it is not subject to the 60-requests-per-hour limit per address.
pub const LATEST_PAGE: &str = "https://github.com/ruslan1us/watchtower/releases/latest";

/// What [`LATEST_PAGE`] redirects to, followed by the tag.
pub const RELEASE_TAG_PREFIX: &str = "https://github.com/ruslan1us/watchtower/releases/tag/";

/// Where [`LATEST_PAGE`] redirects while there is no release.
pub const RELEASES_PAGE: &str = "https://github.com/ruslan1us/watchtower/releases";

/// The GitHub API endpoint for the newest (non-draft, non-pre-release)
/// release: the fallback when [`LATEST_PAGE`] gives no answer.
pub const LATEST_API: &str = "https://api.github.com/repos/ruslan1us/watchtower/releases/latest";

/// Versioned downloads live under `<DOWNLOAD_BASE>/vX.Y.Z/<asset>`.
pub const DOWNLOAD_BASE: &str = "https://github.com/ruslan1us/watchtower/releases/download";

/// The stable URL of the newest release's binary, for the manual command.
pub const LATEST_DOWNLOAD: &str =
    "https://github.com/ruslan1us/watchtower/releases/latest/download/tower-x86_64-linux-musl";

/// Exit code of `tower update --check` when a newer release exists.
pub const EXIT_UPDATE_AVAILABLE: u8 = 10;

/// `curl --max-time` for every request, in seconds.
pub const CURL_MAX_TIME_SECS: u32 = 120;

/// How long the downloaded binary may take to print its version.
const RUN_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Mode for the new binary when the old one's cannot be read.
const DEFAULT_MODE: u32 = 0o755;

/// The arguments `curl` gets before `-o` and the URL: `-q` first (ignore
/// `~/.curlrc`), fail on HTTP errors, quiet but show errors, follow
/// redirects, HTTPS only (redirects included), TLS 1.2 or newer, a time cap.
pub fn curl_args() -> Vec<String> {
    [
        "-q",
        "-fsSL",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--max-time",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([CURL_MAX_TIME_SECS.to_string()])
    .collect()
}

/// The arguments `curl` gets before [`LATEST_PAGE`]: the same `-q`, fail on
/// HTTP errors, HTTPS-only, TLS and time options as [`curl_args`], but no
/// `-L`. The one redirect GitHub answers with is the result: `-w
/// %{redirect_url}` prints its target on stdout, and `-o /dev/null` drops
/// the few bytes of the 302 body. Following it (`-L -w %{url_effective}`)
/// would download the whole release page for nothing, and a HEAD request
/// (`-I`) is a method proxies and GitHub treat less uniformly than GET.
/// Without `-L` no redirect is followed, so `--proto-redir` is moot.
pub fn lookup_args() -> Vec<String> {
    ["-q", "-fsS", "--proto", "=https", "--tlsv1.2", "--max-time"]
        .iter()
        .map(|s| (*s).to_owned())
        .chain([CURL_MAX_TIME_SECS.to_string()])
        .chain(["-o", "/dev/null", "-w", "%{redirect_url}"].map(str::to_owned))
        .collect()
}

/// The release version from the target of [`LATEST_PAGE`]'s redirect. The
/// URL must be exactly `<RELEASE_TAG_PREFIX>vX.Y.Z[-PRE]`; a redirect to
/// [`RELEASES_PAGE`] means no release exists yet, and anything else is an
/// error.
pub fn parse_latest_redirect(url: &str) -> Result<Version, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(format!("{LATEST_PAGE} did not redirect to a release"));
    }
    if url == RELEASES_PAGE || url.strip_prefix(RELEASES_PAGE) == Some("/") {
        return Err("no release found".to_owned());
    }
    let Some(tag) = url.strip_prefix(RELEASE_TAG_PREFIX) else {
        return Err(format!(
            "{LATEST_PAGE} redirected to an unexpected URL `{}`",
            crate::attach::printable(url)
        ));
    };
    match Version::parse(tag) {
        // The tag must be the exact `vX.Y.Z` the download URLs are built from.
        Some(v) if v.tag() == tag => Ok(v),
        _ => Err(format!(
            "the latest release tag `{}` is not a version",
            crate::attach::printable(tag)
        )),
    }
}

/// The URL of a release file (`ASSET` or `ASSET.sha256`) of version `v`.
pub fn download_url(v: &Version, file: &str) -> String {
    format!("{DOWNLOAD_BASE}/{}/{file}", v.tag())
}

/// The manual install command from the README, writing to `target`.
pub fn manual_command(target: &str) -> String {
    format!("curl -Lo {target} {LATEST_DOWNLOAD} && chmod +x {target}")
}

// ---- versions ------------------------------------------------------------

/// A semantic version `MAJOR.MINOR.PATCH[-PRE]` (build metadata ignored).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release identifiers (`1.0.0-rc.1` → `["rc", "1"]`).
    pub pre: Vec<String>,
}

impl Version {
    /// Parses `1.2.3`, `v1.2.3`, `1.2.3-rc.1` or `1.2.3+build`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(core, _)| core);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut number = || -> Option<u64> {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let (major, minor, patch) = (number()?, number()?, number()?);
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_owned).collect();
                let valid = |id: &String| {
                    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                };
                if !ids.iter().all(valid) {
                    return None;
                }
                ids
            }
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// The version this binary was built as (`CARGO_PKG_VERSION`).
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is semver")
    }

    /// The release tag, `vX.Y.Z`.
    pub fn tag(&self) -> String {
        format!("v{self}")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Version {
    /// Semver precedence: numbers first; a pre-release sorts before its
    /// release; pre-release identifiers compare numerically when both are
    /// numbers, numbers before words, and a shorter list first.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let core =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if core != Ordering::Equal {
            return core;
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (a, b) in self.pre.iter().zip(&other.pre) {
            let numeric = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
            let order = match (numeric(a), numeric(b)) {
                (true, true) => {
                    let (x, y) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                    x.len()
                        .cmp(&y.len())
                        .then_with(|| x.cmp(y))
                        .then_with(|| a.cmp(b))
                }
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => a.cmp(b),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ---- errors and results --------------------------------------------------

/// Why an update or check failed. The message is complete and says whether
/// anything was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError(pub String);

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpdateError {}

fn err<T>(message: impl Into<String>) -> Result<T, UpdateError> {
    Err(UpdateError(message.into()))
}

/// The result of `tower update --check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub current: Version,
    pub latest: Version,
}

impl Check {
    /// Whether the latest release is newer than this binary.
    pub fn available(&self) -> bool {
        self.latest > self.current
    }

    /// The line `--check` prints.
    pub fn line(&self) -> String {
        if self.available() {
            format!(
                "{} available (running {})",
                self.latest.tag(),
                self.current.tag()
            )
        } else if self.latest < self.current {
            format!(
                "up to date ({}; latest release {})",
                self.current.tag(),
                self.latest.tag()
            )
        } else {
            format!("up to date ({})", self.current.tag())
        }
    }

    /// 0 when up to date, [`EXIT_UPDATE_AVAILABLE`] when not.
    pub fn exit_code(&self) -> u8 {
        if self.available() {
            EXIT_UPDATE_AVAILABLE
        } else {
            0
        }
    }
}

/// What `tower update` was asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// `--version`: install exactly this release (reinstall or downgrade
    /// allowed); `None` means the latest.
    pub version: Option<Version>,
    /// `--force`: replace a cargo build output, or reinstall the version
    /// already running.
    pub force: bool,
}

/// What `tower update` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Already on the latest release; nothing downloaded.
    UpToDate(Version),
    /// `to` replaced `from` at `path`.
    Installed {
        from: Version,
        to: Version,
        path: PathBuf,
    },
}

// ---- the target ----------------------------------------------------------

/// Whether this OS and architecture have a release binary (Linux x86_64).
pub fn platform_supported(os: &str, arch: &str) -> bool {
    os == "linux" && arch == "x86_64"
}

/// Whether `path` looks like a cargo build output: a file under
/// `target/debug/`, `target/release/` or `target/<triple>/…`
/// (e.g. `target/x86_64-unknown-linux-musl/release/`).
pub fn looks_like_dev_build(path: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = path
        .parent()
        .map(|dir| dir.iter().collect())
        .unwrap_or_default();
    parts.iter().enumerate().any(|(i, part)| {
        *part == "target"
            && parts.get(i + 1).is_some_and(|next| {
                let next = next.to_string_lossy();
                next == "debug" || next == "release" || next.matches('-').count() >= 2
            })
    })
}

/// The running binary with symlinks resolved: the file an update replaces.
pub fn current_target() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|e| UpdateError(format!("cannot find the running binary: {e}")))?;
    fs::canonicalize(&exe).map_err(|e| {
        UpdateError(format!(
            "cannot resolve the running binary {}: {e}",
            exe.display()
        ))
    })
}

// ---- the updater ---------------------------------------------------------

/// Runs checks and updates.
#[derive(Debug, Clone)]
pub struct Updater {
    /// The running version, compared with the releases.
    pub current: Version,
    /// `PATH` for finding `curl`; `None` inherits tower's.
    pub path: Option<OsString>,
    /// Whether the download must be an x86-64 ELF executable. Always on
    /// outside tests, which use a shell script as the fake release.
    #[doc(hidden)]
    pub require_elf: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Updater {
            current: Version::current(),
            path: None,
            require_elf: true,
        }
    }
}

impl Updater {
    /// The running version, the inherited `PATH`, every check enforced.
    pub fn new() -> Updater {
        Updater::default()
    }

    /// The newest release's version: from the redirect of [`LATEST_PAGE`],
    /// or, when that fails, from the GitHub API ([`LATEST_API`]). When both
    /// fail the error names both reasons and how to skip the lookup.
    pub fn latest(&self) -> Result<Version, UpdateError> {
        let web = match self.run_curl(&lookup_args(), LATEST_PAGE) {
            Ok(out) => parse_latest_redirect(&String::from_utf8_lossy(&out)),
            // The API needs curl too: report this once, with the manual way.
            Err(CurlError::NotInstalled(e)) => {
                return err(format!("cannot look up the latest release: {e}"));
            }
            Err(e) => Err(e.to_string()),
        };
        let web_err = match web {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        self.latest_from_api().map_err(|api_err| {
            UpdateError(format!(
                "cannot look up the latest release: releases page: {web_err}; \
                 GitHub API: {api_err}; `tower update --version vX.Y.Z` skips the lookup"
            ))
        })
    }

    /// The newest release's version from the GitHub API.
    fn latest_from_api(&self) -> Result<Version, String> {
        let body = self
            .run_curl(&curl_args(), LATEST_API)
            .map_err(|e| match e {
                CurlError::Failed {
                    status: Some(403 | 429),
                    ..
                } => format!(
                    "GitHub's API rate limit for this network address is used up, \
                     try again later ({e})"
                ),
                e => e.to_string(),
            })?;
        let json: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("the answer is not JSON ({e})"))?;
        let Some(tag) = json.get("tag_name").and_then(|t| t.as_str()) else {
            return Err("the answer has no tag_name".to_owned());
        };
        Version::parse(tag)
            .ok_or_else(|| format!("tag `{}` is not a version", crate::attach::printable(tag)))
    }

    /// `tower update --check`: compares the latest release with this binary.
    pub fn check(&self) -> Result<Check, UpdateError> {
        Ok(Check {
            current: self.current.clone(),
            latest: self.latest()?,
        })
    }

    /// `tower update`: replaces `target` with the requested release.
    /// Progress and the result go to `out`.
    pub fn update(
        &self,
        target: &Path,
        request: &Request,
        out: &mut dyn Write,
    ) -> Result<Outcome, UpdateError> {
        use std::env::consts::{ARCH, OS};
        if !platform_supported(OS, ARCH) {
            return err(format!(
                "releases exist only for Linux x86_64, not {OS} {ARCH}; build from source instead"
            ));
        }
        if !request.force && looks_like_dev_build(target) {
            return err(format!(
                "{} looks like a cargo build output; rebuild it with cargo, \
                 or pass --force to replace it with a release",
                target.display()
            ));
        }
        let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) else {
            return err(format!("{} has no parent directory", target.display()));
        };
        let old = fs::metadata(target)
            .map_err(|e| UpdateError(format!("cannot read {}: {e}", target.display())))?;

        let current = &self.current;
        let to = match &request.version {
            Some(v) => v.clone(),
            None => {
                let latest = self.latest()?;
                if latest == *current && !request.force {
                    let _ = writeln!(out, "tower is up to date ({})", current.tag());
                    return Ok(Outcome::UpToDate(latest));
                }
                if latest < *current && !request.force {
                    return err(format!(
                        "running {} is newer than the latest release {}; nothing was changed \
                         (use --version {} to downgrade)",
                        current.tag(),
                        latest.tag(),
                        latest.tag()
                    ));
                }
                latest
            }
        };

        let tmp = TempDir::create(dir).map_err(|e| {
            if is_permission_error(&e) {
                UpdateError(format!(
                    "cannot write {}: run as its owner (e.g. sudo tower update)",
                    target.display()
                ))
            } else {
                UpdateError(format!(
                    "cannot create a temp dir in {}: {e}",
                    dir.display()
                ))
            }
        })?;

        let note = if to == *current {
            " (reinstall)"
        } else if to < *current {
            " (downgrade)"
        } else {
            ""
        };
        let _ = writeln!(out, "Downloading tower {}{note}…", to.tag());

        let sum_name = format!("{ASSET}.sha256");
        let sum_path = tmp.path().join(&sum_name);
        let new_path = tmp.path().join(ASSET);
        let unchanged = |e: String| UpdateError(format!("{e}; nothing was changed"));
        self.curl(&download_url(&to, &sum_name), Some(&sum_path))
            .map_err(|e| unchanged(format!("cannot download {} {sum_name}: {e}", to.tag())))?;
        self.curl(&download_url(&to, ASSET), Some(&new_path))
            .map_err(|e| unchanged(format!("cannot download {} {ASSET}: {e}", to.tag())))?;

        let sum_text = fs::read_to_string(&sum_path)
            .map_err(|e| unchanged(format!("cannot read {sum_name}: {e}")))?;
        let expected = parse_checksum(&sum_text, ASSET).map_err(unchanged)?;
        let actual = sha256_file(&new_path)
            .map_err(|e| unchanged(format!("cannot read the download: {e}")))?;
        if actual != expected {
            return Err(unchanged(format!(
                "checksum mismatch for {} {ASSET}: expected {expected}, got {actual}",
                to.tag()
            )));
        }
        if self.require_elf {
            check_elf(&new_path).map_err(unchanged)?;
        }
        set_mode(&new_path, 0o700)
            .map_err(|e| unchanged(format!("cannot make the download executable: {e}")))?;
        check_runs(&new_path, &to).map_err(unchanged)?;

        // The old binary's owner (as far as allowed) and mode.
        {
            use std::os::unix::fs::MetadataExt;
            let (uid, gid) = (old.uid(), old.gid());
            if let Ok(new) = fs::metadata(&new_path)
                && (new.uid(), new.gid()) != (uid, gid)
            {
                let _ = std::os::unix::fs::chown(&new_path, Some(uid), Some(gid))
                    .or_else(|_| std::os::unix::fs::chown(&new_path, None, Some(gid)));
            }
        }
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            match old.permissions().mode() & 0o777 {
                0 => DEFAULT_MODE,
                m => m,
            }
        };
        set_mode(&new_path, mode)
            .map_err(|e| unchanged(format!("cannot set the download's mode: {e}")))?;
        if let Ok(f) = fs::File::open(&new_path) {
            let _ = f.sync_all();
        }
        fs::rename(&new_path, target).map_err(|e| {
            if is_permission_error(&e) {
                UpdateError(format!(
                    "cannot write {}: run as its owner (e.g. sudo tower update)",
                    target.display()
                ))
            } else {
                unchanged(format!("cannot replace {}: {e}", target.display()))
            }
        })?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        drop(tmp);

        let _ = writeln!(
            out,
            "tower {current} → {to}{note}: downloaded, checksum OK, installed to {}",
            target.display()
        );
        let _ = writeln!(
            out,
            "Running tower instances keep the old version until restarted."
        );
        Ok(Outcome::Installed {
            from: current.clone(),
            to,
            path: target.to_owned(),
        })
    }

    /// Runs `curl <curl_args> [-o dest] url`. Without `dest`, returns the
    /// body from stdout; with it, an empty vector.
    fn curl(&self, url: &str, dest: Option<&Path>) -> Result<Vec<u8>, String> {
        let mut args: Vec<OsString> = curl_args().into_iter().map(OsString::from).collect();
        if let Some(dest) = dest {
            args.push("-o".into());
            args.push(dest.into());
        }
        self.run_curl(&args, url).map_err(|e| e.to_string())
    }

    /// Runs `curl <args> <url>` directly (no shell) and returns its stdout.
    fn run_curl<A: AsRef<std::ffi::OsStr>>(
        &self,
        args: &[A],
        url: &str,
    ) -> Result<Vec<u8>, CurlError> {
        let mut cmd = Command::new("curl");
        cmd.args(args)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CurlError::NotInstalled(format!(
                    "`curl` is not installed or not on PATH; install it, or update by hand:\n  {}",
                    manual_command("~/.local/bin/tower")
                )));
            }
            Err(e) => {
                return Err(CurlError::Failed {
                    message: format!("cannot run curl: {e}"),
                    status: None,
                });
            }
        };
        if !output.status.success() {
            let raw = String::from_utf8_lossy(&output.stderr);
            let status = http_status(&raw);
            let stderr = crate::attach::printable(raw.trim());
            let code = output
                .status
                .code()
                .map_or_else(|| "killed".to_owned(), |c| format!("exit {c}"));
            let message = if stderr.is_empty() {
                format!("curl failed for {url} ({code})")
            } else {
                format!("{stderr} ({url}, curl {code})")
            };
            return Err(CurlError::Failed { message, status });
        }
        Ok(output.stdout)
    }
}

/// Why a `curl` run failed.
#[derive(Debug)]
enum CurlError {
    /// No `curl` on `PATH`; the message has the manual install command.
    NotInstalled(String),
    /// curl ran and failed; `status` is the HTTP status `-f` reported.
    Failed {
        message: String,
        status: Option<u16>,
    },
}

impl fmt::Display for CurlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurlError::NotInstalled(message) | CurlError::Failed { message, .. } => {
                f.write_str(message)
            }
        }
    }
}

/// The HTTP status from curl's `-f` message, `curl: (22) The requested URL
/// returned error: 403` (older curl versions add the reason phrase).
fn http_status(stderr: &str) -> Option<u16> {
    let (_, rest) = stderr.split_once("returned error: ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() == 3 {
        digits.parse().ok()
    } else {
        None
    }
}

// ---- checks --------------------------------------------------------------

/// The lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash from a `sha256sum` line (`<hex>  <name>` or `<hex> *<name>`)
/// for `name`.
pub fn parse_checksum(text: &str, name: &str) -> Result<String, String> {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut fields = line.split_whitespace();
    let hash = fields.next().unwrap_or("");
    let file = fields.next().map(|f| f.trim_start_matches('*'));
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("the checksum file has no SHA-256 for {name}"));
    }
    if file.is_some_and(|f| f != name) {
        return Err(format!("the checksum file is for another file, not {name}"));
    }
    Ok(hash.to_ascii_lowercase())
}

/// The ELF header of an x86-64 executable (`ET_EXEC`, or `ET_DYN` for a
/// static PIE), 64-bit little-endian.
pub fn is_elf_x86_64(header: &[u8]) -> bool {
    header.len() >= 20
        && header[..4] == *b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        && u16::from_le_bytes([header[18], header[19]]) == 62
}

fn check_elf(path: &Path) -> Result<(), String> {
    let mut header = [0u8; 20];
    let read = fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| format!("cannot read the download: {e}"));
    if read.is_err() || !is_elf_x86_64(&header) {
        return Err("the download is not an x86-64 Linux executable".to_owned());
    }
    Ok(())
}

/// Runs `<path> --version` and checks that it prints `tower <expected>`.
fn check_runs(path: &Path, expected: &Version) -> Result<(), String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run the downloaded binary: {e}"))?;
    let deadline = Instant::now() + RUN_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match other {
                    Err(e) => format!("cannot run the downloaded binary: {e}"),
                    _ => "the downloaded binary did not answer --version in time".to_owned(),
                });
            }
        }
    };
    let mut stdout = String::new();
    if let Some(pipe) = child.stdout.take() {
        let _ = pipe.take(4096).read_to_string(&mut stdout);
    }
    let want = format!("tower {expected}");
    if !status.success() || stdout.trim() != want {
        return Err(format!(
            "the downloaded binary reports `{}`, expected `{want}`",
            crate::attach::printable(stdout.trim())
        ));
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn is_permission_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(libc::EROFS)
}

/// A private temp dir next to the target, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create(parent: &Path) -> io::Result<TempDir> {
        use std::os::unix::fs::DirBuilderExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let mut last = None;
        for attempt in 0..16u32 {
            let name = format!(
                ".tower-update.{}.{}",
                std::process::id(),
                nanos.wrapping_add(attempt)
            );
            let path = parent.join(name);
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(TempDir(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("no free temp dir name")))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_header() {
        let mut h = [0u8; 20];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[5] = 1;
        h[16] = 3;
        h[18] = 62;
        assert!(is_elf_x86_64(&h));
        h[16] = 2;
        assert!(is_elf_x86_64(&h));
        h[18] = 183; // aarch64
        assert!(!is_elf_x86_64(&h));
        assert!(!is_elf_x86_64(b"#!/bin/sh\necho tower 1"));
    }

    #[test]
    fn http_status_from_curl_messages() {
        let new = "curl: (22) The requested URL returned error: 403";
        assert_eq!(http_status(new), Some(403));
        let old = "curl: (22) The requested URL returned error: 429 Too Many Requests";
        assert_eq!(http_status(old), Some(429));
        assert_eq!(http_status("curl: (6) Could not resolve host: x"), None);
        assert_eq!(http_status("returned error: 40"), None);
    }

    #[test]
    fn checksum_lines() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  {ASSET}\n"), ASSET),
            Ok(hash.clone())
        );
        assert_eq!(
            parse_checksum(&format!("{}  *{ASSET}\n", hash.to_uppercase()), ASSET),
            Ok(hash.clone())
        );
        assert!(parse_checksum(&format!("{hash}  other\n"), ASSET).is_err());
        assert!(parse_checksum("abc  x\n", ASSET).is_err());
        assert!(parse_checksum("", ASSET).is_err());
    }
}
