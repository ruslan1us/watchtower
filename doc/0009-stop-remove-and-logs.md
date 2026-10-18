# 0009. Stop, remove and the logs panel

- Status: Accepted · 2026-10-18
- Related: [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [reference](reference/claude-code-cli.md)

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
([0011](0011-keybindings.md)):

- Run `<owning account command> logs <id>` and read **stdout only**; the
  `Session <id> · <name>` banner arrives on another stream and is ignored.
- Feed the bytes into a fresh `vt100::Parser` and render its screen with `tui-term`.
  Colours are kept; blank rows at the top and bottom are trimmed.
- Emulator size: the PTY size is not reported. Tower uses the larger of a default of
  64 rows × 200 columns and the highest row and column addressed in the stream.
- Refresh on every poll, only for the selected row and only while the panel is open.
  Changing the selection fetches the new row immediately.
- Interactive rows have no `id`; the panel shows a short note instead of output.
- Rows without a live process (no `pid` and no `status`,
  [0006](0006-session-states-and-grouping.md)) show the note "no live output — session
  stopped; attach to view it". Tower does not call `logs` for them, since it would only fail
  with `Couldn't read logs for <id> — job not found …` (exit 1).

## Consequences

- One extra `logs` process per poll while the panel is open and the selected row has a live
  process; nothing otherwise.
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
- A reliable way to learn the session's PTY size instead of inferring it.
