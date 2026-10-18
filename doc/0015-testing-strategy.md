# 0015. Testing strategy

- Status: Accepted · 2026-10-18
- Related: [reference](reference/claude-code-cli.md), [0001](0001-thin-wrapper-over-claude-cli.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0013](0013-privacy-and-repository-hygiene.md)

## Context

Tower's logic is mostly parsing and state tracking on top of CLI output. Real
sessions cost quota, need logged-in accounts and behave in timing-dependent
ways, so they cannot run in CI. The one area that needs real sessions is the
terminal hand-over in `claude attach`, which can't be faked meaningfully.

## Decision

**Unit tests from fixtures.** Sanitized captures
([0013](0013-privacy-and-repository-hygiene.md)) live under `tests/fixtures/`:

- `agents/` — `claude agents --json --all` output covering:
  - each `state`: `working`, `blocked`, `done`, `failed`, `stopped`;
  - each `waitingFor`: `permission prompt`, `input needed`,
    `sandbox request`, `worker request`, `dialog open`;
  - `blocked` with `status: idle` and no `waitingFor` (seen on an expired
    login);
  - stopped rows without `pid`/`status`, and interactive rows without `id`
    and `state`;
  - unknown fields, unknown enum values, missing optional fields, an empty
    list, and a malformed row among good ones.
- `logs/` — raw `claude logs` byte streams with the expected reconstructed
  screen as a text snapshot.
- `cli/` — text output of other commands: `backgrounded · <id> · <name>`,
  `Starting background service…`, `note:` lines from `--resume`,
  `No job matching '<x>'…`, `stopped <id>`, `removed <id>`, `rm` refusals.

The tests cover:

- **Parser.** Every fixture parses; unknown values become `Unknown(raw)`; a bad
  row does not fail the listing ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **State mapping.** `(kind, state, status, waitingFor)` maps to the right group
  and label ([0006](0006-session-states-and-grouping.md)).
- **Transitions.** A sequence of snapshots with an injected clock produces the
  expected last-change times, attention highlights set and cleared, and merging
  by `sessionId` across accounts.
- **Sidecar and config.** Round-trip, atomic write, a corrupt file leading to a
  fresh start with a warning, and defaults for a missing config.
- **Command construction.** The argv, working directory and account binary for
  start, stop, remove, logs and hand-off. These are checked against a fake
  `claude` executable placed first on `PATH`, which records its arguments and
  prints fixture output.
- **Rendering.** Key screens drawn with ratatui's `TestBackend` and compared to
  snapshots.

**Manual attach/detach harness (opt-in).** Run on a development machine with
real accounts, never in CI. Tests are marked `#[ignore]` and also require an
environment variable such as `TOWER_LIVE_TESTS=1`. The harness:

1. Starts a tiny background session (a one-line task such as "reply OK").
2. Starts an isolated tmux server (`tmux -L tower-test`) so the user's own tmux
   is untouched.
3. Inside it, runs a wrapper script that records `stty -g`, runs
   `claude attach <id>`, then records the exit code and `stty -g` again.
4. Sends keys with `tmux send-keys` (`C-z`; `Left` then `Escape`).
5. Checks: exit code 0, identical terminal settings before and after, no
   leftover processes, and the session still listed and alive.
6. Stops and removes the session.

This is the method used to verify the detach behaviour described in
[0005](0005-attach-and-detach.md). It is rerun after Claude Code upgrades.

## Consequences

- CI needs no accounts, network or quota.
- A Claude Code output change shows up first as a failing live check or a
  parse warning. The fix is a new fixture plus a parser change.
- Fixtures are only as complete as the captures; states that are hard to
  provoke (e.g. `worker request`) may start out as hand-written fixtures and be
  replaced by captures later.

## Alternatives considered

- **Tests against real accounts in CI.** Costs quota, needs secrets, and is
  flaky.
- **Mocking only at the Rust trait level.** Cheaper, but a fake executable also
  checks the real argv and process handling.
- **Driving the harness with `expect`.** tmux gives a real terminal with
  scriptable keys and does not interfere with the caller's terminal.

## Open items

- A complete fixture set from a real host: every state and `waitingFor` value,
  `failed` and `stopped` rows.
- Captures of `--bg` failures (`Workspace not trusted`, an unknown model), the
  `rm` refusal text, and `logs` exit codes and output for a stopped session.
