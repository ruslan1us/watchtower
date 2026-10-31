# Claude Code CLI behaviour Watchtower relies on

Verified against Claude Code 2.1.302 on 2026-10-18; facts marked 2.1.303 were verified
with that version (renaming and hooks passed with `--settings` on 2026-10-19), and the
[plan usage limits](#plan-usage-limits) with 2.1.312 on 2026-10-28, and
[`respawn`](#restarting-a-session-claude-respawn) with 2.1.316 on 2026-10-31. Re-verify after upgrades.

This page collects the facts about Claude Code that Watchtower's design depends on. Each
fact is marked:

- **documented**: stated in the Claude Code docs (URL given).
- **CLI help**: stated in `claude --help` or a subcommand's `--help`.
- **verified**: observed in tests on Linux, including the tmux attach/detach harness
  ([0015](../0015-testing-strategy.md)). Sanitized captures are under `tests/fixtures/`
  (e.g. `tests/fixtures/agents-json/after-stop-all.json`, `tests/fixtures/logs/`,
  `tests/fixtures/cli-output/`).

Open questions are listed under [Not yet verified](#not-yet-verified).

Watchtower uses only the public CLI and its JSON output; see
[0001](../0001-thin-wrapper-over-claude-cli.md). Ids, names, paths and timestamps below are
made up.

Docs referenced:

- Background sessions and agent view: https://code.claude.com/docs/en/agent-view
- Sessions and resume: https://code.claude.com/docs/en/sessions
- Cross-session messaging: https://code.claude.com/docs/en/cross-session-messaging
- Permission modes: https://code.claude.com/docs/en/permission-modes
- Keybindings: https://code.claude.com/docs/en/keybindings
- Skills and custom commands: https://code.claude.com/docs/en/skills,
  https://code.claude.com/docs/en/slash-commands,
  https://code.claude.com/docs/en/claude-directory
- Plugins: https://code.claude.com/docs/en/plugins-reference (manifest),
  https://code.claude.com/docs/en/plugins/components,
  https://code.claude.com/docs/en/plugins/loading,
  https://code.claude.com/docs/en/plugins/cli-reference

Example accounts used throughout: `main` (command `claude`, config dir `~/.claude`) and
`alt` (command `claude-alt`, a wrapper that sets `CLAUDE_CONFIG_DIR=~/.claude-alt`).

## CLI commands

| Command | Behaviour | Source |
|---|---|---|
| `claude --bg [--name N] [--model M] [--permission-mode P] [--effort E] [--agent A] [--settings F] "task"` | Starts a background session in the current directory (the session's `cwd`). Prints `backgrounded · <id> · <name>` and hint lines on **stdout**; `Starting background service…`, printed first when the supervisor has to start, goes to **stderr**. In an untrusted folder it fails (exit 1, see [Permissions and trust](#permissions-and-trust)). An unknown `--model` name is **not** rejected: exit 0 and the normal `backgrounded` line, then the session ends in `state: failed`. | CLI help, verified |
| `claude --version` | Prints `2.1.316 (Claude Code)` on stdout and exits 0. Starts no session. Used by the welcome window ([0048](../0048-first-run-welcome.md)). | verified (2.1.316) |
| `claude --resume <sessionId> --bg ["prompt"]` | Continues a stopped session in the background under the **same id**. Prints on stderr e.g. `note: woke session 1a2b3c4d with its saved options (--name, --permission-mode, --model).`; the saved options are the flags given at the original dispatch (without `--permission-mode` there, the note lists only `(--name, --model)`; with `--settings` there, it lists `--settings` too and the hooks keep firing, 2.1.303). With a prompt, the prompt becomes the next turn. Without a prompt it exits 0, prints `backgrounded · <id> · <name> (idle — send a prompt to start)` on stdout, and the session sits idle and does not continue by itself (a `done` session keeps `state: done`, `status: idle`; one stopped mid-turn wakes `state: working`, `status: idle`; used by the hand-off without a prompt, [0033](../0033-hand-off-without-a-prompt.md)); a permission prompt pending before the stop is not shown again. If the session is already running, starts a copy under a new id and prints a `note:` line (wording in [Sending a prompt to a background session](#sending-a-prompt-to-a-background-session), 2.1.306). `--` before the prompt ends the options: a prompt starting with `-` is then taken as the prompt, and the session still wakes under its own id. On a stopped session, flags beyond `--bg` (e.g. `--name X`) or the short id in place of the full `sessionId` also start a copy instead of waking it; a prompt such as `"/rename X"` does not ([Renaming a session](#renaming-a-session), 2.1.303). | CLI help, verified (stopped case) |
| `claude --resume` (no argument) | Opens the interactive session picker for the current folder (its worktree; background sessions marked `bg`). In the picker `Ctrl+A` shows every project on the machine, `Ctrl+W` every worktree of the repository, `Esc` leaves. Picking a conversation of a background session that is still running attaches to it (v2.1.285+); picking one of an unrelated project copies a `cd` and resume command to the clipboard instead of resuming. A conversation picked here starts in the permission mode a new session would, except plan mode. Picking an ended conversation opens it in the foreground; backgrounding it then starts a copy under a new session id (see [Attach and detach keys](#attach-and-detach-keys)). Used by `R` ([0019](../0019-resume-past-sessions.md)). | documented ([sessions](https://code.claude.com/docs/en/sessions)); ended case verified (2.1.303) |
| `claude agents --json [--all] [--cwd <path>]` | Lists active sessions, interactive and background. `--all` adds completed background sessions, including sessions stopped with `claude stop`, which are missing without it; `--cwd` filters by directory. Needs no TTY. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude attach <id\|name>` | Attaches the terminal to a background session. Part of a name is accepted. Wakes a stopped session. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude logs <id\|name>` | "Recent terminal output" of the session. Part of a name is accepted. **No line-limit flag.** Format below. Fails (exit 1, empty stdout) for a stopped session, since there is no live process. | CLI help, verified |
| `claude stop <id>` / `claude kill <id>` | Stops the session's process; the conversation is kept. **Short id only**: a name, a full `sessionId` or a pid gives `No job matching '<x>'`. Idempotent: on an already stopped session it prints `stopped <id>` and exits 0. | CLI help, verified |
| `claude rm <id> [--discard-unpushed <commit>@<worktree>] [--force-remove-worktree <worktree>]` | Removes the row and, when safe, the session's worktree. The transcript stays on disk. Works on exited sessions. Refuses on uncommitted changes, unpushed commits or a worktree in use, and prints the flag to pass for a second try. Can also refuse with `couldn't remove <id> — the background service may be restarting. …` (exit 1) while the hosting account's supervisor is not running; another account's command then removes it ([Multiple config dirs](#multiple-config-dirs-accounts), [0034](../0034-remove-with-another-account-when-the-service-is-down.md)). | CLI help, verified (success case; service refusal on the owner's server 2026-10-20) |
| `claude -p --output-format stream-json --verbose --no-session-persistence --safe-mode "/usage"` | Runs `/usage` as a local command (no model turn) and prints the plan's usage, structured in `usage_report`; see [Plan usage limits](#plan-usage-limits). Used by [0043](../0043-plan-usage-display.md). | verified (2.1.312) |
| `claude respawn <id>\|--all` | "Restart a background session (or all of them) so it picks up the current Claude binary." **Short id only**: a name gives `No job matching '<x>'`. Details in [Restarting a session](#restarting-a-session-claude-respawn). `--all` was not run. | CLI help; `<id>` verified (2.1.316) |
| `claude daemon status\|logs\|stop [--any] [--keep-workers]` | Supervisor information (pid, version, socket dir, worker counts), its log, stopping it. `status` is text for people; see [Restarting a session](#restarting-a-session-claude-respawn) for what it says about versions. | CLI help, verified (2.1.316) |

### Option values

| Option | Values | Source |
|---|---|---|
| `--permission-mode` | `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan` | CLI help; documented ([permission modes](https://code.claude.com/docs/en/permission-modes)) |
| `--effort` | `low`, `medium`, `high`, `xhigh`, `max` | CLI help |
| `--model` | aliases such as `fable`, `opus`, `sonnet`, `haiku`, or a full model name | CLI help |

Prompt length: agent view rejects a dispatched prompt shorter than 4 characters. This is
documented ([agent view](https://code.claude.com/docs/en/agent-view)) for agent-view
dispatch only; whether `claude --bg` enforces the same minimum is not verified.

## JSON listing (`claude agents --json`)

Parse leniently: ignore unknown fields, default missing ones. The docs recommend the JSON
output over reading Claude Code's files.

| Field | When present | Meaning |
|---|---|---|
| `cwd` | always | working directory of the session |
| `kind` | always | `interactive` or `background` |
| `startedAt` | always | start time of the **current process**, Unix ms |
| `id` | always on background rows | short id used by `attach`, `logs`, `stop`, `rm` |
| `state` | always on background rows | `working`, `blocked`, `done`, `failed`, `stopped` (documented; after `claude stop` the row keeps its last value instead, see below) |
| `pid` | while the process is alive | OS process id |
| `status` | while the process is alive | `busy`, `waiting`, `idle` |
| `waitingFor` | when `status` is `waiting` | `permission prompt`, `input needed`, `sandbox request`, `worker request`, `dialog open` |
| `sessionId` | when set | full UUID, used by `--resume` |
| `name` | when set | display name |

Source: documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified.

Not in the JSON: last-activity time, status sentence or summary, PR link, pinned flag,
account or config dir. Agent view shows a one-line summary and a time since last activity,
but neither is exposed to the CLI (verified).

Sample rows (made up, shape as observed):

```json
{ "pid": 4242, "cwd": "~/workspace/example-api", "kind": "interactive",
  "startedAt": 1790000000000, "sessionId": "0f0e0d0c-0000-4000-8000-000000000000",
  "name": "refactor-config", "status": "busy" }

{ "pid": 4343, "id": "1a2b3c4d", "cwd": "~/workspace/example-api", "kind": "background",
  "startedAt": 1790000100000, "sessionId": "1a2b3c4d-0000-4000-8000-000000000000",
  "name": "fix-flaky-test", "status": "waiting", "waitingFor": "permission prompt",
  "state": "blocked" }

{ "pid": 4444, "id": "5e6f7a8b", "cwd": "~/workspace/example-web", "kind": "background",
  "startedAt": 1790000200000, "sessionId": "5e6f7a8b-0000-4000-8000-000000000000",
  "name": "update-deps", "status": "idle", "state": "done" }

{ "id": "9c8d7e6f", "cwd": "~/workspace/demo-app", "kind": "background",
  "startedAt": 1790000300000, "sessionId": "9c8d7e6f-0000-4000-8000-000000000000",
  "name": "add-readme", "state": "done" }
```

The last row is a session after `claude stop`: listed only with `--all`, no `pid`, no
`status`, last `state` kept.

Observed `state` / `status` / `waitingFor` combinations (verified):

| Situation | `state` | `status` | `waitingFor` |
|---|---|---|---|
| Working on a turn | `working` | `busy` | — |
| Writing command in `manual` mode (e.g. `touch <file>`); a read-only command such as `date` ran without a prompt | `blocked` | `waiting` | `permission prompt` |
| Question asked with the question tool | `blocked` | `waiting` | `input needed` |
| Turn ended with a question in the reply (e.g. "What would you like to know?"), seen on the orchestrator session | `blocked` | `idle` | — |
| Turn finished | `done` | `idle` | — |
| Turn finished while a background task it started (e.g. a shell) still runs; the footer shows `1 shell` (2.1.303, see [Background shells and the turn](#background-shells-and-the-turn)) | `done` | `busy` | — |
| Failed (e.g. unknown `--model` name); the process stays alive, `pid` present | `failed` | `idle` | — |
| Woken with `--resume <sessionId> --bg` and no prompt after a stop mid-turn, right after the wake (the session does not continue by itself) | `working` | `idle` | — |
| Woken with `--resume <sessionId> --bg` and no prompt after a stop with `state: done` | `done` (kept) | `idle` | — |
| Stopped with `claude stop` (only with `--all`) | last value kept (observed: `done`) | absent | — |
| Process ended without `claude stop` (seen on the owner's server, 2026-10-20, Claude Code version not recorded) | last value kept (observed: `blocked`) | absent | — |

Observed oddities (verified):

- Interactive sessions are listed but have no `id` and no `state`; they cannot be attached.
- `state: "blocked"` with `status: "idle"` and no `waitingFor` is what a session reports when
  its turn ended by asking the user something in the reply (verified with the orchestrator
  session, which ends almost every turn with a question). The same combination was also
  seen on a session whose login had expired, so it does not say why the session waits.
- The short `id` was observed to equal the first 8 characters of `sessionId`. This is
  undocumented; do not rely on it.
- `startedAt` is reset when a session is woken; it is not the session's creation time.
- `state: "stopped"` was seen once (2.1.303): after `claude stop` of a session that had been
  woken with `--resume <sessionId> --bg "/rename X"` and had run no model turn since, the row
  was listed with `state: "stopped"`, no `pid`, no `status`. Sessions stopped after a finished
  turn kept their last value (below).
- `state: "stopped"` was not seen after `claude stop` of a session that had finished a
  turn. The stopped session disappears from `--json` and appears only in `--json --all`,
  keeping its last `state`, with no `pid` and no `status`. While every session was alive, `--json` and `--json --all` returned the same rows.
  A row without a live process is therefore recognised by the missing `pid` and `status`, not
  by its `state`.
- In a check in WSL on 2.1.303 (2026-10-20, one account), a session stopped with
  `claude stop` was listed with `state: "stopped"`.
- A session whose process ended without `claude stop` was listed with `state: "blocked"`, no
  `pid` and no `status` (owner's server, 2026-10-20, version not recorded).

## `claude logs` output

Verified:

- Output is the **raw terminal byte stream** of the session's recent output, not text lines.
- While the session's start or last attach is recent, it starts with a clear-screen and
  full redraw (`ESC 7 ESC 8 ESC[2J ESC[H`), followed by incremental updates: absolute and
  relative cursor moves (`ESC[64;1H`, `ESC[57B`), colour SGR and every spinner frame.
  Later captures hold only incremental updates (see the capture window below).
- Printed to a terminal it looks like a screenshot of the current screen. Plain ANSI-to-text
  conversion does not work because of cursor addressing; it needs a terminal emulator.
- The PTY size is not reported, and `claude logs` has no option for it
  (`Usage: claude logs <id|name>`). After the last full clear the stream mostly uses
  relative moves and relies on autowrap: in one session last attached at 57 × 231, the
  stream after its last `ESC[2J` addressed at most row 57 and column 24 while its prompt
  rules were 231 wide. Replaying it at another size garbles it.
- A `Session <id> · <name>` banner is written to a different stream than the screen bytes
  (first in a pipe, last in a terminal), probably stderr.
- `logs` works only through the binary of the account hosting the session (see below).
- For a stopped session `logs` exits 1 with empty stdout and stderr
  `Couldn't read logs for <id> — job not found — it may have already exited`. An unknown id
  exits 1 with `No job matching '<x>'. …`.

Session terminal size, verified with Claude Code 2.1.303 on Linux:

- A background session's worker process (the `pid` of its row) has fd 0, 1 and 2 on its
  own PTY slave, `/dev/pts/<n>`. Field 7 (`tty_nr`) of its `/proc/<pid>/stat` names that
  device (for example 34819 = major 136, minor 3 = `/dev/pts/3`).
- A session that was never attached has a 50 rows × 200 columns PTY.
- `claude attach` resizes the PTY to the attaching terminal (tested at 57 × 231), and the
  PTY **keeps that size after detaching** (`Ctrl+Z`).

Capture window and repaints, verified with Claude Code 2.1.303 on Linux (a background
session running one four- or five-minute shell command, captured every ~9 seconds, and
attached once from tmux):

- `claude logs` returns the **last ~450 writes** of the session's output, dropped whole:
  32–38 KB, about 40 seconds of a working session (~11 frames a second, ~780 bytes a
  second). A capture always starts at a write boundary.
- Claude Code repaints the whole screen (`ESC[2J ESC[H`, then every row) **only when the
  session starts and when a terminal attaches**, also at the same size. Turn end, new
  messages, tool start and end, and timers are differential frames only. The stream has no
  `ESC[3J` and no alternate screen.
- A differential frame is `ESC[H\r`, relative moves to the changed cells
  (`ESC[<n>B`, `ESC[<n>C`, `\r`) with their new text, and a park sequence
  (`ESC[<rows>;1H ESC[<rows-2>;3H`, the cursor in the prompt box).
- So after about 40 seconds of work no capture holds the last repaint, and replaying one
  capture on its own shows only the cells changed since: spinner, footer fragments and the
  newest message lines, without the header, prompt box or static footer. A finished turn
  looks complete only when its last reply rewrote every visible row.
- Captures ~9 seconds apart overlap by ~25 KB; while the session is idle consecutive
  captures are byte-identical. Appending each capture's bytes after the overlap to the
  previous ones and replaying the result gives the right screen across a whole turn and an
  attach. The overlap is gone when more than ~450 writes pass between two captures.
- The colour depth follows the attached client: 256-colour SGR (`ESC[38;5;<n>m`) while
  attached from tmux, 24-bit (`ESC[38;2;r;g;bm`) otherwise.
- One `claude logs` call on a working session took ~0.65 s wall time, ~0.17 s CPU and
  ~150 MB peak memory (an unknown id: ~0.18 s, ~0.13 s CPU).

Scrolling, verified with Claude Code 2.1.303 on Linux (a background session asked to
print 300 numbered lines, its terminal 50 × 200, never attached), and on every captured
fixture under `tests/fixtures/logs/`:

- The stream contains no line feed (LF, VT, FF), no reverse index (`ESC M`) and no
  scroll-up or scroll-down sequence (`ESC[<n>S`, `ESC[<n>T`). Claude Code draws in place
  with absolute (`ESC[<r>;<c>H`) and relative (`ESC[<n>B`, `ESC[<n>C`, `ESC[<n>G`) moves,
  so the terminal's screen never scrolls and nothing reaches a terminal's scrollback.
- The stream was 9 KB. Replayed at 50 × 200 its final screen showed lines 258–300, the
  end-of-turn line, the prompt box and the footer; lines 1–257 appear in the stream only as
  intermediate frames that later frames overwrite in place. `claude logs` therefore gives
  the session's current screen, not its history.

How Watchtower uses it: [0009](../0009-stop-remove-and-logs.md),
[0025](../0025-session-terminal-size-for-logs.md) (the replay uses the PTY's real size),
[0028](../0028-scrolling-the-logs-panel.md) (scrolling the replayed screen),
[0030](../0030-continuous-logs-stream.md) (one stream per session, stitched across
captures and followed in the background).

## Attach and detach keys

Verified by running `claude attach <id>` from a wrapper script inside tmux, simulating tower.
Docs: [agent view](https://code.claude.com/docs/en/agent-view).

| Key or setup | Result |
|---|---|
| `Ctrl+Z` inside the attached session | `attach` exits to the caller at once. Exit code 0, terminal settings (`stty -g`) identical before and after, no leftover processes, session keeps running. |
| `←` on an empty prompt | Opens Claude Code's built-in agent view. `Esc` there exits to the caller, exit code 0, terminal restored. |
| `/exit` | Opens the built-in agent view (then `Esc` returns to the caller). |
| `Ctrl+C` twice on an empty prompt | Opens the built-in agent view (then `Esc` returns to the caller). |
| `CLAUDE_CODE_DISABLE_AGENT_VIEW=1 claude attach …` | Refused: `'attach' is disabled by CLAUDE_CODE_DISABLE_AGENT_VIEW.`, exit code 1. |
| `claude --settings '{"disableAgentView":true}' attach …` | Refused: `'attach' is disabled by the 'disableAgentView' setting.`, exit code 1. |
| `claude attach <unknown>` | `No job matching '<x>'. Run 'claude agents' to list running sessions.`, exit code 1. |

In a plain interactive session (started in the terminal, not attached), `/bg` (alias of
`/background`) or `←` on an empty prompt moves the conversation into a background session:
a fresh process resumes it under the supervisor and it is listed like any dispatched
session; `←` also opens agent view, where `Esc` leaves. `/bg` in a session with no message
yet asks for one first; `←` works at once. Documented
([agent view](https://code.claude.com/docs/en/agent-view)); the docs say nothing about
`Ctrl+Z` in a plain interactive session.

The two cases differ in agent view. After `←` from an **attached** session (`claude attach`),
`Esc` returns to the caller (table above). After `←` from a **foreground** conversation, here
an ended one opened from the `claude --resume` picker, verified with Claude Code 2.1.303:

| Key or step | Result |
|---|---|
| `←` on an empty prompt | Backgrounds the conversation and opens agent view with the line "Your conversation moved to the background — enter opens it · esc returns to it · ctrl+c twice quits". |
| `Esc` in that agent view | Reopens the conversation (possibly "Still starting — try again in a moment" first). Does **not** exit to the caller. |
| `Ctrl+C` twice in that agent view | Exits agent view; the `claude --resume` process ends and the caller (tower) is back. |
| The backgrounded process | A **copy under a new session id**: it runs with `--session-id <new> --fork-session --resume <transcript>`. The original ended conversation is left as it was. The copy is listed by `claude agents --json` like any background session, under the account whose command ran `--resume`. |

`/bg` in this flow is not tested.

- There is no keybinding action for detaching; `←` and `Ctrl+Z` are not rebindable. The
  `Agents` keybinding context has only `switchView`, `togglePin`, `find`, `rename`,
  `previousGroup`, `nextGroup` (documented, [keybindings](https://code.claude.com/docs/en/keybindings)).
- `leftArrowOpensAgents=false` exists only in the global `~/.claude.json`, cannot be passed
  with `--settings`, and is documented for foreground sessions only. Untested; Watchtower
  does not change it ([0016](../0016-do-not-change-claude-code-defaults.md)).

How Watchtower uses it: [0005](../0005-attach-and-detach.md).

## `claude attach` in a pseudo-terminal

Verified on 2.1.314 in WSL (2026-10-30), with `claude attach <id>` as the child of a PTY:

- It runs correctly with no shell around it, always in fullscreen rendering. It enables
  `?1049h ?2004h ?2031h ?1004h ?1000h ?1002h ?1003h ?1006h` and toggles `?25l` / `?25h`.
- It queries XTVERSION (`CSI >0q`), DA1 (`CSI c`) and, with `COLORTERM=truecolor`, `?2026$p`.
  Nothing breaks if they stay unanswered.
- `TERM_PROGRAM=ghostty` makes it push kitty keyboard flags, so the variables that name a
  terminal change what it speaks; `TERM=xterm-256color` and `COLORTERM=truecolor` without them
  give plain xterm input.
- `vt100` 0.17 rendered a real stream identically to `alacritty_terminal`.
- It paints in about 0.19 s; waking a stopped session this way takes about 0.93 s. Attaching
  keeps a stopped session's short id and wakes it under the command used.
- `SIGHUP`, `SIGTERM`, `SIGKILL` or closing the PTY ends the client in about 0.05 s, and the
  session keeps running.
- `claude stop` while attached: the client exits 0 with a `has exited` message. An unknown id:
  the error text in the terminal, exit 1. `Ctrl+Z` exits 0.
- The session's PTY takes the last client's size, and Claude Code repaints on a resize.
- `Shift+Enter` arrives as `CSI 13;2u`. `Ctrl+B` is Claude Code's. Occasional stale cells come
  from its stream; `Ctrl+L` clears them.
- A mouse release after a drag copies the selection. On WSL that runs PowerShell `Set-Clipboard`
  and overwrites the Windows clipboard, whatever terminal the client has.

How Watchtower uses it: [0045](../0045-workspace-style.md).

## Renaming a session

Verified with Claude Code 2.1.303 on Linux:

- There is no rename command or option: `claude --help`, `claude agents --help`,
  `claude attach --help` and `claude respawn --help` list none.
- Inside a session (here an attached background session), `/rename <name>` prints
  `⎿  Session renamed to: <name>`. It is a local command: no model turn. The `name` in
  `claude agents --json` changes at once, and the prompt box's top rule shows the new name.
  A name with spaces (`/rename two words`) is taken whole.
- The new name survives a stop and a wake: `--resume <sessionId> --bg` afterwards printed
  `note: woke session <id> with its saved options (--model, --name).` and
  `backgrounded · <id> · <new name> (idle — send a prompt to start)`.
- `--resume <sessionId> --bg "/rename X"` on a stopped session wakes it under the running
  command's account (stderr `note: woke session <id> with its saved options (--name, --model).`,
  stdout `backgrounded · <id> · <old name>`), runs `/rename` without a model turn, and the
  listing then shows `name: X`, `state: working`, `status: idle`. The first listing right
  after the wake may still show the old name. A `claude stop <id>` afterwards stops it; it was
  then listed with `state: "stopped"`.
- `--resume <sessionId> --bg --name X` on a stopped session does **not** rename it: stderr
  `note: background session <id> keeps its own saved options, so the flags you passed started a
  copy as <copy id>. Without flags, the same command continues <id> itself.`, stdout
  `backgrounded · <copy id> · X (idle — send a prompt to start)`.
- `--resume <short id> --bg` starts a copy too: ``note: started a copy of that conversation
  as <copy id>. To continue a session under its own id, pass its full session id (lowercase,
  as `claude agents --json` prints it) to --resume.`` A copy started without a name is listed
  with its short id as its name.

How Watchtower uses it: [0027](../0027-rename-sessions.md).

## Sending a prompt to a background session

Verified with Claude Code 2.1.306 on Linux (one account, a session that
had finished a turn, `state: done`, `status: idle`):

- On the **live** session, `claude --resume <sessionId> --bg "text"` does
  not deliver the prompt to it: exit 0, stdout `backgrounded · <copy id>`
  plus the usual hint lines, stderr ``note: session <id> is already running
  in the background, so this started a copy as <copy id>. `claude attach
  <id>` opens the original.`` The copy runs the text.
- After `claude stop <id>` (and once the listing shows no `pid` and no
  `status`), `claude --resume <sessionId> --bg "<text>"` printed
  `backgrounded · <id> · <name>` with the same short id and stderr `note:
  woke session <id> with its saved options (--name, --model).`; the text
  became the next turn. A three-line text arrived intact as one prompt
  (`claude logs` showed the three lines, and the session answered `3`
  when asked how many lines the message had).
- `claude --resume <sessionId> --bg -- "- reply with …"` on the stopped
  session: same id, no copy, no option error; the prompt `- reply with …`
  was the next turn. A multi-line prompt whose lines start with `-v` and
  `--help` passed after `--` arrived intact too. So `--` ends the options
  and does not count as a flag that starts a copy.
- A new session takes it too (verified 2026-10-28, Claude Code 2.1.312, WSL):
  `claude --bg --name <name> -- "- reply with the single word DASH, without using
  tools"` exited 0, printed `backgrounded · <id> · <name>`, gave no option error, and
  the session ran the text as its prompt (`claude logs` showed the text and the reply
  `DASH`). `tower dispatch` passes `--` only before a task that starts with `-`
  ([0044](../0044-orchestrator-starts-and-wakes-sessions.md)).
- A session's `SendMessage` to another session arrives there as that
  agent's message, not as the user's prompt.

How Watchtower uses it: [0040](../0040-quick-reply.md).

## `/clear` in a background session

Verified on 2.1.312 in WSL with one account (2026-10-30), attaching to a background session and
running `/clear`:

- The conversation is gone: asked afterwards about a word it was told to remember, the session
  had no such instruction.
- The short `id` and the `name` stay; the row stays `done` / `idle` with its process.
- The `sessionId` changes to a new one while the session is live. After `claude stop` the row
  shows the original `sessionId` again; `--resume <that id> --bg` wakes the same short `id`, and
  the live row then shows the `sessionId` from after `/clear`.
- A `CLAUDE.md` in the session's folder still applies after `/clear`, without the start prompt
  (verified on 2.1.314 with a session in the orchestrator folder, started with the orchestrator's
  start prompt: after `/clear` it answered an overview question in the format its `CLAUDE.md`
  asks for and, asked to start a session, stated the details and asked for confirmation first).
- Not verified: whether pending `notify_when_idle` notices survive `/clear`.

## Restarting a session (`claude respawn`)

`claude respawn --help` (2.1.316) prints `Usage: claude respawn <id>|--all` and `Restart a
background session (or all of them) so it picks up the current Claude binary.`

Verified with `<id>` on 2.1.316 in WSL with one account (2026-10-31), on one throwaway session
started with `--name`, `--model`, `--permission-mode manual` and `--settings`. Only one version
was installed, so the binary did not change across these restarts:

| Situation before | stdout, exit | Row afterwards |
|---|---|---|
| Idle, `done`, `idle` | `respawned <id>`, 0 | `done`, `idle`; new `pid`, new `startedAt` |
| Permission prompt pending, `blocked`, `waiting` | `respawned <id>`, 0 | `blocked`, `idle`, no `waitingFor`. The prompt is gone and the command did not run; after an attach the transcript shows `Interrupted · What should Claude do instead?` |
| Running turn with a foreground command, `working`, `busy` | `respawned <id>`, 0, in about 1.2 s | `blocked`, `idle`, stable for at least 10 s: the turn does not continue, its command is gone, and the transcript shows `Interrupted · What should Claude do instead?` |
| Stopped (`claude stop`), no `pid` | `respawned <id>`, 0 | **It wakes**: new `pid`, `working`, `idle`, no turn |
| After `/clear`, live | `respawned <id>`, 0 | The `sessionId` from after `/clear` stays |

- Stderr was empty each time. An unknown id or a name: stderr `No job matching '<x>'` and exit 1.
- Kept, each checked after a restart: the short `id`, the `sessionId`, the `name`; the
  conversation (asked afterwards, it quoted an earlier message); the permission mode (the footer
  says `manual mode on`, and a writing command asked again); the model (`/status` shows it); the
  hooks of `--settings` (a `Stop` hook fired on the next turns).
- A `claude attach` client of the session ends when it restarts, with exit 0, printing
  `Session <id> has exited.` and a line with `claude --resume <sessionId>`. Attaching again works.
- While it runs the row lists without `pid` and `status`, so a poll in that moment sees it stopped.
- `/proc/<pid>/cmdline` of the session is `claude bg-spare --bg-spare <socket>` both before and
  after, so it does not show the saved options. `/proc/<pid>/exe` is a versioned path.

Not run: `--all`, and `respawn` without an id, because they would restart the owner's working
sessions. So it is not known whether `--all` wakes stopped sessions as `<id>` does, or what it
reaches when two accounts share their lists.

**Versions.** The listing has no version field. `claude daemon status` prints the supervisor's
version and, as of 2.1.316, a count such as `2 from a different CLI version (most stay attachable
and upgrade automatically once idle — exec runs never respawn)`, per account and not per session.
That automatic upgrade was not observed here.

How Watchtower uses it: [0047](../0047-tab-title-pinning-and-restart.md).

## Supervisor lifecycle

Documented ([agent view](https://code.claude.com/docs/en/agent-view)) and verified:

- One supervisor per `CLAUDE_CONFIG_DIR`. It is transient: started on demand, exits when it
  has no workers or clients. Any command may start it first and print
  `Starting background service…`, which adds a delay.
- Unattached, unpinned sessions that are finished or waiting have their process stopped
  after about 1 hour and wake on attach. The timeout is not configurable.
- Pinning exists only as `Ctrl+T` inside agent view; there is no CLI for it.
- A crashed worker is restarted. After a reboot, sessions show `failed` if the reboot was
  less than 48 h ago, `stopped` otherwise.
- The built-in notifications (`agent_needs_input`, `agent_completed`) fire only while agent
  view is open. A front end has to poll and detect changes itself.

## Multiple config dirs (accounts)

Verified with two config dirs where `~/.claude-alt` symlinks `jobs/`, `sessions/`,
`projects/`, `settings.json`, `history.jsonl` and the rest to `~/.claude`, while `daemon/`,
`.credentials.json`, `.claude.json` and `state/` stay separate per account:

- Both commands list the same sessions (shared `jobs/`), but each account runs its own
  supervisor with its own socket directory.

What the **other** account's command does with a session hosted by `alt`:

| Command | Result |
|---|---|
| `claude logs <id>` | Fails with `job not found`. Only the hosting account's command works. |
| `claude stop <id>` | Succeeds. |
| `claude attach <id>` (session stopped) | Wakes it **under the attaching account**, same id; `/status` inside shows the attaching account. |
| `claude rm <id>` | Succeeds. |
| `claude --resume <sessionId> --bg "prompt"` (session stopped, run from its `cwd`) | Wakes it **under the running command's account**, in the background, same id. Prints the `note: woke session …` line; the prompt becomes the next turn; `logs` then works from that command. |

Consequence: waking with the wrong command silently switches the account. Account hand-off
is built on this ([0004](../0004-account-hand-off.md)).

`rm` while the hosting account's supervisor is not running (verified on the owner's server,
2026-10-20, Claude Code version not recorded; shared session list, each account with its own
transient supervisor):

- A session hosted by `alt` had no live process (no `pid`, no `status`) but still showed
  `state: blocked`: its process had ended without `claude stop`. `claude-alt daemon status`
  printed `not running` and `bg workers: 0 in roster.json`. `main`'s supervisor was running
  for a live session of its own.
- `claude-alt rm <id>` exited 1 with `couldn't remove <id> — the background service may be
  restarting. Try again in a moment.`, every time it was repeated.
- `claude rm <id>` (the other account's command) printed `removed <id>`, and the row was gone
  from both listings.

With one account (WSL, 2.1.303, 2026-10-20), a session stopped with `claude stop`
(`state: stopped`) and a stopped `done` session were removed with `removed <id>` while the
supervisor was not running. So a stopped supervisor alone does not cause the refusal; what
does is not known. Tower's fallback for it is
[0034](../0034-remove-with-another-account-when-the-service-is-down.md).

## Account detection via `/proc`

Verified:

- `/proc/<pid>/environ` of the JSON `pid` contains `CLAUDE_CONFIG_DIR=<dir>` for a session
  running under a non-default config dir. A missing variable means the default `~/.claude`.
- The parent of that pid is an intermediate host process, not the supervisor, so read the
  session process's own `environ`, not its parent's.
- Stopped sessions have no pid; their account must come from Watchtower's own records
  ([0003](../0003-accounts-and-session-ownership.md)).

## Worktrees and git

Documented ([agent view](https://code.claude.com/docs/en/agent-view)):

- A background session in a git repo moves into its own worktree under `.claude/worktrees/`
  before editing. When finished it commits, pushes its branch (never `main`/`master`) and
  opens a draft PR when the task calls for it.
- `worktree.bgIsolation: "none"` disables this. Outside git repos no worktree is used.
- `rm` from agent view deletes uncommitted work in the worktree; `claude rm` refuses instead.

## Permissions and trust

Documented ([permission modes](https://code.claude.com/docs/en/permission-modes)):

- When running as root, `bypassPermissions` is refused ("cannot be used with root/sudo
  privileges").
- Auto mode is available on all plans but not with Haiku 4.5 or older models; there the
  session silently starts in manual mode.
- Workspace trust is stored in `.claude.json` in the config dir, so it is **per account**.
- `claude --bg` in a folder the account has not trusted fails with `Workspace not trusted`
  when no trust dialog can be shown. Verified output: exit 1, empty stdout, stderr
  ``Workspace not trusted. Run `claude` in <dir> once and accept the trust prompt, then retry.``

Verified (observed behaviour; the mechanism is not documented):

- A folder inside a trusted parent folder was covered by the parent's trust until it became a
  git repository (`git init`); after that it was reported untrusted. Each git clone may
  therefore need its own trust acceptance, per account.
- In `manual` mode a read-only Bash command (`date`) ran without a prompt; a writing command
  (`touch <file>`) raised a permission prompt (`state: blocked`, `waitingFor: "permission prompt"`).

## Cross-session messaging

Documented ([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)):

- Sessions use the `ListAgents` and `SendMessage` tools. `notify_when_idle` sends one notice
  when another session next goes idle and expires after 12 h.
- A message can never approve a permission prompt.
- A session in `bypassPermissions` mode holds inbound messages for approval.
- There is no CLI way to reply to a session without attaching: `claude --resume <sessionId> "msg"`
  sends the message and then attaches. (Verified separately: a **stopped** background session
  takes a prompt without an attach through `--resume <sessionId> --bg "msg"`; a live one gets a
  copy instead, see [Sending a prompt to a background session](#sending-a-prompt-to-a-background-session).)

Verified with Claude Code 2.1.303, accounts `main` and `alt` whose config dirs share their
session lists (each supervisor hosts its own sessions):

- A session hosted by `main`'s supervisor (the orchestrator) can message a session hosted by
  `alt`'s supervisor with `SendMessage`, and the reply comes back to it. Messaging does not
  stop at the account boundary.
- Not checked separately: `notify_when_idle` across accounts.

## Skills and custom commands (the `/` menu)

Read 2026-10-28 from the docs listed above; used by the task field's suggestions
([0042](../0042-slash-command-suggestions.md)). Claude Code's own `/` menu also lists
built-in commands, bundled skills, MCP prompts and workflows, which tower does not offer.

**Where Claude Code looks** (documented, [skills](https://code.claude.com/docs/en/skills),
[.claude directory](https://code.claude.com/docs/en/claude-directory)):

| Source | Skills | Command files |
|---|---|---|
| Personal | `~/.claude/skills/<name>/SKILL.md` | `~/.claude/commands/*.md` ("project and global") |
| Project | `.claude/skills/<name>/SKILL.md` in the start folder and every parent up to the repository root | `.claude/commands/*.md`, also searched in parent folders |
| Nested | `<subdir>/.claude/skills/…` below the start folder, loaded only once Claude touches a file there | — |
| Plugin | `<plugin root>/skills/<name>/SKILL.md` | `<plugin root>/commands/*.md` |
| Enterprise | the managed settings folder's `.claude/skills/` | — |
| claude.ai | synced into `~/.claude/skills/synced/` (layout below) | — |

"If you set `CLAUDE_CONFIG_DIR`, every `~/.claude` path on this page lives under that
directory instead" ([.claude directory](https://code.claude.com/docs/en/claude-directory)):
an account's personal folders are under its config dir. Plugins shared through a
repository are not scanned from `.claude/plugins/`. A skill folder named `synced` (any
case) or `anthropic-skills` is reserved.

**Synced skills** (verified with Claude Code 2.1.312 in WSL, 2026-10-28; the layout is not
documented): `<config dir>/skills/synced/<bucket id>/<skill>/SKILL.md`, with a zero-byte
marker file `<config dir>/skills/synced/.bucket-<bucket id>` next to each bucket folder.
A bucket id looks like `<uuid>_<uuid>`; one bucket held 14 skills. Each `SKILL.md` has a
normal frontmatter: a plain `name` equal to the folder name, `description`, and fields
such as `compatibility` and `license`. Claude Code's interactive `/` menu offers them under
their **plain name** (`/<skill>`) with their description. When the plain name clashes with
a built-in command or workflow, the menu shows a namespaced form instead: a synced skill
whose name was also a built-in "[dynamic workflow]" appeared as
`/anthropic-skills:<skill>` (the namespace of that bucket; how it is derived for other
buckets is not known). Not verified: which entry keeps the plain name when a personal or
project skill has the same name as a synced one; tower assumes the personal or project entry
does (its order: personal and project entries first, then synced skills).

**Names** (documented):

- A skill is `/<folder name>`; its frontmatter `name` sets the name the `/` menu shows
  ("unless another command already uses that name"), and the folder name still invokes it.
- A command file is `/<file stem>`; a subfolder adds a segment: `.claude/commands/frontend/component.md`
  is `/frontend:component`. Command files take the skill frontmatter except `name` and `paths`.
- A plugin's entries are `/<plugin>:<name>`: `skills/review/SKILL.md` is `/my-plugin:review`,
  `name: fancy` makes it `/my-plugin:fancy` (it replaces the last segment);
  `commands/db/migrate.md` is `/my-plugin:db:migrate`. A plugin with a `SKILL.md` at its root,
  no `skills/` folder and no `skills` key is one skill, named by its `name`, else the plugin's
  folder name.
- The plugin prefix is the manifest's `name` (`.claude-plugin/plugin.json`); without a
  manifest, the marketplace entry name (the part of the listed `id` before `@`). A
  manifest name has no spaces, `@`, `:`, path separators, control or bidirectional
  characters.
- Names with `<` or `>`, zero-width spaces or control characters are refused and the
  entry does not load.

**Precedence** (documented): enterprise over personal over project; a skill over a file in
`commands/`; a plugin skill never clashes (namespaced); a project-root skill and a nested
one both load (`/deploy` is the root's). Not documented: which of a personal command and a
project skill with the same name wins, and which of a start folder's and a parent folder's
`.claude/skills` wins; tower takes a skill over a command first, then personal over
project, and the folder nearer the repository root, and puts claude.ai's synced skills
below all of them.

**Frontmatter** (documented): read only when the opening `---` is the file's first line.
All fields are optional. `description`, when missing, is "the first non-empty line of the
markdown content". `argument-hint` is "shown during autocomplete", e.g. `[issue-number]`.
`user-invocable: false` hides the skill from the `/` menu (Claude can still use it);
`disable-model-invocation: true` does not hide it. A field name must match exactly; unknown
fields are ignored. Malformed YAML: "Claude Code loads the skill body with empty metadata,
so `/skill-name` still works". The `skillOverrides` setting can also hide a skill (tower
does not read settings, so it does not see that).

**Symlinks** (documented): a `<skill-name>` entry in the personal or project folder can be
a symlink to a folder elsewhere. In plugins, a component path or symlink that resolves
outside the plugin's folder is rejected (`path escapes plugin directory`).

**Plugin layout and manifest** (documented,
[manifest](https://code.claude.com/docs/en/plugins-reference),
[components](https://code.claude.com/docs/en/plugins/components)):

- The manifest is optional, at `.claude-plugin/plugin.json`; `name` is its only required
  key. Unknown top-level keys are stripped.
- `skills`: a path or an array of paths (`./…`, or `.` for the root), each a folder of
  `<name>/SKILL.md` folders or one folder holding a `SKILL.md`; it **adds to** the default
  `skills/` scan.
- `commands`: a path, an array of paths (flat `.md` files or folders of them), or an object
  map of name to `{ source | content, description, argumentHint, model, allowedTools }`; it
  **replaces** the default `commands/` scan.
- Component paths start with `./`, must stay inside the plugin root and exist; on Linux a
  path with a backslash is rejected.
- A marketplace entry can add components to the manifest's (its `skills`, `commands`);
  tower does not read marketplace entries, so those are not offered.

**`claude plugin list --json`** (documented,
[CLI reference](https://code.claude.com/docs/en/plugins/cli-reference); the shape also seen
on the owner's server, sanitized below; in WSL with 2.1.312 and no plugin it prints `[]`):

```json
[
  {
    "id": "review-kit@example-market",
    "version": "1.0.0",
    "scope": "user",
    "enabled": true,
    "installPath": "/home/user/.claude/plugins/cache/example-market/review-kit/1.0.0",
    "installedAt": "2026-10-01T09:00:00.000Z",
    "lastUpdated": "2026-10-20T09:00:00.000Z",
    "projectEnabled": false
  }
]
```

- One object per installation. `id`, `version`, `scope`, `enabled` and `installPath` are
  always present. `id` is `name@marketplace`, `name@inline` (session-only),
  `name@skills-dir` (a plugin folder under a `skills` folder) or `name@synced`.
- `scope`: `user`, `project`, `local` or `managed`; `user` or `project` for skills-folder
  plugins; `session`; `synced`. `projectPath` (project and local scope) names the project
  the install belongs to.
- `enabled`: "whether the plugin is enabled in your merged settings", which include the
  current folder's project settings, so tower runs the listing in the project folder.
  `projectEnabled` (2.1.285+) only says whether the project's shared
  `.claude/settings.json` turns it on; `enabled` already merges it with the local and
  managed settings.
- `installPath`: the folder the plugin loads from, except for a plugin loaded in place from
  its marketplace's folder, which has `readFromFolder` (2.1.289+) instead.
- Other fields (`errors`, `notes`, `mcpServers`, `hasUserConfig`, …) appear when they apply.
  With `--available` the output is an object whose `installed` field holds the array.
- `claude plugin details <name>` prints a component inventory as text; tower does not parse
  it.

Not documented, chosen conservatively by tower: how deep subfolders of `commands/` may go
(tower: 3 levels), whether a plugin with `errors` still offers its commands (tower: it
keeps them).

## Subagents in background sessions

Verified with Claude Code 2.1.316 (2026-10-31): a subagent's foreground shell command still
running after about 3 minutes moves to the background. The subagent then fires `SubagentStop`,
and the parent is told `Agent "<description>" is waiting on its own background work`, while the
command keeps running (the parent's footer says `N shells still running`). So hooks can't tell
this from a finished subagent, and tower's `+N` drops it even though its command is still going.

Verified with Claude Code 2.1.303:

- A background session has the `Agent` tool and can launch subagents.
- Subagents run inside the parent session's process. They are **not** separate rows in
  `claude agents --json` or `claude agents --json --all`.
- While a subagent runs, the parent row stays `state: working` / `status: busy`, and its
  screen shows "Waiting for 1 background agent to finish". The parent turns `done` /
  `idle` only after it has relayed the subagent's result.
- Attached, the subagents are listed under "↓ to manage" and in `/tasks`.
- A finished subagent can be continued with the parent's `SendMessage` tool; it runs
  again under the same agent id (verified 2.1.306, see
  [Hooks passed with `--settings`](#hooks-passed-with---settings)).

Not verified: what a subagent's permission prompt looks like in the listing.

How Watchtower uses it: [0029](../0029-tracking-subagents.md),
[0035](../0035-continued-subagents.md),
[0039](../0039-subagent-hooks-for-resumed-conversations.md).

## Background shells and the turn

Verified with Claude Code 2.1.303:

- A session that starts a background shell and ends its turn in the same turn stays
  `state: working` / `status: busy` while the shell runs.
- When the shell finishes, Claude Code notifies the session, which starts a new turn by
  itself (no prompt from the user) and replies; then it goes `done` / `idle`.
- A session can also be listed `state: done` with `status: busy`: the turn has finished,
  but a background task (a shell) outlived it, and the session's footer shows `1 shell`.
  That the task's completion wakes this session for a new turn too is expected from the
  case above but was not observed directly.

How Watchtower uses it: [0032](../0032-background-task-after-a-finished-turn.md).

## Hooks passed with `--settings`

Verified with Claude Code 2.1.303 (the hook payloads are documented in
https://code.claude.com/docs/en/hooks):

- `claude --bg --settings <file> "task"` is accepted, and hooks from that file fire in
  the background session.
- Hooks from `--settings` are **combined** with hooks from settings files: a project
  `.claude/settings.json` hook and a `--settings` hook both fired for the same events.
- A wake keeps them: after `claude stop`, `--resume <sessionId> --bg "…"` printed
  `note: woke session <id> with its saved options (--name, --settings, --model).` and the
  hooks fired again.
- `SubagentStart` payload: `session_id` (the `sessionId` of `claude agents --json`),
  `prompt_id`, `agent_id` (e.g. `a1b2c3d4e5f607182`), `agent_type` (e.g.
  `general-purpose`), `hook_event_name`, plus the common fields (`transcript_path`,
  `cwd`, `scratchpad_dir`, …).
- `PostToolUse` with matcher `Agent`, for a background launch, fires at launch with
  `tool_input.description`, `tool_input.subagent_type`, and `tool_response`
  `{ "status": "async_launched", "agentId": …, "description": …, … }`.
- `SubagentStop` payload: `agent_id`, `agent_type`, `stop_hook_active`,
  `last_assistant_message`, `agent_transcript_path`, and `background_tasks`, a snapshot
  such as `[{ "id": "a1b2c3d4e5f607182", "type": "subagent", "status": "running",
  "description": "alpha waiter", "agent_type": "general-purpose" }, …]` that still lists
  the agent that is stopping.
- Claude Code's internal helpers also emit `SubagentStop`, with `agent_type: ""` and no
  `SubagentStart` before it.
- A finished subagent continued with `SendMessage` (verified 2.1.306, WSL): the main
  thread launched one `general-purpose` agent in the background, it finished, and
  the main thread sent it a message. The hooks fired in this order:
  1. `SubagentStart` `{agent_id: A, agent_type: "general-purpose", prompt_id: P1}`.
  2. `PostToolUse` `tool_name: "Agent"`, `tool_input {description, subagent_type,
     prompt}`, `tool_response {isAsync: true, status: "async_launched", agentId: A,
     description, resolvedModel, prompt}`.
  3. `SubagentStop` `{agent_id: A, agent_type: "general-purpose", stop_hook_active:
     false, background_tasks: [{id: A, type: "subagent", status: "running",
     description}], session_crons: []}`: the stopping agent is listed as `running`.
  4. `PostToolUse` `tool_name: "SendMessage"`, `tool_input {to: A, recipient: A,
     recipient_kind: "agent", type: "message", summary, message, content}`
     (`prompt_id` P2).
  5. `SubagentStart` `{agent_id: A, agent_type: "general-purpose", prompt_id: P2}`: the
     continued run fires a start again with the **same** `agent_id`.
  6. `SubagentStop` `{agent_id: H, agent_type: "", last_assistant_message,
     background_tasks: [{id: A, status: "running", description, …}]}`: a helper,
     never started, whose snapshot lists A.
  7. `SubagentStop` of A when the continued run ended, its `background_tasks` again
     listing A as `running`.

  The `background_tasks` entries of this run had no `agent_type` key.
- Hooks run concurrently: two subagents launched together fire their hooks at the same
  time.
- While subagents run, the parent row stays `state: working` / `status: busy`; it turns
  `done` only after relaying their results. The JSON row still has only `pid`, `id`, `cwd`,
  `kind`, `startedAt`, `sessionId`, `name`, `status`, `state`.
- `--settings` given to the interactive picker reaches the backgrounded copy (verified
  2.1.306, WSL, clean environment in a private tmux server):
  `claude --settings <file> --resume`, an ended conversation picked, `←` on the empty
  prompt (agent view opens), `Ctrl+C` twice. The copy is listed under a new `sessionId`,
  and its process's `/proc/<pid>/cmdline` is `<claude binary> --session-id <copy
  sessionId> --fork-session --resume <transcript path> --settings <file> --model <model>
  --permission-mode <mode>`. Attached with `claude attach <copy id>` and asked for one
  background `general-purpose` subagent, the copy fired, all with `session_id` = the
  copy's `sessionId`:
  1. `SubagentStart` `{agent_id: A, agent_type: "general-purpose"}`.
  2. `PostToolUse` `tool_name: "Agent"`, `tool_response {status: "async_launched",
     agentId: A}`.
  3. Two `SubagentStop` with `agent_type: ""` (helpers, ids never started), each with
     `background_tasks` listing A as `running`.
  4. `SubagentStop` of A when it ended (about 30 s later), `background_tasks` listing A as
     `running`.

  `tower hook --dir <dir>` from that file wrote `<dir>/subagents/<copy sessionId>.json`
  with A `running`, `background: true`, description from the launch, then moved A to
  `stopped`. Leaving the attach with `←` then `Esc` returned to the shell as in
  [Attach and detach keys](#attach-and-detach-keys). Only the order `--settings <file>
  --resume` was tried.

## Plan usage limits

Verified with Claude Code 2.1.312 on WSL (Linux), 2026-10-28, one account on a
subscription; how Watchtower uses it: [0043](../0043-plan-usage-display.md).

- **No subcommand.** There is no `claude usage` or `claude status`. `claude auth status
  --json` has `subscriptionType` but no usage; `claude agents --json` has no usage fields.
  (CLI help, verified)
- **`/usage` in print mode is a local command.** `<command> -p --output-format stream-json
  --verbose --no-session-persistence --safe-mode "/usage"` (in this order, the prompt
  last) exits 0 and makes no model turn: the `result` line has `num_turns: 0`,
  `duration_api_ms: 0`, `total_cost_usd: 0` and `local_command: "usage"`; the
  `assistant` line has `message.model: "<synthetic>"`, zero tokens and
  `local_command_run: {"command": "usage", "args": ""}`. (verified)
- **Output: three JSON lines.** `system` (`subtype: init`, with `claude_code_version`,
  `cwd`, `session_id`, …), `assistant`, `result`. The `assistant` line's
  `message.content[0].text` and the `result` line's `result` hold `/usage`'s text:
  `Current session: N% used · resets <Mon D>, <h[:mm]am|pm> (<time zone>)`, `Current week
  (all models): …`, `Current week (<model>): …`, then a breakdown of what contributed.
  `--output-format json` and plain text give only that text. (verified)
- **`usage_report`** on the `assistant` line, documented as experimental (the Agent SDK's
  `SDKUsageReport`), made-up values:

  ```json
  {
    "session": { "total_cost_usd": 0, "total_api_duration_ms": 0, "total_duration_ms": 700,
                 "total_lines_added": 0, "total_lines_removed": 0, "model_usage": {} },
    "rate_limits": {
      "limits": [
        { "kind": "session", "group": "session", "percent": 24,
          "resets_at": "2026-10-28T18:00:00.177181+00:00", "scope": null,
          "severity": "normal", "is_active": true },
        { "kind": "weekly_all", "group": "weekly", "percent": 12,
          "resets_at": "2026-11-03T08:00:00.177201+00:00", "scope": null,
          "severity": "normal", "is_active": false },
        { "kind": "weekly_scoped", "group": "weekly", "percent": 3,
          "resets_at": "2026-11-03T08:00:00+00:00",
          "scope": { "model": { "display_name": "Fable" }, "surface": null },
          "severity": "normal", "is_active": false }
      ],
      "extra_usage": { "is_enabled": false, "monthly_limit": null, "used_credits": null,
                       "utilization": null, "currency": null }
    }
  }
  ```

  Only `severity: "normal"` was observed. Sanitized samples: `tests/fixtures/usage/`.
  (verified; experimental per the Agent SDK docs)
- **No plan data.** Logged out, and per the docs with an API key, a setup token or usage
  billing: exit 0 with `usage_report: null`. (research, documented)
- **Per account.** The running command's config dir decides: two config dirs that share
  `jobs/` through symlinks still report their own usage (separate credentials).
  (verified)
- **`--safe-mode`** gives the same data without loading the user's hooks and MCP servers.
  `--no-session-persistence` saves no transcript. (CLI help, verified)
- **`--session-id <uuid>`** works with `-p`: every line carries that `session_id`. (verified)
- **Its own row.** While it runs (about one second) the probe is listed by `claude agents
  --json` as `kind: interactive` with `pid`, `sessionId`, `status` (`busy`, then `idle`),
  `cwd` = the folder it runs in and `name` `<folder name>-<2 hex>` (e.g. `usage-4f`); then
  the row is gone. `claude agents --json --all` printed the same bytes before and after.
  (verified)
- **Cost.** About 1.5 to 1.9 s and a peak near 280 MB per call. (verified)
- **Rate limited itself.** Claude Code's usage endpoint is rate limited: `/usage` may say
  "Showing last-known usage" (a snapshot up to 60 minutes old) or "rate limited until
  <time>" (documented, https://code.claude.com/docs/en/costs; the exact wording was not
  captured).
- **Status line and hooks.** The status line's JSON has `rate_limits`, but only a status
  line command sees it, which would replace the user's
  ([0016](../0016-do-not-change-claude-code-defaults.md)); no hook carries usage.
  (documented)

## Known messages and exit codes

| Situation | Output | Exit code | Source |
|---|---|---|---|
| Unknown id for `attach`, `logs` or `stop` (or a name, full sessionId or pid for `stop`) | `No job matching '<x>'. Run 'claude agents' to list running sessions.` | 1 | verified |
| `rm` success | `removed <id>` | 0 | verified |
| `rm` on a row with no live process while the hosting account's supervisor is not running | `couldn't remove <id> — the background service may be restarting. Try again in a moment.` (stream not recorded) | 1 | verified (owner's server, 2026-10-20, version not recorded) |
| `stop` success, also on an already stopped session | `stopped <id>` | 0 | verified |
| `respawn <id>` success, also on a stopped session, which it wakes | `respawned <id>` on stdout | 0 | verified (2.1.316) |
| `respawn` with an unknown id or a name | stderr `No job matching '<x>'` | 1 | verified (2.1.316) |
| `attach` when its session is restarted | `Session <id> has exited.` and a `claude --resume <sessionId>` line | 0 | verified (2.1.316) |
| `attach` after detach | none | 0 | verified |
| `attach` while agent view is disabled | `'attach' is disabled by …` | 1 | verified |
| `--bg` success | stdout `backgrounded · <id> · <name>` plus hint lines; stderr `Starting background service…` when the supervisor starts | 0 | verified |
| `--bg` with an unknown `--model` name | as `--bg` success; the session then turns `failed` | 0 | verified |
| `--bg` in an untrusted folder | stderr ``Workspace not trusted. Run `claude` in <dir> once and accept the trust prompt, then retry.``; empty stdout | 1 | verified |
| `--resume <sessionId> --bg` without a prompt (stopped session) | stdout `backgrounded · <id> · <name> (idle — send a prompt to start)`; stderr `note: woke session <id> with its saved options (…).` | 0 | verified |
| `--resume <sessionId> --bg "text"` (session still running) | stdout `backgrounded · <copy id>` plus hint lines; stderr ``note: session <id> is already running in the background, so this started a copy as <copy id>. `claude attach <id>` opens the original.`` | 0 | verified (2.1.306) |
| `--bg [--name <name>] -- "<task>"` (task starting with `-`) | stdout `backgrounded · <id> · <name>`; the task is the prompt | 0 | verified (2.1.312) |
| `--resume <sessionId> --bg -- "<text>"` (stopped session; text may start with `-`, may span lines) | stdout `backgrounded · <id> · <name>`; stderr `note: woke session <id> with its saved options (--name, --model).` | 0 | verified (2.1.306) |
| `--resume <sessionId> --bg "/rename X"` (stopped session) | stdout `backgrounded · <id> · <old name>`; stderr `note: woke session <id> with its saved options (--name, --model).` | not captured | verified (2.1.303) |
| `--resume <sessionId> --bg --name X` (stopped session) | stdout `backgrounded · <copy id> · X (idle — …)`; stderr `note: background session <id> keeps its own saved options, so the flags you passed started a copy as <copy id>. …` | not captured | verified (2.1.303) |
| `/rename <name>` inside a session | `⎿  Session renamed to: <name>` | — | verified (2.1.303) |
| `logs` on a stopped session | stderr `Couldn't read logs for <id> — job not found — it may have already exited`; empty stdout | 1 | verified |
| `logs` with another account's command | `job not found` | not captured | verified (output) |

## Not yet verified

- `claude respawn --all` (deliberately not run), `respawn <id>` with another account's command,
  `respawn` of a session with a running background shell, and `respawn` when the installed binary
  really is newer ([Restarting a session](#restarting-a-session-claude-respawn)).
- Whether a live session upgrades by itself once idle, as `claude daemon status` says.
- Whether managed skills and workflows show in the `/` menu the way the docs suggest;
  tower does not offer them ([0042](../0042-slash-command-suggestions.md)). (claude.ai's
  synced skills: verified, see [Skills and custom commands](#skills-and-custom-commands-the--menu).)
- How Claude Code derives a synced bucket's namespace (seen: `anthropic-skills`).
- How long `claude plugin list --json` takes with many plugins, and whether it ever prints
  something before the JSON on stdout.

- Whether Claude Code limits the length or characters of a session name set with `/rename`.

- Exact format of the `rm` worktree refusal message and its exit code.
- What makes `rm` refuse with `the background service may be restarting` for one row
  without a live process but not for another, and on which stream it prints.
- What a subagent's permission prompt looks like in `claude agents --json` (see
  [Subagents in background sessions](#subagents-in-background-sessions)).
- Whether `claude --bg` enforces the 4-character minimum prompt documented for agent view.
- `usage_report` `severity` values other than `normal`, the exact "rate limited until …"
  wording, and the logged-out `usage_report: null` on this version
  ([Plan usage limits](#plan-usage-limits)).
- Whether `--effort` survives a wake with `--resume <sessionId> --bg` (the `note:` line lists
  `--name`, `--permission-mode`, `--settings` and `--model` only).
- Whether attaching with the hosting account after the 1-hour idle stop keeps the account, and
  how such a row looks in the JSON (probably like a row after `claude stop`).
- Whether `claude agents --json` starts a supervisor for an account with no live sessions.
- Whether `notify_when_idle` reaches a session hosted by another account's supervisor
  (`SendMessage` does, see [Cross-session messaging](#cross-session-messaging)).
- Whether background workers refresh an expiring login token themselves.
- Whether `/bg` in a conversation opened from the `claude --resume` picker behaves like `←`
  ([0019](../0019-resume-past-sessions.md)); whether `Ctrl+Z` suspends a plain interactive
  session.
- Effect of `leftArrowOpensAgents=false` (deliberately not tested, see
  [0016](../0016-do-not-change-claude-code-defaults.md)).
- JSON fixtures for the values not yet observed: `state: stopped` and `waitingFor` values
  `sandbox request`, `worker request`, `dialog open`.
