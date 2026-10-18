# 0011. Keybindings

- Status: Proposed · 2026-10-18
- Related: [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0009](0009-stop-remove-and-logs.md), [0010](0010-orchestrator.md), [0004](0004-account-hand-off.md)

## Context

Tower is used many times a day, often over SSH. Keys should be few, single
strokes, and familiar: arrows for everyone, `j`/`k` for vim users, lazygit-style
single letters for actions, and `Space` to peek, as in Claude Code's agent view.

Tower only receives keys while it is in the foreground. While a session is
attached, every key goes to Claude Code, where `←` (empty prompt) opens the
built-in agent view and `Ctrl+Z` returns to tower ([0005](0005-attach-and-detach.md)).

## Decision

**Session list**

| Key | Action |
|---|---|
| `↑` / `↓`, `k` / `j` | Move selection |
| `PgUp` / `PgDn` | Move one page |
| `Home` / `End`, `g` / `G` | First / last row |
| `Enter` | Attach to the selected session; on a group header, collapse/expand |
| `Tab` | Collapse/expand the group of the selected row |
| `Space` | Toggle the logs panel for the selected row ([0009](0009-stop-remove-and-logs.md)) |
| `n` | New session dialog ([0008](0008-new-session-dialog.md)) |
| `a` | Continue under another account ([0004](0004-account-hand-off.md)) |
| `o` | Attach to the orchestrator, starting it if missing ([0010](0010-orchestrator.md)) |
| `s` | Stop the session (confirm when working or blocked) |
| `d` | Remove the session (confirm) |
| `m` | Mark the highlighted session as seen ([0007](0007-visual-only-attention.md)) |
| `/` | Text filter; `Esc` clears it |
| `p` | Toggle grouping: by state / by project ([0006](0006-session-states-and-grouping.md)) |
| `i` | Show / hide interactive sessions |
| `r` | Refresh now |
| `?` | Help overlay listing all keys |
| `q`, `Ctrl+C` | Quit; asks for confirmation only if a dialog is open |
| `Esc` | Close the topmost panel, overlay or filter |

**Confirmation prompts:** `y` confirms, `n` or `Esc` cancels; the default is cancel.

**New-session dialog** (field layout is defined in [0008](0008-new-session-dialog.md))

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Next / previous field |
| `←` / `→`, `↑` / `↓` | Move the cursor or change a choice inside a field |
| `Enter` | In the task field: new line; elsewhere: next field |
| `Ctrl+G` | Edit the task in `$VISUAL`, else `$EDITOR` |
| `Ctrl+S` | Dispatch to the background |
| `Ctrl+Enter`, fallback `Alt+Enter` | Dispatch and attach immediately |
| `Esc` | Cancel (confirm if the task is not empty) |

Many terminals do not report `Ctrl+Enter` distinctly, especially over SSH.
Tower enables the keyboard-enhancement protocol where the terminal supports it
and always accepts `Alt+Enter` as well; the footer of the dialog shows the
variant that works.

**Rules**

- Tower never binds `←` in the session list; it is Claude Code's key and the
  user's muscle memory for "back to the agent list" should not do something
  different in tower. Inside tower's own dialogs `←` / `→` move the cursor,
  which is safe because those keys only reach tower when it is in the foreground.
- `Ctrl+Z` is reserved for detaching from Claude Code. Tower never suspends
  itself; in raw mode `Ctrl+Z` arrives as a key and tower treats it as a no-op,
  optionally with a hint that it is the detach key inside sessions.
- `l` is not used for logs because it means "right" to vim users.
- The footer always shows the most used keys and the detach hint
  (`in a session: Ctrl+Z returns to tower`).
- Keys are fixed in v1. The key handling is table-driven so that remapping
  through the config file can be added later.

## Consequences

- Every action is one keystroke; destructive ones (`d`, and `s` on a working or blocked session) need a second.
- No conflict with Claude Code's own keys, because tower and the attached
  session never receive keys at the same time and tower leaves `←` and `Ctrl+Z` alone.
- `Ctrl+Enter` may not work in a given terminal; the `Alt+Enter` fallback and
  the footer hint cover that.

## Alternatives considered

- **Full vim set (`h`/`l` for navigation, `l` for logs).** Rejected: `l` as logs clashes with "right"; horizontal movement is not needed in the list.
- **Copying the agent view's keys exactly.** Partly taken (`Space` to peek); `←` stays with Claude Code.
- **Modifier-heavy bindings (`Ctrl+N`, …).** Rejected: some combinations are swallowed by terminals or SSH clients; single letters are more portable.
- **User-configurable keys in v1.** Deferred.

## Open items

- Final review of the whole table by the owner.
- Confirm which of `Ctrl+Enter` / `Alt+Enter` are reported over SSH in the
  terminals the project is tested with.
- Whether `Ctrl+S` / `Ctrl+G` clash with anything in common terminal setups.
