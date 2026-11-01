# 0049. Supported platforms

- Status: Accepted · 2026-11-01
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0009](0009-stop-remove-and-logs.md), [0012](0012-build-and-distribution.md), [0014](0014-v1-scope.md), [0015](0015-testing-strategy.md), [0017](0017-ownership-evidence-rules.md), [0023](0023-self-update.md), [0025](0025-session-terminal-size-for-logs.md), [0045](0045-workspace-style.md)
- Supersedes: the non-goal "Platforms other than Linux x86_64" of [0014](0014-v1-scope.md)
- Amends: [0012](0012-build-and-distribution.md), and the `/proc` rules of [0001](0001-thin-wrapper-over-claude-cli.md), [0017](0017-ownership-evidence-rules.md) and [0025](0025-session-terminal-size-for-logs.md)

## Context

Tower was Linux x86_64 only, because account detection reads `/proc`. The owner decided
on 2026-11-01 that tower is to run on macOS next, and on Windows later. The code itself is
close: at the start of this work the library had nine compile errors on macOS, all in the
workspace's PTY code and in the terminal-size read. What is really Linux's is the process
information, and three assumptions about terminals that only a real Mac can check (below).

## Decision

**Where tower stands, per system.**

| System | State |
|---|---|
| Linux x86_64 | The supported release: a static binary, nothing about it changes. |
| Windows, today | Through WSL, where it is Linux x86_64. |
| macOS | In progress: builds from source, no release asset, not yet tried on a real Mac. |
| Windows, native | Later, only after the owner's manual check. |

**Process information is per system.** The `/proc` exception of
[0001](0001-thin-wrapper-over-claude-cli.md) and the reads of
[0017](0017-ownership-evidence-rules.md) and
[0025](0025-session-terminal-size-for-logs.md) become: tower may read the same operating-system
information about a listed process through what its system offers for that, and nothing more:
the process's `CLAUDE_CONFIG_DIR`, its command line and executable, a value that tells a
reused pid apart, and its terminal's size. On Linux that is `/proc`, exactly as those decisions
say. Claude Code's internal files stay out of bounds everywhere.

**macOS for now.** The name `ProcEnviron` stands for `NoProcessInfo` there, whose every read
fails, as a read of another user's process does on Linux. So:

- **Ownership.** Rule 1 gives no evidence and no `proc` record, and the other rules of
  [0017](0017-ownership-evidence-rules.md) decide: the lists, the sidecar, or `?` and asking
  once. Nothing is shown for it. One account owns everything as before; with several accounts
  on shared lists, a session tower did not start or hand off is `?` until answered.
- **Logs size.** The pid-reuse guard remembers nothing, so there is no probe, and the logs panel
  infers the size ([0009](0009-stop-remove-and-logs.md)). It may look garbled as it did before
  [0025](0025-session-terminal-size-for-logs.md).
- **`tower update`.** There is no release to install, so `tower update` and `--check` stop
  at once with "no release for macOS yet; build from source", before any network call.

The next step, after the owner's check, gives macOS its own reads (`libproc`,
`KERN_PROCARGS2`) behind the same `EnvironReader` and `TerminalReader` traits.

**macOS-only code paths.** Each is chosen with `cfg`, and Linux keeps what it had:

- the slave's name from `TIOCPTYGNAME`, not `ptsname_r`;
- `posix_openpt` without `O_CLOEXEC`, and `pipe` instead of `pipe2`, with the flags set right
  after. A process another thread forks in that instant could keep a descriptor open, which
  Linux rules out;
- crossterm's reader is edge-triggered there too, and only Linux is known to wake a terminal's
  readers on a settings change, which the re-arm of [0005](0005-attach-and-detach.md) relies on.
  So a turn without input also asks crossterm for what it has, and a resize is taken while tower
  is idle rather than using up the next key's readiness.

**Native Windows** comes only after the owner's manual check, and accepts weaker account
detection: tower does not read other processes' memory there. Until then Windows users use WSL
([Getting started](guide/getting-started.md)).

**CI.** A `macos-latest` job runs `cargo fmt --check`, clippy and `cargo test` with
`continue-on-error: true`, so a macOS failure does not turn the main check red while we learn.
The Linux job, its musl build and `release.yml` stay as they are. There is no macOS release asset.

## Consequences

- Linux behaviour does not change: the new paths are compiled out, or (the update refusal, the
  idle read) decided by a value that is false there.
- A macOS user with several accounts answers `?` more often, and sees a garbled logs panel more
  often, until the next step.
- Tests that read `/proc` go through one helper, which uses `ps` elsewhere, so the terminal tests
  still run on macOS. Only what `/proc` alone can show (a process's ignored signals, the real
  reads themselves) is Linux only.

## Alternatives considered

- **Making macOS's detection part of this step.** It cannot be checked here, and whether the
  rest works has to be known first.
- **A Linux binary for `tower update` to fetch anyway.** It would not run.
- **A failing macOS job that blocks merging.** Nothing has run there yet, so it would block every
  change on guesses.

## Open items

To check on a real Mac, or from the first CI runs:

- `poll(2)` on a terminal and on a PTY master. tower treats `POLLNVAL` on its terminal as a
  hang-up, so a system that refuses it would make tower exit at once with status 129, and the
  workspace's pump does not expect it on a master. If so, these waits need another call there.
- Whether a key and a resize arriving in the same instant still lose the key's readiness until
  the next key: the idle read covers a resize that comes first.
- Whether `claude attach` and the rest of the CLI behave the same on macOS
  ([reference](reference/claude-code-cli.md)). The reference's facts were all observed on Linux.
