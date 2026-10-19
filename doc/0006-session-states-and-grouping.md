# 0006. Session states, grouping and activity times

- Status: Accepted · 2026-10-18 · a rename's wake is not activity, amended by [0027](0027-rename-sessions.md)
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0007](0007-visual-only-attention.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [0020](0020-visual-styles.md), [CLI reference](reference/claude-code-cli.md)

## Context

The overview is built from `claude agents --json [--all]` (field list in the
[CLI reference](reference/claude-code-cli.md)). Three fields describe a session:

- `state` — background sessions only: `working`, `blocked`, `done`, `failed`, `stopped`.
- `status` — only while the process is alive: `busy`, `waiting`, `idle`.
- `waitingFor` — when `status` is `waiting`: `permission prompt`, `input needed`,
  `sandbox request`, `worker request`, `dialog open`.

Interactive sessions (running in some other terminal) are listed with
`kind: interactive` but have no `id` and no `state`. `state: blocked`,
`status: idle` with no `waitingFor` is what a session reports when its turn
ended with a question to the user; a session whose login had expired was
listed the same way ([CLI reference](reference/claude-code-cli.md#json-listing-claude-agents---json)).

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
| Orchestrator | background rows named exactly `orchestrator` ([0010](0010-orchestrator.md)), whatever their state | the detail of the group its state gives it below; "ready" when it waits for the user (`blocked` without `waitingFor`, or `done`) | expanded |
| Needs me | `state: blocked`, or `status: waiting` in any state | `waitingFor`; "waiting for your reply" when absent | expanded |
| Failed | `state: failed` | "failed" | expanded |
| Ready for review | `state: done` (turn finished, waiting for the next prompt) | "done" | expanded |
| Working | `state: working` | from `status`: `busy` → "working", `idle` → "idle" | expanded |
| Other | any `state` value tower does not know | the raw value | expanded |
| Stopped & finished | background rows with **no live process**: no `pid` and no `status` (the rows only `--all` returns), whatever their `state`; also `state: stopped` if it appears | "stopped" | collapsed |
| Interactive | `kind: interactive` | `status`; dimmed, marked not attachable | shown, toggle with `i` |

Rules:

- The Orchestrator check comes first: every background row named exactly
  `orchestrator` goes there, live or not, in the order `o` picks from (a live
  one first, then the most recently started, [0010](0010-orchestrator.md)).
  Ending a turn with a question is its normal state, so it never sits in
  Needs me or Failed and is never highlighted ([0007](0007-visual-only-attention.md)).
  Its state still counts for actions, e.g. `s` asks first while it is
  working or blocked ([0009](0009-stop-remove-and-logs.md)).
- The no-live-process check comes next: a background row without `pid` and
  `status` goes to Stopped & finished even when its `state` is `done`, `failed`
  or anything else. The other groups apply only to rows with a live process.
- Parsing is lenient: unknown fields are ignored, missing fields get defaults,
  and an unknown `state`/`status`/`waitingFor` value is displayed as-is. It never
  crashes the UI or drops the row.
- Empty groups are hidden. Group headers show a row count, and how many of
  those rows are highlighted ([0007](0007-visual-only-attention.md)).
- Finished and stopped rows stay until the user removes them
  ([0009](0009-stop-remove-and-logs.md)); the user often needs to go back into them.
- Within a group, rows are sorted by last change, most recent first, then by
  name.

**Alternative grouping.** `p` toggles between grouping by state and grouping by
project (`cwd`). In project mode each project is a group, rows inside are
ordered by the state order above, and the Interactive toggle still applies.

- The header names the project as the row shows it (`cwd` with `~`); rows
  without a `cwd` share a "(no folder)" group.
- Projects are ordered by their most recent change, newest first; a project
  with no known time comes last, and ties go by name.
- Inside a project, rows follow the group order of the table, then last change
  (most recent first), then name. Rows keep their state detail and highlight.
- Interactive rows keep their own Interactive group, last, so `i` hides them
  in either mode, and the orchestrator rows keep their Orchestrator group,
  first.

**Collapsing.** `Tab` folds or unfolds the group of the selected line, and so
does `Enter` on a group header. The selection can rest on headers, and a group
that is folded leaves its header selected. Stopped & finished starts collapsed
and every other group starts expanded, projects included. A collapsed header
still shows its row count and its highlight count. Each choice is remembered
per group (a state group or a project) while tower runs. It survives polls and
`p` toggles but is not saved: the next start uses the defaults again, so the
sidecar ([0002](0002-tech-stack-and-files.md)) holds no view settings.

**Interactive toggle.** `i` hides or shows the Interactive group. It is shown,
dimmed, by default. While it is hidden, the status bar says how many
interactive rows are hidden.

**Empty list.** With no sessions at all the list says `No sessions. Press n
to start one.` When sessions exist but the filter or the `i` toggle hides
every row, it names the reason instead, e.g. `Nothing to show: 1 interactive
hidden (i shows)`.

**Filter.** `/` opens a text filter that matches, case-insensitively, against
name, `cwd` and account. Group counts reflect the filter; the attention counter
does not ([0007](0007-visual-only-attention.md)).

- The input line sits above the status bar, and typing filters as you go. The
  query is split at whitespace, and every word must occur in at least one of:
  the name (the short id when unnamed), the `cwd` as listed or with `~`, or
  the account (`?` when unknown).
- `Enter` keeps the filter and gives the keys back to the list. `Esc` clears
  it, either while typing or later in the list (after an attach waiting for
  its refresh and the logs panel). `/` edits the active filter.
- While a filter is active, the status bar shows `filter "<text>": N matches`.
- Groups without a match are hidden.
- On every change of the text, the selection moves to the first match.
- While a filter is active, collapsed groups show their matches, and folding
  waits until the filter is cleared.
- The attention counter keeps counting every highlighted session. It adds
  "(filtered)" when the filter hides some of them.

**Selection.** It stays on the same session across polls, filter changes and
the `p`, `i` and collapse toggles whenever that session is still listed. If
the session is in a collapsed group, its header is selected. If it is gone,
the selection keeps its position, clamped to the list.

**Row content:** the attention marker column ([0007](0007-visual-only-attention.md)),
name (or short id when unnamed), project folder (`cwd` with the
home directory shortened to `~`, e.g. `~/workspace/api`), state detail from the
table above, time since last change, account (`?` when unknown,
[0003](0003-accounts-and-session-ownership.md)).

The age column is right-aligned, four columns wide, rounded down: `now` under
10 s (and for a time in the future), then `42s`, `5m`, `3h`, `2d`.

**Styles.** Everything above holds in both styles of
[0020](0020-visual-styles.md); only the drawing differs. The minimal style
shows headers as `▾ Needs me (3)  ● 2` and the title line's session count.
The dashboard draws headers as separators joined to the list border
(`├─▾ Needs me 3  ● 2 ──┤`), adds a column header, a spinner or state glyph
before the name, `N of M shown` in the list's title when rows are hidden,
per-state count pills above the list, and a detail panel for the selected
row or group.

**Activity times.** Tower tracks them itself:

- For each session (keyed by `sessionId`, or `id` when that is missing) tower
  keeps the last observed tuple (`state`, `status`, `waitingFor`) and the time it
  first observed that tuple. The pair is stored in the sidecar state file as
  `last_state` and `last_change_ms` ([0002](0002-tech-stack-and-files.md)).
- A changed tuple on a poll sets the change time to the poll time. A changed
  tuple also drops the session's `acknowledged` tuple ([0007](0007-visual-only-attention.md)).
  *Amended by [0027](0027-rename-sessions.md):* while tower wakes a stopped
  session to rename it, its tuples are not recorded; once it is listed stopped
  again, that tuple is taken without a new change time.
- On the first sighting of a session (no record, or a record without
  `last_state`) the change time is `startedAt`, or the poll time when the row
  has none.
- Only the rows of the listing that just arrived are compared. The last good
  rows of other accounts, merged again after another tower's change to the
  sidecar is read in, are not, so an older listing never records its tuple
  over a newer one another tower saw.
- Saves are debounced: changed activity times alone cause at most one sidecar
  save per poll cycle; a change seen after that cycle's save goes out with the
  next cycle. Owner changes still save at once.
- When two towers both changed a session's tuple, the merge on save keeps the
  newer change; when both saw the same tuple, it keeps the earlier time
  ([0017](0017-ownership-evidence-rules.md) section 8).
- `tower status --json` reports the same value as `lastChangeAt`, read-only
  ([0010](0010-orchestrator.md)).
- Changes that happened while tower was not running are not observable: the
  row shows the time tower first saw the new tuple. This is a known limitation,
  not an error.
- Entries for sessions no longer listed, even with `--all`, are dropped from the sidecar.

Tower does not scrape the agent view or read transcripts to get a summary or a
better activity time.

## Consequences

- At most one sidecar write per poll cycle for observed changes; no extra CLI
  calls for times.
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
