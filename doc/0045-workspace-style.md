# 0045. Workspace style: Claude's terminal in a pane (first step)

- Status: Accepted · 2026-10-30 · the first step, for the owner to try on real terminals
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0007](0007-visual-only-attention.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0024](0024-settings-window.md), [0028](0028-scrolling-the-logs-panel.md), [0037](0037-git-summary-and-shell.md), [reference](reference/claude-code-cli.md#claude-attach-in-a-pseudo-terminal)
- Overturns, for this style only: the rejected split view of [0005](0005-attach-and-detach.md), the "no PTY handling" of [0001](0001-thin-wrapper-over-claude-cli.md) and the non-goal of [0014](0014-v1-scope.md), "No mouse" of [0028](0028-scrolling-the-logs-panel.md), the key rules of [0011](0011-keybindings.md) and the two styles of [0020](0020-visual-styles.md)

## Context

Attaching hands the whole terminal to one session: the list is gone until `Ctrl+Z`
([0005](0005-attach-and-detach.md)). The owner wants to look through sessions and type
into one with the list still on screen, and decided on 2026-10-30 to try it, as an opt-in
style. `claude attach <id>` runs correctly in a pseudo-terminal, and `vt100`, already a
dependency, rendered its real stream identically to `alacritty_terminal`
([reference](reference/claude-code-cli.md#claude-attach-in-a-pseudo-terminal)).

This step is meant to be tried on Warp, Windows Terminal and over SSH. Cards in the list,
clicks on buttons, a zoom key and lifecycle polish come later, each with its own decision.

## Decision

**The style.** `[ui] style = "workspace"`, also in the Settings window. It is opt-in and
experimental; the default stays `dashboard`, and no other style changes in any way: no
second descriptor in the event loop's wait, no mouse capture, no bracketed paste, no
draw limit.

**Layout.** Today's list in a bar at the left, 18 % of the width kept between 28 and 40
columns, or `[ui] list_width` (20 to 60; 0 is automatic), with the columns a bar has room
for: name with its subagent badge, age, the account when there are several and the name
keeps 14 columns, and the state when grouped by project. The pane takes the rest, in place
of the details panel. Its frame's title has the session's name, state and account and
says who has the keys, with the `git` summary ([0037](0037-git-summary-and-shell.md)) on
the right. One footer row has a `LIST` or `TERMINAL` chip, today's status line and the key
hints that still fit. The filter's input line takes the footer's row while it is edited, so
the pane keeps its size. Below 100 × 24 tower draws the dashboard (which draws minimal
below its own size), says so once, closes the client and turns the capture off; it comes
back when the terminal grows. If a client cannot start, tower draws the dashboard until it
restarts.

**The client ([`embed`](../src/embed.rs)).** One `<owner's command> attach <id>` at a time
([0003](0003-accounts-and-session-ownership.md)), in the session's folder (without one
that is gone), in a new session with the PTY as its controlling terminal, so nothing typed
there can signal tower. The environment has `TERM=xterm-256color` and
`COLORTERM=truecolor` and none of the variables that name the real terminal
(`TERM_PROGRAM`, `TMUX`, `WT_`, `KITTY_`, `WARP_` and similar, `COLUMNS`, `LINES`): with
them Claude Code speaks protocols the emulator does not. A worker thread owns the child:
it reads the PTY into the emulator, answers the queries (DA1, XTVERSION, DSR, `?2026$p`),
writes what is queued without blocking, and reaps. The UI thread therefore never blocks on
the child, a full PTY or a hung folder ([0001](0001-thin-wrapper-over-claude-cli.md)). It
wakes the event loop through a self-pipe in the existing `poll(2)`, so idle costs the same
as before. The workspace draws at most about 30 times a second and holds a draw for up to
200 ms while a `?2026` update is being written. Closing is `SIGHUP` to the client's process
group and closing the PTY, `SIGKILL` after 2 s, then reaping, on the worker; on exit tower
waits up to 0.5 s. The session keeps running.

**Nothing raw reaches the terminal.** Only ratatui writes to the real terminal, so BEL, OSC
notifications and title changes end in the emulator
([0007](0007-visual-only-attention.md)). The one exception: a clipboard write
(`OSC 52 ; <selector> ; <base64>`, at most 4 MiB) is written again by tower, so copying
works over SSH. A clipboard read is never passed on or answered.

**Selection and focus.**

- Selecting a row with a live process attaches after 150 ms of rest, replacing the previous
  client. The arrows can run over rows, and nothing starts or wakes. The pane keeps showing
  the previous client until then.
- A stopped row shows a panel with its details and `Enter wakes it here`. Only `Enter`
  wakes it, through the checked attach of [0005](0005-attach-and-detach.md) (refresh,
  owner, the account picker when unknown), opened in the pane. A row with an unknown owner
  is treated the same way. Interactive rows and group headers show a note or a summary, and
  close the client.
- `Enter` on a live row gives its terminal the keys, attaching at once if it has not yet.
  `●` clears there, not on selection, and what the row asks while its terminal has the
  keys counts as seen ([0007](0007-visual-only-attention.md) amended for this style).
- In the terminal every key and paste goes to the child, `Ctrl+C` included, except `Ctrl+Z`
  and `F12`, which give the list the keys. The client stays. A real `claude attach` would
  exit on `Ctrl+Z`, so here it never gets that key.
- A client without the keys for 10 minutes is released, since an attached session is kept
  from Claude Code's idle stop and tower must not do that.
- A client that exits returns the keys to the list and polls at once. A non-zero exit keeps
  its last screen until the selection moves, so its error can be read.
- Any tower action that stops, wakes or removes the row (`s`, `d`, `a`, `e`, `t`) closes its
  client first and then works as today. So do the foreground attaches, which are unchanged:
  `o`, start-and-attach in the dialog and the rename of a running session.
- After an action, a client's exit or a release, the row is **held**: the listing may still
  show it live, and attaching again could wake what was just stopped. It attaches when
  `Enter` asks, or when the selection comes back to it. The panel says why.

**Keys and windows.** All windows (new session, help, confirmations, Settings) draw over
the pane. While one is open it alone gets the keys and paste, Claude's cursor is hidden, and
Claude keeps painting underneath. `Space` and `→` only say that the pane takes the logs
panel's place. The help lists the pane's keys in place of the dialog's.

**Mouse, minimal, this style only.** Capture is on, with `[ui] mouse = false` to disable
it, and off whenever the workspace is not drawn or the terminal is handed over. While the
terminal has the keys and the emulator's mouse mode is on, events go on in SGR form with the
pane's coordinates, clamped to it, and only those the mode asks for. A gesture that starts
inside goes on outside; a press outside starts nothing there. A gesture ends when the keys
or the selection move, without a synthetic release, which could make a copy-on-release
child write the clipboard. A press on the pane gives it the keys and goes on to the child;
a press on the list gives them back and selects the row under it. Motion is coalesced.
Bracketed paste is on in this style; windows take a paste as typed, as without it, and the
list ignores it, so no pasted letter runs a key.

**Tower never parses the pane's screen for status.** It only copies the cells. State,
attention and everything else still come from the CLI's JSON
([0001](0001-thin-wrapper-over-claude-cli.md)).

## Consequences

- The hard rules of [0001](0001-thin-wrapper-over-claude-cli.md) stand: the client is the
  unmodified `claude attach`, with its own credentials, on a terminal tower provides.
  Tower still supervises no session; a client's death costs nothing.
- Selecting a row resizes the session's PTY, which takes its last client's size, and makes
  Claude Code repaint. This is what `claude attach` does; the logs of
  [0025](0025-session-terminal-size-for-logs.md) follow it.
- One more thread and a small emulator (32 bytes a cell) per client, and only one client.
- The terminal's own text selection usually needs `Shift` while the capture is on.
- The plan usage display of [0043](0043-plan-usage-display.md) shows only in the pane's
  details (no header in this style) in this step.
- Tests use a fake `claude`, the real binary in a PTY; the real client is checked by hand
  with the keyboard only ([0015](0015-testing-strategy.md)). Never send mouse events into a
  real `claude attach` from a test: its copy-on-release runs PowerShell `Set-Clipboard` on
  WSL and overwrites the Windows clipboard.

## Known risks

- Claude Code's fullscreen rendering is a research preview, and `attach` always uses it.
  It may change with any release.
- `←` on an empty prompt opens the built-in agent view inside the pane, where the user can
  open another session: the pane then shows a session other than the selected row. Tower
  does not parse the screen, so it cannot tell. `Esc` there ends the client.
- A row listed as live may have just stopped: the listing is up to one poll old, so an
  automatic attach could wake it in that moment. The hold above covers what tower itself
  stopped.
- Windows Terminal, Warp and SSH are untested. Warp's OSC 52 setting may need enabling for
  clipboard writes. `Shift+Enter` needs a terminal that reports it (`Alt+Enter` otherwise).
- Occasional stale cells come from Claude Code's stream; `Ctrl+L` clears them.
- A character the emulator and ratatui measure differently could shift a row until the next
  paint.

## Alternatives considered

- **A `claude logs` replay as the view, attach only on `Enter`.** No extra client, but
  output is up to a poll old and the switch to typing repaints the pane. The owner chose the
  live client.
- **Passing the child's bytes through to the real terminal in a scroll region.** It would
  carry BEL and OSC past tower, and windows could not draw over it.
- **`alacritty_terminal`.** It rendered the same stream identically, at the cost of a new,
  larger dependency.
- **`openpty`.** It opens both ends without close-on-exec, so a process another thread forks
  at that moment could keep them, a supervisor for good, hiding the client's end. Tower uses
  `posix_openpt` with `O_CLOEXEC` and opens the slave the same way.

## Open items

- The owner's results on Warp, Windows Terminal and over SSH.
- Cards, clicks on the stopped panel, mouse wheel on the list, a zoom key, `o` and
  start-and-attach opening in the pane, plan usage in the bar.
