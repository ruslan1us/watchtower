# 0034. Remove with another account's command when the owner's service does not answer

- Status: Accepted · 2026-10-20
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0006](0006-session-states-and-grouping.md), [0009](0009-stop-remove-and-logs.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [reference](reference/claude-code-cli.md#multiple-config-dirs-accounts)
- Amends: the command routing of [0003](0003-accounts-and-session-ownership.md) ("every command … runs with the owning account's command", for `rm` only) and the remove of [0009](0009-stop-remove-and-logs.md)

## Context

[0003](0003-accounts-and-session-ownership.md) and
[0009](0009-stop-remove-and-logs.md) run `rm` with the owning account's
command, although `rm` works from any account when the session lists are
shared. On a host with `main` (`claude`) and `alt` (`claude-alt`) sharing
the session list (verified,
[reference](reference/claude-code-cli.md#multiple-config-dirs-accounts)):

- A session hosted by `alt` had no live process (no `pid`, no `status`)
  but still showed `state: blocked`: its process had ended without
  `claude stop`. `alt`'s supervisor was not running; `main`'s was, for a
  live session of its own.
- `claude-alt rm <id>` exited 1 with `couldn't remove <id> — the background
  service may be restarting. Try again in a moment.`, every time, from tower
  and from a shell.
- `claude rm <id>` printed `removed <id>` and the row was gone.
- With one account, a session stopped with `claude stop` (`state:
  stopped`) and a stopped `done` session are both removed fine while the
  supervisor is not running. So the refusal is not about a stopped
  supervisor as such; what triggers it is not known.

The row could not be removed from tower at all, only by hand with the
other account's command.

## Decision

- **When.** Only when all of these hold:
  1. the owner's `rm <id>` did not print `removed <id>`;
  2. its stdout or stderr contains `background service may be restarting`
     (matched case-insensitively, the stable part of the message);
  3. the row had no live process (no `pid` and no `status`,
     [0006](0006-session-states-and-grouping.md)) when the remove was
     confirmed. A row no longer listed counts as live.
- **Who.** The other configured accounts that the hand-off guard of
  [0018](0018-account-groups-and-handoff-guard.md) allows for this session's
  owner: the same group, or any account when `allow_cross_group_handoff =
  true`. Tower uses the guard's own target list, in config order. The guard
  is reused because it is the user's statement of which accounts may act on
  each other's sessions; `rm` moves no conversation, but a cross-group
  command still runs under another account.
- **How.** Tower runs `<command> rm <id>` with each of them in turn, the same
  argv as for the owner, never with `--discard-unpushed` or
  `--force-remove-worktree`. The first `removed <id>` wins; later accounts
  are not run. Any failure of a retry moves on to the next account.
- **Success** is a normal remove: tower polls at once, the row leaves the
  list and the selection moves as for any removed row, and the session's
  sidecar record ages out unseen ([0002](0002-tech-stack-and-files.md));
  tower writes no record for the account that ran `rm`. The status line says
  `removed <name> (with main's command; alt's background service was not
  answering)`, with the real account names.
- **Every command failed.** The refusal window stays as in 0009, with the
  owner's message, plus one line:
  - accounts tried: `alt's background service was not answering, so tower
    also tried main's command; it did not remove <name> either.` (several:
    `… also tried the commands of main, spare; none of them removed <name>
    either.`);
  - the guard allowed none: `alt's background service was not answering.
    Tower tried no other account: the account groups allow none for this
    session (allow_cross_group_handoff = false). Retry the remove later, or
    run `rm <id>` with another account's command yourself.`;
  - no other account is configured: `main's background service was not
    answering and no other account is configured. Retry the remove later.`
- **No fallback otherwise.** A row with a live process, the worktree
  refusals that suggest `--discard-unpushed` / `--force-remove-worktree`,
  `No job matching`, and every other error keep 0009's behaviour: one `rm`
  with the owner's command and its answer shown as is.
- `stop`, `logs`, `attach` and wakes keep the owner's command
  ([0003](0003-accounts-and-session-ownership.md)).

## Consequences

- A row left behind by an ended process can be removed from tower while its
  account's supervisor is down, without the user knowing which other command
  to run.
- One extra `rm` per allowed account, only after this specific refusal.
- A retry by another account could in principle meet a worktree refusal
  that the owner's command never reached; the window then shows the owner's
  message only, and the user retries later.
- Users with groups and no cross-group hand-off get no fallback across
  groups; the window says how to proceed.

## Alternatives considered

- **Retry the owner's command after a delay.** The refusal repeated every
  time while the supervisor was down; waiting does not help.
- **Start the owner's supervisor first** (e.g. with `claude-alt daemon …`).
  Changes Claude Code's state beyond the user's request
  ([0016](0016-do-not-change-claude-code-defaults.md)); whether it would
  help is unknown.
- **Use any account, ignoring groups.** Simpler, but runs a command of an
  employer's account on a personal account's session (or the reverse)
  behind the user's back.
- **Fall back for live rows too.** A live row has a working supervisor;
  this refusal there would mean something else, and the owner's command is
  the right one ([0003](0003-accounts-and-session-ownership.md)).

## Open items

- What makes `rm` give this refusal for one row without a live process and
  not for another (`state: blocked` left by an ended process against
  `stopped` / `done`); whether a later Claude Code release removes it.
