# 0043. Plan usage display

- Status: Accepted · 2026-10-28 · per account on the workspace's Home panel, which has no header, by [0045](0045-workspace-style.md)
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0002](0002-tech-stack-and-files.md), [0003](0003-accounts-and-session-ownership.md), [0004](0004-account-hand-off.md), [0007](0007-visual-only-attention.md), [0010](0010-orchestrator.md), [0011](0011-keybindings.md), [0014](0014-v1-scope.md), [0016](0016-do-not-change-claude-code-defaults.md), [0018](0018-account-groups-and-handoff-guard.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md), [0041](0041-details-panel-values-wrap.md), [reference](reference/claude-code-cli.md#plan-usage-limits)
- Supersedes: in [0014](0014-v1-scope.md), the non-goal "Usage and limit display per account"
- Amends: [0001](0001-thin-wrapper-over-claude-cli.md) (one more CLI call), [0002](0002-tech-stack-and-files.md) (the `usage/` folder, the `[usage]` table), [0011](0011-keybindings.md) (`r`), [0016](0016-do-not-change-claude-code-defaults.md) (the probe's flags), [0020](0020-visual-styles.md) (header, details panel, minimal title), [0022](0022-tower-writes-its-own-config.md) and [0024](0024-settings-window.md) (two settings); resolves [0018](0018-account-groups-and-handoff-guard.md)'s open item on limit-related claims for display only

## Context

The owner asked on 2026-10-28 to see each account's plan usage in tower,
as information, and decided the rules below. [0014](0014-v1-scope.md)
listed "usage and limit display per account" as a non-goal, and
[0018](0018-account-groups-and-handoff-guard.md) left open whether usage
limits count as "restrictions" under the Usage Policy, so tower made no
limit-related claims. That open item is about acting on limits; a display
that only repeats what Claude Code tells the user does not act on them.

What Claude Code offers (Claude Code 2.1.312, details and sources in the
[reference](reference/claude-code-cli.md#plan-usage-limits)):

- No `usage` or `status` subcommand; `claude auth status --json` and
  `claude agents --json` carry no usage.
- `<account command> -p --output-format stream-json --verbose
  --no-session-persistence --safe-mode "/usage"` runs `/usage` as a local
  command: no model turn, no tokens, nothing saved, and with `--safe-mode`
  none of the user's hooks or MCP servers. Its `assistant` line carries an
  experimental `usage_report` with each limit's `kind`, `percent`,
  `resets_at` and `severity`, and the extra usage. `null` means no plan
  data (logged out, an API key, a setup token or usage billing).
- The running command's config dir decides the account, as for every
  other call ([0003](0003-accounts-and-session-ownership.md)).
- While it runs (about a second) the probe is listed by `claude agents
  --json` as an `interactive` row in the folder it runs in.
- Claude Code's usage endpoint is rate limited itself: `/usage` may show
  "Showing last-known usage" or "rate limited until <time>".
- A call takes about 1.5 to 1.9 s and peaks near 280 MB.

## Decision

**Firm rules** (the owner's):

1. **Off by default.** `[usage] show = false`. Nothing is probed or drawn
   until the user turns it on.
2. **Only what Claude Code reports.** The percentages, reset times,
   severities and extra usage are shown as `/usage` gives them (a percent
   rounded to a whole number, a time in local time). Tower estimates,
   projects and compares nothing.
3. **Never in the hand-off.** The hand-off picker, its confirmation and the
   hand-off's status texts never contain usage
   ([0004](0004-account-hand-off.md), [0018](0018-account-groups-and-handoff-guard.md)).
4. **No decisions from it.** No sorting, ranking, suggestion or automatic
   action based on usage; it feeds nothing else in tower (hand-off,
   attention, grouping, the orchestrator).
5. **Visual only** ([0007](0007-visual-only-attention.md)): no bell,
   sound or notification; a high value is never a highlight, a `●` or part
   of the attention counter.
6. **Not for the orchestrator.** `tower status [--json]` runs no probe and
   has no usage fields ([0010](0010-orchestrator.md)).

**The probe.** `<account command> -p --output-format stream-json --verbose
--no-session-persistence --safe-mode /usage`, as a background call
([0001](0001-thin-wrapper-over-claude-cli.md)): stdin null, output
captured and capped, its own process group, no inherited
`CLAUDE_CONFIG_DIR`, killed after 20 s, on a worker thread. It runs in
tower's own empty folder `<config dir>/usage/` (created `0700` when
missing), never in a project. The parser lives in `src/usage.rs` and is
lenient: lines that are not JSON are skipped, unknown `kind` and
`severity` values are kept as-is, numbers may be integers, floats or
numeric strings, missing fields are absent. The text's "Showing
last-known usage" and "rate limited until …" are kept.

**The probe's row.** An `interactive` row whose `cwd` is the usage folder
(as tower names it or its canonical path) is dropped from every listing
before anything else sees it: the TUI's list, the sidecar and `tower
status`.

**Schedule.** Only while `[usage] show = true`: each account every
`[usage] refresh_secs` (default 300, 60 to 3600), the first probe at once
at startup or when turned on; one probe per account and one in all at a
time (each is a few hundred MB). `r` also probes accounts whose last probe
started more than 60 s ago. A stated "rate limited until" time holds the
account's next probe until then (at least one interval, at most 24 h; 15
minutes when the time cannot be read), `r` included. The last value is kept
with its time. It is stale when older than twice the interval, when the
last probe failed, or when Claude Code showed its last-known numbers.
Turning the display off forgets the values.

**Display** (only while on):

- *Header* ([0020](0020-visual-styles.md)): after each account's chip,
  `5h 24% · 7d 12%` (`session` and `weekly_all`), each number in the
  theme's text colour, amber or red by its `severity` (`normal` is text; a
  value naming a critical or exceeded state is red; any other is amber);
  a stale value is dim with ` · 20m ago`; `…` before the first answer,
  `—` without plan data or after a failure with no earlier value. Fitted
  like the rest of the header: after the account groups, the week's number
  goes first, then the age, then the numbers; nothing is cut mid-word.
- *Details panel*, side panel only: a `usage` value for the selected row's
  owner, after `account`: one row per limit, `5h 24% · resets 18:00`,
  `7d 12% · resets Tue 08:00`, scoped limits with their model or surface
  (`Fable 7d 3% · …`), other kinds by their raw name; `extra 12.50 of 50
  USD` when extra usage is enabled; `as of 20m ago` (with `last known`)
  when stale, the stated rate limit or the last error. The rows wrap like
  the other values ([0041](0041-details-panel-values-wrap.md)) and take
  the free rows last. An unknown owner or the strip under the list shows
  none.
- *Minimal style*: `usage: main 24%/12%, alt 5%/30%` after the accounts in
  the title line when all of it fits, else nothing; stale values dim.

**Config and Settings** ([0022](0022-tower-writes-its-own-config.md),
[0024](0024-settings-window.md)): `[usage] show` (bool) and `[usage]
refresh_secs` (a parser rule, 60 to 3600), in the General section as
`show plan usage` (a toggle) and `usage refresh` (stepped `1 min`,
`2 min`, `5 min`, `10 min`, `15 min`; a hand-edited value in range is
kept), saved and applied live like the other settings.

## Consequences

- With the display on, each account costs one short Claude Code process
  every five minutes by default, and the usage endpoint's own rate limit
  may answer with older numbers; tower then shows them dimmed.
- `--safe-mode` keeps the user's hooks and MCP servers from running every
  few minutes; tower passes no other option and changes no Claude Code
  setting ([0016](0016-do-not-change-claude-code-defaults.md)).
- `usage_report` is documented as experimental. If it changes, only
  `src/usage.rs` and its fixtures change; until then an account shows `—`
  or its last value.
- One more folder under the config dir, `usage/`, which stays empty.

## Alternatives considered

- **The status line's `rate_limits`.** It needs tower's own status line in
  place of the user's ([0016](0016-do-not-change-claude-code-defaults.md)).
- **Hooks.** No hook carries usage.
- **The text of `--output-format json` or plain text.** The same numbers,
  but in human wording and the local time zone's notation; the
  `usage_report` is structured.
- **Calling the usage endpoint with the account's credentials.** Against
  the hard rules of [0001](0001-thin-wrapper-over-claude-cli.md).
- **Hiding the row by `--session-id <uuid>`.** It works with `-p`, but the
  folder alone identifies the row, with one flag fewer.
- **Probing all accounts at once.** Faster at startup, but several
  processes of a few hundred MB at a time.

## Open items

- `severity` values other than `normal`, and the exact wording of "rate
  limited until …", were not observed; both are read leniently.
- The logged-out, API-key and setup-token cases (`usage_report: null`) come
  from research and the docs, not from a capture of tower's own run.
