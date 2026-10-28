# 0008. New-session dialog

- Status: Accepted · 2026-10-18 · `--settings` for subagent hooks added by [0029](0029-tracking-subagents.md) · template key added by [0036](0036-prompt-templates.md) · slash-command suggestions in the task field added by [0042](0042-slash-command-suggestions.md) · the same dispatch from the command line, `tower dispatch`, added by [0044](0044-orchestrator-starts-and-wakes-sessions.md)
- Related: [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0036](0036-prompt-templates.md), [reference](reference/claude-code-cli.md)

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

*Amended by [0036](0036-prompt-templates.md):* `Ctrl+P` (from any field) opens the
template picker over the dialog; a picked template fills a blank task field or goes in at
the task cursor, and the task field takes the focus.

*Amended by [0042](0042-slash-command-suggestions.md):* typing `/` as the first
non-blank character of the task opens a menu of the chosen account's, project's
and enabled plugins' skills and commands; `↑` / `↓`, `Tab` / `Enter` and `Esc`
work on the menu while it is open, and an accepted command's `argument-hint`
shows on the task's label row. Changing the account or the project loads that
pair's list.

*Amended 2026-10-21 (UI audit):* the dialog's key hints are one row, never wrapped or
cut, joined by ` · ` in this order: `Tab/Shift+Tab field`, `Enter new line` (`Enter next
field` outside the task), `Ctrl+S start`, `Alt+Enter start & attach` (the attach key that
works, [0011](0011-keybindings.md)), `Ctrl+G editor`, `Ctrl+P template`. When the row is
too wide, whole hints are dropped in this order: `Enter`, `Ctrl+G editor`, `Tab/Shift+Tab
field`, the attach key, `Ctrl+P template`; `Ctrl+S start` stays. The blank task field
shows the longest of these placeholders that fits: `describe the task · Enter new line ·
Ctrl+G opens $VISUAL / $EDITOR · Ctrl+P inserts a template`, `describe the task · Enter
new line · Ctrl+G editor · Ctrl+P template`, `describe the task · Ctrl+G editor · Ctrl+P
template`, `describe the task · Ctrl+P template`, `describe the task` (cut with `…` only
when even that does not fit). The model field's hint, `↑↓ default · fable · opus ·
sonnet · haiku · or type a model name` or else `↑↓ presets · or type a model name`, shows
only whole and only while the input keeps `min(max(text + 1, 8), half the value
column)` columns; otherwise the input takes the whole row. Messages below the fields
(`starting…`, the questions, errors) wrap at spaces. The task text wraps at spaces too
(a word breaks inside only when it is longer than the row; the spaces after a row's last
word stay on that row, and the cursor is drawn where the text is). The dialog keeps one
column of padding inside its border on each side; `Esc cancel` is on its bottom border
only, the other windows name `Esc` once, in their keys row. A wrapped ` · ` list never
ends a row with `·`: the separator at a break is left out. `Ctrl+U` is not bound in the
dialog's fields ([0036](0036-prompt-templates.md): they take no `Ctrl` keys); `Backspace`
deletes the model preset.

Dispatch runs, with the project as working directory:

```
<account command> --bg [--name N] [--model M] [--effort E] [--permission-mode P] "<task>"
```

- Flags left at `default` are omitted, so Claude Code's own defaults apply.
- *Amended by [0029](0029-tracking-subagents.md):* while `[sessions]
  track_subagents` is on (default), `--settings <config dir>/claude-settings.json`
  follows `--permission-mode`, before the task: tower's observe-only subagent
  hooks. Tower writes that file first when its content is not current; when it
  cannot, the dispatch runs without the flag.
- The permission mode has no `default` entry, so `--permission-mode` is always passed
  (`default_permission_mode`, built-in `auto`, unless changed).
- The task is trimmed and passed as one argument, never through a shell. The dialog rejects
  a task, name or option value that starts with `-`, because the CLI would read it as an
  option (whether `--` would end the options for `--bg` is not verified).
  *Amended by [0044](0044-orchestrator-starts-and-wakes-sessions.md):* `tower dispatch`
  builds the same request from the command line and passes a task that starts with `-`
  after `--`, which `--bg` takes as the prompt (verified 2.1.312); the dialog still
  refuses it.
- Tower parses `backgrounded · <id> · <name>` from stdout and ignores the hint lines after
  it. stderr carries progress (`Starting background service…`) and error text; it is shown
  only when the dispatch fails.
- Tower records the account for the new session in the sidecar (`account_source: started`),
  updates the project's `recent_projects` entry, and the row appears under Working on the
  next poll. `--bg` reports only the short id, so the record is keyed by it until a listing
  shows the session's `sessionId`, and then moves to that key.
- "Dispatch and attach" (`Ctrl+Enter` / `Alt+Enter`) dispatches and then attaches to the new
  id ([0005](0005-attach-and-detach.md)).

Untrusted folder: if `--bg` exits 1 with a stderr line starting `Workspace not trusted`
(`Starting background service…` may come first), tower
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
  repos deeper than one level need an `extra_projects` entry. It runs on a worker thread
  ([0001](0001-thin-wrapper-over-claude-cli.md)): the dialog opens at once and shows
  `scanning projects…` until the list arrives; a filter typed meanwhile is kept.
  With no project at all it names the config file and whether it was loaded
  (`… in ~/.config/watchtower/config.toml (loaded)`, or `no config at … (using
  defaults)`), so a missing config is not mistaken for empty roots
  ([0002](0002-tech-stack-and-files.md)). The same list serves the resume
  window of `R` ([0019](0019-resume-past-sessions.md)).
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
