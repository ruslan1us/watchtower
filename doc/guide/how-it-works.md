# How it works

How tower sits on top of Claude Code, what it adds and leaves out, where it keeps its files, and its known limitations. Back to the [README](../../README.md).

Watchtower (`tower`) is a terminal UI for keeping track of many parallel Claude Code
background sessions. It is a thin front end over Claude Code's own commands: sessions are
started with `claude --bg`, listed with `claude agents --json --all`, and entered,
inspected, stopped and removed with `claude attach`, `logs`, `stop` and `rm`. Sessions keep
running when tower exits; tower holds nothing they depend on.

## What tower adds

What it adds on top:

- **Several accounts in one view.** Every configured Claude Code account (each its own
  `CLAUDE_CONFIG_DIR`) is polled and merged into one list, grouped by state (Orchestrator,
  pinned at the top, then Needs me, Failed, Ready for review, Working, Stopped & finished,
  Interactive) or by project.
- **Hand-off between your own accounts.** `a` stops a session and continues it under another
  of your own accounts with the same history and project context, after asking. By default
  it stays inside the owner's account group ([Accounts and terms](accounts-and-terms.md)).
- **An orchestrator session.** `o` opens an ordinary background session named
  `orchestrator` that you can ask what your sessions are doing, and, when you ask and
  confirm, to start or wake a session through tower ([The orchestrator](orchestrator.md#the-orchestrator)).
- **Visual-only attention.** A session that starts waiting for you or fails gets a `●`
  marker and bold text until you attach or mark it seen, and the terminal's tab title says
  `tower · 2 need you`, so it shows from another tab. No bell, sound or desktop notifications.
- **Pinned sessions.** `P` keeps a session in its own group at the top, whatever its state
  ([Pinning](sessions.md#pinning-a-session)).
- **Restart after an update.** `U` restarts a session onto the installed Claude Code, `Ctrl+U`
  every running one, each under its own account ([Restarting](sessions.md#restarting-after-an-update)).
- **Quick reply.** `t` on a session waiting for review sends it a text (typed, or a
  [prompt template](sessions.md#prompt-templates)) as your next prompt, under its own account and id,
  without attaching ([Quick reply](sessions.md#quick-reply)).
- A new-session dialog, a logs panel that replays `claude logs`, and a text filter.
- **Slash-command suggestions.** Typing `/` at the start of a task or a quick reply lists
  your skills and custom commands (personal, the project's, synced from claude.ai, enabled
  plugins') with their descriptions and argument hints, as Claude Code's own prompt does
  ([Slash commands](sessions.md#slash-commands)).
- **Subagents under their session.** A working session tower started shows its running
  subagents as a dimmed tree under its row, with a `+N` badge ([0029](../0029-tracking-subagents.md)). A subagent
  whose command runs longer than about 3 minutes leaves that count: Claude Code moves the command
  to the background and reports the subagent as stopped.
- **Plan usage, if you turn it on.** With `[usage] show = true` the header shows the
  numbers Claude Code's own `/usage` reports after each account (`5h 24% · 7d 12%`), and
  the details panel when they reset. Off by default and for your information only
  ([Plan usage](plan-usage.md)).
- **A workspace style, if you turn it on.** `[ui] style = "workspace"` shows sessions as cards on
  the left and the selected session's real Claude Code terminal on the right, with mouse support,
  zoom and a shell pane ([Workspace style](workspace-style.md)).
- **A dashboard look by default:** count pills per state, the list in panels, and a detail
  panel for the selected session; `[ui] style = "minimal"` gives the plain list
  ([Configuration](configuration.md)), and the experimental `"workspace"` shows Claude's real
  terminal beside the list ([Workspace style](workspace-style.md)).

## What it does not do

Tower never runs, supervises or reconfigures agents. It has no PTY handling, no tmux, no
Agent SDK and no direct API calls, and it does not read Claude Code's internal files
([0001](../0001-thin-wrapper-over-claude-cli.md)); for the slash-command suggestions it
only reads the heads of your own skill and command files and asks `claude plugin list
--json` which plugins are enabled ([0042](../0042-slash-command-suggestions.md)). It never changes Claude
Code's settings, keybindings or defaults ([0016](../0016-do-not-change-claude-code-defaults.md)).
The one thing it adds to the sessions it starts is a `--settings` file with three
observe-only hooks that record their running subagents in tower's own files; turn it off
with `[sessions] track_subagents = false` ([0029](../0029-tracking-subagents.md)).
It never reads, stores or passes on credentials or session tokens, never runs
`claude setup-token`, never proxies or routes requests, and has no hosted, shared or
multi-user mode: it is a single-user local tool around the unmodified official `claude`
binary. The quick reply (`t`) sends a session a text without attaching, and the plan
usage display is opt-in and only shows what Claude Code reports
([0043](../0043-plan-usage-display.md)). There is no automatic or limit-based hand-off,
no sorting, suggestion or action based on usage, and no chat or diff views. `P` pins in tower's
own list only; Claude Code's own pin has no CLI.
The full list of non-goals is in [0014](../0014-v1-scope.md).

## Where state lives

Next to the config, in `~/.config/watchtower/`:

- `state.json`: what the CLI does not expose: each session's owning account, tracked
  state-change times, acknowledged highlights, pinned sessions, recent projects. Written atomically; losing
  it loses only conveniences, and a corrupt one is moved aside as `state.json.corrupt`.
- `state.json.lock`: the lock that lets several tower instances share `state.json`.
- `config.toml.bak`: a one-time copy of your config, made before tower first writes it;
  `config.toml.lock`: the lock held while tower saves the config
  ([0022](../0022-tower-writes-its-own-config.md)).
- `orchestrator/`: the orchestrator's folder.
- `templates/`: your prompt templates; tower creates the empty folder and only reads
  the files
  ([0036](../0036-prompt-templates.md)).
- `usage/`: the empty folder the [plan usage](plan-usage.md) probe runs in, created
  (mode `0700`) when the display is on ([0043](../0043-plan-usage-display.md)).

## Known limitations

- Linux x86_64 only.
- Cross-account messaging is unverified: the orchestrator may only reach sessions hosted
  by its own account ([0010](../0010-orchestrator.md)).
- Claude Code stops idle sessions (the orchestrator too) after about an hour; they wake,
  more slowly, on the next attach.
- `Ctrl+Enter` is not reported by many terminals, especially over SSH; use `Alt+Enter`.
- The tab title comes back through the terminal's own title stack, so a terminal without one
  keeps `tower` until your shell's prompt sets a title, and a dropped SSH connection can leave a
  count behind. Only tmux was tested; Warp and Windows Terminal were not. `[ui] tab_title = false`
  turns it off ([0047](../0047-tab-title-pinning-and-restart.md)).
- Keys are fixed. Open questions are listed in each decision in [`doc/`](../README.md).
