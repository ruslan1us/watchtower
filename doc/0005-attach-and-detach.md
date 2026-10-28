# 0005. Attaching to and detaching from a session

- Status: Accepted · 2026-10-18 · input still waiting after a read re-arms crossterm's reader, amended 2026-10-28
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
   `exec`, so the child's own signal behaviour is unchanged. `signal-hook`
   cannot put `SIG_DFL` back once its handler is installed, so the handlers
   are installed once, on the first attach, as conditional defaults: a flag
   makes them a no-op while attached and emulate the default action otherwise.

**While attached**, tower draws nothing. The child stays in tower's own
process group, the terminal's foreground group, so it reads the terminal and
gets the terminal's signals itself. Tower waits for it with `WUNTRACED`: if
`claude attach` is stopped (for example `Ctrl+Z` reaching it as `SIGTSTP` while
it is still in cooked mode during a slow wake), tower sends `SIGCONT` to the
child's whole process group instead of hanging on a stopped child. `Ctrl+Z`
stops every process in the group, so an account command that is a wrapper
script (one that does not `exec`) would otherwise wait forever on the real
`claude` behind it. Tower does no job control. A child that stops again right
after being resumed is resumed at most once every 100 ms, so tower never
spins on it and never leaves it stopped (this matters for a plain interactive
`claude`, [0019](0019-resume-past-sessions.md)).
The poller may keep running so that transitions of other sessions are recorded
and their highlights are visible on return
([0007](0007-visual-only-attention.md)). Every background command tower runs
uses `stdin` = null and piped stdout/stderr, so nothing it spawns can write to,
or read from, the terminal the attached session owns. Each one also runs in a
process group of its own ([0001](0001-thin-wrapper-over-claude-cli.md)), so
`Ctrl+C` or `Ctrl+Z` typed into the attached session never reaches an
in-flight poll, `stop`, `rm`, dispatch or hand-off.

**Terminal settings.** Tower saves the terminal settings (`tcgetattr`) once,
before it first enters raw mode, and puts them back (`tcsetattr`) right after
every foreground child has been reaped and on exit. Crossterm saves whatever
settings are current each time it enters raw mode, which after a hand-over can
be the raw settings of a child that died; without tower's own copy, such a
child would leave the user's shell in raw mode after tower exits. After a
non-zero exit or a death by a signal, tower also writes
`ESC[?1000l ESC[?1002l ESC[?1003l ESC[?1006l ESC[?2004l ESC[<u` (mouse
tracking, bracketed paste and one level of keyboard enhancement off) before
the prompt below, unless the terminal is gone.

**After the child exits** with a non-zero code or by a signal, tower first
prints `tower: attach exited with code N. Press any key to return to tower.` and
waits for a key, so Claude Code's error text (stderr stays inherited, see open
items) is readable before the alternate screen hides it. Then tower re-enters
the alternate screen and raw mode, re-applies the modes from step 2, clears,
re-reads the terminal size, redraws and forces an immediate poll. The no-op
handlers from step 6 stay in place until raw mode is back: until then the
terminal is in cooked mode, and a second `Ctrl+C` typed during a slow wake
would otherwise kill tower once the child has gone.

**Detach keys.** `Ctrl+Z` is the documented way back to tower and is shown as a
footer hint ("in a session: Ctrl+Z returns to tower"). `←` then `Esc` also works
and needs no handling. Inside tower `Ctrl+Z` is a no-op ([0011](0011-keybindings.md)).

**Cases tower handles itself:**

- Interactive sessions (`kind: interactive`, no `id`) cannot be attached; tower
  runs nothing and the status line says how to bring one in, for `Enter`,
  `a`, `s` and `d` alike: "<name> runs in another terminal — press ← on an
  empty prompt there (or /bg) to move it into the background; then it can be
  managed here".
- Unknown owning account (account `?`): tower asks which account to use, stores
  the answer and then attaches ([0003](0003-accounts-and-session-ownership.md)).
  The answer goes through the refresh below and is stored only if the
  refresh still shows no owner.
- Non-zero exit: after the key pause above, tower shows the exit code in the
  status line.
- A session that has disappeared since the last poll: tower refreshes first and
  reports "session no longer listed" instead of attaching. On `Enter` (and on
  the account picker's `Enter`) tower polls every account at once and looks
  for another tower's sidecar changes ([0017](0017-ownership-evidence-rules.md)
  section 8), both on worker threads; the status line shows
  `checking <name>… (Esc cancels)` and the UI keeps running. Tower attaches
  only once a listing that started after `Enter`, of the account whose command
  attaches, still contains the short id; otherwise it reports "session no
  longer listed". The account is the owner the refreshed merge shows, so an
  owner another tower recorded meanwhile (for example by a hand-off) wins over
  the one shown at `Enter`, and over the picker's answer. The wait is bounded
  at 5 s: then, or when that listing failed, tower attaches anyway and says so
  in the status line after the session returns. `Esc` cancels the wait, and
  an attach whose refresh ends while another window is open is dropped with a
  message. A row being handed off is refused, and an owner that became
  unknown opens the account picker, as on `Enter`. Dispatch-and-attach does
  not wait for a refresh: the new session is not listed yet.

**SIGTERM and SIGHUP.** Tower handles both with flag-setting `signal-hook`
handlers (installed once, reset on `exec` like the ones above). Outside an
attach the event loop sees the flag within one tick, restores the terminal and
exits with 128 + signal; after `SIGHUP` the terminal may be gone, so tower only
turns raw mode off and writes nothing. During an attach the child owns the
terminal, so tower forwards nothing and keeps waiting: the signal takes effect
only after `claude attach` exits (on an SSH hang-up the child loses the terminal
too and gets its own `SIGHUP`). Tower then skips the key pause and the redraw
and exits. The session is never stopped. While a hand-off runs, tower first
waits up to 15 s for it to report back and records its result
([0004](0004-account-hand-off.md)).

**A terminal that is gone.** The `SIGHUP` may never come: when a tmux server
or SSH connection goes away, only the session leader is sure to get it. The
terminal's input then reports EOF, an error or `POLLHUP`, and crossterm's
reader retries such a read forever without sleeping (tower at full CPU,
ignoring `SIGTERM`). So tower waits for input with its own `poll(2)` and only
calls crossterm when input is ready; any input EOF, error or hang-up, and a
failed draw on a hung-up terminal, count as `SIGHUP`: tower ends the loop and
exits with 129, writing nothing to the terminal. Every loop that waits for
input (the event loop, the key pause) waits at most one tick at a time and
checks the termination flag on every turn; none spins without a blocking wait
or a sleep. As a last resort against the instant between `poll` and
crossterm's read, a watchdog thread ends tower with 129 when it finds the
main thread inside crossterm's input code on a hung-up terminal twice, a
second apart.

*Amended 2026-10-28:* crossterm's reader waits with an edge-triggered
`epoll` and, when one wait reports a `SIGWINCH` together with terminal input,
returns the resize and leaves the input unread with its readiness used up;
it then sees that input only once more arrives. Since tower waits on the
input alone, a resize while tower is idle (terminals resize their window
when a full-screen program starts, and after a hand-over) was always
reported with the next key, which was then handled only after the key after
it, while the loop spun on the input `poll` kept reporting. So after every
read, while `poll` still reports input waiting, tower re-arms crossterm's
reader and reads again: it sets the terminal's current settings again
(`tcsetattr` with `TCSANOW`, nothing changed or flushed, nothing written),
and Linux wakes a terminal's readers on every settings change. At most 64
re-arms per read (a long paste goes on in the next turn); if a re-arm reads
nothing while input is still waiting, tower waits the turn's timeout
instead of spinning.

The same procedure is used by dispatch-and-attach in the new-session dialog
([0008](0008-new-session-dialog.md)), by the orchestrator key
([0010](0010-orchestrator.md)) and by `R`, which runs `<account command>
--resume` ([0019](0019-resume-past-sessions.md)).

## Consequences

- No PTY, no screen proxying, no key interception: the attached session is
  exactly Claude Code's own UI with all its keys and commands.
- Attaching to a stopped session wakes it, under the account whose command is
  used. Using the wrong command silently moves the session to another account,
  so the account lookup is a hard requirement.
- A user who set `CLAUDE_CODE_DISABLE_AGENT_VIEW` or `disableAgentView` cannot
  attach through tower; tower reports Claude Code's message and does not override it.
- Tower must be careful that no thread reads stdin or writes to the terminal
  between steps 1 and the resume. A panic on a worker thread therefore
  neither restores nor writes to the terminal: only a panic of the main
  thread does ([0001](0001-thin-wrapper-over-claude-cli.md)).
- The lines tower prints itself around a hand-over (the announcement, the
  key-pause prompt) quote session names, folders and commands; control
  characters in them are printed as `?` ([0007](0007-visual-only-attention.md)).
- A `kill <tower pid>` while attached appears to do nothing until the user
  detaches; then tower exits instead of returning to the list.
- A failed attach costs one extra key press, the price of a readable error.

## Alternatives considered

- **Own PTY and a split view inside tower.** Rejected: process and PTY management are out of scope ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **Running sessions in tmux panes.** Rejected for the same reason; sessions already live in Claude Code's supervisor.
- **Setting `leftArrowOpensAgents=false` so `←` detaches.** Rejected: global config, changes the user's normal sessions ([0016](0016-do-not-change-claude-code-defaults.md)).
- **`claude --resume <sessionId>` instead of `attach`.** Rejected: on a running session it starts a copy under a new id.

## Open items

- Whether `claude attach` behaves the same with stderr piped. If it does, tower
  captures stderr to show the error text; if not, tower leaves stderr inherited
  and, on a non-zero exit, waits for a key before re-entering the alternate
  screen so the message stays readable. Tower currently uses this fallback.
- Whether the no-op signal handlers are needed at all in practice; verify with
  the tmux harness ([0015](0015-testing-strategy.md)).
- ~~Waking a session that was stopped by Claude Code's idle rule (about 1 hour)
  rather than by `claude stop`: confirm the owning account is kept.~~ Verified by the
  owner 2026-10-28: attaching such a session on `alt` from tower kept `alt`.
