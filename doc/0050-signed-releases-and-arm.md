# 0050. Signed releases, immutable releases and an ARM build

- Status: Accepted · 2026-11-01
- Related: [0002](0002-tech-stack-and-files.md), [0012](0012-build-and-distribution.md), [0013](0013-privacy-and-repository-hygiene.md), [0015](0015-testing-strategy.md), [0026](0026-latest-release-from-the-web-redirect.md), [0049](0049-supported-platforms.md)
- Amends: [0012](0012-build-and-distribution.md) (the release job, its assets, the checklist), [0023](0023-self-update.md) (what `tower update` checks, the missing-`curl` message), [0049](0049-supported-platforms.md) (the Linux row, `release.yml`)

## Context

`tower update` replaces a binary and runs it, and until now its only check is a checksum from
the same place as the binary ([0023](0023-self-update.md)). That finds a broken download, not a
changed one. Releases also exist only for x86_64, and the release job holds a write token while
it runs every test and build script.

## Decision

**Sign with minisign.** Each release signs one file, `SHA256SUMS`. minisign has one small
signature format and one key type, and `minisign-verify` checks it in pure Rust, so the static
build stays as it is ([0012](0012-build-and-distribution.md)).

**The release contract.** A release `vX.Y.Z` has these assets:

- `tower-x86_64-linux-musl` and `tower-x86_64-linux-musl.sha256`, unchanged, so older towers
  keep updating;
- `tower-aarch64-linux-musl` and `tower-aarch64-linux-musl.sha256`;
- `SHA256SUMS`: plain `sha256sum` lines for both binaries;
- `SHA256SUMS.minisig`: its signature, not in the legacy format, with the trusted comment
  exactly `watchtower vX.Y.Z`. The comment is signed, so an old signature cannot pass as a
  newer release.

**`tower update`.** The first signed version is `0.7.0-rc.1`. Tower checks the signature and the
comment before it runs anything it downloaded. `--force` never skips this. A missing signature
fails from that version on, and an older version needs `--allow-unsigned`, which checks its
`.sha256` alone.

- `--allow-unsigned` has no effect from `0.7.0-rc.1` on. On aarch64 an older version is refused
  even with it, because those releases have no ARM asset.
- `--check` does not change, and `--check --allow-unsigned` is rejected.
- `SHA256SUMS` and its signature are capped at 64 KiB, because they are read into memory before
  anything vouches for them.
- The trusted comment is cut to 40 characters and reduced to plain ASCII before an error shows
  it. Other text from the network in an error (a redirect target, a tag, curl's message) has its
  control characters replaced.
- An empty pinned key list, or a key that does not parse, fails closed: a signed release is
  refused and nothing changes.
- Without `curl`, the error no longer suggests a command that needs `curl`. It says to install
  `curl`, or to download the asset some other way and save it as `~/.local/bin/tower`. That
  manual route has no signature check, though the message does not say so.

**Two pinned keys.** `PINNED_KEYS` in `src/update.rs` holds two public keys, each a plain literal
on its own line: the primary, and a backup whose secret half is kept outside GitHub. A signature
from either is accepted. To rotate: put the backup's secret key in the environment, pin the
backup first and a newly generated key second, and release. Older towers accept that release
because they trust the backup, and it moves them off the old primary. A tower that never updates
keeps trusting the old key, and nothing can reach it.

**The key is a GitHub secret.** `MINISIGN_SECRET_KEY` (the whole encrypted key file) and
`MINISIGN_PASSWORD` live in a `release` environment open to `v*` tags only. This guards against
assets changed after the build, a leaked token that can write releases but not read secrets, and
a changed download on the way to the user. It does not survive an account takeover, or anyone who
can push a tag with a changed workflow: they can read the secret and sign what they like. The
owner accepts this. A signature says a file came out of this pipeline, not that its code or
dependencies are sound.

**`release.yml` has two jobs.**

- `build` has no secrets and only `contents: read`. On native runners it checks the tag against
  `Cargo.toml`, runs the gates and the `pinned_keys_are_ready_for_release` test, builds, checks
  the file is static and that `--version` is right, and uploads an artifact. It uses no shared
  cache, so nothing from another run can reach a release.
- `publish` (`needs: build`, `contents: write`, `environment: release`) runs nothing from the
  repository and never runs the binaries. It writes the checksums, signs, deletes the key file
  in a step that always runs, and verifies the signature with the keys it reads from
  `src/update.rs`, so a secret that does not match them stops the release. Then it creates a
  draft, uploads, confirms the assets by name and size, and only then publishes.

The password reaches minisign on standard input, and the key through a `umask 077` file, so
neither is on a command line. minisign is the 0.12 release binary with its version and SHA-256
in the workflow. Ubuntu 24.04's universe has 0.11; the pin keeps the tool the one tested.

Tags containing `-` publish as a pre-release and do not become latest, so `releases/latest` and
older towers do not see them. All actions in both workflows are pinned by full commit SHA.

**Immutable releases.** The repository's release immutability is on: once published, a release's
assets and tag are locked. Title, notes and the latest or pre-release marks can still change.
This costs something: a broken release needs a new number, because even after deleting the
release its tag name cannot be used again. That is why assets go on a draft first. A run that
fails before publishing costs nothing: fix it and tag again, and the next run removes the
draft it finds.

**ARM.** `aarch64-unknown-linux-musl` is built and tested natively on `ubuntu-24.04-arm`, so the
tests and `--version` really run on that architecture. It links with `rust-lld` because the
default `cc` failed, which gives a non-PIE static binary. `ci.yml` has a matching job with
`continue-on-error: true`. It warns only: the same failure in `release.yml` stops the release.

A real CI run on 2026-11-01 confirmed this. The image has no `rustup`, so the workflows' step
installed its own. All tests passed natively (1,629 passed, 0 failed), and the build is
`statically linked`, non-PIE. The two tests that failed in an earlier local run under emulation
pass natively, so that was the emulator.

**Rollout.** First a dry run with `v0.7.0-rc.1`, before immutability, so it can be redone. Then
enable immutability, which applies only to future releases. Then `v0.7.0`. The steps are in
[Development](guide/development.md).

**No OS code signing** for macOS or Windows for now. Neither has a release asset yet.

## Consequences

- A download changed after the build, or a signature from an older release, fails with nothing
  replaced, on any tower from `0.7.0-rc.1` on. Older towers keep checking only the checksum.
- Releases take longer: tests run on two architectures and without a cache.
- A failing ARM test stops the release of both architectures. CI's job is the early warning.
- The crates only the tests use to sign (`minisign`, `ed25519-compact`, `ct-codecs`) are
  dev-dependencies and are not in the release binary.
- Checking by hand needs `minisign` on the user's machine. The public keys are in the source and
  in [Getting started](guide/getting-started.md).
- `v0.7.0-rc.1` pins an earlier backup key that was replaced before `v0.7.0`, and that key's secret
  half no longer exists. Towers on rc.1 still accept the primary, and `v0.7.0` moves them to the new
  list.

## Alternatives considered

- **An offline key on the owner's machine.** It survives an account takeover, which a secret does
  not. It adds a manual step to every release, and the owner chose not to take that for now. The
  contract stays the same if that changes.
- **Sigstore/cosign, keyless.** It signs with the workflow's GitHub identity and records it in a
  public log. Trust rests on that identity, so it also falls with the account, and tower would
  have to check certificates and the log instead of one signature against a pinned key.
- **GitHub artifact attestations.** Checked with `gh attestation verify`, so they need `gh` on the
  user's machine, and they rest on GitHub too. Immutable releases add a release attestation at no
  cost; tower does not rely on it.
- **No signing.** Leaves the gap [0023](0023-self-update.md) names.
- **Cross-compiling ARM on x86_64.** It builds, but neither the tests nor the binary could run.

## Open items

Only a real release run can show this:

- The `release` environment's tag rule, and `gh`'s draft, upload and publish calls.
