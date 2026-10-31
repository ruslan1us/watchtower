# Watchtower design decisions

Watchtower (`tower` on the command line) is a terminal UI for keeping track of
many parallel Claude Code sessions. It is a thin front end over Claude Code's
built-in background sessions: sessions are started with `claude --bg`, listed
with `claude agents --json`, and entered, inspected, stopped and removed with
`claude attach`, `claude logs`, `claude stop` and `claude rm`. Tower adds a
layout, keys and a workflow on top of that: one screen for every session across
several Claude Code accounts, a highlighted row when a session needs you, a
new-session dialog, a way to continue a session under another of your own accounts, and an
orchestrator session you can ask about the others. It never runs, supervises or
reconfigures agents itself.

This directory records the design decisions behind it, one file per decision.

## Index

| No. | Decision | Status |
|---|---|---|
| — | [Claude Code CLI behaviour Watchtower relies on](reference/claude-code-cli.md) | Reference |
| 0001 | [Thin wrapper over the Claude Code CLI](0001-thin-wrapper-over-claude-cli.md) | Accepted (`/proc` exception amended by 0025, subagent hooks by 0029, `git status` by 0037, skill and command files and `plugin list` by 0042, the usage probe by 0043; a PTY client for the workspace style by 0045) |
| 0002 | [Tech stack, config and state files](0002-tech-stack-and-files.md) | Accepted (config writing allowed by 0022, subagent files by 0029, templates folder by 0036; theme names and `[ui] color_depth` by 0038; `[usage]` and the `usage/` folder by 0043; `[ui] mouse`, `list_width`, `card_size` and `card_gap` by 0045; more theme names by 0046; `[ui] tab_title` and `pins` by 0047) |
| 0003 | [Multiple accounts and session ownership](0003-accounts-and-session-ownership.md) | Accepted (`rm` routing amended by 0034) |
| 0004 | [Continue a session under another account](0004-account-hand-off.md) | Accepted (empty prompt allowed by 0033) |
| 0005 | [Attaching to and detaching from a session](0005-attach-and-detach.md) | Accepted (input still waiting after a read re-arms crossterm's reader, 2026-10-28; the split view taken up for the workspace style only by 0045) |
| 0006 | [Session states, grouping and activity times](0006-session-states-and-grouping.md) | Accepted (rename wake amended by 0027; subagent lines added by 0029; busy rows outside `working` amended by 0032; header folders fitted to the width, 2026-10-21; the Pinned group by 0047) |
| 0007 | [Attention is visual only](0007-visual-only-attention.md) | Accepted (rename wake amended by 0027; the workspace style's clipboard writes and focus by 0045; the terminal's title by 0047) |
| 0008 | [New-session dialog](0008-new-session-dialog.md) | Accepted (`--settings` added by 0029; template key by 0036; slash-command suggestions by 0042; dispatch path shared with `tower dispatch` by 0044) |
| 0009 | [Stop, remove and the logs panel](0009-stop-remove-and-logs.md) | Accepted (emulator size amended by 0025; scrolling amended by 0028; fetching and replay amended by 0030; remove fallback added by 0034) |
| 0010 | [Orchestrator session](0010-orchestrator.md) | Accepted (`--settings` and `subagents` field added by 0029; role, rules and updating an unedited instruction file amended by 0044; `o` in the pane by 0045) |
| 0011 | [Keybindings](0011-keybindings.md) | Accepted (rename key added by 0027; logs panel keys amended by 0028; template key by 0036; quick reply key by 0040; `!` shell key by 0037; slash-command menu keys by 0042; `r` refreshes usage by 0043; the workspace's terminal keys, `h`, `z` and `F2` by 0045; `P`, `U`, `Ctrl+U` by 0047) |
| 0012 | [Static musl build and distribution](0012-build-and-distribution.md) | Accepted |
| 0013 | [No personal information in the repository](0013-privacy-and-repository-hygiene.md) | Accepted |
| 0014 | [Scope of v1 and non-goals](0014-v1-scope.md) | Accepted (usage display non-goal superseded by 0043; PTY handling for the workspace style by 0045; pinning in tower's own list by 0047) |
| 0015 | [Testing strategy](0015-testing-strategy.md) | Accepted |
| 0016 | [Never change Claude Code's own behaviour](0016-do-not-change-claude-code-defaults.md) | Accepted (observe-only `--settings` hooks allowed by 0029; the usage probe's flags by 0043) |
| 0017 | [Ownership evidence rules](0017-ownership-evidence-rules.md) | Accepted (`/proc` reads amended by 0025) |
| 0018 | [Account groups and the hand-off guard](0018-account-groups-and-handoff-guard.md) | Accepted (open item on limit claims resolved for display only by 0043) |
| 0019 | [Resume a past conversation into the background](0019-resume-past-sessions.md) | Accepted (`--settings` added by 0039) |
| 0020 | [Visual styles: dashboard and minimal](0020-visual-styles.md) | Accepted (colour section superseded by 0021; spinner rule amended by 0032; `git` line and four-line strip by 0037; minimal columns and window titles fitted to the width, 2026-10-21; values wrap, titles cut and a two-row key line by 0041; group separators and the status line fitted to the width, 2026-10-21; plan usage by 0043; the opt-in workspace style by 0045; the terminal's title by 0047) |
| 0021 | [Dashboard colour themes](0021-dashboard-themes.md) | Accepted (light themes, `ansi16` and the colour depth added by 0038; the workspace's selected card painted whole by 0045; published palettes by 0046) |
| 0022 | [Tower writes its own config](0022-tower-writes-its-own-config.md) | Accepted (`[sessions] track_subagents` added by 0029; `logs_follow_secs` by 0031; `[ui] color_depth` by 0038; `[usage]` by 0043; `[ui] card_size` and `card_gap` by 0045) |
| 0023 | [Self-update command](0023-self-update.md) | Accepted (release lookup amended by 0026) |
| 0024 | [Settings window](0024-settings-window.md) | Accepted (logs-follow row added by 0031; empty hand-off prompt by 0033; colour-depth row by 0038; detail-panel values wrap inside a padding column, 2026-10-21; usage rows by 0043; `workspace` among the styles, `card size` and `gap between cards` by 0045; the pick list scrolls with the cursor near its middle by 0046; the `terminal tab title` row by 0047) |
| 0025 | [The session terminal's real size for the logs replay](0025-session-terminal-size-for-logs.md) | Accepted |
| 0026 | [Latest release from the web redirect, the API as fallback](0026-latest-release-from-the-web-redirect.md) | Accepted |
| 0027 | [Rename a session](0027-rename-sessions.md) | Accepted (in the workspace's pane by 0045) |
| 0028 | [Scrolling the logs panel](0028-scrolling-the-logs-panel.md) | Accepted (paused panel amended by 0030; title name shortened before it goes, 2026-10-21; mouse capture in the workspace style by 0045) |
| 0029 | [Tracking the subagents of sessions tower starts](0029-tracking-subagents.md) | Accepted (continued subagents and the `stopped_agents` field amended by 0035; `R` gets `--settings` by 0039) |
| 0030 | [Following the logs stream across fetches](0030-continuous-logs-stream.md) | Accepted (interval and off switch amended by 0031) |
| 0031 | [The background logs-follow interval](0031-logs-follow-interval.md) | Accepted |
| 0032 | [A background task that outlives a finished turn](0032-background-task-after-a-finished-turn.md) | Accepted (status sentence wrapping amended by 0041) |
| 0033 | [Hand-off without a prompt](0033-hand-off-without-a-prompt.md) | Accepted |
| 0034 | [Remove with another account's command when the owner's service does not answer](0034-remove-with-another-account-when-the-service-is-down.md) | Accepted |
| 0035 | [A subagent continued with `SendMessage`](0035-continued-subagents.md) | Accepted |
| 0036 | [Prompt templates](0036-prompt-templates.md) | Accepted (picker also used by 0040) |
| 0037 | [Git summary in the details panel and a shell in the session's folder](0037-git-summary-and-shell.md) | Accepted (side-panel `git` wrapping, long-branch rule, zero counts and key line amended by 0041; `!` in the workspace's pane by 0045) |
| 0038 | [Light themes, a 16-colour theme and the colour depth](0038-light-and-16-colour-themes.md) | Accepted |
| 0039 | [Subagent hooks for conversations resumed with `R`](0039-subagent-hooks-for-resumed-conversations.md) | Accepted |
| 0040 | [Quick reply](0040-quick-reply.md) | Accepted (slash-command suggestions by 0042; wake path shared with `tower wake` by 0044) |
| 0041 | [Long values in the details panel wrap](0041-details-panel-values-wrap.md) | Accepted |
| 0042 | [Slash-command suggestions in the task field](0042-slash-command-suggestions.md) | Accepted (claude.ai synced skills added 2026-10-28) |
| 0043 | [Plan usage display](0043-plan-usage-display.md) | Accepted (per account on the workspace's Home and in its bar by 0045) |
| 0044 | [The orchestrator starts and wakes sessions when asked](0044-orchestrator-starts-and-wakes-sessions.md) | Accepted |
| 0045 | [Workspace style: Claude's terminal in a pane (first step)](0045-workspace-style.md) | Accepted (Home, group clicks, the wheel and cards, then the filled selected card, `card_gap`, attaches and the shell in the pane, the usage block and scroll hints, the pane's selection, the shell's history and zoom added 2026-10-31; tower's own title by 0047) |
| 0046 | [Colour themes from published palettes](0046-more-colour-themes.md) | Accepted |
| 0047 | [Attention in the tab title, pinned sessions, restart after an update](0047-tab-title-pinning-and-restart.md) | Accepted |

The reference document collects observed and documented Claude Code CLI
behaviour (commands, JSON fields, output formats, exit codes). Decision files
link to it instead of repeating those facts.

## How to add a decision

1. Take the next free number and create `NNNN-short-kebab-title.md` in this
   directory.
2. Use the template below. Keep it short (roughly 30–100 lines), technical, in
   the present tense.
3. Add a row to the index above.
4. Never rewrite the substance of an accepted decision. To change it, write a
   new decision and set the old one's status to `Superseded by NNNN`.

Statuses:

- `Proposed`: under discussion, may still change.
- `Accepted`: in force; code follows it.
- `Superseded by NNNN`: replaced by a later decision, kept for history.

Template:

```markdown
# NNNN. Title

- Status: Proposed · YYYY-MM-DD
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), …

## Context

What problem or constraint forces a decision. Facts, not opinions; link to
reference/claude-code-cli.md for CLI behaviour.

## Decision

What we do, stated plainly.

## Consequences

What follows from it, good and bad.

## Alternatives considered

What else was on the table and why it lost.

## Open items

Things still to verify or decide. Omit the section if there are none.
```

Content rules for every file here are in
[0013](0013-privacy-and-repository-hygiene.md): no personal names, e-mail
addresses, hostnames, real paths, real session ids or account names. Use the
example accounts `main` (`claude`, `~/.claude`) and `alt` (`claude-alt`,
`~/.claude-alt`) and made-up session values.
