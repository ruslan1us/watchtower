# 0048. First start: the welcome window and `tower setup`

- Status: Accepted · 2026-10-31
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md), [0045](0045-workspace-style.md), [reference](reference/claude-code-cli.md)
- Amends: [0002](0002-tech-stack-and-files.md) (what a missing file means to interactive `tower`), [0011](0011-keybindings.md) (the window's keys), [0022](0022-tower-writes-its-own-config.md) (how a missing file is created), [0024](0024-settings-window.md) (its state also carries the steps), [0045](0045-workspace-style.md) (preselected for a new user)

## Context

A new user had to copy `config.example.toml` by hand, list accounts and folders, and only then
start tower. Without the copy the new-session dialog has no projects and the look is the default,
which says little about what tower can do. The owner decided that a new user should not need to
copy anything, and that the window asks for what tower cannot guess.

## Decision

**When it opens.** Interactive `tower` with no config file at the path it uses
(`Origin::FirstRun`), and `tower setup` at any time (`Origin::Setup`). A file that exists, even an
empty one, never opens it for plain `tower`. `tower status`, `dispatch`, `wake`, `hook`, `update`,
`--help` and `--version` never open it and never write. `tower setup` needs a terminal, as the TUI
does. With a file that does not parse, both stop at the error they stop at today.

**The steps**, in one full-screen window in the window look of [0024](0024-settings-window.md):

1. **Welcome:** what tower is, Claude Code's version from `<command> --version` on a worker thread,
   and the sessions once the first listing is in. A command that cannot be run is said plainly,
   and the user can go on.
2. **Look:** style, `workspace` preselected and marked experimental; theme; card size, shown only
   in the workspace style. Each change previews at once, as in Settings.
3. **Projects:** the folders offered in the new-session dialog. Those of `~/projects`,
   `~/workspace`, `~/code`, `~/src`, `~/dev`, `~/repos`, `~/git` and `~/work` that exist are listed
   with their count of git clones, from `scan_root` on a worker thread. A first start ticks those
   that hold clones. The user can type another folder; `~` is allowed and kept, and it must exist.
   A typed folder holding clones goes to `project_roots`; any other goes to `extra_projects`, so
   the dialog offers it itself instead of finding nothing in it.
4. **Options:** plan usage (off), track subagents (on), tab title (on), a line on each.
5. **Done:** a summary and the file; `Enter` saves.

**Accounts are not edited** ([0022](0022-tower-writes-its-own-config.md)). One line says `main` runs
`claude` and points to the guide. Tower cannot infer a user's wrapper command, and a wrong one wakes
sessions under the wrong account.

**Keys.** `Enter` next, `Backspace` or `Shift+Tab` back, `↑↓` move, `←→` change, `Space` ticks.
`Esc` skips the rest and saves the current choices, so the window never comes back; in `tower
setup` it cancels without writing. `Ctrl+C` quits at once without asking and writes no config, so a
first start's window shows again. In the field for another folder keys are text (Cyrillic stays as
typed); `Enter` adds, `Tab` completes the name, `Esc` and `Ctrl+U` clear it, `↑` leaves it; with
nothing typed `Enter`, `Backspace` and `Esc` are the step's.

**Small terminals.** The steps need 80×24, the dashboard's minimum. Below it at the start no window
opens: a status-line notice, the defaults, nothing written, and the next start tries again. A
window that is already open shows a plain notice until the terminal grows; `Esc` there goes on
without writing.

**The file.** A first start writes `config.example.toml`'s full text, embedded in the binary, with
the chosen values set through the save of [0022](0022-tower-writes-its-own-config.md): under the
lock, validated, atomic, parent folders created, mode as for any new file, no backup since nothing
existed. `first_run_template` makes that text from the example so that every active value is the
built-in default: the example folders become `[]`, `main` stays active without a group, and the
`alt` account becomes a comment. A user without a `claude-alt` wrapper would otherwise get an
account whose polls fail. So saving changes no running account and needs no restart.

**`tower setup`** starts from the file's values, preselects nothing and saves only what changed,
through the same writer: comments, unknown keys, the one-time backup and symlinks as in 0022. A
value changed and changed back, also for a key not in the file, is no change. Without a file it
starts like a first start and writes the whole text, but `Esc` still cancels.

**After saving,** the running tower applies the config as after a Settings save and goes on to the
normal screen; below 100 columns the workspace style's fallback applies
([0045](0045-workspace-style.md)).

**If the write fails,** the window shows the error and goes on with the built-in defaults for this
run (in `tower setup`, the current settings), and the status line says so; a first start's window
comes back next time.

**A file that appears meanwhile** (another tower) is the conflict question of
[0024](0024-settings-window.md). `y` overwrites those keys; `n` in a first start closes the window
and keeps that file, which the next reload applies.

**The built-in default stays `dashboard`.** A config without `[ui] style` gets `dashboard`, and
`config.example.toml` keeps showing the built-in defaults, as `tests/config.rs` checks. `workspace`
comes from this window alone, written to the file as the user's choice.

**How it is built.** The window is the Settings window's state (`SettingsWindow`) with a `Welcome`
on top, so the draft, previews, save, conflict question and folder jobs are the same code. The first
start's hooks file ([0029](0029-tracking-subagents.md)) waits until a dispatch needs it, so `Ctrl+C`
leaves no settings of any kind. `state.json` is not a setting: polls behind the window record what
they see in it, as in any run ([0002](0002-tech-stack-and-files.md)).

## Consequences

- A new user gets a working config with their folders and every setting documented in the file,
  without copying anything. Writing the file by hand before the first start still works, and tower
  never replaces a file that exists.
- New users start in an experimental style, and see that it is one. A terminal below 100 columns
  gets the dashboard until it grows.
- `Esc` means two things: it saves in a first start and cancels in `tower setup`. Both match what
  the user is doing: skipping a question, or backing out of a change.
- A hung mount under one of the usual folders holds up that step's list, not the window: the folder
  job runs on one worker at a time (0024), and the typed folder waits behind it.
- `Esc` in the instant before the scan finishes saves no folders. They can be added with `c`.
- Nothing here touches Claude Code's settings ([0016](0016-do-not-change-claude-code-defaults.md)),
  logs in or starts a session; `--version` is the one new command.

## Alternatives considered

- **Clicks on the choices.** Tower captures the mouse only in the workspace's pane
  ([0045](0045-workspace-style.md)). Capturing it here would stop the user selecting the guide's path
  or the file's name with the terminal's own selection, in the one window that shows them for
  copying. Keys are enough for five short steps.
- **`Esc` cancels in both.** A first start would then come back each time for a user who wanted the
  defaults; the owner chose that skipping saves.
- **A short file with only the chosen keys.** That is what the Settings window writes, but a new user
  would never see the other settings or how to add an account.
- **Editing accounts here.** See above; a wrapper command cannot be guessed.
- **`dashboard` as the built-in default changes to `workspace`.** That would move every existing user
  whose config lacks the key to an experimental style without asking.
