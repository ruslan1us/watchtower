# 0042. Slash-command suggestions in the task field

- Status: Accepted · 2026-10-28
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0008](0008-new-session-dialog.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0036](0036-prompt-templates.md), [0040](0040-quick-reply.md), [CLI reference](reference/claude-code-cli.md#skills-and-custom-commands-the--menu)
- Amends: [0001](0001-thin-wrapper-over-claude-cli.md) (what tower reads outside the CLI), [0008](0008-new-session-dialog.md) (the task field), [0011](0011-keybindings.md) (keys of the task field and the reply text), [0040](0040-quick-reply.md) (the reply text)

## Context

The owner starts many sessions with a prompt such as `/fix-bug <ticket link>`, where
`fix-bug` is a skill. Claude Code's own prompt suggests commands while `/` is typed;
tower's task field does not, so names have to be remembered and typed whole. The owner's
skills come from the account's personal folder, the project's `.claude/` (most of them)
and enabled plugins. Where Claude Code looks, how it names entries and which win are
documented ([CLI reference](reference/claude-code-cli.md#skills-and-custom-commands-the--menu)).

## Decision

**What is offered.** For an account and a project folder (the cache key):

- *personal*: `<config dir>/skills/<name>/SKILL.md` and `<config dir>/commands/**/*.md`
  of the account (its configured `config_dir`, the account's `CLAUDE_CONFIG_DIR`);
- *project*: `.claude/skills` and `.claude/commands` of the folder and of each parent up to
  the repository root (the first folder with a `.git`, at most 16 levels up; outside a
  repository only the folder itself); a `.claude` that is the account's config dir counts
  as personal only;
- *plugin*: the entries of each plugin that `<account command> plugin list --json`, run in
  the folder with the account's command ([0003](0003-accounts-and-session-ownership.md)),
  reports with `enabled: true`. `project` and `local` installs whose `projectPath` is
  another folder and `session` scope are left out. The plugin is read under
  `readFromFolder`, else `installPath`, which must be an absolute, existing folder. Its
  `.claude-plugin/plugin.json` (at most 64 KiB) gives the namespace (`name`, else the id's
  part before `@`) and the `skills` (added) and `commands` (replacing) keys.

Names follow the docs: `/<skill folder>` or the frontmatter `name`, `/<a>:<b>` for
`commands/a/b.md` (subfolders at most 3 deep), `/<plugin>:<name>` for plugins. Clashes:
a skill over a command file, then personal over project, then the folder nearer the
repository root; within one source the first found (folders in name order); plugins are
namespaced, and of two plugins with one name the first listed wins. `user-invocable: false`
hides an entry. Built-in commands, bundled skills, managed skills, MCP prompts, workflows,
claude.ai's synced skills (`skills/synced/`) and components added by marketplace entries
are not offered.

**What is read, and how.** Only the head of each file: lines up to the closing `---` of a
frontmatter that opens on the first line, and, without a `description`, up to the first
non-empty line after it (or of the file), at most 16 KiB. The frontmatter reader takes
`name`, `description`, `argument-hint` and `user-invocable` from top-level `key: value`
lines (plain, quoted and block scalars; a flow value such as `[issue]` as written); a
frontmatter that is not valid YAML gives empty metadata, as in Claude Code. Skipped
silently: hidden entries, reserved names, invalid names, non-regular files, files that are
not UTF-8, cannot be read or whose frontmatter does not close within the cap. Folder
listings stop after 512 entries, the list at 2000 entries, plugins at 64. Personal and
project symlinks are followed (the docs allow a skill folder to be a link), bounded by the
depth limit; inside a plugin a link is followed only while its target stays inside the
plugin's folder. Files are opened with `O_NONBLOCK`, so a FIFO never blocks. Tower never
writes anything there.

**When.** The load runs on a worker thread (`Effect::LoadCommands`, `commands::CommandLoad`;
`plugin list` with a 10 s timeout and its own process group, like every background call,
[0001](0001-thin-wrapper-over-claude-cli.md)). The app keeps one list per account and folder
and refreshes it when the new-session dialog opens or its account or project changes, and
when a quick reply window opens (for the owner and the session's `cwd`); `App::tick` asks
for at most one load per turn, never two for one key at once. A cached list shows
meanwhile; a failed plugin listing leaves only the plugins out.

**The menu.** In the dialog's task field and the reply text. It opens while `/` is the first
non-blank character of the whole text and the cursor is inside that first word (a word
with a second `/`, a path, does not count). It lists the matches of the word (ignoring
case): names starting with it, then names whose part after a `:` does, then names
containing it, at most 8 rows at once, each `/name [argument-hint]`, the description
dimmed and the source tag right-aligned (`personal`, `project`, `plugin <name>`); the
description and then the tag are left out when the row is too narrow, cuts end with `…`.
It is a bordered window titled `Commands`, inside the host's border and padding, below the
cursor's row (above it when there is no room below); its keys are on its bottom border,
`↑↓ choose · Tab insert · Esc close` (whole hints dropped from `↑↓ choose` up). The reply
window grows while the menu is open so its plan and keys stay visible. While the first load
runs, the menu shows `loading commands…`; with nothing found or no match there is no menu.

**Keys** (open menu only; closed, every key keeps its meaning): `↑` / `↓` choose
(wrapping), `Tab` or `Enter` accept: the word becomes `/name ` (one space, or the cursor
moves past the space that is there) and the menu closes; `Esc` closes only the menu, until
the word changes. Typing goes on filtering. Modified keys (`Shift+Tab`, `Alt+Enter`,
`Ctrl+…`) are never the menu's. While loading, only `Esc` is the menu's. After an accept
with an `argument-hint`, the hint shows as `/name hint`, dimmed, in the field's hint row
(the dialog's task label row, the reply's row under the text) while the text still starts
with that command. The help overlay does not list these keys (it must fit 24 rows); the
menu names them.

**Why reading these files is fine under 0001.** [0001](0001-thin-wrapper-over-claude-cli.md)
forbids Claude Code's internal state (`jobs/`, `daemon/`, transcripts, credentials, the
plugin records and caches tower would have to interpret) because it is undocumented and can
change in any release. Skill and command files are the user's own documented configuration,
written by the user or shipped by a plugin, with a documented layout and frontmatter; the
plugin folders are the ones the CLI itself reports (`installPath`). Tower reads them the way
an editor would, read-only and only their heads, and runs one documented read-only CLI
command. It touches no session, settings file or Claude Code state and changes nothing about
how Claude Code behaves ([0016](0016-do-not-change-claude-code-defaults.md)): the accepted
text is only the task passed to `--bg` or the reply as before.

## Consequences

- The owner's skills are a few keys away, with their argument hints, under the account
  the session will run with.
- Suggestions can differ from Claude Code's menu: tower does not see `skillOverrides`,
  managed or synced skills, workflows, marketplace-entry components, or names Claude Code
  refuses for reasons not documented. A suggestion is only text; Claude Code decides what
  it runs.
- One `plugin list` call per dialog opening, account or project change and reply window,
  plus small file reads, all off the UI thread.
- A rule in the docs that changes (a new location, a new field) needs a change here and in
  `commands.rs`.

## Alternatives considered

- **Parsing `claude plugin details`.** Human-readable text with costs; unstable.
- **Reading `~/.claude/plugins/installed_plugins.json`.** Claude Code's internal record,
  rejected by [0001](0001-thin-wrapper-over-claude-cli.md); `plugin list --json` is the
  supported interface.
- **A full YAML parser.** A new dependency for four fields; the reader takes the documented
  fields and treats anything it cannot read as malformed, as Claude Code does.
- **Loading only when `/` is typed.** Simpler, but the first `/` would always show
  `loading commands…`; the request was for a list loaded when the window opens.

## Open items

- Whether Claude Code's menu shows the synced, managed and workflow entries tower leaves
  out, and in which order it ranks matches.
