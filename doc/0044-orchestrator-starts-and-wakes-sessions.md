# 0044. The orchestrator starts and wakes sessions when asked

- Status: Accepted · 2026-10-28
- Related: [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [0029](0029-tracking-subagents.md), [0033](0033-hand-off-without-a-prompt.md), [0040](0040-quick-reply.md), [CLI reference](reference/claude-code-cli.md#cli-commands)
- Amends: the role ("does not start … sessions"), the rules and when the instruction file is written ([0010](0010-orchestrator.md)); [0008](0008-new-session-dialog.md) and [0040](0040-quick-reply.md), whose dispatch and wake paths `tower dispatch` and `tower wake` share

## Context

[0010](0010-orchestrator.md) makes the orchestrator report and relay only;
starting, stopping, removing and handing off sessions are the user's
actions in tower. The owner decided (2026-10-28) that the orchestrator may
also **start new background sessions** and **wake stopped ones**, but only
when the user explicitly asks it to, and only through tower commands that
take tower's own paths: the account's command, ownership records, the
subagent hooks, copy detection. A raw `claude --bg` or `--resume` typed by
the orchestrator would skip all of that, and a wake with the wrong
account's command moves the session to that account
([0003](0003-accounts-and-session-ownership.md)). Stop, remove, rename and
hand-off stay manual in tower.

The orchestrator's `CLAUDE.md` was written only when missing, so an
existing folder would keep the old rules forever. User edits to it must
still stay ([0010](0010-orchestrator.md)).

## Decision

**`tower dispatch --project <dir> [--account <name>] [--name <n>] [--model M]
[--effort E] [--mode P] [--json] [--] <task>`** starts a session the way `n`
does ([0008](0008-new-session-dialog.md)), with the same
`DispatchRequest`:

- The account's command (`default_account` unless `--account`, which must
  name a configured account); `--model`, `--effort`, `--mode` default to
  `default_model`, `default_effort`, `default_permission_mode`. The folder
  (`~` expanded, relative to the current directory) must exist and be a
  folder. The dialog's validation applies (task of 4 characters or more,
  single-line name, no `bypassPermissions` as root), and its Haiku warning
  is printed on stderr as `tower: warning: …`.
- `--settings <config dir>/claude-settings.json` while `track_subagents` is
  on, written first when not current; when it cannot be, a warning and no
  flag ([0029](0029-tracking-subagents.md)).
- Options come first: the first word that is not an option, or everything
  after `--`, is the task, words joined by single spaces. A task that
  starts with `-` must follow `--` and is passed after `--` to
  `claude --bg`, as the quick reply passes its text
  ([0040](0040-quick-reply.md); verified for `--bg`,
  [reference](reference/claude-code-cli.md#sending-a-prompt-to-a-background-session));
  any other task is passed exactly as the dialog passes it. The dialog
  still refuses a leading `-`.
- On success tower records the owner (`started`) and the project in
  `recent_projects`, as the dialog does. It lists the account (up to three
  times, 0.3 s apart) to learn the `sessionId` and, when it shows, keys the
  record by it at once; otherwise by the short id, which the next poll
  moves ([0017](0017-ownership-evidence-rules.md) section 5).
- An untrusted folder runs no trust flow: tower prints Claude Code's message
  and `<account> has not trusted <dir>; start a session there once from
  tower (`n`, then `Ctrl+T` to trust the folder), then try again`.
- Output: `started <id> · <name> under <account>`; with `--json` one line
  `{"id", "sessionId", "name", "account"}` (`sessionId` and `name` `null`
  when unknown). Exit codes: 0 started, 1 `claude --bg` failed (shown with
  its code and stderr), 2 bad arguments, config or request (nothing run),
  3 untrusted folder.

**`tower wake <id or name> [--account <name>] [--text <text> | -- <text>]`**
wakes a session as the quick reply wakes a stopped one:

- It polls every account once and merges as `tower status` does (same
  owner rules, `/proc` evidence, the usage probe's row hidden, sidecar read
  only). The session is the row whose short id or `sessionId` is the
  argument, else the only row with exactly that name; several such rows are
  refused with their ids.
- Only background rows with a short id, a `sessionId` and **no live
  process** (no `pid`, no `status` in `claude agents --json --all`). A live
  row is refused with its reason: idle at the end of a turn, `use the quick
  reply (t) in tower`; otherwise `is running (<detail>)`.
- **Owner.** The owner's command. An `--account` that differs from a known
  owner is refused: a wake never moves a session (the hand-off does,
  [0004](0004-account-hand-off.md)). An unknown owner (`?`) is refused
  unless `--account` is given, and then also while the accounts span
  several groups without `allow_cross_group_handoff`
  ([0018](0018-account-groups-and-handoff-guard.md), as `t` does). A wake
  with `--account` for an unknown owner records it as `asked`.
- **Another tower.** The session's record is read before the listings and
  compared with the state file right before the wake; when another tower
  recorded another account meanwhile, nothing runs
  ([0017](0017-ownership-evidence-rules.md) section 8).
- In the row's `cwd`: `<owner> --resume <sessionId> --bg` without a text
  (it wakes idle, [0033](0033-hand-off-without-a-prompt.md)), or
  `<owner> --resume <sessionId> --bg -- <text>` with one (trimmed, one
  argument, at most 100 KiB, no NUL; [0040](0040-quick-reply.md)). The same
  short id is success; another id is a copy: an error naming it, the copy
  recorded as `started` under the owner, nothing else stopped or removed.
- Output: `woke <id> · <name> under <account> · waiting for your prompt` or
  `· with your text as its next prompt`. Exit codes: 0 woken, 1 `--resume`
  failed or started a copy, 2 bad arguments or config, 3 untrusted folder,
  4 refused (not found, several, live, owner unknown or different, another
  tower's record, the folder gone, no account listed).

**Orchestrator instructions** (`assets/orchestrator/CLAUDE.md`). The role
becomes "report and relay; start or wake a session only when the user
explicitly asks". Before `tower dispatch` or `tower wake` it states the
project, account, name and exact task text (for a wake: the session, its
account and the exact text or none) and asks the user to confirm. It never
invents or rewords task text, passes `--account`, `--model`, `--effort` and
`--mode` only when the user named them, never wakes a session to answer
that session's question or prompt on the user's behalf, asks the user for
the account of an unknown owner, and never runs `claude --bg`, `--resume`,
`stop`, `rm`, `attach`, `respawn` or `daemon` itself. Every other rule of
[0010](0010-orchestrator.md) stays. The start prompt is unchanged.

**The instruction file is kept current unless the user edited it.** Tower
writes `CLAUDE.md` (atomically: temp file, fsync, rename; mode `0600`) when
it is missing, or when it is still exactly a version tower wrote and differs
from the current one: at startup when the orchestrator folder exists (the
folder itself is still created only by `o`), and before each orchestrator
start.

- *A version tower wrote:* the file, with the tower path put back as
  `{{TOWER}}`, has the SHA-256 of one of the shipped templates, which tower
  embeds (`orchestrator::SHIPPED_TEMPLATES`: every version of
  `assets/orchestrator/CLAUDE.md` from the history, and the current one; a
  test fails when the template changes without its hash). The path is the
  word in ``Run `<tower> status --json` ``, put back after every backtick
  (the current form) or only there (older versions name the binary once).
- *Anything else* is an edit and stays as it is ([0010](0010-orchestrator.md)).
  The TUI then shows once per run, in the status line: `orchestrator
  instructions were edited; the new dispatch and wake rules aren't in them;
  delete <path> to get tower's version`. `tower dispatch` and `tower wake`
  do not look at the file.
- A symlink in its place is left alone, without a note.

A running orchestrator reads the new file on its next start (a wake after
the idle stop, a hand-off, a new session), or when told to re-read
`CLAUDE.md`.

## Consequences

- The user can ask the orchestrator "start a session in billing-ui to fix
  the flaky test" or "wake schema-migration and tell it to rebase"; it
  confirms first and runs one tower command. The session starts or wakes
  exactly as from the dialog or `t`, with the hooks, the owner record and
  the account checks.
- Every command keeps the user's own text as the session's prompt; the
  orchestrator never writes a task itself, so it is still no "conductor"
  ([0010](0010-orchestrator.md) alternatives).
- An instruction file tower wrote gets the new rules at the next start of
  tower; one the user edited keeps their text and lacks the new rules until
  they delete it (or merge the template by hand), which the note says.
- `tower status` now counts a sidecar record still keyed by a short id for
  the listed row with that id, as the TUI does before each merge, so a
  session `tower dispatch` started is owned at once.
- Both commands use quota like any start or wake. Nothing here changes
  Claude Code ([0016](0016-do-not-change-claude-code-defaults.md)).

## Alternatives considered

- **Let the orchestrator run `claude --bg` / `--resume` itself.** Skips the
  owner record, the hooks, copy detection and the group guard; a wrong
  account's command moves the session. Rejected by the owner.
- **Waking live sessions too (stop, then wake with the text).** That is the
  quick reply; it interrupts a process, and stop stays manual. `t` in tower
  does it.
- **Running the trust flow from `tower dispatch`.** It needs a terminal the
  orchestrator does not have; tower says to trust the folder once from the
  dialog instead.
- **Always passing `--` before the task.** It works for `--bg` too (verified
  2.1.312), but passing it only for a task that starts with `-` keeps every
  other dispatch on exactly the dialog's command line.
- **Rewriting the instruction file whenever it differs.** Simple, but it
  would replace the user's edits, which 0010 keeps.
- **Recording a hash of what tower last wrote in the state file.** Covers
  only files written after this change; the embedded hashes also recognise
  files older towers wrote.

## Open items

- ~~Whether `claude --bg -- <task>` takes a task starting with `-` as the
  prompt.~~ It does: verified 2026-10-28 (Claude Code 2.1.312, WSL), see the
  [reference](reference/claude-code-cli.md#sending-a-prompt-to-a-background-session).
- Whether the instructions need tuning after real use of the two commands.
