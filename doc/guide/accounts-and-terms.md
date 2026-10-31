# Accounts and terms

What using several accounts with tower means, and how tower protects your sessions and work. Back to the [README](../../README.md).

Tower only switches between accounts you already use with Claude Code. This is a short
summary of what that means, not legal advice.

- **Your own accounts, official login.** Every configured account must be your own and
  logged in through the official `claude` CLI
  ([Authentication](https://code.claude.com/docs/en/authentication)). Tower never handles
  credentials.
- **A hand-off moves the whole conversation.** After `a`, the full conversation and project
  context continue under the target account, under that account's terms. Training,
  retention and admin controls differ between consumer (Free, Pro, Max) and commercial
  (Team, Enterprise, API) accounts; see
  [Data usage](https://code.claude.com/docs/en/data-usage).
- **Keep employer and personal accounts apart.** Don't move sessions between an
  employer-provided account and a personal one without your organisation's permission.
  Give each account a `group` (e.g. `work`, `personal`): `a` then offers only accounts in
  the session owner's group, because `allow_cross_group_handoff` is off by default
  ([Configuration](configuration.md)).
- **Usage limits.** When an account hits its usage limits, use Anthropic's official
  options: usage credits, upgrading your plan, API usage, or waiting for the reset.
- **Shared history mixes accounts.** Config dirs that share history through symlinks mix
  those accounts' transcripts and memory.
- Claude Code is governed by the terms listed under
  [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance) and by the
  [Usage Policy](https://www.anthropic.com/legal/aup).

## Safety notes

- **Wrong-account wakes.** Attaching to or resuming a stopped session with another
  account's `claude` silently moves it to that account, with its login and terms, and the
  listing does not say which account hosts a session. So tower tracks ownership (from
  `/proc`, the per-account lists, its own dispatches and hand-offs) and runs every command
  with the owning account's command. When the owner is unknown the row shows `?`, and tower
  asks which account to use before it wakes anything
  ([0003](../0003-accounts-and-session-ownership.md),
  [0017](../0017-ownership-evidence-rules.md)).
- **Removing never destroys work.** `d` runs plain `claude rm <id>`. When `rm` refuses
  because of uncommitted or unpushed work, tower shows its message and never passes
  `--discard-unpushed` or `--force-remove-worktree`
  ([0009](../0009-stop-remove-and-logs.md)). Transcripts stay on disk.
- **Claude Code is left as it is.** Tower writes no Claude Code file, never passes
  `--dangerously-skip-permissions`, and does not accept workspace trust for you
  ([0016](../0016-do-not-change-claude-code-defaults.md)). The only `--settings` it passes is
  its file of observe-only subagent hooks, which `[sessions] track_subagents = false` turns
  off ([0029](../0029-tracking-subagents.md)).
