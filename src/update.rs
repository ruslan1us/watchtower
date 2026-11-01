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
//! An update works in a fresh temp dir next to the target binary. From
//! [`SIGNED_SINCE`] on it downloads `SHA256SUMS` and its minisign signature,
//! verifies the signature against [`PINNED_KEYS`] and that the signature's
//! trusted comment names exactly this release, and only then downloads the
//! binary and compares it with its line in `SHA256SUMS`. An older release has
//! no signature and is refused unless `--allow-unsigned` asks for its
//! `.sha256` alone. Either way it then checks that the file is an ELF
//! executable for this architecture, and that it runs and prints the expected
//! `tower X.Y.Z`, and only then gives it the old binary's mode and owner and
//! renames it over the target, so the swap is atomic. Nothing downloaded runs
//! before those checks pass. On any failure the old binary is untouched and
//! the temp dir is removed.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// The release's checksum manifest: plain `sha256sum` lines for its binaries.
pub const MANIFEST: &str = "SHA256SUMS";

/// The minisign signature of [`MANIFEST`].
pub const MANIFEST_SIGNATURE: &str = "SHA256SUMS.minisig";

/// The first version whose release is signed. This one and everything semver
/// orders after it must have a good signature; anything before it has none.
pub const SIGNED_SINCE: &str = "0.7.0-rc.1";

/// The public keys a release signature may come from. Two exist so that one
/// can be rotated out: a signature from either is accepted. An empty list
/// fails closed, and the release workflow refuses to ship with fewer than two
/// (`pinned_keys_are_ready_for_release` in tests/update.rs). It stays empty
/// until the owner generates the keys.
///
/// The release workflow reads this list with a grep, so each key must stay a
/// plain quoted literal alone on its own line, with no other text there.
pub const PINNED_KEYS: &[&str] = &[];

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

/// The stable URLs of the newest release's files are `<LATEST_DOWNLOAD>/<asset>`.
pub const LATEST_DOWNLOAD: &str =
    "https://github.com/ruslan1us/watchtower/releases/latest/download";

/// Exit code of `tower update --check` when a newer release exists.
pub const EXIT_UPDATE_AVAILABLE: u8 = 10;

/// `curl --max-time` for every request, in seconds.
pub const CURL_MAX_TIME_SECS: u32 = 120;

/// How long the downloaded binary may take to print its version.
const RUN_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Mode for the new binary when the old one's cannot be read.
const DEFAULT_MODE: u32 = 0o755;

/// The most `SHA256SUMS` or its signature may hold: they are read into memory
/// before anything vouches for them, and both are a few lines long.
const MAX_SIGNED_FILE_BYTES: u64 = 64 * 1024;

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

/// The URL of a release file (an asset, `SHA256SUMS`, …) of version `v`.
pub fn download_url(v: &Version, file: &str) -> String {
    format!("{DOWNLOAD_BASE}/{}/{file}", v.tag())
}

/// What to do without `curl` on this platform. The way around it must not
/// need `curl` either.
fn curl_missing(os: &str, arch: &str) -> String {
    let install = "`curl` is not installed or not on PATH; install it and run this again";
    match asset_for(os, arch) {
        Some(asset) => format!(
            "{install}, or download {LATEST_DOWNLOAD}/{asset} some other way, \
             save it as ~/.local/bin/tower and `chmod +x` it"
        ),
        None => install.to_owned(),
    }
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
    /// already running. It never skips a verification.
    pub force: bool,
    /// `--allow-unsigned`: install a release older than [`SIGNED_SINCE`] on
    /// its `.sha256` alone. No effect on a release that must be signed.
    pub allow_unsigned: bool,
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

/// The release binary for this OS and architecture, as in `std::env::consts`.
pub fn asset_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("tower-x86_64-linux-musl"),
        ("linux", "aarch64") => Some("tower-aarch64-linux-musl"),
        _ => None,
    }
}

/// Whether this OS and architecture have a release binary.
pub fn platform_supported(os: &str, arch: &str) -> bool {
    asset_for(os, arch).is_some()
}

/// Whether a release must be signed: [`SIGNED_SINCE`] and everything after it.
pub fn is_signed(v: &Version) -> bool {
    Version::parse(SIGNED_SINCE).is_some_and(|since| *v >= since)
}

/// Why `tower update` and `--check` stop at once on an OS tower runs on but has
/// no release for, however new the latest release is (doc/0049). A Linux binary
/// would not run there, and "newer release available" would promise an update
/// that cannot happen.
pub fn no_release_yet(os: &str) -> Option<&'static str> {
    (os == "macos")
        .then_some("no release for macOS yet; build from source (see doc/guide/getting-started.md)")
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
    /// Whether the download must be an ELF executable for this architecture.
    /// Always on outside tests, which use a shell script as the fake release.
    #[doc(hidden)]
    pub require_elf: bool,
    /// The OS and architecture to decide for, as in `std::env::consts`. Always
    /// the real ones outside tests, whose fake releases work on any system.
    #[doc(hidden)]
    pub platform: (&'static str, &'static str),
    /// The keys a release signature may come from. Always [`PINNED_KEYS`]
    /// outside tests, which sign with throwaway keys.
    #[doc(hidden)]
    pub keys: Vec<String>,
}

impl Default for Updater {
    fn default() -> Self {
        Updater {
            current: Version::current(),
            path: None,
            require_elf: true,
            platform: (std::env::consts::OS, std::env::consts::ARCH),
            keys: PINNED_KEYS.iter().map(|k| (*k).to_owned()).collect(),
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
            // The API needs curl too: report this once, with the way around it.
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
        if let Some(refusal) = no_release_yet(self.platform.0) {
            return err(refusal);
        }
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
        let (os, arch) = self.platform;
        if let Some(refusal) = no_release_yet(os) {
            return err(refusal);
        }
        let Some(asset) = asset_for(os, arch) else {
            return err(format!(
                "releases exist only for Linux x86_64 and aarch64, not {os} {arch}; \
                 build from source instead"
            ));
        };
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
        let signed = is_signed(&to);
        if signed && self.keys.is_empty() {
            return err(format!(
                "this build has no release signing keys, so it cannot verify {}; \
                 nothing was changed",
                to.tag()
            ));
        }
        if !signed && arch != "x86_64" {
            return err(format!(
                "{} has no {arch} build, the first is v{SIGNED_SINCE}; nothing was changed",
                to.tag()
            ));
        }
        if !signed && !request.allow_unsigned {
            return err(format!(
                "{} has no signature, signed releases start at v{SIGNED_SINCE}; \
                 pass --allow-unsigned to install it on its checksum alone; nothing was changed",
                to.tag()
            ));
        }

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

        let unchanged = |e: String| UpdateError(format!("{e}; nothing was changed"));
        let new_path = tmp.path().join(asset);
        if signed {
            self.fetch_signed(&to, asset, tmp.path(), out)
                .map_err(unchanged)?;
        } else {
            let _ = writeln!(
                out,
                "{} has no signature: checking only its checksum, as --allow-unsigned asks.",
                to.tag()
            );
            self.fetch_unsigned(&to, asset, tmp.path())
                .map_err(unchanged)?;
        }
        if self.require_elf {
            check_elf(&new_path, arch).map_err(unchanged)?;
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

        let verified = if signed {
            "signature and checksum OK"
        } else {
            "no signature, checksum OK"
        };
        let _ = writeln!(
            out,
            "tower {current} → {to}{note}: downloaded, {verified}, installed to {}",
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

    /// Downloads `SHA256SUMS` and its signature into `dir`, verifies them, and
    /// only then downloads `asset` and compares it with its line there.
    fn fetch_signed(
        &self,
        to: &Version,
        asset: &str,
        dir: &Path,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let manifest_path = dir.join(MANIFEST);
        let signature_path = dir.join(MANIFEST_SIGNATURE);
        self.download(to, MANIFEST, &manifest_path)?;
        self.download(to, MANIFEST_SIGNATURE, &signature_path)?;
        let manifest = read_signed_file(&manifest_path, MANIFEST)?;
        let signature = read_signed_file(&signature_path, MANIFEST_SIGNATURE)?;
        verify_manifest(&self.keys, &manifest, &signature, to)?;
        let _ = writeln!(out, "Signature OK for {}.", to.tag());

        let text = std::str::from_utf8(&manifest)
            .map_err(|_| format!("{MANIFEST} of {} is not text", to.tag()))?;
        let expected = manifest_hash(text, asset)?;
        self.download(to, asset, &dir.join(asset))?;
        check_hash(&dir.join(asset), &expected, to, asset)
    }

    /// Downloads `asset` and its `.sha256` into `dir` and compares them.
    fn fetch_unsigned(&self, to: &Version, asset: &str, dir: &Path) -> Result<(), String> {
        let sum_name = format!("{asset}.sha256");
        let sum_path = dir.join(&sum_name);
        self.download(to, &sum_name, &sum_path)?;
        self.download(to, asset, &dir.join(asset))?;
        let sum_text =
            fs::read_to_string(&sum_path).map_err(|e| format!("cannot read {sum_name}: {e}"))?;
        let expected = parse_checksum(&sum_text, asset)?;
        check_hash(&dir.join(asset), &expected, to, asset)
    }

    /// Downloads a file of release `to` to `dest`.
    fn download(&self, to: &Version, file: &str, dest: &Path) -> Result<(), String> {
        self.curl(&download_url(to, file), Some(dest))
            .map(|_| ())
            .map_err(|e| format!("cannot download {} {file}: {e}", to.tag()))
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
                let (os, arch) = self.platform;
                return Err(CurlError::NotInstalled(curl_missing(os, arch)));
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
    /// No `curl` on `PATH`; the message says how to do without it.
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

/// The hash on the line of a `SHA256SUMS` manifest that names exactly `name`.
/// Unlike a one-file `.sha256`, where [`parse_checksum`] allows a bare hash,
/// the manifest holds several binaries, so the name is required, whole. A
/// name given twice is refused. The errors quote nothing from the manifest.
pub fn manifest_hash(text: &str, name: &str) -> Result<String, String> {
    let mut found = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(hash), Some(file), None) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if file.strip_prefix('*').unwrap_or(file) != name {
            continue;
        }
        if found.is_some() {
            return Err(format!("{MANIFEST} has more than one line for {name}"));
        }
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{MANIFEST} has no valid SHA-256 for {name}"));
        }
        found = Some(hash.to_ascii_lowercase());
    }
    found.ok_or_else(|| format!("{MANIFEST} has no line for {name}"))
}

/// Compares the file at `path` with `expected`, a hash already checked to be
/// plain hex.
fn check_hash(path: &Path, expected: &str, to: &Version, asset: &str) -> Result<(), String> {
    let actual = sha256_file(path).map_err(|e| format!("cannot read the download: {e}"))?;
    if actual != expected {
        return Err(format!(
            "checksum mismatch for {} {asset}: expected {expected}, got {actual}",
            to.tag()
        ));
    }
    Ok(())
}

/// Reads `SHA256SUMS` or its signature, refusing what is far too long for one.
fn read_signed_file(path: &Path, name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_SIGNED_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("cannot read {name}: {e}"))?;
    if bytes.len() as u64 > MAX_SIGNED_FILE_BYTES {
        return Err(format!("{name} is too long"));
    }
    Ok(bytes)
}

/// Verifies `signature` over `manifest` with any of `keys`, strictly (no legacy
/// signatures), and then that its trusted comment names exactly release `to`,
/// so that an old release's signed manifest cannot be replayed under a newer
/// tag. The comment is checked only here, after the signature covers it.
fn verify_manifest(
    keys: &[String],
    manifest: &[u8],
    signature: &[u8],
    to: &Version,
) -> Result<(), String> {
    use minisign_verify::{Error, PublicKey, Signature};
    let tag = to.tag();
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| Signature::decode(text).ok())
        .ok_or_else(|| format!("{MANIFEST_SIGNATURE} of {tag} is not a minisign signature"))?;

    let keys = keys
        .iter()
        .map(|key| PublicKey::from_base64(key))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "a release signing key of this build does not parse".to_owned())?;

    let mut why = None;
    let mut verified = false;
    for key in &keys {
        match key.verify(manifest, &signature, false) {
            Ok(()) => {
                verified = true;
                break;
            }
            // Another key's signature: the next key may be the one.
            Err(Error::UnexpectedKeyId) => {}
            Err(Error::UnexpectedAlgorithm) => why = Some("uses the legacy format"),
            Err(_) => why = Some("does not match"),
        }
    }
    if !verified {
        return Err(match why {
            Some(why) => format!("the signature of {MANIFEST} for {tag} {why}"),
            None => format!("the signature of {MANIFEST} for {tag} is from an unknown key"),
        });
    }

    let want = format!("watchtower {tag}");
    if signature.trusted_comment() != want {
        return Err(format!(
            "the signature of {MANIFEST} is not for this release: expected `{want}`, found `{}`",
            sanitized(signature.trusted_comment())
        ));
    }
    Ok(())
}

/// Remote text cut short and reduced to plain ASCII, for an error message.
fn sanitized(text: &str) -> String {
    const LIMIT: usize = 40;
    let mut short: String = text
        .chars()
        .take(LIMIT)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if text.chars().count() > LIMIT {
        short.push('…');
    }
    short
}

/// The ELF machine number (`e_machine`) of an architecture with a release.
pub fn elf_machine(arch: &str) -> Option<u16> {
    match arch {
        "x86_64" => Some(62),
        "aarch64" => Some(183),
        _ => None,
    }
}

/// Whether `header` starts an ELF executable for `machine` (`ET_EXEC`, or
/// `ET_DYN` for a static PIE), 64-bit little-endian.
pub fn is_elf_executable(header: &[u8], machine: u16) -> bool {
    header.len() >= 20
        && header[..4] == *b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        && u16::from_le_bytes([header[18], header[19]]) == machine
}

fn check_elf(path: &Path, arch: &str) -> Result<(), String> {
    let mut header = [0u8; 20];
    let read = fs::File::open(path).and_then(|mut f| f.read_exact(&mut header));
    let right = elf_machine(arch).is_some_and(|machine| is_elf_executable(&header, machine));
    if read.is_err() || !right {
        let name = if arch == "x86_64" { "x86-64" } else { arch };
        return Err(format!("the download is not an {name} Linux executable"));
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
        assert!(is_elf_executable(&h, 62));
        h[16] = 2;
        assert!(is_elf_executable(&h, 62));
        assert!(!is_elf_executable(&h, 183));
        h[18] = 183; // aarch64
        assert!(is_elf_executable(&h, 183));
        assert!(!is_elf_executable(&h, 62));
        assert!(!is_elf_executable(b"#!/bin/sh\necho tower 1", 62));
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

    const ASSET: &str = "tower-x86_64-linux-musl";

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

    #[test]
    fn manifest_lines_match_the_whole_name_only() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let other = "tower-aarch64-linux-musl";
        let both = format!("{b}  {other}\n{a}  {ASSET}\n");
        assert_eq!(manifest_hash(&both, ASSET), Ok(a.clone()));
        assert_eq!(manifest_hash(&both, other), Ok(b.clone()));
        assert_eq!(
            manifest_hash(&format!("{}  *{ASSET}\n", a.to_uppercase()), ASSET),
            Ok(a.clone())
        );

        // Near misses of the name, or none at all.
        for line in [
            format!("{b}  {ASSET}.sha256"),
            format!("{b}  x{ASSET}"),
            format!("{b}  ./{ASSET}"),
            format!("{b}  **{ASSET}"),
            format!("{b}  {ASSET} extra"),
            b.clone(),
            String::new(),
        ] {
            let err = manifest_hash(&line, ASSET).unwrap_err();
            assert!(err.contains("no line for"), "{line}: {err}");
        }
        // The name twice, even with one hash, or with something that is no hash.
        let err = manifest_hash(&format!("{a}  {ASSET}\n{b}  {ASSET}\n"), ASSET).unwrap_err();
        assert!(err.contains("more than one line"), "{err}");
        for hash in ["abc", "\u{1b}[31m"] {
            let err = manifest_hash(&format!("{hash}  {ASSET}\n"), ASSET).unwrap_err();
            assert!(err.contains("no valid SHA-256"), "{err}");
            assert!(!err.contains(hash), "quotes nothing: {err}");
        }
    }

    #[test]
    fn sanitized_text_is_short_plain_ascii() {
        assert_eq!(sanitized("watchtower v1.0.0"), "watchtower v1.0.0");
        assert_eq!(sanitized("a\u{1b}[31m\u{202e}b"), "a?[31m?b");
        let long = sanitized(&"x".repeat(100));
        assert_eq!(long, format!("{}…", "x".repeat(40)));
    }
}
