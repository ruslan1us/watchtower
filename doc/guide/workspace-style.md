# Workspace style (experimental)

Claude Code's real terminal in a pane beside the session list. Back to the [README](../../README.md).

`[ui] style = "workspace"` (also in Settings, `c`) draws the list in a bar and Claude Code's
real terminal in a pane beside it, where the details panel was. It's still experimental: it
has been used in Warp, locally and over SSH, but other terminals are untested, and Claude
Code's fullscreen view it relies on is a research preview
([0045](../0045-workspace-style.md)).

- Nothing attaches at the start: the pane shows **Home**, with no row selected: the version, the
  counts per group, the sessions that need you, each account's plan usage when `[usage] show` is
  on, and the main keys. The first `↓` or click chooses a row. `h` or a click on `◆ tower` goes
  back to Home and closes nothing; an unused terminal is released after 10 minutes as usual.
- Selecting a running session attaches to it in the pane; moving the selection wakes nothing.
  A stopped session shows `Enter wakes it here`, and only `Enter` does.
- Sessions are cards, outlined in their state's colour; the selected one has a heavy outline and
  is highlighted whole, and `●` marks attention. `[ui] card_size` (also in Settings) is `large`
  (4 rows), `medium` (3, the default) or `small` (2, a coloured edge and no outline); `[ui]
  card_gap` (0 or 1, default 1) is the blank row between cards and before each group. Cards show
  the account and a `+N` subagent badge; git stays in the pane's title. A click on a card selects
  it, a click on a group folds or unfolds it, and the wheel moves through the list.
- `o`, start-and-attach in the new-session dialog and renaming a running session open it in the
  pane with the keys; `Ctrl+Z` or `F12` returns. A session just started opens when it is listed.
- `!` opens a shell in the row's folder in the pane, on stopped rows too, without waking them. It
  lives beside the Claude terminal and stays while you browse: `!` on its row toggles between the
  two, `!` on another row replaces it, and `exit` returns to the list. `Ctrl+Z` and `F12` return to
  the list here too, so `Ctrl+Z` is not the shell's job control.
- For a program that does not ask for the mouse (the shell at its prompt, `less`), a drag in the pane
  selects text, and the release copies it with OSC 52 and says `copied N characters`; tower never runs
  a clipboard tool. A double click takes a word, a click or `Esc` clears it, and output over it ends
  it. A program that asks for the mouse, Claude Code among them, keeps getting everything.
- The shell keeps 10,000 lines of history (about 42 MB at 130 columns when full): the wheel and
  `Shift+PgUp` / `Shift+PgDn` scroll it while the program is not on the alternate screen, the title says
  `↑ N lines`, a selection works up there, and any key for the program returns to the bottom.
- `z` or `F2` is zoom: the terminal fills the window and has the keys, for Claude Code and the shell,
  with one footer row saying `ZOOM`. `F2` leaves it, and `Ctrl+Z` or `F12` also return to the list.
- A list that does not fit says so: `▲ 3 more` and `▼ 5 more`, which a click moves by a page. With
  `[usage] show` on, each account's usage is fixed at the bar's bottom, and on Home with reset
  times.
- `Enter` gives the terminal the keys, and `●` clears then. Every key goes to Claude Code,
  `Ctrl+C` too, except `Ctrl+Z` and `F12`, which return to the list and stay attached. After
  10 minutes without the keys the terminal is released.
- `s`, `d`, `a`, `e` and `t` close that session's terminal first and work as usual; `Enter`
  attaches again. `U` closes it too, and it comes back by itself once the session has restarted.
  Windows draw over the pane and take the keys meanwhile.
- The mouse is captured in this style, so Claude Code gets it and a click moves the keys; hold
  `Shift` for your terminal's own selection (zoom leaves no frame in its way), or set `[ui] mouse =
  false`. Clipboard writes
  from Claude Code pass to your terminal (Warp may need its OSC 52 setting enabled). Bells and
  other escape sequences do not.
- It needs 100x24; a smaller terminal gets the dashboard until it grows, though zoom goes on and
  `z` opens it there. `[ui] list_width` sets the bar's width (20 to 60).
- `←` on an empty prompt opens Claude Code's agent view inside the pane, where it can show
  another session than the selected row.
