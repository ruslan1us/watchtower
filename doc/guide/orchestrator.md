# The orchestrator and the `tower` commands

The orchestrator session, and the commands that report on and start sessions without the TUI. Back to the [README](../../README.md).

## The orchestrator

`o` attaches to the background session named `orchestrator`. If there is none, tower starts
one under `default_account` in `~/.config/watchtower/orchestrator/` with permission mode
`auto`, then attaches. It is started only when you press `o`, so it costs nothing until
then. It reports and relays: it runs `tower status --json` and `claude logs`, and uses
Claude Code's cross-session messaging to ask other sessions questions. It does not answer
prompts or approve permissions ([0010](../0010-orchestrator.md)).

Only when you explicitly ask, it starts a session with `tower dispatch` or wakes a stopped
one with `tower wake`: it first states the project, account, name and the exact task text
and waits for your yes. It never makes up a task, never wakes a session to answer that
session's question for you, and never runs `claude --bg` or `--resume` itself. Stopping,
removing, renaming and handing off stay yours, in tower
([0044](../0044-orchestrator-starts-and-wakes-sessions.md)).

There is one orchestrator, and its conversation grows like any session's. It keeps nothing
tower needs and reads the session list fresh each time, so you lose nothing about your
sessions by resetting it:

- `/clear` inside it starts a new conversation in the same row (same id, name and account).
  Its rules still apply: they live in `CLAUDE.md` in its folder, which Claude Code loads by
  itself, so the start prompt isn't needed again.
- `/compact` shrinks the conversation but keeps a summary.
- `d` on its row removes it; the next `o` starts a new one.

Its instructions are `~/.config/watchtower/orchestrator/CLAUDE.md`, written from the
template [`assets/orchestrator/CLAUDE.md`](../../assets/orchestrator/CLAUDE.md) (built into the
binary) when the file is missing. A file tower wrote and you did not edit is updated to
the current template at startup and before tower starts an orchestrator, so a new tower
brings the new rules; a running orchestrator reads them on its next start, or when you
tell it to re-read `CLAUDE.md`. Your edits stay: an edited file (or a symlink) is never
overwritten, and the status line says once that it lacks the new dispatch and wake
rules; delete it to get tower's version.

## `tower status`

`tower status` polls every account once and prints the merged sessions as a short table;
`tower status --json` prints them as JSON with `config` (the config file and whether it
was found), `sessions` (id, name, folder, state, group, owning account and its command,
last change time, running subagents) and `errors` (accounts whose listing failed). The table starts with a
`config: <path>` line. It reads `state.json` but never writes it, so it is safe to run next to a running
tower. Exit code 0 when at least one account was listed, 1 when every listing failed, 2 for
a config error. The format is in [0010](../0010-orchestrator.md).

## `tower dispatch` and `tower wake`

Two commands start and wake sessions without the TUI, on the same paths as `n` and `t`
([0044](../0044-orchestrator-starts-and-wakes-sessions.md)):

```sh
tower dispatch --project ~/workspace/billing-ui [--account alt] [--name fix-flaky-test] \
    [--model opus] [--effort high] [--mode plan] [--json] -- 'fix the flaky test'
tower wake fix-flaky-test                       # wakes it; it waits for your prompt
tower wake 1a2b3c4d --text 'rebase on main'     # the text is its next prompt
```

- `tower dispatch` runs the account's command (`default_account` unless `--account`) in
  the folder, with the config's model, effort and permission mode unless given and tower's
  subagent hooks, records the owner and the recent project, and prints
  `started <id> · <name> under <account>` (`--json`: `id`, `sessionId`, `name`,
  `account`). Options come before the task; a task that starts with `-` goes after `--`.
  It runs no trust flow: in a folder the account has not trusted it exits 3 and says to
  start a session there once from tower (`n`, then `Ctrl+T`). Exit 0 started, 1 `claude`
  failed, 2 bad arguments or config, 3 untrusted.
- `tower wake <id or name>` wakes a session that has no live process, with the command of
  the account that owns it, keeping its id: `--resume <sessionId> --bg`, or with
  `--text` (or text after `--`) `--resume <sessionId> --bg -- <text>`. A running session
  is refused (for an idle one use `t`). `--account` is only for a session whose account
  is unknown (`?`), within one account group; it never moves a session to another
  account. Exit 0 woken, 1 failed or a copy started, 2 bad arguments or config,
  3 untrusted, 4 refused.
