# 0036. Prompt templates

- Status: Accepted · 2026-10-21 · also used in the quick reply window of [0040](0040-quick-reply.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0008](0008-new-session-dialog.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0022](0022-tower-writes-its-own-config.md)
- Amends: the task field and keys of [0008](0008-new-session-dialog.md), the new-session dialog keys and the untranslated text fields of [0011](0011-keybindings.md), the files of [0002](0002-tech-stack-and-files.md)

## Context

The owner types the same texts into the new-session dialog again and
again, mostly review instructions. They approved templates kept as plain
files that a key in the dialog inserts. A later "quick reply" window, which
sends a short text to a session, is to reuse the same picker.

## Decision

**Folder.** `<config dir>/templates/`, next to `config.toml` (default
`~/.config/watchtower/templates/`). One file per template, `<name>.md`.
Tower only reads the folder: it never creates, edits or deletes a template
file, and a missing folder simply means no templates. *Amended 2026-10-21
(owner request):* when a picker opens and the folder is missing, tower
creates it empty (mode `0700`) if the config dir exists, so the folder the
picker names is there to put files in; a failed creation changes nothing.

**File rules.** A load lists the folder's entries once, without recursion:

- Only names ending in `.md` count; the name is the file stem. Hidden files
  (starting with `.`) are ignored silently.
- Only regular files. Symlinks inside the folder are never followed
  (checked on the entry and again with `O_NOFOLLOW` when opening); FIFOs,
  devices, sockets and folders are skipped. The folder itself may be a
  symlink (for a dotfiles repository).
- Files over 64 KiB, empty or whitespace-only files, and files whose
  content or name is not UTF-8 are skipped.
- The text is the content with trailing whitespace and newlines trimmed;
  the start is kept as written.
- Templates are sorted by name ignoring case.

Skipped `.md` files are reported once per load in the status line, as a
warning: `skipped template big.md: over 64 KiB`, or `skipped 3 template
files: big.md (over 64 KiB) and 2 more`. A folder that exists but cannot
be listed shows its error in the picker.

**Loading.** Each time a picker opens, the folder is read afresh on a
worker thread ([0001](0001-thin-wrapper-over-claude-cli.md), as the project
scan); the picker shows `loading templates…` until the list arrives, and a
filter typed meanwhile is kept. A result for a picker that was closed or
reopened meanwhile is dropped.

**Picker.** A small window titled `Templates` over the window that opened
it. A filter line (`› type to filter`, with `n of m`), then the names with
the first non-blank line of each text as a dimmed preview, the chosen one
on the selection bar with `▶`. Typed text filters by name (every word,
ignoring case; Cyrillic is kept as typed), `Backspace` edits and `Ctrl+U`
empties the filter, `↑` / `↓` (`PgUp` / `PgDn`) choose, `Enter` picks,
`Esc` closes it and leaves the field as it was. `Ctrl+C` closes it and
counts as the `Ctrl+C` of the window beneath. With no template the list
reads `No templates yet: put .md files in ~/.config/watchtower/templates`
(the folder actually used); with no match, `no template matches the
filter`. It follows the style and theme like every other window
([0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md)).

The picker is a component: `templates::TemplatePicker` (state and keys, no
I/O, returning a `PickerAction`: `None`, `Close`, `Pick(text)`, `Quit`),
`App::new_template_picker` (a picker and its `Effect::LoadTemplates`),
`App::templates_loaded` (fills the open picker) and
`ui::templates::draw_picker`. The quick reply window reuses it as it is.

**New-session dialog.** `Ctrl+P` opens the picker from any field of the
dialog (not while a dispatch runs or a question is open). A pick goes into
the task field: a blank field is replaced by the text, otherwise the text
goes in at the cursor; the cursor ends after it and the task field has the
focus. The dialog's hint line shows `Ctrl+P template`; the help merges it
into the `Ctrl+G` row, so the help still fits 24 rows.

`Ctrl+P` was free in the dialog: the dialog binds `Ctrl+C`, `Ctrl+G`,
`Ctrl+J`, `Ctrl+S`, `Ctrl+T` and `Ctrl+Enter` / `Alt+Enter`, the text
fields take no `Ctrl` keys, and terminals pass `Ctrl+P` through (it is not
a flow-control or job-control key).

## Consequences

- Reused texts are a key away; templates are edited with any editor and
  can live in version control.
- Nothing about Claude Code changes: the template only fills the task text
  that is passed to `--bg` as before
  ([0016](0016-do-not-change-claude-code-defaults.md)).
- The folder is read on every opening, so a new or edited file shows up at
  once, at the cost of one small directory read per opening.
- Templates are not parameterised; the user edits the inserted text.

## Alternatives considered

- **Templates in `config.toml`.** Multi-line TOML strings are awkward to
  edit, and the Settings window would have to edit them
  ([0022](0022-tower-writes-its-own-config.md)).
- **Following symlinks inside the folder.** Would let a link read any file
  the user can read; regular files only keep the rule simple.
- **Loading once at start.** Edits would need a restart or a reload check.
