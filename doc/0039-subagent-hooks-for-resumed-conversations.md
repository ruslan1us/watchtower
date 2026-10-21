# 0039. Subagent hooks for conversations resumed with `R`

- Status: Accepted · 2026-10-21
- Related: [0005](0005-attach-and-detach.md), [0016](0016-do-not-change-claude-code-defaults.md), [0019](0019-resume-past-sessions.md), [0029](0029-tracking-subagents.md), [reference](reference/claude-code-cli.md#hooks-passed-with---settings)
- Amends: the `R` part of the "Source" paragraph of [0029](0029-tracking-subagents.md), and the command `R` runs in [0019](0019-resume-past-sessions.md)

## Context

`R` runs `<account command> --resume` with no other argument
([0019](0019-resume-past-sessions.md)). A conversation the user picks there and
moves to the background with `←` continues as a copy under a new session id, and
[0029](0029-tracking-subagents.md) left it without the subagent hooks because it
was not known whether a `--settings` given to the interactive `--resume` reaches
that copy. The owner saw such copies show no subagents in tower.

Verified with Claude Code 2.1.306 on WSL
([reference](reference/claude-code-cli.md#hooks-passed-with---settings)):
`claude --settings <file> --resume`, a conversation picked, `←` on the empty
prompt. The copy's process runs with `--session-id <new> --fork-session --resume
<transcript> --settings <file> --model … --permission-mode …`, and when the copy,
attached with `claude attach`, launched a background subagent, `SubagentStart`,
`PostToolUse` (`Agent`, `async_launched`) and `SubagentStop` fired with the copy's
`session_id`; `tower hook` wrote `subagents/<copy sessionId>.json` with the agent
running and moved it to `stopped` when it ended.

## Decision

While `[sessions] track_subagents` is on, `R` runs
`<account command> --settings <config dir>/claude-settings.json --resume`, the
same file the new-session dispatch and the orchestrator start pass
([0029](0029-tracking-subagents.md)). `--settings` goes before `--resume`, as
verified; `--resume` stays last and without a value, so it still opens the picker.

The file follows the dispatch rules: before the hand-over tower writes it when its
content is not current, for the running binary. When it cannot be written, or the
binary or config path is refused (relative, not UTF-8, control characters), the
picker runs without `--settings`, and after it returns the status line gets the
same warning as at startup ("subagents: …; sessions start without the subagent
hooks"); it is a persistent warning, shown once however often it happens. With
tracking off, nothing is passed.

Everything else stays as [0019](0019-resume-past-sessions.md) says: the folder,
the account, the hand-over, and the printed line and the window's note, which
still name `<account command> --resume`.

## Consequences

- A conversation backgrounded from `R` shows its subagents like a session tower
  started, under the copy's session id. A wake of the copy is expected to keep the
  flag, as wakes of other sessions with `--settings` do (not verified for a copy).
- The interactive conversation also runs with the hooks before it is
  backgrounded: a subagent it runs there gets a file under its session id, which
  the pruning of [0029](0029-tracking-subagents.md) removes once no listing shows
  it.
- A conversation the user resumes and then leaves without backgrounding has run
  with observe-only hooks for that while; nothing else changes
  ([0016](0016-do-not-change-claude-code-defaults.md), as amended by 0029).
- A running background session picked in the picker is attached, not copied
  ([0019](0019-resume-past-sessions.md)); it keeps whatever settings it was
  started with.

## Alternatives considered

- **Leaving `R` without hooks.** The copies then never show subagents, which is
  what the owner reported.
- **`--resume --settings <file>`.** Not verified; `--resume` takes an optional
  value, so the verified order is kept.
