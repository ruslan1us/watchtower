# 0027. Rename a session

- Status: Accepted · 2026-10-19
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0005](0005-attach-and-detach.md), [0006](0006-session-states-and-grouping.md), [0007](0007-visual-only-attention.md), [0008](0008-new-session-dialog.md), [0010](0010-orchestrator.md), [0011](0011-keybindings.md), [0016](0016-do-not-change-claude-code-defaults.md), [0017](0017-ownership-evidence-rules.md), [0021](0021-dashboard-themes.md), [CLI reference](reference/claude-code-cli.md#renaming-a-session)

## Context

A session's name is set with `--name` at dispatch ([0008](0008-new-session-dialog.md)) and is what the list shows. Names chosen at start often stop fitting the work. Claude Code has no rename command or option in `claude --help`, `claude agents --help`, `claude attach --help` or `claude respawn --help`. Verified with 2.1.303 ([CLI reference](reference/claude-code-cli.md#renaming-a-session)):

- Inside a session, `/rename <name>` is a local command (no model turn): it prints `⎿  Session renamed to: <name>`, and `claude agents --json` shows the new `name` at once. A name with spaces works.
- The name survives a stop and a wake: `--resume <full id> --bg` afterwards wakes the same id under the new name.
- `--resume <full id> --bg "/rename X"` on a stopped session wakes it (same id, the old name on the `backgrounded` line), runs `/rename` with no model turn, and the listing then shows `name: X`, `state: working`, `status: idle`.
- `--resume <full id> --bg --name X` on a stopped session does not rename it: it starts a copy named `X`. `--resume <short id> --bg` also starts a copy.

## Decision

`e` on a session row (`у` on a RU/UA layout, [0011](0011-keybindings.md)) opens a **Rename session** window drawn with the dashboard `Look` like every other window ([0021](0021-dashboard-themes.md)). It has one field, `Name`, prefilled with the current name (the short id for an unnamed row), cursor at the end, with the editing keys of every single-line field and `Ctrl+U` to empty it. Typed Cyrillic stays as typed. `Enter` confirms, `Esc` cancels, `Ctrl+C` quits as in the list.

**Name check** (`rename::check_name`). The name is trimmed. It must not be empty, must not contain control characters (line breaks included), and has at most 80 characters (the list shows 32 columns of it; Claude Code documents no limit). `orchestrator` is refused: tower would take the session for the orchestrator ([0010](0010-orchestrator.md)). A name equal to the current one closes the window with a note; nothing runs.

**Not offered**, with a status-line note instead of the window:

- interactive rows: "rename it there with /rename <name>";
- rows without a short id, and stopped rows without a `sessionId` (nothing to wake);
- the orchestrator: tower knows it by its exact name `orchestrator` (its own group, `o`); renamed, it would become an ordinary session and `o` would start a new orchestrator;
- a row being handed off, a row with an attach waiting for its refresh, and any row while a rename of a stopped session runs (one at a time). The row being renamed also refuses `Enter`, `s`, `d` and `a` until it reports back, as for a hand-off ([0004](0004-account-hand-off.md)).

**A stopped row** (no live process). The window says that `Enter` wakes the session under its owner without attaching, runs `/rename` there and stops it again. Then, on a worker thread (`rename::run`):

1. The owner is the row's owner ([0003](0003-accounts-and-session-ownership.md), [0017](0017-ownership-evidence-rules.md)). When it is unknown (`?`), the account picker opens as for a wake; the window explains that waking with another account moves the session to it. The answer is recorded as `asked`, as for the picker's other actions; an owner a poll learned meanwhile wins.
2. Right before the wake, the session's record in the state file is compared with the one this tower planned with, as for a hand-off ([0017](0017-ownership-evidence-rules.md) section 8): another tower's record naming another account ends the rename, nothing run.
3. `<owner command> --resume <sessionId> --bg "/rename <name>"` in the session's `cwd`, with the usual environment hygiene, process group and timeout ([0001](0001-thin-wrapper-over-claude-cli.md), [0017](0017-ownership-evidence-rules.md) section 10). `Workspace not trusted` opens the trust window of [0008](0008-new-session-dialog.md): `Ctrl+T` runs the owner's command in the folder, then the rename is retried once; `Esc` leaves the session as it was.
4. The `backgrounded · <id>` line must name the session's own short id. Another id means Claude Code started a copy: tower stops there and says so, naming the copy. Nothing is stopped or removed; the copy's owner is recorded as `started` under its short id, as for a dispatch.
5. Tower lists with the owner every 0.5 s for up to 10 s until the row (by `sessionId`, else short id) shows the new name. A failed listing is no evidence. On timeout the session is left running and the status line says the rename was not confirmed.
6. `<owner command> stop <short id>`, so the session is stopped as before. A failed stop is reported; the session then runs under its new name.

The status line shows each step (`renaming a → b: waking it under main…`) and ends with `renamed <old> → <new>`.

**The wake is not activity.** While the rename runs, tower records no state tuple for the session ([0006](0006-session-states-and-grouping.md)), never highlights it ([0007](0007-visual-only-attention.md)), and its age column keeps the time recorded before the wake. Once the rename succeeded and a fresh listing shows the row without a live process (within 30 s), that row's tuple becomes the recorded one without a new change time and without touching `acknowledged`. A rename that fails, starts a copy or leaves the session running ends this at once: the session's states count again from the next poll. A hand-off does nothing of the kind, because its wake is the session really continuing.

**A running row** (live process). Claude Code renames a running session from inside it, so the window keeps the `Name` field and explains: `Enter` attaches; type `/rename <name>` (shown with the typed name filled in) and press `Enter` there, then `Ctrl+Z` to come back. `Enter` attaches exactly like `Enter` on the row: the refresh, the owner, the account picker for `?`, the refusals ([0005](0005-attach-and-detach.md)). After a clean return (exit code 0), tower polls at once and compares the name in the first listing of the attaching account from that poll or a later one with the requested name: `renamed <old> → <new>` when they match, otherwise the neutral note `name unchanged: <name> (to rename it, type /rename <name> inside the session)`. A failed or cancelled attach compares nothing.

## Consequences

- Every background session can be renamed from tower; tower never writes a name itself, so the name stays Claude Code's own and `tower status --json` and every Claude Code view agree.
- A stopped session costs no model turn to rename, but its process runs for a few seconds, under its owner. A pending permission prompt before the stop is not shown again after the wake (as for a hand-off, [0004](0004-account-hand-off.md)).
- Quitting tower or a SIGTERM while a rename runs does not wait for it: the session may stay running under its new name and is then an ordinary running session. Another tower sees the wake like any state change.
- After the rename's stop the row may be listed with `state: stopped` instead of its earlier state ([CLI reference](reference/claude-code-cli.md#json-listing-claude-agents---json)); it stays in the Stopped group either way.
- The orchestrator cannot be renamed from tower, and no session can be renamed to `orchestrator`.

## Alternatives considered

- **A label kept only in tower's sidecar.** Rejected: the name would differ between tower, `claude agents`, agent view and the orchestrator's `tower status --json`, and it would be lost with the sidecar record.
- **`--resume <id> --bg --name X` on the wake.** Rejected: on a stopped session it starts a copy instead of renaming (verified).
- **Typing `/rename` into the running session for the user** (through `attach` or a PTY). Rejected: tower never types into a session or proxies its terminal ([0005](0005-attach-and-detach.md), [0016](0016-do-not-change-claude-code-defaults.md)); the user does it inside Claude Code's own UI.

## Open items

- Whether Claude Code limits a name's length or characters; tower's 80-character limit is its own.
- Whether the copy case of `--resume <full id> --bg "/rename X"` (for example on a session that started running meanwhile) prints the same `note:` lines as the copies observed with `--name` and with a short id.
