# 0028. Scrolling the logs panel

- Status: Accepted · 2026-10-19 · paused panel and the scrollback across fetches amended by [0030](0030-continuous-logs-stream.md) · title name shortened before it goes, amended 2026-10-21
- Related: [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0025](0025-session-terminal-size-for-logs.md), [reference](reference/claude-code-cli.md#claude-logs-output)
- Amends: the logs panel of [0009](0009-stop-remove-and-logs.md) and the key table of [0011](0011-keybindings.md)

## Context

The logs panel ([0009](0009-stop-remove-and-logs.md)) replays `claude logs` in an
emulator without scrollback and shows only the bottom rows that fit, half the body at
most. The session's terminal is usually taller than that panel (50 rows before the
first attach, the attaching terminal's size after, [0025](0025-session-terminal-size-for-logs.md))
and up to 600 columns wide, so most of the replayed screen was out of reach, and long
lines were cut at the panel's right edge.

Claude Code draws in place with cursor moves and never scrolls the terminal: its
`claude logs` stream holds the current screen, not the session's history
([reference](reference/claude-code-cli.md#claude-logs-output)). Other streams (a
future renderer, a corrupted stream) may scroll.

## Decision

**History.** The replay keeps a bounded scrollback: lines that scroll off the top of
the emulator's screen during the replay stay above it. The cap is 5000 lines, and at
most 1 000 000 cells, so wider screens keep fewer (5000 lines up to 200 columns, 1666 at
600). vt100 stores 32 bytes per cell, so the scrollback costs at most 32 MB, on the logs
worker thread and only during the replay; the panel keeps the converted lines (text and
spans of the trimmed rows), and draws share them instead of copying. The history is the
scrollback, oldest line first, then the screen, without blank rows at its top and bottom.
For Claude Code's streams it is the screen.
*Amended by [0030](0030-continuous-logs-stream.md):* the emulator is kept across
fetches, so the scrollback gathers the lines that scroll off over all of them, within
the same cap.

**Bounded replay work.** A scroll costs a new row and a shift of all rows (and with the
scrollback keeps the old row), for one byte. Line feeds (LF, VT, FF) and reverse index
(`ESC M`) therefore share a work budget like the ones of [0009](0009-stop-remove-and-logs.md):
each costs rows + columns, whether it scrolls or not, against 125 000 000 per replay
(about 470 000 line feeds at 64 × 200, 138 000 at 300 × 600); once it is spent, further
ones are dropped. Claude Code's streams have none. Any 2 MiB window, 2 MiB of nothing
but line feeds on a 300 × 600 screen included, replays in about 0.3 s in a release build
(it took about two seconds before, without the scrollback).

**Focus.** While the panel is open the list keeps the keys, as before. `→` or `l` give
them to the panel; its border turns thick (in the dashboard also in the theme's
selection-marker colour) and its title bold, and the footer shows the panel's keys.
`←`, `h` or `Esc` give them back to the list; `Esc` does not close the panel from there,
a second `Esc` from the list does. With the panel closed, `→` only says that `Space`
opens it.

**Keys with the panel focused:**

| Key | Action |
|---|---|
| `↑` / `↓`, `k` / `j` | One line |
| `PgUp` / `PgDn`, `Ctrl+U` / `Ctrl+D` | One page (the panel's height) |
| `Home` / `g` | Top |
| `End` / `G` | Bottom: follow the output again and fetch at once |
| `f` | Full screen on / off: the panel takes the whole body area (the list's place in the minimal style, list and detail in the dashboard), so long lines get the terminal's width |
| `←`, `h`, `Esc` | Back to the list; leaves full screen |
| `Space` | Close the panel |
| `q`, `Ctrl+C`, `?`, `r`, `Ctrl+Z` | As in the list |

Other list keys do nothing there; the status line says that `←` or `Esc` returns to the
list. The help (`?`) opened from the panel lists these keys instead of the dialog keys,
so it still fits 24 rows. The Cyrillic layout map of [0011](0011-keybindings.md) covers
the panel's keys like any command context (`д` is `l`, `р` is `h`, `а` is `f`, …).

**Lines.** The panel scrolls through the history as [0009](0009-stop-remove-and-logs.md)
fits it: unchanged when it fits the panel, otherwise with runs of blank rows collapsed
to one. Following, it shows the last page, which is what the panel showed before.

**Following and paused.** At the bottom the panel follows the output: it refreshes on
every poll, as before. Moving up from the bottom pauses it on the snapshot it shows:
polls fetch nothing for it, and a fetch already running when it paused is dropped.
*Amended by [0030](0030-continuous-logs-stream.md):* the snapshot stays, but the
session's stream is still fed by the background follow while it works, and `End`
shows the stream as it is then. The
title says where it is: `logs · <name> (<account>) · paused · line 120–160 of 900 · End
follows`, dropping the name when the panel is too narrow. *Amended 2026-10-21 (owner
request to fix cut text):* the name is first shortened with `…`, the account kept, and
goes only when fewer than 12 columns are left for it; a title without the position or a
note never drops it. `End` / `G`, or scrolling back
down to the last page, follows again and fetches at once. Selecting another row (or a
row with a note) follows from the bottom, and so does returning from an attach.

**Resizes.** The renderer reports the panel's height before drawing it; a paused
panel's first line is clamped to the new last page. Tiny terminals draw what fits.

**No mouse.** Tower does not enable mouse capture: it would take the terminal's own
text selection away.

## Consequences

- The whole replayed screen is reachable, and full screen shows lines up to the
  terminal's width. For Claude Code that is the current screen, not the conversation's
  history; the history needs `Enter` (attach).
- A paused panel can be stale; the title says so and `End` refreshes.
- One fewer `logs` process per poll while paused (the background follow of
  [0030](0030-continuous-logs-stream.md) still fetches a working session every 10
  seconds).
- `←` gets a meaning in tower, but only inside the logs panel, where it means "back",
  as in Claude Code; the session list still leaves it alone
  ([0011](0011-keybindings.md)).
- Streams that scroll keep up to 5000 lines; older lines are lost, as in a terminal.

## Alternatives considered

- **Keep refreshing while paused and keep the old snapshot.** Same view, but one wasted
  `logs` call and replay per poll; dropped for not fetching at all.
- **Mouse wheel scrolling.** Needs mouse capture, which breaks the terminal's text
  selection; rejected.
- **Unbounded scrollback.** A long scrolling stream could cost hundreds of megabytes;
  a cap in cells bounds it for any width.
- **Wrapping long lines instead of full screen.** Wrapped Claude Code screens (rules,
  boxes) become unreadable; full screen keeps the screen as drawn.
- **Scrolling from the list with modifier keys** (`Shift+↑`, …). No focus switch, but
  terminals report shifted arrows unevenly, especially over SSH ([0011](0011-keybindings.md)
  prefers single strokes); the focus keeps the familiar keys.
