# Test fixtures

Sanitized captures of real Claude Code CLI output, used by the unit tests
described in [0015](../../doc/0015-testing-strategy.md). Every file here was
sanitized by the rules in [0013](../../doc/0013-privacy-and-repository-hygiene.md).
Nothing in this tree identifies a person, machine or account.

## Capture

- Claude Code **2.1.302** on Linux, captured 2026-10-18.
- Throwaway background sessions started with `claude --bg --name <name> "<task>"`
  in one scratch folder, plus one interactive session that happened to be
  running at the same time.
- `claude agents --json` (and `--all`) stdout was saved as is. `claude logs <id>`
  stdout was saved as raw bytes. Other commands had stdout and stderr saved to
  separate files and the exit code recorded.
- `stop.stdout`, `stop-unknown.stderr` and `rm.stdout` were noted from the
  console rather than redirected to files. They are written out here with the
  CLI's exact wording (see the exit-code table).

## Layout

The directory names follow [0015](../../doc/0015-testing-strategy.md):
`agents-json/`, `logs/`, `cli-output/`.

### `agents-json/` — `claude agents --json`

The JSON is byte-for-byte the CLI's formatting: 2-space indent, a trailing
newline, fields in the order the CLI prints them (`pid`, `id`, `cwd`, `kind`,
`startedAt`, `sessionId`, `name`, `status`, `waitingFor`, `state`). Only
values were changed. No rows or fields were added or removed.

| File | Captured as | What it shows |
|---|---|---|
| `mixed-states.json` | `claude agents --json` | An `interactive` row with no `id` and no `state`. `background` rows: `failed`/`idle` (unknown model, `bad-model`), `working`/`busy` (`run-test-suite`), `done`/`idle` (`print-date`, `say-ok`), `blocked`/`waiting` with `waitingFor: "input needed"` (`pick-colour`, a pending multiple-choice question). `--all` printed the same bytes at that moment, because no session was stopped yet. |
| `permission-prompt.json` | `claude agents --json`, later | `blocked`/`waiting` with `waitingFor: "permission prompt"` (`touch-config`, waiting on a Bash permission). `run-test-suite` has moved from `working`/`busy` to `done`/`idle`. `--all` also printed the same bytes. |
| `after-stop.json` | `claude agents --json` after `claude stop` on `say-ok` | The stopped session is gone from the default (active) list. |
| `after-stop-all.json` | `claude agents --json --all`, same moment | The stopped session is still listed, in its `startedAt` position, with `state: "done"` and **no `pid` and no `status`**. Its `startedAt` is 582 ms earlier than in the live row (`1767232821923` against `1767232822505`). So `startedAt` is not a stable key; match rows by `sessionId`. |

### `logs/` — `claude logs <id>` stdout

`<name>.ansi` is the raw terminal byte stream: it saves and restores the
cursor, clears the screen, then uses cursor addressing (CUU/CUD/CUF/CHA/CUP),
erase-line and 24-bit SGR colour. The stream is written for a 200-column
terminal; the bottom bar is drawn on row 50. `claude logs` printed a
`Session <id> · <name>` line on stderr (see `cli-output/logs-done.stderr`).

`<name>.screen.txt` is the expected reconstructed screen: the `.ansi` file
played into a 200×64 tmux pane (`tmux -L <socket> -f /dev/null new-session -d
-x 200 -y 64 "cat <file>; sleep 30"`, then `capture-pane -p`), with trailing
spaces removed from each line. Rows that end up blank are kept, so line N is
screen row N.

| File | Session | What it shows |
|---|---|---|
| `done.ansi` | `say-ok`, `done`/`idle` | Finished turn ("OK"), a `✻ … · done <time>` line, the prompt box with the session name in the top rule, footer `⏵⏵ auto mode on … · ← 2 agents`. |
| `failed.ansi` | `bad-model`, `failed`/`idle` | `--model no-such-model-xyz`: a `⚠ Model … isn't recognized` banner, then an error reply. The header shows the model name in place of `Opus 5.5 with xhigh effort`. |
| `permission-prompt.ansi` | `touch-config`, `blocked`/`permission prompt` | The `Bash command` permission dialog (`Do you want to proceed?`, options 1–4, `Esc to cancel · Tab to amend`). Option 2 contains the absolute cwd. |
| `question.ansi` | `pick-colour`, `blocked`/`input needed` | The multiple-choice question dialog (`☐ Colour`, options, `Enter to select · ↑/↓ to navigate · Esc to cancel`). |
| `working.ansi` | `run-test-suite` | The longest stream. It includes the in-progress frames (`esc to interrupt`, `· 1 shell still running`). The session had finished by the time `logs` ran, so the final screen shows the finished turn (`DONE`). |
| `resumed-idle.ansi` | `touch-config` after `claude --resume <sessionId> --bg` with no prompt | The resumed session at an empty prompt: the interrupted Bash call (`⎿ Interrupted · What should Claude do instead?`), footer `⏸ manual mode on · ← 1 agent`, `◉ xhigh · /effort`. |

There is no stream for a stopped session: `claude logs` on it printed nothing
on stdout (see `cli-output/logs-stopped.stderr`).

### `cli-output/` — other command output

| File | Command | Content |
|---|---|---|
| `bg-success.stdout` | `claude --bg` with `--model no-such-model-xyz` | `backgrounded · <id> · <name>` plus the four hint lines. The start succeeds even with an unknown model; the session fails later (see `agents-json/` and `logs/failed.ansi`). |
| `bg-success.stderr` | same run | `Starting background service…`, printed when the supervisor has to start first. Only this run had its stderr saved. |
| `bg-untrusted.stderr` | `claude --bg "…"` in a folder that was never trusted | `Workspace not trusted. Run \`claude\` in <path> once and accept the trust prompt, then retry.` stdout was empty. |
| `resume-noprompt.stdout` | `claude --resume <sessionId> --bg` (no prompt), on `touch-config` after it had been stopped | `backgrounded · <id> · <name> (idle — send a prompt to start)` plus hints. The short id stays the same. |
| `resume-noprompt.stderr` | same run | `note: woke session <id> with its saved options (--name, --permission-mode, --model).` |
| `resume-prompt.stdout` | **hand-written**: `claude --resume <sessionId> --bg "continue"` on the same session | `resume-noprompt.stdout` without the ` (idle — …)` suffix, as the verified flow in [0004](../../doc/0004-account-hand-off.md) reports for a wake with a prompt. Not a capture. |
| `resume-prompt.stderr` | **hand-written**, same | The same `note:` line as `resume-noprompt.stderr`. Not a capture. |
| `logs-done.stderr` | `claude logs <id>` on `say-ok` | `Session <id> · <name>` header (stdout is `logs/done.ansi`). |
| `logs-stopped.stderr` | `claude logs <id>` on `say-ok` after it was stopped | `Couldn't read logs for <id> — job not found — it may have already exited`. stdout was empty. |
| `logs-unknown.stderr` | `claude logs nosuchjob` | `No job matching 'nosuchjob'. Run 'claude agents' to list running sessions.` |
| `stop-unknown.stderr` | `claude stop <name>` (a session name, not a short id) | `No job matching '<name>'. Run 'claude agents' to list running sessions.` Noted from the console; the name `say-ok` is illustrative. |
| `stop.stdout` | `claude stop <id>` on a session that was already stopped | `stopped <id>`. Noted from the console. |
| `rm.stdout` | `claude rm <id>` | `removed <id>`. Captured to a file; the console showed the same. |

### `usage/` — `claude -p --output-format stream-json … "/usage"` stdout

Shaped after a capture with Claude Code **2.1.312** (WSL, 2026-10-28) of the plan
usage probe of [0043](../../doc/0043-plan-usage-display.md): one JSON value per line,
`system`, `assistant` (with `usage_report`) and `result`. Every value is made up (the
percentages, reset times, time zone, ids, paths and the `/usage` text, shortened), and
fields tower does not read were left out. None of them is a capture byte for byte.

| File | What it shows |
|---|---|
| `plan.jsonl` | A subscription: `session` 24 %, `weekly_all` 12 %, a `weekly_scoped` limit for `Fable` 3 %, all `severity: normal`; extra usage off. The layout of the capture. |
| `no-plan.jsonl` | **hand-written**: `usage_report: null`, as for a logged-out account, an API key or usage billing (from research, not captured). |
| `odd.jsonl` | **hand-written**: a line that is not JSON, float and string percentages, `warning`, `critical` and an unknown `severity`, an unknown `kind` with a `surface` scope, a limit with only `percent`, a value that is not an object, extra usage on, and the text "Showing last-known usage · rate limited until 1:30pm". |

### `orchestrator/` — earlier orchestrator templates

`CLAUDE.md.1` to `CLAUDE.md.3` are the versions of `assets/orchestrator/CLAUDE.md`
tower shipped before [0044](../../doc/0044-orchestrator-starts-and-wakes-sessions.md),
taken from the repository's history unchanged (with the `{{TOWER}}` placeholder).
They are not CLI captures: the tests check that an unedited file tower wrote from one of
them is replaced by the current template, and that their hashes are listed.

### Exit codes

| Command | Exit code | Source |
|---|---|---|
| `claude --bg …` (start succeeds, even with an unknown model) | 0 | captured |
| `claude --bg …` in an untrusted folder | 1 | captured |
| `claude --resume <sessionId> --bg` with no prompt | not captured | — |
| `claude logs <id>` on a running session | not captured | — |
| `claude logs <id>` on a stopped session | not captured (it fails; stdout empty) | — |
| `claude logs <unknown>` | not captured | — |
| `claude stop <name>` | 1 | console |
| `claude stop <id>` on an already stopped session | 0 | console |
| `claude rm <id>` | 0 | console |

## Sanitization

These replacements were applied to every file, in JSON, text and the raw
byte streams alike. Only the fake values are listed here.

- **Paths.** The home directory is `/home/user`. The background sessions' folder
  is `/home/user/workspace/demo-app` (`~/workspace/demo-app` in the log
  header). The interactive session's cwd is also `/home/user/workspace/demo-app`.
  The untrusted folder is `/home/user/workspace/untrusted-repo`.
- **Plan label.** The plan label in the log headers is `Claude Pro`
  (same length as the original).
- **Session names, short ids, sessionIds and PIDs.** Each real value has one
  fake value, used the same way in every file. Each fake `sessionId` starts with
  its fake short id.

  | Name | `id` | `sessionId` | `pid` |
  |---|---|---|---|
  | `demo-app-3f` (interactive) | — | `e6c3b5d9-4f7a-4c2e-b1d8-9a0f2c4e6b13` | 1201 |
  | `bad-model` | `3e9a1c07` | `3e9a1c07-5b2d-4f60-9a1e-7c3b2d4e5f60` | 4241 |
  | `run-test-suite` | `5a7c2e91` | `5a7c2e91-0d4b-4c8e-8f21-6b3a9d0e7c12` | 4240 |
  | `print-date` | `7b4d9f02` | `7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93` | 4290 |
  | `pick-colour` | `9c6e1a35` | `9c6e1a35-8f2b-4d09-a7c4-5e1b3d6f8a20` | 4315 |
  | `say-ok` | `b2f8d064` | `b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81` | 4342 (gone once stopped) |
  | `touch-config` | `d4a1f7c8` | `d4a1f7c8-2e5b-4b3a-8c90-1f6d3a5b7e42` | 4377 |

  The PIDs keep the original order, which is not the start order:
  `run-test-suite`'s process has a lower pid than `bad-model`'s.
- **Timestamps.** Every `startedAt` was shifted by the same amount, so the
  interactive session starts at `1767225600000` (2026-01-01T00:00:00Z). All
  differences between rows are exact. The clock times in the logs
  (`done <time>`) were shifted by the same amount and shown in UTC (`1:59 AM`,
  `2:00 AM`).
- **Byte streams.** All replacements were done on bytes. When a replacement
  changes the length of the text, the stream was fixed up so the screen still
  reads correctly:
  - The prompt box's top rule (`──── <name> ─`) got more or fewer `─` so the
    rule stays 200 columns wide.
  - In `permission-prompt.ansi`, the absolute column moves (`CSI n G`) after
    the shorter cwd in option 2 were moved left by the same amount.

  Each sanitized stream was played in an isolated tmux server next to its raw
  original. The two screens differ only in the replaced text.
- **Kept as is.** Task prompts and replies (throwaway tasks), model names, the
  `nosuchjob` argument and the background task id `baotqvww8` in
  `working.ansi`, which is a random per-command id and not a session id.
