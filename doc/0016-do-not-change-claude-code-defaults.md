# 0016. Never change Claude Code's own behaviour

- Status: Accepted · 2026-10-18 · observe-only subagent hooks with `--settings` allowed by [0029](0029-tracking-subagents.md)
- Related: [reference](reference/claude-code-cli.md), [0001](0001-thin-wrapper-over-claude-cli.md), [0005](0005-attach-and-detach.md), [0008](0008-new-session-dialog.md), [0009](0009-stop-remove-and-logs.md), [0011](0011-keybindings.md)

## Context

Tower sits above Claude Code. The user also runs Claude Code directly, in other
terminals and inside sessions attached from tower. Several tower conveniences
would be easier if Claude Code behaved slightly differently: a different detach
key, edits in place instead of worktrees, no permission prompts. Each of those
would silently change how Claude Code behaves everywhere, not just under tower,
and would make tower's behaviour depend on settings it had changed.

## Decision

Tower builds on top of Claude Code and does not change its logic, settings or
defaults.

- Tower never writes `~/.claude.json`, `settings.json`, `keybindings.json` or
  any other Claude Code file of any account.
- Tower never passes `--settings` or environment variables that change agent
  behaviour.
- The only flags tower passes are per-session invocation options the user picks
  in the new-session dialog (`--name`, `--model`, `--effort`,
  `--permission-mode`) and the documented options for an action (`--bg`,
  `--resume`, `--json`, `--all`). Tower never passes the `rm` flags that discard
  work ([0009](0009-stop-remove-and-logs.md)). A choice of "default" omits the
  flag.
- *Amended by [0029](0029-tracking-subagents.md):* on the sessions it starts,
  tower also passes `--settings` with a file of its own that holds only three
  observe-only hooks (`SubagentStart`, `SubagentStop`, `PostToolUse` of
  `Agent`). They run `tower hook`, which exits 0 and prints nothing, so agent
  behaviour does not change; they are combined with the user's own hooks, and
  no Claude Code file is written. `[sessions] track_subagents = false` turns
  this off. Nothing else is ever passed with `--settings`.
- When Claude Code itself is configured differently by the user, tower respects
  that and reports CLI errors as they come.

Concrete things tower does not do:

- Set `leftArrowOpensAgents` to stop `←` opening agent view. `Ctrl+Z` is the
  documented way back to tower instead ([0005](0005-attach-and-detach.md)).
- Disable agent view (`disableAgentView` or `CLAUDE_CODE_DISABLE_AGENT_VIEW`).
  That also disables `claude attach`, which then fails with exit code 1.
- Set `worktree.bgIsolation`. Background sessions keep their own git worktree,
  and agents decide on commits, pushes and pull or merge requests as usual.
- Pass `--dangerously-skip-permissions` or `--permission-mode bypassPermissions`
  by default. The default mode is `auto`
  (https://code.claude.com/docs/en/permission-modes); bypass is refused anyway
  when Claude Code runs as root.
- Rebind Claude Code keys, or add a detach binding (none exists).
- Pin sessions. Pinning exists only as a key in agent view, with no CLI.
- Change or work around the idle stop after about an hour; it can't be
  configured anyway.
- Log in, refresh tokens or touch credentials.
- Run `claude setup-token`, proxy or route a session's requests, or offer a
  hosted, shared or multi-user mode. Each session is the unmodified official
  binary under the user's own login (hard rules in
  [0001](0001-thin-wrapper-over-claude-cli.md)).
- Accept workspace trust on the user's behalf. Tower can open interactive
  Claude Code in the folder so the user accepts trust in Claude Code's own
  dialog ([0008](0008-new-session-dialog.md)).

## Consequences

- An attached session behaves exactly like one started by hand; nothing has to
  be relearned.
- Tower can be removed at any time and leaves no changed configuration behind.
- Some rough edges stay: `←` on an empty prompt opens agent view instead of
  returning to tower (`Esc` then returns), idle sessions get stopped and woken,
  and worktrees appear under `.claude/worktrees/`.
- Tower's own keys avoid the ones Claude Code uses while attached
  ([0011](0011-keybindings.md)).

## Alternatives considered

- **Change `leftArrowOpensAgents` in the global config.** It would make `←`
  behave differently in every Claude Code session, not only tower's.
- **Per-session `--settings` overrides.** They are less invasive than global
  edits, but still change agent behaviour invisibly, and some keys, including
  `leftArrowOpensAgents`, can't be passed that way.
- **Edit in place (`bgIsolation: none`) to simplify the project view.** It
  breaks parallel sessions on the same repository, which rely on separate
  worktrees.
