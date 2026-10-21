# 0019. Resume a past conversation into the background

- Status: Accepted · 2026-10-19 · `--settings` with the subagent hooks added by [0039](0039-subagent-hooks-for-resumed-conversations.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0008](0008-new-session-dialog.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [reference](reference/claude-code-cli.md)

## Context

Tower lists what `claude agents --json --all` returns: live sessions and
background sessions that have not been removed. A conversation that ended,
an interactive one the user closed, or a background one removed with
`claude rm`, is no longer listed, although its transcript stays on disk
and Claude Code can resume it
([reference](reference/claude-code-cli.md#cli-commands)). Tower may not read
transcripts or any other internal file to find such conversations
([0001](0001-thin-wrapper-over-claude-cli.md)).

Claude Code already has the tool for this: `claude --resume` with no
argument opens its interactive session picker in the current folder
(https://code.claude.com/docs/en/sessions). `Ctrl+A` there widens the list to
every project, `Ctrl+W` to every worktree of the repository. Picking a
conversation of a background session that is still running attaches to it
instead of loading a copy. Inside the resumed conversation, `←` on an empty
prompt (or `/bg`, documented) moves it into the background, where it keeps
running under the supervisor of the account that runs it
(https://code.claude.com/docs/en/agent-view).

Verified with Claude Code 2.1.303 on an ended conversation picked in the
picker:

- `←` on an empty prompt backgrounds it and opens agent view with the line
  "Your conversation moved to the background — enter opens it · esc returns
  to it · ctrl+c twice quits".
- `Esc` there reopens the conversation (it may first say "Still starting —
  try again in a moment"); it does **not** leave Claude Code. This differs
  from `claude attach`, where `←` then `Esc` returns to the caller
  ([0005](0005-attach-and-detach.md)).
- `Ctrl+C` twice leaves agent view: the `--resume` child ends and tower takes
  the terminal back.
- Backgrounding a resumed conversation starts a **copy under a new session
  id**: the background process runs with `--session-id <new> --fork-session
  --resume <transcript>`. The original ended conversation stays where it was.
- Tower then lists the copy as an ordinary background row under the account
  whose command ran `--resume`.

## Decision

`R` (Shift+R) on the session list opens a small window with two fields, built
from the new-session dialog's widgets ([0008](0008-new-session-dialog.md)):

| Field | Behaviour |
|---|---|
| Project | The dialog's project list (scanned on a worker thread, type to filter, `↑` / `↓`). Preselected: the selected row's `cwd`, or the listed project that contains it (a background session's worktree lies inside its project); else the first, most recently used, project. |
| Account | The configured accounts. Preselected: the selected row's owner when known, else `default_account`. `←` / `→` (or `↑` / `↓`) change it. |

`Tab` / `Shift+Tab` switch fields, `Enter` confirms, `Esc` cancels and
`Ctrl+C` quits as in the list. `Enter` without a project (still scanning,
none found, none matching) shows why and does nothing.

On `Enter` tower hands the terminal over exactly like an attach
([0005](0005-attach-and-detach.md): release, no-op signal handlers, saved
terminal settings put back, key pause on a non-zero exit, reclaim) and runs
`<account command> --resume` in the chosen folder, with no other argument.
*Amended by [0039](0039-subagent-hooks-for-resumed-conversations.md):* while
subagent tracking is on, `--settings <config dir>/claude-settings.json` goes
before `--resume`, so the backgrounded copy reports its subagents; the printed
line and the note below are unchanged.
Before handing over it prints one sanitised line: where it runs, and "pick a
conversation (Ctrl+A shows all projects); then press ← on an empty prompt
and Ctrl+C twice to come back to tower (Esc there reopens the conversation)
— it keeps running in the background as a copy and appears in tower". The
resume window's note says the same. `/bg` is left out of both because it is
untested in this flow.

When the child exits, tower takes the terminal back, reports how it ended
and polls every account at once. A conversation the user moved to the
background shows up as an ordinary background row, under the new session
id of the copy Claude Code started; its owner is found by the
usual `/proc` detection ([0003](0003-accounts-and-session-ownership.md),
[0017](0017-ownership-evidence-rules.md)), since it runs under the account
whose command ran the picker. If the user leaves without backgrounding,
nothing changes.

Trust: in a folder the account has not trusted, Claude Code shows its own
trust prompt interactively; tower does nothing special
([0016](0016-do-not-change-claude-code-defaults.md)).

`Ctrl+Z`: inside a plain interactive `claude`, `Ctrl+Z` may suspend it.
Tower waits with `WUNTRACED` and resumes the child's process group with
`SIGCONT` ([0005](0005-attach-and-detach.md)), so the picker or the resumed
conversation comes straight back instead of hanging. A child that stops
again right after being resumed is resumed at most once every 100 ms, so
tower never spins on it. `Ctrl+Z` therefore does not return to tower here;
`←` then `Ctrl+C` twice does, and so does `/exit`.

## Consequences

- The backgrounded conversation is a fork with a new session id; the ended
  original is not changed. Tower shows only the copy, since only that one is
  listed.
- Ended conversations become reachable without tower knowing anything about
  transcripts; the list, search and preview are Claude Code's own.
- Tower cannot tell in advance which conversations exist, and does not know
  which one was picked: the only result is the next listing.
- Picking a conversation from an unrelated project after `Ctrl+A` makes
  Claude Code copy a `cd` and resume command to the clipboard instead of
  resuming it (documented). Choosing the right folder in tower's window
  avoids this.
- A conversation resumed under another account than the one it ran under
  continues under the account of the command, as any resume does
  ([reference](reference/claude-code-cli.md#multiple-config-dirs-accounts)).
  The window shows which account that is.
- The permission mode of a conversation picked in the picker follows
  Claude Code's rules for the picker, not the mode it ended in
  (https://code.claude.com/docs/en/sessions). Tower passes no flag (only the
  subagent hooks' `--settings`, [0039](0039-subagent-hooks-for-resumed-conversations.md)).
- Users who turned `leftArrowOpensAgents` off use `/bg` (documented,
  untested here); tower changes no setting.

## Alternatives considered

- **Reading transcripts to list ended conversations in tower.** Rejected:
  internal files with no published format
  ([0001](0001-thin-wrapper-over-claude-cli.md)).
- **A typed session id or name, run as `<account command> --resume <id>`.**
  Possible later; it needs the id from somewhere, and the picker already
  searches by name and title.
- **`claude --resume --bg` without the picker.** Needs a session id; see the
  previous item.

## Resolved

- How to return to tower after a resume (verified, Claude Code 2.1.303): `←`
  on an empty prompt, then `Ctrl+C` twice in agent view ends the `--resume`
  child and tower is back; its status line starts "back from claude --resume
  in …". `Esc` there reopens the conversation instead. The backgrounded copy
  is listed under the account whose command ran `--resume`.

## Open items

- Whether `/bg` in a resumed conversation behaves like `←` (backgrounds a
  copy under a new id) and how agent view is left afterwards.
- Whether `Ctrl+Z` in a plain interactive `claude` suspends it at all in the
  Claude Code version in use; tower handles either case.
