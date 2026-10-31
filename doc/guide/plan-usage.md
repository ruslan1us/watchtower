# Plan usage

An optional display of the plan usage Claude Code reports for each account. Back to the [README](../../README.md).

Off by default. With `[usage] show = true` (or `show plan usage` in the Settings window)
tower asks each account's Claude Code for its plan usage every `refresh_secs` seconds
(default 5 minutes), one account at a time, and shows only the numbers Claude Code
itself reports ([0043](../0043-plan-usage-display.md)):

- In the dashboard's header, after each account's chip: `5h 24% · 7d 12%` (the current
  session's and the week's share), coloured by Claude Code's own severity, dimmed with
  its age (`· 20m ago`) when it is old or the last probe failed. A narrow header drops
  the week's number first, then the numbers. `—` means no plan data: the account is
  logged out, uses an API key or is billed by usage.
- In the details panel, for the selected session's account: each limit with when it
  resets, in your local time (`5h 24% · resets 18:00`, `7d 12% · resets Tue 08:00`,
  per-model weekly limits by name), extra usage when it is on, and the age of an old value.
- In the minimal style, a compact `main 24%/12%` in the title line when it fits.

The probe is `<command> -p --output-format stream-json --verbose --no-session-persistence
--safe-mode /usage`, run in tower's own empty folder `~/.config/watchtower/usage/`:
Claude Code's `/usage` as a local command, with no model turn and no tokens, nothing
saved, and none of your hooks or MCP servers (`--safe-mode`). Each probe is a short-lived
Claude Code process (about two seconds). While it runs, `claude agents` lists it as an
interactive session in that folder; tower hides that row. `r` also asks again for values
older than a minute. When Claude Code says it is rate limited, tower waits until the time
it states.

The numbers are for your information only: tower never sorts, ranks or suggests anything
by them, never acts on them, and never shows them in the hand-off windows or in `tower
status --json`. When an account hits its limits, see [Accounts and terms](accounts-and-terms.md).
