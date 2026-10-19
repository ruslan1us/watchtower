# 0010. Orchestrator session

- Status: Accepted · 2026-10-18
- Related: [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md), [CLI reference](reference/claude-code-cli.md)

## Context

The user wants to ask in plain language "what are my agents doing?", "ask fix-flaky-test whether it reached the tests", "tell me when the migration finishes". Claude Code already has cross-session messaging: `ListAgents`, `SendMessage` and `notify_when_idle` ([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)). A message can never approve a permission prompt, and a session in bypass mode holds incoming messages for approval. There is no CLI to reply to a session without attaching, and no CLI to pin a session.

## Decision

The orchestrator is an **ordinary Claude Code background session named `orchestrator`**. Tower adds no agent logic of its own.

- **Start, lazily.** Tower does not dispatch the orchestrator on startup. Only when `o` is pressed does it look for a background row named exactly `orchestrator` in the merged listing, and only once every account has been listed at least once (until then `o` says so and does nothing, so a listing still loading never starts a second one). If none exists, it dispatches one under `default_account`, off the UI thread like any dispatch, and then attaches without waiting for a refresh, as dispatch-and-attach does ([0005](0005-attach-and-detach.md)):
  `cd <config dir>/orchestrator && <default command> --bg --name orchestrator [--model M] [--effort E] --permission-mode auto "You are the Watchtower orchestrator. Read CLAUDE.md in this folder, then wait for my questions."`
  `--effort` is `default_effort` (omitted when `default`). The owner is recorded in the sidecar as `started`, as for any dispatch; the folder is not added to the dialog's recent projects. Until a listing shows the new session (at most three listings of its account), `o` attaches to it again instead of starting another. If a window is open when the dispatch reports back, tower does not attach and says to press `o`. This avoids quota use on every start of tower, and an orchestrator the user removed on purpose is not silently recreated.
- **Duplicates.** If several background rows are named `orchestrator`, `o` takes one with a live process first, then the most recently started (`startedAt`), and the status line says how many there are and which id it used.
- **Folder.** `<config dir>/orchestrator/` (normally `~/.config/watchtower/orchestrator/`) is not a git repo, so no worktree is created. Tower creates it right before the dispatch when it is missing, with mode `0700`, and writes a `CLAUDE.md` there (mode `0600`) with the instructions if the file is missing. An existing file, or a symlink in its place, is never overwritten, so user edits stay; to get a newer template, delete the file and press `o` with no orchestrator listed. The template is `assets/orchestrator/CLAUDE.md` in the repository, built into the binary; at write time tower puts the absolute path of the running `tower` binary into its `status --json` command, with a note to use plain `tower` if that path no longer exists. `CLAUDE.md` is used instead of `--append-system-prompt` because it is re-read on every wake, resume and hand-off, while not all launch flags are restored on resume.
- **Trust.** Each account that runs the orchestrator must trust the folder. On `Workspace not trusted` tower opens a window offering the trust flow from [0008](0008-new-session-dialog.md): `Ctrl+T` runs the account's interactive command in the folder, then the dispatch is retried once; `Esc` gives up.
- **Permission mode `auto`.** Bypass mode would hold every incoming cross-session reply for approval, and it is refused when tower runs as root ([permission modes](https://code.claude.com/docs/en/permission-modes)). Auto mode is unavailable with Haiku 4.5, so the orchestrator uses `default_model` (no `--model` flag when it is `default`), never Haiku: when `default_model` names a Haiku model (any name containing `haiku`), tower passes no `--model` and Claude Code's default applies.
- **Attach.** `o` on a listed orchestrator attaches exactly like `Enter` on its row: with the owning account's command ([0003](0003-accounts-and-session-ownership.md)), after the refresh, through the account picker when the owner is unknown, and refused while it is being handed off; `Ctrl+Z` returns to tower ([0005](0005-attach-and-detach.md)).
- **Accounts.** Switchable with the normal hand-off, `a` on its row ([0004](0004-account-hand-off.md)).
- **Place in the list.** Every background row named exactly `orchestrator` is shown in its own group, `Orchestrator`, pinned at the top in both state and project grouping, before Needs me, in the order `o` picks from (see Duplicates) ([0006](0006-session-states-and-grouping.md)). Its detail is its state's detail, with "ready" while it waits for the user (a turn that ended with or without a question), e.g. "working" while busy. It ends almost every turn with a question, which Claude Code lists as `blocked` + `idle` with no `waitingFor`, so it is excluded from attention: no `●`, not in the counter, never in Needs me or Failed ([0007](0007-visual-only-attention.md)). Every row key works on it as on any row (`Enter`, `Space`, `s`, `d`, `a`; `m` says it is not highlighted), and the text filter applies to it.
- **Role: report and relay only.** It summarises state, reads logs, forwards questions and answers, and sets idle notices. It does not answer other sessions' questions on its own, does not try to approve permissions (messages cannot do that), never asks a session to do something that was denied elsewhere, and does not start, stop, remove or hand off sessions; those stay user actions in tower.

### Tools available to it

- `tower status --json`: one merged snapshot across all accounts, with ownership and tower's tracked times. It polls every account once, in parallel, with the usual timeout, and merges and resolves owners by the same rules as the TUI ([0017](0017-ownership-evidence-rules.md)). It reads the sidecar but never writes, locks or moves it, so it cannot race a running tower. The output is an object: `config`, the config file this run used (`path`) and whether it was found (`found`; `false` means the built-in defaults, [0002](0002-tech-stack-and-files.md)), `sessions`, an array with one object per merged row, and `errors`, one object per account whose listing failed. Absent values are `null`.

  ```json
  {
    "config": { "path": "/home/user/.config/watchtower/config.toml", "found": true },
    "sessions": [
      {
        "id": "1a2b3c4d",
        "sessionId": "1a2b3c4d-0000-4000-8000-000000000000",
        "name": "fix-flaky-test",
        "cwd": "/home/user/workspace/example-repo",
        "kind": "background",
        "state": "blocked",
        "status": "waiting",
        "waitingFor": "permission prompt",
        "pid": 4242,
        "startedAt": 1700000000000,
        "account": "alt",
        "accountCommand": "claude-alt",
        "group": "needs-me",
        "detail": "permission prompt",
        "lastChangeAt": 1700000000000
      }
    ],
    "errors": [
      { "account": "main", "accountCommand": "claude", "error": "listing failed (exit 1): …" }
    ]
  }
  ```

  Fields from the listing keep their names and values. `account` is the configured name or `"?"`, with `accountCommand` `null` when unknown. `group` is the overview group of [0006](0006-session-states-and-grouping.md) (`orchestrator`, `needs-me`, `failed`, `ready-for-review`, `working`, `other`, `stopped`, `interactive`; `orchestrator` for every row named so, whatever its state) and `detail` the row's detail text from the same table. `lastChangeAt` is Unix ms of the last state change tracked by tower (the sidecar's `last_change_ms` while its `last_state` is the row's current tuple; the time of this call when the tuple has changed since), falling back to `startedAt` ([0006](0006-session-states-and-grouping.md)). Exit code 0 when at least one account was listed, 1 when every listing failed (the JSON is still printed), 2 for a config error (nothing on stdout). Warnings (skipped rows, an unreadable sidecar, an inherited `CLAUDE_CONFIG_DIR`) go to stderr. Without `--json`, `tower status` prints a short table for people, starting with a `config: <path>` line (`(not found — using defaults)` appended when the file is missing).
- `<accountCommand> logs <id>` for recent output (raw terminal stream; the instructions tell it to read it as a screen snapshot).
- Built-in `ListAgents` / `SendMessage`, and `notify_when_idle` for "tell me when X finishes". It sends one notice and expires after 12 h.

### Outline of `CLAUDE.md`

- Who you are: the Watchtower orchestrator; you report and relay, you do not do project work.
- For an overview, run `<path of tower> status --json` (plain `tower` if that path is gone); group the answer by `group`: needs-me and failed, ready for review, working; the `orchestrator` group is itself and is left out.
- For detail on one session, run `<accountCommand> logs <id>`, always with the command from the status output, never another account's; read it as a screen snapshot.
- Relay questions with `SendMessage`, name the target session, and report replies verbatim when short.
- For "tell me when…", use `notify_when_idle` and say it expires after 12 h.
- Never answer a session's question or prompt on the user's behalf; tell the user to attach.
- Never ask a session to do something that was denied elsewhere or to work around a permission.
- Never run `stop`, `rm`, `--bg`, `--resume`, `attach`, `respawn` or `daemon` commands, and never change Claude Code settings.
- Say so when unsure; keep answers short.

## Consequences

- The orchestrator is not pinned (no CLI), so after about 1 h idle its process stops; it wakes on the next `o`, and the first answer is slower. Accepted.
- It costs quota like any session, but only once the user presses `o`; the first dispatch is one short turn.
- Its row has its own group at the top but otherwise acts like any other session; it can be handed off, stopped or removed. After removal, the next `o` dispatches a new one.
- Changes to the instruction text take effect on the next wake.

## Alternatives considered

- **Tower-side chat or quick reply without attaching**: no CLI for it; dropped for v1 ([0014](0014-v1-scope.md)).
- **`--append-system-prompt`**: not reliably restored on resume or hand-off.
- **Bypass permissions**: holds incoming messages and is refused as root.
- **Answering other sessions autonomously** (a "conductor" model): out of scope; the user stays the decision maker.

## Open items

- Whether sessions hosted by another account's supervisor appear in `ListAgents` and receive `SendMessage` and `notify_when_idle`. Until then, relaying may work only within the orchestrator's own account.
- Whether the instruction text and start prompt need tuning after real use (see `assets/orchestrator/CLAUDE.md`).
