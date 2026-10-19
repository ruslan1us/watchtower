# 0007. Attention is visual only

- Status: Accepted · 2026-10-18 · a rename's wake is never highlighted, amended by [0027](0027-rename-sessions.md)
- Related: [0002](0002-tech-stack-and-files.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0010](0010-orchestrator.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md)

## Context

Sessions regularly stop and wait for the user: a permission prompt, a question,
a failed run. The user must notice this without watching every row. Claude Code
has built-in notifications (`Notification` hook types `agent_needs_input` and
`agent_completed`), but they fire only while the built-in agent view is open
(https://code.claude.com/docs/en/agent-view), so tower has to detect changes
itself by polling.

The owner's requirement is explicit: attention is shown in the terminal only.
No bell, no sound, no desktop notification.

## Decision

**When a row is highlighted.** Tower highlights a session when it observes a
transition into the "Needs me" or "Failed" group ([0006](0006-session-states-and-grouping.md)),
i.e. into `state: blocked`, `status: waiting` or `state: failed`. Sessions that
are already in one of these states when tower starts are highlighted too,
unless the sidecar records that the same (`state`, `status`, `waitingFor`) tuple
was already acknowledged.

Only background rows are highlighted. Interactive rows (`kind: interactive`, no
`id`, not attachable) stay in their own dimmed group ([0006](0006-session-states-and-grouping.md))
and are never highlighted or counted in the attention counter.

The orchestrator ([0010](0010-orchestrator.md)) is never highlighted either.
It ends almost every turn with a question, which Claude Code reports as
`state: blocked`, `status: idle` with no `waitingFor`
([CLI reference](reference/claude-code-cli.md#json-listing-claude-agents---json)),
so it would sit in Needs me and in the counter all the time. Waiting for the
user is its normal state, not a signal. Its rows have their own Orchestrator
group at the top ([0006](0006-session-states-and-grouping.md)): no marker, no
header count, not in the attention counter, whatever its state, `failed`
included (shown as plain text). `m` on it says it is not highlighted.

Put exactly: a background row is highlighted while it is in the Needs me or
Failed group and the sidecar's `acknowledged` tuple is not its current
(`state`, `status`, `waitingFor`) tuple. Stopped rows (no live process) are in
neither group, whatever their `state`, and neither are orchestrator rows.

**When the highlight clears.**

- The user attaches to the session and the attach ends with exit code 0
  ([0005](0005-attach-and-detach.md)); a failed attach acknowledges nothing, or
- the user acknowledges it with `m` ("mark seen") on the row ([0011](0011-keybindings.md)), or
- the session leaves the Needs me / Failed groups.

Both acknowledgements store the row's current tuple, as of tower's latest
listing, in the sidecar as `acknowledged` ([0002](0002-tech-stack-and-files.md)).
Only a row in Needs me or Failed is acknowledged; `m` on any other row says
it is not highlighted and stores nothing.

When tower observes the session's tuple change ([0006](0006-session-states-and-grouping.md)),
it drops `acknowledged`: the acknowledgement covered a state the session has
left. So a later transition highlights the row again, for example a second
permission prompt after the first was answered (`blocked` → `working` →
`blocked` with the same tuple), and so does a new tuple inside Needs me (a
question after a permission prompt). A change that happened while tower was not
running is noticed at the first poll after startup, against the sidecar's
`last_state`.

**How it looks.**

- A marker glyph in a fixed leading column (`●`) plus bold text, so the
  highlight is readable without colour.
- With colour enabled, an accent foreground on the marker and the state
  column. `NO_COLOR` is honoured; the glyph and bold remain.
- A counter at the start of the status bar, `● 1 needs you`, `● 2 need you`, in
  the same style, counting all highlighted sessions, including those hidden by
  a filter or a collapsed group. It is not shown when nothing is highlighted.
- Group headers show how many of their rows are highlighted (`Needs me (3)  ● 2`).

Highlights are computed from polling, which continues while the user is
attached to another session, so new highlights are already there on return.
*Amended by [0027](0027-rename-sessions.md):* a session tower wakes to rename
it is not highlighted while the rename runs.

**What tower never does.** It never writes BEL (`\a`), never plays sound,
never emits OSC 9, OSC 777 or OSC 99 desktop-notification sequences, and does
not change the terminal or tab title to signal attention. No option to enable
these exists in v1.

**Nothing from the data reaches the terminal raw.** Session names, folders
and commands come from the CLI's JSON, the config or the environment, and
could carry BEL or escape sequences. ratatui filters what it draws; the lines
tower prints outside it (the announcements and key-pause prompts around a
hand-over ([0005](0005-attach-and-detach.md)) and error messages on exit)
replace every control character with `?`.

## Consequences

- Attention is only noticed when the tower screen is visible. This is accepted:
  the user returns to tower between sessions anyway.
- No dependence on terminal-specific notification support, which varies widely
  across terminals and SSH setups.
- No Claude Code hook or settings changes are needed.
- Acknowledgement state adds a small amount of data to the sidecar.

## Alternatives considered

- **Terminal bell on each transition, de-duplicated per session.** Rejected by the owner.
- **Desktop notifications through OSC 777 / OSC 9 / OSC 99.** Rejected by the owner; support is also inconsistent.
- **Claude Code's own `Notification` hooks (`agent_needs_input`, `agent_completed`).**
  Rejected: they fire only while the built-in agent view is open, and using them
  requires adding hook configuration to Claude Code's settings
  ([0016](0016-do-not-change-claude-code-defaults.md)).
- **Highlighting `done` sessions too.** Not done: "Ready for review" is its own
  group near the top and is not urgent. Can be revisited.
- **Terminal title with a count.** Rejected: signals outside the tower screen are out of scope.
