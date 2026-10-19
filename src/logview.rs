//! The logs panel's data: `claude logs <id>` fetched and replayed through a
//! terminal emulator (doc/0009).
//!
//! `claude logs` prints the session's raw terminal byte stream (a clear-screen
//! and full redraw followed by incremental updates with cursor addressing),
//! not text lines (reference/claude-code-cli.md, "`claude logs` output").
//! [`fetch`] runs the call with the owning account's command; [`render`]
//! replays the bytes into a fresh [`vt100::Parser`] and converts the final
//! screen, and the lines that scrolled off its top, into ratatui [`Line`]s
//! and plain strings. The panel keeps one emulator per session across
//! fetches instead ([`crate::logstream`], doc/0030), built from the same
//! pieces; [`LogScreen::partial`] says when no full clear was replayed. The conversion is done here rather than with
//! `tui-term`, so the panel can trim blank rows, keep only the rows that
//! fit, scroll, and fall back to plain text.
//!
//! ## History
//!
//! The emulator keeps a bounded scrollback (doc/0028): lines that scroll
//! off the top of the screen during the replay are kept, at most
//! [`scrollback_len`] of them ([`SCROLLBACK_LINES`], fewer on screens wider
//! than 200 columns, so the scrollback never holds more than
//! [`SCROLLBACK_CELLS`] cells). [`LogScreen`] keeps them above the screen;
//! [`LogScreen::history_lines`] gives both, [`LogScreen::page`] the part a
//! scrolled panel shows. Claude Code's own streams draw in place with
//! cursor addressing and never scroll the screen, so for them the history
//! is the screen (reference/claude-code-cli.md, "`claude logs` output").
//!
//! ## Emulator size
//!
//! `claude logs` does not report the PTY size. When the logs job could
//! read the session terminal's real size ([`crate::ptysize`], doc/0025),
//! [`render_sized`] uses exactly that, capped at [`MAX_ROWS`] ×
//! [`MAX_COLS`]: Claude Code redraws mostly with relative moves and relies
//! on the terminal's autowrap, so any other size garbles the replay.
//! Otherwise ([`render`], or no size) the size is inferred from the stream
//! ([`infer_size`]): the larger of [`DEFAULT_ROWS`] × [`DEFAULT_COLS`]
//! (64 × 200) and the highest row and column the stream addresses
//! *absolutely*:
//!
//! - rows: CUP/HVP `ESC[<r>;<c>H` / `ESC[<r>;<c>f` (also `ESC[<r>H`), VPA
//!   `ESC[<r>d`, and the bottom margin of DECSTBM `ESC[<t>;<b>r`;
//! - columns: the column of CUP/HVP, CHA `ESC[<n>G`, HPA ``ESC[<n>` ``
//!   (final byte a backtick), and CUF `ESC[<n>C` directly after a carriage
//!   return (column `n + 1`).
//!
//! Missing or zero parameters count as 1, private sequences (`ESC[?…`) are
//! ignored, and other relative moves are not counted (a terminal clamps them,
//! so they say nothing about its size). The result is capped at
//! [`MAX_ROWS`] × [`MAX_COLS`] so a garbage sequence cannot allocate a huge
//! screen.
//!
//! ## Robustness
//!
//! Any byte sequence is accepted: invalid UTF-8 becomes U+FFFD and broken
//! escape sequences are dropped by the emulator. Only the last
//! [`MAX_INPUT_BYTES`] are replayed; within that window replay starts at the
//! last full-screen clear (`ESC[2J` or `ESC c`) when there is one, otherwise
//! at the first `ESC` so it does not begin inside a sequence. The same window
//! is used for the size inference.
//!
//! vt100 does some sequences in time proportional to a count or to the
//! screen size, so before the replay their counts and positions are clamped
//! to the screen, and once the costly ones have used up a work budget the
//! rest are dropped (`bound_counts`). The replay of any 2 MiB window then
//! takes well under a second.

use std::borrow::Cow;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::cli::{CliError, CliRunner};
use crate::config::Account;

/// Default emulator height (doc/0009).
pub const DEFAULT_ROWS: u16 = 64;
/// Default emulator width (doc/0009).
pub const DEFAULT_COLS: u16 = 200;
/// Upper bound for the emulator height, real or inferred.
pub const MAX_ROWS: u16 = 300;
/// Upper bound for the emulator width, real or inferred.
pub const MAX_COLS: u16 = 600;
/// Only this many bytes from the end of the stream are replayed.
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
/// Most lines the replay keeps above the screen (doc/0028).
pub const SCROLLBACK_LINES: usize = 5000;
/// Most cells the scrollback holds. vt100 keeps every cell of a line (32
/// bytes each), so this caps the scrollback at 32 MB while the replay
/// runs, however wide the screen.
pub const SCROLLBACK_CELLS: usize = 1_000_000;

/// The scrollback for a screen `cols` wide: [`SCROLLBACK_LINES`], or fewer
/// so it holds at most [`SCROLLBACK_CELLS`] cells (5000 lines up to 200
/// columns, 1666 at 600).
pub fn scrollback_len(cols: u16) -> usize {
    (SCROLLBACK_CELLS / usize::from(cols.max(1))).min(SCROLLBACK_LINES)
}

/// Why [`fetch`] returned no stream.
#[derive(Debug, thiserror::Error)]
pub enum LogsError {
    /// The session has no live process: `Couldn't read logs for <id> — job
    /// not found — it may have already exited`.
    #[error("no live output — session stopped; attach to view it")]
    NotRunning,
    /// The id is not known to this account: `No job matching '<id>'. …`.
    /// Usually the session is hosted by another account, or was removed.
    #[error("no such session for this account")]
    UnknownSession,
    /// Any other non-zero exit; `stderr` is the CLI's message, trimmed.
    #[error("logs failed (exit {}): {stderr}", code.map_or("signal".to_owned(), |c| c.to_string()))]
    Failed { code: Option<i32>, stderr: String },
    /// The call could not run or timed out.
    #[error(transparent)]
    Cli(#[from] CliError),
}

/// Runs `<account command> logs <id>` and returns its stdout bytes.
///
/// `account` must be the owning account and `id` the short id (doc/0003,
/// doc/0009). stderr carries the `Session <id> · <name>` banner and errors;
/// it is used only to classify a failure. A non-zero exit is classified by
/// its message: `job not found` → [`LogsError::NotRunning`],
/// `No job matching` → [`LogsError::UnknownSession`], anything else →
/// [`LogsError::Failed`]. Blocks; call it from a worker thread.
pub fn fetch(runner: &CliRunner, account: &Account, id: &str) -> Result<Vec<u8>, LogsError> {
    let out = runner.run(account, ["logs", id])?;
    if out.success() {
        return Ok(out.stdout);
    }
    let stderr = out.stderr.trim();
    if stderr.contains("job not found") {
        Err(LogsError::NotRunning)
    } else if stderr.contains("No job matching") {
        Err(LogsError::UnknownSession)
    } else {
        Err(LogsError::Failed {
            code: out.code,
            stderr: stderr.to_owned(),
        })
    }
}

/// One screen row: its styled spans and its plain text, both without
/// trailing blank cells.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    spans: Vec<Span<'static>>,
    plain: String,
}

impl Row {
    fn is_blank(&self) -> bool {
        self.plain.is_empty()
    }

    fn line(&self) -> Line<'static> {
        Line::from(self.spans.clone())
    }
}

/// The reconstructed screen of a `claude logs` stream, with the lines that
/// scrolled off its top during the replay.
///
/// The screen methods ([`LogScreen::plain_lines`], [`LogScreen::lines`],
/// [`LogScreen::content_range`], [`LogScreen::tail`], …) are
/// screen-aligned: index `i` is screen row `i` (0-based), from the top of
/// the screen through the last row with visible text; blank rows below it
/// are dropped. Trailing spaces are removed from each row (in the styled
/// form only those with no visible background, reverse or underline). The
/// history methods ([`LogScreen::history_lines`], [`LogScreen::page`], …)
/// put the scrollback, oldest line first, above the screen (doc/0028).
#[derive(Debug, Clone, PartialEq)]
pub struct LogScreen {
    size: (u16, u16),
    /// The scrollback, oldest line first, then the screen rows.
    rows: Vec<Row>,
    /// Index of screen row 0 in `rows`: the number of scrollback lines.
    screen_start: usize,
    /// No full clear was replayed: the screen may lack the parts Claude
    /// Code drew only once (doc/0030).
    partial: bool,
}

impl LogScreen {
    /// The emulator size used for the replay, as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// True when the screen has text but no full clear (`ESC[2J` or
    /// `ESC c`) was replayed into it, so the parts of the screen Claude
    /// Code drew only in its last full repaint may be missing (doc/0030).
    pub fn partial(&self) -> bool {
        self.partial && !self.is_empty()
    }

    fn screen(&self) -> &[Row] {
        &self.rows[self.screen_start..]
    }

    /// Number of screen rows from the top through the last non-blank row.
    pub fn row_count(&self) -> usize {
        self.screen().len()
    }

    /// True when neither the screen nor the scrollback has visible text.
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(Row::is_blank)
    }

    /// The rows the panel shows (doc/0009): without blank rows at the top
    /// and bottom. Empty when the screen is empty.
    pub fn content_range(&self) -> Range<usize> {
        let screen = self.screen();
        let start = screen
            .iter()
            .position(|r| !r.is_blank())
            .unwrap_or(screen.len());
        start..screen.len()
    }

    /// Number of rows in [`LogScreen::content_range`].
    pub fn content_rows(&self) -> usize {
        self.content_range().len()
    }

    /// Widest content row in terminal columns.
    pub fn content_width(&self) -> usize {
        self.screen()
            .iter()
            .map(|r| r.plain.width())
            .max()
            .unwrap_or(0)
    }

    /// Screen-aligned plain text, one string per row (see [`LogScreen`]).
    /// Blank interior rows are empty strings.
    pub fn plain_lines(&self) -> Vec<String> {
        self.screen().iter().map(|r| r.plain.clone()).collect()
    }

    /// Screen-aligned styled lines (see [`LogScreen`]), with colours and
    /// bold/dim/italic/underline/reverse. Cells with the terminal's default
    /// colour carry no colour, so the panel's own style shows through. Lines
    /// are as wide as the stream drew them; render them without wrapping so
    /// a narrow panel clips the right-hand side.
    pub fn lines(&self) -> Vec<Line<'static>> {
        self.screen().iter().map(Row::line).collect()
    }

    /// [`LogScreen::lines`] without blank rows at the top and bottom.
    pub fn content_lines(&self) -> Vec<Line<'static>> {
        self.screen()[self.content_range()]
            .iter()
            .map(Row::line)
            .collect()
    }

    /// At most `rows` styled lines of the screen for a panel `rows` high.
    ///
    /// When the content ([`LogScreen::content_lines`]) fits it is returned
    /// unchanged. Otherwise runs of blank rows are first collapsed to a single
    /// blank row (Claude Code leaves a large gap between the conversation and
    /// the prompt box at the bottom), and if it still does not fit only the
    /// last `rows` rows are kept: the newest conversation, the prompt box or
    /// dialog, and the footer. The scrollback is not used; the scrolled
    /// panel uses [`LogScreen::page`].
    pub fn tail(&self, rows: usize) -> Vec<Line<'static>> {
        let screen = self.screen();
        self.tail_indices(rows)
            .into_iter()
            .map(|i| screen[i].line())
            .collect()
    }

    /// Plain-text form of [`LogScreen::tail`], e.g. for a narrow fallback.
    pub fn tail_plain(&self, rows: usize) -> Vec<String> {
        let screen = self.screen();
        self.tail_indices(rows)
            .into_iter()
            .map(|i| screen[i].plain.clone())
            .collect()
    }

    fn tail_indices(&self, rows: usize) -> Vec<usize> {
        let mut picked = fit_indices(self.screen(), self.content_range(), rows);
        let skip = picked.len().saturating_sub(rows);
        picked.split_off(skip)
    }

    /// Number of lines kept above the screen.
    pub fn scrollback_rows(&self) -> usize {
        self.screen_start
    }

    /// The history as indices into `rows`: the scrollback, oldest line
    /// first, then the screen, without blank rows at the top and bottom of
    /// the whole.
    fn history_range(&self) -> Range<usize> {
        let start = self
            .rows
            .iter()
            .position(|r| !r.is_blank())
            .unwrap_or(self.rows.len());
        let end = self
            .rows
            .iter()
            .rposition(|r| !r.is_blank())
            .map_or(start, |p| p + 1);
        start..end
    }

    /// Number of lines in [`LogScreen::history_lines`].
    pub fn history_rows(&self) -> usize {
        self.history_range().len()
    }

    /// The scrollback, oldest line first, then the screen, as plain text,
    /// without blank rows at the top and bottom of the whole.
    pub fn history_plain(&self) -> Vec<String> {
        self.rows[self.history_range()]
            .iter()
            .map(|r| r.plain.clone())
            .collect()
    }

    /// Styled form of [`LogScreen::history_plain`].
    pub fn history_lines(&self) -> Vec<Line<'static>> {
        self.rows[self.history_range()]
            .iter()
            .map(Row::line)
            .collect()
    }

    /// The lines a panel `height` rows high scrolls through, as indices
    /// into `rows`: the history unchanged when it fits, else with runs of
    /// blank rows collapsed to one, as [`LogScreen::tail`] does.
    fn scroll_indices(&self, height: usize) -> Vec<usize> {
        fit_indices(&self.rows, self.history_range(), height)
    }

    /// Number of lines a panel `height` rows high scrolls through: the
    /// history, with runs of blank rows collapsed when it does not fit.
    pub fn scroll_len(&self, height: usize) -> usize {
        self.scroll_indices(height).len()
    }

    /// The largest first line of a page `height` rows high: 0 when
    /// everything fits.
    pub fn max_top(&self, height: usize) -> usize {
        self.scroll_len(height).saturating_sub(height)
    }

    /// The page a panel `height` rows high shows: `height` lines from line
    /// `top` (clamped to [`LogScreen::max_top`]) of the lines it scrolls
    /// through, or the last `height` lines when `top` is `None` (following
    /// the output). Without scrollback the last page is
    /// [`LogScreen::tail`].
    pub fn page(&self, height: usize, top: Option<usize>) -> Vec<Line<'static>> {
        self.page_indices(height, top)
            .into_iter()
            .map(|i| self.rows[i].line())
            .collect()
    }

    /// Plain-text form of [`LogScreen::page`].
    pub fn page_plain(&self, height: usize, top: Option<usize>) -> Vec<String> {
        self.page_indices(height, top)
            .into_iter()
            .map(|i| self.rows[i].plain.clone())
            .collect()
    }

    fn page_indices(&self, height: usize, top: Option<usize>) -> Vec<usize> {
        let all = self.scroll_indices(height);
        let max_top = all.len().saturating_sub(height);
        let top = top.map_or(max_top, |t| t.min(max_top));
        all.into_iter().skip(top).take(height).collect()
    }
}

/// The indices of `range` in `rows` for a panel `height` rows high: all of
/// them when they fit, else with runs of blank rows collapsed to one.
fn fit_indices(rows: &[Row], range: Range<usize>, height: usize) -> Vec<usize> {
    if range.len() <= height {
        return range.collect();
    }
    let mut picked: Vec<usize> = Vec::with_capacity(range.len());
    for i in range {
        let blank = rows[i].is_blank();
        if blank && picked.last().is_some_and(|&p| rows[p].is_blank()) {
            continue;
        }
        picked.push(i);
    }
    picked
}

/// Replays a `claude logs` byte stream at an inferred size and returns the
/// final screen: [`render_sized`] without a known size.
pub fn render(bytes: &[u8]) -> LogScreen {
    render_sized(bytes, None)
}

/// Replays a `claude logs` byte stream and returns the final screen, with
/// the lines that scrolled off its top (see the module docs).
///
/// `real` is the session terminal's size as `(rows, cols)` when it is known
/// (doc/0025); see [`emulator_size`]. Never fails; see the module docs for
/// the handling of huge or malformed input.
pub fn render_sized(bytes: &[u8], real: Option<(u16, u16)>) -> LogScreen {
    let bytes = replay_window(bytes);
    let (rows, cols) = emulator_size(bytes, real);
    let mut parser = new_parser(rows, cols);
    parser.process(&bound_counts(bytes, rows, cols));
    snapshot(&mut parser, !has_clear(bytes))
}

/// An emulator of `rows` × `cols` with the scrollback of doc/0028.
pub(crate) fn new_parser(rows: u16, cols: u16) -> vt100::Parser {
    vt100::Parser::new(rows, cols, scrollback_len(cols))
}

/// Whether `bytes` hold a full clear: `ESC[2J` or a reset (`ESC c`).
pub fn has_clear(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|w| w == b"\x1b[2J") || bytes.windows(2).any(|w| w == b"\x1bc")
}

/// The emulator's screen and the lines that scrolled off its top, as a
/// [`LogScreen`]; `partial` as [`LogScreen::partial`]. Leaves the
/// emulator's view at the bottom.
pub(crate) fn snapshot(parser: &mut vt100::Parser, partial: bool) -> LogScreen {
    let (rows, cols) = parser.screen().size();
    let screen = parser.screen_mut();

    // The scrollback, oldest line first: scrolled back by `kept - first`
    // lines, the screen's visible row `r` is scrollback line `first + r`.
    screen.set_scrollback(usize::MAX);
    let kept = screen.scrollback();
    let mut out: Vec<Row> = Vec::with_capacity(kept + usize::from(rows));
    for first in (0..kept).step_by(usize::from(rows.max(1))) {
        screen.set_scrollback(kept - first);
        let n = (kept - first).min(usize::from(rows)) as u16;
        out.extend((0..n).map(|r| convert_row(screen, r, cols)));
    }
    screen.set_scrollback(0);
    out.extend((0..rows).map(|r| convert_row(screen, r, cols)));
    while out.len() > kept && out.last().is_some_and(Row::is_blank) {
        out.pop();
    }
    LogScreen {
        size: (rows, cols),
        rows: out,
        screen_start: kept,
        partial,
    }
}

/// The part of `bytes` that is replayed (see the module docs).
pub(crate) fn replay_window(bytes: &[u8]) -> &[u8] {
    if bytes.len() <= MAX_INPUT_BYTES {
        return bytes;
    }
    let window = &bytes[bytes.len() - MAX_INPUT_BYTES..];
    let clear = window.windows(4).rposition(|w| w == b"\x1b[2J");
    let reset = window.windows(2).rposition(|w| w == b"\x1bc");
    let last_clear = clear.max(reset);
    let start = last_clear
        .or_else(|| window.iter().position(|&b| b == 0x1b))
        .unwrap_or(0);
    &window[start..]
}

/// The emulator size for the replayed part of a stream, as `(rows, cols)`:
/// the session terminal's real size when it is known and not zero, capped
/// at [`MAX_ROWS`] × [`MAX_COLS`]; otherwise [`infer_size`] (doc/0025).
pub fn emulator_size(bytes: &[u8], real: Option<(u16, u16)>) -> (u16, u16) {
    match real {
        Some((rows, cols)) if rows > 0 && cols > 0 => (rows.min(MAX_ROWS), cols.min(MAX_COLS)),
        _ => infer_size(bytes),
    }
}

/// The inferred emulator size for a stream, as `(rows, cols)`: the larger
/// of the default 64 × 200 and the highest absolutely addressed row and
/// column, capped at [`MAX_ROWS`] × [`MAX_COLS`]. See the module docs for
/// the sequences that count.
pub fn infer_size(bytes: &[u8]) -> (u16, u16) {
    let mut max_row: u32 = u32::from(DEFAULT_ROWS);
    let mut max_col: u32 = u32::from(DEFAULT_COLS);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b || bytes.get(i + 1) != Some(&b'[') {
            i += 1;
            continue;
        }
        let after_cr = i > 0 && bytes[i - 1] == b'\r';
        let Some((csi, len)) = parse_csi(&bytes[i + 2..]) else {
            i += 2;
            continue;
        };
        i += 2 + len;
        if csi.private {
            continue;
        }
        let p = |n: usize| csi.param(n);
        match csi.final_byte {
            b'H' | b'f' => {
                max_row = max_row.max(p(0));
                max_col = max_col.max(p(1));
            }
            b'd' => max_row = max_row.max(p(0)),
            b'G' | b'`' => max_col = max_col.max(p(0)),
            b'r' if csi.params.len() >= 2 => max_row = max_row.max(p(1)),
            b'C' if after_cr => max_col = max_col.max(p(0).saturating_add(1)),
            _ => {}
        }
    }
    let rows = max_row.min(u32::from(MAX_ROWS)) as u16;
    let cols = max_col.min(u32::from(MAX_COLS)) as u16;
    (rows, cols)
}

/// Work `bound_counts` lets the count-proportional sequences (ICH, DCH,
/// ECH, IL, DL, SU, SD, REP) do per replay, in cells touched. A count
/// clamped to the screen still costs up to a row or a screen per sequence,
/// so 2 MiB of nothing but `ESC[600@` would otherwise still keep the logs
/// worker busy for a minute. Claude Code's own streams use none of them.
pub const COUNT_BUDGET: u64 = 20_000_000;

/// Work `bound_counts` lets the screen-wide sequences do per replay, in
/// cells touched: erase in display (ED, DECSED), the alternate screen
/// (`ESC[?1049h`/`l`, `?1047`, `?47`) and reset (`ESC c`, counted eight
/// times, as it rebuilds the screen). Each costs the whole screen, so 2 MiB
/// of `ESC[2J` on a 300 × 600 screen took about two minutes. Claude Code
/// clears once per full redraw, far below this (over a thousand redraws at
/// the largest size).
pub const SCREEN_BUDGET: u64 = 250_000_000;

/// Work `bound_counts` lets line feeds (LF, VT, FF) and reverse index
/// (`ESC M`) do per replay, in cells touched. Each may scroll the screen,
/// which costs a new row and a shift of all rows, and with the scrollback
/// (doc/0028) keeps the old row; one byte per scroll, so 2 MiB of line
/// feeds on a 300 × 600 screen took about two seconds. Each is charged as
/// a scroll (rows + columns), whether it scrolls or not: about 470 000
/// line feeds at 64 × 200, 138 000 at 300 × 600. Claude Code's streams
/// have none: they move the cursor with escape sequences.
pub const SCROLL_BUDGET: u64 = 125_000_000;

/// Whether `b` is a line feed for vt100 (LF, VT or FF).
fn is_line_feed(b: u8) -> bool {
    matches!(b, 0x0a..=0x0c)
}

/// `bytes` with the count and position parameters of CSI sequences clamped
/// to the `rows` × `cols` screen, as [`render`] replays them.
///
/// vt100 0.17 runs ICH `@`, DCH `P`, ECH `X`, IL `L`, DL `M`, SU `S`, SD `T`
/// and REP `b` in time proportional to the count (`ESC[65535@` takes over a
/// second), so a count above what the screen can show is clamped: to the
/// width for ICH, DCH, ECH, CUF `C`, CUB `D`, CHA `G`, HPA `` ` `` and the
/// CUP/HVP `H`/`f` column; to the height for IL, DL, SU, SD, CUU `A`, CUD `B`,
/// CNL `E`, CPL `F`, VPA `d` and the CUP/HVP row; REP to one screenful plus
/// its remainder modulo the width, so the cursor ends up where it would.
/// Every clamped value has the same effect as the original. Once a kind of
/// sequence has used up its budget ([`COUNT_BUDGET`], [`SCREEN_BUDGET`],
/// [`SCROLL_BUDGET`] for line feeds and reverse index), further ones of
/// that kind are dropped. Sequences that need no change are kept byte for
/// byte.
///
/// The scan follows vt100's parser (vte): only the first value of each
/// parameter counts, values saturate at 65535, and control characters
/// inside a sequence are carried out before it, so a rewritten or dropped
/// sequence leaves them in its place (line feeds among them are charged
/// like any other, and dropped together once the budget is spent).
pub(crate) fn bound_counts(bytes: &[u8], rows: u16, cols: u16) -> Cow<'_, [u8]> {
    let mut budget = Budget::new(rows, cols);
    let mut out: Option<Vec<u8>> = None;
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if is_line_feed(bytes[i]) {
            if !budget.line_feeds(1) {
                let buf = out.get_or_insert_with(|| Vec::with_capacity(bytes.len()));
                buf.extend_from_slice(&bytes[copied..i]);
                copied = i + 1;
            }
            i += 1;
            continue;
        }
        if bytes[i] != 0x1b {
            i += 1;
            continue;
        }
        let start = i;
        let Some(seq) = scan_escape(bytes, i + 1) else {
            i += 1;
            continue;
        };
        i = seq.end;
        let feeds = seq.executed.iter().filter(|&&b| is_line_feed(b)).count();
        let keep_feeds = budget.line_feeds(feeds);
        let change = budget.change(&seq);
        if change == Change::Keep && keep_feeds {
            continue;
        }
        let buf = out.get_or_insert_with(|| Vec::with_capacity(bytes.len()));
        buf.extend_from_slice(&bytes[copied..start]);
        let kept = |b: &&u8| keep_feeds || !is_line_feed(**b);
        if change == Change::Keep {
            // Only its line feeds go.
            buf.extend(bytes[start..seq.end].iter().filter(kept));
        } else {
            buf.extend(seq.executed.iter().filter(kept));
        }
        if let Change::Replace(values) = change {
            buf.extend_from_slice(b"\x1b[");
            let text: Vec<String> = values.iter().map(u16::to_string).collect();
            buf.extend_from_slice(text.join(";").as_bytes());
            buf.push(seq.final_byte);
        }
        copied = seq.end;
    }
    match out {
        None => Cow::Borrowed(bytes),
        Some(mut buf) => {
            buf.extend_from_slice(&bytes[copied..]);
            Cow::Owned(buf)
        }
    }
}

/// What [`bound_counts`] does with a sequence.
#[derive(Debug, PartialEq)]
enum Change {
    /// Keep it byte for byte.
    Keep,
    /// Drop it (its budget is spent); its control characters stay.
    Drop,
    /// Replace its parameters with these.
    Replace(Vec<u16>),
}

/// An escape sequence as vte sees it: a CSI sequence without intermediates,
/// a reset (`ESC c`) or a reverse index (`ESC M`).
struct Escape {
    /// The private marker of a CSI sequence (`?` in `ESC[?1049h`).
    private: Option<u8>,
    /// The first value of each parameter, saturated at 65535; 0 if missing.
    params: Vec<u16>,
    /// The final byte; `c` or `M` with no parameters for the two escapes.
    final_byte: u8,
    /// Whether this is `ESC c` or `ESC M` rather than a CSI sequence.
    plain: bool,
    /// Control characters inside the sequence, carried out before it.
    executed: Vec<u8>,
    /// Index just past the final byte.
    end: usize,
}

/// The work left for one replay ([`COUNT_BUDGET`], [`SCREEN_BUDGET`],
/// [`SCROLL_BUDGET`]).
struct Budget {
    rows: u16,
    cols: u16,
    counts: u64,
    screens: u64,
    scrolls: u64,
}

impl Budget {
    fn new(rows: u16, cols: u16) -> Budget {
        Budget {
            rows: rows.max(1),
            cols: cols.max(1),
            counts: COUNT_BUDGET,
            screens: SCREEN_BUDGET,
            scrolls: SCROLL_BUDGET,
        }
    }

    /// The cost of one possible scroll: a new row and a shift of the rows.
    fn scroll_cost(&self) -> u64 {
        u64::from(self.rows) + u64::from(self.cols)
    }

    /// Charges `n` line feeds; false when they do not all fit and must be
    /// dropped.
    fn line_feeds(&mut self, n: usize) -> bool {
        if n == 0 {
            return true;
        }
        let cost = self.scroll_cost().saturating_mul(n as u64);
        Budget::charge(&mut self.scrolls, cost, Change::Keep) == Change::Keep
    }

    /// What to do with `seq`, charging its work.
    fn change(&mut self, seq: &Escape) -> Change {
        let (rows, cols) = (self.rows, self.cols);
        let cells = u64::from(rows) * u64::from(cols);
        let first = seq.params.first().copied().unwrap_or(0);
        if seq.plain {
            return match seq.final_byte {
                b'M' => {
                    let cost = self.scroll_cost();
                    Budget::charge(&mut self.scrolls, cost, Change::Keep)
                }
                _ => Budget::charge(&mut self.screens, 8 * cells, Change::Keep),
            };
        }
        match (seq.private, seq.final_byte) {
            (None | Some(b'?'), b'J') if first <= 2 => {
                return Budget::charge(&mut self.screens, cells, Change::Keep);
            }
            (Some(b'?'), b'h' | b'l')
                if seq.params.iter().any(|p| [47, 1047, 1049].contains(p)) =>
            {
                return Budget::charge(&mut self.screens, cells, Change::Keep);
            }
            (None, _) => {}
            _ => return Change::Keep,
        }
        let count = first.max(1);
        let (bound, weight) = match seq.final_byte {
            b'@' | b'P' => (cols, u64::from(cols)),
            b'X' => (cols, 1),
            b'L' | b'M' | b'S' | b'T' => (rows, u64::from(rows) + u64::from(cols)),
            b'b' => {
                let screen = u32::from(rows) * u32::from(cols);
                let n = u32::from(count);
                let n = if n > screen {
                    screen + (n - screen) % u32::from(cols)
                } else {
                    n
                };
                let n = u16::try_from(n).unwrap_or(count);
                return Budget::charge(&mut self.counts, u64::from(n) * 32, rewrite(first, n));
            }
            b'C' | b'D' | b'G' | b'`' => return clamp(&seq.params, &[cols]),
            b'A' | b'B' | b'E' | b'F' | b'd' => return clamp(&seq.params, &[rows]),
            b'H' | b'f' => return clamp(&seq.params, &[rows, cols]),
            _ => return Change::Keep,
        };
        let n = count.min(bound);
        Budget::charge(&mut self.counts, u64::from(n) * weight, rewrite(first, n))
    }

    /// Takes `cost` from `left`: `change` while it lasts, then
    /// [`Change::Drop`].
    fn charge(left: &mut u64, cost: u64, change: Change) -> Change {
        match left.checked_sub(cost) {
            Some(rest) => {
                *left = rest;
                change
            }
            None => {
                *left = 0;
                Change::Drop
            }
        }
    }
}

/// A count sequence with `first` as its count, clamped to `n`.
fn rewrite(first: u16, n: u16) -> Change {
    if n < first {
        Change::Replace(vec![n])
    } else {
        Change::Keep
    }
}

/// The leading `params` clamped to `bounds`; kept when all fit.
fn clamp(params: &[u16], bounds: &[u16]) -> Change {
    let values: Vec<u16> = (0..bounds.len())
        .map(|n| params.get(n).copied().unwrap_or(0))
        .collect();
    if values.iter().zip(bounds).all(|(v, b)| v <= b) {
        return Change::Keep;
    }
    Change::Replace(values.iter().zip(bounds).map(|(&v, &b)| v.min(b)).collect())
}

/// Follows vte from just after an `ESC` at `bytes[at - 1]`. Returns the
/// reset, reverse index or CSI sequence that starts there, or `None` when
/// it is another escape, has intermediates or a misplaced marker, is cut
/// short, or is cancelled.
fn scan_escape(bytes: &[u8], at: usize) -> Option<Escape> {
    let mut executed = Vec::new();
    let mut i = at;
    // Escape state: controls are carried out, DEL and 8-bit bytes ignored.
    loop {
        let b = *bytes.get(i)?;
        i += 1;
        match b {
            b'[' => break,
            b'c' | b'M' => {
                return Some(Escape {
                    private: None,
                    params: Vec::new(),
                    final_byte: b,
                    plain: true,
                    executed,
                    end: i,
                });
            }
            0x18 | 0x1a | 0x1b => return None,
            0x00..=0x1f => executed.push(b),
            0x7f..=0xff => {}
            _ => return None,
        }
    }
    let mut private = None;
    let mut params: Vec<u16> = Vec::new();
    let mut current: u16 = 0;
    let mut in_first_value = true;
    let mut entry = true;
    loop {
        let b = *bytes.get(i)?;
        i += 1;
        match b {
            0x18 | 0x1a | 0x1b => return None,
            0x00..=0x1f => executed.push(b),
            0x7f..=0xff => {}
            b'0'..=b'9' => {
                if in_first_value {
                    current = current
                        .saturating_mul(10)
                        .saturating_add(u16::from(b - b'0'));
                }
            }
            b':' => in_first_value = false,
            b';' => {
                params.push(current);
                current = 0;
                in_first_value = true;
            }
            // A private marker counts only first; later (or an
            // intermediate) the sequence is none of those handled here.
            0x3c..=0x3f if entry => private = Some(b),
            0x20..=0x2f | 0x3c..=0x3f => return None,
            0x40..=0x7e => {
                params.push(current);
                return Some(Escape {
                    private,
                    params,
                    final_byte: b,
                    plain: false,
                    executed,
                    end: i,
                });
            }
        }
        // Ignored and executed bytes leave vte in its entry state.
        if !matches!(b, 0x00..=0x1f | 0x7f..=0xff) {
            entry = false;
        }
    }
}

/// A parsed CSI sequence (only what [`infer_size`] needs).
struct Csi {
    private: bool,
    params: Vec<Option<u32>>,
    final_byte: u8,
}

impl Csi {
    /// Parameter `n`, with a missing or zero value counting as 1.
    fn param(&self, n: usize) -> u32 {
        match self.params.get(n).copied().flatten() {
            Some(0) | None => 1,
            Some(v) => v,
        }
    }
}

/// Parses the body of a CSI sequence (after `ESC [`). Returns the sequence
/// and its length in bytes, or `None` when it is cut short or malformed.
fn parse_csi(body: &[u8]) -> Option<(Csi, usize)> {
    let mut private = false;
    let mut params: Vec<Option<u32>> = vec![None];
    for (n, &b) in body.iter().enumerate() {
        match b {
            b'0'..=b'9' => {
                let cur = params.last_mut().expect("never empty");
                let v = cur.unwrap_or(0);
                *cur = Some(v.saturating_mul(10).saturating_add(u32::from(b - b'0')));
            }
            b';' => params.push(None),
            // Sub-parameters (`:`) and private markers: not a plain address.
            b':' | b'<'..=b'?' => private = true,
            // Intermediate bytes: an unusual sequence, never an address.
            0x20..=0x2f => private = true,
            0x40..=0x7e => {
                return Some((
                    Csi {
                        private,
                        params,
                        final_byte: b,
                    },
                    n + 1,
                ));
            }
            _ => return None,
        }
    }
    None
}

/// Converts one emulator row into a [`Row`].
fn convert_row(screen: &vt100::Screen, row: u16, cols: u16) -> Row {
    // (text, style, visible) per cell, wide continuations skipped.
    let mut cells: Vec<(String, Style, bool, bool)> = Vec::with_capacity(usize::from(cols));
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let text = if cell.has_contents() {
            cell.contents().to_owned()
        } else {
            " ".to_owned()
        };
        let has_text = text != " ";
        let shows_blank =
            cell.bgcolor() != vt100::Color::Default || cell.inverse() || cell.underline();
        cells.push((text, cell_style(cell), has_text, has_text || shows_blank));
    }

    let plain_end = cells.iter().rposition(|c| c.2).map_or(0, |p| p + 1);
    let plain: String = cells[..plain_end].iter().map(|c| c.0.as_str()).collect();

    let styled_end = cells.iter().rposition(|c| c.3).map_or(0, |p| p + 1);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut style = Style::new();
    for (t, s, _, _) in &cells[..styled_end] {
        if *s != style && !text.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut text), style));
        }
        style = *s;
        text.push_str(t);
    }
    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
    Row { spans, plain }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::new();
    if let Some(c) = color(cell.fgcolor()) {
        style = style.fg(c);
    }
    if let Some(c) = color(cell.bgcolor()) {
        style = style.bg(c);
    }
    let mut m = Modifier::empty();
    if cell.bold() {
        m |= Modifier::BOLD;
    }
    if cell.dim() {
        m |= Modifier::DIM;
    }
    if cell.italic() {
        m |= Modifier::ITALIC;
    }
    if cell.underline() {
        m |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        m |= Modifier::REVERSED;
    }
    if !m.is_empty() {
        style = style.add_modifier(m);
    }
    style
}

fn color(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csi_parsing() {
        assert_eq!(infer_size(b"\x1b[80;250H"), (80, 250));
        assert_eq!(infer_size(b"\x1b[;300H"), (64, 300));
        assert_eq!(infer_size(b"\x1b[?1049h\x1b[?999;999H"), (64, 200));
        assert_eq!(infer_size(b"\x1b[70d\x1b[220G"), (70, 220));
        assert_eq!(infer_size(b"\r\x1b[230C"), (64, 231));
        assert_eq!(infer_size(b"x\x1b[230C"), (64, 200));
        assert_eq!(infer_size(b"\x1b[1;90r"), (90, 200));
        assert_eq!(
            infer_size(b"\x1b[99999999999;99999999999H"),
            (MAX_ROWS, MAX_COLS)
        );
        assert_eq!(infer_size(b"\x1b[80"), (64, 200));
    }

    /// The screen vt100 shows for `bytes` at `rows` × `cols`, as text.
    fn screen_text(bytes: &[u8], rows: u16, cols: u16) -> (String, (u16, u16)) {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(bytes);
        let screen = parser.screen();
        (screen.contents(), screen.cursor_position())
    }

    #[test]
    fn clamped_counts_have_the_same_effect() {
        let (rows, cols) = (24, 80);
        let setup = b"\x1b[2J\x1b[H"
            .iter()
            .copied()
            .chain((0..24).flat_map(|r| {
                format!("\x1b[{};1Hrow {r} {}", r + 1, "abcdefghij".repeat(7)).into_bytes()
            }));
        let setup: Vec<u8> = setup.collect();
        for seq in [
            &b"\x1b[5;10H\x1b[3000@"[..],
            b"\x1b[5;10H\x1b[3000P",
            b"\x1b[5;10H\x1b[3000X",
            b"\x1b[5;10H\x1b[3000L",
            b"\x1b[5;10H\x1b[3000M",
            b"\x1b[3000S",
            b"\x1b[3000T",
            b"\x1b[5;10Hx\x1b[3001b",
            b"\x1b[5;10H\x1b[3000C!\x1b[3000D!\x1b[3000A!\x1b[3000B!",
            b"\x1b[3000G!\x1b[3000d!\x1b[3000;3000H!\x1b[3000;3000f!",
            b"\x1b[5;10H\x1b[3000E!\x1b[3000F!\x1b[3000`!",
            b"\x1b[5;10H\x1b[30\n00:7@",
            b"\x1b[5;10H\x1b\n[3000;1@",
        ] {
            let bytes = [setup.as_slice(), seq].concat();
            let bounded = bound_counts(&bytes, rows, cols);
            assert!(matches!(bounded, Cow::Owned(_)), "{seq:?} rewritten");
            assert_eq!(
                screen_text(&bounded, rows, cols),
                screen_text(&bytes, rows, cols),
                "{:?}",
                String::from_utf8_lossy(seq)
            );
        }
    }

    #[test]
    fn counts_within_the_screen_are_kept_byte_for_byte() {
        for bytes in [
            &b"\x1b[5;10H\x1b[80@\x1b[24L\x1b[?2004h\x1b[999 q\x1b[>999999c"[..],
            b"\x1b[9999m\x1b[9999J\x1b[24;80H",
        ] {
            assert!(matches!(bound_counts(bytes, 24, 80), Cow::Borrowed(_)));
        }
    }

    #[test]
    fn spent_budgets_drop_further_sequences() {
        let (rows, cols) = (MAX_ROWS, MAX_COLS);
        let cells = u64::from(rows) * u64::from(cols);
        let count =
            |bytes: &[u8], seq: &[u8]| bytes.windows(seq.len()).filter(|w| *w == seq).count();

        let clears = b"\x1b[2J\x07".repeat(2000);
        let kept = bound_counts(&clears, rows, cols);
        let allowed = usize::try_from(SCREEN_BUDGET / cells).unwrap();
        assert_eq!(count(&kept, b"\x1b[2J"), allowed);
        // The control characters of dropped sequences stay.
        assert_eq!(count(&kept, b"\x07"), 2000);

        let inserts = b"\x1b[600@".repeat(1000);
        let kept = bound_counts(&inserts, rows, cols);
        let allowed = usize::try_from(COUNT_BUDGET / (600 * 600)).unwrap();
        assert_eq!(count(&kept, b"\x1b[600@"), allowed);
    }

    #[test]
    fn line_feeds_and_reverse_index_share_the_scroll_budget() {
        let (rows, cols) = (MAX_ROWS, MAX_COLS);
        let cost = u64::from(rows) + u64::from(cols);
        let allowed = usize::try_from(SCROLL_BUDGET / cost).unwrap();
        let count = |bytes: &[u8], b: u8| bytes.iter().filter(|&&x| x == b).count();

        let feeds = b"\n\x0b\x0c".repeat(allowed);
        let kept = bound_counts(&feeds, rows, cols);
        assert_eq!(kept.len(), allowed, "only the budget's worth is kept");
        assert_eq!(&kept[..3], b"\n\x0b\x0c");

        let ri = b"\x1bM".repeat(allowed + 10);
        let kept = bound_counts(&ri, rows, cols);
        assert_eq!(kept.len(), allowed * 2);

        // Line feeds inside a sequence count too, and go together once the
        // budget is spent; the sequence itself stays.
        let mut bytes = b"\n".repeat(allowed - 1);
        bytes.extend_from_slice(b"\x1b[\n\n5A\x1b[\n\n5B");
        let kept = bound_counts(&bytes, rows, cols);
        assert_eq!(count(&kept, b'\n'), allowed - 1);
        assert!(
            kept.ends_with(b"\x1b[5A\x1b[5B"),
            "{:?}",
            &kept[kept.len() - 12..]
        );

        // Within the budget nothing changes.
        let text = b"one\r\ntwo\x1bM\r\nthree".to_vec();
        assert!(matches!(bound_counts(&text, 24, 80), Cow::Borrowed(_)));
    }

    #[test]
    fn scrollback_is_capped_by_lines_and_cells() {
        assert_eq!(scrollback_len(80), SCROLLBACK_LINES);
        assert_eq!(scrollback_len(200), 5000);
        assert_eq!(scrollback_len(231), 4329);
        assert_eq!(scrollback_len(600), 1666);
        assert_eq!(scrollback_len(0), SCROLLBACK_LINES);
        for cols in [1, 80, 200, 231, 600] {
            assert!(scrollback_len(cols) * usize::from(cols) <= SCROLLBACK_CELLS.max(cols.into()));
        }
    }

    #[test]
    fn window_starts_at_last_clear() {
        let mut big = vec![b'x'; MAX_INPUT_BYTES];
        big.extend_from_slice(b"\x1b[2J\x1b[Hhello");
        let w = replay_window(&big);
        assert!(w.starts_with(b"\x1b[2J"));
    }
}
