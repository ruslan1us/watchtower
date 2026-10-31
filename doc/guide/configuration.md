# Configuration

The welcome window, the config file, the Settings window, and what each setting does. Back to the [README](../../README.md).

## The welcome window and `tower setup`

When `tower` starts and there is no config file, a welcome window opens
([0048](../0048-first-run-welcome.md)). Five steps: what tower found (Claude Code's version and
your sessions), the look (style, with `workspace` preselected, theme, and the card size in that
style), the project folders (the usual ones under your home that exist, with how many git clones
each holds; those with clones are ticked, and you can type another), three options (plan usage,
tracking subagents, the tab title), and a summary. The [keys](keys.md#welcome-window) are few:
`Enter` goes on, `Backspace` goes back, and `Esc` skips the rest and saves what you have chosen.

It writes the whole of [`config.example.toml`](../../config.example.toml) with its comments and
your values, with one active account, `main`, running `claude`, so more accounts are a matter of
editing the end of that file ([below](#accounts)). Then tower carries on with those settings.
`Ctrl+C` quits and writes nothing, so the window comes back. If the file cannot be written, the
window says so and tower runs on its defaults; on a terminal below 80×24 tower says so in the
status line and does the same. Commands such as `tower status` never open it.

`tower setup` opens the same window at any time, starting from the file's values. It saves only
what you change, the way the Settings window does, and `Esc` there cancels without writing.

## The config file

The config is `$XDG_CONFIG_HOME/watchtower/config.toml` (default
`~/.config/watchtower/config.toml`). Every key is optional; see
[`config.example.toml`](../../config.example.toml) for each key with its default, and
[0002](../0002-tech-stack-and-files.md) for the details. The example defines two
accounts: `main` (`claude`, `~/.claude`) and `alt` (`claude-alt`, `~/.claude-alt`).

To write the file by hand, copy the example there before the first start and edit it. Tower never
replaces a file that exists, so the welcome window then does not open.

## The Settings window

Press `c` in tower to change settings: the Settings window
([0024](../0024-settings-window.md)) edits the style, theme and colour depth
(previewed as you browse), the workspace's card size and gap, the new-session defaults, the project folders, the hand-off prompt and guard,
the poll interval, the background logs follow, subagent tracking and the plan usage display; `Ctrl+S` saves, `e` opens the file in `$VISUAL` / `$EDITOR`.
Accounts are shown read-only there; edit them in the file and restart tower. The window
saves through [0022](../0022-tower-writes-its-own-config.md): it writes only the keys you change
and keeps your comments and formatting; resetting a setting to its default removes the
key (its comment lines stay). Before its first write it copies the file once to
`config.toml.bak`, and a symlinked config is written through to its target. A running
tower reloads `config.toml` when it changes, whoever changed it: everything applies at
once except `[[accounts]]`, which need a restart (the status line says so). A file that
does not parse is not applied; the status line shows the error until it is fixed.

## Accounts

The welcome window never edits accounts; a file it wrote has `main` active and `alt` as a comment
to start from. Each account has a `name`, the `command` tower runs for it, and its
`config_dir`. `default_account` is used for new sessions and the orchestrator; the
new-session dialog lets you pick another one per session.

## Wrappers

`command` is run as-is. Tower never sets `CLAUDE_CONFIG_DIR` itself and
removes an inherited one from every command it runs (and warns when started with one set),
so a second account needs a wrapper that sets it, e.g. a `claude-alt` script on `PATH`:

```sh
#!/bin/sh
CLAUDE_CONFIG_DIR="$HOME/.claude-alt" exec claude "$@"
```

## `config_dir`

`config_dir` (default `~/.claude`) is how tower recognises which account hosts a
running session: it reads `CLAUDE_CONFIG_DIR` from the session process's
`/proc/<pid>/environ` and compares paths. Every account needs its own `config_dir`;
two accounts on the same directory are rejected.

## Shared lists

Accounts whose config dirs share session storage through symlinks list
the same sessions. Tower merges them by session id and tracks the owner itself
([0003](../0003-accounts-and-session-ownership.md)).

## Groups

An account may set `group`; accounts without one share one implicit group.
A hand-off offers only accounts in the same group as the session's owner. The top-level
`allow_cross_group_handoff` (default `false`) allows hand-offs across groups. While the
owner is unknown and the accounts span several groups, tower refuses the hand-off until
the owner is known ([0018](../0018-account-groups-and-handoff-guard.md)). With the
example accounts:

```toml
allow_cross_group_handoff = false

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal"
```

Here `a` refuses to move a session from `main` to `alt` and says which groups differ.

## Styles

`[ui] style` picks the look of the session list
([0020](../0020-visual-styles.md)). The welcome window preselects `"workspace"` and writes your
choice; a config without the key gets the default, `"dashboard"`, which draws rounded panels:
a header with a count pill per state and the accounts, the session list with section
separators, and a detail panel for the selected session (on the right from 130 columns,
a strip under the list below that; the logs panel takes its place while open). When the
session's folder is a git work tree, the panel has a `git` line with the branch, the
number of changed files and the commits not pushed yet, e.g.
`git  feature/login · 3 changed · ↑2`, or `git  main · clean` when there is nothing
to push or pull ([0037](../0037-git-summary-and-shell.md)). In the side panel long
names, folders and branches wrap onto the free rows; the strip cuts them with `…`
([0041](../0041-details-panel-values-wrap.md)). It needs
at least 80×24 and uses the plain layout on a smaller terminal. For the plain
single-column list, switch back to `"minimal"`:

```toml
[ui]
style = "minimal"
```

Colours follow `NO_COLOR` in both styles.

## Themes

`[ui] theme` picks the dashboard's colours
([0021](../0021-dashboard-themes.md)): `"neutral"` (the default, grey text with colour
only for states and attention), `"warm"` (warm greys, a clay-orange accent),
`"terminal"` (your terminal's own named colours, reverse video for the selection and
pills) or `"tokyo-night"` (the original blue-grey palette). On a light (white)
terminal background pick `"neutral-light"` or `"warm-light"`
([0038](../0038-light-and-16-colour-themes.md)); tower cannot tell a light
background from a dark one without querying the terminal, which it never does.
`"ansi16"` uses only the 16 ANSI colours plus bold, dim and reverse video, and reads
on dark and light backgrounds alike. The minimal style keeps its own look, and
`NO_COLOR` turns colour off under every theme.

Ten published palettes are mapped to tower's roles
([0046](../0046-more-colour-themes.md), with their sources, licences and credit):
`"catppuccin-mocha"`, `"dracula"`, `"gruvbox-dark"`, `"kanagawa"`, `"nord"`,
`"one-dark"`, `"rose-pine"` for a dark background, and `"catppuccin-latte"`,
`"gruvbox-light"`, `"one-light"` for a light one. Tower never paints the background:
for the palette's own look, set your terminal's background to the palette's. On plain
black under a dark palette, or white under a light one, they stay readable; a dark
palette on a light terminal is not. A few values are deeper or lighter than published
to stay readable, most in `"catppuccin-latte"`.

```toml
[ui]
theme = "neutral-light"
```

## Colour depth

`[ui] color_depth` says how many colours the terminal shows
([0038](../0038-light-and-16-colour-themes.md)). The default `"auto"` reads the
environment and never asks the terminal: `COLORTERM=truecolor` (or `24bit`) means full
colour; otherwise only a `TERM` known to be limited (unset, `dumb`, `linux`, `vt100`
and the other `vt*`, `ansi`, `cons25`, `cygwin`, `sun`, `xterm-color`, `*-16color`)
means 16 colours, where the themes give way to `"ansi16"` (`"terminal"` and `"ansi16"`
stay) and the status line says so once. Every other `TERM` (`xterm`, `screen`,
`tmux`, `*-256color`, also over SSH) draws in full colour as before. Set
`"256"` to have the themes drawn with the nearest of the 256 colours on a terminal that
shows no more, or `"truecolor"` / `"16"` to override the detection:

```toml
[ui]
color_depth = "256"   # or "truecolor", "16"
```

## Tab title

Tower sets the terminal's window or tab title in every style: `tower`, or
`tower · 2 need you` while sessions need attention, so it shows from another tab
([0047](../0047-tab-title-pinning-and-restart.md)). It is written only when the text changes,
comes from tower alone, and the previous title comes back when tower hands the terminal over or
exits. `[ui] tab_title = false` (also in Settings) leaves the title alone.

## Background logs follow

Tower fetches `claude logs` of every working session, and
of the session you are attached to, every `logs_follow_secs` seconds (default `10`), so
the logs panel keeps the whole screen however long a session works
([0030](../0030-continuous-logs-stream.md)). It may be `5` to `30`: a capture covers
only about the last 40 seconds of a working session, so a longer interval loses the
overlap and the screen breaks up again. `0` turns these fetches off; the open logs
panel still fetches on every poll, and tower still fetches once right after it starts a
session or an attach returns ([0031](../0031-logs-follow-interval.md)).

```toml
logs_follow_secs = 0
```

## Plan usage

`[usage] show = true` turns on the [plan usage](plan-usage.md) display;
`[usage] refresh_secs` (default `300`, `60` to `3600`) is the time between one
account's probes.

```toml
[usage]
show = true
refresh_secs = 600
```
