# 0030. Following the logs stream across fetches

- Status: Accepted · 2026-10-19
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0016](0016-do-not-change-claude-code-defaults.md), [0025](0025-session-terminal-size-for-logs.md), [reference](reference/claude-code-cli.md#claude-logs-output)
- Amends: the logs panel of [0009](0009-stop-remove-and-logs.md) (which rows are fetched, how often, and how the stream is replayed) and the paused panel of [0028](0028-scrolling-the-logs-panel.md)

## Context

The logs panel replayed each `claude logs` capture on its own
([0009](0009-stop-remove-and-logs.md)). For a session that had been working
for more than about 40 seconds it showed only fragments: the spinner line, a
few footer cells and the newest message lines, without the header, the
prompt box or the static footer. Attaching once fixed it for a while.
Verified with Claude Code 2.1.303
([reference](reference/claude-code-cli.md#claude-logs-output)):

- `claude logs` returns the session's last ~450 writes, dropped whole:
  32–38 KB, about 40 seconds of a working session (~11 frames a second,
  ~780 bytes a second). A capture always starts at a write boundary.
- Claude Code repaints the whole screen (`ESC[2J ESC[H` and every row) only
  when the session starts and when a terminal attaches, even at the same
  size. Everything else (turn end, new messages, tool start and end, timers)
  is a differential frame: `ESC[H\r`, relative moves to the changed cells,
  and a park sequence (`ESC[<rows>;1H ESC[<rows-2>;3H`).
- So once the last repaint has left the window, no capture holds the parts
  of the screen that were drawn only then.
- Captures nine seconds apart overlap by ~25 KB; captures of an idle session
  are identical. Stitching them and feeding only the new bytes into one
  emulator gave the right screen across a five-minute turn and an attach.
  The overlap is lost when more than ~450 writes pass between two fetches.

Forcing a repaint (a hidden `claude attach`, a signal, a PTY resize) would
change the session ([0016](0016-do-not-change-claude-code-defaults.md)) and
go around the CLI ([0001](0001-thin-wrapper-over-claude-cli.md)).

## Decision

**One stream per session.** Tower keeps an emulator per followed session,
at the session terminal's real size ([0025](0025-session-terminal-size-for-logs.md);
the inference of [0009](0009-stop-remove-and-logs.md) otherwise), and feeds
it every capture's new bytes:

- The anchor is the last 256 bytes of the previous capture (tower keeps up
  to 256 KiB of it). An occurrence of the anchor in the new capture counts
  only when the bytes before it equal the previous capture's back to the
  start of either capture; the latest that counts wins, at most 64 are
  checked. The bytes after it are fed; an identical capture feeds nothing.
- No overlap, a known real size other than the emulator's, or a known pid
  other than the stream's starts the stream over: the new capture is
  replayed into a fresh emulator as before.
- The stream has a **base** once a full clear (`ESC[2J` or `ESC c`) was fed
  since it started over. Each feed has the work bounds and the scrollback of
  [0009](0009-stop-remove-and-logs.md) and [0028](0028-scrolling-the-logs-panel.md);
  the scrollback now gathers across feeds.

**Following.** A follower thread owns the streams and runs every fetch
with the owning account's command ([0003](0003-accounts-and-session-ownership.md))
on a short-lived worker thread:

- After every poll tower hands it the live background rows with a short id
  whose owning account is known and configured: the panel's row first, then
  the working rows (`state: working` or `status: busy`,
  [0006](0006-session-states-and-grouping.md)), then the rest; at most 16.
  Working rows are fetched every **10 seconds**, whatever the poll interval;
  the others keep their stream but are not fetched in the background.
- The panel's own refresh stays as it was: on every poll of the owner, at
  once on another row and on `End`; it feeds the same stream and gets the
  screen back.
- Tower also fetches right after it dispatches a session (`n`) or starts the
  orchestrator (`o`), and right after an attach returns. While it has
  handed the terminal to `claude attach`, that session is fetched every
  10 seconds whatever its state. The follower runs on its own thread, so
  it keeps going during every hand-over of the terminal.
- Bounds: at most 3 background fetches at a time, the longest due first;
  one fetch per session at a time (a request during a fetch runs right
  after it); at most 16 streams. Beyond 16 the rows that are not working
  go first, from the end of the list, then streams of rows not listed (yet),
  then working rows, the attached session last. A stream whose row has left
  the list (gone, stopped, owner unknown) is dropped after 30 seconds, so a
  session dispatched a moment ago keeps its stream until a listing shows it.
  A `logs` call that finds no live process or no such session drops the
  stream.

**Partial note.** When the panel's stream has no base, its title says
`partial output · attach once for a full screen` in both styles, next to the
paused indicator of [0028](0028-scrolling-the-logs-panel.md); the name goes
first and then the note shortens to `partial output` when the panel is
narrow. The bottom border starts with the same note when it fits. The note
goes away once a base arrives.

**Paused.** A paused panel keeps its snapshot, but its session's stream is
still fed by the background follow while it works. `End` / `G` fetches and
shows the stream as it is now.

**Cost.** One `claude logs` call measured on a working session: ~0.65 s
wall time, ~0.17 s CPU, ~150 MB peak memory while it runs, ~33 KB of output.
Following one working session costs about 2 % of a CPU core; 16 at once
about 27 %, with up to three calls (~450 MB) alive at a time. The 10-second
cadence leaves a capture four times the interval, so the overlap survives
frames several times faster than the spinner's or a fetch delayed by a slow
machine. Feeding a capture into a stream costs well under a millisecond.

## Consequences

- The panel shows the whole screen of any session tower has followed since
  its start or last attach, however long it works, and after it is done.
- Sessions that were working before tower started, or whose output outran
  the cadence (more than ~450 writes in 10 seconds), show the partial note
  until they are attached once.
- Working sessions cost a `logs` process every 10 seconds each even when
  the panel is closed.
- A session attached outside tower is caught only while it works or while
  its panel is open; otherwise its next repaint is the one tower sees.
- Each stream keeps an emulator: 32 bytes a cell (320 KB at 50 × 200) plus
  the bounded scrollback of [0028](0028-scrolling-the-logs-panel.md), which
  Claude Code's streams never fill.

## Alternatives considered

- **Force a repaint** (hidden attach, resize, `SIGWINCH`). Changes the
  session's terminal and wakes stopped sessions; rejected by
  [0016](0016-do-not-change-claude-code-defaults.md) and
  [0001](0001-thin-wrapper-over-claude-cli.md).
- **Follow only the panel's row.** Loses the base as soon as the user looks
  at another session for 40 seconds.
- **Follow every live row.** Idle rows write nothing; their stream keeps
  its base without fetches.
- **Keep every capture and replay the concatenation.** Same screen, growing
  memory and replay time.
- **Schedule fetches from the UI loop.** Simpler, but nothing would be
  fetched while tower is attached or in an editor, which is when sessions
  are watched least.

## Open items

- How fast Claude Code writes while streaming a long reply; above ~45
  writes a second the 10-second cadence loses the overlap.
