# Keys

Every key in tower, window by window. The workspace style's own keys are in [Workspace style](workspace-style.md). Back to the [README](../../README.md).

## Session list

Decision: [0011](../0011-keybindings.md).

| Key | Action |
|---|---|
| `↑` / `↓`, `k` / `j` | Move the selection (rows and group headers) |
| `PgUp` / `PgDn`, `Home` / `End`, `g` / `G` | One page; first / last line |
| `Enter` | Attach to the session (after a refresh); on a group header, collapse / expand |
| `Tab` | Collapse / expand the group of the selected line |
| `Space` | Toggle the logs panel for the selected session |
| `→`, `l` | Scroll the open logs panel (keys below) |
| `n` | New-session dialog |
| `a` | Continue under another of your accounts (hand-off; always asks, same group only by default) |
| `o` | Attach to the orchestrator, starting it if missing |
| `R` | Resume a past conversation: pick an account and folder, then Claude Code's own `--resume` picker; then `←` on an empty prompt and `Ctrl+C` twice to come back with it running in the background as a copy (`Esc` there reopens it) ([0019](../0019-resume-past-sessions.md)) |
| `s` | Stop the session (asks first when it is working or blocked) |
| `d` | Remove the session (asks first) |
| `m` | Mark a highlighted session as seen |
| `t` | [Quick reply](sessions.md#quick-reply): send the session a text as your next prompt (idle or stopped sessions) |
| `P` | [Pin or unpin](sessions.md#pinning-a-session) the session |
| `U` | [Restart](sessions.md#restarting-after-an-update) the session onto the installed Claude Code (asks first); on a group header, the group's running sessions |
| `Ctrl+U` | Restart every running session (asks first, per account; only `Y` goes) |
| `/` | Text filter on name, project and account; `Esc` clears it |
| `p` | Group by state / by project |
| `i` | Show / hide interactive sessions |
| `r` | Refresh now (also the [plan usage](plan-usage.md) older than a minute, when it is on) |
| `c` | Settings window ([0024](../0024-settings-window.md)) |
| `e` | Rename the session: a stopped one is woken with `/rename` and stopped again, without attaching; for a running one tower attaches and you type `/rename <name>` there, then `Ctrl+Z` ([0027](../0027-rename-sessions.md)) |
| `!` | A shell (`$SHELL`, else `/bin/sh`) in the session's folder; `exit` returns to tower ([0037](../0037-git-summary-and-shell.md)) |
| `?` | Help |
| `q`, `Ctrl+C` | Quit |
| `Esc` | Cancel a waiting attach, else close the panel, overlay or filter |

## Logs panel

After `→` ([0028](../0028-scrolling-the-logs-panel.md)):

| Key | Action |
|---|---|
| `↑` / `↓`, `k` / `j` | One line |
| `PgUp` / `PgDn`, `Ctrl+U` / `Ctrl+D` | One page |
| `Home` / `g`, `End` / `G` | Top; bottom, following the output again |
| `f` | Full screen on / off |
| `←`, `h`, `Esc` | Back to the list |
| `Space` | Close the panel |

Scrolled up, the panel pauses on what it shows and its title says
`paused · line 120–160 of 900 · End follows`; `End` refreshes it.

## New-session dialog

Decision: [0008](../0008-new-session-dialog.md).

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Next / previous field |
| `←` / `→`, `↑` / `↓` | Move the cursor or change a choice |
| `Enter` | New line in the task field; next field elsewhere |
| `Ctrl+G` | Edit the task in `$VISUAL`, else `$EDITOR` |
| `Ctrl+P` | Insert a [prompt template](sessions.md#prompt-templates) into the task |
| `/` (at the start of the task) | [Slash-command](sessions.md#slash-commands) menu: `↑` / `↓` choose, `Tab` / `Enter` insert, `Esc` closes the menu |
| `Ctrl+S` | Start in the background |
| `Ctrl+Enter`, `Alt+Enter` | Start and attach |
| `Ctrl+T` | After `Workspace not trusted`: run the account's command in the folder to accept trust, then retry |
| `Esc` | Cancel (asks first if the task is not empty) |

## Quick reply window

Decision: [0040](../0040-quick-reply.md).

| Key | Action |
|---|---|
| `Enter`, `Ctrl+J` | New line |
| `Ctrl+P` | Insert a [prompt template](sessions.md#prompt-templates) at the cursor |
| `/` (at the start of the text) | [Slash-command](sessions.md#slash-commands) menu, as in the dialog |
| `Ctrl+S` | Send |
| `Ctrl+T` | After `Workspace not trusted`: run the owner's command in the folder to accept trust, then send once more |
| `Esc` | Cancel (asks first if text was typed) |

## Welcome window

The window of a first start and of `tower setup`. Decision: [0048](../0048-first-run-welcome.md).

| Key | Action |
|---|---|
| `Enter` | Next step; on the last one, save and go on to tower |
| `Backspace`, `Shift+Tab` | Previous step |
| `↑` / `↓` | Move over the step's rows |
| `←` / `→` | Change the value (style, theme, card size, an option) |
| `Space` | Tick a folder, switch an option |
| `Esc` | First start: skip the rest and save the current choices. `tower setup`: cancel, writing nothing |
| `Ctrl+C` | Quit, writing nothing |

In the field for another folder: typing goes into the field, `Enter` adds the folder, `Tab` completes
its name, `Ctrl+U` or `Esc` clears it, and `↑` goes back to the list. With nothing typed `Enter`,
`Backspace` and `Esc` do what they do on the rest of the step.

## Settings window

Decision: [0024](../0024-settings-window.md).

| Key | Action |
|---|---|
| `↑` / `↓`, `k` / `j` | Move over the settings and accounts |
| `Tab` / `Shift+Tab` | Next / previous section |
| `Enter` | Edit: pick list, toggle, inline input, or the folder list |
| `←` / `→` | Previous / next value of a choice (style, theme and colour depth preview at once) |
| `Del` | Back to the built-in default; saving removes the key from the file |
| `Ctrl+S` | Save; the window stays open and says what was written |
| `e` | Open `config.toml` in `$VISUAL` / `$EDITOR` (asks first about unsaved changes) |
| `Esc` | Close; with unsaved changes it asks: `y` save and close, `n` discard, `Esc` keep editing |
| `a`, `Enter`, `d`; `Tab` | Folder list: add, edit, remove; `Tab` completes a folder name |

## Keyboard layouts and confirmations

Command keys also work with a Russian or Ukrainian (ЙЦУКЕН) layout active: `т` acts as
`n`, `ф` as `a`, and so on; text fields keep Cyrillic as typed ([0011](../0011-keybindings.md)).

Confirmations take `y`; `n`, `Esc` and `Enter` cancel. The hand-off confirmation names
both accounts and their groups and says that the full conversation and project context
continue under the target account and its terms. Tower never binds `←` in the session
list (only inside the focused logs panel, where it means "back") and treats
`Ctrl+Z` as a no-op, because inside an attached session those are Claude Code's keys
(`←` on an empty prompt opens agent view, `Ctrl+Z` returns to tower).
