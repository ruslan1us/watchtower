# 0007. Attention is visual only

- Status: Accepted · 2026-10-18
- Related: [0002](0002-tech-stack-and-files.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md)

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

**When the highlight clears.**

- The user attaches to the session ([0005](0005-attach-and-detach.md)), or
- the user acknowledges it with `m` ("mark seen") on the row ([0011](0011-keybindings.md)), or
- the session leaves the Needs me / Failed groups.

Acknowledgement is stored per session in the sidecar as `acknowledged`, the
tuple it acknowledged ([0002](0002-tech-stack-and-files.md)). A later transition (for example a second permission prompt
after the first was answered) highlights the row again.

**How it looks.**

- A marker glyph in a fixed leading column (`●`) plus bold text, so the
  highlight is readable without colour.
- With colour enabled, an accent foreground on the marker and the state
  column. `NO_COLOR` is honoured; the glyph and bold remain.
- A counter in the status bar, e.g. `2 need you`, counting all highlighted
  sessions, including those hidden by a filter or a collapsed group.
- Group headers show how many of their rows are highlighted.

Highlights are computed from polling, which continues while the user is
attached to another session, so new highlights are already there on return.

**What tower never does.** It never writes BEL (`\a`), never plays sound,
never emits OSC 9, OSC 777 or OSC 99 desktop-notification sequences, and does
not change the terminal or tab title to signal attention. No option to enable
these exists in v1.

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
