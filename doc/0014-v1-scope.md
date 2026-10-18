# 0014. Scope of v1 and non-goals

- Status: Accepted · 2026-10-18
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0004](0004-account-hand-off.md), [0007](0007-visual-only-attention.md), [0010](0010-orchestrator.md), [0016](0016-do-not-change-claude-code-defaults.md)

## Context

Most of the value is in a few things: one list of every session, getting in and
out of a session quickly, and seeing who needs attention. Everything else can
wait until that works on a real host every day. A clear list of what is out of
scope also keeps the thin-wrapper rule ([0001](0001-thin-wrapper-over-claude-cli.md))
from eroding one feature at a time.

## Decision

v1 is delivered in five milestones. Each one is usable on its own.

1. **Read-only list and attach/detach.** Poll `agents --json --all` for the
   configured accounts, show every session with name, project, state, what it
   waits for and account. Enter attaches with the hosting account's binary;
   `Ctrl+Z` returns to tower with a refreshed list
   ([0005](0005-attach-and-detach.md)).
2. **Start, stop, remove, logs panel.** The new-session dialog
   ([0008](0008-new-session-dialog.md)); stop and remove with confirmation; the
   logs panel replaying `claude logs` ([0009](0009-stop-remove-and-logs.md)).
3. **Account selection and hand-off.** Choose the account when starting;
   detect the account of running sessions, ask for unknown stopped ones;
   "continue under another account" ([0003](0003-accounts-and-session-ownership.md),
   [0004](0004-account-hand-off.md)).
4. **Orchestrator session and instruction file.** The `orchestrator` session,
   dispatched on the first `o` and attached like any other, its instruction
   file under the config dir, and `tower status --json`
   ([0010](0010-orchestrator.md)).
5. **Polish.** Text filter, group by state or by project, collapsed and dimmed
   groups, attention highlight, tracked activity times, the full config file
   ([0006](0006-session-states-and-grouping.md),
   [0007](0007-visual-only-attention.md), [0002](0002-tech-stack-and-files.md)).

**Non-goals for v1**

- Own supervisor, PTY handling, tmux or process management.
- Agent SDK, direct API calls, reading Claude Code's internal files or
  credentials ([0001](0001-thin-wrapper-over-claude-cli.md)).
- Changing Claude Code's settings, keys or defaults
  ([0016](0016-do-not-change-claude-code-defaults.md)).
- Bell, sound, or OSC 9/777 desktop notifications
  ([0007](0007-visual-only-attention.md)).
- Replying to a session without attaching ("quick reply"); talk to the
  session, or ask the orchestrator to relay.
- An orchestrator that answers other sessions' questions on its own; it only
  reports and relays.
- Chat views, diffs, PR or merge-request buttons: attach and work in Claude
  Code as usual.
- Tower-only tags or custom groups.
- Usage and limit display per account.
- Automatic hand-off, or detecting that an account hit its limit.
- A default account per project; there is one global default.
- Pinning sessions (Claude Code has no CLI for it).
- Deleting transcripts. `claude rm` keeps them, and tower adds nothing that
  removes them.
- Platforms other than Linux x86_64. Account detection depends on `/proc`.

## Consequences

- Milestone 1 is enough to replace `claude agents` for daily use on one
  account; later milestones add the multi-account parts.
- Ideas outside this list go into a new decision before code is written.

## Alternatives considered

- **Hand-off first.** Hand-off is the main feature the built-in view lacks, but
  it is useless without a reliable list and attach, so it comes third.
- **Notifications in v1.** Dropped. Sound and desktop pop-ups are unwanted, and
  a highlighted row is enough while tower is on screen
  ([0007](0007-visual-only-attention.md)).
- **Quick reply in v1.** The CLI has no way to send a message without
  attaching, so it would need the orchestrator in the loop. Deferred.
