# 0032. A background task that outlives a finished turn

- Status: Accepted · 2026-10-19 · how the status sentence wraps amended by [0041](0041-details-panel-values-wrap.md)
- Amends: the row content, styles and detail panel of [0006](0006-session-states-and-grouping.md) and [0020](0020-visual-styles.md)
- Related: [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0010](0010-orchestrator.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [reference](reference/claude-code-cli.md#background-shells-and-the-turn)

## Context

Tower groups a row by its `state` ([0006](0006-session-states-and-grouping.md))
and, in the dashboard, draws a spinner for every live row with `status: busy`
([0020](0020-visual-styles.md)). Claude Code can list a session as
`state: done` with `status: busy`: its turn has finished, but a background
task it started, such as a shell, is still running, and its footer says
`1 shell`. When the task ends Claude Code tells the session, which may start
a new turn by itself ([reference](reference/claude-code-cli.md#background-shells-and-the-turn)).
Such a row sat in Ready for review with the label `done` and the busy spinner,
so it looked finished and working at once.

## Decision

- **Grouping is unchanged.** The row stays in the group its `state` gives it
  ([0006](0006-session-states-and-grouping.md)): `done` + `busy` is in Ready
  for review, the orchestrator keeps its own group.
- **The spinner is drawn only for `state: working`** with `status: busy`, in
  the row and in the detail panel's status line.
- **A background task.** A live background row with `status: busy` in any other
  `state` (`done`, `blocked`, `failed`, an unknown value) gets its state's
  normal `▪` glyph, and its STATE column appends ` · bg task` in both styles:
  `done · bg task`, the orchestrator's `ready · bg task`. Where the dashboard
  shortens state details for a narrow list it becomes ` · bg` (`done · bg`,
  `reply · bg`); a column still too narrow is cut with `…` as before.
- **Detail panel** (dashboard side panel): the status line reads
  `busy — a background task (e.g. a shell) is still running; Claude Code may
  continue this session when it ends`, wrapped at spaces to the panel width
  and continued under the value column. *Amended by
  [0041](0041-details-panel-values-wrap.md):* it wraps like the panel's other
  values (also after `/` and `-`), whole while the panel has the rows and cut
  with `…` in a short panel. The three-line strip shows the compact
  `busy (bg task)` in its `state · status · changed` line. The minimal style has
  no detail panel; its STATE column carries the same meaning.
- **Attention is unchanged.** No new highlight rule: a `done` + `busy` row is
  highlighted, or not, exactly as before ([0007](0007-visual-only-attention.md)).
- `tower status --json` is unchanged: `detail` stays the group detail
  (`done`), and `state` and `status` are reported raw
  ([0010](0010-orchestrator.md)).

## Consequences

- A finished turn with a running shell no longer looks like a working session,
  and the row says why it may change again without the user.
- The rule needs no new CLI call or field: it reads `state` and `status` only.
- The STATE column can grow by 10 columns; the dashboard's shortening levels
  and the column caps of both styles absorb it.

## Alternatives considered

- **Move `done` + `busy` rows to Working.** Rejected: the turn has finished and
  the reply is ready for review; grouping by `state` stays the single rule.
- **Keep the spinner and only change the label.** Rejected: the spinner is what
  says "working" at a glance, and that is what was misleading.
- **Highlight such rows.** Rejected: nothing waits for the user.
