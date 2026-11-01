# Getting started

Requirements, every way to install and update tower, and the first steps. Back to the [README](../../README.md).

## Requirements

- Linux x86_64 or aarch64 (account detection reads `/proc`), or Windows through
  [WSL](#on-windows-use-wsl). macOS is coming: [build from source](#macos-build-from-source-for-now)
  for now.
- Claude Code 2.1.300 or newer, with background sessions and agent view (`claude --bg`,
  `claude agents`); see [doc/reference/claude-code-cli.md](../reference/claude-code-cli.md).
- `curl`, only for `tower update`.
- Rust 1.88 or newer (via rustup), only to build from source.

## Build and install

### Install from a release

Each release ships a fully static Linux binary for x86_64 and for aarch64 that runs whatever
the glibc version ([0012](../0012-build-and-distribution.md)). `uname -m` picks yours:

```sh
mkdir -p ~/.local/bin
curl -fLo ~/.local/bin/tower https://github.com/ruslan1us/watchtower/releases/latest/download/tower-$(uname -m)-linux-musl && chmod +x ~/.local/bin/tower
```

When installing as root you may prefer `/usr/local/bin/tower`.

### Check a download by hand

Releases from v0.7.0-rc.1 on are signed ([0050](../0050-signed-releases-and-arm.md)). To check
one you need [minisign](https://jedisct1.github.io/minisign/). Fetch the binary, the checksums and
their signature, verify, then install:

```sh
cd "$(mktemp -d)"
asset=tower-$(uname -m)-linux-musl
base=https://github.com/ruslan1us/watchtower/releases/latest/download
curl -fL -O "$base/$asset" -O "$base/SHA256SUMS" -O "$base/SHA256SUMS.minisig"
minisign -Vm SHA256SUMS -P RWS+zpUTdXjkNWD8/tze42z4CklcBNrRULoQXW9fHK6713A3Akbzja1G
grep " $asset\$" SHA256SUMS | sha256sum -c - && install -m 755 "$asset" ~/.local/bin/tower
```

minisign must say the signature is verified, and the trusted comment it prints must name the
version you expect: `watchtower vX.Y.Z`. Stop if either is wrong.

The keys are also in `src/update.rs`. A release is signed with the primary, and with the backup
only after the primary has been replaced:

```text
RWS+zpUTdXjkNWD8/tze42z4CklcBNrRULoQXW9fHK6713A3Akbzja1G
RWQqbNDvLlUx1dDYHpXxFk3BsVC6/x1MfB5bSQbt3ET9nhA96hJI9iRz
```

Each binary also has its own `.sha256` file. It comes from the same place as the binary, so it
only finds a broken download.

### Updating

`tower update` (or `tower --update`) replaces the installed binary with the latest
release ([0023](../0023-self-update.md)):

```sh
tower update                    # install the latest release
tower update --check            # only report; exit 0 up to date, 10 newer release, 1 error
tower update --version v0.7.0   # install that release (reinstall or downgrade)
```

On x86_64 and aarch64 it picks the binary for your machine. It finds the latest version from
the redirect of the `releases/latest` page, falling back to the GitHub API (which allows only
about 60 requests an hour per address), and `--version` skips that lookup
([0026](../0026-latest-release-from-the-web-redirect.md)). It downloads `SHA256SUMS` and its
signature with the system `curl` into a temp dir next to the installed file, verifies the
signature against the keys built into tower and that it names this release
([0050](../0050-signed-releases-and-arm.md)), then downloads the binary, verifies its SHA-256,
checks that it is an executable for your machine, runs and reports the expected version, and
only then swaps it in with one atomic rename, keeping the file's permissions. On any failure
the old binary stays as it was. If the binary's directory is not yours (e.g. `/usr/local/bin`),
run `sudo tower update`. Running tower instances keep the old version until restarted. This is
the only command that uses the network; tower never checks for updates on its own. A cargo
build under `target/` is refused unless you pass `--force`. `--force` never skips a check.

Releases before v0.7.0-rc.1 have no signature, so tower refuses them. To install one on x86_64,
for example to downgrade, add `--allow-unsigned`: tower then checks only its checksum. The flag
changes nothing for a signed release, and `--check` does not take it. Those older releases have
no aarch64 binary, so on aarch64 they are refused even with the flag.

Without `curl`, the error says to install it or to download the binary some other way. That way
has no signature check unless you [check it by hand](#check-a-download-by-hand).

Versions before `update` existed (v0.2.x) need the install command above once, to get
the first release that has it.

### Build from source

`cargo build --release` builds `target/release/tower` for the local machine. For the same
static binary as the releases, on x86_64:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
install -m 755 target/x86_64-unknown-linux-musl/release/tower ~/.local/bin/
```

On aarch64, use `aarch64-unknown-linux-musl` in those commands and link with `rust-lld`:

```sh
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
```

For another host, copy that one file into its `~/.local/bin` (e.g. with `scp`).

### On Windows: use WSL

Tower runs inside WSL as on any Linux. In PowerShell run `wsl --install` once and restart if
asked, open the Ubuntu terminal it adds, install Claude Code there (a `claude` on the Windows
side is another installation, with its own sessions) and install tower as under
[Install from a release](#install-from-a-release). Run everything from that terminal, and keep
your projects in the Linux file system (`~/workspace`, not `/mnt/c`). A native Windows version
may come later ([0049](../0049-supported-platforms.md)).

### macOS: build from source for now

There is no macOS release yet. Tower's owner has built it from source and used it on a real Mac,
in iTerm2 and Warp, and what they tried worked; that is the only use on a Mac we know of so far.
You need Apple's command line tools (`xcode-select --install`) and Rust ([rustup](https://rustup.rs)):

```sh
git clone https://github.com/ruslan1us/watchtower
cd watchtower
cargo build --release
mkdir -p ~/.local/bin
install -m 755 target/release/tower ~/.local/bin/
```

Until tower can read process information on macOS, some things are weaker
([0049](../0049-supported-platforms.md)): with several accounts, a session tower did not
start or hand off may show `?` and ask for its account once, and the logs panel guesses the session's
terminal size, so it may look garbled. `tower update` refuses; to update, run `git pull` and
build again.

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
