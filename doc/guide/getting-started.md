# Getting started

Requirements, every way to install and update tower, and the first steps. Back to the [README](../../README.md).

## Requirements

- Linux x86_64 or aarch64 (account detection reads `/proc`), [macOS](#macos) on Apple silicon or
  Intel, or Windows through [WSL](#on-windows-use-wsl).
- Claude Code 2.1.300 or newer, with background sessions and agent view (`claude --bg`,
  `claude agents`); see [doc/reference/claude-code-cli.md](../reference/claude-code-cli.md).
- `curl`, only for `tower update`.
- Rust 1.88 or newer (via rustup), only to build from source.

## Build and install

### Install from a release

On Linux, each release ships a fully static binary for x86_64 and for aarch64 that runs whatever
the glibc version ([0012](../0012-build-and-distribution.md)). `uname -m` picks yours. For a Mac,
see [macOS](#macos).

```sh
mkdir -p ~/.local/bin
curl -fLo ~/.local/bin/tower https://github.com/ruslan1us/watchtower/releases/latest/download/tower-$(uname -m)-linux-musl && chmod +x ~/.local/bin/tower
```

When installing as root you may prefer `/usr/local/bin/tower`.

### Check a download by hand

Releases from v0.7.0-rc.1 on are signed ([0050](../0050-signed-releases-and-arm.md)). To check
one you need [minisign](https://jedisct1.github.io/minisign/). Fetch the binary, the checksums and
their signature, verify, then install. This is for Linux; [macOS](#macos) has its own version:

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

On Linux and macOS it picks the binary for your machine. It finds the latest version from
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

Releases before v0.7.0-rc.1 have no signature, so tower refuses them. To install one on Linux
x86_64, for example to downgrade, add `--allow-unsigned`: tower then checks only its checksum. The flag
changes nothing for a signed release, and `--check` does not take it. Those older releases have
no aarch64 binary, so on aarch64 they are refused even with the flag. macOS binaries start at
v0.7.1-rc.1, so on macOS anything older is refused too.

Without `curl` on Linux, the error says to install it or to download the binary some other way.
That way has no signature check unless you [check it by hand](#check-a-download-by-hand). macOS
includes `curl`, so there the error says to add `/usr/bin` to `PATH`.

Versions before `update` existed (v0.2.x) need the install command above once, to get
the first release that has it.

### Build from source

`cargo build --release` builds `target/release/tower` for the local machine. For the same
static binary as the Linux releases, on x86_64:

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

### macOS

Releases from v0.7.1-rc.1 on have a binary for Apple silicon and one for Intel
([0051](../0051-macos-release-downloads.md)). They are built for macOS 11 or newer, and Claude Code
itself asks for macOS 13 or newer. `uname -m` picks yours:

```sh
mkdir -p ~/.local/bin
curl -fLo ~/.local/bin/tower.new https://github.com/ruslan1us/watchtower/releases/latest/download/tower-$(uname -m)-macos && chmod +x ~/.local/bin/tower.new && mv -f ~/.local/bin/tower.new ~/.local/bin/tower
```

`~/.local/bin` isn't on `PATH` by default on macOS. For zsh, the default shell, add it and open a
new terminal:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
```

**Reinstalling by hand.** Don't write over an existing binary: Apple silicon can kill a binary
that was overwritten in place. The command above is safe because it renames a new file over the
old one. If you use `cp`, or `curl` straight to the target, run `rm -f ~/.local/bin/tower` first.

**A quarantine message.** The binaries aren't signed by Apple. A `curl` download works as it is, but
if you downloaded with a browser, macOS may refuse to open the file. If you trust the file, ideally
after checking it as below, remove the quarantine:

```sh
xattr -d com.apple.quarantine ~/.local/bin/tower
```

**Updating.** `tower update` works as on Linux, from v0.7.1-rc.1 on. A tower you built from source at
an older version refuses to update, so install it once with the command above.

**Checking a download by hand.** The steps and keys are the same as in
[Check a download by hand](#check-a-download-by-hand), with the macOS asset, and with `shasum`
because macOS may not have `sha256sum`:

```sh
cd "$(mktemp -d)"
asset=tower-$(uname -m)-macos
base=https://github.com/ruslan1us/watchtower/releases/latest/download
curl -fL -O "$base/$asset" -O "$base/SHA256SUMS" -O "$base/SHA256SUMS.minisig"
minisign -Vm SHA256SUMS -P RWS+zpUTdXjkNWD8/tze42z4CklcBNrRULoQXW9fHK6713A3Akbzja1G
grep " $asset\$" SHA256SUMS | shasum -a 256 -c - && rm -f ~/.local/bin/tower && install -m 755 "$asset" ~/.local/bin/tower
```

**Building from source** is the alternative. You need Apple's command line tools
(`xcode-select --install`) and Rust ([rustup](https://rustup.rs)):

```sh
git clone https://github.com/ruslan1us/watchtower
cd watchtower
cargo build --release
mkdir -p ~/.local/bin
rm -f ~/.local/bin/tower
install -m 755 target/release/tower ~/.local/bin/
```

Until tower can read process information on macOS, some things are weaker
([0049](../0049-supported-platforms.md)): with several accounts, a session tower did not
start or hand off may show `?` and ask for its account once, and the logs panel guesses the session's
terminal size, so it may look garbled.

Tower's owner has used it on a real Mac, in iTerm2 and Warp, and what they tried worked; that is
the only use on a Mac we know of so far, and it was on a source build.

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
