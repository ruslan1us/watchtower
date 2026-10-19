//! A session's logs followed across captures (doc/0030), on synthetic
//! output in Claude Code's shape: stitching overlapping captures, the
//! reset when they do not overlap or the size or process changes,
//! identical captures, and when the screen has a base.

#[path = "support/synthetic.rs"]
mod synthetic;

use synthetic::{Session, trimmed};
use watchtower::logstream::{Fed, LogStream, ResetCause};
use watchtower::logview;

const ROWS: u16 = 30;
const COLS: u16 = 100;
const SIZE: Option<(u16, u16)> = Some((ROWS, COLS));

fn shown(stream: &mut LogStream) -> Vec<String> {
    trimmed(&stream.screen().plain_lines())
}

#[test]
fn overlapping_captures_keep_the_full_screen() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(5);
    let mut stream = LogStream::new(s.capture(), SIZE, Some(7));
    assert!(stream.has_base());
    // Fetch every 25 writes: each capture of 40 overlaps the previous one,
    // and soon none holds the start's repaint.
    for turn in 0..10 {
        s.spin(20);
        s.set(8 + turn, &format!("message {turn}"));
        s.frame();
        s.spin(4);
        let fed = stream.feed(s.capture(), SIZE, Some(7));
        assert!(matches!(fed, Fed::Stitched(n) if n > 0), "{fed:?}");
    }
    assert!(!logview::has_clear(&s.capture()), "the base left the ring");
    assert!(stream.has_base());
    let screen = stream.screen();
    assert!(!screen.partial());
    assert_eq!(trimmed(&screen.plain_lines()), trimmed(&s.reference()));
    let text = screen.plain_lines().join("\n");
    for part in [
        "Example Code v9.9.9",
        "> run the slow task",
        "message 0",
        "message 9",
        "auto mode on",
    ] {
        assert!(text.contains(part), "{part} missing:\n{text}");
    }

    // The last capture on its own: the header, the first messages and the
    // prompt box are missing, and the screen says it is partial.
    let single = logview::render_sized(&s.capture(), SIZE);
    assert!(single.partial());
    let alone = single.plain_lines().join("\n");
    assert!(!alone.contains("Example Code"), "{alone}");
    assert!(!alone.contains("message 0"), "{alone}");
}

#[test]
fn identical_captures_feed_nothing() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(60);
    let mut stream = LogStream::new(s.capture(), SIZE, None);
    let before = shown(&mut stream);
    for _ in 0..3 {
        assert_eq!(stream.feed(s.capture(), SIZE, None), Fed::Stitched(0));
    }
    assert_eq!(shown(&mut stream), before);
}

#[test]
fn no_overlap_starts_over_without_a_base() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(10);
    let mut stream = LogStream::new(s.capture(), SIZE, None);
    assert!(stream.has_base());
    // More writes than the ring between two fetches: the overlap is gone.
    s.spin(60);
    s.set(12, "late message");
    s.frame();
    assert_eq!(
        stream.feed(s.capture(), SIZE, None),
        Fed::Reset(ResetCause::NoOverlap)
    );
    assert!(!stream.has_base());
    let screen = stream.screen();
    assert!(screen.partial());
    let text = screen.plain_lines().join("\n");
    assert!(text.contains("late message"), "{text}");
    assert!(!text.contains("Example Code"), "{text}");
}

#[test]
fn an_attach_repaint_gives_a_partial_stream_its_base() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(100);
    // Followed only from here on: no base.
    let mut stream = LogStream::new(s.capture(), SIZE, None);
    assert!(!stream.has_base());
    assert!(stream.screen().partial());
    s.spin(10);
    assert!(matches!(
        stream.feed(s.capture(), SIZE, None),
        Fed::Stitched(_)
    ));
    assert!(!stream.has_base(), "differential frames bring no base");
    // A terminal attaches: Claude Code repaints everything.
    s.repaint();
    s.spin(10);
    assert!(matches!(
        stream.feed(s.capture(), SIZE, None),
        Fed::Stitched(_)
    ));
    assert!(stream.has_base());
    assert!(!stream.screen().partial());
    assert_eq!(shown(&mut stream), trimmed(&s.reference()));
    // Long after the attach the base stays.
    for _ in 0..5 {
        s.spin(25);
        stream.feed(s.capture(), SIZE, None);
    }
    assert!(stream.has_base());
    assert_eq!(shown(&mut stream), trimmed(&s.reference()));
}

#[test]
fn a_new_size_or_process_starts_over() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(10);
    let mut stream = LogStream::new(s.capture(), SIZE, Some(7));
    assert_eq!(stream.size(), (ROWS, COLS));
    s.spin(5);
    // Unknown size or pid: no reason to start over.
    assert!(matches!(
        stream.feed(s.capture(), None, None),
        Fed::Stitched(_)
    ));
    // Another real size (e.g. attached from a larger terminal).
    s.spin(5);
    assert_eq!(
        stream.feed(s.capture(), Some((40, 120)), Some(7)),
        Fed::Reset(ResetCause::Size)
    );
    assert_eq!(stream.size(), (40, 120));
    assert!(stream.has_base(), "the capture still holds the start");
    // Another process: the session was stopped and woken.
    s.spin(5);
    assert_eq!(
        stream.feed(s.capture(), Some((40, 120)), Some(8)),
        Fed::Reset(ResetCause::Pid)
    );
    s.spin(5);
    assert!(matches!(
        stream.feed(s.capture(), Some((40, 120)), Some(8)),
        Fed::Stitched(_)
    ));
}

#[test]
fn a_reset_on_a_capture_with_a_clear_has_a_base() {
    let mut s = Session::start(ROWS, COLS, 40);
    s.spin(100);
    let mut stream = LogStream::new(s.capture(), SIZE, None);
    assert!(!stream.has_base());
    s.repaint();
    s.spin(100);
    s.repaint();
    s.spin(3);
    assert_eq!(
        stream.feed(s.capture(), SIZE, None),
        Fed::Reset(ResetCause::NoOverlap)
    );
    assert!(stream.has_base());
    assert_eq!(shown(&mut stream), trimmed(&s.reference()));
}

#[test]
fn garbage_and_empty_captures_never_fail() {
    let mut stream = LogStream::new(Vec::new(), None, None);
    assert!(stream.screen().is_empty());
    assert!(!stream.screen().partial(), "nothing to be partial about");
    let fed = stream.feed(b"\x1b[999999@\xff\xfe\x1b[".to_vec(), None, None);
    assert_eq!(fed, Fed::Reset(ResetCause::NoOverlap));
    let fed = stream.feed(Vec::new(), None, None);
    assert_eq!(fed, Fed::Reset(ResetCause::NoOverlap));
    stream.feed(b"\x1b[2J\x1b[Hhello".to_vec(), None, None);
    assert!(stream.has_base());
    assert_eq!(shown(&mut stream), ["hello"]);
}
