# 0004. Continue a session under another account

- Status: Accepted · 2026-10-18
- Related: [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [CLI reference](reference/claude-code-cli.md)

## Context

When an account hits its usage limits, the user wants the session to continue under another account with the same history and context. A stopped background session can be woken by any account's binary with `--resume <sessionId> --bg`, and it then runs under that binary's account ([0003](0003-accounts-and-session-ownership.md); [sessions docs](https://code.claude.com/docs/en/sessions)). Usage and limits are not in the listing JSON.

Verified flow, accounts sharing their session lists:

1. `<A> stop <id>`. Only the short id works; a name, a full `sessionId` or a pid gives `No job matching '<x>'`.
2. From the session's `cwd`: `<B> --resume <sessionId> --bg "continue"`. Output:
   `note: woke session <id> with its saved options (--name, --model).` (stderr) then `backgrounded · <id> · <name>` (stdout). The short id is unchanged, the prompt becomes the next turn, and `logs` then works with B's binary.

No attach is needed.

The saved options are the flags given at the original dispatch: a session dispatched with `--permission-mode` is woken with `(--name, --permission-mode, --model)`, so an explicitly set permission mode survives the hand-off. Without a prompt, `--resume <sessionId> --bg` also exits 0 but prints `backgrounded · <id> · <name> (idle — send a prompt to start)`; the session sits idle (`state: working`, `status: idle`) and does not continue by itself, and a permission prompt pending before the stop is not shown again ([CLI reference](reference/claude-code-cli.md#cli-commands)).

## Decision

Key `a` on a row hands the session off. v1 only has the manual trigger.

- **Target account**: with two accounts, the other one; with more, a picker of every account except the owner.
- **Confirmation**: required when the session is working (`state: working` or `status: busy`), since `stop` interrupts the current turn. Tower asks for every row with a live process in the Working or Needs me group, as `s` does ([0009](0009-stop-remove-and-logs.md)): stopping a blocked session also drops its pending prompt. The prompt says what will happen, e.g. "stop under main, continue under alt".
- **Steps**:
  1. If the session has a process (`pid` present), run `<owner> stop <id>`, then poll the listing until the process is gone (short timeout; abort with an error if it stays). Tower lists with the same command every 0.5 s for up to 10 s; the row counts as gone when it has neither `pid` nor `status`, or is not listed.
  2. Run `<target> --resume <sessionId> --bg "<handoff_prompt>"` with the working directory set to the row's `cwd`. `handoff_prompt` is configurable; the default is `continue`. The prompt is always passed: without one the woken session stays idle.
  3. Parse `backgrounded · <id> · <name>` from stdout. If the id equals the original, record the target account in the sidecar (`account_source: handoff`, [0002](0002-tech-stack-and-files.md)). The stderr `note:` lines are kept for messages. A line that still ends in `(idle — send a prompt to start)` is reported as a warning.
  The sequence runs on a worker thread; the status line shows the current step. Any other failure is shown with the CLI's text, and the sidecar is not changed.
- **Copy detection**: if `--resume <sessionId> --bg` starts a copy instead of continuing (Claude Code prints a `note:` line and a different id, e.g. because the session was still running), tower shows a warning. On the next poll it finds the copy by the short id printed in `backgrounded · <id> · <name>` and reads its `sessionId`. It records the copy under that `sessionId` in the sidecar with the target account and `account_source: handoff`, and leaves the original session's entry as it was. Until then the record is keyed by the copy's short id, the same mechanism as a dispatch ([0008](0008-new-session-dialog.md)).
- **Unknown owner** (`?`): `stop` works from any account, so tower stops with the target's command and asks only for the target.
- **Preconditions**, checked before step 1:
  - Background session with an `id` and `sessionId`. Interactive rows cannot be handed off, and the action is not offered for them.
  - The `cwd` still exists.
  - The target account trusts the folder. Trust is stored per account. If step 2 fails with `Workspace not trusted`, tower offers the trust flow from [0008](0008-new-session-dialog.md) (`Ctrl+T` runs the target's command in the folder) and then retries once, without a second `stop`.
- The orchestrator ([0010](0010-orchestrator.md)) is handed off the same way.

### Why manual

Limits are not in the JSON. A later version could take per-account 5 h / 7 d usage from a status-line script and suggest a hand-off. That suggestion must tell spent quota apart from a short rate limit, where waiting is cheaper than switching. Usage display is future work ([0014](0014-v1-scope.md)).

## Consequences

- The same short id, history and worktree continue under the target's limits; rows, logs and attach follow the new owner from the next poll.
- The interrupted turn is lost; the hand-off prompt asks the agent to pick up where it stopped. A permission prompt that was pending is not shown again.
- The permission mode carries over only when it was passed at dispatch. Tower always passes `--permission-mode` on dispatch ([0008](0008-new-session-dialog.md)), so its sessions keep their mode.
- The hand-off depends on the target account seeing the session's transcript, so the config dirs must share their history (as in the verified setup).
- If the target account's login has expired, the woken session shows `blocked` with no `waitingFor` (see the [CLI reference](reference/claude-code-cli.md)). Tower shows that state; logging in is up to the user (run the target command interactively). Tower never touches credentials.
- `startedAt` resets on wake. Activity times come from the sidecar ([0006](0006-session-states-and-grouping.md)).

## Alternatives considered

- **Attach with the target binary** (wakes it under the target too): needs a terminal round trip; the background wake does the same without one.
- **Start a fresh session under the target with a summary**: loses context and the worktree link.
- **Automatic hand-off on limit hit**: no reliable limit signal in v1.

## Open items

- Whether effort survives the wake. The `note:` lists `--name`, `--permission-mode` (when set at dispatch) and `--model` only; the resume docs say auto mode is restored when the account still qualifies.
- Whether a session stopped by the ~1 h idle rule (not by `stop`) keeps its account when its owner wakes it.
- Hand-off between accounts that do not share history has not been tested.
- The output of the copy case (wording of its `note:` line, exit code) has not been captured; tower only compares the short id.
- Tower's implementation has been tested against fake commands only; the end-to-end hand-off with real accounts, including how long a stopped process takes to leave the listing, is still to be checked on a host with two accounts.
