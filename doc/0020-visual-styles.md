# 0020. Visual styles: dashboard and minimal

- Status: Accepted · 2026-10-19 · colour section superseded by [0021](0021-dashboard-themes.md) · spinner only for `state: working`, amended by [0032](0032-background-task-after-a-finished-turn.md) · `git` line and a four-line strip added by [0037](0037-git-summary-and-shell.md) · minimal columns and window titles fitted to the width, amended 2026-10-21 · values wrap, titles cut with `…`, two-row key line by [0041](0041-details-panel-values-wrap.md) · group separators and the status line fitted to the width, amended 2026-10-21 (UI audit) · plan usage in the header, the details panel and the minimal title added by [0043](0043-plan-usage-display.md) · a third, opt-in style by [0045](0045-workspace-style.md) · tower's own title in the terminal by [0047](0047-tab-title-pinning-and-restart.md)
- Related: [0002](0002-tech-stack-and-files.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [0018](0018-account-groups-and-handoff-guard.md)

## Context

The owner asked for a richer look than the single-column list. Four mockups
were rendered from the same sample data with ratatui's `TestBackend` at
160×40, 100×30, 80×24 and 60×20 and compared: a "refined minimal" list
with colour pills, a btop-like "dashboard" with panels and a detail panel,
a k9s-like "powerline" bar layout (with and without a Nerd Font), and
today's list as the baseline. The dashboard turns the empty right half of a
wide terminal into information about the selected session, which none of
the others do. Its mockup also had a per-row "last 10 minutes" state
timeline.

## Decision

**Two styles**, picked with `[ui] style` in `config.toml`
([0002](0002-tech-stack-and-files.md)): `"dashboard"` (the default) and
`"minimal"`, today's list, kept unchanged. *Amended by [0045](0045-workspace-style.md):* also the opt-in,
experimental `"workspace"`, the list beside Claude's real terminal; its windows and theme
are the dashboard's, and it does not change these two. *Amended 2026-10-21 (owner
request to fix cut text):* the minimal list's columns are fitted to the
width, as the dashboard's are: folders first become `…/api`, then state
details their short form (`permission`, `reply`), then the project and
then the name are cut with `…`; the age and account columns always stay.
Where every column fits, the list is drawn as before. Any other value is a config
error that names both. The style changes presentation only: lines, order,
selection, collapse, filter, keys, attention and orchestrator handling come
from the same app state in both ([0006](0006-session-states-and-grouping.md),
[0007](0007-visual-only-attention.md), [0011](0011-keybindings.md)).

**Dashboard layout** (rounded panels):

- Header panel: `◆ tower`, the grouping (`by state` / `by project`), a
  count pill per non-empty state group in its colour (Orchestrator
  included), followed by `● n` when the group has highlighted rows, and on
  the right each account as a chip with its group and session count. From
  26 rows, and only with colour, a bar shows each state's share. Narrower
  headers use short pill labels, then drop the account groups, then the
  accounts, then the Stopped and Interactive pills. *Amended by
  [0043](0043-plan-usage-display.md):* while the plan usage display is on, each account's
  chip is followed by its `5h 24% · 7d 12%`; after the account groups the
  week's number goes first, then a stale value's age, then the numbers,
  before the accounts.
- Sessions panel: `N of M shown` in its title when folded groups, the
  filter or `i` hide rows, else the session count; a column header from 26
  rows; each group as a separator joined to the border
  (`├─▾ Needs me 2  ● 2 ──┤`, `▸` when folded; *amended 2026-10-21 (UI
  audit):* the arrow, count and `●` count stay, with at least three `─`
  before the border, and a long folder becomes `…/last`, then is cut
  with `…`, [0006](0006-session-states-and-grouping.md)); rows with the attention
  marker, a Braille spinner for busy rows (one frame per redraw tick, no
  extra wake-ups) or a `▪` in the group colour, name, project, state
  detail, age and account. Interactive rows are dimmed. Narrow lists shorten
  folders to `…/api`, then state details (`permission`, `reply`). The list
  scrolls to keep the selection in view.
- Detail panel for the selected line: what it waits for (with `●` when
  highlighted), group, raw `state`, `status` with `waitingFor`, time since
  the last change, folder, account with group and command, short id, kind,
  whether a hand-off target exists ([0018](0018-account-groups-and-handoff-guard.md)),
  and the keys that apply. A selected group header shows a summary: rows,
  highlighted rows, accounts, and the states inside a project. From 130
  columns it is a 48-column panel right of the list; below that a
  three-line strip under the list, shown from 26 rows. *Amended by
  [0037](0037-git-summary-and-shell.md):* a `git` line (branch, changed
  files, commits not pushed) follows the folder when the folder is a git
  work tree, and the strip has a fourth line for it. *Amended by
  [0041](0041-details-panel-values-wrap.md):* in the side panel the name,
  status, folder and `git` values wrap onto up to 3 rows each, using only
  free rows; the strip's halves and both titles are cut with `…`, and a
  shortened side-panel title puts the full name in a `name` line; the keys
  may take a second row.
- The logs panel ([0009](0009-stop-remove-and-logs.md)), while open, takes
  the detail panel's place by the same rule: right of the list from 130
  columns, under it below that, half the space either way.
- Status line and footer carry the same content and rules as the minimal
  style (counter first, segments, whole key hints dropped least used first,
  `? help` and the detach hint kept), in the dashboard's palette.
  *Amended 2026-10-21 (UI audit):* in both styles the status line is
  fitted to the width. After the attention counter, whole segments are
  kept in priority order: errors, then the view's state, progress and
  messages (filter, hidden interactive rows, hand-off, rename, reply,
  attach check, status message), then warnings, each in their usual
  order, and `refreshed …` (or `loading…`) last. The first segment that
  does not fit is the last one shown: its folders become `…/last` and
  its long words are shortened with `…`, then its end is cut with `…`;
  with fewer than 12 columns left it is left out. The line never starts
  or ends with `·`.
- Overlays, dialogs, pickers and help are the same windows in both styles,
  with the same text and layout; while the dashboard is drawn they take
  its theme's look, colours from [0021](0021-dashboard-themes.md).
  *Amended 2026-10-21 (owner request to fix cut text):* a window's or
  panel's title is fitted to its width less 4 columns. A window first
  grows to show its whole title, up to 100 columns and the screen less 4;
  past that the variable part (a session name) is shortened with `…` and
  the fixed words, counts and markers are kept, e.g.
  `Continue refactor-the-auth… under which account?`.

**Fallback.** The dashboard needs 80×24, the mockup's own minimum (its
borders cost 4 rows and 2 columns; at 60×20 the list already scrolls).
Below that tower draws the minimal layout and says once per run, in the
status line, that the dashboard needs 80×24. It returns to the dashboard
when the terminal grows.

**Colour.** A dark truecolor palette: a hue per state group, one per
account group in config order. With `NO_COLOR` no colour is emitted at all:
the selection is `▶` plus reversed, interactive rows dim, and attention
stays the `●` marker plus bold ([0007](0007-visual-only-attention.md)). No
BEL, OSC or title changes (*amended by [0047](0047-tab-title-pinning-and-restart.md):* tower's own `OSC 2`
title, in every style); only single-width glyphs (box drawing, block
elements, Braille, `● ◆ ▪ ▸ ▾`), no Nerd Font.

**No timeline.** The per-row state timeline is not built, and tower keeps
no per-session history beyond the last tuple and its change time
([0006](0006-session-states-and-grouping.md)).

## Consequences

- New users get the panels; anyone who prefers the old list sets
  `style = "minimal"`. Tests of the minimal list run unchanged.
- The palette assumes a dark background; a light terminal shows it with
  less contrast until themes exist.
- The library's `App` starts with the minimal renderer; the runtime applies
  the configured style, as it applies `NO_COLOR`.

## Alternatives considered

- **Refined minimal** (today's layout with colour pills, a column header and
  account chips). Not built: a smaller gain than the detail panel, and the
  minimal style stays as the plain option.
- **Powerline / k9s** bars and an info block. Dropped: the solid separators
  need a Nerd Font to look right, the info block costs 6 rows and repeats
  the header and footer.
- **A per-row timeline of the last 10 minutes.** Dropped by the owner: it
  needs an in-memory history per session and a change to 0006 for a small
  gain. It can come back as its own decision.

## Open items

- Themes (`light`, `ansi16` for 16-colour terminals and the Linux console),
  and detecting truecolor support from `COLORTERM`.
- Optional state icons (`unicode`, `nerd-font`) as a separate setting.
