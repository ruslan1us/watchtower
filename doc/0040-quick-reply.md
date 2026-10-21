# 0040. Quick reply

- Status: Accepted · 2026-10-21
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [0027](0027-rename-sessions.md), [0036](0036-prompt-templates.md), [CLI reference](reference/claude-code-cli.md#sending-a-prompt-to-a-background-session)
- Amends: the session list keys of [0011](0011-keybindings.md) (`t`), and [0036](0036-prompt-templates.md) (the template picker is also used in the reply window)

## Context

The owner sends the same texts, mostly review instructions, to sessions
waiting for review. Attaching for each one is a terminal round trip. The
text must reach the session as the user's own prompt, under the account
that owns it, and keep its id. Verified with Claude Code 2.1.306
([CLI reference](reference/claude-code-cli.md#sending-a-prompt-to-a-background-session)):

- On a **stopped** session, `--resume <sessionId> --bg <text>` wakes the
  same id and the text is the next turn. A multi-line text arrives intact.
- On a **live** session the same command does not deliver it: exit 0,
  stdout `backgrounded · <copy id>`, stderr `note: session <id> is already
  running in the background, so this started a copy as <copy id>. …`.
- `--resume <sessionId> --bg -- <text>` takes a text that starts with `-`
  as the prompt, and still wakes the same id.
- A `SendMessage` relayed through another session arrives as that agent's
  message, not the user's.

## Decision

**Key.** `t` on a session row (`е` on a RU/UA layout). `t` was free in the
list, the logs panel and the Settings window.

**Offered** for background rows with a short id and a `sessionId` that are:
Ready for review (`done` / `idle`); blocked with `status: idle` and no
`waitingFor` (a question that ended the turn); `failed` / `idle`;
`working` / `idle`, the state a wake without a prompt leaves
([0033](0033-hand-off-without-a-prompt.md)); the orchestrator under the
same rules; and any row without a live process (stopped and finished,
failed, stopped mid-turn). **Not offered**, with a status-line note
instead: `status: busy` (a running turn), including
`done · bg task` ([0032](0032-background-task-after-a-finished-turn.md));
`status: waiting` or any `waitingFor` (`… attach to answer it (Enter)`); a
live row without `status`; interactive rows; an unknown owner (`?`) while
the accounts span several groups and `allow_cross_group_handoff` is not set
([0018](0018-account-groups-and-handoff-guard.md)); a row being handed off,
renamed or replied to, or with an attach waiting for its refresh.

**Owner.** The reply runs with the owner's command
([0003](0003-accounts-and-session-ownership.md)). With an unknown owner in
one group the account picker opens first (`… to send your text, which moves
the session to it`), its answer is recorded as `asked`, and the window
opens under that account.

**Window** `Reply to <name>`, in the dashboard `Look` like every window: a
multi-line text area (the dialog's `TextArea`), one line saying what will
happen — live: `Stops the idle session and wakes it under <account> with
your text as the next prompt.`; stopped: `Wakes the session under
<account> with your text as the next prompt.` — and the keys `Enter new
line · Ctrl+S send · Ctrl+P template · Esc cancel` (*amended 2026-10-21,
UI audit:* the only place `Esc` is named; one row, whole hints dropped
when it is too wide in this order: `Enter new line`, `Ctrl+P template`,
`Esc cancel`, while `Ctrl+S send` stays; the text wraps at words as the
dialog's task, [0008](0008-new-session-dialog.md)). The dialog's
conventions apply: `Enter` and `Ctrl+J` are new lines, `Ctrl+S` sends,
`Esc` and `Ctrl+C` ask first when text was typed. Typed Cyrillic stays as
typed. `Ctrl+P` opens the template picker of
[0036](0036-prompt-templates.md) unchanged (`App::template_picker_mut`
finds it in this window too); a pick replaces a blank text, else goes in at
the cursor. A blank text sends nothing. The text is trimmed; line breaks
inside are kept; at most 100 KiB, no NUL.

**Send.** `Ctrl+S` checks the row again. A row no longer offered keeps the
window, the text and the reason. A row whose live process or owner changed
says what `Ctrl+S` does now and waits for a second press. Otherwise the
window closes and a worker thread runs (`reply::run`):

1. Live row: `<owner> stop <id>`, then the hand-off's stop wait
   ([0004](0004-account-hand-off.md)).
2. In the row's `cwd`: `<owner> --resume <full sessionId> --bg -- <text>`.
   `--` ends the options, so a text starting with `-` stays text; the text
   is one argument.
3. `backgrounded · <id> …` with the session's own short id is success.
   Another id is a copy: an error naming the copy, nothing else is stopped
   or removed, and the copy's owner is recorded as `started`.

Before the stop and again before the wake, the session's record in the
state file is compared as for a hand-off
([0017](0017-ownership-evidence-rules.md) section 8). `Workspace not
trusted` opens a trust window: `Ctrl+T` runs the owner's command in the
folder, then the reply is retried once without a second stop; `Esc` leaves
the session stopped. The status line shows `stopping <name>…`, `waking
<name> with your text…`, then `sent to <name>`. Replies to different rows
may run at once; each row refuses `Enter`, `s`, `d`, `a`, `e` and `t`
until its reply reports back.

## Consequences

- A reply is one key, a text and `Ctrl+S`; templates make the usual ones a
  few keys more. Nothing about Claude Code changes
  ([0016](0016-do-not-change-claude-code-defaults.md)): tower only stops and
  wakes the session with its own CLI.
- The wake is real activity: the session works on the text, and its states
  count as usual ([0006](0006-session-states-and-grouping.md),
  [0007](0007-visual-only-attention.md)).
- A live session's process is restarted; `startedAt` resets. A session
  waiting on a permission prompt or a question dialog is never offered, so
  no pending prompt is dropped.
- Quitting tower while a reply runs does not wait for it: the session may
  stay stopped without the text, and is then an ordinary stopped row.
- A row that is `working` / `idle` after a wake without a prompt
  ([0033](0033-hand-off-without-a-prompt.md)) is offered: no turn runs, so
  stopping it interrupts nothing (owner decision, 2026-10-21). A `working`
  row whose status is `busy` or missing is still refused.

## Alternatives considered

- **`--resume <sessionId> --bg <text>` on the live session.** Rejected: it
  starts a copy (verified).
- **`SendMessage` through the orchestrator.** Rejected: the text arrives as
  an agent's message, not the user's.
- **Typing into the session through `attach` or a PTY.** Rejected: tower
  never types into a session ([0005](0005-attach-and-detach.md),
  [0027](0027-rename-sessions.md)).
- **Prefixing a leading `-` with a space.** Not needed: `--` works and
  leaves the text as typed.
