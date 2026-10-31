# Getting started

Requirements, every way to install and update tower, and the first steps. Back to the [README](../../README.md).

## Requirements

- Linux x86_64 (account detection reads `/proc`).
- Claude Code 2.1.300 or newer, with background sessions and agent view (`claude --bg`,
  `claude agents`); see [doc/reference/claude-code-cli.md](../reference/claude-code-cli.md).
- `curl`, only for `tower update`.
- Rust 1.88 or newer (via rustup), only to build from source.

## Build and install

### Install from a release

Each release ships a fully static x86_64 Linux binary that runs whatever the glibc
version ([0012](../0012-build-and-distribution.md)):

```sh
mkdir -p ~/.local/bin
curl -Lo ~/.local/bin/tower https://github.com/ruslan1us/watchtower/releases/latest/download/tower-x86_64-linux-musl && chmod +x ~/.local/bin/tower
```

When installing as root you may prefer `/usr/local/bin/tower`.

To check the download against the published checksum, fetch both files under their
release names, verify, then install:

```sh
cd "$(mktemp -d)"
base=https://github.com/ruslan1us/watchtower/releases/latest/download
curl -LO "$base/tower-x86_64-linux-musl" -LO "$base/tower-x86_64-linux-musl.sha256"
sha256sum -c tower-x86_64-linux-musl.sha256 && install -m 755 tower-x86_64-linux-musl ~/.local/bin/tower
```

### Updating

`tower update` (or `tower --update`) replaces the installed binary with the latest
release ([0023](../0023-self-update.md)):

```sh
tower update                    # install the latest release
tower update --check            # only report; exit 0 up to date, 10 newer release, 1 error
tower update --version v0.2.1   # install that release (reinstall or downgrade)
```

It finds the latest version from the redirect of the `releases/latest` page, falling back
to the GitHub API (which allows only about 60 requests an hour per address), and
`--version` skips that lookup ([0026](../0026-latest-release-from-the-web-redirect.md)).
It downloads the binary and its checksum with the system `curl` into a temp dir next to
the installed file, verifies the SHA-256, checks that the new binary runs and reports the
expected version, and only then swaps it in with one atomic rename, keeping the file's
permissions. On any failure the old binary stays as it was. If the binary's directory is
not yours (e.g. `/usr/local/bin`), run `sudo tower update`. Running tower instances keep
the old version until restarted. This is the only command that uses the network; tower
never checks for updates on its own. A cargo build under `target/` is refused unless you
pass `--force`.

Versions before `update` existed (v0.2.x) need the install command above once, to get
the first release that has it.

### Build from source

`cargo build --release` builds `target/release/tower` for the local machine. For the same
static binary as the releases:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
install -m 755 target/x86_64-unknown-linux-musl/release/tower ~/.local/bin/
```

For another host, copy that one file into its `~/.local/bin` (e.g. with `scp`).

## Quick start

1. Run `tower`. Without a config file a welcome window opens
   ([0048](../0048-first-run-welcome.md)): it says what it found, then asks for a style, a
   theme, the folders that hold your git clones and three options. `Enter` goes on and saves at
   the end; `Esc` saves what you have chosen so far; `Ctrl+C` quits without writing anything.
   It writes `~/.config/watchtower/config.toml`, with one account, `main`, running `claude`.
   More accounts, or writing the file by hand first, are in
   [Configuration](configuration.md). `tower setup` opens the window again at any time.
2. The existing background sessions of every account appear in the list, in the style you
   chose. `tower status` and `tower --help` name the config file in use.
3. Press `n`, pick a project, type a task and press `Ctrl+S` to start it in the background
   (or `Ctrl+Enter` / `Alt+Enter` to start and attach). If the account does not trust the
   folder yet, `Ctrl+T` opens `claude` there so you can accept, then retries.
4. Press `Enter` on a session to attach. Inside it, `Ctrl+Z` returns to tower.
5. Press `?` for all keys, `q` to quit.
