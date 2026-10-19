//! A synthetic background session's terminal output, in the shape Claude
//! Code writes it (reference/claude-code-cli.md, "`claude logs` output"):
//! a full repaint (`ESC[2J ESC[H`, every row addressed absolutely) when the
//! session starts and when a terminal attaches, and otherwise differential
//! frames that start with `ESC[H\r`, reach the changed rows with relative
//! moves and park the cursor in the prompt box. `claude logs` returns only
//! the newest writes, dropped whole, which [`Session::capture`] mimics with
//! a ring of [`Session::ring`] writes. Made-up content only.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// A made-up session drawing a `rows` × `cols` screen.
pub struct Session {
    pub rows: u16,
    pub cols: u16,
    /// How many of the newest writes a capture holds.
    pub ring: usize,
    /// The rows' text as an attached terminal shows it (1-based).
    screen: BTreeMap<u16, String>,
    /// Rows changed since the last frame.
    dirty: Vec<u16>,
    writes: Vec<Vec<u8>>,
}

impl Session {
    /// A session that has just started: header, prompt line, prompt box and
    /// footer, drawn by a full repaint.
    pub fn start(rows: u16, cols: u16, ring: usize) -> Session {
        let mut s = Session {
            rows,
            cols,
            ring,
            screen: BTreeMap::new(),
            dirty: Vec::new(),
            writes: Vec::new(),
        };
        s.screen.insert(2, " Example Code v9.9.9".to_owned());
        s.screen
            .insert(3, " example-model · ~/workspace/demo".to_owned());
        s.screen.insert(6, "> run the slow task".to_owned());
        let rule = "-".repeat(usize::from(cols));
        s.screen.insert(rows - 3, rule.clone());
        s.screen.insert(rows - 2, "> ".to_owned());
        s.screen.insert(rows - 1, rule);
        s.screen.insert(rows, "  >> auto mode on".to_owned());
        s.repaint();
        s
    }

    /// A full repaint of the current screen, as on attach.
    pub fn repaint(&mut self) {
        let mut w = b"\x1b7\x1b8\x1b[2J\x1b[H".to_vec();
        for (row, text) in &self.screen {
            w.extend_from_slice(format!("\x1b[{row};1H{text}").as_bytes());
        }
        self.park(&mut w);
        self.dirty.clear();
        self.writes.push(w);
    }

    /// Sets row `row` to `text`; drawn by the next [`Session::frame`].
    pub fn set(&mut self, row: u16, text: &str) {
        self.screen.insert(row, text.to_owned());
        if !self.dirty.contains(&row) {
            self.dirty.push(row);
        }
    }

    /// One differential frame with the rows changed since the last one.
    pub fn frame(&mut self) {
        let mut rows = std::mem::take(&mut self.dirty);
        rows.sort_unstable();
        let mut w = b"\x1b[H\r".to_vec();
        let mut at = 1;
        for row in rows {
            if row > at {
                w.extend_from_slice(format!("\x1b[{}B", row - at).as_bytes());
                at = row;
            }
            w.push(b'\r');
            let text = self.screen.get(&row).cloned().unwrap_or_default();
            w.extend_from_slice(text.as_bytes());
            w.extend_from_slice(b"\x1b[K");
        }
        self.park(&mut w);
        self.writes.push(w);
    }

    /// `n` spinner frames on the spinner row, each a frame of its own.
    pub fn spin(&mut self, n: usize) {
        for _ in 0..n {
            let k = self.writes.len();
            let glyph = ["*", "+", "x", "o"][k % 4];
            self.set(self.rows - 5, &format!("{glyph} Working... ({k}s)"));
            self.frame();
        }
    }

    fn park(&self, w: &mut Vec<u8>) {
        w.extend_from_slice(format!("\x1b[{};1H\x1b[{};3H", self.rows, self.rows - 2).as_bytes());
    }

    /// What `claude logs` would return now: the newest [`Session::ring`]
    /// writes.
    pub fn capture(&self) -> Vec<u8> {
        let from = self.writes.len().saturating_sub(self.ring);
        self.writes[from..].concat()
    }

    /// Every write since the start.
    pub fn all(&self) -> Vec<u8> {
        self.writes.concat()
    }

    /// Number of writes so far.
    pub fn writes(&self) -> usize {
        self.writes.len()
    }

    /// The screen an attached terminal shows, from replaying every write,
    /// one string per row without trailing spaces.
    pub fn reference(&self) -> Vec<String> {
        let mut parser = vt100::Parser::new(self.rows, self.cols, 0);
        parser.process(&self.all());
        parser
            .screen()
            .rows(0, self.cols)
            .map(|r| r.trim_end().to_owned())
            .collect()
    }
}

/// `lines` without the blank rows at the top and bottom.
pub fn trimmed(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |p| p + 1);
    lines[start..end].to_vec()
}
