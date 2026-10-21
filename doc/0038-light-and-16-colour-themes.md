# 0038. Light themes, a 16-colour theme and the colour depth

- Status: Accepted · 2026-10-21
- Related: [0002](0002-tech-stack-and-files.md), [0007](0007-visual-only-attention.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md), [0021](0021-dashboard-themes.md), [0022](0022-tower-writes-its-own-config.md), [0024](0024-settings-window.md)
- Amends: the theme list and open items of [0021](0021-dashboard-themes.md), the editable keys of [0022](0022-tower-writes-its-own-config.md) and the Appearance rows and live preview of [0024](0024-settings-window.md)

## Context

The truecolor themes of [0021](0021-dashboard-themes.md) assume a dark
background: on white, `neutral`'s text (`#ebebeb`) is 1.2:1. `terminal`
follows the terminal's palette but draws states in the bright colours, which
vanish on a light background (in xterm's default palette `LightYellow` on
white is 1.07:1, `LightCyan` 1.25:1). A terminal that shows only 16 colours
(the Linux console, some SSH and multiplexer setups) cannot show the
truecolor themes at all: their 24-bit escapes are approximated or misdrawn.
0021 left both open.

Whether the background is light cannot be told reliably without asking the
terminal (an OSC 11 query, answered by some terminals and multiplexers and
not others); tower never queries the terminal. How many colours it shows is
announced, imperfectly, in the environment: `COLORTERM` and `TERM`.

## Decision

**Three more themes** for `[ui] theme`, listed after the four of 0021 in the
config error and the Settings window: `neutral-light`, `warm-light` and
`ansi16`. The user picks a light theme; tower does not guess the background.
`terminal` has no light variant (it already takes the terminal's own
colours), nor has `tokyo-night` (0021 keeps it as the 0.2 palette,
unchanged).

**Light variants.** The same roles as `neutral` and `warm`, tuned for a
white background: dark greys, deeper state accents with white text on their
pills, a light selection bar with near-black text, `warm-light`'s account
chips on a pale tint (12 % of the hue over white). Targets against
`#ffffff`, checked by a unit test over every role (the windows' too): text
and secondary ≥ 7:1, dim and state accents ≥ 4.5:1, lines ≥ 3:1, text on the
selection bar ≥ 7:1, on pills and chips (bold) ≥ 4.5:1, the bar's `▌` / `▶`
≥ 4.5:1.

| Role | neutral-light | warm-light |
|---|---|---|
| text | 16.48 | 15.48 |
| secondary | 8.19 | 7.70 |
| dim | 5.33 | 5.27 |
| lines, borders | 3.45 | 3.37 |
| weakest state accent, the same on its pill | 5.39 | 5.73 |
| attention, errors (red) | 5.63 | 5.95 |
| selected row text on its bar | 14.45 | 13.76 |
| `▌` / `▶` on the bar | 14.45 | 5.06 |
| weakest account chip | 12.02 | 4.84 |
| logo, titles, keys | 16.48 | 6.81 |

**`ansi16`**: only the 16 named colours (`Black` … `White`, the default
colour), no RGB or indexed colour anywhere, and attributes where a colour
cannot be trusted on both backgrounds:

- text, secondary text, keys, titles and account names are the default
  foreground (bold where 0021 makes them bold); dim text and lines are the
  default foreground with the dim attribute;
- hues (`Red`, `Yellow`, `Green`, `Blue`, `Cyan`, `Magenta`, `Gray`) go on
  glyphs (`▪`, the spinner), the share bar, the detail panel's border and the
  pills, never on text: the state column, group titles, warnings and `✓
  saved` are plain or bold. Attention stays the `●` marker plus bold red
  ([0007](0007-visual-only-attention.md));
- pills are fixed pairs of palette colours, independent of the background:
  white on red, blue and magenta, black on yellow, green, cyan and gray, not
  bold (xterm draws bold black as dark grey); the selection and the
  Interactive pill are reverse video.

No 16-colour palette is the same in every terminal. The pills' weakest pair
is 4.7:1 in xterm's defaults, 5.4:1 in Windows Terminal's Campbell, 4.0:1 in
the VGA palette of the Linux console, 3.6:1 in GNOME's Tango and 3.4:1 in
Solarized.

**Colour depth**, a new key `[ui] color_depth`: `"auto"` (the default),
`"truecolor"`, `"256"` or `"16"`; anything else is a config error.

- `auto` is read once at start, never by asking the terminal. `COLORTERM`
  `truecolor` or `24bit` is full colour. Otherwise only a `TERM` known to
  be limited means 16 colours: unset or empty, `dumb`, `linux`, the DEC
  terminals (`vt52`, `vt100`, `vt102`, `vt220`, any `vt` plus a digit),
  `ansi`, `cons25`, `cygwin`, `sun`, `xterm-color` and any `*-16color`.
  Every other `TERM` (`xterm`, `screen`, `tmux`, `rxvt`, `*-256color`,
  `*-direct`, …) draws in full colour exactly as before this decision, with
  no reduction. The environment is a weak signal: SSH forwards `TERM` but
  usually not `COLORTERM`, so a truecolor terminal over SSH shows up as
  `xterm-256color`; `screen` and `tmux` sit inside a multiplexer that
  down-samples colours itself; and plain `xterm` is often a capable
  terminal. Tower never queries the terminal, so it cannot tell these from
  limited ones. `auto` therefore changes nothing for anyone whose terminal
  is not known to be limited, and never picks 256.
- At **16** colours a theme that needs more (`neutral`, `warm`,
  `tokyo-night` and the light ones) gives way to `ansi16`; `terminal` and
  `ansi16` stay. The status line says so once per run.
- At **256** colours, only when set explicitly, the theme stays; every RGB
  colour is drawn as the
  nearest colour of the 6×6×6 cube and grey ramp (indices 16 to 255, never
  the terminal's own 0 to 15). Text, secondary, dim, lines, state accents,
  pills and the selection bar of the four truecolor themes with targets
  (`neutral`, `warm` and their light variants) keep them at 256 colours, a
  unit test checks it; only `warm-light`'s `▌` / `▶` on its bar drops, to
  4.1:1.
- The reduction runs on the drawn frame (`ui::theme::reduce_buffer`), so it
  covers everything: at 16 colours the logs panel's replayed colours and the
  theme swatches become the nearest named colour too.
- `NO_COLOR` keeps its meaning: no colour at any depth.

**Settings window** ([0024](0024-settings-window.md)): `colour depth` is an
Appearance row after `theme`, a choice (`auto`, `truecolor`, `256`, `16`)
changed with `←` / `→` or its pick list, which says what `auto` finds here.
It is previewed live like the style and theme: the pick list's value under
the cursor, else the unsaved value, else the running one; closing without
saving reverts it. The header names it (`colour depth 16`). It applies at
once after a save, like the theme.

## Consequences

- Light-terminal users pick `neutral-light` or `warm-light` and get the
  same layout and text in readable colours.
- Nothing visible changes for anyone whose `TERM` is not on the limited
  list, SSH and multiplexers included: the output is the same RGB as
  before.
- A real 256-colour terminal with a `256color` `TERM` still receives RGB,
  as before; `color_depth = "256"` gives it the approximation.
- A 16-colour terminal with an ordinary `TERM` (an old `xterm`, a
  `screen` outside a multiplexer) still receives RGB, as before;
  `color_depth = "16"` gives it `ansi16`. A capable terminal with a
  limited `TERM` (`linux` over a serial console, `xterm-color`) gets
  `ansi16`; `color_depth = "truecolor"` brings its theme back.
- `ansi16` looks plainer than the truecolor themes: its colour is in glyphs
  and pills, not in text.
- `terminal` is unchanged; on a light background `ansi16` is the safer
  16-colour choice.

## Alternatives considered

- **Detecting a light background** with an OSC 11 query or `COLORFGBG`.
  Rejected: tower does not query the terminal, and `COLORFGBG` is set by
  only some terminals and goes stale when the profile changes.
- **Falling back to `terminal`** at 16 colours. Its bright hues on text fail
  on light backgrounds, which a fallback cannot rule out.
- **Falling back to 16 colours at 256** as well. The cube and ramp
  approximate the truecolor palettes closely enough to keep their targets.
- **Detecting 256 colours from a `256color` `TERM`** and approximating.
  Rejected: over SSH (`COLORTERM` not forwarded) a truecolor terminal would
  lose its exact colours, a regression for users who saw them before.
- **16 colours for every `TERM` without `256color`**. Rejected: `xterm`,
  `screen` and `tmux` are common on capable terminals and inside
  multiplexers, and would lose their themes.
- **Changing `terminal`** to the attribute rules of `ansi16`. It would
  change the look of a theme users chose; a new theme leaves it alone.
