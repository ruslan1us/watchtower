# Claude Code CLI behaviour Watchtower relies on

Verified against Claude Code 2.1.302 on 2026-10-18; re-verify after upgrades.

This page collects the facts about Claude Code that Watchtower's design depends on. Each
fact is marked:

- **documented**: stated in the Claude Code docs (URL given).
- **CLI help**: stated in `claude --help` or a subcommand's `--help`.
- **verified**: observed in tests on Linux, including the tmux attach/detach harness
  ([0015](../0015-testing-strategy.md)).

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
| `claude --bg [--name N] [--model M] [--permission-mode P] [--effort E] [--agent A] "task"` | Starts a background session in the current directory (the session's `cwd`). Prints `backgrounded · <id> · <name>`, preceded by `Starting background service…` when the supervisor has to start first. | CLI help, verified |
| `claude --resume <sessionId> --bg ["prompt"]` | Continues a stopped session in the background under the **same id**. Prints e.g. `note: woke session 1a2b3c4d with its saved options (--name, --model).`; the prompt becomes the next turn. If the session is already running, starts a copy under a new id and prints a `note:` line. | CLI help, verified (stopped case) |
| `claude agents --json [--all] [--cwd <path>]` | Lists active sessions, interactive and background. `--all` adds completed background sessions; `--cwd` filters by directory. Needs no TTY. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude attach <id\|name>` | Attaches the terminal to a background session. Part of a name is accepted. Wakes a stopped session. | documented ([agent view](https://code.claude.com/docs/en/agent-view)), verified |
| `claude logs <id\|name>` | "Recent terminal output" of the session. **No line-limit flag.** Format below. | CLI help, verified |
| `claude stop <id>` / `claude kill <id>` | Stops the session's process; the conversation is kept. **Short id only**: a full `sessionId` or a pid gives `No job matching '<x>'`. | CLI help, verified |
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
| `state` | always on background rows | `working`, `blocked`, `done`, `failed`, `stopped` |
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
```

Observed oddities (verified):

- Interactive sessions are listed but have no `id` and no `state`; they cannot be attached.
- `state: "blocked"` with `status: "idle"` and no `waitingFor` occurs, e.g. when a session
  failed with `Login expired · Please run /login`.
- The short `id` was observed to equal the first 8 characters of `sessionId`. This is
  undocumented; do not rely on it.
- `startedAt` is reset when a session is woken; it is not the session's creation time.

## `claude logs` output

Verified:

- Output is the **raw terminal byte stream** of the session's recent output, not text lines.
- It starts with a clear-screen and full redraw (`ESC 7 ESC 8 ESC[2J ESC[H`), followed by
  incremental updates: absolute and relative cursor moves (`ESC[64;1H`, `ESC[57B`), 24-bit
  colour SGR (`ESC[38;2;r;g;bm`) and every spinner frame.
- Printed to a terminal it looks like a screenshot of the current screen. Plain ANSI-to-text
  conversion does not work because of cursor addressing; it needs a terminal emulator.
- The PTY size is not reported. In one test the stream addressed row 64 and the screen was
  about 200 columns wide.
- A `Session <id> · <name>` banner is written to a different stream than the screen bytes
  (first in a pipe, last in a terminal), probably stderr.
- `logs` works only through the binary of the account hosting the session (see below).

How Watchtower uses it: [0009](../0009-stop-remove-and-logs.md).

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

- There is no keybinding action for detaching; `←` and `Ctrl+Z` are not rebindable. The
  `Agents` keybinding context has only `switchView`, `togglePin`, `find`, `rename`,
  `previousGroup`, `nextGroup` (documented, [keybindings](https://code.claude.com/docs/en/keybindings)).
- `leftArrowOpensAgents=false` exists only in the global `~/.claude.json`, cannot be passed
  with `--settings`, and is documented for foreground sessions only. Untested; Watchtower
  does not change it ([0016](../0016-do-not-change-claude-code-defaults.md)).

How Watchtower uses it: [0005](../0005-attach-and-detach.md).

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
  when no trust dialog can be shown (documented; exact output not yet captured).

## Cross-session messaging

Documented ([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)):

- Sessions use the `ListAgents` and `SendMessage` tools. `notify_when_idle` sends one notice
  when another session next goes idle and expires after 12 h.
- A message can never approve a permission prompt.
- A session in `bypassPermissions` mode holds inbound messages for approval.
- There is no CLI way to reply to a session without attaching: `claude --resume <sessionId> "msg"`
  sends the message and then attaches.

## Known messages and exit codes

| Situation | Output | Exit code | Source |
|---|---|---|---|
| Unknown id for `attach` or `stop` (or full sessionId / pid for `stop`) | `No job matching '<x>'. Run 'claude agents' to list running sessions.` | 1 | verified |
| `rm` success | `removed <id>` | 0 | verified |
| `stop` success | `stopped <id>` | not captured | verified (output) |
| `attach` after detach | none | 0 | verified |
| `attach` while agent view is disabled | `'attach' is disabled by …` | 1 | verified |
| `--bg` success | `backgrounded · <id> · <name>` | not captured | verified (output) |
| `logs` with another account's command | `job not found` | not captured | verified (output) |

## Not yet verified

- Exact stderr text and exit codes of `--bg` failures (untrusted folder, unknown model name).
- Exact format of the `rm` refusal message and its exit code.
- Exit codes of `logs`, and its behaviour for a stopped session.
- How to learn the PTY size of a session for replaying `logs`.
- Whether `claude --bg` enforces the 4-character minimum prompt documented for agent view.
- Whether `--resume <sessionId> --bg` works without a prompt, and whether the permission mode
  is kept (the `note:` line mentions only `--name` and `--model`).
- Whether attaching with the hosting account after the 1-hour idle stop keeps the account.
- Whether `ListAgents` / `SendMessage` reach sessions hosted by another account's
  supervisor.
- Whether background workers refresh an expiring login token themselves.
- Effect of `leftArrowOpensAgents=false` (deliberately not tested, see
  [0016](../0016-do-not-change-claude-code-defaults.md)).
- JSON fixtures covering every `state` and `waitingFor` value.
