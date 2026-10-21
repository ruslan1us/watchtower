# 0037. Git summary in the details panel and a shell in the session's folder

- Status: Accepted · 2026-10-21
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0005](0005-attach-and-detach.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md)
- Amends: [0001](0001-thin-wrapper-over-claude-cli.md) (what tower reads outside the CLI), [0011](0011-keybindings.md) (the `!` key), [0020](0020-visual-styles.md) (detail panel content, strip height)

## Context

The owner asked for two things (2026-10-21): to see the git state of the
selected session's folder without leaving tower, and to get a shell there
quickly, e.g. to look at a diff, run the tests or commit what a session did.

Every session row has a `cwd` (background and interactive rows alike).
[0001](0001-thin-wrapper-over-claude-cli.md) keeps tower away from Claude
Code's internal files and says `/proc` is the only thing tower reads
outside the CLI and its own files. A folder's git state is not Claude
Code's: it belongs to the user's project, and `git status --porcelain=v2`
is git's documented, stable machine format.

## Decision

**The `git` line.** For the selected session row, the dashboard's details
panel shows one line, e.g. `git       feature/login · 3 changed · ↑2`:

- the branch, or `detached <short commit>` (7 characters);
- the changed files: staged, unstaged, unmerged and untracked entries
  (porcelain lines `1`, `2`, `u` and `?`), each file once; ignored files do
  not count; `clean` when there are none;
- the commits not pushed to the upstream, `↑N`, with ` ↓M` added when the
  upstream has commits the branch lacks; `no upstream` without one,
  `upstream gone` when the configured upstream ref no longer exists.

The side panel shows it under `folder`. The strip under the list (80–129
columns, from 26 rows) has four lines now, the fourth being the `git` line
across the full width; it stays empty when there is no summary, so the
layout does not jump when the selection moves. A long branch name is cut
with `…` first, so the counts stay readable; control characters are drawn
as `?`. Group headers, the minimal style, the dashboard's minimal fallback
below 80×24 and an open logs panel show no `git` line. The line is omitted
when the folder is not inside a git work tree, git is missing, the folder
is gone, or the call fails, times out or prints something else. The colours
are the theme's label and text roles ([0021](0021-dashboard-themes.md)).

**How git runs.** One call:
`git --no-pager --no-optional-locks -C <cwd> status --porcelain=v2 --branch`,
with `GIT_OPTIONAL_LOCKS=0` and `GIT_TERMINAL_PROMPT=0`, stdin null and
output captured, in a process group of its own, killed with its group after
2 s, like every background call ([0001](0001-thin-wrapper-over-claude-cli.md)).
It runs on a worker thread that reports exactly once; a worker that panics
reports the folder as unavailable. The UI thread never waits for it.

**When.** The renderer notes on every frame whether it drew a details
panel. On each turn of the event loop, while it did, tower asks for the
selected row's folder (`cwd` as listed, the cache key) unless that folder
has a result younger than 5 s or a job running. So a new selection is
asked at once and a selected folder is read again about every 5 s. At most
one job runs per folder and at most three in all. A refresh keeps showing
the previous result until the new one arrives. Above 64 folders, results
older than 5 s are dropped.

**Why the git CLI is fine under 0001.** [0001](0001-thin-wrapper-over-claude-cli.md)
forbids Claude Code's internal, undocumented files because they can change
in any release and are not the supported interface. Git's porcelain v2 is
the supported interface of a different tool, about the user's own project.
The call is read-only (`--no-optional-locks`: no index refresh is written,
so it never competes with a session's own git commands for `index.lock`),
never prompts and never pages. It touches no session and no Claude Code
state ([0016](0016-do-not-change-claude-code-defaults.md)). Tower still
reads no Claude Code file.

**The shell: `!`.** On a session row with a `cwd` (background, live or
stopped, interactive, the orchestrator), `!` hands the terminal over
exactly like an attach ([0005](0005-attach-and-detach.md),
`attach::run_foreground`: input paused and drained, modes popped, the
alternate screen left, the no-op `SIGINT`/`SIGQUIT`/`SIGTSTP` handlers,
`WUNTRACED` with `SIGCONT`, the saved terminal settings put back, the key
pause after a non-zero exit or a signal, then raw mode and a redraw) and
runs `$SHELL` (`/bin/sh` when unset or empty) with no arguments, its
working directory the row's `cwd`. On the terminal the shell is
interactive by itself. Its environment is tower's minus `CLAUDE_CONFIG_DIR`,
as for every command tower runs ([0017](0017-ownership-evidence-rules.md));
nothing is added.

- Before the hand-over tower prints one line, its control characters as
  `?`: `tower: shell in ~/workspace/api — type exit to return to tower`.
  A failure prompts `tower: shell exited with code N. Press any key to
  return to tower.` (or `… was ended by a signal …`).
- An interactive shell with job control puts itself into a process group of
  its own and makes it the terminal's foreground group, and gives the
  terminal back when it exits. One that dies abnormally (e.g. `kill -9 $$`)
  does not, so right after the shell has been reaped tower makes its own
  group the foreground group again (`tcsetpgrp` with `SIGTTOU` blocked; a
  no-op otherwise), before anything else touches the terminal.
- When the shell exits, tower takes the terminal back, polls at once and
  says `back from the shell in <folder>` (or the exit code or signal; an
  error when it could not run). `SIGTERM` and `SIGHUP` act as during an
  attach.
- Whether the folder still exists is checked right before the hand-over;
  when it does not, nothing runs, the terminal is not released and the
  status line says `<folder> no longer exists; no shell started`. A row
  without a `cwd` says `<name> has no folder`; a group header says to
  select a session.

**Key.** `!` is free in the list and in every window
([0011](0011-keybindings.md)); text fields take it as a character and the
Cyrillic mapping leaves it alone. `t` stays reserved for another feature.
The footer shows `! shell` after `e rename`, the details panel's key line
`! shell` when it has room, and the help line `!, c, r, ?` reads `shell in
the folder; settings; refresh now; help`, so the help still fits 24 rows.

## Consequences

- While a repository row is selected and the panel is visible, tower runs
  one short `git status` about every 5 s; nothing when the panel is hidden.
- A very large repository, or one whose `git status` takes longer than 2 s
  (slow disk, network mount), shows no line; tower never waits for it.
- The strip is one row taller, so the list loses a row on 80–129 columns.
- A git that prints its messages in another language is still handled:
  whatever is not a summary is omitted.
- The shell is the user's own: anything typed there, e.g. `claude`, runs
  as it would in any terminal (a plain `claude` shows up as an interactive
  row). Tower does not wake, stop or change the session.

## Alternatives considered

- **`git rev-parse` + `git status --porcelain` + `git rev-list` separately.**
  Three processes per refresh; porcelain v2 with `--branch` gives the branch,
  the upstream and the ahead/behind counts in one.
- **A git library (libgit2, gitoxide).** A large dependency for one line, and
  it would read the repository files itself instead of asking git.
- **Refreshing on every poll for every row.** Many processes for rows nobody
  looks at; only the selected folder is asked.
- **Running the shell in a new terminal window or tmux pane.** Out of scope
  ([0001](0001-thin-wrapper-over-claude-cli.md) has no PTY or tmux
  handling); the hand-over is already there.
- **Passing `-i` to the shell.** Not needed on a terminal, and not every
  shell takes it.
