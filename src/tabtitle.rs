//! The terminal's window or tab title (doc/0047): `tower`, or `tower · 2 need
//! you` while sessions need attention, so it shows from another tab.
//!
//! This module only works out the bytes; the runtime ([`crate::tui`]) writes
//! them. The text is tower's own, built from a count, so nothing from the CLI's
//! data or a pane's program can get into it. The title is set with `OSC 2`
//! ended by `ST`, never by BEL (doc/0007), and only when the text changes.
//!
//! The previous title is kept with XTWINOPS: `CSI 22;2t` saves it on the
//! terminal's title stack and `CSI 23;2t` brings it back. A terminal that does
//! not know them ignores them, and those parameters are below 24, where
//! `CSI Ps t` would mean a resize; it then has no previous title to bring back.
//! So the neutral `tower` is set right before every `CSI 23;2t`: on such a
//! terminal the title stays `tower` instead of an attention count that is no
//! longer watched.

/// The title with nothing to report.
pub const NAME: &str = "tower";

/// XTWINOPS: save the window title on the terminal's title stack.
pub const PUSH: &str = "\x1b[22;2t";

/// XTWINOPS: bring the saved window title back.
pub const POP: &str = "\x1b[23;2t";

/// `OSC 2 ; <text> ST`: sets the window title.
pub fn set_sequence(text: &str) -> String {
    format!("\x1b]2;{text}\x1b\\")
}

/// The title for `attention` highlighted sessions, in the words of the status
/// line's counter (doc/0007).
pub fn text(attention: usize) -> String {
    match attention {
        0 => NAME.to_owned(),
        1 => format!("{NAME} · 1 needs you"),
        n => format!("{NAME} · {n} need you"),
    }
}

/// What tower has done to the title.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabTitle {
    /// The previous title is on the terminal's stack.
    pushed: bool,
    /// The text tower last set; `None` after a hand-over, whose child may have
    /// set its own.
    shown: Option<String>,
}

impl TabTitle {
    /// A title not touched yet.
    pub const fn new() -> TabTitle {
        TabTitle {
            pushed: false,
            shown: None,
        }
    }

    /// The bytes that make the title `wanted` (`None`: tower leaves the title
    /// alone, `[ui] tab_title = false`); empty when it already is.
    pub fn update(&mut self, wanted: Option<&str>) -> String {
        let Some(wanted) = wanted else {
            return self.restore();
        };
        if self.shown.as_deref() == Some(wanted) {
            return String::new();
        }
        let mut out = String::new();
        if !self.pushed {
            out.push_str(PUSH);
            self.pushed = true;
        }
        out.push_str(&set_sequence(wanted));
        self.shown = Some(wanted.to_owned());
        out
    }

    /// The bytes for a hand-over: the child sees the previous title, which
    /// stays saved for tower's exit, so a title the child leaves behind is
    /// never taken for it. The next [`TabTitle::update`] sets tower's again.
    pub fn hand_over(&mut self) -> String {
        if !self.pushed {
            return String::new();
        }
        self.shown = None;
        format!("{}{POP}{PUSH}", set_sequence(NAME))
    }

    /// The bytes that bring the previous title back, for good (exit, a panic,
    /// or the setting turned off); empty when tower never touched it.
    pub fn restore(&mut self) -> String {
        if !self.pushed {
            return String::new();
        }
        self.pushed = false;
        self.shown = None;
        format!("{}{POP}", set_sequence(NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counter_has_the_status_lines_words() {
        assert_eq!(text(0), "tower");
        assert_eq!(text(1), "tower · 1 needs you");
        assert_eq!(text(2), "tower · 2 need you");
    }

    #[test]
    fn the_first_title_saves_the_previous_one_and_no_title_repeats() {
        let mut title = TabTitle::new();
        assert_eq!(title.update(Some("tower")), "\x1b[22;2t\x1b]2;tower\x1b\\");
        assert_eq!(title.update(Some("tower")), "");
        assert_eq!(
            title.update(Some("tower · 2 need you")),
            "\x1b]2;tower · 2 need you\x1b\\"
        );
        assert_eq!(title.update(Some("tower · 2 need you")), "");
    }

    #[test]
    fn off_never_touches_the_title() {
        let mut title = TabTitle::new();
        assert_eq!(title.update(None), "");
        assert_eq!(title.hand_over(), "");
        assert_eq!(title.restore(), "");
    }

    #[test]
    fn turned_off_brings_the_previous_title_back_once() {
        let mut title = TabTitle::new();
        title.update(Some("tower · 1 needs you"));
        assert_eq!(title.update(None), "\x1b]2;tower\x1b\\\x1b[23;2t");
        assert_eq!(title.update(None), "");
        // On again: it is saved again.
        assert!(title.update(Some("tower")).starts_with("\x1b[22;2t"));
    }

    #[test]
    fn a_hand_over_shows_the_previous_title_and_keeps_it_saved() {
        let mut title = TabTitle::new();
        title.update(Some("tower · 1 needs you"));
        assert_eq!(title.hand_over(), "\x1b]2;tower\x1b\\\x1b[23;2t\x1b[22;2t");
        // Back: set again though the text is the same, and not saved twice.
        assert_eq!(
            title.update(Some("tower · 1 needs you")),
            "\x1b]2;tower · 1 needs you\x1b\\"
        );
        assert_eq!(title.restore(), "\x1b]2;tower\x1b\\\x1b[23;2t");
        assert_eq!(title.restore(), "");
    }

    #[test]
    fn no_sequence_holds_a_bel() {
        let mut title = TabTitle::new();
        let all = [
            title.update(Some("tower · 2 need you")),
            title.hand_over(),
            title.update(Some("tower")),
            title.restore(),
        ]
        .concat();
        assert!(!all.contains('\x07'));
    }
}
