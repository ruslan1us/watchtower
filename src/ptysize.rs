//! The real terminal size of a background session, for the logs replay
//! (doc/0025).
//!
//! `claude logs` replays the session's raw terminal output but does not say
//! how big that terminal is, and Claude Code redraws mostly with relative
//! cursor moves, so a replay at the wrong size garbles the screen
//! (reference/claude-code-cli.md, "`claude logs` output"). A background
//! session's worker process (the listed `pid`) has its own PTY slave on
//! fd 0/1/2; that PTY is 50 × 200 until the session is first attached, and
//! `claude attach` resizes it to the attaching terminal, where it stays
//! after detaching. [`session_size`] reads it:
//!
//! 1. `/proc/<pid>/stat` is read once. Its field 22 (start ticks) must equal
//!    the ticks the pid-reuse guard remembered for the row (doc/0017), so a
//!    reused pid gives nothing; field 7 is `tty_nr`, the device number of
//!    the process's controlling terminal.
//! 2. `tty_nr` is decoded ([`decode_dev`]) and mapped to `/dev/pts/<n>`
//!    ([`pts_number`]). Only Unix98 PTY slaves are accepted; 0 (no
//!    terminal) and every other device are rejected.
//! 3. [`TerminalReader::pts_size`] opens that device read-only with
//!    `O_NOCTTY | O_NONBLOCK | O_CLOEXEC`, checks with `fstat` that it is a
//!    character device with the same device number (a wrong mapping or
//!    another devpts instance is rejected), asks `TIOCGWINSZ` and closes
//!    it. It never reads from or writes to the device.
//! 4. A size with 0 rows or 0 columns counts as unknown.
//!
//! Every failure gives `None`, and the replay falls back to inferring the
//! size from the stream (doc/0009).

use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};

use crate::merge::{ProcEnviron, parse_start_ticks};

/// First major number of the Unix98 PTY slaves (`UNIX98_PTY_SLAVE_MAJOR`).
pub const PTS_MAJOR_FIRST: u32 = 136;
/// Last major number reserved for Unix98 PTY slaves (136 + 8 − 1).
pub const PTS_MAJOR_LAST: u32 = 143;

/// What the logs job needs to read a session's terminal size: the listed
/// pid and the start ticks the pid-reuse guard remembered for it
/// (doc/0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtyProbe {
    /// The row's `pid`.
    pub pid: u32,
    /// Field 22 of `/proc/<pid>/stat` when the guard accepted the process.
    pub start_ticks: u64,
}

/// The OS reads behind [`session_size`]. Abstracted so tests can fake
/// them; the real one is [`ProcEnviron`].
pub trait TerminalReader: Send + Sync {
    /// The whole `/proc/<pid>/stat` line. `Err` when it cannot be read.
    fn stat(&self, pid: u32) -> io::Result<String>;

    /// The window size of `/dev/pts/<pts>` as `(rows, cols)`, after
    /// checking that the device is a character device numbered
    /// `major:minor`. `Err` when it cannot be opened, fails a check or the
    /// `ioctl` fails.
    fn pts_size(&self, pts: u32, major: u32, minor: u32) -> io::Result<(u16, u16)>;
}

/// Field 7 (`tty_nr`) of a `/proc/<pid>/stat` line, as the kernel's 32-bit
/// device number. The kernel prints it as a signed `int`, so a negative
/// value is taken modulo 2³². Fields are counted after the last `)`, as
/// for the start ticks.
pub fn parse_tty_nr(stat: &str) -> Option<u32> {
    let after = &stat[stat.rfind(')')? + 1..];
    // Field 3 is the first one after the name, so field 7 is the 5th.
    let value: i64 = after.split_whitespace().nth(4)?.parse().ok()?;
    // Truncating keeps the low 32 bits: the value modulo 2³².
    (i64::from(i32::MIN)..=i64::from(u32::MAX))
        .contains(&value)
        .then_some(value as u32)
}

/// `(major, minor)` of a Linux 32-bit device number as `/proc/<pid>/stat`
/// prints it (`new_encode_dev`): bits 8–19 are the major, bits 0–7 and
/// 20–31 the minor.
pub fn decode_dev(dev: u32) -> (u32, u32) {
    let major = (dev >> 8) & 0xfff;
    let minor = (dev & 0xff) | ((dev >> 12) & 0xf_ff00);
    (major, minor)
}

/// The `n` of `/dev/pts/<n>` for a Unix98 PTY slave `major:minor`, or
/// `None` for any other device (0:0 is "no terminal").
///
/// Linux reserves majors 136–143 for PTY slaves (`devices.txt`). A current
/// kernel (devpts, 20-bit minors) creates every slave as major 136 with the
/// pts index as its minor, so `/dev/pts/300` is 136:300; the encoding of
/// [`decode_dev`] already undoes the split of minors ≥ 256. The majors
/// 137–143 come from the older 8-bit-minor layout, where index `n` was
/// major `136 + n / 256`, minor `n % 256`; there the minor is below 256.
/// `(major − 136) · 256 + minor` gives the index in both layouts, so it is
/// used for every accepted major, and a 137–143 major with a minor ≥ 256
/// (which neither layout produces) is rejected. The caller still checks
/// that the device it opens has this very number.
pub fn pts_number(major: u32, minor: u32) -> Option<u32> {
    match major {
        PTS_MAJOR_FIRST => Some(minor),
        m if m > PTS_MAJOR_FIRST && m <= PTS_MAJOR_LAST && minor < 256 => {
            Some((m - PTS_MAJOR_FIRST) * 256 + minor)
        }
        _ => None,
    }
}

/// The terminal size of the session process `probe` names, as
/// `(rows, cols)`, or `None` when it cannot be known (see the module docs):
/// unreadable `stat`, different start ticks (a reused pid), no Unix98 PTY,
/// a device that fails the checks, or a zero size.
pub fn session_size(reader: &dyn TerminalReader, probe: TtyProbe) -> Option<(u16, u16)> {
    let stat = reader.stat(probe.pid).ok()?;
    if parse_start_ticks(&stat)? != probe.start_ticks {
        return None;
    }
    let (major, minor) = decode_dev(parse_tty_nr(&stat)?);
    let pts = pts_number(major, minor)?;
    let (rows, cols) = reader.pts_size(pts, major, minor).ok()?;
    (rows > 0 && cols > 0).then_some((rows, cols))
}

impl TerminalReader for ProcEnviron {
    fn stat(&self, pid: u32) -> io::Result<String> {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
    }

    fn pts_size(&self, pts: u32, major: u32, minor: u32) -> io::Result<(u16, u16)> {
        // Read-only, never the controlling terminal, no wait for a
        // carrier, not inherited by children. std adds `O_CLOEXEC` itself;
        // it is named here as well so the flags read as doc/0025 says.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/dev/pts/{pts}"))?;
        let meta = file.metadata()?;
        let rdev = meta.rdev();
        if !meta.file_type().is_char_device()
            || libc::major(rdev) != major
            || libc::minor(rdev) != minor
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not the session's terminal device",
            ));
        }
        let mut ws = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `file` keeps the descriptor open for the whole call, and
        // TIOCGWINSZ writes exactly one `winsize` through the pointer, which
        // points at a live, properly aligned local of that type.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((ws.ws_row, ws.ws_col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_observed_tty_nr() {
        // /dev/pts/3 as seen in a session's stat (reference doc).
        assert_eq!(decode_dev(34819), (136, 3));
        assert_eq!(pts_number(136, 3), Some(3));
    }

    #[test]
    fn real_reader_rejects_a_missing_device() {
        assert!(ProcEnviron.pts_size(u32::MAX, 136, u32::MAX).is_err());
    }
}
