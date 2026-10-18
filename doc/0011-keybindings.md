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
| `Enter` | Attach to the selected session after a refresh ([0005](0005-attach-and-detach.md)); on a group header, collapse/expand |
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
| `q`, `Ctrl+C` | Quit; asks for confirmation only if a dialog is open or a hand-off runs |
| `Esc` | Cancel an attach waiting for its refresh; else close the topmost panel, overlay or filter |

**Confirmation prompts:** `y` confirms, `n` or `Esc` cancels; the default is cancel, so
`Enter` cancels too.

**Hand-off** ([0004](0004-account-hand-off.md)): with more than one candidate account, `a`
opens a target picker (`↑` / `↓`, `Enter` continues, `Esc` cancels). After a `Workspace not
trusted` failure a window offers `Ctrl+T` (run the target account's command in the folder,
then retry once, as in the dialog) and `Esc` (leave the session stopped). While a hand-off
runs, `q` and `Ctrl+C` (from any window, including the new-session dialog) ask "a hand-off
of X is running; quitting may leave it stopped — quit?": `y` quits; `n`, `Esc` and `Enter`
keep tower running, and other keys are ignored. Enter, `s`, `d` and `a` on the session being
handed off are refused until it reports back.

**New-session dialog** (field layout is defined in [0008](0008-new-session-dialog.md))

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Next / previous field |
| `←` / `→`, `↑` / `↓` | Move the cursor or change a choice inside a field |
| `Enter` | In the task field: new line; elsewhere: next field |
| `Ctrl+G` | Edit the task in `$VISUAL`, else `$EDITOR` |
| `Ctrl+S` | Dispatch to the background |
| `Ctrl+Enter`, fallback `Alt+Enter` | Dispatch and attach immediately |
| `Ctrl+T` | After a `Workspace not trusted` error: suspend tower and run the account's interactive command in the project to accept trust, then retry the dispatch ([0008](0008-new-session-dialog.md)) |
| `Esc` | Cancel (confirm if the task is not empty) |

In the project field, typed characters filter the list and `↑` / `↓` choose
a project; in the model field `↑` / `↓` step through the presets and typing
gives a free-text model name. A line feed (`Ctrl+J`, which some terminals send
for `Ctrl+Enter`) is a new line in the task field and never dispatches.

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
- A window that a background result opens (the hand-off trust window, the
  full text of an `rm` refusal, the account picker after an attach's
  refresh) never replaces or covers a window that is open: the open
  confirmation, picker, help, target picker, popup, new-session dialog or
  quit question keeps the keys. The new window waits in a queue, the status
  line says `1 window is waiting; it opens when this one closes`, and queued
  windows open in arrival order once nothing else is open.
- Inside a popup, `Ctrl+C` follows the rules of the window beneath it: over
  the list it quits, asking the hand-off question while a hand-off runs;
  over the new-session dialog it is the dialog's `Ctrl+C`, so the dialog asks
  its own question and the popup waits until the dialog closes.

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
- Whether `Ctrl+S` / `Ctrl+G` / `Ctrl+T` clash with anything in common terminal setups.
