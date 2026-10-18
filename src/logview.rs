//! The logs panel's data: `claude logs <id>` fetched and replayed through a
//! terminal emulator (doc/0009).
//!
//! `claude logs` prints the session's raw terminal byte stream (a clear-screen
//! and full redraw followed by incremental updates with cursor addressing),
//! not text lines (reference/claude-code-cli.md, "`claude logs` output").
//! [`fetch`] runs the call with the owning account's command; [`render`]
//! replays the bytes into a fresh [`vt100::Parser`] and converts the final
//! screen into ratatui [`Line`]s and plain strings. The conversion is done here
//! rather than with `tui-term`, so the panel can trim blank rows, keep only
//! the rows that fit, and fall back to plain text.
//!
//! ## Emulator size
//!
//! The PTY size is not reported, so [`render`] infers it from the stream
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
/// Upper bound for an inferred height.
pub const MAX_ROWS: u16 = 300;
/// Upper bound for an inferred width.
pub const MAX_COLS: u16 = 600;
/// Only this many bytes from the end of the stream are replayed.
pub const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

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

/// The reconstructed screen of a `claude logs` stream.
///
/// Rows are screen-aligned: index `i` is screen row `i` (0-based), from the
/// top of the screen through the last row with visible text; blank rows below
/// it are dropped. Trailing spaces are removed from each row (in the styled
/// form only those with no visible background, reverse or underline).
#[derive(Debug, Clone, PartialEq)]
pub struct LogScreen {
    size: (u16, u16),
    rows: Vec<Row>,
}

impl LogScreen {
    /// The emulator size used for the replay, as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Number of screen rows from the top through the last non-blank row.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// True when the screen has no visible text at all.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows the panel shows (doc/0009): without blank rows at the top
    /// and bottom. Empty when the screen is empty.
    pub fn content_range(&self) -> Range<usize> {
        let start = self
            .rows
            .iter()
            .position(|r| !r.is_blank())
            .unwrap_or(self.rows.len());
        start..self.rows.len()
    }

    /// Number of rows in [`LogScreen::content_range`].
    pub fn content_rows(&self) -> usize {
        self.content_range().len()
    }

    /// Widest content row in terminal columns.
    pub fn content_width(&self) -> usize {
        self.rows.iter().map(|r| r.plain.width()).max().unwrap_or(0)
    }

    /// Screen-aligned plain text, one string per row (see [`LogScreen`]).
    /// Blank interior rows are empty strings.
    pub fn plain_lines(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.plain.clone()).collect()
    }

    /// Screen-aligned styled lines (see [`LogScreen`]), with colours and
    /// bold/dim/italic/underline/reverse. Cells with the terminal's default
    /// colour carry no colour, so the panel's own style shows through. Lines
    /// are as wide as the stream drew them; render them without wrapping so
    /// a narrow panel clips the right-hand side.
    pub fn lines(&self) -> Vec<Line<'static>> {
        self.rows.iter().map(Row::line).collect()
    }

    /// [`LogScreen::lines`] without blank rows at the top and bottom.
    pub fn content_lines(&self) -> Vec<Line<'static>> {
        self.rows[self.content_range()]
            .iter()
            .map(Row::line)
            .collect()
    }

    /// At most `rows` styled lines for a panel `rows` high.
    ///
    /// When the content ([`LogScreen::content_lines`]) fits it is returned
    /// unchanged. Otherwise runs of blank rows are first collapsed to a single
    /// blank row (Claude Code leaves a large gap between the conversation and
    /// the prompt box at the bottom), and if it still does not fit only the
    /// last `rows` rows are kept: the newest conversation, the prompt box or
    /// dialog, and the footer.
    pub fn tail(&self, rows: usize) -> Vec<Line<'static>> {
        self.tail_indices(rows)
            .into_iter()
            .map(|i| self.rows[i].line())
            .collect()
    }

    /// Plain-text form of [`LogScreen::tail`], e.g. for a narrow fallback.
    pub fn tail_plain(&self, rows: usize) -> Vec<String> {
        self.tail_indices(rows)
            .into_iter()
            .map(|i| self.rows[i].plain.clone())
            .collect()
    }

    fn tail_indices(&self, rows: usize) -> Vec<usize> {
        let range = self.content_range();
        if range.len() <= rows {
            return range.collect();
        }
        let mut picked: Vec<usize> = Vec::with_capacity(range.len());
        for i in range {
            let blank = self.rows[i].is_blank();
            if blank && picked.last().is_some_and(|&p| self.rows[p].is_blank()) {
                continue;
            }
            picked.push(i);
        }
        let skip = picked.len().saturating_sub(rows);
        picked.split_off(skip)
    }
}

/// Replays a `claude logs` byte stream and returns the final screen.
///
/// Never fails; see the module docs for the emulator size and the handling
/// of huge or malformed input.
pub fn render(bytes: &[u8]) -> LogScreen {
    let bytes = replay_window(bytes);
    let (rows, cols) = infer_size(bytes);
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(bytes);
    let screen = parser.screen();

    let mut out: Vec<Row> = (0..rows).map(|r| convert_row(screen, r, cols)).collect();
    while out.last().is_some_and(Row::is_blank) {
        out.pop();
    }
    LogScreen {
        size: (rows, cols),
        rows: out,
    }
}

/// The part of `bytes` that is replayed (see the module docs).
fn replay_window(bytes: &[u8]) -> &[u8] {
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

/// The emulator size for a stream, as `(rows, cols)`: the larger of the
/// default 64 × 200 and the highest absolutely addressed row and column,
/// capped at [`MAX_ROWS`] × [`MAX_COLS`]. See the module docs for the
/// sequences that count.
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

    #[test]
    fn window_starts_at_last_clear() {
        let mut big = vec![b'x'; MAX_INPUT_BYTES];
        big.extend_from_slice(b"\x1b[2J\x1b[Hhello");
        let w = replay_window(&big);
        assert!(w.starts_with(b"\x1b[2J"));
    }
}
