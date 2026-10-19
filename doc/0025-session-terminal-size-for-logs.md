# 0025. The session terminal's real size for the logs replay

- Status: Accepted · 2026-10-19
- Related: [0001](0001-thin-wrapper-over-claude-cli.md), [0009](0009-stop-remove-and-logs.md), [0013](0013-privacy-and-repository-hygiene.md), [0015](0015-testing-strategy.md), [0017](0017-ownership-evidence-rules.md), [reference](reference/claude-code-cli.md#claude-logs-output)

## Context

The logs panel replays the raw `claude logs` stream in a terminal emulator
([0009](0009-stop-remove-and-logs.md)). The stream does not say how big the
session's terminal is, and `claude logs` has no option for it, so tower
inferred the size: the larger of 64 × 200 and the highest absolutely
addressed row and column. After its last full clear Claude Code redraws
mostly with relative moves and relies on the terminal's autowrap, so any
size other than the real one puts every redraw on the wrong cells. On a
session last attached from a 57 × 231 terminal, the stream after the last
clear addressed at most row 57 and column 24 while its prompt rules were
231 wide; tower replayed it at 64 × 200 and showed duplicated prompt lines,
overwritten spinner lines, stray fragments at line starts and broken table
borders.

A background session's worker process (the listed `pid`) has its own PTY
slave on fd 0/1/2. It is 50 × 200 until the session is first attached;
`claude attach` resizes it to the attaching terminal, and it keeps that size
after detaching ([reference](reference/claude-code-cli.md#claude-logs-output)).
The kernel reports that terminal as `tty_nr`, field 7 of
`/proc/<pid>/stat`, which tower already reads for the pid-reuse guard
([0017](0017-ownership-evidence-rules.md)).

## Decision

**Reading the size.** This extends the `/proc` exception of
[0001](0001-thin-wrapper-over-claude-cli.md) and
[0017](0017-ownership-evidence-rules.md) to the size of the session's
terminal, and to nothing else:

- Only for a row whose `pid` the pid-reuse guard has accepted: the logs job
  gets the pid and the start ticks the guard remembered for it.
- In the same background job as the `claude logs` call, right after it
  returns, tower reads `/proc/<pid>/stat` once. Field 22 must still equal
  the remembered start ticks (else the pid was reused: no size). Field 7,
  `tty_nr`, is decoded as a Linux device number
  (`major = (tty_nr >> 8) & 0xfff`,
  `minor = (tty_nr & 0xff) | ((tty_nr >> 12) & 0xfff00)`).
- Only Unix98 PTY slaves are accepted (majors 136–143); 0 (no terminal) and
  every other device give no size. The device is `/dev/pts/<n>` with
  `n = (major − 136) · 256 + minor`: a current kernel puts every slave on
  major 136 with the index as its minor, the older 8-bit-minor layout spread
  indices over majors 137–143 with minors below 256, and the formula fits
  both (a 137–143 major with a larger minor is rejected).
- Tower opens that device with `O_RDONLY | O_NOCTTY | O_NONBLOCK |
  O_CLOEXEC`, requires with `fstat` a character device whose `st_rdev` is
  the same major and minor (a wrong mapping, or another devpts instance,
  gives no size), asks `ioctl(TIOCGWINSZ)` and closes it. It never reads
  from or writes to the device and changes nothing about it.
- A size with 0 rows or 0 columns counts as unknown.

All of this is operating-system process and device information, not a
Claude Code file. The reads run on the logs worker thread, never on the UI
thread.

**Using it.** When the size is known, the emulator is exactly that size,
capped at 300 × 600. Otherwise (no pid, guard not passed, unreadable,
no PTY, failed checks, zero size) the inference of
[0009](0009-stop-remove-and-logs.md) applies unchanged. Everything after
the replay stays as it is: trimming, fitting to the panel, keeping the
bottom rows, and cutting a screen wider than the panel at its right edge
(no wrapping).

## Consequences

- The panel shows what an attached terminal of the session's size shows,
  for every session tower can see the process of. Sessions owned by another
  user, or whose process fails the guard, keep the inferred size and may
  still be garbled.
- The size is read a moment after the stream; a session attached and
  resized in between is replayed at the new size until the next refresh.
- One more small read of `/proc/<pid>/stat` and one open/`ioctl`/close per
  logs refresh, on the worker thread.
- Tests fake the reads behind a small reader trait; the device access
  itself is checked manually against a real session
  ([0015](0015-testing-strategy.md)).

## Alternatives considered

- **Better inference from the stream.** The stream after the last clear
  often carries no absolute address near the real width; wide rules and
  autowrapped text only hint at it. Guessing stays wrong in exactly the
  cases that garble.
- **Reading `/proc/<pid>/fd/0` or `/proc/<pid>/fd/*` links.** Gives a path,
  but `tty_nr` comes from the `stat` tower already reads, and the `fstat`
  check ties the opened device to it either way.
- **`stty -F /dev/pts/<n> size`.** Spawns a process for the same `ioctl`,
  with open flags and checks outside tower's control.
- **Asking Claude Code.** `claude logs` has no size option and the JSON
  listing has no size field.
