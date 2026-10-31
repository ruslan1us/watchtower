# 0046. Colour themes from published palettes

- Status: Accepted · 2026-10-31
- Related: [0007](0007-visual-only-attention.md), [0013](0013-privacy-and-repository-hygiene.md), [0020](0020-visual-styles.md), [0022](0022-tower-writes-its-own-config.md), [0038](0038-light-and-16-colour-themes.md), [0045](0045-workspace-style.md)
- Amends: the theme list of [0021](0021-dashboard-themes.md) and [0002](0002-tech-stack-and-files.md), and the pick list of [0024](0024-settings-window.md)

## Context

Users of a terminal palette want tower in it. The palettes are made for code on
their own background; tower needs more from them: seven group colours that stay
apart on a card's outline ([0045](0045-workspace-style.md)), a red that means
attention ([0007](0007-visual-only-attention.md)), text readable on a filled
selected card, and contrast targets ([0038](0038-light-and-16-colour-themes.md)).
It also paints no background ([0021](0021-dashboard-themes.md)), so what the user
sees depends on a terminal background tower does not set.

How others apply them: k9s's Catppuccin skin uses red for errors, peach for
pending, green for added, a muted grey for completed; helix's uses red, yellow,
sky and teal for error, warning, info and hint, with a surface colour for the
selection. btop's Dracula uses pink for the selected row's background and leaves
the main background empty on request, for transparency; lazygit defaults to the
terminal's own colours with a blue selected line. So: meaning first, the palette's
own hue for it, the selection on a surface colour, the background left alone.

## Decision

**Ten more themes** for `[ui] theme`, after the seven of 0021 and 0038 in the
config error and the Settings window. Each is one `Palette` in `src/ui/theme.rs`;
`tokyo-night` and the other themes are unchanged.

| Theme | Palette, source of the values | Licence, credit |
|---|---|---|
| `catppuccin-mocha`, `catppuccin-latte` | [Catppuccin](https://catppuccin.com/palette/) (`catppuccin/palette`, `palette.json`) | MIT, © Catppuccin |
| `dracula` | [Dracula specification](https://spec.draculatheme.com/) | MIT, © Dracula Theme |
| `gruvbox-dark`, `gruvbox-light` | [Gruvbox](https://github.com/morhetz/gruvbox) (`colors/gruvbox.vim`), medium contrast | MIT/X11 as its README says, by morhetz |
| `kanagawa` | [Kanagawa](https://github.com/rebelot/kanagawa.nvim) (`lua/kanagawa/colors.lua`), the wave variant | MIT, © Tommaso Laurenzi |
| `nord` | [Nord](https://www.nordtheme.com/docs/colors-and-palettes) | MIT, © Sven Greb |
| `one-dark`, `one-light` | Atom's [One Dark](https://github.com/atom/atom/blob/master/packages/one-dark-syntax/styles/colors.less) and [One Light](https://github.com/atom/atom/blob/master/packages/one-light-syntax/styles/colors.less) syntax themes; [onedark.vim](https://github.com/joshdick/onedark.vim) for the UI greys | MIT, © GitHub Inc.; MIT, © Joshua Dick |
| `rose-pine` | [Rosé Pine](https://rosepinetheme.com/palette/) (`rose-pine/neovim`, `palette.lua`) | MIT, © Rosé Pine |

Colour values only: no code or theme file of these projects is copied, and no new
dependency.

**Rejected:**

- *Solarized*, both (MIT, © Ethan Schoonover). Its body text is 4.75:1 in dark and
  4.13:1 in light, and 4 of its 8 accents need a move above ΔE 5 on either side (red
  is 3.25:1 in dark). It is defined by those fixed lightness relations, so moving them
  is no longer Solarized.
- *Rosé Pine Dawn*: 4 of the 6 accents move more than ΔE 5, gold by 20.6 (2.05:1),
  rose by 13.8.
- *Everforest light* (MIT, © sainnhe): text 5.18:1, every accent below 3.2:1.
- *Everforest dark*: it meets the contrast targets, but its yellow and green are 7.3
  apart, 1.0 under protanopia, and its blue and grey 5.5.
- *Kanagawa Lotus*: its green and grey are 7.6 apart, with two big accent moves.

**How a palette is mapped**, the same for all:

1. A published colour is used unchanged wherever it reaches the target.
2. Otherwise it moves in OKLCH lightness only, hue and chroma kept, by the smallest
   step that reaches the target; a neutral with no such colour is the palette's
   foreground over its own background or comment colour.
3. Every value moved is listed below. A palette is rejected when its body text must
   move by more than ΔE 5, or more than half of the accents it uses move by more
   than ΔE 5. (ΔE: distance in OKLab, ×100.) `catppuccin-latte` is exactly at that
   limit. Secondary text stays a step from text, as elsewhere in tower.
4. Meaning decides the role, not the palette's name for the colour: failed and `●`
   are its red, needs me its yellow, ready for review its green, working its blue,
   stopped its grey. Where that collapses, another hue takes the role:

| Theme | Needs me | Working | Ready | Failed, `●` | Stopped | Orchestrator | Other |
|---|---|---|---|---|---|---|---|
| catppuccin-mocha | yellow | blue | green | red | overlay 2 | mauve | peach |
| dracula | yellow | cyan | green | red | comment over fg | purple | orange |
| gruvbox-dark | yellow | purple | green | red | fg4 | aqua | orange |
| kanagawa | carp yellow | crystal blue | spring green | peach red | fuji grey | wave aqua | surimi orange |
| nord | nord13 | nord8 | nord14 | nord11 | snow over nord0 | nord15 | nord12 |
| one-dark | yellow | blue | green | red | mono-2 | purple | dark yellow |
| rose-pine | gold | iris | foam | love | subtle | leaf | rose |
| catppuccin-latte | yellow | blue | green | red | subtext 1 | mauve | peach |
| gruvbox-light | yellow | blue | aqua | red | fg3 | purple | orange |
| one-light | hue-6 | hue-2 | hue-4 | hue-5-2 | mono-2 | hue-3 | hue-5 |

Teal is not used for the orchestrator in most: next to green it reads as the same
state. Gruvbox's blue is 5.6 from its grey, so its purple means working in dark;
Rosé Pine has no green, so foam means ready, as in its own git colours; its `ok`
colour, leaf, is a grey-green that sits nearer its grey, so it marks the orchestrator.

**Moved values** (published → used, ΔE). `catppuccin-mocha` moves none.

| Theme | Accents and text | Neutrals |
|---|---|---|
| dracula | none | selection `#44475a` → `#404355` (1.5), so red and grey stand at 3:1 on it; dim and stopped `#8491b6`, secondary `#b0b8cd`: fg over comment |
| gruvbox-dark | red `#fb4934` → `#fe513c` (1.4) | none |
| kanagawa | none | fuji grey `#727169` → `#88877f` (7.5) as stopped |
| nord | nord11 `#bf616a` → `#e28088` (10.2, reads as salmon), nord12 `#d08770` → `#e79c84` (6.8), nord15 `#b48ead` → `#c19bba` (4.2) | nord3 is 1.69:1: dim and stopped `#979da9`, lines `#797f8a`, nord4 over nord0 |
| one-dark | text `#abb2bf` → `#b9c0cd` (4.4), secondary `#b1b8c5` (1.9); red `#e06c75` → `#e36f78` (0.9) | mono-2 `#828997` → `#8c93a1` (3.3), mono-3 `#5c6370` → `#6e7583` (6.3); bar text `#e6e9ef` |
| rose-pine | none | secondary `#a4a2b6`: text over base |
| catppuccin-latte | text `#4c4f69` → `#3f425c` (4.8), the published one is secondary; yellow `#df8e1d` → `#9b5f02` (17.7), peach `#fe640b` → `#bd4700` (14.5), green `#40a02b` → `#217f02` (10.4), teal `#179299` → `#037a81` (7.5, a chip only), blue `#1e66f5` → `#1b63f1` (1.0) | bar text `#2a2c3e` |
| gruvbox-light | yellow `#b57614` → `#996102` (7.7), aqua `#427b58` → `#407956` (0.7) | none |
| one-light | hue-4 `#50a14f` → `#318332` (9.7), hue-5 `#e45649` → `#ce4136` (6.2), hue-5-2 `#ca1243` → `#b30f3c` (4.9, so it stays off green in deuteranopia), hue-2 `#4078f2` → `#356ce5` (3.8), hue-6 `#b76b01` → `#a86201` (3.7), hue-1 `#0184bc` → `#037aae` (3.3) | mono-3 `#a0a1a7` → `#909096` (5.5); secondary `#4c4e57`: mono-1 towards mono-2; bar and surface: mono-1 over its background |

**Targets**, on each palette's own background: text and secondary ≥ 7:1, dim and
accents ≥ 4.5:1, lines ≥ 3:1, text on the selection ≥ 7:1, pills and chips ≥ 4.5:1,
`▌` / `▶` on the bar ≥ 4.5:1. New: each state colour on the bar ≥ 3:1, as a selected
card's outline and `●` ([0045](0045-workspace-style.md)). A unit test checks every
role and the windows' styles; `cargo test contrast_table -- --nocapture` prints:

| Theme | text | secondary | dim | lines | accents | red | bar text | `▌` | pills | chips | on bar | apart | protan | deutan |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| catppuccin-mocha | 11.34 | 9.26 | 5.81 | 3.36 | 5.81 | 7.08 | 8.69 | 7.03 | 5.81 | 5.61 | 4.45 | 11.0 | 2.4 | 6.6 |
| dracula | 13.36 | 7.18 | 4.55 | 3.03 | 4.53 | 4.53 | 9.15 | 8.73 | 4.53 | 5.05 | 3.11 | 15.9 | 3.1 | 8.3 |
| gruvbox-dark | 10.75 | 8.59 | 5.30 | 3.03 | 4.53 | 4.53 | 10.22 | 4.59 | 4.53 | 4.72 | 3.56 | 9.5 | 3.6 | 2.8 |
| kanagawa | 11.26 | 8.89 | 5.02 | 3.33 | 4.53 | 5.44 | 8.93 | 7.72 | 4.53 | 4.94 | 3.59 | 11.9 | 4.1 | 4.4 |
| nord | 10.26 | 9.25 | 4.59 | 3.10 | 4.57 | 4.57 | 7.49 | 7.49 | 4.57 | 4.75 | 3.15 | 9.5 | 5.4 | 5.6 |
| one-dark | 7.66 | 7.02 | 4.54 | 3.02 | 4.54 | 4.54 | 8.02 | 5.65 | 4.54 | 4.79 | 3.16 | 10.6 | 2.1 | 6.5 |
| rose-pine | 13.39 | 7.08 | 5.48 | 3.42 | 5.48 | 6.07 | 7.93 | 6.19 | 5.48 | 5.64 | 3.25 | 12.2 | 3.7 | 6.5 |
| catppuccin-latte | 8.66 | 7.06 | 5.53 | 3.49 | 4.53 | 4.80 | 8.90 | 5.17 | 4.53 | 4.77 | 3.32 | 16.0 | 5.0 | 2.8 |
| gruvbox-light | 10.22 | 7.78 | 5.74 | 3.24 | 4.53 | 7.60 | 10.75 | 4.91 | 4.53 | 4.63 | 3.75 | 9.7 | 5.7 | 4.6 |
| one-light | 10.86 | 7.94 | 5.01 | 3.04 | 4.54 | 6.60 | 9.01 | 4.86 | 4.54 | 4.52 | 3.76 | 14.6 | 5.0 | 3.9 |

Each column is the weakest of its roles.

**Distinct states.** The five a user tells apart on an outline (needs me, working,
ready, failed, stopped) are at least 9 apart in OKLab (ΔE, `apart` above), and 6 at
256 colours; the orchestrator and Other stay at least 5 off them. Under simulated
protanopia and deuteranopia (Machado et al. 2009, severity 1, `protan` and `deutan`,
the closest pair) no pair falls below 2, and failed and ready stay 3 apart. That is a
floor, not comfort: yellow and green come close in several palettes under protanopia
and the state is then told by its word and glyph, as everywhere in tower
([0007](0007-visual-only-attention.md)).

**The background stays the terminal's** ([0021](0021-dashboard-themes.md) rejected
painting one). The targets are computed on the palette's own background, the harder
case: a darker terminal under a dark palette, or a lighter one under a light palette,
only adds contrast. So every target also holds on plain black or plain white, and a
test checks it. What the user sees:

- **The terminal set to the palette's background**: the palette as made. Tower does
  not set it or ask for it (it never writes or queries OSC 10, 11).
- **Plain black under a dark palette, or white under a light one**: readable, every
  target kept, a little crisper than designed; the base colour's character is missing,
  and the selection, pills and chips are painted in the palette's own colours.
- **A dark background lighter than the palette's**: below target. `catppuccin-mocha`
  on `#3b4252` has text at 6.96 and dim at 3.56, `dracula`'s dim is 3.21.
- **The wrong side**: unreadable, as with every truecolor theme since 0021. A dark
  palette's text on white is 1.07 to 1.83, a light palette's on black 1.81 to 2.15.
  Tower does not guess the background ([0038](0038-light-and-16-colour-themes.md));
  the Settings help says so and the live preview shows it at once.

**Colour depth** ([0038](0038-light-and-16-colour-themes.md)). At 16 colours they
give way to `ansi16`, like every truecolor theme. At 256 they stay, nearest in the
cube and ramp, where they are approximate: the published values are kept for truecolor,
not bent towards the cube. Only what tower paints is reduced, so roles on the terminal
are measured against the real background. A test holds them to lower floors there: 6
for a target of 7, 3.5 for 4.5, 2.5 for 3. The weakest at 256:

| Theme | text | secondary | dim | lines | accents | bar text | `▌` | pills | chips | on bar | apart |
|---|---|---|---|---|---|---|---|---|---|---|---|
| catppuccin-mocha | 11.76 | 7.76 | 4.78 | 3.61 | 4.78 | 8.15 | 5.63 | 4.41 | 5.44 | 3.31 | 10.8 |
| dracula | 12.27 | 6.74 | 4.15 | 2.54 | 4.15 | 8.39 | 9.23 | 3.84 | 5.25 | 2.84 | 16.1 |
| gruvbox-dark | 9.99 | 9.99 | 4.66 | 2.81 | 4.66 | 11.12 | 4.72 | 4.78 | 4.84 | 3.60 | 6.7 |
| kanagawa | 11.07 | 8.07 | 4.76 | 3.11 | 4.55 | 8.94 | 6.52 | 4.21 | 4.84 | 3.67 | 13.0 |
| nord | 10.76 | 9.82 | 4.66 | 3.16 | 4.58 | 7.17 | 7.17 | 4.17 | 4.76 | 3.05 | 7.1 |
| one-dark | 8.20 | 7.37 | 4.61 | 3.08 | 3.92 | 8.39 | 4.81 | 3.69 | 4.07 | 2.73 | 8.6 |
| rose-pine | 12.66 | 7.43 | 5.14 | 2.92 | 4.94 | 6.98 | 4.94 | 4.77 | 5.76 | 2.73 | 11.4 |
| catppuccin-latte | 7.36 | 6.29 | 5.35 | 3.18 | 3.63 | 8.56 | 4.61 | 3.54 | 3.76 | 2.66 | 16.7 |
| gruvbox-light | 10.03 | 7.33 | 5.63 | 3.48 | 3.62 | 10.25 | 3.71 | 4.01 | 3.54 | 2.78 | 10.2 |
| one-light | 10.90 | 7.97 | 5.03 | 2.91 | 3.53 | 8.95 | 4.80 | 3.69 | 4.10 | 2.90 | 9.1 |

**Settings pick list** ([0024](0024-settings-window.md)). The seventeen themes fit at
80×24 in 21 rows, swatches included. A list longer than its rows, here below 24 rows,
where the window has the minimal look without swatches, scrolls: the cursor stays near
the middle while the list goes on either way, and the rows above and under the items
say `↑ 3 more` and `↓ 2 more`. The value column widens to the longest name.

## Consequences

- People get the palette they use elsewhere, and its look when the terminal's
  background matches. It is not an exact copy: moved values are listed above.
- `catppuccin-latte` is the one a Latte user will notice most: its yellow, peach and
  green are far deeper than published. `nord`'s red reads as salmon.
- `one-dark` and `catppuccin-latte` have body text a step stronger than published, as
  their published text is at or under 7:1 and secondary text needs room below it.
- `tests/ui_audit.rs` draws `catppuccin-mocha` and `catppuccin-latte` as its
  representatives; `tests/ui_themes.rs` draws every new theme's main windows.
- A theme is now a mapping to review, not just a palette: distinct states and the
  bar's outlines are part of the targets for any later one.

## Alternatives considered

- **Paint the palette's background.** The look would be exact everywhere, but 0021
  rejected it: it hides the user's background and transparency and mismatches at the
  edges, and it would have to stay out of the terminal in the workspace's pane.
- **Set the terminal's background with OSC 11.** Changes the user's terminal, and
  would have to be undone on every exit path, a crash included.
- **Use published colours only, and relax the targets per palette.** The owner asked
  for the same targets; a red of 3:1 for attention or a yellow of 2:1 on a card is
  what they exclude.
- **Keeping the full targets at 256 by bending truecolor values to the cube.** It would
  move every palette further from its published values for a depth set by hand.
- **Teal for the orchestrator everywhere.** Close to green in the pastel palettes.
