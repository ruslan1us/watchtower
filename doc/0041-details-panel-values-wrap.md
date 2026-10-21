# 0041. Long values in the details panel wrap

- Status: Accepted · 2026-10-21
- Related: [0020](0020-visual-styles.md), [0032](0032-background-task-after-a-finished-turn.md), [0037](0037-git-summary-and-shell.md), [0011](0011-keybindings.md)
- Amends: [0037](0037-git-summary-and-shell.md) (the `git` line is no longer one line in the side panel, the long-branch rule, zero counts), [0020](0020-visual-styles.md) (the panel's title, value lines and key line), [0032](0032-background-task-after-a-finished-turn.md) (how the status sentence wraps)

## Context

A UI audit drew every screen at nine sizes with long data: a 56-column
session name, a 66-column folder and a 60-column branch. In the 48-column
side panel, with 20 and more free rows below:

- the branch was cut although the rows were free:
  `git       feature/move-the-ses… · clean · ↑0`;
- `↑0` was shown for a branch with nothing to push;
- the folder ran into the border without `…`:
  `folder    ~/workspace/billing-platform-fronte│`, and so did the right half
  of the strip under the list (`folder    ~/workspace/billing-platform-fron│`);
- the title was clipped by the border, the full name shown nowhere:
  `╭ refactor-the-authentication-middleware-and-se╮`;
- the keys row dropped `s stop` and `! shell` although `s stop` fitted: the
  width check counted the gap after the last key, and the keys never used a
  second row.

## Decision

**Side panel values wrap.** The name (see titles), the status, the folder
and the `git` summary wrap onto up to 3 rows each, continued under the
value column. A row ends at a space (dropped), after a `/` or after a `-`,
at the last such place that fits; a part longer than a row is broken where
the row ends. A space next to a `·` is not a break, so a separator never
starts or ends a row. A value longer than its rows ends in `…`.

**Free rows only.** The values use the rows the panel has free: when every
value fits on its 3 rows, all of them are drawn whole; otherwise each gets
one row (cut with `…`) and the rows left over go to them in turn: folder,
`git`, status, name. A second row of keys gives its row back first. Every
other line stays.

**The `git` line** (amends [0037](0037-git-summary-and-shell.md)):

- zero counts are omitted: `↑N` and `↓M` only when above 0. With an
  upstream and both 0 nothing is shown for it: `feature/x · clean`.
  `no upstream` and `upstream gone` stay.
- In the side panel the branch is never shortened while the summary fits
  its 3 rows: `feature/move-the-session-store-` / `onto-the-new-cache-with-`
  / `retry · clean`. A branch too long for 3 rows is cut with `…` so the
  counts stay on the third.
- In the strip it stays one row across both halves, the branch cut with
  `…` first so the counts stay, as before.

**The status sentence of a background task**
([0032](0032-background-task-after-a-finished-turn.md)) is tower's own text
and is shown whole (4 rows in the 48-column panel) while the panel has the
rows; in a short panel it gets its turn like the other values.

**Single-row places cut with `…`.** In the strip both halves are cut to
their width with `…` (the right half was not fitted before), each keeping a
one-column margin.

**Titles.** The side panel's and the strip's titles are cut with `…` to fit
between the corners. When the side panel's title is shortened, the full
name is a body line under the headline, wrapped like a value:
`name      refactor-the-authentication-` / `middleware-and-session-store`.
A group's title is on its pill in the body, wrapped the same way.

**Keys.** Only the gaps between keys count toward the width, and the keys
may take a second row under the first. The order is the session's own
actions first and the hand-off last: `Enter attach`, `Space logs`,
`m seen` (highlighted rows), `s stop`, `! shell`, `a hand-off`. A key that
does not fit the first row starts the second; one that fits neither is
left out.

## Consequences

- A long folder, branch or name is readable in full without a wider
  terminal, and nothing in the panel runs into its border.
- The side panel's height now depends on the values; the rule and keys stay
  at the bottom.
- The footer and the minimal style are unchanged.

## Alternatives considered

- **Cut every value with `…` on one row.** The audit's frames had 20 and
  more free rows below the cut values.
- **Wrap only at spaces.** Folders and branches have no spaces; breaking
  after `/` and `-` keeps whole path and word parts on a row.
- **Wrap without a cap.** A pathological value (a very long branch) could
  push the account and hand-off lines out of a short panel.
