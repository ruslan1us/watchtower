# 0006. Session states, grouping and activity times

- Status: Accepted · 2026-10-18
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0007](0007-visual-only-attention.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [CLI reference](reference/claude-code-cli.md)

## Context

The overview is built from `claude agents --json [--all]` (field list in the
[CLI reference](reference/claude-code-cli.md)). Three fields describe a session:

- `state` — background sessions only: `working`, `blocked`, `done`, `failed`, `stopped`.
- `status` — only while the process is alive: `busy`, `waiting`, `idle`.
- `waitingFor` — when `status` is `waiting`: `permission prompt`, `input needed`,
  `sandbox request`, `worker request`, `dialog open`.

Interactive sessions (running in some other terminal) are listed with
`kind: interactive` but have no `id` and no `state`. Combinations are not
always tidy: a session whose login had expired was listed as `state: blocked`,
`status: idle`, with no `waitingFor`.

`state: stopped` is documented but was not seen after `claude stop`: the stopped
session disappears from `--json`, appears only in `--json --all`, and keeps its
last `state` (observed: `done`) with no `pid` and no `status`. While every
session was alive, both listings returned the same rows
([CLI reference](reference/claude-code-cli.md#json-listing-claude-agents---json)).
So `state` alone cannot tell a stopped session from a live one.

The JSON has no last-activity time. `startedAt` is the start of the *current
process* and is reset every time a stopped session is woken, so it is neither
the creation time nor the time of the last change. Claude Code's built-in agent
view shows a one-line summary and an activity age, but they are not in the JSON
(https://code.claude.com/docs/en/agent-view).

## Decision

**Groups, in display order:**

| Group | Rows | Detail shown | Default |
|---|---|---|---|
| Needs me | `state: blocked`, or `status: waiting` in any state | `waitingFor`; "blocked (no reason given)" when absent | expanded |
| Failed | `state: failed` | "failed" | expanded |
| Ready for review | `state: done` (turn finished, waiting for the next prompt) | "done" | expanded |
| Working | `state: working` | from `status`: `busy` → "working", `idle` → "idle" | expanded |
| Other | any `state` value tower does not know | the raw value | expanded |
| Stopped & finished | background rows with **no live process**: no `pid` and no `status` (the rows only `--all` returns), whatever their `state`; also `state: stopped` if it appears | "stopped" | collapsed |
| Interactive | `kind: interactive` | `status`; dimmed, marked not attachable | shown, toggle with `i` |

Rules:

- The no-live-process check comes first: a background row without `pid` and
  `status` goes to Stopped & finished even when its `state` is `done`, `failed`
  or anything else. The other groups apply only to rows with a live process.
- Parsing is lenient: unknown fields are ignored, missing fields get defaults,
  and an unknown `state`/`status`/`waitingFor` value is displayed as-is. It never
  crashes the UI or drops the row.
- Empty groups are hidden. Group headers show a row count.
- Finished and stopped rows stay until the user removes them
  ([0009](0009-stop-remove-and-logs.md)); the user often needs to go back into them.
- Within a group, rows are sorted by last change, most recent first.

**Alternative grouping.** `p` toggles between grouping by state and grouping by
project (`cwd`). In project mode each project is a group, rows inside are
ordered by the state order above, and the Interactive toggle still applies.

**Filter.** `/` opens a text filter that matches, case-insensitively, against
name, `cwd` and account. Group counts reflect the filter; the attention counter
does not ([0007](0007-visual-only-attention.md)).

**Row content:** name (or short id when unnamed), project folder (`cwd` with the
home directory shortened to `~`, e.g. `~/workspace/api`), state detail from the
table above, time since last change ("3m", "2h", "4d"), account (`?` when unknown,
[0003](0003-accounts-and-session-ownership.md)).

**Activity times.** Tower tracks them itself:

- For each session (keyed by `sessionId`, or `id` when that is missing) tower
  keeps the last observed tuple (`state`, `status`, `waitingFor`) and the time it
  first observed that tuple. The pair is stored in the sidecar state file as
  `last_state` and `last_change_ms` ([0002](0002-tech-stack-and-files.md)).
- A changed tuple on a poll sets the change time to the poll time.
- On the first sighting of a session the change time is `startedAt`.
- Changes that happened while tower was not running are not observable: the
  row shows the time tower first saw the new tuple. This is a known limitation,
  not an error.
- Entries for sessions no longer listed, even with `--all`, are dropped from the sidecar.

Tower does not scrape the agent view or read transcripts to get a summary or a
better activity time.

## Consequences

- One sidecar write per observed change; no extra CLI calls for times.
- Times after a restart of tower can be too recent for sessions that changed
  while it was not running.
- A new `state` value from a future Claude Code release shows up in "Other"
  instead of breaking the list; only the mapping table needs updating.
- No summary column; the user peeks with the logs panel or attaches.

## Alternatives considered

- **`startedAt` as the activity time.** Rejected: reset on every wake.
- **File times of transcripts or job files.** Rejected: internal files are out of bounds ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **Scraping the agent view for summary and age.** Rejected: breaks with every UI change.
- **`done` inside "Needs me" or with idle sessions.** Rejected: done sessions are not urgent but should not get lost; a separate group right after "Needs me" mirrors the agent view.
- **Hiding finished sessions after N hours.** Rejected: they are needed later; a collapsed group keeps them out of the way.

## Open items

- How a session whose process Claude Code stopped by its idle rule (about
  1 hour) looks in the JSON. After `claude stop` the row keeps its last `state`
  and has no `pid` or `status`; the idle stop is not verified but probably looks
  the same. Either way it lands in Stopped & finished by the no-live-process
  rule.
- A full sanitized fixture set covering every `state` and `waitingFor` value
  ([0015](0015-testing-strategy.md)).
