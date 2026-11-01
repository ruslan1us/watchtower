# Development

The checks every change must pass, and how the tests and releases work. Back to the [README](../../README.md).

Design decisions are in [`doc/`](../README.md); read the relevant one before changing
behaviour. Every change must pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release --target x86_64-unknown-linux-musl
```

CI runs the same checks on every push to `main` and on pull requests; a pushed `vX.Y.Z` tag
builds and publishes a release ([below](#releases)). A macOS job runs the first three as well,
and must pass, because releases include macOS ([0051](../0051-macos-release-downloads.md)). A
native aarch64 job runs them and the static aarch64 build, and may fail too, but the same failure stops a release
([0050](../0050-signed-releases-and-arm.md)).

The tests need no Claude Code account: they use sanitized fixtures under `tests/fixtures/`
and a fake `claude` placed on `PATH` ([0015](../0015-testing-strategy.md)). Nothing
committed may contain personal details ([0013](../0013-privacy-and-repository-hygiene.md)).

## Releases

Why releases work this way is in [0050](../0050-signed-releases-and-arm.md) and, for macOS,
[0051](../0051-macos-release-downloads.md). The build job runs on four native runners:
`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-26` (arm64) and `macos-26-intel`. CI has no Intel job, so a
release is where the Intel tests first run. A failure on any runner stops the release.

### One-time setup

The owner does this once. Nobody else sees the secret keys, and none of it goes in the repository.

1. On a machine you trust, generate two key pairs, each with its own password:

   ```sh
   minisign -G -s primary.key -p primary.pub
   minisign -G -s backup.key -p backup.pub
   ```

2. Pin both public keys in `PINNED_KEYS` in `src/update.rs`, primary first, each alone on its line.
   Put them in [Getting started](getting-started.md#check-a-download-by-hand) too.
3. In the repository's settings, create an environment called `release`. Under deployment branches
   and tags, allow selected ones only, and add one rule: ref type Tag, pattern `v*`.
4. Add the primary to that environment's secrets, not the repository's:

   ```sh
   gh secret set MINISIGN_SECRET_KEY --env release < primary.key
   gh secret set MINISIGN_PASSWORD --env release
   ```

   The second command asks for the password, so it stays out of your shell history.
5. Keep `backup.key` and its password outside GitHub, somewhere you will still have them in years.
   Don't put them in the environment unless the primary has to be replaced.

### First rollout

1. Release `v0.7.0-rc.1` with the checklist below. Check it by hand
   ([Getting started](getting-started.md#check-a-download-by-hand)), and check that
   `releases/latest` still names the previous release. On a test host on each architecture, install
   it by hand and run `tower update --version v0.7.0-rc.1` with it. Only that reinstall exercises
   the signature check: an older tower checks the checksum, and aarch64 has no older tower.
2. In the repository's settings, under Releases, select **Enable release immutability**. It applies
   to future releases only.
3. Release `v0.7.0`.

### macOS rollout

1. Release `v0.7.1-rc.1` with the checklist below. Check that the Intel job really ran the tests,
   and read what the `Check the Mach-O binary` step printed for each CPU.
2. On the owner's Mac, install it by hand from the versioned URL
   (`releases/download/v0.7.1-rc.1/tower-$(uname -m)-macos`), because `releases/latest` has no macOS
   binary until `v0.7.1`. Check it by hand ([Getting started](getting-started.md#macos)), run it,
   and run `tower update --version v0.7.1-rc.1` with it to exercise the format check and the rename.
3. Release `v0.7.1`. Until then the macOS install commands in the docs return 404.

### Each release

1. Set `version` in `Cargo.toml` to `X.Y.Z` and run `cargo build` so `Cargo.lock` follows. The
   checks above must pass.
2. Commit, e.g. `Release vX.Y.Z`, push to `main` and wait for CI. Don't ignore a red macOS or
   aarch64 job: the aarch64 one may fail in CI, but neither may fail in a release.
3. Tag that commit and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`. A tag containing
   `-`, such as `v0.7.0-rc.1`, publishes a pre-release that does not become latest.
4. Check the Release workflow run and the release page: ten assets, marked latest or pre-release
   as intended, and the `releases/latest/download/...` URL serves the new binary.

**If the run fails before publishing,** nothing is locked. Fix it on `main`, delete the tag locally
and remotely (`git push origin :refs/tags/vX.Y.Z`) and tag again. The next run removes the draft
the failed one left.

**If a published release is broken,** it cannot be changed, and its tag name cannot be used again,
even after deleting it. Release a new version. Only the title, the notes and the latest or
pre-release marks can still be edited.

### Replacing a signing key

If the primary leaks or is lost:

1. Put the backup's secret key and password in the `release` environment.
2. Generate a new pair, and keep it outside GitHub as the new backup.
3. In `src/update.rs`, pin the old backup first and the new key second. Update
   [Getting started](getting-started.md), and release.

Towers already installed accept that release because they trust the old backup, and it moves them
to the new list. A tower that never updates keeps trusting the old key. If both secret keys are
lost, the only way forward is a manual install: say so in the release notes.
