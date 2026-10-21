# 0011. Keybindings

- Status: Accepted · 2026-10-18 · `e` rename key added by [0027](0027-rename-sessions.md) · logs panel focus and keys amended by [0028](0028-scrolling-the-logs-panel.md) · `Ctrl+P` template key added by [0036](0036-prompt-templates.md)
- Related: [0028](0028-scrolling-the-logs-panel.md), [0019](0019-resume-past-sessions.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0009](0009-stop-remove-and-logs.md), [0010](0010-orchestrator.md), [0004](0004-account-hand-off.md), [0018](0018-account-groups-and-handoff-guard.md), [0024](0024-settings-window.md), [0027](0027-rename-sessions.md), [0036](0036-prompt-templates.md)

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
| `↑` / `↓`, `k` / `j` | Move selection; it moves over group headers and rows |
| `PgUp` / `PgDn` | Move one page |
| `Home` / `End`, `g` / `G` | First / last line |
| `Enter` | Attach to the selected session after a refresh ([0005](0005-attach-and-detach.md)); on a group header, collapse/expand |
| `Tab` | Collapse/expand the group of the selected row or header ([0006](0006-session-states-and-grouping.md)) |
| `Space` | Toggle the logs panel for the selected row ([0009](0009-stop-remove-and-logs.md)) |
| `→`, `l` | *Added by [0028](0028-scrolling-the-logs-panel.md):* give the keys to the open logs panel to scroll it; its own keys (lines, pages, top, bottom, `f` full screen, `←` / `h` / `Esc` back) are listed there |
| `n` | New session dialog ([0008](0008-new-session-dialog.md)) |
| `a` | Continue under another of the user's accounts ([0004](0004-account-hand-off.md)); always confirm, same group only unless `allow_cross_group_handoff = true` ([0018](0018-account-groups-and-handoff-guard.md)) |
| `o` | Attach to the orchestrator as `Enter` on its row would, starting it if missing ([0010](0010-orchestrator.md)); after a `Workspace not trusted` start a window offers `Ctrl+T` (run the account's command in its folder, then retry once) and `Esc` |
| `R` | Resume a past conversation into the background: a window picks the account and project folder, then Claude Code's own `--resume` picker runs there ([0019](0019-resume-past-sessions.md)) |
| `s` | Stop the session (confirm when working or blocked) |
| `d` | Remove the session (confirm) |
| `m` | Mark the selected highlighted session as seen: stores its current tuple as acknowledged, so the highlight clears until its next transition ([0007](0007-visual-only-attention.md)); on a row that is not highlighted the status line says so |
| `/` | Text filter: type or edit it in the input line ([0006](0006-session-states-and-grouping.md)); `Esc` clears it |
| `p` | Toggle grouping: by state / by project ([0006](0006-session-states-and-grouping.md)) |
| `i` | Show / hide interactive sessions (shown by default) |
| `r` | Refresh now |
| `c` | Settings window ([0024](0024-settings-window.md)) |
| `e` | Rename the session ([0027](0027-rename-sessions.md)): a window with the name; a stopped session is renamed from tower, a running one inside it after an attach |
| `?` | Help overlay listing all keys |
| `q`, `Ctrl+C` | Quit; asks for confirmation only if a dialog is open or a hand-off runs |
| `Esc` | Cancel an attach waiting for its refresh; else close the topmost panel, overlay or filter |

**Filter input line** ([0006](0006-session-states-and-grouping.md)): typed
characters, including list letters such as `q` or `j`, go into the filter, and
the list follows as you type. `Backspace` deletes a character and `Ctrl+U`
empties the line. `↑` / `↓` move the selection. `Enter` keeps the filter and
gives the keys back to the list; `Esc` clears it. `Ctrl+C` quits as in the
list, `Ctrl+Z` stays a no-op, and `←` is not bound.

**Confirmation prompts:** `y` confirms, `n` or `Esc` cancels; the default is cancel, so
`Enter` cancels too.

**Hand-off** ([0004](0004-account-hand-off.md), [0018](0018-account-groups-and-handoff-guard.md)):
`a` offers only the accounts the session may go to: the other accounts of the owner's
`group`, or every other account when `allow_cross_group_handoff = true`. With more than one
candidate it opens a target picker (`↑` / `↓`, `Enter` continues, `Esc` cancels). Every
hand-off then asks for confirmation, naming both accounts with their groups and saying what
moves, e.g. with `allow_cross_group_handoff = true`:

```text
Continue fix-flaky-test (1a2b3c4d) under alt? It is idle.
From main (work) to alt (personal): tower will stop under main, continue under alt.
The full conversation and project context will continue under alt (personal) and its terms.
```

When no account is allowed, `a` opens nothing and the status line gives the reason, e.g.
`hand-off from main (work) to alt (personal) is disabled: different account groups; set
allow_cross_group_handoff = true to allow`. With an unknown owner (`?`) while the accounts
span several groups it says that the session's account is unknown and the accounts are in
different groups, and to attach first (`Enter`) so tower learns the owner. An account
without `group` is shown as `(no group)` in these messages. After a `Workspace not
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
| `Ctrl+P` | *Added by [0036](0036-prompt-templates.md):* open the template picker from any field; the pick goes into the task field at its cursor (a blank field is replaced) |
| `Ctrl+S` | Dispatch to the background |
| `Ctrl+Enter`, fallback `Alt+Enter` | Dispatch and attach immediately |
| `Ctrl+T` | After a `Workspace not trusted` error: suspend tower and run the account's interactive command in the project to accept trust, then retry the dispatch ([0008](0008-new-session-dialog.md)) |
| `Esc` | Cancel (confirm if the task is not empty) |

**Template picker** (*added by [0036](0036-prompt-templates.md)*, `Ctrl+P` in
the dialog): typed characters filter by name, `Backspace` edits and `Ctrl+U`
empties the filter, `↑` / `↓` (`PgUp` / `PgDn`) choose, `Enter` inserts the
template, `Esc` closes the picker only; `Ctrl+C` closes it and asks the
dialog's quit question.

**Resume window** (`R`, [0019](0019-resume-past-sessions.md)): `Tab` /
`Shift+Tab` switch between the project and account fields; in the project
field typed characters filter and `↑` / `↓` choose, in the account field
`←` / `→` (or `↑` / `↓`) change it; `Enter` runs the picker, `Esc` cancels,
`Ctrl+C` quits as in the list.

**Rename window** (`e`, [0027](0027-rename-sessions.md)): the `Name`
field takes the editing keys of every single-line field (`←` / `→`,
`Home` / `End`, `Backspace`, `Del`) and `Ctrl+U` empties it; `Enter`
renames (a stopped session) or attaches (a running one, where the user types
`/rename <name>` and `Ctrl+Z` returns), `Esc` cancels, `Ctrl+C` quits as in
the list. After a `Workspace not trusted` failure a window offers `Ctrl+T`
(run the owner's command in the folder, then retry once) and `Esc` (leave
the session as it was). `e` on an interactive row or the orchestrator opens
nothing and the status line says why.

**Settings window** (`c`, [0024](0024-settings-window.md)): `↑` / `↓`
(`k` / `j`, `PgUp` / `PgDn`, `Home` / `End`) move over the settings and the
read-only account rows, `Tab` / `Shift+Tab` jump to the next / previous
section; `Enter` edits (a pick list for a choice, a toggle, an inline input
for text and numbers, the list editor for folders); `←` / `→` step a choice
or toggle a bool; `Del` resets to the default; `Ctrl+S` saves and keeps the
window open; `e` opens `config.toml` in `$VISUAL` / `$EDITOR` (asking first
about unsaved changes: `y` save, `n` discard, `Esc` cancel); `?` lists these
keys; `Esc` closes, asking first when something is unsaved (`y` save and
close, `n` discard, `Esc` / `Enter` keep editing); `Ctrl+C` quits, asking
first when something is unsaved. In a pick list `↑` / `↓` move (the style
and theme preview at once), `Enter` keeps, `Esc` cancels. In an inline input
`Enter` keeps a valid value, `Esc` undoes the edit, `Ctrl+U` clears. In the
list editor `a` adds, `Enter` edits, `d` removes, `Esc` goes back; while a
folder is typed `Tab` completes its name, `Enter` keeps it, `Esc` cancels.
`←` / `→` inside this window follow the rule for tower's own dialogs below.

In the project field, typed characters filter the list and `↑` / `↓` choose
a project; in the model field `↑` / `↓` step through the presets and typing
gives a free-text model name. A line feed (`Ctrl+J`, which some terminals send
for `Ctrl+Enter`) is a new line in the task field and never dispatches.

Many terminals do not report `Ctrl+Enter` distinctly, especially over SSH.
Tower enables the keyboard-enhancement protocol where the terminal supports it
and always accepts `Alt+Enter` as well; the footer of the dialog shows the
variant that works.

**Non-Latin keyboard layouts.** Without the kitty keyboard protocol a
terminal reports the character a key types, not the physical key, so with a
Russian or Ukrainian layout active the `n` key arrives as `т`. Tower maps
such characters back to the Latin key on the same physical key of a QWERTY
keyboard, using the standard ЙЦУКЕН layout (`й`→`q`, `ф`→`a`, `т`→`n`, …;
shifted letters give the shifted key, `К`→`R`). It is always on, with no
setting.

- Command contexts translate plain keys: the session list, confirmations
  (`н` is `y`, `т` is `n`), the account and target pickers, the help
  overlay, popups, the trust windows, the quit question, the dialog's own
  questions, and the Settings window outside its text fields (rows, pick
  lists, the list editor and its questions: `с` opens it, `н` / `т`
  answer). `у` opens the rename window. There the RU layout's `.` and `,` (typed on the `/` key) also act
  as `/` and `?`, so the filter and help open; `.` and `,` are not bound
  anywhere else.
- Text input is never translated: the new-session dialog's fields (task,
  name, model, project filter), the template picker's filter (*added by
  [0036](0036-prompt-templates.md)*), the resume window's project filter, the `/`
  filter line, the rename window's name and the Settings window's inline
  input and folder input keep Cyrillic as typed. There, and everywhere else, only a key with `Ctrl` or
  `Alt` held is translated, so `Ctrl+ы` is `Ctrl+S` and
  `Ctrl+с` is `Ctrl+C` when the terminal reports them that way.
- Limits: only the RU and UA ЙЦУКЕН letters are mapped (UA `і`→`s`, `ї`→`]`,
  `є`→`'`, `ґ`→`\`, as on the xkb `ua` layout; RU `ё`→`` ` ``). Other
  layouts (Greek, Hebrew, Dvorak, …) are not mapped, and tower does not use
  the kitty protocol's base-layout key reporting.

**Rules**

- Tower never binds `←` in the session list; it is Claude Code's key and the
  user's muscle memory for "back to the agent list" should not do something
  different in tower. Inside tower's own dialogs `←` / `→` move the cursor,
  which is safe because those keys only reach tower when it is in the foreground.
  *Amended by [0028](0028-scrolling-the-logs-panel.md):* with the logs panel
  focused, `←` returns to the list, the same "back" meaning.
- `Ctrl+Z` is reserved for detaching from Claude Code. Tower never suspends
  itself; in raw mode `Ctrl+Z` arrives as a key and tower treats it as a no-op,
  optionally with a hint that it is the detach key inside sessions.
- `l` is not used for logs because it means "right" to vim users.
  *Amended by [0028](0028-scrolling-the-logs-panel.md):* `l` moves the focus
  right into the open logs panel, `h` back, which keeps that meaning.
- The footer always shows the most used keys (including `m seen`) and the
  detach hint (`in a session: Ctrl+Z returns to tower`). On a narrow terminal
  it drops whole key hints, least used first, and keeps `? help` and the
  detach hint.
- The help overlay fits a 24-row terminal: related list keys share a line
  (`s, d`, `p, i`, `a, e`, `c, r, ?`, …), followed by the dialog keys. The Settings
  window has its own key list (`?` there). *Amended by
  [0028](0028-scrolling-the-logs-panel.md):* opened from the focused logs panel,
  the help lists the panel's keys instead of the dialog keys, and the footer
  shows the panel's keys while it has the focus.
- Keys are fixed in v1. The key handling is table-driven so that remapping
  through the config file can be added later.
- A window that a background result opens (the hand-off, orchestrator and
  rename trust windows, the full text of an `rm` refusal, the account picker after
  an attach's refresh) never replaces or covers a window that is open: the open
  confirmation, picker, help, target picker, popup, new-session dialog or
  quit question keeps the keys. The new window waits in a queue, the status
  line says `1 window is waiting; it opens when this one closes`, and queued
  windows open in arrival order once nothing else is open.
- Inside a popup, `Ctrl+C` follows the rules of the window beneath it: over
  the list it quits, asking the hand-off question while a hand-off runs;
  over the new-session dialog it is the dialog's `Ctrl+C`, so the dialog asks
  its own question and the popup waits until the dialog closes.

## Consequences

- Every action is one keystroke; destructive or account-changing ones (`d`, `a`, and `s` on a working or blocked session) need a second.
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
