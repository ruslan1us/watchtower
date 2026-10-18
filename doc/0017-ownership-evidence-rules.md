# 0017. Ownership evidence rules

- Status: Accepted · 2026-10-18
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0008](0008-new-session-dialog.md), [CLI reference](reference/claude-code-cli.md)

## Context

[0003](0003-accounts-and-session-ownership.md) resolves a session's owner from a live process (rule 1), from being listed by exactly one account (rule 2), from the sidecar (rule 3), or asks (rule 4). Waking or attaching with the wrong account's command silently moves a session to that account, so a wrong owner is worse than `?`.

Review found ways the rules as written produce a wrong owner:

- Rule 2 compared listings taken at different times. An account that briefly returned an empty successful listing, a new session that showed up in one account's poll before the other's, a removed row still in another account's last listing, or a row one listing's parser skipped all looked like "listed by exactly one account". The resulting `list` record then overwrote `started`, `handoff` or `asked`.
- Rule 1 ran on pids from stale snapshots. A pid freed by an exited session and reused by an unrelated process gave that process's config dir.
- Two accounts with the same config dir (for example one with `config_dir` left unset next to `main`) could not be told apart, and every live session went to whichever came first.

Facts relied on ([CLI reference](reference/claude-code-cli.md)): `startedAt` is the start of the session's current process, reset on every wake; `pid` is present only while that process is alive.

## Decision

This decision supersedes the ownership-resolution details of [0003](0003-accounts-and-session-ownership.md). The order of the rules, their sources and single-account mode stay as 0003 describes them.

### 1. Which row a merged session uses

When several listings contain the same session, the kept row is the one from a fresh listing over one from a stale snapshot (kept after a failed poll), then the one from the most recently received listing, then the first with a live process, then the first in config order. The row decides which `pid` rule 1 looks at.

### 2. Rule 1: live process

Rule 1 applies only when the kept row comes from a fresh listing. A row known only from a stale snapshot gives no `/proc` evidence and no `proc` record.

Pid-reuse guard, per (session key, pid) pair, with no wall-clock input:

- **First sighting.** Tower reads `/proc/<pid>/cmdline` and accepts the process only if it looks like Claude Code: `argv[0]` or the `/proc/<pid>/exe` target is a file named `claude` or lies in a Claude Code installation (`…/claude/versions/<version>`, `…/@anthropic-ai/claude-code/…`; a ` (deleted)` suffix after an update is ignored), or it is `node`/`nodejs` whose script (first non-option argument) is such a path. The native binary's `exe` is named after its version, and `argv[0]` is often the bare `claude`, so both are checked. When accepted, tower remembers the raw start ticks (field 22 of `/proc/<pid>/stat`) for the pair, in memory only, never in the sidecar.
- **Later polls.** Tower re-reads field 22 and compares it with the remembered value. Equal: the same process, rule 1 applies. Different: the pid was reused; no evidence, the pair is forgotten and the next poll treats it as a first sighting. Both values come from the boot-relative clock, so a suspended host (a paused VM) or a stepped wall clock does not matter.
- A pair is also forgotten when its row disappears or the row's pid changes. A row kept only from a stale snapshot keeps its pair.
- An unreadable `cmdline` or `stat`, or a process that does not look like Claude Code, gives no rule-1 evidence; resolution falls through to rule 2.

### 3. `CLAUDE_CONFIG_DIR` values

- Unset or empty: the default `~/.claude`.
- An absolute path: compared with each account's `config_dir`, both with symlinks resolved. A path that does not exist compares by its expanded, normalised form (a trailing `/` does not matter).
- Not an absolute path, including a literal `~/…` (the process sees the variable unexpanded): it depends on the process's working directory, so it is never mapped to an account. The row is `?` and the raw value is shown in the details.
- An absolute path that matches no account: `?` with the raw path, as before.

### 4. Rule 2: listed by exactly one account

Rule 2 applies to a row only when all of these hold:

- at least two accounts are configured, and every one has a fresh listing;
- all those listings come from the same poll cycle: the poller numbers every trigger (tick or forced poll); an account whose poll was skipped because the previous one still ran stays a cycle behind, and a re-run reports the cycle that asked for it;
- no listing is empty, and none is partial (the parser skipped rows, [0001](0001-thin-wrapper-over-claude-cli.md));
- no row of the whole merge appears in more than one account's listing: one shared row shows the lists are shared, and then rule 2 is skipped for every row;
- the sidecar has no record other than `list` for the session that names a configured account.

### 5. Sidecar records

- A `proc` detection replaces any record: it is the live process's own config dir.
- A `list` detection is written only when the session has no record or a `list` record. It never replaces `started`, `handoff`, `asked` or `proc`.
- When a record keyed by a short id ([0008](0008-new-session-dialog.md)) moves to its `sessionId`, an existing record there wins, except a `list` record, which takes the account and source of the short-id record.

### 6. Duplicate config dirs

Config load fails when two accounts have the same `config_dir`, compared as in section 3 (`~` expanded, symlinks resolved, non-existent paths by expanded path; an unset `config_dir` is `~/.claude`). The error names both accounts and the directory.

### 7. Reading `/proc`

This amends the exception in [0001](0001-thin-wrapper-over-claude-cli.md): for a listed `pid`, tower reads `/proc/<pid>/environ`, `/proc/<pid>/cmdline`, the `/proc/<pid>/exe` link and field 22 of `/proc/<pid>/stat` (no `btime`). All are operating-system process information, not Claude Code files.

### 8. Several tower instances on one sidecar

Two towers (for example in two SSH sessions) share `state.json`. Neither may revert or corrupt what the other wrote.

- Every save holds an exclusive `flock` on `state.json.lock` for the whole read-modify-write. The lock file is never removed. Tower retries a non-blocking lock for up to 2 s. On timeout it writes nothing and shows a warning. The change stays pending: the next save writes it, and after a failed save the next poll asks for a retry once 10 s have passed.
- Under the lock, tower re-reads the file and applies only what this instance changed since its last load or save. It compares memory with the state it last read or wrote: added, changed and removed records, and changed fields. Everything else comes from the file, including the format version and unknown fields. The merged result is written (a temp file named with the pid and a per-save counter, fsync, rename, fsync of the directory) and becomes the in-memory state.
- Each record carries `updated_ms`, when tower last changed its `account` / `account_source`. When both instances changed a record's owner, a `list` detection never beats another source, as in section 5, so `proc` never loses to `list`. Otherwise the newer `updated_ms` wins, and the saving instance wins a tie. A record moved from its short id keeps the short-id record's `updated_ms`.
- Other fields: `last_seen_ms` takes the later time, and `last_state`, `acknowledged` and unknown fields take the saving instance's value. If both instances changed `recent_projects`, the lists are united: one entry per path with the later `last_used_ms`, newest first, capped at 50.
- A removal (pruning, or a short-id record moved to its `sessionId`) applies only if the file's record is unchanged since this instance read it. A record changed here but removed by the other instance is written back.
- A missing file, or a corrupt one (moved aside as on load), is replaced by the whole in-memory state.

### 9. Pruning

- Each record carries `last_seen_ms`. It is set whenever the record's key is a row of an account's latest successful listing, at most once an hour per record, so polls do not cause writes.
- Pruning runs at each applied poll, and only when every account's latest poll succeeded with a full listing, with no skipped rows. While any account's poll fails, nothing is pruned.
- A record is pruned when the later of `last_seen_ms` and `updated_ms` is 14 days old. This covers removed sessions, interactive rows' `proc` records and hand-off copies whose row never appeared.
- A `started` record keyed by a short id (no `-`) is pruned when it was never seen and its `updated_ms` is 1 hour old: the dispatch never produced a listed row.
- A record written by an older tower, with neither time, first gets `updated_ms` set to the current time, so it is aged rather than dropped at once.
- A removal is a recorded change, so it survives the merge on save (section 8).

## Consequences

- Rule 2 applies less often. An account with no sessions gives no `list` evidence for the other account's rows, and an account whose polls take longer than the poll interval rarely shares a cycle with the others. Such rows rely on `started`, `handoff`, `asked` or `proc` records, or are `?` and asked once. Being asked is safe; a wrong owner is not.
- A suspended host or a stepped wall clock no longer costs rule-1 evidence. The first sighting trusts the `cmdline`: a row whose exited process's pid was taken by another Claude Code process before tower saw the row (for example while tower was not running) gives that process's config dir. The pairs live in memory, so every tower start is a first sighting for every live row.
- A process with a literal `~` in `CLAUDE_CONFIG_DIR` shows `?` until the user answers.
- A config that worked before with two accounts on one config dir now fails to load. The error says how to fix it.

## Alternatives considered

- **Comparing the process start time with `startedAt`** (the first version of this guard: `btime` plus field 22, within 30 s). `startedAt` is wall clock and the start time is boot-relative; after a host sleep a live session was measured over 10,000 s off, so every live session lost rule-1 evidence after any pause.
- **Dropping rule 2.** Separate lists with sessions started outside tower would then always be `?` after they stop. The conditions above keep the case where the evidence is solid.
- **Keeping several cycles per account** to compare the latest cycle all accounts finished. More state for a rule that is the weakest evidence; not worth it yet.
- **Expanding `~` in `CLAUDE_CONFIG_DIR`.** Guesses how Claude Code reads the variable; a wrong guess moves sessions.

## Open items

- Verify that a live background worker's `cmdline` and `exe` match the accepted forms. So far only an interactive process of a native install was inspected (bare `claude` as `argv[0]`, `exe` under `claude/versions/<version>`); the `node` form is inferred from the npm package layout, not observed.
- Whether Claude Code expands a literal `~` in `CLAUDE_CONFIG_DIR`.
- An instance sees another instance's sidecar changes only when it saves (section 8). Until then it resolves owners from its own copy. Reloading when the file's mtime changes would close that gap.
