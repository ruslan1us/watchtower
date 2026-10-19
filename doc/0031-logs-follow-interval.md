# 0031. The background logs-follow interval

- Status: Accepted · 2026-10-19
- Related: [0003](0003-accounts-and-session-ownership.md), [0005](0005-attach-and-detach.md), [0009](0009-stop-remove-and-logs.md), [0010](0010-orchestrator.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md), [0028](0028-scrolling-the-logs-panel.md), [0030](0030-continuous-logs-stream.md), [reference](reference/claude-code-cli.md#claude-logs-output)
- Amends: the fixed 10-second cadence of [0030](0030-continuous-logs-stream.md), the editable keys of [0022](0022-tower-writes-its-own-config.md) and the settings of [0024](0024-settings-window.md)

## Context

[0030](0030-continuous-logs-stream.md) fetches `claude logs` of every
working session, and of the session tower is attached to, every 10 seconds
in the background. Each fetch is a `claude logs` process (~0.65 s, ~0.17 s
CPU, ~150 MB while it runs), whether the logs panel is open or not. The
owner approved making the interval a setting, with a way to turn it off.

A capture holds only the last ~450 writes, about 40 seconds of a working
session ([reference](reference/claude-code-cli.md#claude-logs-output)). Two
captures must overlap for the stream to be stitched; when they do not, the
stream starts over without the last full repaint and the panel shows
fragments again (the partial note of 0030).

## Decision

**Key.** Top-level `logs_follow_secs`, next to `poll_interval_secs`,
default `10`. Allowed: `0` (off) or `5` to `30`. Anything else is a config
error naming the range (`logs_follow_secs must be 0 (off) or 5 to 30
seconds, not 31`), as for the poll interval.

- **Why at most 30.** A capture covers about 40 seconds of a working
  session. At 30 seconds a capture still overlaps the previous one, with a
  margin for a fetch that takes long; a longer interval loses the overlap
  whenever the session writes steadily, and the screen breaks up again.
- **Why at least 5.** At five seconds each capture already overlaps the
  next seven; shorter intervals only add `logs` processes (~0.65 s and
  ~150 MB each).

**Off** (`0`) means no periodic fetches: neither the plan's working rows
nor the session tower is attached to are fetched at a cadence. Everything
else stays as 0030 has it:

- The open logs panel fetches on every poll of its owner, at once on
  another row and on `End`, and feeds the same stream, so it keeps the
  stitched screen while it is open.
- Tower still fetches once right after it dispatches a session (`n`),
  starts the orchestrator (`o`) and an attach returns.
- Streams are kept, bounded and dropped as in 0030; only fetches feed them.

So with the follow off, a session that worked for more than about 40
seconds while its panel was closed shows the partial note until it is
attached once (the repaint on attach is caught by the fetch after it,
unless the session writes more than a capture before the attach returns).

**Settings window.** In the General section, after the poll interval: the
row `follow logs in background` steps through `off`, `5 s`, `10 s`,
`15 s`, `20 s`, `30 s` like a choice (`←` / `→`, or `Enter` for a pick
list); `Del` resets it to the default. The file may hold any allowed value
(`7` by hand); the arrows then go to the next step up or down. Saving
writes only that key, as every save of [0022](0022-tower-writes-its-own-config.md).

**Applied live.** A save or a reload that changes the key tells the running
follower at once (`Follower::set_cadence`, a message to its thread, so no
restart and no race with running fetches): each session's next background
fetch is due one new interval after its last fetch, `0` stops the
background fetches, and a later non-zero value resumes them at once for
the sessions that are due.

## Consequences

- A user who rarely opens the logs panel can save the `logs` processes of
  working sessions, at the cost of the partial note on sessions that worked
  for a while unseen.
- Shorter intervals keep the overlap for faster writers at more cost; the
  30-second cap keeps a misconfiguration from silently breaking the screen.
- The default is unchanged, so nothing changes for users who do not set it.

## Alternatives considered

- **Any number of seconds.** Larger values look valid but lose the overlap
  for any steadily working session; the cap makes the limit visible.
- **Off also stops the panel's fetches.** The panel then shows nothing new;
  its fetches are on demand and few.
- **A separate on/off switch and an interval.** Two keys for one setting;
  `0` as off is what the window shows as `off`.
- **Restart the follower with new settings.** Its streams would be lost and
  running fetches would report to a stopped thread; a message keeps both.
