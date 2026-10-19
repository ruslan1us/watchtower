# 0033. Hand-off without a prompt

- Status: Accepted · 2026-10-19
- Related: [0004](0004-account-hand-off.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0018](0018-account-groups-and-handoff-guard.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md), [reference](reference/claude-code-cli.md#cli-commands)
- Amends: step 2 ("the prompt is always passed") and the idle warning of step 3 of [0004](0004-account-hand-off.md), and the hand-off prompt row of [0024](0024-settings-window.md)

## Context

[0004](0004-account-hand-off.md) always passes `handoff_prompt` (default
`continue`) to `--resume <sessionId> --bg`, so the woken session starts a
turn under the target account at once. The owner tested the hand-off on a
host with two accounts and does not want the session prompted
automatically: they want to look at it first and send the next message
themselves.

Without a prompt, `<target> --resume <sessionId> --bg` (verified,
[reference](reference/claude-code-cli.md#cli-commands)):

- exits 0; stdout `backgrounded · <id> · <name> (idle — send a prompt to
  start)`, stderr `note: woke session <id> with its saved options (…).`;
- keeps the short id, so tower's continue/copy check works unchanged;
- leaves the session idle; it does not continue by itself. A session that
  was `done` keeps `state: done`, `status: idle`; one stopped mid-turn
  wakes as `state: working`, `status: idle`.

## Decision

- **Empty means no prompt.** `handoff_prompt = ""`, or a value that is
  blank after trimming, is valid. The hand-off then runs
  `<target command> --resume <full sessionId> --bg` with no prompt
  argument. The default stays `continue`; no prompt is opt-in. A non-empty
  prompt is trimmed and must not start with `-`, as before.
- **Parsing.** `backgrounded · <id> · <name>` is parsed with or without the
  `(idle — send a prompt to start)` suffix, also when the name is missing.
  The short id decides continued or copy as in 0004; stop, the stop wait and
  the sidecar record are unchanged.
- **Confirmation.** The dialog has a `Prompt:` line. With a prompt:
  `Prompt: "continue", sent as the next turn under alt.` Without one:
  `Prompt: none. The session will wait for you under alt; no turn is
  started.` For a working or blocked row, the interrupt note without a
  prompt says the session does not pick up by itself and may show as
  `working · idle` until the user sends one.
- **Status line.** After success without a prompt:
  `continued <name> under alt · waiting for your prompt`, as information.
  0004's warning (`… but it woke idle; attach and send a prompt`) stays for
  a wake that printed the idle suffix although a prompt was passed.
- **Rows.** The woken session is shown as Claude Code reports it:
  `working` / `idle` in Working (detail `idle`, no spinner), `done` /
  `idle` in Ready for review ([0006](0006-session-states-and-grouping.md)).
  Waiting idle raises no highlight; [0007](0007-visual-only-attention.md)
  applies as to any other row.
- **Settings window.** The hand-off prompt row accepts an empty value: the
  typed text is trimmed, blanks save as `handoff_prompt = ""`, and the row
  shows `(none — waits for you)`. A value starting with `-` is refused as
  typed. `Del` still removes the key, so `continue` applies.

## Consequences

- The user decides what the session does first under the new account; the
  hand-off itself starts no turn.
- A session handed off mid-turn sits as `working` / `idle` with nothing
  running until the user attaches or messages it. It is in Working, not
  Needs me, so nothing points at it except the status line after the
  hand-off.
- A permission prompt that was pending before the stop is not shown again
  (as in 0004); with no prompt, nothing asks the agent to retry it either.

## Alternatives considered

- **A separate boolean (`handoff_send_prompt = false`).** Two keys for one
  choice; an empty prompt already says "nothing to send".
- **Make no prompt the default.** Changes behaviour for existing users; the
  owner asked for an opt-in.
- **Highlight the woken session until attached.** Waiting idle is not a
  signal Claude Code reports, and 0007 highlights only Needs me and Failed.
