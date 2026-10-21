# 0035. A subagent continued with `SendMessage`

- Status: Accepted · 2026-10-21
- Related: [0002](0002-tech-stack-and-files.md), [0006](0006-session-states-and-grouping.md), [0016](0016-do-not-change-claude-code-defaults.md), [0029](0029-tracking-subagents.md), [reference](reference/claude-code-cli.md#hooks-passed-with---settings)
- Amends: the hook command's `SubagentStart` and `SubagentStop` rules and the file format of [0029](0029-tracking-subagents.md)

## Context

A session can continue a subagent that has finished by sending it a message with
its `SendMessage` tool. Claude Code then runs the agent again under the **same**
`agent_id` and fires `SubagentStart` for it again, after the `SubagentStop` of the
first run (verified on 2.1.306,
[reference](reference/claude-code-cli.md#hooks-passed-with---settings)). The
`SubagentStop`'s `background_tasks` lists the stopping agent itself as `running`, and
Claude Code's helpers stop with `agent_type: ""` and a `background_tasks` snapshot of
the session's agents.

[0029](0029-tracking-subagents.md) remembers the last 64 stopped ids so that a launch
reported after its agent ended is not taken as running, and it ignored a start of such
an id too. A continued agent was therefore running but never shown: an owner's session
had 19 ids in `stopped` and no agents in the file while its subagents worked.

## Decision

- **A start of a stopped id is a continuation.** `SubagentStart` for an id in
  `stopped` removes the id from `stopped` and adds the agent as running again, with
  `started_ms` set to now. Claude Code runs `SubagentStart` hooks before the
  subagent's turn begins (a start hook can add context to the agent), so a start
  never arrives after the stop of the same run. The `stopped` check stays for the
  `PostToolUse` `async_launched` launch, which is what it exists for.
- **The description survives.** The file remembers, for the same last 64 stopped
  agents, the type, description and background flag. A continued agent gets back
  its description and background flag; its type comes from the start.
- **Any stop fills descriptions.** A `SubagentStop` with `background_tasks`, also one
  of an agent not in the file (a helper), fills a missing description and type of
  agents in the file from their running entries there. Only the stop of an agent in
  the file drops anything, as before.
- **`SendMessage` is not hooked.** Its `PostToolUse` names the agent
  (`recipient_kind: "agent"`, `to`) but adds nothing the following start does not;
  the `PostToolUse` matcher stays `Agent`, so the settings file is unchanged.
- **File format.** A new optional field next to `stopped`, left out when empty:

  ```json
  "stopped_agents": [
    { "id": "a1b2c3d4e5f60718", "type": "general-purpose",
      "description": "probe agent", "background": true }
  ]
  ```

  `version` stays `1`. The reader of 0.4.x ignores unknown fields (no
  `deny_unknown_fields`), so a running older TUI reads files an updated hook writes;
  a file without the field reads as nothing remembered, and such a continued agent
  shows its type until a snapshot gives its description. Still no prompts, messages
  or paths are stored.

## Consequences

- A continued subagent shows under its session again, with its age from the
  continuation, and goes at its next stop.
- A hook binary of 0.4.x that writes a file drops `stopped_agents`; a continuation
  after that shows the type until a snapshot fills the description.
- A continued agent whose stop is lost stays in the file like any agent whose stop
  was lost: it shows while the row is `working`, until a later snapshot drops it (for
  a background agent) or the row leaves `working`.

## Alternatives considered

- **Objects in `stopped`.** One list, but a 0.4.x reader expects strings there and
  would fail to read the whole file.
- **Bumping `version`.** No reader checks it; a new optional field is enough.
- **Hooking `SendMessage`** to mark the continuation: the start that follows already
  says it, and the settings file would change for every session.
