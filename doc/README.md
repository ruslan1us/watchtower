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
| 0001 | [Thin wrapper over the Claude Code CLI](0001-thin-wrapper-over-claude-cli.md) | Accepted (`/proc` exception amended by 0025) |
| 0002 | [Tech stack, config and state files](0002-tech-stack-and-files.md) | Accepted (config writing allowed by 0022) |
| 0003 | [Multiple accounts and session ownership](0003-accounts-and-session-ownership.md) | Accepted |
| 0004 | [Continue a session under another account](0004-account-hand-off.md) | Accepted |
| 0005 | [Attaching to and detaching from a session](0005-attach-and-detach.md) | Accepted |
| 0006 | [Session states, grouping and activity times](0006-session-states-and-grouping.md) | Accepted (rename wake amended by 0027) |
| 0007 | [Attention is visual only](0007-visual-only-attention.md) | Accepted (rename wake amended by 0027) |
| 0008 | [New-session dialog](0008-new-session-dialog.md) | Accepted |
| 0009 | [Stop, remove and the logs panel](0009-stop-remove-and-logs.md) | Accepted (emulator size amended by 0025; scrolling amended by 0028; fetching and replay amended by 0030) |
| 0010 | [Orchestrator session](0010-orchestrator.md) | Accepted |
| 0011 | [Keybindings](0011-keybindings.md) | Accepted (rename key added by 0027; logs panel keys amended by 0028) |
| 0012 | [Static musl build and distribution](0012-build-and-distribution.md) | Accepted |
| 0013 | [No personal information in the repository](0013-privacy-and-repository-hygiene.md) | Accepted |
| 0014 | [Scope of v1 and non-goals](0014-v1-scope.md) | Accepted |
| 0015 | [Testing strategy](0015-testing-strategy.md) | Accepted |
| 0016 | [Never change Claude Code's own behaviour](0016-do-not-change-claude-code-defaults.md) | Accepted |
| 0017 | [Ownership evidence rules](0017-ownership-evidence-rules.md) | Accepted (`/proc` reads amended by 0025) |
| 0018 | [Account groups and the hand-off guard](0018-account-groups-and-handoff-guard.md) | Accepted |
| 0019 | [Resume a past conversation into the background](0019-resume-past-sessions.md) | Accepted |
| 0020 | [Visual styles: dashboard and minimal](0020-visual-styles.md) | Accepted (colour section superseded by 0021) |
| 0021 | [Dashboard colour themes](0021-dashboard-themes.md) | Accepted |
| 0022 | [Tower writes its own config](0022-tower-writes-its-own-config.md) | Accepted |
| 0023 | [Self-update command](0023-self-update.md) | Accepted (release lookup amended by 0026) |
| 0024 | [Settings window](0024-settings-window.md) | Accepted |
| 0025 | [The session terminal's real size for the logs replay](0025-session-terminal-size-for-logs.md) | Accepted |
| 0026 | [Latest release from the web redirect, the API as fallback](0026-latest-release-from-the-web-redirect.md) | Accepted |
| 0027 | [Rename a session](0027-rename-sessions.md) | Accepted |
| 0028 | [Scrolling the logs panel](0028-scrolling-the-logs-panel.md) | Accepted (paused panel amended by 0030) |
| 0030 | [Following the logs stream across fetches](0030-continuous-logs-stream.md) | Accepted |

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
