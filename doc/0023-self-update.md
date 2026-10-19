# 0023. Self-update command

- Status: Accepted · 2026-10-19 · release lookup amended by [0026](0026-latest-release-from-the-web-redirect.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0012](0012-build-and-distribution.md), [0013](0013-privacy-and-repository-hygiene.md), [0016](0016-do-not-change-claude-code-defaults.md)

## Context

Releases are static musl binaries on GitHub with a `sha256sum` file next to each
([0012](0012-build-and-distribution.md)). Installing a new one means a manual `curl`, a
checksum check and a `chmod`, on every host tower runs on. The owner asked to avoid those
manual downloads. Until now tower made no network calls at all: everything it does goes
through the `claude` CLI and local files ([0001](0001-thin-wrapper-over-claude-cli.md)).

## Decision

`tower update` (also spelled `tower --update`) replaces the running binary with a release.

- **Commands.** `tower update` installs the latest release. `--check` only reports
  `up to date (vX)` or `vY available (running vX)`, with exit 0 when up to date, 10 when a
  newer release exists and 1 on error. `--version vX.Y.Z` installs that release, also to
  reinstall or downgrade, and says which. `--force` skips the cargo-build and
  same-version refusals.
- **Network only on these commands.** This is the one exception to "no network", and it is
  scoped to the update command: nothing checks for updates on start, in the background or
  on a timer, and the TUI never does.
- **The system `curl`, no HTTP or TLS code in tower.** Each request runs
  `curl -q -fsSL --proto =https --proto-redir =https --tlsv1.2 --max-time 120 [-o <tmp>] <url>`
  directly, with no shell. `-q` comes first so a `~/.curlrc` cannot weaken the options.
  Without `curl` on `PATH` the error shows the manual install command from the README.
- **Which release.** The latest version is `tag_name` from
  `https://api.github.com/repos/ruslan1us/watchtower/releases/latest`, compared semver-style
  with the running version. The binary and its checksum are then fetched from the
  versioned URLs `releases/download/vX.Y.Z/tower-x86_64-linux-musl[.sha256]`, so both come
  from the same release even if a new one is published meanwhile.
- **Target.** `current_exe()` with symlinks resolved: the real file is replaced and the
  symlinks keep pointing at it.
- **Verify before touching anything.** Both files go into a fresh private temp dir in the
  target's own directory. Tower checks the SHA-256 (pure-Rust `sha2`), that the file is a
  64-bit little-endian x86-64 ELF executable, and that running it with `--version` prints
  `tower X.Y.Z` for the requested version. Only then does the file get the old binary's
  mode (default 0755) and, as far as permitted, its owner and group, and `rename()` puts
  it over the target. The rename is atomic because it stays on one filesystem.
- **On any failure** the old binary is untouched, the temp dir is removed and the error
  says that nothing was changed.
- **Refusals.**
  - A target under `target/debug/`, `target/release/` or `target/<triple>/` is a cargo
    build output. Tower refuses it before any network call unless `--force` is given.
  - When the latest release is the running version, tower prints that it is up to date
    and downloads nothing. `--force` reinstalls.
  - When the running version is newer than the latest release, tower refuses unless
    `--force` or `--version` is given.
  - Tower refuses on anything but Linux x86_64.
  - When the target's directory is not writable, the error is
    `cannot write <path>: run as its owner (e.g. sudo tower update)`.
- **Output.** One line per step, ending with
  `tower 0.2.1 → 0.3.0: downloaded, checksum OK, installed to <path>` and
  `Running tower instances keep the old version until restarted.` Tower writes nothing
  outside the target's directory and its temp dir.

## Consequences

- One command updates any host that has `curl`, with the same checksum check the README
  describes for a manual install, plus a check that the binary actually runs.
- Tower now has network behaviour, but only on an explicit command. Running `tower`,
  `tower status` or the TUI still makes no network call.
- `curl` becomes a soft requirement for updates only. TLS, proxies (`https_proxy`) and CA
  certificates follow the system's `curl`.
- The checksum comes from the same place as the binary, so it guards against a corrupt
  or truncated download, not against a compromised release. Signatures would.
- Running tower processes keep the old inode and old version until restarted.
- The GitHub API allows about 60 unauthenticated requests per hour per address. A failed
  lookup is reported as an error (exit 1 for `--check`).
- Hosts on v0.2.x need one manual `curl` to get the first release that has `update`.

## Alternatives considered

- **A bundled HTTP/TLS client (e.g. `ureq` with `rustls`).** It adds a large dependency
  tree and its own CA handling to a small static binary, for a command run rarely. The
  system `curl` is already present on the hosts tower targets and follows their proxy and
  CA setup.
- **Background or start-up update checks.** These cause network traffic the user did not
  ask for, and they can be slow or fail offline. Nothing should change, or phone home,
  without an explicit command.
- **A package manager (deb/rpm, a distro repository, `cargo install`).** Too much
  overhead for one static binary ([0012](0012-build-and-distribution.md)), and it needs
  root or a Rust toolchain on the target.
- **Writing to `releases/latest/download/…` directly.** It skips the API call, but the
  version is then known only after running the download, and the binary and checksum
  could come from two different releases.

## Open items

- Signed releases (e.g. minisign or Sigstore) and verifying the signature in `update`.
- Other architectures (e.g. aarch64): they need release assets and an asset name per
  target ([0012](0012-build-and-distribution.md)).
