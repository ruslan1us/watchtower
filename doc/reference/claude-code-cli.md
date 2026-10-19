# Claude Code CLI behaviour Watchtower relies on

Verified against Claude Code 2.1.302 on 2026-10-18; facts marked 2.1.303 were verified
with that version (renaming and hooks passed with `--settings` on 2026-10-19). Re-verify after upgrades.

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

Example accounts used throughout: `main` (command `claude`, config dir `~/.claude`) and
`alt` (command `claude-alt`, a wrapper that sets `CLAUDE_CONFIG_DIR=~/.claude-alt`).

## CLI commands

| Command | Behaviour | Source |
|---|---|---|
| `claude --bg [--name N] [--model M] [--permission-mode P] [--effort E] [--agent A] [--settings F] "task"` | Starts a background session in the current directory (the session's `cwd`). Prints `backgrounded · <id> · <name>` and hint lines on **stdout**; `Starting background service…`, printed first when the supervisor has to start, goes to **stderr**. In an untrusted folder it fails (exit 1, see [Permissions and trust](#permissions-and-trust)). An unknown `--model` name is **not** rejected: exit 0 and the normal `backgrounded` line, then the session ends in `state: failed`. | CLI help, verified |
| `claude --resume <sessionId> --bg ["prompt"]` | Continues a stopped session in the background under the **same id**. Prints on stderr e.g. `note: woke session 1a2b3c4d with its saved options (--name, --permission-mode, --model).`; the saved options are the flags given at the original dispatch (without `--permission-mode` there, the note lists only `(--name, --model)`; with `--settings` there, it lists `--settings` too and the hooks keep firing, 2.1.303). With a prompt, the prompt becomes the next turn. Without a prompt it exits 0, prints `backgrounded · <id> · <name> (idle — send a prompt to start)` on stdout, and the session sits idle and does not continue by itself; a permission prompt pending before the stop is not shown again. If the session is already running, starts a copy under a new id and prints a `note:` line. On a stopped session, flags beyond `--bg` (e.g. `--name X`) or the short id in place of the full `sessionId` also start a copy instead of waking it; a prompt such as `"/rename X"` does not ([Renaming a session](#renaming-a-session), 2.1.303). | CLI help, verified (stopped case) |
| `claude --resume` (no argument) | Opens the interactive session picker for the current folder (its worktree; background sessions marked `bg`). In the picker `Ctrl+A` shows every project on the machine, `Ctrl+W` every worktree of the repository, `Esc` leaves. Picking a conversation of a background session that is still running attaches to it (v2.1.285+); picking one of an unrelated project copies a `cd` and resume command to the clipboard instead of resuming. A conversation picked here starts in the permission mode a new session would, except plan mode. Picking an ended conversation opens it in the foreground; backgrounding it then starts a copy under a new session id (see [Attach and detach keys](#attach-and-detach-keys)). Used by `R` ([0019](../0019-resume-past-sessions.md)). | documented ([sessions](https://code.claude.com/docs/en/sessions)); ended case verified (2.1.303) |
| `claude agents --json [--all] [--cwd <path>]` | Lists active sessions, interactive and background. `--all` adds completed background sessions, including sessions stopped with `claude stop`, which are missing without it; `--cwd` filters by directory. Needs no TTY. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude attach <id\|name>` | Attaches the terminal to a background session. Part of a name is accepted. Wakes a stopped session. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude logs <id\|name>` | "Recent terminal output" of the session. Part of a name is accepted. **No line-limit flag.** Format below. Fails (exit 1, empty stdout) for a stopped session, since there is no live process. | CLI help, verified |
| `claude stop <id>` / `claude kill <id>` | Stops the session's process; the conversation is kept. **Short id only**: a name, a full `sessionId` or a pid gives `No job matching '<x>'`. Idempotent: on an already stopped session it prints `stopped <id>` and exits 0. | CLI help, verified |
| `claude rm <id> [--discard-unpushed <commit>@<worktree>] [--force-remove-worktree <worktree>]` | Removes the row and, when safe, the session's worktree. The transcript stays on disk. Works on exited sessions. Refuses on uncommitted changes, unpushed commits or a worktree in use, and prints the flag to pass for a second try. | CLI help, verified (success case) |
| `claude respawn <id>\|--all` | Restarts sessions onto the currently installed binary. | CLI help |
| `claude daemon status\|logs\|stop [--any] [--keep-workers]` | Supervisor information (pid, version, socket dir, worker counts), its log, stopping it. | CLI help |

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
| Failed (e.g. unknown `--model` name); the process stays alive, `pid` present | `failed` | `idle` | — |
| Woken with `--resume <sessionId> --bg` and no prompt, right after the wake | `working` | `idle` | — |
| Stopped with `claude stop` (only with `--all`) | last value kept (observed: `done`) | absent | — |

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
  sends the message and then attaches.

## Subagents in background sessions

Verified with Claude Code 2.1.303:

- A background session has the `Agent` tool and can launch subagents.
- Subagents run inside the parent session's process. They are **not** separate rows in
  `claude agents --json` or `claude agents --json --all`.
- While a subagent runs, the parent row stays `state: working` / `status: busy`, and its
  screen shows "Waiting for 1 background agent to finish". The parent turns `done` /
  `idle` only after it has relayed the subagent's result.
- Attached, the subagents are listed under "↓ to manage" and in `/tasks`.

Not verified: what a subagent's permission prompt looks like in the listing.

How Watchtower uses it: [0029](../0029-tracking-subagents.md).

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
- Hooks run concurrently: two subagents launched together fire their hooks at the same
  time.
- While subagents run, the parent row stays `state: working` / `status: busy`; it turns
  `done` only after relaying their results. The JSON row still has only `pid`, `id`, `cwd`,
  `kind`, `startedAt`, `sessionId`, `name`, `status`, `state`.

Not verified: whether `--settings` given to the interactive `claude --resume` reaches the
copy a backgrounded conversation starts as.

## Known messages and exit codes

| Situation | Output | Exit code | Source |
|---|---|---|---|
| Unknown id for `attach`, `logs` or `stop` (or a name, full sessionId or pid for `stop`) | `No job matching '<x>'. Run 'claude agents' to list running sessions.` | 1 | verified |
| `rm` success | `removed <id>` | 0 | verified |
| `stop` success, also on an already stopped session | `stopped <id>` | 0 | verified |
| `attach` after detach | none | 0 | verified |
| `attach` while agent view is disabled | `'attach' is disabled by …` | 1 | verified |
| `--bg` success | stdout `backgrounded · <id> · <name>` plus hint lines; stderr `Starting background service…` when the supervisor starts | 0 | verified |
| `--bg` with an unknown `--model` name | as `--bg` success; the session then turns `failed` | 0 | verified |
| `--bg` in an untrusted folder | stderr ``Workspace not trusted. Run `claude` in <dir> once and accept the trust prompt, then retry.``; empty stdout | 1 | verified |
| `--resume <sessionId> --bg` without a prompt (stopped session) | stdout `backgrounded · <id> · <name> (idle — send a prompt to start)`; stderr `note: woke session <id> with its saved options (…).` | 0 | verified |
| `--resume <sessionId> --bg "/rename X"` (stopped session) | stdout `backgrounded · <id> · <old name>`; stderr `note: woke session <id> with its saved options (--name, --model).` | not captured | verified (2.1.303) |
| `--resume <sessionId> --bg --name X` (stopped session) | stdout `backgrounded · <copy id> · X (idle — …)`; stderr `note: background session <id> keeps its own saved options, so the flags you passed started a copy as <copy id>. …` | not captured | verified (2.1.303) |
| `/rename <name>` inside a session | `⎿  Session renamed to: <name>` | — | verified (2.1.303) |
| `logs` on a stopped session | stderr `Couldn't read logs for <id> — job not found — it may have already exited`; empty stdout | 1 | verified |
| `logs` with another account's command | `job not found` | not captured | verified (output) |

## Not yet verified

- Whether Claude Code limits the length or characters of a session name set with `/rename`.

- Exact format of the `rm` refusal message and its exit code.
- What a subagent's permission prompt looks like in `claude agents --json` (see
  [Subagents in background sessions](#subagents-in-background-sessions)).
- Whether `claude --bg` enforces the 4-character minimum prompt documented for agent view.
- Whether `--effort` survives a wake with `--resume <sessionId> --bg` (the `note:` line lists
  `--name`, `--permission-mode`, `--settings` and `--model` only).
- Whether `--settings` on the interactive `claude --resume` reaches the backgrounded copy
  ([Hooks passed with `--settings`](#hooks-passed-with---settings)).
- Whether attaching with the hosting account after the 1-hour idle stop keeps the account, and
  how such a row looks in the JSON (probably like a row after `claude stop`).
- Whether `claude agents --json` starts a supervisor for an account with no live sessions.
- Whether `ListAgents` / `SendMessage` reach sessions hosted by another account's
  supervisor.
- Whether background workers refresh an expiring login token themselves.
- Whether `/bg` in a conversation opened from the `claude --resume` picker behaves like `←`
  ([0019](../0019-resume-past-sessions.md)); whether `Ctrl+Z` suspends a plain interactive
  session.
- Effect of `leftArrowOpensAgents=false` (deliberately not tested, see
  [0016](../0016-do-not-change-claude-code-defaults.md)).
- JSON fixtures for the values not yet observed: `state: stopped` and `waitingFor` values
  `sandbox request`, `worker request`, `dialog open`.
