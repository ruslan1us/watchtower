//! A session's `claude logs` output followed across fetches (doc/0030).
//!
//! `claude logs` returns only the newest part of a session's output: the
//! last few hundred writes, dropped whole, about 40 seconds of a working
//! session (reference/claude-code-cli.md, "`claude logs` output"). Claude
//! Code repaints the whole screen only when the session starts and when a
//! terminal attaches; everything else is a differential frame that redraws
//! the cells that changed. A single capture of a session that has worked
//! for longer than that window therefore holds no full repaint, and
//! replaying it on its own shows only fragments of the screen.
//!
//! A [`LogStream`] keeps one emulator per session and feeds it every
//! capture's *new* bytes, so the screen built from the last full repaint
//! carries over from fetch to fetch:
//!
//! - **Stitching.** The anchor is the last [`ANCHOR_BYTES`] bytes of the
//!   previous capture. Every place it occurs in the new capture is a
//!   candidate; the bytes before it must match the previous capture
//!   backwards all the way to the start of the new capture (or of the kept
//!   previous capture), since a capture always begins at a write
//!   boundary. Among those the one that ends latest wins, and only the
//!   bytes after it are fed. A capture identical to the previous one feeds
//!   nothing.
//! - **Reset.** No overlap (the fetches were too far apart, or the session
//!   started over), a different real terminal size (doc/0025) or another
//!   process id replays the new capture into a fresh emulator, as
//!   [`logview::render_sized`] does.
//! - **Base.** [`LogStream::has_base`] says whether a full clear (`ESC[2J`
//!   or `ESC c`) was fed since the last reset. Without one the screen may
//!   lack what Claude Code drew only in its last full repaint, and
//!   [`LogScreen::partial`] says so.
//!
//! Every feed is bounded like a replay (doc/0009, doc/0028): its counts are
//! clamped and its costly sequences share fresh work budgets, and the
//! emulator keeps the bounded scrollback of doc/0028.

use crate::logview::{self, LogScreen};

/// Length of the anchor taken from the end of the previous capture.
pub const ANCHOR_BYTES: usize = 256;

/// Most bytes of the previous capture kept for stitching. Claude Code's
/// captures are about 35 KB; longer ones keep their end.
pub const TAIL_BYTES: usize = 256 * 1024;

/// How a capture was taken in ([`LogStream::feed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// The capture overlapped the previous one; this many new bytes were
    /// fed (0 when nothing was new).
    Stitched(usize),
    /// The capture was replayed into a fresh emulator.
    Reset(ResetCause),
}

/// Why a [`LogStream`] started over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetCause {
    /// No overlap with the previous capture was found.
    NoOverlap,
    /// The session terminal's real size changed (doc/0025).
    Size,
    /// The session runs under another process id.
    Pid,
}

/// One session's output, followed across `claude logs` captures (see the
/// module docs).
pub struct LogStream {
    parser: vt100::Parser,
    /// The previous capture, at most [`TAIL_BYTES`] of its end.
    tail: Vec<u8>,
    /// Whether a full clear was fed since the last reset.
    has_base: bool,
    /// The process the captures came from, when known.
    pid: Option<u32>,
}

impl std::fmt::Debug for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogStream")
            .field("size", &self.size())
            .field("tail", &self.tail.len())
            .field("has_base", &self.has_base)
            .field("pid", &self.pid)
            .finish()
    }
}

impl LogStream {
    /// A stream that starts with `capture`, replayed as
    /// [`logview::render_sized`] does: at the real size `real` when known,
    /// else at the size inferred from the capture. `pid` is the session's
    /// process, when known.
    pub fn new(capture: Vec<u8>, real: Option<(u16, u16)>, pid: Option<u32>) -> LogStream {
        let window = logview::replay_window(&capture);
        let (rows, cols) = logview::emulator_size(window, real);
        let mut parser = logview::new_parser(rows, cols);
        parser.process(&logview::bound_counts(window, rows, cols));
        let has_base = logview::has_clear(window);
        LogStream {
            parser,
            tail: keep_tail(capture),
            has_base,
            pid,
        }
    }

    /// Takes in the next capture of the same session (see the module
    /// docs): feeds only its new bytes, or starts over when it does not
    /// overlap the previous capture, `real` is known and differs from the
    /// emulator's size, or `pid` is known and differs from the stream's.
    pub fn feed(&mut self, capture: Vec<u8>, real: Option<(u16, u16)>, pid: Option<u32>) -> Fed {
        let cause = if pid.is_some() && self.pid.is_some() && pid != self.pid {
            Some(ResetCause::Pid)
        } else if real.is_some_and(|r| logview::emulator_size(&[], Some(r)) != self.size()) {
            Some(ResetCause::Size)
        } else {
            None
        };
        let start = match cause {
            None => overlap_end(&self.tail, &capture),
            Some(_) => None,
        };
        let Some(start) = start else {
            *self = LogStream::new(capture, real, pid.or(self.pid));
            return Fed::Reset(cause.unwrap_or(ResetCause::NoOverlap));
        };
        let new = &capture[start..];
        if !new.is_empty() {
            let (rows, cols) = self.size();
            self.parser.process(&logview::bound_counts(new, rows, cols));
            self.has_base |= logview::has_clear(new);
        }
        let fed = new.len();
        if pid.is_some() {
            self.pid = pid;
        }
        self.tail = keep_tail(capture);
        Fed::Stitched(fed)
    }

    /// Whether a full clear was fed since the last reset: the screen holds
    /// everything an attached terminal would show.
    pub fn has_base(&self) -> bool {
        self.has_base
    }

    /// The emulator size, as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    /// The current screen and the lines kept above it, for the panel. Takes
    /// `&mut` only to read the scrollback; the stream is unchanged.
    pub fn screen(&mut self) -> LogScreen {
        logview::snapshot(&mut self.parser, !self.has_base)
    }
}

/// The end of `capture` limited to [`TAIL_BYTES`], without copying when it
/// fits.
fn keep_tail(mut capture: Vec<u8>) -> Vec<u8> {
    if capture.len() > TAIL_BYTES {
        capture.drain(..capture.len() - TAIL_BYTES);
    }
    capture
}

/// Where the overlap of `prev` (the previous capture's kept end) with
/// `next` ends in `next`: the bytes of `next` from there on are new.
/// `None` when they do not overlap, or when `prev` is empty.
///
/// The anchor is the last [`ANCHOR_BYTES`] of `prev`. An occurrence in
/// `next` counts only when the bytes before it equal the bytes before the
/// anchor in `prev` back to the start of `next` or of `prev`; the latest
/// such occurrence wins. At most [`MAX_CANDIDATES`] occurrences are
/// checked, latest first, so a capture of one repeated pattern cannot make
/// the search slow; when none of them counts there is no overlap.
pub fn overlap_end(prev: &[u8], next: &[u8]) -> Option<usize> {
    if prev.is_empty() {
        return None;
    }
    let len = prev.len().min(ANCHOR_BYTES);
    let anchor = &prev[prev.len() - len..];
    let before = &prev[..prev.len() - len];
    let mut end = next.len();
    for _ in 0..MAX_CANDIDATES {
        let at = rfind(&next[..end], anchor)?;
        let reach = common_suffix(&next[..at], before);
        if reach == at || reach == before.len() {
            return Some(at + len);
        }
        // The next candidate ends before this one does.
        end = at + len - 1;
    }
    None
}

/// Most anchor occurrences [`overlap_end`] checks.
pub const MAX_CANDIDATES: usize = 64;

/// The last position of `needle` in `hay`; `None` for an empty needle.
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = *needle.last()?;
    let mut end = hay.len();
    while end >= needle.len() {
        // The last byte of a match sits at `p`.
        let p = hay[needle.len() - 1..end]
            .iter()
            .rposition(|&b| b == last)?
            + needle.len()
            - 1;
        let at = p + 1 - needle.len();
        if &hay[at..=p] == needle {
            return Some(at);
        }
        end = p;
    }
    None
}

/// The length of the longest common suffix of `a` and `b`.
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfind_and_suffix() {
        assert_eq!(rfind(b"abcabc", b"ca"), Some(2));
        assert_eq!(rfind(b"abcabc", b"abc"), Some(3));
        assert_eq!(rfind(b"abc", b"abcd"), None);
        assert_eq!(rfind(b"abc", b""), None);
        assert_eq!(rfind(b"aab", b"ab"), Some(1));
        assert_eq!(rfind(b"abab", b"ba"), Some(1));
        assert_eq!(rfind(b"aaaa", b"aa"), Some(2));
        assert_eq!(common_suffix(b"xxabc", b"yabc"), 3);
        assert_eq!(common_suffix(b"", b"abc"), 0);
    }

    #[test]
    fn overlap_needs_a_match_back_to_a_start() {
        let prev: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        // `next` starts inside `prev` and goes on.
        let mut next = prev[300..].to_vec();
        next.extend_from_slice(b"new bytes");
        assert_eq!(overlap_end(&prev, &next), Some(700));
        // Identical: nothing new.
        assert_eq!(overlap_end(&prev, &prev), Some(prev.len()));
        // The anchor alone, with other bytes before it: no overlap.
        let mut fake = b"other".to_vec();
        fake.extend_from_slice(&prev[prev.len() - ANCHOR_BYTES..]);
        fake.extend_from_slice(b"tail");
        assert_eq!(overlap_end(&prev, &fake), None);
        // Nothing in common.
        assert_eq!(overlap_end(&prev, b"unrelated"), None);
        assert_eq!(overlap_end(b"", b"x"), None);
    }

    #[test]
    fn a_repeated_pattern_overlaps_at_its_latest_place() {
        let prev = b"ab".repeat(2000);
        let mut next = b"ab".repeat(1500);
        next.extend_from_slice(b"new");
        // Periodic: the latest occurrence reaches the start of `next`.
        assert_eq!(overlap_end(&prev, &next), Some(3000));
        // One byte almost everywhere, but the bytes before the anchor
        // never match back to a start: the search gives up after a bounded
        // number of candidates.
        let prev = [b"z".as_slice(), &b"x".repeat(10_000)].concat();
        let next = [b"y".as_slice(), &b"x".repeat(100_000)].concat();
        assert_eq!(overlap_end(&prev, &next), None);
    }
}
