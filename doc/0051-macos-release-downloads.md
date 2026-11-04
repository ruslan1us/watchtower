# 0051. macOS release downloads

- Status: Accepted · 2026-11-01
- Related: [0015](0015-testing-strategy.md), [0026](0026-latest-release-from-the-web-redirect.md)
- Amends: [0012](0012-build-and-distribution.md) (assets, per-OS checks, a required macOS CI job), [0023](0023-self-update.md) (what `tower update` does on macOS), [0049](0049-supported-platforms.md) (the macOS row, the update refusal, CI), [0050](0050-signed-releases-and-arm.md) (the assets, the build job's checks)

## Context

Macs had to build tower from source, and `tower update` refused there
([0049](0049-supported-platforms.md)). That was to learn first whether it runs at all. It does:
the owner used a source build on a real Mac, in iTerm2 and Warp, and the macOS CI job passes. So
the owner decided that macOS gets release downloads.

## Decision

**The contract.** From `0.7.1-rc.1` on, a release also has:

- `tower-arm64-macos` and `tower-x86_64-macos`, each with its own `.sha256`;
- a line for each in `SHA256SUMS`.

They are thin Mach-O binaries, not universal. Signing does not change: the same `SHA256SUMS`,
signature and pinned keys ([0050](0050-signed-releases-and-arm.md)).

**The names use `arm64`,** not `aarch64`, because that is what `uname -m` prints on Apple silicon,
so `tower-$(uname -m)-macos` works in the install command.

**Native runners.** `release.yml` builds on `macos-26` (arm64) and `macos-26-intel` (Intel), so the
tests and `--version` really run on each CPU. [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
lists both labels, and the CI job's log shows `macos-latest` is the same arm64 image today. The check
step fails if `uname -m` is not the asset's CPU. No cache, as for Linux.

**The oldest macOS is 11.0, for both CPUs,** set with `MACOSX_DEPLOYMENT_TARGET` so it does not
drift with the runner image, and checked in the binary's load commands. 11.0 is the
[lowest Rust supports on arm64](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html)
(Intel could go lower). Nothing has run on a macOS that old. [Claude Code's own
documentation](https://code.claude.com/docs/en/setup) asks for macOS 13.0, so in practice that is
the floor.

**Checks in the build job, on macOS,** in place of the static check:

- `file` reports a 64-bit Mach-O executable for that CPU;
- `otool -L` lists only `/usr/lib` and `/System/Library`, so nothing from Homebrew leaks in;
- on arm64, `codesign --verify` passes and the signature is ad hoc. Apple silicon refuses unsigned
  code, and the linker signs it by default. On Intel [the linker's default is no
  signature](https://developer.apple.com/forums/thread/703059), and Intel runs that, so there a
  missing signature passes and a broken one fails.

**`tower update`** behaves on macOS as on Linux from then on. A target older than `0.7.1-rc.1` is
refused, because it has no macOS build. A tower built from source at an older version refuses to
update at all, so it needs one manual install. If `curl` is not found, the error says to add
`/usr/bin` to `PATH`, because macOS includes `curl`.

Before running a download it checks that the file is a Mach-O executable for the right CPU. This
matters more here than a wrong binary failing to start: by the `execvp` man page, a file whose
header the system does not recognise is run with `/bin/sh`. Without the check, a wrong file would
be run as a script.

**No Apple code signing or notarization** for now, by the owner's decision. Gatekeeper quarantines
browser downloads but [not `curl` ones](https://unit42.paloaltonetworks.com/gatekeeper-bypass-macos),
so `curl` and `tower update` work with nothing more. For a browser download the way round it is
`xattr -d com.apple.quarantine <file>`.

**Never overwrite a binary in place.** The kernel [caches a file's signature
information](https://developer.apple.com/forums/thread/669145), and a signed binary rewritten in
place can be killed on Apple silicon. `tower update` renames a new file
over the old one, so it is safe. By hand, the documented install downloads to a new name and
renames, and a reinstall with `cp` or `curl` straight to the target must remove the old file first.

**The macOS CI job is now required.** It loses `continue-on-error`, because a failure there now
stops a release. The ARM job stays as it is ([0050](0050-signed-releases-and-arm.md)).

**Real account detection stays parked and is unrelated.** The list of what is weaker on macOS in
[0049](0049-supported-platforms.md) still holds, apart from the update refusal.

**Rollout.** A dry run with `v0.7.1-rc.1`, a try on the owner's Mac, then `v0.7.1`. The steps are in
[Development](guide/development.md).

*Noted 2026-11-01:* the `v0.7.1-rc.1` release run showed:

- On `macos-26` and `macos-26-intel` the full test suite passed with no failures, including the
  release gate and the test that installs the real binary through its own format check.
- `file` printed exactly `Mach-O 64-bit executable arm64` and `Mach-O 64-bit executable x86_64`,
  so the checks matched.
- `otool -L` listed only `/usr/lib/libSystem.B.dylib` and `/usr/lib/libiconv.2.dylib`.
- The arm64 binary was signed ad hoc and passed `codesign --verify`. The Intel binary came out
  unsigned, as expected, and passed.
- The publish job made all ten assets, and the signature verified with the primary key and the
  trusted comment `watchtower v0.7.1-rc.1`.
- A `v0.7.0` tower on Linux updated to rc.1, so older towers read the longer `SHA256SUMS`.
- `releases/latest` stayed on `v0.7.0` while rc.1 was a pre-release.

## Consequences

- A Mac user no longer needs Rust or Apple's command line tools, and can update like anyone else.
- A release takes longer, and any macOS failure stops all four binaries.
- Older towers on Linux keep working: they look up their own line in `SHA256SUMS`, by name.
- The `releases/latest/download/` URLs for macOS return 404 until `v0.7.1` is published, because a
  pre-release does not become latest. During the dry run, use the versioned URL.
- The Intel build is the least tried. CI has no Intel job, so its tests run only in a release
  (they passed in rc.1), and Rust has `x86_64-apple-darwin` at Tier 2, so upstream does not
  guarantee its tests pass.
- Unsigned by Apple means browser downloads need the extra step above.

## Alternatives considered

- **A universal binary.** It doubles the download and needs another macOS step to join the two
  builds, and `tower update` would have to check its slices.
- **`aarch64` in the name.** It matches Linux's asset but needs a mapping in the install command.
- **Cross-compiling Intel on an arm64 runner.** The tests would not be native.
- **Apple signing and notarization.** A paid account and another secret in the release job.

## Open items

Still unproven:

- Whether a downloaded binary runs on a real Mac. So far it has run only on the release runners.
- Whether tower runs on a real Intel Mac. None has run it.
- The quarantine and overwrite behaviour above, which come from public reports and Apple's forums,
  not from a test here.

To watch: [GitHub said in 2025](https://github.com/actions/runner-images/issues/13045) that
`macos-15-intel`, available until August 2027, would be its last Intel image. Its reference now also lists `macos-26-intel`, with no end date that we found. When
Intel runners go, the Intel asset needs a new decision.
