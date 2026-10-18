# 0003. Multiple accounts and session ownership

- Status: Accepted · 2026-10-18
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0009](0009-stop-remove-and-logs.md), [0016](0016-do-not-change-claude-code-defaults.md), [CLI reference](reference/claude-code-cli.md)

## Context

A user may have several Claude Code accounts on one host, each selected by its own `CLAUDE_CONFIG_DIR`, usually through a wrapper command. Example used throughout these docs:

| Account | Command | Config dir |
|---|---|---|
| `main` | `claude` | `~/.claude` (default) |
| `alt` | `claude-alt` (sets `CLAUDE_CONFIG_DIR=~/.claude-alt`) | `~/.claude-alt` |

Each config dir runs its own transient supervisor that hosts its background sessions ([agent view docs](https://code.claude.com/docs/en/agent-view)). The docs describe each config dir as having its own sessions. In practice users often share parts of the config dirs through symlinks (for example `jobs/`, `projects/`, `history.jsonl`); then both commands list the same sessions while the supervisors, credentials and `.claude.json` stay separate. This has been verified.

Running a command with account B's binary against a session hosted by account A's supervisor (verified, shared lists):

| Command | Result |
|---|---|
| `logs <id>` | Fails with `job not found` |
| `stop <id>` | Succeeds |
| `attach <id>` (session stopped) | Wakes it **under account B**, same short id |
| `rm <id>` | Succeeds |
| `--resume <sessionId> --bg "…"` (stopped, run from its `cwd`) | Wakes it **under account B**, same short id |

So waking or attaching with the wrong binary silently moves the session to another account, with its limits and login. The listing JSON does not say which account hosts a session (see the [CLI reference](reference/claude-code-cli.md)).

## Decision

**Configuration.** The config lists N accounts, each with `name`, `command` and optional `config_dir` (defaults to `~/.claude` when unset), plus one global `default_account`. There is no per-project account: the user switches accounts only when one hits its limits, using the hand-off ([0004](0004-account-hand-off.md)). The new-session dialog preselects `default_account` and lets the user pick another account for that session ([0008](0008-new-session-dialog.md)).

```toml
default_account = "main"

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
```

**Listing.** Each poll runs `<command> agents --json --all` for every account and merges the results, de-duplicating by `sessionId`, falling back to the short `id` (a defensive rule; background rows always carry `id`) and, for rows with neither, to `pid` ([0001](0001-thin-wrapper-over-claude-cli.md)). This works whether or not the lists are shared.

**Ownership resolution**, first match wins:

1. **Running session** (`pid` present): read `CLAUDE_CONFIG_DIR` from `/proc/<pid>/environ`; a missing variable means `~/.claude`. Map it to the configured account by comparing canonical paths (`~` expanded, symlinks resolved). The parent pid is an intermediate host process, not the supervisor, so tower reads the session's own environment rather than walking to the parent. This is the only OS-level read tower does ([0001](0001-thin-wrapper-over-claude-cli.md)).
2. **Seen in exactly one account's list** (lists not shared): that account.
3. **Sidecar record** (`account`, `account_source`) written by tower on dispatch (`started`), on hand-off (`handoff`) and on every successful detection by rule 1 (`proc`) or 2 (`list`) ([0002](0002-tech-stack-and-files.md)).
4. **Unknown**: the row shows account `?`. On the first wake or attach tower asks which account to use and records the answer in the sidecar (`asked`).

A config dir found in `environ` that matches no configured account also yields `?`, with the raw path shown in the row details.

**Command routing.** Every command that needs the hosting supervisor (`logs`, `attach`, wake via `--resume --bg`) runs with the owning account's command. `stop` and `rm` also use it, for consistency, although they work from any account.

## Consequences

- Polling costs one CLI call per account per interval.
- Live detection through `/proc` beats the sidecar, so a session the user woke by hand under another account is picked up on the next poll and the sidecar is corrected.
- `/proc/<pid>/environ` is readable only for the user's own processes (or by root). If the read fails, tower falls through to rules 2–4.
- A stopped session started outside tower, with shared lists and no sidecar record, has no reliable owner; asking once is the honest answer.
- Tower never moves a session between accounts by accident: rows with `?` require an explicit choice before any waking command runs.

## Alternatives considered

- **Parse `claude daemon status` text** to match sessions to supervisors: rejected; human-readable output, unstable across releases.
- **Read the supervisor's internal files** (roster, `jobs/`, `daemon/`): rejected; internal state, against [0001](0001-thin-wrapper-over-claude-cli.md).
- **Sidecar only**: fails for sessions started outside tower and goes stale when the user wakes a session by hand.
- **Account per project in config**: does not match how accounts are used (switch on limits, not by project).

## Open items

- Cross-session messaging (`ListAgents`, `SendMessage`) between sessions hosted by different accounts' supervisors is not yet verified; this affects the orchestrator ([0010](0010-orchestrator.md)).
- `attach` with another account's binary on a session that is still running has not been tested.
- Whether `agents --json` starts a supervisor for an account with no live sessions (adding latency to each poll) is not yet known.
