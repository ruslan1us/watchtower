# Working with sessions

Quick reply, prompt templates, slash commands, pinning and restarting after an update. Back to the [README](../../README.md).

## Quick reply

`t` on a session opens its **Reply to** window: type the text (or `Ctrl+P` for a template;
`Enter` starts a new line) and press `Ctrl+S`. Tower sends it as your own next prompt, with
the command of the account that owns the session, and the session keeps its id:

- a session that is idle after its turn (Ready for review, or blocked on a question it
  asked in its reply) is stopped, and once it is gone woken with
  `<account command> --resume <sessionId> --bg -- <text>`;
- a session without a live process (stopped, finished, failed) is only woken that way.

The window says which of the two will happen. Claude Code only takes a prompt this way on a
stopped session; on a running one it would start a copy, which tower reports by its id
instead of the reply. `t` is not offered while a turn runs (also `done · bg task`), or
while a session waits on a permission prompt or a question dialog: attach to answer those
(`Enter`). A session woken without a prompt (`working · idle`) is offered. The status line shows `stopping <name>…`, `waking <name> with your text…` and
`sent to <name>` ([0040](../0040-quick-reply.md)).

## Prompt templates

Texts you type again and again (review instructions, say) can live as templates: one
file per template in `~/.config/watchtower/templates/` (the `templates` folder next to
your `config.toml`), named `<name>.md`. Tower creates the empty folder the first time a
picker opens, so it is there to put files in. In the new-session dialog `Ctrl+P` opens a picker
with the names and each template's first line; type to filter, `↑` / `↓` to choose,
`Enter` inserts the text into the task (at the cursor, or as the whole task when it is
empty), `Esc` closes the picker. The [quick reply](#quick-reply) window has the same
picker on `Ctrl+P`.

```text
~/.config/watchtower/templates/
├── review.md          # "Review the diff for bugs and missing tests. …"
└── release-notes.md
```

Tower only reads the folder, afresh each time the picker opens. Only regular `.md` files
directly in it count; hidden files and subfolders are ignored, and symlinks, files over
64 KiB, empty files and files that are not UTF-8 are skipped, with a note in the status
line ([0036](../0036-prompt-templates.md)).

## Slash commands

In the new-session dialog's task field and the [quick reply](#quick-reply) text, a `/` as
the first character opens a **Commands** menu below the cursor, like the one in Claude
Code's prompt. It lists the skills and custom commands Claude Code would offer for the
dialog's account and project (for a reply: the session's owner and folder):

- personal: `skills/<name>/SKILL.md` and `commands/**/*.md` in the account's config dir
  (`~/.claude`, or its `CLAUDE_CONFIG_DIR`);
- project: `.claude/skills` and `.claude/commands` of the project and its parent folders up
  to the repository root;
- claude.ai: the skills claude.ai syncs into `skills/synced/<bucket>/` of the account's
  config dir, under their plain name (`/<name>`); a personal or project entry of the same
  name wins;
- plugins: the enabled plugins `<account command> plugin list --json` reports, as
  `/<plugin>:<name>` (e.g. `/review-kit:code-review`).

Each row shows `/name`, its `argument-hint` (e.g. `/fix-bug [ticket]`), the description and
where it comes from (`personal`, `project`, `claude.ai`, `plugin <name>`). Typing filters (names that
start with the text first), `↑` / `↓` choose, `Tab` or `Enter` insert `/name ` and close the
menu, `Esc` closes only the menu; with the menu closed every key keeps its meaning. After
an insert the argument hint stays in the field's hint row while you type the rest. Entries
with `user-invocable: false` are hidden; built-in commands are not listed. Claude Code
shows a synced skill whose name is also a built-in under a namespaced form (such as
`/anthropic-skills:<name>`); tower does not know the built-ins or that namespace, so it
offers the plain `/<name>`.

Tower reads only the frontmatter of each file (at most 16 KiB), on a worker thread, and
never writes there; the list is refreshed when the dialog opens, when its account or
project changes, and when a reply window opens. The exact rules (names, precedence, limits,
symlinks) are in [0042](../0042-slash-command-suggestions.md).

## Pinning a session

`P` on a row pins it, `P` again unpins it ([0047](../0047-tab-title-pinning-and-restart.md)).
Grouped by state, pinned sessions have their own `Pinned` group right after Orchestrator, whatever
their state, stopped ones too; the row still shows its state, and its `●` still counts. Grouped by
project they sort first within their project. It folds, filters and counts like any group, in every
style, and in the workspace it is a group of cards. The orchestrator and interactive rows cannot be
pinned.

The pin lives in tower's `state.json`, by the session's short id, so `/clear` does not lose it. It
is tower's list only: Claude Code knows nothing of it, and a pinned session still stops after about
an hour of idling. A pin whose session no listing showed for 14 days is dropped.

## Restarting after an update

A running session stays on the Claude Code it started with. `U` on a row asks, then runs
`<owner's command> respawn <id>` ([0047](../0047-tab-title-pinning-and-restart.md)). The
conversation, id, name, account and saved options stay. **What is lost is said first:** a running
turn is interrupted and does not continue, and a pending prompt is dropped and what it asked for
does not run. A terminal attached to the session elsewhere is disconnected.

- `U` on a group header restarts that group's running sessions, and `Ctrl+U` every running session.
  That window counts them per account (`main: 3 sessions`), says what it leaves out, and only a
  capital `Y` goes.
- Tower goes by id, with each session's own account's command, and never uses `respawn --all`.
- A stopped session has nothing to restart: it runs the installed Claude Code when woken.
- In the workspace the pane's terminal closes first, and comes back once the session is running.
- Tower cannot tell you which sessions run an older version: the CLI does not say, per session.
