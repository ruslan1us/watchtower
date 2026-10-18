# 0001. Thin wrapper over the Claude Code CLI

- Status: Accepted · 2026-10-18
- Related: [reference](reference/claude-code-cli.md), [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0016](0016-do-not-change-claude-code-defaults.md)

## Context

Claude Code already keeps sessions alive without a terminal. A per-account
supervisor runs background workers, restarts crashed ones, stops idle ones
after about an hour and wakes them on attach. It also ships its own overview
(`claude agents`, see https://code.claude.com/docs/en/agent-view). What it does
not offer is a layout and workflow of our choosing, one view across several
accounts, or a hand-off between accounts.

The documentation says to use the CLI's JSON output rather than its files. The
internal files (`jobs/`, `daemon/`, transcripts, credentials) have no published
format and can change in any release.

## Decision

Tower is a front end and nothing more. Everything it knows about sessions comes
from the `claude` CLI, and everything it does to them goes through the CLI:

- **Listing.** Tower runs `<account command> agents --json --all` for every
  configured account every `poll_interval_secs` (default 2 s). It merges the
  results by `sessionId`, falling back to the short `id` and then `pid`, because
  accounts that share session storage list the same sessions
  ([0003](0003-accounts-and-session-ownership.md)).
- **Actions.** It uses `--bg`, `--resume <sessionId> --bg`, `attach`, `logs`,
  `stop` and `rm`, always with the binary of the account that hosts the session
  ([0003](0003-accounts-and-session-ownership.md)).
- **Lenient parsing.** Unknown fields are ignored and missing fields get
  defaults. Unknown values of `kind`, `state`, `status` and `waitingFor` are kept
  as `Unknown(<raw string>)` and shown as-is, never treated as errors. A
  malformed row is skipped with a warning; it does not fail the whole listing.
- **One parser module.** All knowledge of the JSON shape and of the CLI's text
  output lives in one module. If Claude Code changes its output, only that
  module and its fixtures change.
- **The UI thread never blocks.** Supervisors are transient, so any CLI call may
  first print `Starting background service…` and take seconds. All CLI calls
  run on worker threads that talk to the UI over channels. The UI renders the
  last good snapshot, marks it stale when a poll fails or times out, and shows
  the error in the status line. A poll is skipped while the previous one for
  the same account is still running, so calls never pile up. `attach` is the
  only call that runs in the foreground, because it takes over the terminal
  ([0005](0005-attach-and-detach.md)).
- **Workers always report back.** Each background job (dispatch, `stop`,
  `rm`, `logs`, hand-off) sends exactly one result. A job that panics, or
  whose thread cannot be started, reports an error of its own kind, so no
  dialog, hand-off or logs fetch is left waiting. Tower's panic hook restores
  the terminal only when the main thread panics; on any other thread it
  prints nothing and keeps the message for the status line.
- **Background calls in their own process group.** Every captured-output call
  (polls, `stop`, `rm`, `logs`, dispatch, hand-off resume) runs in a new
  process group, outside tower's foreground group, so `Ctrl+C` or `Ctrl+Z`
  typed into an attached session or the editor cannot kill or stop it. On a
  timeout tower kills the whole group with `SIGKILL` and then reaps the child,
  so a wrapper's own children (the real `claude` behind a script) die with
  it. A timed-out dispatch may still have started its session, so the error
  says to check the list and tower polls at once. Foreground commands with
  inherited stdio (attach, the trust flow, the editor) stay in tower's group.
- **Output caps.** Each call keeps at most the last 4 MiB of stdout and of
  stderr; older bytes are dropped. Once the result is taken, the pipes are
  still drained (a supervisor that inherited them never blocks on a full
  pipe) but the data is discarded.
- **One exception.** For a running session, tower reads `/proc/<pid>/environ`
  of the listed `pid` to find its `CLAUDE_CONFIG_DIR`, and so its account. This
  is process information from the operating system, not a Claude Code file. It
  is the only thing tower reads outside the CLI and its own files.
  [0017](0017-ownership-evidence-rules.md) widens it: tower also reads
  `/proc/<pid>/cmdline`, `/proc/<pid>/exe` and field 22 of `/proc/<pid>/stat`
  (no `btime`), to check that the process is the listed session's and not a
  reused pid.

Tower has no PTY handling, no process supervision, no tmux, no Agent SDK and no
direct API calls. It does not read, write or refresh credentials.

## Consequences

- Sessions keep running when tower exits, crashes or the terminal closes; tower
  holds no state that sessions depend on.
- Tower inherits Claude Code's behaviour, including its limits (no CLI for
  pinning, no line limit on `logs`, an idle timeout that cannot be configured).
- Data the JSON lacks, such as last-activity time and the account of a stopped
  session, is tracked by tower in its sidecar file
  ([0002](0002-tech-stack-and-files.md), [0006](0006-session-states-and-grouping.md)).
- Polling N accounts every 2 s costs N short process spawns per tick. That is
  acceptable for a handful of accounts.
- `/proc/<pid>/environ` is readable only for processes of the same user (or by
  root). If it can't be read, tower falls back to the sidecar.

## Alternatives considered

- **Own tmux or PTY management** (the claude-squad, agent-deck and ccmanager
  approach). It duplicates the supervisor Claude Code already has, and status
  detection relies on screen scraping, which breaks with Claude Code UI
  changes.
- **Reading `jobs/*/state.json` and other internal files.** They are faster and
  richer than the JSON, but internal, undocumented and explicitly not the
  supported interface. This would also blur the line set in
  [0016](0016-do-not-change-claude-code-defaults.md).
- **Agent SDK or the API directly.** That would mean running agents ourselves.
  The sessions would not show up in `claude agents`, could not be attached with
  the normal UI, and would bring their own credential handling.
- **Only using the built-in agent view.** It has no merged view across
  accounts, no hand-off, and its layout and keys are fixed. It stays available
  inside every attached session ([0005](0005-attach-and-detach.md)).

## Open items

- Whether `agents --json` itself starts a supervisor, and whether polling every
  2 s keeps idle supervisors from exiting. If it does, the poll interval for
  idle accounts may need to grow.
- Behaviour of `agents --json` while a supervisor is starting (empty list,
  error or delay).
