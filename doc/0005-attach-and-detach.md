# 0005. Attaching to and detaching from a session

- Status: Accepted · 2026-10-18
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [CLI reference](reference/claude-code-cli.md)

## Context

Entering a session is the most frequent action. Claude Code already provides it:
`claude attach <id>` takes over the terminal, shows the normal Claude Code UI
and returns when the user detaches. The session keeps running afterwards. See
https://code.claude.com/docs/en/agent-view and the
[CLI reference](reference/claude-code-cli.md).

Behaviour verified with a wrapper script running `claude attach` inside an
isolated tmux server (standing in for tower):

| Key inside the attached session | Result |
|---|---|
| `Ctrl+Z` | `attach` exits to the caller. Exit code 0, `stty -g` identical before and after, no leftover processes, session keeps running |
| `←` on an empty prompt | Opens Claude's built-in agent view; `Esc` there exits to the caller, exit code 0 |
| `/exit`, or `Ctrl+C` twice on an empty prompt | Built-in agent view, then `Esc` exits to the caller, exit code 0 |
| `CLAUDE_CODE_DISABLE_AGENT_VIEW=1`, or `--settings '{"disableAgentView":true}'` | `attach` refuses: `'attach' is disabled by …`, exit code 1 |
| Unknown id | `No job matching '<x>'. Run 'claude agents' to list running sessions.`, exit code 1 |

Claude Code has no keybinding action for detaching; `←` and `Ctrl+Z` cannot be
rebound. `leftArrowOpensAgents` exists only in the global `~/.claude.json`, so
changing it would also change every normal `claude` session. Tower does not
touch it ([0016](0016-do-not-change-claude-code-defaults.md)).

Other terminal UIs that hand the terminal to a child process have hit an input
race: the TUI's event reader and the child both read stdin, and keystrokes get
lost or delivered to the wrong process. claude-squad is one known case.

## Decision

Attach runs `<owning account command> attach <id>`, using the short `id` (never
the name, which may match several sessions) and the account that owns the
session ([0003](0003-accounts-and-session-ownership.md)). Tower suspends its UI,
runs the command with inherited stdin/stdout/stderr, waits for it to exit and
then resumes.

**Before spawning**, tower fully releases the terminal:

1. Stop reading input events (the input loop is paused or exits; no other thread reads stdin).
2. Pop keyboard-enhancement flags and disable mouse capture and bracketed paste, if enabled.
3. Show the cursor, leave the alternate screen, disable raw mode, flush stdout.
4. Drain pending input events so no buffered keystroke reaches the child.
5. Print one plain line to the normal screen, e.g. `tower: attaching to fix-flaky-test (alt)…`.
   Waking a stopped session can take several seconds, and a supervisor may have to start first.
6. Install no-op handlers for `SIGINT`, `SIGQUIT` and `SIGTSTP` in tower for the
   duration of the attach. Handlers, unlike `SIG_IGN`, reset to the default on
   `exec`, so the child's own signal behaviour is unchanged.

**While attached**, tower draws nothing. The poller may keep running so that
transitions of other sessions are recorded and their highlights are visible on
return ([0007](0007-visual-only-attention.md)). Every background command tower
runs uses `stdin` = null and piped stdout/stderr, so nothing it spawns can
write to, or read from, the terminal the attached session owns.

**After the child exits**, tower restores its handlers, re-enters the alternate
screen and raw mode, re-applies the modes from step 2, clears, re-reads the
terminal size, redraws and forces an immediate poll.

**Detach keys.** `Ctrl+Z` is the documented way back to tower and is shown as a
footer hint ("in a session: Ctrl+Z returns to tower"). `←` then `Esc` also works
and needs no handling. Inside tower `Ctrl+Z` is a no-op ([0011](0011-keybindings.md)).

**Cases tower handles itself:**

- Interactive sessions (`kind: interactive`, no `id`) cannot be attached; tower
  shows a status-line message instead of running anything.
- Unknown owning account (account `?`): tower asks which account to use, stores
  the answer and then attaches ([0003](0003-accounts-and-session-ownership.md)).
- Non-zero exit: tower shows the exit code and the error text in the status line.
- A session that has disappeared since the last poll: tower refreshes first and
  reports "session no longer listed" instead of attaching.

The same procedure is used by dispatch-and-attach in the new-session dialog
([0008](0008-new-session-dialog.md)) and by the orchestrator key
([0010](0010-orchestrator.md)).

## Consequences

- No PTY, no screen proxying, no key interception: the attached session is
  exactly Claude Code's own UI with all its keys and commands.
- Attaching to a stopped session wakes it, under the account whose command is
  used. Using the wrong command silently moves the session to another account,
  so the account lookup is a hard requirement.
- A user who set `CLAUDE_CODE_DISABLE_AGENT_VIEW` or `disableAgentView` cannot
  attach through tower; tower reports Claude Code's message and does not override it.
- Tower must be careful that no thread reads stdin or writes to the terminal
  between steps 1 and the resume.

## Alternatives considered

- **Own PTY and a split view inside tower.** Rejected: process and PTY management are out of scope ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **Running sessions in tmux panes.** Rejected for the same reason; sessions already live in Claude Code's supervisor.
- **Setting `leftArrowOpensAgents=false` so `←` detaches.** Rejected: global config, changes the user's normal sessions ([0016](0016-do-not-change-claude-code-defaults.md)).
- **`claude --resume <sessionId>` instead of `attach`.** Rejected: on a running session it starts a copy under a new id.

## Open items

- Whether `claude attach` behaves the same with stderr piped. If it does, tower
  captures stderr to show the error text; if not, tower leaves stderr inherited
  and, on a non-zero exit, waits for a key before re-entering the alternate
  screen so the message stays readable.
- Whether the no-op signal handlers are needed at all in practice; verify with
  the tmux harness ([0015](0015-testing-strategy.md)).
- Waking a session that was stopped by Claude Code's idle rule (about 1 hour)
  rather than by `claude stop`: confirm the owning account is kept.
