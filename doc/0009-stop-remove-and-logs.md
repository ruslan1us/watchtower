# 0009. Stop, remove and the logs panel

- Status: Accepted · 2026-10-18 · emulator size amended by [0025](0025-session-terminal-size-for-logs.md) · scrollback, scrolling and the line-feed budget amended by [0028](0028-scrolling-the-logs-panel.md) · rows fetched, cadence and the replay across fetches amended by [0030](0030-continuous-logs-stream.md)
- Related: [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [reference](reference/claude-code-cli.md)

## Context

Besides attaching, the overview needs three actions on a row: stop a session, remove it from
the list, and peek at its recent output without attaching. Claude Code provides `stop`, `rm`
and `logs` ([reference](reference/claude-code-cli.md#cli-commands)). `logs` only works with
the command of the account hosting the session, and only while the session has a live
process (for a stopped session it exits 1); `stop` takes the short id only, not a name; `rm`
can delete a worktree. `logs` returns a raw terminal byte stream, not text
([reference](reference/claude-code-cli.md#claude-logs-output)). Destructive actions need a
confirmation, and work must never be deleted silently.

## Decision

All three actions use the command of the **owning account**
([0003](0003-accounts-and-session-ownership.md)) and the short `id`.

**Stop.** Run `<owning account command> stop <id>` with the short `id`; a name is rejected
with `No job matching '<name>'`. Sessions in state `working` or
`blocked` ask for confirmation first; others stop directly. Success prints `stopped <id>`
(exit 0, also when the session was already stopped); the conversation
is kept and the session can be woken by attaching.

**Remove.** After confirmation, run `<owning account command> rm <id>`. The transcript stays
on disk and remains resumable with `claude --resume`. `rm` also deletes the session's
worktree when that is safe. When it refuses (uncommitted changes, unpushed commits, worktree
in use) it prints a `--discard-unpushed …` or `--force-remove-worktree …` flag for a second
try.

Tower **never** passes `--discard-unpushed` or `--force-remove-worktree`. It shows the
refusal text and leaves resolution to the user: attach and commit or push, or run the
suggested command manually. These flags destroy work.

**Logs panel.** `Space` toggles a side panel for the selected row
([0011](0011-keybindings.md)); on terminals narrower than 150 columns it sits below
the list instead, since the replayed screen is wide. In the dashboard style
([0020](0020-visual-styles.md)) it takes the detail panel's place: right of the
list from 130 columns, below it on narrower terminals, half the space either way:

- Run `<owning account command> logs <id>` and read **stdout only**; the
  `Session <id> · <name>` banner arrives on another stream and is ignored.
- Feed the bytes into a fresh `vt100::Parser` and convert its screen cells to ratatui
  lines (tower's own conversion, not `tui-term`, so rows can be trimmed and fitted).
  *Amended by [0030](0030-continuous-logs-stream.md):* the parser is kept per session
  and each capture's new bytes are stitched onto it; a fresh replay happens only when
  captures do not overlap, the real size or the pid changes.
  Colours and bold/dim/italic/underline/reverse are kept; trailing spaces and blank rows at
  the top and bottom are trimmed. When the panel is too short, runs of blank rows collapse
  to one and the bottom rows are kept (prompt box or dialog and footer included).
- Emulator size: the PTY size is not reported. Tower uses the larger of a default of
  64 rows × 200 columns and the highest row and column addressed absolutely in the stream
  (CUP/HVP, VPA, CHA/HPA, the DECSTBM bottom margin, and CUF right after a carriage
  return), capped at 300 × 600. Other relative moves are not counted.
  *Amended by [0025](0025-session-terminal-size-for-logs.md):* when tower can read the
  session terminal's real size, the emulator uses exactly that (capped at 300 × 600); this
  inference is the fallback.
- Only the last 2 MiB of the stream are replayed, starting at the last full clear in that
  window, if any. Malformed sequences and invalid UTF-8 never fail the panel.
- Bounded replay work: vt100 handles some sequences in time proportional to a count
  (`ESC[999999@` took over a second). Before the replay, the count and position parameters
  of ICH, DCH, ECH, IL, DL, SU, SD, REP, CUF, CUB, CUU, CUD, CNL, CPL, CHA, HPA, VPA and
  the CUP/HVP row and column are clamped to the emulator size (REP to one screenful plus
  its remainder modulo the width), which leaves the result unchanged; positions inside
  the screen are kept as they are. The count sequences and the screen-wide ones (ED,
  DECSED, alternate-screen switches, reset) also share fixed work budgets per replay;
  once one is spent, further sequences of that kind are dropped. Claude Code's streams
  stay far below them. Any 2 MiB window then replays in well under a second.
  *Amended by [0028](0028-scrolling-the-logs-panel.md):* line feeds and reverse index
  share a budget too, and the emulator keeps a bounded scrollback (5000 lines, at most
  1 000 000 cells) above the screen.
- Refresh on every poll, only for the selected row and only while the panel is open.
  Changing the selection fetches the new row immediately.
  *Amended by [0030](0030-continuous-logs-stream.md):* in addition, every working row
  is fetched in the background every 10 seconds, and a session right after its
  dispatch and after an attach, so the panel keeps the full screen of long turns.
  *Amended by [0028](0028-scrolling-the-logs-panel.md):* `→` / `l` give the panel the
  keys to scroll it and `f` shows it full screen; while scrolled up it is paused on its
  snapshot and not refreshed until `End` or the last page.
- Interactive rows have no `id`; the panel shows a short note instead of output.
- Rows without a live process (no `pid` and no `status`,
  [0006](0006-session-states-and-grouping.md)) show the note "no live output — session
  stopped; attach to view it". Tower does not call `logs` for them, since it would only fail
  with `Couldn't read logs for <id> — job not found …` (exit 1).

## Consequences

- One extra `logs` process per poll while the panel is open and the selected row has a live
  process; nothing otherwise. *Amended by [0030](0030-continuous-logs-stream.md):* plus one
  every 10 seconds per working session.
- A row whose owning account is unknown ("?") cannot show logs until the account is known;
  the panel says so.
- Removing a session with unfinished work in its worktree is always a manual step outside
  tower's automation.
- The panel shows what the agent's own terminal shows, including spinners and Claude Code's
  UI chrome, so it changes with Claude Code releases without any parser change in tower.

## Alternatives considered

- **Strip ANSI and show text lines.** Breaks on cursor addressing; the result is garbled.
- **Read transcript files.** Violates the no-internal-files rule
  ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **Offer the force flags behind a second confirmation.** Rejected: one wrong key loses work,
  and the user can still run the printed command manually.
- **Try every account's command for `logs`.** Wasteful and masks a wrong sidecar entry.

## Open items

- Exit code and exact message format of an `rm` refusal.
- ~~A reliable way to learn the session's PTY size instead of inferring it.~~ Closed by
  [0025](0025-session-terminal-size-for-logs.md): read from the session process's PTY.
