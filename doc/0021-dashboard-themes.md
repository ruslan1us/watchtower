# 0021. Dashboard colour themes

- Status: Accepted · 2026-10-19 · light themes, `ansi16` and the colour depth added by [0038](0038-light-and-16-colour-themes.md) · the workspace's selected card is painted whole, an exception recorded in [0045](0045-workspace-style.md) · published palettes added by [0046](0046-more-colour-themes.md)
- Related: [0002](0002-tech-stack-and-files.md), [0007](0007-visual-only-attention.md), [0016](0016-do-not-change-claude-code-defaults.md), [0020](0020-visual-styles.md)
- Supersedes: the "Colour" section of [0020](0020-visual-styles.md)

## Context

The dashboard style ([0020](0020-visual-styles.md)) shipped with one dark
truecolor palette, a blue-grey "Tokyo Night" look. The owner reported it
unreadable on a black terminal. The dashboard paints nothing behind
ordinary text, so the terminal's own background shows through; the problem
is that the secondary, dim and line colours are dark blue-greys with too
little luminance against black.

Three alternatives were rendered by the real binary against a fake
`claude` and measured as WCAG 2 contrast ratios against `#000000`:

| Role | tokyo-night | neutral | warm |
|---|---|---|---|
| text | 13.0 | 17.6 | 16.9 |
| secondary | 5.0 | 9.3 | 9.6 |
| dim | 3.2 | 6.1 | 6.1 |
| lines, borders | 2.1 | 4.1 | 4.1 |
| selected row text on its bar | 6.2 | 11.4 | 11.7 |
| weakest state pill (text on accent) | 5.0 | 7.3 | 7.1 |
| account chip (text on its background) | 4.1 | 8.7 | 7.7 |

The fourth, `terminal`, has no fixed values: it uses the terminal's named
colours, so its contrast is whatever the user's terminal theme gives.

## Decision

**Four themes**, picked with `[ui] theme` in `config.toml`
([0002](0002-tech-stack-and-files.md)). Any other value is a config error
that lists the valid names.

- `neutral`, **the default**: no tint. Text a light grey, secondary, dim
  and lines in steps of grey. Colour only where it carries meaning: state
  pills, the share bar, group titles and `▪` glyphs, the state column and
  the attention marker, with bright saturated accents. Titles, keys,
  account chips and the account column are white or grey; every border,
  the detail panel's included, is the line grey. The selected row is a
  dark-grey bar with white text, its `▌` white and the `●` still red.
- `warm`: warm greys and a clay-orange accent for `◆ tower`, panel titles,
  key hints and the selection bar's `▌`; a dark warm-brown selection with
  near-white text; warmer state accents; account chips keep a hue on a
  faint tint.
- `terminal`: named ANSI colours only, no truecolor. Text is the default
  foreground, secondary `Gray`, dim and lines `DarkGray`, states in the
  bright named colours. The selected row and the pills use **reverse
  video**, so their text is the terminal's own background. It adapts to
  the user's terminal theme, light ones included, and to terminals
  without truecolor.
- `tokyo-night`: the 0020 palette, unchanged, for anyone who liked it.

*Amended by [0038](0038-light-and-16-colour-themes.md):* also
`neutral-light` and `warm-light` for light backgrounds, and `ansi16` (the 16
named colours plus attributes, for dark and light backgrounds), which the
truecolor themes give way to on a 16-colour terminal (`[ui] color_depth`).

*Amended by [0046](0046-more-colour-themes.md):* also ten published palettes:
`catppuccin-mocha`, `dracula`, `gruvbox-dark`, `kanagawa`, `nord`, `one-dark`,
`rose-pine` for dark backgrounds, `catppuccin-latte`, `gruvbox-light`, `one-light`
for light ones, each with its own background as its contrast reference.

**Contrast targets** for `neutral` and `warm`, on black: text and
secondary ≥ 7:1, dim ≥ 4.5:1, lines ≥ 3:1, state accents ≥ 4.5:1, and any
text on a painted background (selection, pills, chips) ≥ 7:1. A unit test
computes them from the palettes.

**Backgrounds.** No theme paints behind ordinary text or fills panels:
painted backgrounds are only the selected row (and the selection bar in
windows), the state pills and the account chips. The terminal's
background stays the background.

**Overlays.** In the dashboard style every window over the screen follows
the theme: the new-session dialog, the resume window, confirmations and
the quit questions, the account and target pickers, help, popups, the
"not trusted" windows and the filter's input line. They have rounded
borders in the line colour with the title in the title colour; labels
are secondary, values text, hints dim with key names in the key colour;
the selected item (a project, an account, a focused choice) and the
dialog's discard and quit questions sit on the selection bar with `▶` in
the mark colour (reverse video in `terminal`); errors are the theme's
red, the Haiku warning its amber. The styles come from one place, `Look`
in `src/ui/theme.rs` (`overlay_block`, `field_label`, `hint_line`,
`selected_item`, `selected_marker`, `choice`, `prompt`, `error`,
`warning`, …), for later windows such as settings to reuse. Text, layout
and sizes are the same as in the minimal style, which keeps its own look,
as does the minimal fallback below 80×24. The contrast test covers the
windows' roles too, with 4.5:1 for the `▶` marker on the bar.

**Scope.** The theme applies to the dashboard style only; the minimal
style keeps its own look. The roles a theme sets are the text greys,
the selection, the state and account hues, the pill text, the chip
painting, and the logo, title, key, `Ctrl+Z`, selection marker, project
and logs-border colours. Layout and text are the same in every theme.

**`NO_COLOR`** overrides every theme: no colour at all, the selection is
`▶` plus reversed, interactive rows dim, attention stays the `●` marker
plus bold ([0007](0007-visual-only-attention.md)).

**Runtime.** The theme is app state, set from the config at start and
switchable while running, for a later settings window; no key changes it.

## Consequences

- Existing users see `neutral` after upgrading; `theme = "tokyo-night"`
  restores the old look exactly.
- `terminal` follows light terminal themes; the truecolor themes still
  assume a dark background.
- Adding a theme is one palette value; renderers take colours from the
  theme, never from constants.

## Alternatives considered

- **Keep one palette and brighten its greys.** It fixes contrast but
  forces the blue tint on everyone; the owner preferred a choice.
- **Paint a dark background behind the whole dashboard.** It hides the
  user's terminal background and transparency, and mismatches at the
  edges; rejected.
- **Pick the theme from `COLORTERM`.** Implicit and surprising; left open.

## Open items

- Choosing `terminal` automatically when `COLORTERM` does not announce
  truecolor (or offering an `auto` value).
- Light-background variants of `neutral` and `warm`.

*Both settled by [0038](0038-light-and-16-colour-themes.md):* `[ui]
color_depth` (default `auto`, from `COLORTERM` and `TERM`) chooses `ansi16`,
not `terminal`, at 16 colours; `neutral-light` and `warm-light` exist.
