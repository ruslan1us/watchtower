# 0045. Workspace style: Claude's terminal in a pane (first step)

- Status: Accepted · 2026-10-30 · the first step, for the owner to try on real terminals · the Home panel, clicks on group headers, the wheel on the list and cards added after the owner's trial, 2026-10-31 · the whole selected card highlighted, `[ui] card_gap`, and `o`, start-and-attach and the rename in the pane, after the second trial, 2026-10-31 · the shell in the pane, the usage block and the scroll hints, 2026-10-31
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0007](0007-visual-only-attention.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0024](0024-settings-window.md), [0028](0028-scrolling-the-logs-panel.md), [0037](0037-git-summary-and-shell.md), [reference](reference/claude-code-cli.md#claude-attach-in-a-pseudo-terminal)
- Overturns, for this style only: the rejected split view of [0005](0005-attach-and-detach.md), the "no PTY handling" of [0001](0001-thin-wrapper-over-claude-cli.md) and the non-goal of [0014](0014-v1-scope.md), "No mouse" of [0028](0028-scrolling-the-logs-panel.md), the key rules of [0011](0011-keybindings.md) and the two styles of [0020](0020-visual-styles.md)

## Context

Attaching hands the whole terminal to one session: the list is gone until `Ctrl+Z`
([0005](0005-attach-and-detach.md)). The owner wants to look through sessions and type
into one with the list still on screen, and decided on 2026-10-30 to try it, as an opt-in
style. `claude attach <id>` runs correctly in a pseudo-terminal, and `vt100`, already a
dependency, rendered its real stream identically to `alacritty_terminal`
([reference](reference/claude-code-cli.md#claude-attach-in-a-pseudo-terminal)).

This step is meant to be tried on Warp, Windows Terminal and over SSH. Clicks on buttons, a
zoom key and lifecycle polish come later, each with its own decision. *Added 2026-10-31,
after the owner's trial on WSL:* the owner did not want the orchestrator to open at every
start, so nothing attaches until a row is chosen (**Home**), and the planned cards replaced
the list's rows (**Cards**). What the trial did not question stands.

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
the pane keeps its size. *Amended 2026-10-31:* the bar is no panel any more: its top row
has `◆ tower`, the grouping and the count, with a `▌` while the list has the keys, and each
session is a card (below). The columns above belong to the first step. Below 100 × 24 tower draws the dashboard (which draws minimal
below its own size), says so once, closes the client and turns the capture off; it comes
back when the terminal grows. If a client cannot start, tower draws the dashboard until it
restarts.

**Home** (*added 2026-10-31*). At the start the pane shows a Home panel and no row is
selected, so tower attaches nothing and starts nothing until the user chooses a row. It has
the tower's name and version; the counts per group in the state colours (`2 need you · 1 ready
· 3 working`); the sessions that need attention by name, state and account, from the same
listing and rule as `●` ([0007](0007-visual-only-attention.md)), hidden rows too; each
account's plan usage while `[usage] show` is on ([0043](0043-plan-usage-display.md)), since this
style has no header; and a short key list. A long attention list ends in `… N more` so the
rest still fits. Home only reads what tower already has: no model request, no attach, no
`git status`.

- The first move of the selection chooses: an arrow, `j` / `k`, a page key or a notch of
  the wheel lands on the first row, not on a header, and it attaches after the usual rest.
  `Home` / `g` and `End` / `G` go where they say, and a click selects what is under it. Row
  keys (`Enter`, `Tab`, `s`, `d`, `a`, `e`, `t`, `m`, `!`) have no row and do nothing;
  `Enter` says to choose a session first. `n`, `o`, `R`, `c`, `/`, `p`, `i`, `r`, `?` and `q`
  work as in the list.
- `h`, or a click on the bar's top row (`◆ tower`), goes back to Home. **Going Home closes
  nothing:** a client stays attached, hidden, and the 10-minute release applies to it as to any
  client without the keys, so there is one rule, and coming back shows the terminal at once. The
  next move returns to the row chosen before, not the first; another row replaces the client
  as usual. A press on the Home panel reaches nothing, so a hidden client never gets the keys.
- Whenever the style comes on (the start, or a switch to it in Settings) it starts on Home,
  whatever the selection was in another style. The dashboard fallback has a normal selection:
  a row chosen there is attached when the terminal grows, and if none was, Home shows.

**Cards** (*added 2026-10-31*). The list's groups, order, folding, filter and keys are the
same; a session is a card of `[ui] card_size` (also in Settings; default `medium`).

```text
large (4 rows)            medium (3 rows)           small (2 rows)
╭─ ● ▪ name ──── +2 ─╮    ╭─ ● ▪ name ──── +2 ─╮    ▌ ● ▪ name       +2 8m
│ state           8m │    │ state       …/last │    ▌ state  …/last   main
│ ~/workspace/folder │    ╰─ main ─────── 8m ─╯
╰─ main ─────────────╯
```

- The outline has the state's theme colour; `small` has a coloured `▌` edge instead. The
  selected card's outline is heavy (`┏━┓`; in `small` the edge is `█`, or `▶` where the selection
  is reverse video, since a full block would vanish there).
- *Amended 2026-10-31, the owner's second trial:* the whole selected card sits on the selection,
  every row across its whole width and in all sizes, whether the list or the terminal has the keys
  (`▌`, the `LIST` / `TERMINAL` chip and the pane's frame say that). The theme's selection
  background is painted, or reverse video where the theme or `NO_COLOR` selects that way. An account chip keeps its own background, and the outline its state colour. Text on the
  card takes the selection's text colour, the state's and the folder's too, because a state or
  dim colour may not read on the bar; the outline still says the state. **This is an exception to
  [0021](0021-dashboard-themes.md)'s rule that only the selected row is painted**, for this style's
  selected card only.
- *Amended 2026-10-31:* contrast inside a card: the name is bold in the normal text colour, the
  state in its state colour, the folder dim. The `+N` badge and the outlined cards' `▪` no longer
  repeat the outline's colour.
- Interactive rows are dim with a dashed outline (`┆` edge in `small`). Attention stays a red
  `●` and a bold name ([0007](0007-visual-only-attention.md)).
- Cards show the account, not git; git stays in the pane's title. Subagents are the `+N` badge
  on the title: the child lines of [0029](0029-tracking-subagents.md) are not drawn on cards.
- The selected card does not grow, so the list does not shift under the mouse. The viewport
  moves by whole entries and keeps the selected card whole; a card that does not fit at the
  bottom is not drawn. `PgUp` / `PgDn` move by the entries that show.
- *Added 2026-10-31:* `[ui] card_gap` (0 or 1, default 1, also in Settings) is the blank rows
  between cards and before each group header; a header stays with its first card, and the top of
  the list has none. A click on a gap selects nothing. The viewport and the page keys count it.
  Cards that fit with 6 group headers, when the list scrolls (the footer, the bar's title row and
  the two hint rows are taken; the usage block takes its rows too):

  | Terminal | `large` | `medium` | `small` |
  |---|---|---|---|
  | 200 × 50, `card_gap = 1` | 8 | 10 | 13 |
  | 120 × 36, `card_gap = 1` | 5 | 6 | 9 |
  | 200 × 50, `card_gap = 0` | 10 | 13 | 20 |
  | 120 × 36, `card_gap = 0` | 6 | 8 | 13 |

- *Added 2026-10-31, the third request:* a list that does not fit keeps one row at each end, the
  same rows wherever the selection goes, for a dim `▲ 3 more` and `▼ 5 more`, counting the hidden
  sessions (`▼ 2 more groups` when only folded groups are hidden). A click on one moves a page; a
  list that fits has neither hint nor reserved rows. A burst of the wheel is absorbed by the 150 ms
  of rest, so one client attaches, for the row it stopped on. A frame's work over the list is
  linear in its entries, and each card's texts are built only for the cards that show.
- *Added 2026-10-31:* while `[usage] show` is on, a block fixed at the bar's bottom, a rule and one
  row per account (`main  5h 24% · 7d 12%`), with Home's colours, stale dimming and `…` / `—`. Cards
  scroll above it and never overlap it, and the viewport, the clicks and the page keys use what
  is left. The week goes first, then a stale value's age, and only then is the name cut. With fewer
  than 12 rows left for the list the block is hidden. Reset times are on Home only.
- Fitting on one row: a folder first becomes `…/last`, then the state its short form
  (`permission`, `reply`), then the folder and then the state are cut with `…`. A name is cut
  with `…`. On a row too narrow for both the folder goes.

**The shell in the pane** (*added 2026-10-31*, amending [0037](0037-git-summary-and-shell.md)
for this style). `!` on a row runs `$SHELL` (`/bin/sh` when unset) in a PTY of its own in the row's
folder, with the client's clean environment, and shows it in the pane with the keys, titled
`shell  ~/workspace/api`. It wakes nothing, so stopped rows work too. The worker looks at the folder: when
it is gone nothing starts and the status line says so, as in 0037. Below 100 × 24 `!` stays
the foreground hand-over.

- One shell at a time, separate from the Claude client: both can exist, and each keeps running
  unseen. The shell stays alive while the selection browses other rows, whose Claude the pane shows
  as usual; back on its row it shows again, with the keys left on the list.
- `!` on its row toggles the pane between the shell and what the row normally shows; going to the
  shell gives it the keys. While the shell shows, `Enter` gives it the keys, and the row needs no
  client, so none is started. `!` on another row closes the old shell with a status note and
  opens a new one there.
- `exit`, or the shell ending any way, returns the pane to what the row normally shows, with
  the list focused, and polls. Tower's quit ends it like a client: `SIGHUP` to its group, closing the
  PTY, `SIGKILL` after 2 s, and reaping. So does the row leaving the listing, or leaving the style. A
  terminal too small for the workspace only hides it.
- `Ctrl+Z` and `F12` return to the list, as in Claude's terminal. **So `Ctrl+Z` never reaches the
  shell's job control in this style**; for a job to stop, use the foreground `!` of a smaller window
  or another style. Everything else goes to the shell, `Ctrl+C` included. The shell answers nothing
  a row asks, so `●` stays, and its keys do not count as the Claude client's.
- **A limit, recorded rather than built:** the pane has no scrollback and tower no text selection of
  its own, so a plain drag does nothing unless the program in the shell asks for the mouse.
  `Shift`+drag is the real terminal's own selection. Programs with their own scrolling work.

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

- Selecting a row with a live process (*amended 2026-10-31:* once a row is chosen, see Home)
  attaches after 150 ms of rest, replacing the previous
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
  client first and then works as today.
- *Amended 2026-10-31:* every attach opens in the pane while the workspace is drawn: `o`,
  start-and-attach in the dialog and the rename of a running session, as `Enter` on a stopped row
  already did. Each selects its row (opening a folded group, clearing a filter that hides it),
  attaches after the same checks with the owner's command, and gives the terminal the keys;
  `Ctrl+Z` and `F12` return as usual, and a client already there is kept. A session tower has just
  started opens when the listing shows it, unless the user has moved the selection meanwhile, or
  after 30 s; a window or the filter line holds it until it closes. Failures and the trust windows work as
  before, and below the size limit the foreground attach of [0005](0005-attach-and-detach.md) stays.
  For a rename ([0027](0027-rename-sessions.md)) the status line says what to type, and the name
  is checked when the keys return to the list or the client ends cleanly; a failed client drops it.
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
*Amended 2026-10-31:* a press on any row of a card selects it; a press on a group header
selects it and folds or unfolds it, as `Tab` does there; the wheel over the list moves the
selection like the arrows, and gives the list the keys.
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
- The plan usage display of [0043](0043-plan-usage-display.md) shows in the pane's details and,
  *since 2026-10-31,* per account on Home, with reset times, and in the bar's block (no header in
  this style).
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
- Done 2026-10-31: cards, the wheel on the list, clicks on group headers, Home.
- Done in the second trial, 2026-10-31: `o`, start-and-attach and the rename opening in the pane,
  the whole selected card highlighted, `[ui] card_gap`.
- Done in the third request, 2026-10-31: the shell in the pane, the usage block and the scroll hints.
- Still to come: clicks on the stopped panel, a zoom key, lifecycle polish, and scrollback and a
  selection of its own in the pane if the limit above proves too tight. The mockup's `a → alt` hand-off hint on the selected card is not
  built.
