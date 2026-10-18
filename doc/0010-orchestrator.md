# 0010. Orchestrator session

- Status: Accepted · 2026-10-18
- Related: [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0008](0008-new-session-dialog.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md), [CLI reference](reference/claude-code-cli.md)

## Context

The user wants to ask in plain language "what are my agents doing?", "ask fix-flaky-test whether it reached the tests", "tell me when the migration finishes". Claude Code already has cross-session messaging: `ListAgents`, `SendMessage` and `notify_when_idle` ([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)). A message can never approve a permission prompt, and a session in bypass mode holds incoming messages for approval. There is no CLI to reply to a session without attaching, and no CLI to pin a session.

## Decision

The orchestrator is an **ordinary Claude Code background session named `orchestrator`**. Tower adds no agent logic of its own.

- **Start, lazily.** Tower does not dispatch the orchestrator on startup. Only when `o` is pressed does it look for a background row named exactly `orchestrator` in the merged listing. If none exists, it dispatches one under `default_account` and then attaches:
  `cd ~/.config/watchtower/orchestrator && <default command> --bg --name orchestrator --permission-mode auto [--model M] "<short start prompt>"`
  This avoids quota use on every start of tower, and an orchestrator the user removed on purpose is not silently recreated.
- **Folder.** `~/.config/watchtower/orchestrator/` is not a git repo, so no worktree is created. Tower writes a `CLAUDE.md` there with the instructions if the file is missing; it does not overwrite user edits. `CLAUDE.md` is used instead of `--append-system-prompt` because it is re-read on every wake, resume and hand-off, while not all launch flags are restored on resume.
- **Trust.** Each account that runs the orchestrator must trust the folder. On `Workspace not trusted` tower offers the trust flow from [0008](0008-new-session-dialog.md).
- **Permission mode `auto`.** Bypass mode would hold every incoming cross-session reply for approval, and it is refused when tower runs as root ([permission modes](https://code.claude.com/docs/en/permission-modes)). Auto mode is unavailable with Haiku 4.5, so the orchestrator uses `default_model` (no `--model` flag when it is `default`), never `haiku`.
- **Attach.** `o` attaches with the owning account's command ([0003](0003-accounts-and-session-ownership.md)); `Ctrl+Z` returns to tower ([0005](0005-attach-and-detach.md)).
- **Accounts.** Switchable with the normal hand-off, `a` on its row ([0004](0004-account-hand-off.md)).
- **Role: report and relay only.** It summarises state, reads logs, forwards questions and answers, and sets idle notices. It does not answer other sessions' questions on its own, does not try to approve permissions (messages cannot do that), never asks a session to do something that was denied elsewhere, and does not start, stop, remove or hand off sessions; those stay user actions in tower.

### Tools available to it

- `tower status --json`: one merged snapshot across all accounts, with ownership and tower's tracked times. Exit code 0 on success. Proposed schema: an array of objects; absent values are `null`.

  ```json
  [
    {
      "id": "1a2b3c4d",
      "sessionId": "1a2b3c4d-0000-4000-8000-000000000000",
      "name": "fix-flaky-test",
      "cwd": "/home/user/workspace/example-repo",
      "kind": "background",
      "state": "blocked",
      "status": "waiting",
      "waitingFor": "permission prompt",
      "account": "alt",
      "accountCommand": "claude-alt",
      "lastChangeAt": 1700000000000
    }
  ]
  ```

  Fields from the listing keep their names and values. `account` is the configured name or `"?"`, with `accountCommand` `null` when unknown. `lastChangeAt` is Unix ms of the last state change tracked by tower, falling back to `startedAt` ([0006](0006-session-states-and-grouping.md)).
- `<accountCommand> logs <id>` for recent output (raw terminal stream; the instructions tell it to read it as a screen snapshot).
- Built-in `ListAgents` / `SendMessage`, and `notify_when_idle` for "tell me when X finishes". It sends one notice and expires after 12 h.

### Outline of `CLAUDE.md`

- Who you are: the Watchtower orchestrator; you report and relay, you do not do project work.
- For an overview, run `tower status --json`; group the answer by needs-me, ready for review, working.
- For detail on one session, run `<accountCommand> logs <id>`, always with the command from the status output.
- Relay questions with `SendMessage`, name the target session, and report replies verbatim when short.
- For "tell me when…", use `notify_when_idle` and say it expires after 12 h.
- Never answer a session's question or prompt on the user's behalf; tell the user to attach.
- Never ask a session to do something that was denied elsewhere or to work around a permission.
- Never run `stop`, `rm`, `--bg`, `--resume` or `attach`.
- Keep answers short.

## Consequences

- The orchestrator is not pinned (no CLI), so after about 1 h idle its process stops; it wakes on the next `o`, and the first answer is slower. Accepted.
- It costs quota like any session, but only once the user presses `o`; the first dispatch is one short turn.
- Its row is listed like any other session; it can be handed off, stopped or removed. After removal, the next `o` dispatches a new one.
- Changes to the instruction text take effect on the next wake.

## Alternatives considered

- **Tower-side chat or quick reply without attaching**: no CLI for it; dropped for v1 ([0014](0014-v1-scope.md)).
- **`--append-system-prompt`**: not reliably restored on resume or hand-off.
- **Bypass permissions**: holds incoming messages and is refused as root.
- **Answering other sessions autonomously** (a "conductor" model): out of scope; the user stays the decision maker.

## Open items

- Whether sessions hosted by another account's supervisor appear in `ListAgents` and receive `SendMessage` and `notify_when_idle`. Until then, relaying may work only within the orchestrator's own account.
- The exact `CLAUDE.md` text and start prompt.
