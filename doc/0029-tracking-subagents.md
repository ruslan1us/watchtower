# 0029. Tracking the subagents of sessions tower starts

- Status: Accepted · 2026-10-19 · a start of a stopped id taken as a continuation, stopped agents' descriptions remembered and filled from any stop's snapshot by [0035](0035-continued-subagents.md) · `R` passes `--settings` too, amended by [0039](0039-subagent-hooks-for-resumed-conversations.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0019](0019-resume-past-sessions.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md), [reference](reference/claude-code-cli.md#subagents-in-background-sessions)

## Context

A session that uses its `Agent` tool runs subagents inside its own process. They are
not rows of `claude agents --json`, and the parent row only stays `working` / `busy`
while they run ([reference](reference/claude-code-cli.md#subagents-in-background-sessions)).
The owner wants to see them under each session. Claude Code's official hooks report
them: `SubagentStart`, `SubagentStop`, and `PostToolUse` of the `Agent` tool
([reference](reference/claude-code-cli.md#hooks-passed-with---settings)). Hooks can come
from a settings file, or per session from `claude --settings <file>`, and the two are
combined. A wake keeps the session's saved `--settings`.

Between editing a Claude Code `settings.json` and passing hooks per session, the owner
chose the per-session `--settings`.

## Decision

**Source.** Every session tower starts gets `--settings <config dir>/claude-settings.json`
(normally `~/.config/watchtower/claude-settings.json`): the new-session dispatch
([0008](0008-new-session-dialog.md)) and the orchestrator start
([0010](0010-orchestrator.md)). Wakes get nothing more: hand-off and rename wake a
stopped session with `--resume <sessionId> --bg`, which keeps its saved `--settings`
(verified), and any extra flag there would start a copy instead; attach passes no flags
either. `R` ([0019](0019-resume-past-sessions.md)) is unchanged: whether a
`--settings` given to the interactive `claude --resume` reaches the backgrounded copy is
not verified. Sessions started anywhere else show no subagents.
*Amended by [0039](0039-subagent-hooks-for-resumed-conversations.md):* it does (verified,
2.1.306), so `R` passes the same `--settings` file, before `--resume`, under the same
writing and refusal rules as a dispatch.

**The settings file** holds only the three hooks, nothing else:

```json
{
  "hooks": {
    "PostToolUse": [{ "matcher": "Agent", "hooks": [HOOK] }],
    "SubagentStart": [{ "hooks": [HOOK] }],
    "SubagentStop": [{ "hooks": [HOOK] }]
  }
}
```

with `HOOK` = `{ "type": "command", "command": "<tower> hook --dir <config dir>",
"timeout": 10 }`. `<tower>` is the running binary (`current_exe`, a ` (deleted)` suffix
after `tower update` dropped), each path one shell word, single-quoted when it holds
anything but plain path characters. A relative path, one that is not UTF-8 or one with
control characters is refused: tower then warns once and starts sessions without
`--settings`. Tower writes the file at startup and again before each dispatch, atomically
and only when the content differs; a dispatch that cannot write it runs without the flag.

**The hook command.** `tower hook [--dir <config dir>]` (internal; without `--dir`, the
usual config dir) reads one JSON payload from stdin, at most 1 MiB (a larger one is
ignored). It always exits 0 and never prints anything, not even a panic, so it cannot
block, alter or comment on what the session does; on any problem it records nothing.

- `SubagentStart`: adds the agent (`agent_id`, `agent_type`); a start without a type is
  ignored.
- `PostToolUse` of `Agent` with `tool_response.status` `async_launched`: adds or completes
  the agent (`agentId`, `tool_input.description`, `tool_input.subagent_type`) and marks it
  as background. Any other status (a foreground agent that finished) drops it.
- `SubagentStop`: drops the agent. A stop of an id not in the file is ignored (Claude
  Code's own helpers stop with `agent_type: ""` and no start). Otherwise, when
  `background_tasks` is given, background agents not running there are dropped and the
  others take descriptions and types from it.
- Everything else, and payloads with a `session_id` that is not a lowercase UUID or an
  agent id outside `[a-z0-9]{1,64}`, is ignored. Descriptions and types are made one
  printable line (control and bidirectional formatting characters removed) of at most
  120 and 64 characters.

**Files.** `<config dir>/subagents/<sessionId>.json` (folder mode `0700`), one per
session, written only by the hook:

```json
{
  "version": 1,
  "agents": [
    { "id": "a1b2c3d4e5f60718", "type": "general-purpose", "description": "alpha waiter",
      "started_ms": 1790000000000, "status": "running", "background": true }
  ],
  "stopped": ["f0e1d2c3b4a59687"]
}
```

`started_ms` is tower's clock when the hook first saw the agent; `stopped` remembers the
last 64 stopped ids, so a launch reported after its agent ended is not taken as running.
At most 64 agents per file; beyond that the oldest is dropped. Nothing else is stored: no
transcript paths, prompts or messages.

**Locking.** Hooks can run at the same time. Each read-modify-write holds an exclusive
`flock` on `subagents/.lock` (never removed), waiting at most 2 s, and writes a temp file
named with the pid and a counter, fsyncs and renames it over the file
([0002](0002-tech-stack-and-files.md)). Readers take no lock.

**Reading.** After each poll the TUI reads, on a worker thread, the files of rows that are
live and `working`, and exposes their running agents (id, type, description, start) per
`sessionId`. A row in any other state has none: the parent stays `working` while its
subagents run. `tower status --json` adds `subagents: [{id, type, description,
startedAt}]` to every session, empty when none ([0010](0010-orchestrator.md)); it only
reads. The list draws them as below.

**Display.** In both styles ([0020](0020-visual-styles.md)) a row with running agents
gets a count badge at the end of its name column and its agents as a tree under it:

```text
├─▾ Working 2 ─────────────────────────────────────────────────────────────────────
│▌  ⠋ schema-migration  +3  ~/workspace/billing-ui  working         5m  main
│▌     ├─ Internet research Queue→Stream         general-purpose    2m
│▌     ├─ Related docs search                    general-purpose    2m
│▌     └─ Codebase queue audit                   Explore            1m
│   ⠋ disk-analyze      +1  ~/workspace/analysis    working        29m  main
│      └─ Pipeline watcher                       general-purpose    4m
```

- **Badge** `+N`, N the running agents, right-aligned in the name column so badges line
  up; the column widens by the badge only when a badged name needs it, and a narrow column
  cuts the name first. Plain ASCII on purpose: `⑂` (U+2442), the first idea, is one cell
  for `unicode-width`, but none of the DejaVu and Ubuntu fonts checked has it (`↳` was
  only in DejaVu), so terminals would draw it from a fallback font whose glyph can spill
  into the next cell; `+` is in every font.
- **Child lines**, at most four agents in start order (oldest first); with more, the
  fourth is followed by `└─ … N more`. Each line: the tree glyph (`├─`, `└─` on the last;
  box drawing, one cell) one cell right of the name column's start; the description (else
  the type, else the id); the type in one column for the whole list, ending two cells
  before the age; the time since `started_ms` in the age column, in its format
  ([0006](0006-session-states-and-grouping.md)). The state and account columns stay empty.
- **Dimmed** with the theme's tokens ([0021](0021-dashboard-themes.md)): the description
  in the secondary grey, tree, type and age in the dim one, no background; in the minimal
  style and with `NO_COLOR` the dim attribute.
- **Not entries.** Child lines are drawn, not listed: every navigation key moves between
  sessions and group headers only, actions act on the session, and the selection (bar,
  `▌` and highlight) covers the row and its child lines. They follow their row: hidden
  when its group is collapsed, under it in project grouping, shown when the filter
  matches the session (the filter never matches child text). `N of M shown` and the group
  counts count sessions.
- **Viewport.** The list scrolls by whole entries and keeps the selected row with its
  child lines in view when they fit, else the row itself.
- **Narrow lists** cut descriptions first, then drop the type column; when fewer than
  8 cells are left for a description (the minimal style puts the age at the right edge
  when its age column is cut off), no child lines are drawn and only the badges show.
- A row without running agents, any row while tracking is off, and every row of a
  terminal without any subagents look exactly as before.

**Pruning**, at most once a minute by the TUI, under the lock: a file not written for
14 days; a file of a session that no listing shows, once it was not written for 1 hour,
and only while every account's latest poll succeeded with a full listing (the conditions
and ages of [0017](0017-ownership-evidence-rules.md) section 9); temp files older than
1 hour.

**Config.** `[sessions] track_subagents` (default `true`), also editable in the Settings
window as "track subagents" under General ([0022](0022-tower-writes-its-own-config.md),
[0024](0024-settings-window.md)). When `false`, no `--settings` is passed and the files
are ignored; sessions already started keep their saved hooks.

## Consequences

- Consistent with [0016](0016-do-not-change-claude-code-defaults.md), as amended here: no
  Claude Code settings file is written; the hooks only observe (exit 0, no output), apply
  only to sessions tower starts, and are combined with the user's own hooks, which keep
  working. The `--settings` flag is saved with the session and restored on wakes, so a
  session keeps reporting after a hand-off or rename.
- Consistent with [0001](0001-thin-wrapper-over-claude-cli.md): hooks are an official
  Claude Code interface, and tower reads only the files it wrote itself, never a
  transcript or other internal file.
- Each subagent start and stop costs a short `tower` process. Sessions keep the settings
  file's path, so moving or deleting the binary breaks their hooks until a tower writes
  the file again; Claude Code then reports a failing hook, the session is unaffected.
- Sessions started before this change, outside tower or through `R` show no subagents.
  *Amended by [0039](0039-subagent-hooks-for-resumed-conversations.md):* a conversation
  backgrounded from `R` with tracking on shows them.

## Alternatives considered

- **Editing a `settings.json`** (user or project): changes every session, not only
  tower's, and edits a Claude Code file; rejected by the owner and by 0016.
- **Reading transcripts or `jobs/`**: internal files, against 0001.
- **Scraping `claude logs`**: fragile and tied to the screen layout.

## Resolved

- Whether `--settings` on the interactive `claude --resume` of `R` reaches the
  backgrounded copy: it does (verified 2.1.306, WSL); see
  [0039](0039-subagent-hooks-for-resumed-conversations.md) and the
  [reference](reference/claude-code-cli.md#hooks-passed-with---settings).

## Open items

- What a subagent's permission prompt looks like in the listing.
