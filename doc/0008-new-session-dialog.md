# 0008. New-session dialog

- Status: Accepted · 2026-10-18
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [reference](reference/claude-code-cli.md)

## Context

Starting work means: pick a project, describe the task, choose the account, model, effort
and permission mode, and start a background session there. Claude Code does this with
`claude --bg` run in the project directory
([reference](reference/claude-code-cli.md#cli-commands)). Most projects are git clones
under a few root folders (e.g. `~/workspace`), with a handful elsewhere. Workspace trust is
stored per account, and `--bg` cannot show the trust dialog. A folder covered by a trusted
parent folder was observed to lose that cover once it became a git repository, so each git
clone may need its own trust acceptance, per account
([reference](reference/claude-code-cli.md#permissions-and-trust)). `--bg` does not validate
`--model`: an unknown name exits 0 and the session then turns `failed`.

## Decision

`n` on the overview opens a dialog with these fields:

| Field | Behaviour |
|---|---|
| Project | List of git repos found one level deep under each `project_roots` entry, plus every `extra_projects` entry. Most-recently-used first (`recent_projects` in the sidecar, [0002](0002-tech-stack-and-files.md)), then alphabetical. Type to filter. |
| Task | Multi-line text. `Ctrl+G` suspends tower and opens the task in `$VISUAL`, else `$EDITOR`; the saved text replaces the field. Must be at least 4 characters; the dialog validates this. The minimum is documented for agent-view dispatch and not verified for `--bg` ([reference](reference/claude-code-cli.md#option-values)). |
| Session name | Optional. Passed as `--name`. |
| Account | Configured accounts, preselected with `default_account` from the config. The user can pick another account for this session; there is no per-project account ([0003](0003-accounts-and-session-ownership.md)). Moving it later is a hand-off ([0004](0004-account-hand-off.md)). |
| Model | `default`, `fable`, `opus`, `sonnet`, `haiku`, or free text. Choosing `haiku` (or a free-text Haiku 4.5 or older model) together with `auto` shows a warning: auto mode is unavailable there and the session silently starts in manual mode. |
| Effort | `default`, `low`, `medium`, `high`, `xhigh`, `max`. |
| Permission mode | `auto` (default), `acceptEdits`, `manual`, `plan`, `dontAsk`, `bypassPermissions`. `bypassPermissions` is hidden when tower runs as root, because Claude Code refuses it there. |

Model, effort and permission mode start from `default_model`, `default_effort` and
`default_permission_mode` in the config ([0002](0002-tech-stack-and-files.md)).

Keys (source of truth: [0011](0011-keybindings.md)): `Tab` / `Shift+Tab` move between
fields, `Enter` adds a new line in the task field, `Ctrl+G` opens the task in the editor,
`Ctrl+S` dispatches, `Ctrl+Enter` (fallback `Alt+Enter`) dispatches and attaches, `Esc`
cancels.

Dispatch runs, with the project as working directory:

```
<account command> --bg [--name N] [--model M] [--effort E] [--permission-mode P] "<task>"
```

- Flags left at `default` are omitted, so Claude Code's own defaults apply.
- The permission mode has no `default` entry, so `--permission-mode` is always passed
  (`default_permission_mode`, built-in `auto`, unless changed).
- Tower parses `backgrounded · <id> · <name>` from stdout and ignores the hint lines after
  it. stderr carries progress (`Starting background service…`) and error text; it is shown
  only when the dispatch fails.
- Tower records the account for the new session in the sidecar (`account_source: started`),
  updates the project's `recent_projects` entry, and the row appears under Working on the
  next poll.
- "Dispatch and attach" (`Ctrl+Enter` / `Alt+Enter`) dispatches and then attaches to the new
  id ([0005](0005-attach-and-detach.md)).

Untrusted folder: if `--bg` exits 1 with stderr starting `Workspace not trusted`, tower
shows the error and offers a key that suspends tower and runs the interactive `<account command>` in that
folder. The user accepts the trust dialog and exits (`Ctrl+C` twice or `/exit`); tower then
resumes and retries the same dispatch. Trust is per account, so the same folder may need
this once per account, and a git clone under an already trusted root may still need it.

Tower does not change worktree, commit or push behaviour. Background sessions isolate in
`.claude/worktrees/` and decide on commits, pushes and merge requests as Claude Code
defines; the task text is the place to ask for something else
([0016](0016-do-not-change-claude-code-defaults.md)).

## Consequences

- Project discovery is a cheap directory scan: a folder counts if it contains `.git`. Nested
  repos deeper than one level need an `extra_projects` entry.
- The dialog never edits Claude Code's trust state itself; the user accepts trust in Claude
  Code's own dialog.
- Any other `--bg` failure is shown verbatim with the exit code; the dialog keeps its
  contents so the user can fix a field and retry.
- Model names cannot be validated up front: a typo in the free-text model dispatches normally,
  and the session turns `failed` shortly after start. It then shows in the Failed group with
  the usual attention highlight ([0006](0006-session-states-and-grouping.md),
  [0007](0007-visual-only-attention.md)).
- `--agent` is not exposed in v1.

## Alternatives considered

- **Explicit project list only.** Tedious for many clones under one root.
- **Recursive scan of roots.** Slow on large trees and picks up vendored repos.
- **Single-line task.** Too limiting for real task descriptions.
- **Pre-trusting folders by editing `.claude.json`.** Changes Claude Code's own state; rejected
  by [0016](0016-do-not-change-claude-code-defaults.md).
- **Setting `worktree.bgIsolation: "none"`.** Changes how agents work; rejected.
