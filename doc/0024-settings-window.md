# 0024. Settings window

- Status: Accepted · 2026-10-19 · the logs-follow row added by [0031](0031-logs-follow-interval.md) · an empty hand-off prompt allowed by [0033](0033-hand-off-without-a-prompt.md) · the colour-depth row added by [0038](0038-light-and-16-colour-themes.md) · detail-panel values wrap inside a padding column, amended 2026-10-21 · the two plan usage rows added by [0043](0043-plan-usage-display.md) · `workspace` among the styles by [0045](0045-workspace-style.md)
- Related: [0002](0002-tech-stack-and-files.md), [0007](0007-visual-only-attention.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0018](0018-account-groups-and-handoff-guard.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0022](0022-tower-writes-its-own-config.md)

## Context

[0022](0022-tower-writes-its-own-config.md) lets tower write its own
`config.toml` and defines how a save works (lock, conflicts, `toml_edit`,
validation, the one-time backup, the atomic write, reload). It left the
window itself to the UI. The owner approved a mock of a full-screen
Settings window and decided its keys, the live preview, the questions and
what stays read-only.

## Decision

**Opening.** `c` in the session list (`с` on a RU/UA layout,
[0011](0011-keybindings.md)) reads `config.toml` and opens the window over
the whole screen. A missing file opens with the built-in defaults; the
header says `(not found — using defaults)` and `Ctrl+S` creates the file
with the changed keys only. A file that does not parse is not opened; the
status line names the error.

**Layout**, as in the mock: a header panel (the file, `(found)`, and
`* N unsaved changes`, `saving…` or `✓ saved`), the list panel with a
separator per section (Appearance, New sessions, Projects, Hand-off,
General, Accounts) joined to its border, and from 130 columns a 50-column
detail panel with the selected setting's help, key, values, default,
file value, unsaved value and when it applies
(`config_edit::SettingKey::applies`). *Amended 2026-10-21 (UI audit):*
its text keeps one padding column before each border; a long value
wraps under its label, a list of values (`default · fable · …`) breaks
between its items so no row starts or ends with `·`, and a word longer
than a row breaks after a `-`, `/` or `_` where it can. Short values
keep their one row. Below 130 columns a one-line help
strip under the list replaces it. A row shows its value, a dim `default`
when it equals the built-in default, and an amber `*` when it has an
unsaved change (`●` stays the attention marker,
[0007](0007-visual-only-attention.md)). The status line and the footer
stay at the bottom. The window uses the overlay helpers of `Look`
([0021](0021-dashboard-themes.md)): the theme's look in the dashboard
style, the minimal look in the minimal style and below 80×24, no colour
under `NO_COLOR`.

**Editable and read-only.** Every key 0022 lists is editable, with the
values `config_edit::EditSession::offered` gives: choices (style, theme,
default account, model with "other…" for a full name, effort, permission
mode without `bypassPermissions`), a bool (cross-group hand-off), a number
(poll interval, 1 to 60), text (hand-off prompt) and folder lists (project
roots, extra projects). *Amended by [0031](0031-logs-follow-interval.md):*
also a stepped number, `follow logs in background` (`logs_follow_secs`)
in General after the poll interval: `off`, `5 s`, `10 s`, `15 s`, `20 s`,
`30 s`, changed like a choice (`←` / `→`, `Enter` for the pick list).
*Amended by [0033](0033-hand-off-without-a-prompt.md):* the hand-off
prompt may be empty (no prompt; the row shows `(none — waits for you)`)
and must not start with `-`.
*Amended by [0038](0038-light-and-16-colour-themes.md):* also `colour
depth` (`[ui] color_depth`) in Appearance after the theme, a choice of
`auto`, `truecolor`, `256`, `16`, previewed live like the style and theme
(the header says `colour depth 16`), its pick list saying what `auto`
finds.
*Amended by [0043](0043-plan-usage-display.md):* also `show plan usage` (`[usage] show`, a
toggle) and `usage refresh` (`[usage] refresh_secs`, stepped `1 min`,
`2 min`, `5 min`, `10 min`, `15 min` like the logs-follow row, never off)
in General after `track subagents`. `[[accounts]]` are shown read-only with the note
that a wrong command or config dir can wake sessions under the wrong
account; `e` is the way to edit them.

**Keys** (also in [0011](0011-keybindings.md)): `↑` / `↓` (`k` / `j`)
move, `Tab` / `Shift+Tab` jump between sections, `Enter` edits (pick list,
toggle, inline input, list editor), `←` / `→` step a choice, `Del` resets
to the default (the save removes the key), `Ctrl+S` saves and keeps the
window open, `e` opens the file in `$VISUAL` / `$EDITOR`, `?` lists the
keys, `Esc` closes, `Ctrl+C` quits.

- An inline input checks the value as it is typed
  (`EditSession::check`): an invalid one shows a red field and the message
  (`✗ must be 1 to 60 seconds`); `Enter` keeps only a valid value and
  `Esc` undoes the edit. `Ctrl+S` in an invalid field saves nothing.
- The list editor: `a` adds, `Enter` edits, `d` removes, `Esc` goes back
  (changes stay unsaved). Next to each folder a note says what the scan
  finds (`✓ 3 git clones`, `✓ git clone`, `not found; skipped when
  scanning`); while a folder is typed, `Tab` completes its name from the
  disk. These look at the disk on a worker thread, one job at a time, so a
  hung mount never blocks the window. Duplicates and empty entries are
  refused; a trailing `/` is dropped.
- Text fields (the inline input, the folder input) are text contexts:
  Cyrillic stays as typed. Everywhere else in the window keys are command
  keys and Cyrillic letters map to their Latin keys.

**Questions.** Each takes `y`; the default is to cancel.

- `Esc` with unsaved changes lists them and asks: `y` save and close, `n`
  discard, `Esc` / `Enter` keep editing.
- `e` with unsaved changes asks the same with save, discard or cancel
  before the editor; without changes the editor opens at once. The window
  closes for the editor and opens again after it when the file is valid.
- A save that finds keys changed in the file meanwhile
  (`SaveError::Conflict`) lists each key with the file's value and this
  window's: `y` saves with force, `n` saves nothing and keeps the changes.
- Turning `allow_cross_group_handoff` on shows the terms note of
  [0018](0018-account-groups-and-handoff-guard.md) (terms follow the
  account) and asks; turning it off does not.
- `Ctrl+C` with unsaved changes asks before quitting.

**Saving** follows [0022](0022-tower-writes-its-own-config.md): `Ctrl+S`
runs the save on a worker thread (`saving…` in the header), then the
status line says what was written (`✓ Saved 3 changes to … · comments and
formatting kept · backup config.toml.bak · applied now`) and the running
tower applies the saved config as on a reload. This settles 0022's open
item: the window stays open after `Ctrl+S`.

**Live preview.** While the window is open the style and theme it shows
are the pick list's value under the cursor, else the unsaved value, else
the running config's. So the whole window, and the screen after it, is
drawn in the previewed look. Cancelling a pick list, discarding or closing
without saving goes back to the running style and theme.

**Reload while open.** A config reload ([0022](0022-tower-writes-its-own-config.md))
that arrives while the window is open is applied as usual and becomes the
running look; the window's preview is drawn over it again, so an unsaved
style or theme is not clobbered, and a style or theme the window did not
change follows the file. The window keeps the file as it opened it; its
next save merges onto the file on disk and asks about conflicts. Windows
a background result opens wait until the Settings window closes
([0011](0011-keybindings.md)).

## Consequences

- Every setting except accounts changes without leaving tower, with the
  look visible before it is saved.
- The window never writes anything of Claude Code
  ([0016](0016-do-not-change-claude-code-defaults.md)) and never offers
  `bypassPermissions` as a default.
- A preview is app state: other parts of tower draw in the previewed look
  until the window closes.
- The folder notes and the completion cost a `stat` and a `read_dir` per
  folder on a worker thread.

## Alternatives considered

- **Save and close on `Ctrl+S`.** One key less for the common case, but a
  second change needs the window opened again; the mock kept it open.
- **Editing accounts in the window.** Out of scope: a wrong command or
  config dir wakes sessions under the wrong account; the editor (`e`) is
  the way, followed by a restart.
- **Applying `ui.*` from a reload only after the window closes.** Simpler,
  but the running config would lag the file while the window is open;
  re-applying the preview over the reloaded look keeps both right.
