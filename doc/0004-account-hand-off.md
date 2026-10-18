# 0004. Continue a session under another account

- Status: Accepted · 2026-10-18
- Related: [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [CLI reference](reference/claude-code-cli.md)

## Context

When an account hits its usage limits, the user wants the session to continue under another account with the same history and context. A stopped background session can be woken by any account's binary with `--resume <sessionId> --bg`, and it then runs under that binary's account ([0003](0003-accounts-and-session-ownership.md); [sessions docs](https://code.claude.com/docs/en/sessions)). Usage and limits are not in the listing JSON.

Verified flow, accounts sharing their session lists:

1. `<A> stop <id>`. Only the short id works; a full `sessionId` or a pid gives `No job matching '<x>'`.
2. From the session's `cwd`: `<B> --resume <sessionId> --bg "continue"`. Output:
   `note: woke session <id> with its saved options (--name, --model).` then `backgrounded · <id> · <name>`. The short id is unchanged, the prompt becomes the next turn, and `logs` then works with B's binary.

No attach is needed.

## Decision

Key `a` on a row hands the session off. v1 only has the manual trigger.

- **Target account**: with two accounts, the other one; with more, a picker of every account except the owner.
- **Confirmation**: required when the session is working (`state: working` or `status: busy`), since `stop` interrupts the current turn.
- **Steps**:
  1. If the session has a process (`pid` present), run `<owner> stop <id>`, then poll the listing until the process is gone (short timeout; abort with an error if it stays).
  2. Run `<target> --resume <sessionId> --bg "<handoff_prompt>"` with the working directory set to the row's `cwd`. `handoff_prompt` is configurable; the default is `continue`.
  3. Parse `backgrounded · <id> · <name>`. If the id equals the original, record the target account in the sidecar (`account_source: handoff`, [0002](0002-tech-stack-and-files.md)).
- **Copy detection**: if `--resume <sessionId> --bg` starts a copy instead of continuing (Claude Code prints a `note:` line and a different id, e.g. because the session was still running), tower shows a warning. On the next poll it finds the copy by the short id printed in `backgrounded · <id> · <name>` and reads its `sessionId`. It records the copy under that `sessionId` in the sidecar with the target account and `account_source: handoff`, and leaves the original session's entry as it was.
- **Unknown owner** (`?`): `stop` works from any account, so tower stops with the target's command and asks only for the target.
- **Preconditions**, checked before step 1:
  - Background session with an `id` and `sessionId`. Interactive rows cannot be handed off, and the action is not offered for them.
  - The `cwd` still exists.
  - The target account trusts the folder. Trust is stored per account. If step 2 fails with `Workspace not trusted`, tower offers the trust flow from [0008](0008-new-session-dialog.md) and then retries.
- The orchestrator ([0010](0010-orchestrator.md)) is handed off the same way.

### Why manual

Limits are not in the JSON. A later version could take per-account 5 h / 7 d usage from a status-line script and suggest a hand-off. That suggestion must tell spent quota apart from a short rate limit, where waiting is cheaper than switching. Usage display is future work ([0014](0014-v1-scope.md)).

## Consequences

- The same short id, history and worktree continue under the target's limits; rows, logs and attach follow the new owner from the next poll.
- The interrupted turn is lost; the hand-off prompt asks the agent to pick up where it stopped.
- The hand-off depends on the target account seeing the session's transcript, so the config dirs must share their history (as in the verified setup).
- If the target account's login has expired, the woken session shows `blocked` with no `waitingFor` (see the [CLI reference](reference/claude-code-cli.md)). Tower shows that state; logging in is up to the user (run the target command interactively). Tower never touches credentials.
- `startedAt` resets on wake. Activity times come from the sidecar ([0006](0006-session-states-and-grouping.md)).

## Alternatives considered

- **Attach with the target binary** (wakes it under the target too): needs a terminal round trip; the background wake does the same without one.
- **Start a fresh session under the target with a summary**: loses context and the worktree link.
- **Automatic hand-off on limit hit**: no reliable limit signal in v1.

## Open items

- Whether `--resume <sessionId> --bg` works with no prompt.
- Whether the permission mode (and effort) survives the wake. The `note:` mentions only `--name` and `--model`; the resume docs say auto mode is restored when the account still qualifies.
- Whether a session stopped by the ~1 h idle rule (not by `stop`) keeps its account when its owner wakes it.
- Hand-off between accounts that do not share history has not been tested.
