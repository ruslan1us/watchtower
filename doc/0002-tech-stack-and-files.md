# 0002. Tech stack, config and state files

- Status: Accepted · 2026-10-18
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0012](0012-build-and-distribution.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [0020](0020-visual-styles.md)

## Context

Tower is a single-user terminal program that runs on a remote Linux host,
usually over SSH. It must start fast, ship as one file
([0012](0012-build-and-distribution.md)), render a terminal stream in a panel
([0009](0009-stop-remove-and-logs.md)), and keep a little state the CLI does not
expose.

## Decision

**Stack**

- Rust. Crate `watchtower`, binary `tower`.
- `ratatui` with the `crossterm` backend for the UI.
- `serde` and `serde_json` for the CLI's JSON and the sidecar file, `toml` for
  the config.
- `vt100` to replay `claude logs` output, its screen converted to ratatui
  lines by tower ([0009](0009-stop-remove-and-logs.md)).
- `std::process::Command` for every CLI call. Worker threads and
  `std::sync::mpsc` channels, no async runtime.
- Further dependencies only where they replace real code (e.g. path or XDG
  helpers), not for convenience.

**Subcommands.** `tower` starts the TUI. `tower status --json` prints the merged
session list with account and tracked times for the orchestrator
([0010](0010-orchestrator.md)); without `--json` it prints a short table. It
only reads `state.json`, never writes it.

**Config:** `$XDG_CONFIG_HOME/watchtower/config.toml`, default
`~/.config/watchtower/config.toml`. Tower only reads it. A missing file means
built-in defaults with a single account `main` using `claude`. `~` is expanded
in all paths. Because a missing file is easy to miss, tower names the file it
uses: `tower --help` prints it with whether it exists, `tower status` starts
with `config: <path>` (plus `(not found — using defaults)`), `tower status
--json` has a `config` object ([0010](0010-orchestrator.md)), and the TUI
shows a status-line warning when the file was not found; the new-session
dialog's empty project list names the file and whether it was loaded
([0008](0008-new-session-dialog.md)).

```toml
# Account used for new sessions and the orchestrator.
default_account = "main"

poll_interval_secs = 2

# Folders scanned for git clones in the new-session dialog (see 0008).
project_roots = ["~/workspace"]
# Folders listed even though they are not under a root or not git clones.
extra_projects = ["~/notes"]

# Dialog defaults. "default" means the flag is not passed.
default_model = "default"            # default | fable | opus | sonnet | haiku | full model name
default_effort = "default"           # default | low | medium | high | xhigh | max
default_permission_mode = "auto"

# Prompt used when a session continues under another account (see 0004).
handoff_prompt = "continue"
# Allow hand-offs between accounts of different groups (see 0018).
allow_cross_group_handoff = false

[ui]
style = "dashboard"                  # dashboard | minimal (see 0020)

[[accounts]]
name = "main"
command = "claude"
config_dir = "~/.claude"
group = "work"                       # optional

[[accounts]]
name = "alt"
command = "claude-alt"               # wrapper that sets CLAUDE_CONFIG_DIR=~/.claude-alt
config_dir = "~/.claude-alt"
group = "personal"
```

`command` is run as-is; tower does not set `CLAUDE_CONFIG_DIR` itself, and
it removes an inherited one from every command it runs, so a value exported
in tower's own shell never reaches an account's command (a wrapper such as
`claude-alt` sets its own). Tower started with it set shows a warning
([0017](0017-ownership-evidence-rules.md), environment hygiene).
`config_dir` is used to recognise which account hosts a running session
([0003](0003-accounts-and-session-ownership.md)). It defaults to `~/.claude`.
`[ui] style` picks the look of the session list: `dashboard` (default) or
`minimal`; any other value is a config error naming both
([0020](0020-visual-styles.md)).
`group` is an optional string per account; accounts without it share one
implicit group. A hand-off offers only accounts in the owner's group unless
the top-level `allow_cross_group_handoff` (default `false`) is `true`
([0018](0018-account-groups-and-handoff-guard.md)).

**Sidecar state:** `state.json` next to `config.toml`. It holds only what the
CLI does not expose:

- per session, keyed by `sessionId`:
  - `account`, the account currently hosting it, and `account_source`, how tower
    learned it: `proc` (read from `/proc`), `list` (seen in only one account's
    list), `started` (dispatched by tower), `handoff`, or `asked` (the user's
    answer for a session whose account was unknown)
    ([0003](0003-accounts-and-session-ownership.md));
  - `last_state`, the last observed (`state`, `status`, `waitingFor`) tuple
    (absent members omitted), and `last_change_ms`, when tower first observed
    it: the poll time of the change, or `startedAt` when the session was first
    seen in it. Every listed row with a `sessionId` (or short `id`) gets them,
    interactive rows included ([0006](0006-session-states-and-grouping.md));
  - `acknowledged`, the tuple the user last acknowledged (by a successful
    attach or with `m`) while the row needed attention; tower drops it when
    it observes the tuple change, so it is normally absent or equal to
    `last_state` ([0007](0007-visual-only-attention.md));
  - `updated_ms`, when tower last changed `account` / `account_source`, and
    `last_seen_ms`, when a successful listing last showed the session (to the
    hour), both Unix ms
    ([0017](0017-ownership-evidence-rules.md));
- `recent_projects`, most-recently-used projects for the new-session dialog
  ([0008](0008-new-session-dialog.md)), newest first, paths in the `~/…` form,
  capped at 50 entries.

```json
{
  "version": 1,
  "sessions": {
    "1a2b3c4d-0000-4000-8000-000000000000": {
      "account": "alt",
      "account_source": "handoff",
      "last_state": { "state": "blocked", "status": "waiting", "waitingFor": "permission prompt" },
      "last_change_ms": 1790000000000,
      "acknowledged": { "state": "blocked", "status": "waiting", "waitingFor": "permission prompt" },
      "updated_ms": 1790000000000,
      "last_seen_ms": 1790000000000
    }
  },
  "recent_projects": [
    { "path": "~/workspace/demo-app", "last_used_ms": 1790000000000 }
  ]
}
```

- Writes are atomic: write a temp file named with the pid and a per-save
  counter (`state.json.tmp.<pid>.<n>`) in the same directory, fsync, rename
  over `state.json`, fsync the directory.
- Several tower instances may share the file. A save holds an exclusive
  `flock` on `state.json.lock`, waiting up to 2 s; on timeout it is skipped
  with a warning and retried later. Under the lock tower re-reads the file
  and applies only its own changes since its last load or save (merge on
  save, rules in [0017](0017-ownership-evidence-rules.md)).
- Saves run on a worker thread, never on the UI thread: one save at a time,
  plus at most one queued behind it (a save or a reload check). Changes made
  while a save runs stay in memory on top of the state it wrote and go out
  with the next save. On exit tower waits up to 5 s for the running save and
  then writes what is still unsaved.
- Tower also takes in other instances' changes without saving: at each poll
  a worker compares the file's device, inode, size and modification time
  with those of the version tower last read or wrote and, when they differ,
  reads it and merges it like on save, with this instance's unsaved changes
  applied on top. The same check runs right before an attach, and a
  hand-off re-reads the session's record right before it stops and before
  it wakes the session ([0017](0017-ownership-evidence-rules.md) section 8).
- Writes happen only when something changed, not on every poll. Changed
  activity times alone are saved at most once per poll cycle
  ([0006](0006-session-states-and-grouping.md)).
- On a missing file tower starts empty. On an unreadable or corrupt one it
  moves it aside as `state.json.corrupt`, starts fresh and shows a warning. It
  never refuses to start.
- Unknown fields are kept where practical. The `version` field allows
  migration.
- Entries are pruned when no successful listing showed them for 14 days,
  and a short-id `started` entry when its row did not appear within 1 hour.
  Nothing is pruned while any account's poll fails or is partial
  ([0017](0017-ownership-evidence-rules.md)).

**Orchestrator working directory:** under the config dir, e.g.
`~/.config/watchtower/orchestrator/`, holding its instruction file. Details are
in [0010](0010-orchestrator.md).

## Consequences

- One static binary, no runtime dependencies on the host.
- Synchronous threads keep the code simple; there are only a few concurrent
  calls (one poll per account, plus the selected session's logs).
- Losing `state.json` loses only conveniences: the accounts of stopped
  sessions (tower asks again), activity times (fall back to `startedAt`) and
  recent projects. No session is harmed.

## Alternatives considered

- **Go, or Python with Textual.** Workable, but Rust with ratatui has a
  maintained `vt100` crate for the logs panel and builds a fully static binary
  easily.
- **tokio.** Not needed for a few child processes and a 2 s tick.
- **State under `$XDG_STATE_HOME`.** Arguably more correct, but keeping it next
  to the config gives the user one directory to inspect or back up. This can
  move later without changing the format.
- **Tower writing its own config.** Rejected; the user edits `config.toml`, and
  tower writes only `state.json`.

## Open items

None.
