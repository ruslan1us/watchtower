# 0012. Static musl build and distribution

- Status: Accepted · 2026-10-18
- Related: [0002](0002-tech-stack-and-files.md), [0015](0015-testing-strategy.md)

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
  `vt100`), so the musl build needs no C toolchain or cross-compiled C libraries.
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

- Optional future: publish release artifacts (static binary and checksum) from GitHub
  Actions.
