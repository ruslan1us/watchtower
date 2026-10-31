//! Claude's real terminal inside tower, the `workspace` style (doc/0045):
//! one `<account command> attach <id>` at a time in a pseudo-terminal, and
//! beside it at most one shell, each with a `vt100` emulator for its output
//! that the renderer copies into the pane. Only ratatui writes to the real terminal, so nothing the child
//! prints reaches it raw: BEL, OSC notifications and title changes end in the
//! emulator (doc/0007). The one exception is an OSC 52 clipboard write, which
//! the runtime writes again itself; a clipboard read is never passed on. A
//! program that does not ask for the mouse gets tower's own text selection and,
//! in the shell, a bounded history to scroll, both read from the emulator.
//!
//! A worker thread owns the child: it starts it, reads the PTY into the
//! emulator, answers the terminal queries the emulator gets, writes what is
//! queued for the PTY without blocking, and reaps the child. So the UI thread
//! never blocks on the child, on a full PTY or on a hung folder (doc/0001).
//! It wakes the event loop through a self-pipe ([`Embed::wake_fd`]), created
//! on the first client, so the other styles have no extra descriptor at all.
//!
//! Closing a client is `SIGHUP` to its process group and closing the PTY; the
//! client ends in about 0.05 s and the session keeps running
//! (reference/claude-code-cli.md). A client that ignores it gets `SIGKILL`
//! after [`CLOSE_WAIT`].

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

use crate::cli::CommandLine;

/// How long a closed client gets to end before `SIGKILL`.
pub const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// How long a `?2026` synchronized update may hold the pane's redraw, for a
/// child that never ends it.
pub const SYNC_HOLD: Duration = Duration::from_millis(200);

/// The largest OSC 52 write passed on (base64, about 3 MiB of text); a
/// larger one is dropped, as terminals limit it too.
pub const CLIPBOARD_MAX: usize = 4 << 20;

/// The lines of history a shell keeps above its screen (doc/0045): at 32 bytes
/// a cell and 130 columns about 42 MB when full, and there is one shell at a
/// time. The Claude client keeps none: it scrolls itself.
pub const SHELL_HISTORY: usize = 10_000;

/// The most bytes queued for the PTY; more (a huge paste into a child that
/// reads nothing) are dropped.
const QUEUE_MAX: usize = 16 << 20;

/// The most query answers kept for one read of the PTY.
const REPLIES_MAX: usize = 4096;

/// What the emulator does beside drawing: the answers to terminal queries,
/// clipboard writes, and the two modes `vt100` does not keep.
#[derive(Default)]
struct Callbacks {
    replies: Vec<u8>,
    clipboard: Vec<Vec<u8>>,
    focus_events: bool,
    sync_since: Option<Instant>,
}

impl Callbacks {
    fn reply(&mut self, bytes: &[u8]) {
        if self.replies.len() + bytes.len() <= REPLIES_MAX {
            self.replies.extend_from_slice(bytes);
        }
    }
}

impl vt100::Callbacks for Callbacks {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        // vt100 has checked the selector and that the data is base64, and
        // sends a read (`?`) to `paste_from_clipboard`, which does nothing.
        if data.len() > CLIPBOARD_MAX {
            return;
        }
        let mut seq = Vec::with_capacity(ty.len() + data.len() + 8);
        seq.extend_from_slice(b"\x1b]52;");
        seq.extend_from_slice(ty);
        seq.push(b';');
        seq.extend_from_slice(data);
        seq.extend_from_slice(b"\x1b\\");
        self.clipboard.push(seq);
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, i2, c) {
            (None, None, 'c') if first == 0 => self.reply(b"\x1b[?62;22c"),
            (Some(b'>'), None, 'q') => {
                let answer = format!("\x1bP>|tower {}\x1b\\", env!("CARGO_PKG_VERSION"));
                self.reply(answer.as_bytes());
            }
            (None, None, 'n') if first == 5 => self.reply(b"\x1b[0n"),
            (None, None, 'n') if first == 6 => {
                let (row, col) = screen.cursor_position();
                let answer = format!("\x1b[{};{}R", row + 1, col + 1);
                self.reply(answer.as_bytes());
            }
            (Some(b'?'), None, 'h' | 'l') => {
                let on = c == 'h';
                for param in params {
                    match param.first() {
                        Some(1004) => self.focus_events = on,
                        Some(2026) => self.sync_since = on.then(Instant::now),
                        _ => {}
                    }
                }
            }
            (Some(b'?'), Some(b'$'), 'p') if first == 2026 => self.reply(b"\x1b[?2026;2$y"),
            _ => {}
        }
    }
}

/// A cell of the pane, row and column from 0.
pub type PaneCell = (u16, u16);

/// What a selection covers on the screen as it is now (doc/0045).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selected {
    /// The text to copy: a wrapped line whole, no spaces at the end of a line.
    pub text: String,
    /// The cells to highlight: row, first and last column of each row. What
    /// shows is what is copied, so a row's blank end has none.
    pub spans: Vec<(u16, u16, u16)>,
}

/// The cells between `a` and `b` in reading order, both included, as the
/// screen shows them now, its history included.
fn read_selection(s: &vt100::Screen, a: PaneCell, b: PaneCell) -> Selected {
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    let (_, cols) = s.size();
    let last_col = cols.saturating_sub(1);
    let mut out = Selected::default();
    for r in start.0..=end.0 {
        // A row of the history may be narrower or wider than the screen now:
        // a cell it does not have is blank, and what is past the pane is not
        // shown, so not copied.
        let mut from = if r == start.0 {
            start.1.min(last_col)
        } else {
            0
        };
        if from > 0
            && s.cell(r, from)
                .is_some_and(vt100::Cell::is_wide_continuation)
        {
            from -= 1;
        }
        let mut to = if r == end.0 {
            end.1.min(last_col)
        } else {
            last_col
        };
        if to < last_col && s.cell(r, to).is_some_and(vt100::Cell::is_wide) {
            to += 1;
        }
        // Inside a wrapped line a space at the row's end is text; the empty
        // cell a wide character left when it did not fit is not. The emulator
        // does not mark that wrap: only one cell empty and a wide character
        // next says it.
        let wide_went_on = last_col > 0
            && s.cell(r, last_col).is_some_and(|c| !c.has_contents())
            && s.cell(r, last_col - 1)
                .is_some_and(vt100::Cell::has_contents)
            && s.cell(r + 1, 0).is_some_and(vt100::Cell::is_wide);
        let wraps = r < end.0 && (s.row_wrapped(r) || wide_went_on);
        let blank = |col: u16| {
            s.cell(r, col).is_none_or(|cell| {
                !cell.is_wide_continuation()
                    && (!cell.has_contents() || (!wraps && cell.contents() == " "))
            })
        };
        while to > from && blank(to) {
            to -= 1;
        }
        if to > from || !blank(to) {
            out.spans.push((r, from, to));
            for col in from..=to {
                match s.cell(r, col) {
                    Some(cell) if cell.is_wide_continuation() => {}
                    Some(cell) if cell.has_contents() => out.text.push_str(cell.contents()),
                    _ => out.text.push(' '),
                }
            }
        }
        if !wraps {
            out.text.truncate(out.text.trim_end_matches(' ').len());
            if r < end.0 {
                out.text.push('\n');
            }
        }
    }
    out.text.truncate(out.text.trim_end_matches('\n').len());
    out
}

/// Whether a cell belongs to a word for a double click: anything but a blank,
/// quotes and brackets, so a path or an address is one word.
fn in_word(cell: &vt100::Cell) -> bool {
    cell.is_wide_continuation()
        || cell.contents().chars().next().is_some_and(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '"' | '\''
                        | '`'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '<'
                        | '>'
                        | '|'
                        | ','
                        | ';'
                )
        })
}

/// The whole `OSC 52` write of `text` to the clipboard, for the real terminal.
/// `None` when it is longer than [`CLIPBOARD_MAX`], which terminals limit too.
pub fn clipboard_write(text: &str) -> Option<Vec<u8>> {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if text.len() / 3 * 4 >= CLIPBOARD_MAX {
        return None;
    }
    let mut seq = b"\x1b]52;c;".to_vec();
    for chunk in text.as_bytes().chunks(3) {
        let at = |k: usize| u32::from(chunk.get(k).copied().unwrap_or(0));
        let bits = at(0) << 16 | at(1) << 8 | at(2);
        for k in 0..4 {
            seq.push(if k <= chunk.len() {
                DIGITS[(bits >> (18 - 6 * k) & 63) as usize]
            } else {
                b'='
            });
        }
    }
    seq.extend_from_slice(b"\x1b\\");
    Some(seq)
}

/// The child's screen: the emulator the worker thread feeds and the renderer
/// reads. Tests build one without a child and feed it bytes.
#[derive(Clone)]
pub struct PaneScreen {
    parser: Arc<Mutex<vt100::Parser<Callbacks>>>,
}

impl PaneScreen {
    /// An empty screen. A size of 0 would make `vt100` panic, so it is at
    /// least 1 × 1.
    pub fn new(rows: u16, cols: u16) -> PaneScreen {
        PaneScreen::with_history(rows, cols, 0)
    }

    /// [`PaneScreen::new`] keeping up to `lines` lines that leave the top.
    pub fn with_history(rows: u16, cols: u16, lines: usize) -> PaneScreen {
        PaneScreen {
            parser: Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
                rows.max(1),
                cols.max(1),
                lines,
                Callbacks::default(),
            ))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, vt100::Parser<Callbacks>> {
        self.parser.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs what the child wrote through the emulator.
    pub fn feed(&self, bytes: &[u8]) {
        self.lock().process(bytes);
    }

    /// Reads the screen.
    pub fn with<R>(&self, read: impl FnOnce(&vt100::Screen) -> R) -> R {
        read(self.lock().screen())
    }

    /// Resizes the emulator.
    pub fn set_size(&self, rows: u16, cols: u16) {
        self.lock().screen_mut().set_size(rows.max(1), cols.max(1));
    }

    /// Whether arrow keys use the application form (`ESC O A`).
    pub fn application_cursor(&self) -> bool {
        self.with(vt100::Screen::application_cursor)
    }

    /// Whether the child asked for bracketed paste.
    pub fn bracketed_paste(&self) -> bool {
        self.with(vt100::Screen::bracketed_paste)
    }

    /// The child's mouse mode.
    pub fn mouse_mode(&self) -> vt100::MouseProtocolMode {
        self.with(vt100::Screen::mouse_protocol_mode)
    }

    /// Whether the child asked for focus events (`?1004`).
    pub fn focus_events(&self) -> bool {
        self.lock().callbacks().focus_events
    }

    /// Whether tower scrolls this screen itself: it keeps a history, and the
    /// child is not on the alternate screen and did not ask for the mouse
    /// (doc/0045).
    pub fn scrollable(&self) -> bool {
        self.with(|s| {
            s.scrollback_len() > 0
                && !s.alternate_screen()
                && s.mouse_protocol_mode() == vt100::MouseProtocolMode::None
        })
    }

    /// How many lines the view is above the bottom. Output moves this too, so
    /// the view stays on the same lines.
    pub fn scrolled(&self) -> usize {
        self.with(vt100::Screen::scrollback)
    }

    /// How many lines the history holds now.
    pub fn history(&self) -> usize {
        let mut parser = self.lock();
        let screen = parser.screen_mut();
        let at = screen.scrollback();
        // `scrollback_len` is the limit, not the count; this clamps to it.
        screen.set_scrollback(usize::MAX);
        let lines = screen.scrollback();
        screen.set_scrollback(at);
        lines
    }

    /// Moves the view by `lines`, up when positive. Whether it moved.
    pub fn scroll(&self, lines: isize) -> bool {
        let mut parser = self.lock();
        let screen = parser.screen_mut();
        let at = screen.scrollback();
        screen.set_scrollback(at.saturating_add_signed(lines));
        screen.scrollback() != at
    }

    /// Puts the view back at the bottom. Whether it moved.
    pub fn to_bottom(&self) -> bool {
        let mut parser = self.lock();
        let screen = parser.screen_mut();
        let at = screen.scrollback();
        screen.set_scrollback(0);
        at != 0
    }

    /// What the cells between `a` and `b` show now.
    pub fn selected(&self, a: PaneCell, b: PaneCell) -> Selected {
        self.with(|s| read_selection(s, a, b))
    }

    /// The first and last cell of the word at `at` on its row, if there is one.
    pub fn word_at(&self, at: PaneCell) -> Option<(PaneCell, PaneCell)> {
        self.with(|s| {
            let (row, col) = at;
            let is_word = |c: u16| s.cell(row, c).is_some_and(in_word);
            if !is_word(col) {
                return None;
            }
            let mut first = col;
            while first > 0 && is_word(first - 1) {
                first -= 1;
            }
            let mut last = col;
            while is_word(last + 1) {
                last += 1;
            }
            Some(((row, first), (row, last)))
        })
    }

    /// Whether a `?2026` update is still being written, for at most
    /// [`SYNC_HOLD`].
    pub fn syncing(&self, now: Instant) -> bool {
        self.lock()
            .callbacks()
            .sync_since
            .is_some_and(|at| now.saturating_duration_since(at) < SYNC_HOLD)
    }

    /// The answers to terminal queries since the last call.
    pub fn take_replies(&self) -> Vec<u8> {
        std::mem::take(&mut self.lock().callbacks_mut().replies)
    }

    /// The clipboard writes since the last call, each a whole `OSC 52`
    /// sequence to write to the real terminal.
    pub fn take_clipboard(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.lock().callbacks_mut().clipboard)
    }
}

/// The bytes a terminal sends for `key`, given the child's cursor key mode.
/// Nothing for a key without an encoding.
pub fn encode_key(key: &KeyEvent, application_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let param = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |body: String| format!("\x1b[{body}").into_bytes();
    let meta = |rest: &[u8]| {
        let mut out = Vec::with_capacity(rest.len() + 1);
        if alt {
            out.push(0x1b);
        }
        out.extend_from_slice(rest);
        out
    };
    // `ESC [ A`, `ESC O A` or, with modifiers, `ESC [ 1 ; m A`.
    let letter = |c: char, application: bool| {
        if param > 1 {
            csi(format!("1;{param}{c}"))
        } else if application {
            format!("\x1bO{c}").into_bytes()
        } else {
            csi(c.to_string())
        }
    };
    // `ESC [ n ~` or, with modifiers, `ESC [ n ; m ~`.
    let tilde = |n: u8| {
        if param > 1 {
            csi(format!("{n};{param}~"))
        } else {
            csi(format!("{n}~"))
        }
    };
    match key.code {
        KeyCode::Char(c) if ctrl => match control_byte(c) {
            Some(b) => meta(&[b]),
            None => Vec::new(),
        },
        KeyCode::Char(c) => {
            let mut utf8 = [0; 4];
            meta(c.encode_utf8(&mut utf8).as_bytes())
        }
        KeyCode::Enter if shift || ctrl => csi(format!("13;{param}u")),
        KeyCode::Enter => meta(b"\r"),
        KeyCode::Tab if shift || ctrl => csi(format!("9;{param}u")),
        KeyCode::Tab => meta(b"\t"),
        KeyCode::BackTab => meta(b"\x1b[Z"),
        KeyCode::Esc => meta(b"\x1b"),
        KeyCode::Backspace if ctrl => meta(&[0x08]),
        KeyCode::Backspace => meta(&[0x7f]),
        KeyCode::Up => letter('A', application_cursor),
        KeyCode::Down => letter('B', application_cursor),
        KeyCode::Right => letter('C', application_cursor),
        KeyCode::Left => letter('D', application_cursor),
        KeyCode::Home => letter('H', application_cursor),
        KeyCode::End => letter('F', application_cursor),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => letter(char::from(b'P' + n - 1), true),
        KeyCode::F(n @ 5) => tilde(10 + n),
        KeyCode::F(n @ 6..=10) => tilde(11 + n),
        KeyCode::F(n @ 11..=12) => tilde(12 + n),
        KeyCode::Null => vec![0],
        _ => Vec::new(),
    }
}

/// The control byte of `Ctrl+c`, as terminals send it.
fn control_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 & 0x1f),
        ' ' | '2' | '@' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' | '-' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// `text` as the child should receive it: in brackets when it asked for them,
/// without anything that could end the brackets early; otherwise with line
/// feeds as carriage returns, as a terminal sends typed text.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let clean = text.replace("\x1b[201~", "");
        let mut out = Vec::with_capacity(clean.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// An SGR mouse report for `kind` at the pane's cell `x`, `y` (from 1).
/// Nothing for an event SGR has no code for.
pub fn encode_mouse(
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    x: u16,
    y: u16,
) -> Option<Vec<u8>> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, last) = match kind {
        MouseEventKind::Down(b) => (button(b), 'M'),
        MouseEventKind::Up(b) => (button(b), 'm'),
        MouseEventKind::Drag(b) => (button(b) + 32, 'M'),
        MouseEventKind::Moved => (35, 'M'),
        MouseEventKind::ScrollUp => (64, 'M'),
        MouseEventKind::ScrollDown => (65, 'M'),
        MouseEventKind::ScrollLeft => (66, 'M'),
        MouseEventKind::ScrollRight => (67, 'M'),
    };
    let held = 4 * u8::from(modifiers.contains(KeyModifiers::SHIFT))
        + 8 * u8::from(modifiers.contains(KeyModifiers::ALT))
        + 16 * u8::from(modifiers.contains(KeyModifiers::CONTROL));
    Some(format!("\x1b[<{};{x};{y}{last}", code + held).into_bytes())
}

/// Terminal-identifying variables the child must not see: it would answer
/// them with protocols the emulator does not speak (`TERM_PROGRAM=ghostty`
/// makes `claude attach` push kitty keys).
pub fn hides_from_child(name: &str) -> bool {
    const NAMES: [&str; 10] = [
        "COLUMNS",
        "LINES",
        "STY",
        "WINDOW",
        "TERMINAL_EMULATOR",
        "VTE_VERSION",
        "LC_TERMINAL",
        "LC_TERMINAL_VERSION",
        "TERMCAP",
        "TERMINFO",
    ];
    const PREFIXES: [&str; 10] = [
        "TERM_PROGRAM",
        "TMUX",
        "WT_",
        "KITTY_",
        "WEZTERM_",
        "ALACRITTY_",
        "GHOSTTY_",
        "KONSOLE_",
        "ITERM_",
        "WARP_",
    ];
    NAMES.contains(&name) || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// A new master and slave, both close-on-exec from their first moment, so a
/// process another thread forks meanwhile cannot keep them open (a kept slave
/// would hide the client's end, a kept master its hang-up). The master does
/// not block.
fn open_pty(rows: u16, cols: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    // SAFETY: posix_openpt returns a new descriptor or -1.
    let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened and is owned here.
    let master = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: plain calls on our master; `name` is a valid buffer of the
    // length passed, which ptsname_r NUL-terminates on success.
    let path_ok = unsafe {
        libc::grantpt(fd) == 0
            && libc::unlockpt(fd) == 0
            && libc::ptsname_r(fd, name.as_mut_ptr(), name.len()) == 0
    };
    if !path_ok {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `name` is the NUL-terminated path ptsname_r wrote.
    let slave_fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `slave_fd` was just opened and is owned here.
    let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };
    set_nonblocking(fd)?;
    set_winsize(fd, rows, cols)?;
    Ok((master, slave))
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: a valid winsize on an open PTY descriptor.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A pipe whose ends are close-on-exec and do not block.
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` is a valid out-array of two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both were just opened and are owned here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn poke(fd: &OwnedFd) {
    // SAFETY: one byte from a valid buffer to a descriptor we own. A full
    // pipe (EAGAIN) already says what the byte would.
    unsafe { libc::write(fd.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
}

fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 64];
    // SAFETY: reads into a valid buffer until the non-blocking pipe is empty.
    while unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
}

/// The self-pipe that wakes the event loop's `poll(2)`. At most one byte
/// waits in it.
struct Waker {
    read: OwnedFd,
    write: OwnedFd,
    armed: AtomicBool,
}

impl Waker {
    fn new() -> io::Result<Waker> {
        let (read, write) = pipe()?;
        Ok(Waker {
            read,
            write,
            armed: AtomicBool::new(false),
        })
    }

    fn wake(&self) {
        if !self.armed.swap(true, Ordering::SeqCst) {
            poke(&self.write);
        }
    }

    fn drain(&self) {
        self.armed.store(false, Ordering::SeqCst);
        drain(&self.read);
    }
}

/// How a client ended without being closed by tower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// The child could not be started.
    Failed { generation: u64, error: String },
    /// The child exited with this code, `None` for a signal.
    Exited { generation: u64, code: Option<i32> },
    /// A shell's folder no longer exists, so nothing started (doc/0037).
    FolderGone { generation: u64 },
}

/// What the UI thread and a client's worker share.
struct Shared {
    generation: u64,
    screen: PaneScreen,
    waker: Arc<Waker>,
    close: AtomicBool,
    done: AtomicBool,
    queue: Mutex<VecDeque<u8>>,
    event: Mutex<Option<ClientEvent>>,
    clipboard: Mutex<Vec<Vec<u8>>>,
    control_read: OwnedFd,
    control_write: OwnedFd,
}

impl Shared {
    fn report(&self, event: ClientEvent) {
        *self.event.lock().unwrap_or_else(|e| e.into_inner()) = Some(event);
        self.waker.wake();
    }

    fn queue(&self) -> MutexGuard<'_, VecDeque<u8>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Reports an end when the worker stops without one (a panic), so no pane
/// waits for a client that is gone; and marks the worker finished.
struct WorkerDone(Arc<Shared>);

impl Drop for WorkerDone {
    fn drop(&mut self) {
        let mut event = self.0.event.lock().unwrap_or_else(|e| e.into_inner());
        if !self.0.close.load(Ordering::SeqCst) && event.is_none() && thread::panicking() {
            *event = Some(ClientEvent::Exited {
                generation: self.0.generation,
                code: None,
            });
        }
        drop(event);
        self.0.done.store(true, Ordering::SeqCst);
        self.0.waker.wake();
    }
}

/// A client tower holds: what the UI thread needs of it.
struct Client {
    shared: Arc<Shared>,
    master: Arc<OwnedFd>,
}

/// The PTY children of the workspace style: one Claude client, and one shell,
/// which lives beside it (doc/0045).
#[derive(Default)]
pub struct Embed {
    waker: Option<Arc<Waker>>,
    current: Option<Client>,
    shell: Option<Client>,
    /// Closed clients whose workers are still reaping them.
    closing: Vec<Arc<Shared>>,
}

/// What to run in the pane.
pub struct OpenSpec {
    pub generation: u64,
    pub line: CommandLine,
    pub rows: u16,
    pub cols: u16,
    /// Nothing starts unless `line`'s folder exists, which the worker looks
    /// at, as a hung folder must not block the UI thread. Otherwise a folder
    /// that is gone is left out (doc/0045).
    pub needs_cwd: bool,
    /// The lines of history its screen keeps ([`SHELL_HISTORY`] for a shell).
    pub history: usize,
}

impl Embed {
    /// The descriptor that becomes readable when a client has something new,
    /// once there has been a client.
    pub fn wake_fd(&self) -> Option<RawFd> {
        self.waker.as_ref().map(|w| w.read.as_raw_fd())
    }

    /// Empties the wake pipe.
    pub fn drain_wake(&self) {
        if let Some(waker) = &self.waker {
            waker.drain();
        }
    }

    /// Whether a client is open.
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    /// Whether a child's update is still being written (`?2026`).
    pub fn syncing(&self, now: Instant) -> bool {
        [&self.current, &self.shell]
            .into_iter()
            .flatten()
            .any(|c| c.shared.screen.syncing(now))
    }

    /// Closes the current client and starts `spec`, on a worker thread.
    /// An error means no PTY or thread could be had; a child that cannot be
    /// started is reported later as [`ClientEvent::Failed`].
    pub fn open(&mut self, spec: OpenSpec) -> io::Result<PaneScreen> {
        self.close();
        let (client, screen) = self.start(spec)?;
        self.current = Some(client);
        Ok(screen)
    }

    /// [`Embed::open`] for the shell, which leaves the Claude client alone.
    pub fn open_shell(&mut self, spec: OpenSpec) -> io::Result<PaneScreen> {
        self.close_shell();
        let (client, screen) = self.start(spec)?;
        self.shell = Some(client);
        Ok(screen)
    }

    fn start(&mut self, spec: OpenSpec) -> io::Result<(Client, PaneScreen)> {
        let waker = match &self.waker {
            Some(waker) => Arc::clone(waker),
            None => {
                let waker = Arc::new(Waker::new()?);
                self.waker = Some(Arc::clone(&waker));
                waker
            }
        };
        let (master, slave) = open_pty(spec.rows.max(1), spec.cols.max(1))?;
        let (control_read, control_write) = pipe()?;
        let screen = PaneScreen::with_history(spec.rows, spec.cols, spec.history);
        let master = Arc::new(master);
        let shared = Arc::new(Shared {
            generation: spec.generation,
            screen: screen.clone(),
            waker,
            close: AtomicBool::new(false),
            done: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            event: Mutex::new(None),
            clipboard: Mutex::new(Vec::new()),
            control_read,
            control_write,
        });
        let line = spec.line;
        let needs_cwd = spec.needs_cwd;
        let worker = Arc::clone(&shared);
        let worker_master = Arc::clone(&master);
        thread::Builder::new()
            .name("embed-client".into())
            .spawn(move || run_client(worker, worker_master, slave, line, needs_cwd))?;
        Ok((Client { shared, master }, screen))
    }

    /// Closes the current client, if any: the worker sends `SIGHUP` and reaps
    /// it, whatever the UI does meanwhile.
    pub fn close(&mut self) {
        let client = self.current.take();
        self.close_client(client);
    }

    /// Closes the shell, if any, as [`Embed::close`] does the client.
    pub fn close_shell(&mut self) {
        let client = self.shell.take();
        self.close_client(client);
    }

    fn close_client(&mut self, client: Option<Client>) {
        self.closing.retain(|s| !s.done.load(Ordering::SeqCst));
        if let Some(client) = client {
            client.shared.close.store(true, Ordering::SeqCst);
            poke(&client.shared.control_write);
            self.closing.push(client.shared);
        }
    }

    /// Closes both children and waits up to `wait` for every closed one to be
    /// reaped: tower is exiting. What is left then ends with tower, whose
    /// exit closes the PTY.
    pub fn shutdown(&mut self, wait: Duration) {
        self.close();
        self.close_shell();
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline
            && self.closing.iter().any(|s| !s.done.load(Ordering::SeqCst))
        {
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Queues `bytes` for the PTY; never blocks.
    pub fn write(&self, bytes: &[u8]) {
        if let Some(client) = &self.current {
            client.write(bytes);
        }
    }

    /// [`Embed::write`] for the shell.
    pub fn write_shell(&self, bytes: &[u8]) {
        if let Some(client) = &self.shell {
            client.write(bytes);
        }
    }

    /// Resizes the PTY, which sends the child `SIGWINCH`, and the emulator.
    pub fn resize(&self, rows: u16, cols: u16) {
        if let Some(client) = &self.current {
            client.resize(rows, cols);
        }
    }

    /// [`Embed::resize`] for the shell.
    pub fn resize_shell(&self, rows: u16, cols: u16) {
        if let Some(client) = &self.shell {
            client.resize(rows, cols);
        }
    }

    /// How a child ended, if one did. It is then no longer held.
    pub fn take_event(&mut self) -> Option<ClientEvent> {
        for slot in [&mut self.current, &mut self.shell] {
            let event = slot.as_ref().and_then(|client| {
                client
                    .shared
                    .event
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
            });
            if event.is_some() {
                *slot = None;
                return event;
            }
        }
        None
    }

    /// The clipboard writes waiting to go to the real terminal.
    pub fn take_clipboard(&self) -> Vec<Vec<u8>> {
        [&self.current, &self.shell]
            .into_iter()
            .flatten()
            .flat_map(|client| {
                std::mem::take(
                    &mut *client
                        .shared
                        .clipboard
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()),
                )
            })
            .collect()
    }
}

impl Client {
    fn write(&self, bytes: &[u8]) {
        let mut queue = self.shared.queue();
        if queue.len() + bytes.len() <= QUEUE_MAX {
            queue.extend(bytes);
        }
        drop(queue);
        poke(&self.shared.control_write);
    }

    fn resize(&self, rows: u16, cols: u16) {
        let _ = set_winsize(self.master.as_raw_fd(), rows.max(1), cols.max(1));
        self.shared.screen.set_size(rows, cols);
    }
}

impl Drop for Embed {
    fn drop(&mut self) {
        self.close();
        self.close_shell();
    }
}

/// The folder to run in: the session's, unless it is gone. A folder that was
/// removed must not look like a failed PTY (doc/0045).
fn usable_cwd(cwd: Option<PathBuf>) -> Option<PathBuf> {
    cwd.filter(|dir| dir.is_dir())
}

fn spawn_child(mut line: CommandLine, slave: OwnedFd) -> io::Result<Child> {
    line.cwd = usable_cwd(line.cwd.take());
    let mut cmd = line.to_command();
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(hides_from_child) {
            cmd.env_remove(name);
        }
    }
    cmd.env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe calls between fork and exec. The PTY
    // becomes the controlling terminal of a new session, so nothing typed
    // there can signal tower, nor tower's terminal the child.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
}

/// Runs `line` in the PTY until it ends or tower closes it.
fn run_client(
    shared: Arc<Shared>,
    master: Arc<OwnedFd>,
    slave: OwnedFd,
    line: CommandLine,
    needs_cwd: bool,
) {
    let _done = WorkerDone(Arc::clone(&shared));
    if needs_cwd && !line.cwd.as_ref().is_some_and(|dir| dir.is_dir()) {
        shared.report(ClientEvent::FolderGone {
            generation: shared.generation,
        });
        return;
    }
    let program = line.program.clone();
    let mut child = match spawn_child(line, slave) {
        Ok(child) => child,
        Err(err) => {
            shared.report(ClientEvent::Failed {
                generation: shared.generation,
                error: format!("cannot run `{program}`: {err}"),
            });
            return;
        }
    };
    if let Some(code) = pump(&shared, &master, &mut child) {
        shared.report(ClientEvent::Exited {
            generation: shared.generation,
            code,
        });
        return;
    }
    drop(master);
    hang_up(&mut child);
}

/// Moves bytes until the child has ended (its exit code, `None` for a
/// signal) or tower closes the client (`None` outside).
fn pump(shared: &Shared, master: &OwnedFd, child: &mut Child) -> Option<Option<i32>> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut hung_up = false;
    let mut hung_up_at = None;
    loop {
        if shared.close.load(Ordering::SeqCst) {
            return None;
        }
        let wants_write = !hung_up && !shared.queue().is_empty();
        let mut fds = [
            libc::pollfd {
                fd: if hung_up { -1 } else { master.as_raw_fd() },
                events: if wants_write {
                    libc::POLLIN | libc::POLLOUT
                } else {
                    libc::POLLIN
                },
                revents: 0,
            },
            libc::pollfd {
                fd: shared.control_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = if hung_up { 20 } else { 1000 };
        // SAFETY: two valid, initialised pollfds that outlive the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if ready > 0 && fds[1].revents != 0 {
            drain(&shared.control_read);
        }
        if ready > 0 && fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            // SAFETY: reads into a valid buffer of the length passed.
            let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                take_output(shared, &buf[..n as usize]);
            } else if n == 0 || io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
                // EIO: every slave is closed. Nothing more can come.
                hung_up = true;
                hung_up_at = Some(Instant::now());
            }
        }
        if !hung_up && ready > 0 && fds[0].revents & libc::POLLOUT != 0 {
            write_queued(shared, master);
        }
        // A child that closed its terminal is about to exit; one still
        // alive long after that is told to go. Without a hang-up the child
        // may have left something holding the PTY open: looked at on every
        // timeout.
        if hung_up || ready == 0 {
            if let Ok(Some(status)) = child.try_wait() {
                return Some(status.code());
            }
            if hung_up_at.is_some_and(|at| at.elapsed() > CLOSE_WAIT) {
                hang_up(child);
                return Some(child.try_wait().ok().flatten().and_then(|s| s.code()));
            }
        }
    }
}

/// Runs the child's output through the emulator; hands on what it asks for.
fn take_output(shared: &Shared, bytes: &[u8]) {
    shared.screen.feed(bytes);
    let replies = shared.screen.take_replies();
    if !replies.is_empty() {
        shared.queue().extend(replies);
    }
    let writes = shared.screen.take_clipboard();
    if !writes.is_empty() {
        shared
            .clipboard
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(writes);
    }
    shared.waker.wake();
}

fn write_queued(shared: &Shared, master: &OwnedFd) {
    let mut queue = shared.queue();
    while !queue.is_empty() {
        let (front, _) = queue.as_slices();
        // SAFETY: writes from a valid slice to a non-blocking descriptor.
        let n = unsafe { libc::write(master.as_raw_fd(), front.as_ptr().cast(), front.len()) };
        if n <= 0 {
            return;
        }
        queue.drain(..n as usize);
    }
}

/// `SIGHUP` to the child's process group (a wrapper command's `claude`
/// included), `SIGKILL` if it is still there after [`CLOSE_WAIT`], then reaps
/// it, so it never stays a zombie.
fn hang_up(child: &mut Child) {
    let pid = child.id() as libc::pid_t;
    // SAFETY: the child is not reaped yet, so its pid and group (it called
    // setsid) are still its own.
    unsafe {
        libc::kill(-pid, libc::SIGHUP);
        libc::kill(pid, libc::SIGHUP);
    }
    let deadline = Instant::now() + CLOSE_WAIT;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    // SAFETY: as above.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn keys_as_a_terminal_sends_them() {
        let none = KeyModifiers::NONE;
        let cases: Vec<(KeyEvent, &[u8])> = vec![
            (press(KeyCode::Char('a'), none), b"a"),
            (press(KeyCode::Char('я'), none), "я".as_bytes()),
            (press(KeyCode::Char('c'), KeyModifiers::CONTROL), b"\x03"),
            (press(KeyCode::Char(' '), KeyModifiers::CONTROL), b"\0"),
            (press(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx"),
            (press(KeyCode::Enter, none), b"\r"),
            (press(KeyCode::Enter, KeyModifiers::ALT), b"\x1b\r"),
            (press(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b[13;2u"),
            (press(KeyCode::Enter, KeyModifiers::CONTROL), b"\x1b[13;5u"),
            (press(KeyCode::Tab, none), b"\t"),
            (press(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z"),
            (press(KeyCode::Esc, none), b"\x1b"),
            (press(KeyCode::Backspace, none), b"\x7f"),
            (press(KeyCode::Up, none), b"\x1b[A"),
            (press(KeyCode::Left, KeyModifiers::SHIFT), b"\x1b[1;2D"),
            (press(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C"),
            (press(KeyCode::Home, none), b"\x1b[H"),
            (press(KeyCode::Delete, none), b"\x1b[3~"),
            (press(KeyCode::PageUp, none), b"\x1b[5~"),
            (press(KeyCode::F(1), none), b"\x1bOP"),
            (press(KeyCode::F(5), none), b"\x1b[15~"),
            (press(KeyCode::F(6), none), b"\x1b[17~"),
            (press(KeyCode::F(11), none), b"\x1b[23~"),
        ];
        for (key, want) in cases {
            assert_eq!(encode_key(&key, false), want, "{key:?}");
        }
    }

    #[test]
    fn arrows_follow_the_cursor_key_mode() {
        let up = press(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(encode_key(&up, false), b"\x1b[A");
        assert_eq!(encode_key(&up, true), b"\x1bOA");
        let shifted = press(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(encode_key(&shifted, true), b"\x1b[1;2A");
    }

    #[test]
    fn paste_brackets_cannot_be_closed_from_inside() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(
            encode_paste("x\x1b[201~y", true),
            b"\x1b[200~xy\x1b[201~".to_vec()
        );
    }

    #[test]
    fn mouse_reports_are_sgr() {
        let none = KeyModifiers::NONE;
        let left = MouseButton::Left;
        let report = |kind, modifiers| encode_mouse(kind, modifiers, 3, 7).unwrap();
        assert_eq!(report(MouseEventKind::Down(left), none), b"\x1b[<0;3;7M");
        assert_eq!(report(MouseEventKind::Up(left), none), b"\x1b[<0;3;7m");
        assert_eq!(report(MouseEventKind::Drag(left), none), b"\x1b[<32;3;7M");
        assert_eq!(report(MouseEventKind::Moved, none), b"\x1b[<35;3;7M");
        assert_eq!(report(MouseEventKind::ScrollUp, none), b"\x1b[<64;3;7M");
        assert_eq!(
            report(MouseEventKind::Down(left), KeyModifiers::SHIFT),
            b"\x1b[<4;3;7M"
        );
    }

    #[test]
    fn terminal_identity_stays_out_of_the_childs_environment() {
        for name in [
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "TMUX",
            "TMUX_PANE",
            "WT_SESSION",
            "KITTY_WINDOW_ID",
            "WEZTERM_PANE",
            "VTE_VERSION",
            "COLUMNS",
        ] {
            assert!(hides_from_child(name), "{name}");
        }
        for name in ["TERM", "COLORTERM", "HOME", "PATH", "SSH_TTY", "NO_COLOR"] {
            assert!(!hides_from_child(name), "{name}");
        }
    }

    #[test]
    fn queries_are_answered_and_modes_noted() {
        let screen = PaneScreen::new(10, 40);
        screen.feed(b"\x1b[c");
        assert_eq!(screen.take_replies(), b"\x1b[?62;22c");
        screen.feed(b"\x1b[>0q");
        assert!(screen.take_replies().starts_with(b"\x1bP>|tower "));
        screen.feed(b"\x1b[?2026$p");
        assert_eq!(screen.take_replies(), b"\x1b[?2026;2$y");
        screen.feed(b"ab\x1b[6n");
        assert_eq!(screen.take_replies(), b"\x1b[1;3R");
        assert!(!screen.focus_events());
        screen.feed(b"\x1b[?1000;1004h");
        assert!(screen.focus_events());
        screen.feed(b"\x1b[?1004l");
        assert!(!screen.focus_events());
        let now = Instant::now();
        assert!(!screen.syncing(now));
        screen.feed(b"\x1b[?2026h");
        assert!(screen.syncing(Instant::now()));
        assert!(!screen.syncing(Instant::now() + SYNC_HOLD));
        screen.feed(b"\x1b[?2026l");
        assert!(!screen.syncing(Instant::now()));
    }

    fn typed(screen: &PaneScreen, text: &str) {
        screen.feed(text.as_bytes());
    }

    #[test]
    fn a_selection_is_what_the_cells_show_trimmed_at_each_lines_end() {
        let screen = PaneScreen::new(5, 20);
        typed(&screen, "alpha beta   \r\n\r\ngamma\r\n");
        // Across rows, with an empty row, spaces at the ends gone.
        let got = screen.selected((0, 6), (2, 19));
        assert_eq!(got.text, "beta\n\ngamma");
        assert_eq!(got.spans, vec![(0, 6, 9), (2, 0, 4)]);
        // Either direction of the drag.
        assert_eq!(screen.selected((2, 4), (0, 6)), got);
        // Blank rows after the text are not copied, nor highlighted.
        assert_eq!(screen.selected((2, 0), (4, 19)).text, "gamma");
        // Nothing but blanks.
        let none = screen.selected((3, 0), (4, 19));
        assert_eq!(none, Selected::default());
    }

    #[test]
    fn a_wide_character_is_selected_whole_from_either_half() {
        let screen = PaneScreen::new(3, 20);
        typed(&screen, "ab界cd");
        assert_eq!(screen.selected((0, 3), (0, 4)).text, "界c");
        assert_eq!(screen.selected((0, 3), (0, 4)).spans, vec![(0, 2, 4)]);
        assert_eq!(screen.selected((0, 2), (0, 2)).text, "界");
        assert_eq!(screen.selected((0, 2), (0, 2)).spans, vec![(0, 2, 3)]);
    }

    #[test]
    fn a_wrapped_line_comes_back_as_one() {
        let screen = PaneScreen::new(5, 10);
        // A space falls on the end of the first row: part of the text.
        typed(&screen, "abcdefghi jklmn\r\nnext");
        assert!(screen.with(|s| s.row_wrapped(0)));
        assert_eq!(
            screen.selected((0, 0), (2, 9)).text,
            "abcdefghi jklmn\nnext"
        );
        // A wide character that did not fit leaves an empty cell: not text.
        let screen = PaneScreen::new(5, 10);
        typed(&screen, "abcdefghi界z");
        assert_eq!(screen.selected((0, 0), (1, 9)).text, "abcdefghi界z");
    }

    #[test]
    fn a_double_click_finds_the_word_on_its_row() {
        let screen = PaneScreen::new(3, 30);
        typed(&screen, "see (src/embed.rs:12) 日本語 x");
        assert_eq!(screen.word_at((0, 1)), Some(((0, 0), (0, 2))));
        assert_eq!(screen.word_at((0, 3)), None);
        assert_eq!(screen.word_at((0, 4)), None, "the bracket");
        let (a, b) = screen.word_at((0, 8)).unwrap();
        assert_eq!(screen.selected(a, b).text, "src/embed.rs:12");
        // From the second half of a wide character too.
        let (a, b) = screen.word_at((0, 25)).unwrap();
        assert_eq!(screen.selected(a, b).text, "日本語");
        assert_eq!(screen.word_at((2, 3)), None, "an empty row");
    }

    #[test]
    fn the_clipboard_write_is_exact_base64() {
        let cases: [(&str, &str); 5] = [
            ("", ""),
            ("a", "YQ=="),
            ("ab", "YWI="),
            ("abc", "YWJj"),
            ("héllo 界", "aMOpbGxvIOeVjA=="),
        ];
        for (text, b64) in cases {
            let want = format!("\x1b]52;c;{b64}\x1b\\");
            assert_eq!(clipboard_write(text).unwrap(), want.as_bytes(), "{text}");
        }
        assert!(clipboard_write(&"x".repeat(CLIPBOARD_MAX)).is_none());
        // What the emulator itself takes for base64: the write it would pass.
        let screen = PaneScreen::new(3, 20);
        screen.feed(&clipboard_write("héllo 界").unwrap());
        assert_eq!(
            screen.take_clipboard(),
            vec![clipboard_write("héllo 界").unwrap()]
        );
    }

    fn numbered(screen: &PaneScreen, lines: std::ops::Range<usize>) {
        for k in lines {
            screen.feed(format!("line{k}\r\n").as_bytes());
        }
    }

    fn top_row(screen: &PaneScreen) -> String {
        screen.selected((0, 0), (0, 200)).text
    }

    #[test]
    fn the_history_is_bounded_and_scrolling_stays_inside_it() {
        let screen = PaneScreen::with_history(5, 20, 100);
        assert_eq!(screen.history(), 0);
        assert!(!screen.scroll(3), "nothing to scroll to");
        numbered(&screen, 0..1000);
        // The last row is the empty one after the last line.
        assert_eq!(screen.history(), 100);
        assert_eq!(screen.scrolled(), 0);
        assert!(screen.scroll(10_000));
        assert_eq!(screen.scrolled(), 100);
        assert_eq!(top_row(&screen), "line896");
        assert!(!screen.scroll(1), "at the top");
        assert!(screen.scroll(-99));
        assert_eq!(screen.scrolled(), 1);
        assert!(screen.scroll(-5));
        assert_eq!(screen.scrolled(), 0);
        assert_eq!(top_row(&screen), "line996");
        // `history` leaves the view where it is.
        screen.scroll(7);
        assert_eq!(screen.history(), 100);
        assert_eq!(screen.scrolled(), 7);
        assert!(screen.to_bottom());
        assert!(!screen.to_bottom());
    }

    #[test]
    fn the_shells_limit_holds_what_it_says() {
        let screen = PaneScreen::with_history(5, 20, SHELL_HISTORY);
        numbered(&screen, 0..SHELL_HISTORY + 500);
        assert_eq!(screen.history(), SHELL_HISTORY);
    }

    #[test]
    fn new_output_does_not_move_a_view_that_is_scrolled_up() {
        let screen = PaneScreen::with_history(5, 20, 100);
        numbered(&screen, 0..50);
        screen.scroll(20);
        let top = top_row(&screen);
        numbered(&screen, 50..60);
        assert_eq!(top_row(&screen), top);
        assert_eq!(screen.scrolled(), 30);
        // At the limit the oldest lines go, and the view with them last.
        numbered(&screen, 60..1000);
        assert_eq!(screen.scrolled(), 100);
        assert!(screen.scrollable());
    }

    #[test]
    fn a_selection_reads_the_history_while_scrolled() {
        let screen = PaneScreen::with_history(5, 20, 100);
        numbered(&screen, 0..50);
        screen.scroll(20);
        let top = top_row(&screen);
        let got = screen.selected((0, 0), (1, 19));
        assert!(
            got.text.starts_with(&format!("{top}\nline")),
            "{}",
            got.text
        );
    }

    #[test]
    fn only_a_program_without_the_mouse_on_the_main_screen_scrolls() {
        let screen = PaneScreen::with_history(5, 20, 100);
        assert!(screen.scrollable());
        screen.feed(b"\x1b[?1000h");
        assert!(!screen.scrollable());
        screen.feed(b"\x1b[?1000l\x1b[?1049h");
        assert!(!screen.scrollable());
        screen.feed(b"\x1b[?1049l");
        assert!(screen.scrollable());
        // The Claude client keeps no history.
        assert!(!PaneScreen::new(5, 20).scrollable());
        // The alternate screen comes back at the bottom.
        numbered(&screen, 0..50);
        screen.scroll(5);
        screen.feed(b"\x1b[?1049h\x1b[?1049l");
        assert_eq!(screen.scrolled(), 0);
    }

    #[test]
    fn a_resize_keeps_the_view_inside_the_history() {
        let screen = PaneScreen::with_history(10, 20, 100);
        numbered(&screen, 0..80);
        screen.scroll(30);
        let top = top_row(&screen);
        // Lines of the old width stay; a selection reads what they have.
        screen.set_size(4, 8);
        assert_eq!(screen.scrolled(), 30);
        assert_eq!(top_row(&screen), top);
        screen.set_size(30, 120);
        assert_eq!(top_row(&screen), top);
        assert!(!screen.selected((0, 0), (29, 119)).text.is_empty());
        numbered(&screen, 80..90);
        assert!(screen.scroll(1000));
        assert!(screen.to_bottom());
    }

    #[test]
    fn only_clipboard_writes_pass() {
        let screen = PaneScreen::new(10, 40);
        screen.feed(b"\x07\x1b]0;title\x07\x1b]9;note\x07\x1b]777;notify;a;b\x07");
        screen.feed(b"\x1b]52;c;?\x07");
        assert!(screen.take_clipboard().is_empty());
        assert!(screen.take_replies().is_empty());
        screen.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(screen.take_clipboard(), vec![b"\x1b]52;c;aGVsbG8=\x1b\\"]);
        screen.feed(b"\x1b]52;c;not base64!\x07");
        assert!(screen.take_clipboard().is_empty());
    }
}
