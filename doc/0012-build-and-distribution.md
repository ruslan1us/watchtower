# 0012. Static musl build and distribution

- Status: Accepted · 2026-10-18
- Related: [0002](0002-tech-stack-and-files.md), [0015](0015-testing-strategy.md), [0023](0023-self-update.md)

## Context

Tower is developed on one machine and run on another: a development machine (e.g. Ubuntu
22.04 under WSL, glibc 2.35) and a remote Linux host running an EL9-family distribution
(glibc 2.34). A default `x86_64-unknown-linux-gnu` build links against the build machine's
glibc, so a binary built on the development machine may fail on the target with
`GLIBC_2.35 not found`. The target should not need a Rust toolchain just to run tower.

## Decision

- Release builds target `x86_64-unknown-linux-musl` and produce a fully static binary.
- The toolchain is installed with rustup in the user's home directory, plus
  `rustup target add x86_64-unknown-linux-musl`. No system packages are changed.
- Dependencies stay pure Rust (`ratatui`, `crossterm`, `serde`, `serde_json`, `toml`,
  `vt100`, `sha2`), so the musl build needs no C toolchain or cross-compiled C libraries.
  A new dependency with C code needs a justification.
- Release profile:

  ```toml
  [profile.release]
  lto = true
  strip = true
  codegen-units = 1
  ```

- Build with `cargo build --release --target x86_64-unknown-linux-musl` and copy the single
  `tower` binary to the target, e.g. `scp` into `~/.local/bin`.
- Building directly on the target with cargo (rustup in the user's home) is an accepted
  alternative; either target triple works there.
- GitHub Actions gates every push to `main` and every pull request
  (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test` and the musl release build.
- GitHub Actions publishes release artifacts on version tags `v*`
  (`.github/workflows/release.yml`). It fails unless the tag is `v` + the `version` in
  `Cargo.toml`, runs the same gates, builds the static musl binary only, and creates a
  normal (not draft, not pre-release) GitHub release with two assets under stable names:
  - `tower-x86_64-linux-musl`, the binary, so
    `https://github.com/ruslan1us/watchtower/releases/latest/download/tower-x86_64-linux-musl`
    always fetches the newest release;
  - `tower-x86_64-linux-musl.sha256`, the `sha256sum` line for that file name.

  The release notes are GitHub's generated notes plus a short install snippet.
- `tower update` consumes these releases: it reads the latest tag from the GitHub API and
  fetches both assets from the versioned `releases/download/vX.Y.Z/` URLs, so the asset
  names, the `sha256sum` format and `tower --version` printing `tower X.Y.Z` must stay as
  they are ([0023](0023-self-update.md)).

### Release checklist

1. Set `version` in `Cargo.toml` to `X.Y.Z` and run `cargo build` so `Cargo.lock`
   follows; the gates above must pass.
2. Commit, e.g. `Release vX.Y.Z`, and push to `main`; wait for CI to pass.
3. Tag that commit and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. Check the Release workflow run and the release page: both assets are attached, it is
   marked latest, and the `releases/latest/download/...` URL serves the new binary.
5. If the workflow fails, fix on `main`, delete the tag locally and remotely
   (`git push origin :refs/tags/vX.Y.Z`) and tag again.

## Consequences

- One file to copy, no runtime dependencies beyond the Linux kernel and the `claude`
  commands already on the host.
- musl's slower allocator does not matter: tower is a TUI that polls every few seconds and
  holds little data.
- The binary runs on any x86_64 Linux regardless of glibc version.
- Other architectures (e.g. aarch64) are not built in v1 but need only another target.

## Alternatives considered

- **gnu build on the development machine.** Breaks on older glibc targets.
- **gnu build against an old glibc (container or `cargo zigbuild`).** Works but adds
  tooling for no gain over musl.
- **Distribution packages (rpm/deb).** Too much overhead for a single binary.

## Open items

- Other architectures (e.g. aarch64) and signed artifacts are not published yet.
