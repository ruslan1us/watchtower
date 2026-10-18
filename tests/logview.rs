//! Logs panel data: replaying `claude logs` streams (doc/0009) and fetching
//! them through a fake `claude` on `PATH` (doc/0015).

mod common;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use ratatui::style::{Color, Modifier};
use watchtower::cli::CliRunner;
use watchtower::config::Account;
use watchtower::logview::{
    self, DEFAULT_COLS, DEFAULT_ROWS, LogScreen, LogsError, MAX_COLS, MAX_INPUT_BYTES, MAX_ROWS,
};

const FIXTURES: [&str; 6] = [
    "done",
    "failed",
    "permission-prompt",
    "question",
    "resumed-idle",
    "working",
];

fn stream(name: &str) -> Vec<u8> {
    std::fs::read(common::fixture_path(&format!("logs/{name}.ansi"))).unwrap()
}

fn expected_screen(name: &str) -> Vec<String> {
    let text = common::fixture(&format!("logs/{name}.screen.txt"));
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches(' ').to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn render_fixture(name: &str) -> LogScreen {
    logview::render(&stream(name))
}

#[test]
fn every_fixture_has_a_screen_snapshot() {
    let dir = common::fixture_path("logs");
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.strip_suffix(".ansi").map(str::to_owned)
        })
        .collect();
    found.sort();
    assert_eq!(found, FIXTURES);
}

#[test]
fn fixtures_render_to_their_screen_snapshots() {
    for name in FIXTURES {
        let screen = render_fixture(name);
        assert_eq!(screen.size(), (DEFAULT_ROWS, DEFAULT_COLS), "{name}");
        let got = screen.plain_lines();
        let want = expected_screen(name);
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "{name}: row {}", i + 1);
        }
        assert_eq!(got.len(), want.len(), "{name}: row count");
        assert_eq!(screen.row_count(), want.len(), "{name}");
    }
}

#[test]
fn styled_lines_match_plain_text() {
    for name in FIXTURES {
        let screen = render_fixture(name);
        let lines = screen.lines();
        assert_eq!(lines.len(), screen.row_count());
        for (line, plain) in lines.iter().zip(screen.plain_lines()) {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            // The styled form may keep trailing cells with a background.
            assert!(text.starts_with(&plain), "{name}: {text:?} vs {plain:?}");
            assert!(text[plain.len()..].chars().all(|c| c == ' '), "{name}");
        }
    }
}

#[test]
fn colours_and_attributes_are_kept() {
    let screen = render_fixture("done");
    let spans: Vec<_> = screen.lines().into_iter().flat_map(|l| l.spans).collect();
    // The logo is drawn in Claude Code's orange.
    assert!(
        spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Rgb(215, 119, 87)))
    );
    // "Claude Code" in the header is bold.
    let title = spans
        .iter()
        .find(|s| s.content.contains("Claude Code"))
        .expect("header span");
    assert!(title.style.add_modifier.contains(Modifier::BOLD));
    // The echoed prompt row has a grey background that runs past its text.
    let prompt = screen
        .lines()
        .into_iter()
        .find(|l| {
            l.spans
                .iter()
                .any(|s| s.content.contains("reply with the single word OK"))
        })
        .expect("prompt row");
    assert!(
        prompt
            .spans
            .iter()
            .any(|s| s.style.bg == Some(Color::Rgb(55, 55, 55)))
    );
    // Default-coloured text carries no colour of its own.
    let ok = spans
        .iter()
        .find(|s| s.content.contains("OK") && !s.content.contains("reply"))
        .expect("reply span");
    assert_eq!(ok.style.bg, None);
}

#[test]
fn content_trims_blank_rows_at_top_and_bottom() {
    let screen = render_fixture("done");
    let range = screen.content_range();
    assert_eq!(range.start, 1, "row 1 of the done screen is blank");
    assert_eq!(range.end, screen.row_count());
    assert_eq!(screen.content_lines().len(), range.len());
    assert_eq!(screen.content_rows(), range.len());
    assert!(screen.content_width() <= usize::from(DEFAULT_COLS));
}

#[test]
fn tail_fits_and_keeps_the_conversation() {
    let screen = render_fixture("done");
    // Enough room: unchanged content.
    let all = screen.tail(200);
    assert_eq!(all, screen.content_lines());

    // A short panel: blank runs collapse first, so the reply and the footer
    // both fit.
    let tail = screen.tail_plain(20);
    assert!(tail.len() <= 20);
    assert!(tail.iter().any(|l| l.contains("OK")), "{tail:#?}");
    assert!(tail.iter().any(|l| l.contains("auto mode on")), "{tail:#?}");
    assert!(!tail.windows(2).any(|w| w[0].is_empty() && w[1].is_empty()));
    assert_eq!(screen.tail(20).len(), tail.len());

    // A tiny panel keeps the bottom rows.
    let tiny = screen.tail_plain(2);
    assert_eq!(tiny.len(), 2);
    assert!(tiny[1].contains("auto mode on"), "{tiny:#?}");
    assert!(screen.tail(0).is_empty());
}

#[test]
fn size_is_inferred_from_absolute_addressing() {
    let bytes = b"\x1b[2J\x1b[H\x1b[80;1Hbottom\x1b[1;250Hx";
    assert_eq!(logview::infer_size(bytes), (80, 250));
    let screen = logview::render(bytes);
    assert_eq!(screen.size(), (80, 250));
    let lines = screen.plain_lines();
    assert_eq!(lines.len(), 80);
    assert_eq!(lines[0], format!("{}x", " ".repeat(249)));
    assert_eq!(lines[79], "bottom");
    // CHA and VPA count too.
    assert_eq!(logview::infer_size(b"\x1b[90d\x1b[300G"), (90, 300));
    // Never smaller than the default.
    assert_eq!(
        logview::infer_size(b"\x1b[3;4H"),
        (DEFAULT_ROWS, DEFAULT_COLS)
    );
    // Capped.
    assert_eq!(
        logview::infer_size(b"\x1b[65535;65535H"),
        (MAX_ROWS, MAX_COLS)
    );
}

#[test]
fn empty_input_is_an_empty_screen() {
    let screen = logview::render(b"");
    assert!(screen.is_empty());
    assert_eq!(screen.row_count(), 0);
    assert!(screen.plain_lines().is_empty());
    assert!(screen.lines().is_empty());
    assert!(screen.content_lines().is_empty());
    assert!(screen.tail(10).is_empty());
    assert_eq!(screen.content_range(), 0..0);
    assert_eq!(screen.content_width(), 0);
}

#[test]
fn truncated_and_garbage_escapes_do_not_panic() {
    for bytes in [
        &b"\x1b"[..],
        b"\x1b[",
        b"\x1b[38;2;255",
        b"hello\x1b[12",
        b"\x1b]0;title",
        b"\x1bP garbage",
        b"\x1b[;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;m",
        b"\x1b[999999999999999999999999A\x1b[-5H",
    ] {
        let _ = logview::render(bytes);
    }
    let screen = logview::render(b"ok\x1b[38;2;255");
    assert_eq!(screen.plain_lines(), ["ok"]);
    // A fixture cut in the middle of a sequence still renders its start.
    let done = stream("done");
    let cut = logview::render(&done[..done.len() / 2]);
    assert!(cut.plain_lines().iter().any(|l| l.contains("Claude Code")));
}

#[test]
fn invalid_utf8_is_replaced() {
    let screen = logview::render(b"a\xff\xfeb \xe2\x9c\n");
    let lines = screen.plain_lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with('a'));
    assert!(lines[0].contains('b'));
}

#[test]
fn huge_input_replays_only_the_tail() {
    // An old screen, a lot of noise, then a fresh redraw near the end.
    let mut bytes = b"\x1b[2J\x1b[Hold screen".to_vec();
    bytes.extend(std::iter::repeat_n(b'.', MAX_INPUT_BYTES + 1024));
    bytes.extend_from_slice(b"\x1b[2J\x1b[Hnew screen");
    let screen = logview::render(&bytes);
    assert_eq!(screen.plain_lines(), ["new screen"]);

    // No clear in the window: still renders without panicking.
    let noise = vec![b'x'; MAX_INPUT_BYTES * 2];
    let screen = logview::render(&noise);
    assert!(!screen.is_empty());
}

// ---- fetch ---------------------------------------------------------------

/// Serializes the fake-executable tests (ETXTBSY, see tests/cli.rs).
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A temp dir holding a fake `claude` that records its argv to `argv.txt`
/// and runs `body`; the runner puts the dir first on `PATH`.
struct FakeClaude {
    dir: tempfile::TempDir,
}

impl FakeClaude {
    fn new(body: &str) -> FakeClaude {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        let argv = dir.path().join("argv.txt");
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{body}\n",
            argv.display()
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        FakeClaude { dir }
    }

    fn runner(&self) -> CliRunner {
        let path = format!("{}:/usr/bin:/bin", self.dir.path().display());
        CliRunner::new().with_path(path)
    }

    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("argv.txt"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn main_account() -> Account {
    Account::new("main", "claude", None, Path::new(common::HOME)).unwrap()
}

fn fixture_file(rel: &str) -> String {
    common::fixture_path(rel).display().to_string()
}

#[test]
fn fetch_returns_stdout_only() {
    let _g = lock();
    let fake = FakeClaude::new(&format!(
        "cat '{}' >&2\ncat '{}'",
        fixture_file("cli-output/logs-done.stderr"),
        fixture_file("logs/done.ansi"),
    ));
    let bytes = logview::fetch(&fake.runner(), &main_account(), "b2f8d064").unwrap();
    assert_eq!(fake.argv(), ["logs", "b2f8d064"]);
    assert_eq!(bytes, stream("done"));
    assert_eq!(
        logview::render(&bytes).plain_lines(),
        expected_screen("done")
    );
}

#[test]
fn fetch_uses_the_account_command_words() {
    let _g = lock();
    let fake = FakeClaude::new("printf 'x'");
    let account = Account::new("alt", "claude --flag", None, Path::new(common::HOME)).unwrap();
    let bytes = logview::fetch(&fake.runner(), &account, "d4a1f7c8").unwrap();
    assert_eq!(fake.argv(), ["--flag", "logs", "d4a1f7c8"]);
    assert_eq!(bytes, b"x");
}

#[test]
fn fetch_stopped_session_is_not_running() {
    let _g = lock();
    let fake = FakeClaude::new(&format!(
        "cat '{}' >&2\nexit 1",
        fixture_file("cli-output/logs-stopped.stderr"),
    ));
    let err = logview::fetch(&fake.runner(), &main_account(), "b2f8d064").unwrap_err();
    assert!(matches!(err, LogsError::NotRunning), "{err:?}");
}

#[test]
fn fetch_unknown_id_is_unknown_session() {
    let _g = lock();
    let fake = FakeClaude::new(&format!(
        "cat '{}' >&2\nexit 1",
        fixture_file("cli-output/logs-unknown.stderr"),
    ));
    let err = logview::fetch(&fake.runner(), &main_account(), "nosuchjob").unwrap_err();
    assert_eq!(fake.argv(), ["logs", "nosuchjob"]);
    assert!(matches!(err, LogsError::UnknownSession), "{err:?}");
}

#[test]
fn fetch_other_failures_keep_stderr() {
    let _g = lock();
    let fake = FakeClaude::new("echo '  something broke  ' >&2\nexit 3");
    let err = logview::fetch(&fake.runner(), &main_account(), "b2f8d064").unwrap_err();
    match err {
        LogsError::Failed { code, stderr } => {
            assert_eq!(code, Some(3));
            assert_eq!(stderr, "something broke");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn fetch_missing_program_is_a_cli_error() {
    let dir = tempfile::tempdir().unwrap();
    let runner = CliRunner::new().with_path(dir.path());
    let err = logview::fetch(&runner, &main_account(), "b2f8d064").unwrap_err();
    assert!(matches!(err, LogsError::Cli(_)), "{err:?}");
}
