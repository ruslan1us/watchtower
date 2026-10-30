# 0022. Tower writes its own config

- Status: Accepted · 2026-10-19 · `[sessions] track_subagents` added by [0029](0029-tracking-subagents.md) · `logs_follow_secs` added by [0031](0031-logs-follow-interval.md) · `[ui] color_depth` added by [0038](0038-light-and-16-colour-themes.md) · `[usage] show` and `refresh_secs` added by [0043](0043-plan-usage-display.md) · `[ui] card_size` added by [0045](0045-workspace-style.md)
- Related: [0002](0002-tech-stack-and-files.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0018](0018-account-groups-and-handoff-guard.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md)
- Supersedes: in [0002](0002-tech-stack-and-files.md), "Tower only reads it" and the rejected alternative "Tower writing its own config"

## Context

The owner asked to change tower's settings inside tower instead of editing
`config.toml` by hand, and approved a mock of a full-screen Settings window
(style, theme, the new-session defaults, project folders, the hand-off
prompt and guard, the poll interval; accounts shown read-only). 0002 said
tower only reads its config and rejected tower writing it. The file is the
user's: it carries their comments, layout and, for some, a symlink into a
dotfiles repository. Several towers may run at once (two SSH sessions), and
the user may edit the file by hand while tower runs.

## Decision

**Scope.** Tower may write its own `config.toml`, and nothing else of the
kind: never a Claude Code file of any account
([0016](0016-do-not-change-claude-code-defaults.md)). Editable keys:
`[ui] style`, `[ui] theme`, `default_account` (one of the configured
accounts), `default_model` (`default`, `fable`, `opus`, `sonnet`, `haiku` or
a full model name), `default_effort`, `default_permission_mode`,
`project_roots`, `extra_projects`, `handoff_prompt`,
`allow_cross_group_handoff`, `poll_interval_secs`. `[[accounts]]` is shown,
never edited. *Amended by [0029](0029-tracking-subagents.md):* also
`[sessions] track_subagents`. *Amended by [0031](0031-logs-follow-interval.md):*
also `logs_follow_secs` (0 for off, or 5 to 30; a parser rule like the
poll interval's), applied live like the poll interval.
*Amended by [0038](0038-light-and-16-colour-themes.md):* also `[ui]
color_depth` (`auto`, `truecolor`, `256` or `16`), applied live like the
theme. *Amended by [0043](0043-plan-usage-display.md):* also `[usage] show` and `[usage]
refresh_secs` (60 to 3600, a parser rule), applied live.
*Amended by [0045](0045-workspace-style.md):* also `[ui] card_size` (`large`, `medium` or `small`),
applied live.

**Values offered.** The window offers, per setting, the values the code
lists (`config_edit::EditSession::offered`). `bypassPermissions` is not
offered as a default permission mode; the parser still accepts it in the
file, and the new-session dialog still offers it per session
([0008](0008-new-session-dialog.md)). `poll_interval_secs` must be 1 to 60;
this is a parser rule, so a hand-edited 0 or 61 is a config error naming
the range.

**Reset to default.** `Del` removes the key from the file, so tower's
built-in default applies. The comment lines above the key stay: they move
to the next key of the same table, else to the next table header, else to
the end of the file. A trailing comment on the key's own line goes with it.

**Saving** (`Ctrl+S`, on a worker thread):

1. Under an exclusive `flock` on `config.toml.lock` (waiting up to 2 s, as
   for `state.json`, [0017](0017-ownership-evidence-rules.md) section 8),
   re-read the file.
2. Conflicts: a key this window changed whose file value differs both from
   the value when the window opened and from the window's new value was
   changed elsewhere meanwhile. The save then writes nothing and reports the
   keys; the window asks, and a forced save overwrites them. Keys the window
   did not change keep the file's current value.
3. Apply only the changed keys with `toml_edit`, keeping comments, blank
   lines, key order and each value's decor (`style = "minimal"   # …`).
   A changed list keeps a multi-line layout. A new top-level key goes after
   the last top-level key; a new `[ui]` key into `[ui]`, which is created at
   the end of the file when missing.
4. Validate the resulting text with the same parser tower loads with. If it
   fails, write nothing and return the error.
5. Before tower's first write to the file, copy it once to
   `config.toml.bak` (next to the path tower uses). An existing `.bak` is
   never replaced.
6. Write atomically, as `state.json` is written
   ([0002](0002-tech-stack-and-files.md)): a temp file in the same
   directory, fsync, rename, fsync of the directory. The file's permission
   bits are kept. When `config.toml` is a symlink, the write goes to its
   final target, so the link stays a link.
7. A missing file is created, with its parent folders, holding a short
   header comment that points to `config.example.toml` and only the changed
   keys.

**Editor.** A key in the Settings window opens `config.toml` in
`$VISUAL`, else `$EDITOR`, in the foreground like an attach
([0005](0005-attach-and-detach.md)). A missing file is created first with
the header. After a clean exit tower reads it back and validates it as on
reload. This is the way to edit accounts.

**Reload.** A running tower takes in changes to `config.toml`, from another
tower, a hand edit or the editor: at each poll a worker compares the file's
device, inode, size and modification time with those of the version tower
last read or wrote, and reads and validates it when they differ, off the UI
thread. A save records the version it wrote, so tower does not reload its
own write, and a check that started before a save or an editor run is
dropped. A missing file means the built-in defaults, as at start.

- Applied live: the style and theme (the next frame), the new-session
  defaults, project roots and extra projects (rescanned when the dialog next
  opens), the hand-off prompt and guard (the next hand-off), the poll
  interval (the next tick is one new interval after the last one).
- Not applied live: accounts. While the file's accounts differ from the
  running ones, tower keeps its own accounts (and its `default_account`
  when the new one names none of them), and the status line says
  "accounts changed — restart tower to apply".
- An invalid file keeps the current config; the status line shows the error
  until the file is fixed.

## Consequences

- Settings change without leaving tower, and a hand edit applies without a
  restart.
- The user's file keeps its comments and layout; a reset never deletes a
  comment block. The one-time `.bak` keeps the hand-written original.
- Two new files can appear next to the config: `config.toml.lock` (never
  removed, like `state.json.lock`) and `config.toml.bak`.
- Removing a key instead of writing the default means a changed built-in
  default reaches users who reset a setting.
- A forced save can overwrite another tower's change to the same key; the
  window asks first.
- One more dependency, `toml_edit`, which shares its parser crates with
  `toml`. It replaces real code: a format-preserving TOML writer.

## Alternatives considered

- **Write the default value explicitly on reset.** Keeps the comment
  attached to its key, but pins today's default in the file, so later
  default changes never reach that user.
- **No reload; apply only this window's saves.** Simpler, but other towers
  and hand edits would need a restart, and two towers would drift apart.
- **Serialise the whole config with `toml`.** Loses every comment and the
  user's layout.

## Open items

- Whether `Ctrl+S` keeps the window open (as drawn in the mock) or closes it
  is the UI's choice. Settled in [0024](0024-settings-window.md): it stays
  open.
- Editing accounts in the window is out of scope for now.
