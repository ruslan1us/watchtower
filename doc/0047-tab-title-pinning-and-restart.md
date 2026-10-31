# 0047. Attention in the tab title, pinned sessions, restart after an update

- Status: Accepted · 2026-10-31
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0009](0009-stop-remove-and-logs.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [0022](0022-tower-writes-its-own-config.md), [reference](reference/claude-code-cli.md#restarting-a-session-claude-respawn)
- Amends: the title rule of [0007](0007-visual-only-attention.md) and [0020](0020-visual-styles.md); the groups of [0006](0006-session-states-and-grouping.md); the state file of [0002](0002-tech-stack-and-files.md); the keys of [0011](0011-keybindings.md); the rows of [0024](0024-settings-window.md); what passes to the terminal in [0045](0045-workspace-style.md); the pinning non-goal of [0014](0014-v1-scope.md), for tower's own list only

## Context

The owner asked for three things on 2026-10-31.

1. Attention was only visible on tower's own screen, and tower often sits in another tab. It
   should show from there, without sound.
2. Some sessions matter more than their state says, and should stay at the top of the list.
3. After a Claude Code update, running sessions stay on the old binary until they restart.
   Claude Code has `claude respawn <id>|--all`, which tower did not offer.

[0007](0007-visual-only-attention.md) and [0020](0020-visual-styles.md) ruled out a title with
a count: "signals outside the tower screen are out of scope". The owner's requirement behind
0007 was no bell, no sound and no desktop notification. A title is none of those: it makes no
sound, opens nothing, takes no focus, and the user sees it only by looking at it. So the owner's
request fits as visual, and it is the scope sentence that changes, here.

`respawn` was verified first, with Claude Code 2.1.316, on a throwaway session; the facts are in
the [reference](reference/claude-code-cli.md#restarting-a-session-claude-respawn).

## Decision

### The tab title

Tower sets the terminal's window or tab title: `tower`, or `tower · 2 need you` while sessions
need attention (`tower · 1 needs you`), from the same counter as the status line
([0007](0007-visual-only-attention.md)), hidden rows included, without its `(filtered)`. All
styles, and on by default: `[ui] tab_title = true`, with a row in Settings (`terminal tab title`,
at once). `false` means tower never writes a title byte.

- **Only tower writes it, and only tower's text.** It is built from a number, so no session name
  or other data reaches it. The pane's programs never reach the title: the rule of
  [0045](0045-workspace-style.md) stands, and their `OSC 0`, `1` and `2` and XTWINOPS end in the
  emulator. In a foreground hand-over ([0005](0005-attach-and-detach.md)) the terminal is the
  child's, as ever, and the child sets what it likes.
- **Only when the text changes.** Polls that find the same count write nothing.
- **`OSC 2` ended by `ST`**, never by BEL, which tower never writes
  ([0007](0007-visual-only-attention.md)). crossterm's `SetTitle` ends in BEL, so tower does not
  use it.
- **The previous title is kept on the terminal's own stack**, with XTWINOPS: `CSI 22;2t` before
  the first title, `CSI 23;2t` to bring it back. Tower never reads a title: the `CSI 21t` report
  is off in most terminals, and its answer is untrusted input.
- **Terminals that ignore XTWINOPS.** `CSI 22` and `23` are safe to send: only `Ps` from 24 up
  means a resize (xterm's control sequences), and a terminal that does not know them drops them.
  Such a terminal has no previous title to bring back, so tower sets the neutral `tower` right
  before every `CSI 23;2t`: an old attention count is never left behind. It then shows `tower`
  until the shell's prompt sets a title, as many do.
- **Exit, SIGTERM and a panic** restore it. **A hand-over** shows the previous title while the
  child has the terminal and keeps it saved (`tower`, `CSI 23;2t`, `CSI 22;2t`), so a title the
  child leaves behind is not taken for it; tower's own comes back on return. **Turning the
  setting off** restores it at once.
- **A hang-up writes nothing.** After SIGHUP or EOF on the input the terminal may be gone, and
  [0005](0005-attach-and-detach.md) forbids writing to it. So this one way out cannot restore: a
  dropped SSH connection can leave the local tab showing `tower · 2 need you`, until something
  there sets a title. Nothing tower can do reaches that terminal.

This is still [0007](0007-visual-only-attention.md): no BEL, no sound, no `OSC 9`, `99` or `777`,
no focus. `OSC 2` and those two XTWINOPS are all it adds.

### Pinned sessions

`P` on a row pins or unpins it (free in [0011](0011-keybindings.md), and `p` next to it is a
grouping toggle, so a slip costs nothing). `З` maps to it, as every shifted letter does.

- **By state:** pinned rows have their own group, `Pinned`, right after Orchestrator, whatever
  their state, stopped rows too, so a pinned row is never folded away with Stopped & finished.
  It is expanded by default, folds with `Tab` like any group, is sorted like any group, and has
  its row and highlight counts. The row keeps its state: its detail, colour, spinner, `●` and the
  header pill of its state ([0020](0020-visual-styles.md)) are what they were, and its attention
  still counts. The details panel says `Working · pinned`.
- **By project:** no Pinned group; pinned rows sort first within their project.
- **In every style,** the workspace included, where it is an ordinary group of cards.
- **What cannot be pinned:** the orchestrator, which has its own group at the top, and
  interactive rows, which have no short id. The status line says so.
- **Filter:** a filter reaches into the group, and an empty result hides it, like any group.

The pin is stored in the sidecar, in a top-level `pins` object keyed by the short `id`
([0002](0002-tech-stack-and-files.md)), not with the session's record: the `sessionId` changes
while a session is live after `/clear` and shows the original again when stopped
([reference](reference/claude-code-cli.md#clear-in-a-background-session)), while the short id
stays. A record holds `pinned_ms` and `last_seen_ms`, and unknown fields are kept. A pin no
successful listing showed for 14 days is dropped, under the same rules as session records:
nothing while any account's poll failed or was partial ([0017](0017-ownership-evidence-rules.md)
section 9). Two towers merge by `pinned_ms`: what one pinned or unpinned wins over one that only
saw the row, and an unpin does not remove a newer pin.

**This is tower's list only.** It is not Claude Code's own pin (`Ctrl+T` in agent view), which
has no CLI ([reference](reference/claude-code-cli.md#supervisor-lifecycle)) and keeps a session
from its idle stop. A session pinned in tower still stops after about an hour. So
[0016](0016-do-not-change-claude-code-defaults.md) is untouched, and the non-goal of
[0014](0014-v1-scope.md) is lifted for tower's list and not for that.

### Restart after an update

`U` on a row asks, naming the session and its account, then runs
`<owner's command> respawn <id>` on a worker thread, like `s`
([0003](0003-accounts-and-session-ownership.md), [0009](0009-stop-remove-and-logs.md)). Success
means exit 0 and `respawned <id>`. The window says what it costs, from the row, in what the
reference records: a running turn is interrupted and does not continue, and the session then
waits for you; a pending prompt is dropped and what it asked for does not run; a background task
is named as not verified. It always says that the conversation, id, name, account and saved
options stay, and that a terminal attached elsewhere is disconnected. An unknown owner opens the
account picker first, as for `s`.

- **A stopped row** says there is nothing to restart. `respawn` would wake it, and a wake starts
  the installed binary anyway ([0005](0005-attach-and-detach.md)). An interactive row gets the
  usual hint.
- **Several sessions:** `U` on a group header takes that group's running rows, those the filter
  lets through; `Ctrl+U` takes every running session, whatever the view hides. `г` with `Ctrl`
  maps to it. The stronger window says how many on each account (`main: 3 sessions`), how many
  are working, waiting on a prompt or running a background task, and what it leaves out: stopped,
  of an unknown account, or already being worked on by tower. **Only a capital `Y` confirms**
  (`Н` on a RU/UA layout). `y` says so and keeps the window; `n`, `Esc` and `Enter` cancel, as
  everywhere.
- **Always by id, never `--all`.** Each session runs with its own account's command, one after
  the other, and a failure does not stop the rest. `--all` was not run on the real CLI, since it
  would restart the owner's working sessions, so what it does is not known: whether it wakes
  stopped sessions as `respawn <id>` does, and what it reaches when accounts share their lists.
  Tower's id check refuses anything starting with `-`, so it cannot be passed by accident.
- **While it runs,** the status line says `restarting…`, with how many are done, and the row
  refuses other actions as during a hand-off. A restart is not activity: the row lists as stopped
  for a moment, its tuple is not recorded meanwhile, and the selection comes back to it. A row
  back in the same tuple keeps its age and acknowledgement. One whose turn was cut is `blocked`,
  `idle` by then, a change like any other, and gets its `●` ([0007](0007-visual-only-attention.md)).
- **What it reports:** `restarted <name> (<id>)`, or `restarted 5 sessions (main 3, alt 2)`. One
  failure is on the status line, with the CLI's words. Several sessions' failures also open a
  window naming each session, account and answer, which waits behind an open window
  ([0011](0011-keybindings.md)). Then it polls.
- **In the workspace** the pane's client on one of the sessions closes first, since `respawn`
  ends every attached client, and the row is held. Once the restart succeeded and the listing
  shows it live, the pane attaches again after the usual rest, if the selection is still there.
  A failed restart keeps the hold, and `Enter` asks.
- **Versions are not shown.** The listing has no version field, `claude daemon status` gives only
  a count per account in text meant for people ([reference](reference/claude-code-cli.md#restarting-a-session-claude-respawn)),
  and [0003](0003-accounts-and-session-ownership.md) rejected parsing it. `/proc/<pid>/exe` shows a
  versioned path, but that is no CLI output and [0001](0001-thin-wrapper-over-claude-cli.md) does
  not allow it for this. So tower says nothing about which sessions are old, and restarts what it
  is asked to.

## Consequences

- From another tab the owner sees that something waits, without a sound.
- A dropped connection can leave a title behind, and so does a terminal without a title stack
  (`tower`, until a prompt sets its own). Windows Terminal and Warp could not be tested here:
  checked only in tmux 3.2a, which honours all of it. Over SSH to a tmux, `set-titles` decides
  whether the outer terminal sees it.
- One key more in the help, which still fits 24 rows by merging the page keys into the first row.
- A restart costs a running turn when the user confirms it; Claude Code's own upgrade, which its
  `daemon status` says happens once a session is idle, is not touched and not relied on.
- Pinning adds a small map to the sidecar; an older tower keeps it as an unknown field.

## Alternatives considered

- **`OSC 0`.** It also sets the icon name, which tower has no reason to touch. `OSC 2` is the
  window title alone.
- **Saving and writing back the title by hand.** It would need `CSI 21t`, usually disabled and an
  answer to read and trust.
- **Pinning by `sessionId`, like every other record.** A `/clear` would lose the pin. Rejected on
  the reference's evidence.
- **A pin marker on the row.** Not asked for, and the group says it in `state` grouping. By
  project the details panel says it.
- **`claude respawn --all`.** See above. Looping by id has only the documented behaviour of one
  id to rely on, is per account by construction, and leaves stopped rows asleep.
- **Typing a word to restart all sessions.** A capital `Y` is enough next to a window that counts
  them, and a slip of `y` runs nothing.
- **Restarting without asking when a session is idle.** The owner asked for a confirmation, and an
  attached client elsewhere still drops.

## Open items

- The owner's results on Warp, Windows Terminal and over SSH: whether XTWINOPS is honoured or
  ignored there. If a terminal does something else with `CSI 22;2t`, `[ui] tab_title = false` is
  the way out.
- What `respawn` does to a running background task, to a session on another account's supervisor,
  and when the binary really changed: only the same version was available here
  ([reference](reference/claude-code-cli.md#restarting-a-session-claude-respawn)).
