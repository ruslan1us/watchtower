//! The windows of the new-session dialog (doc/0008), the resume window
//! (doc/0019), the quick reply window (doc/0040) and the template picker
//! over the dialog and the reply window (doc/0036), drawn at 80 and 104
//! columns: one column of padding inside the border on each side, the
//! picker inset so its border never touches its host's, `Esc` named once,
//! and the task and reply texts wrapped at words with the cursor after the
//! last typed character.

mod common;

use std::time::Instant;

use common::{app_single, app_two_with, ch, ctrl, key, project_list, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::app::{App, Effect};
use watchtower::config::UiStyle;
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::templates::{Template, TemplateList};

const SIZES: [(u16, u16); 2] = [(80, 24), (104, 32)];

const TEXT: &str = "Найди, почему падают интеграционные тесты платежей после обновления \
                    библиотеки, и исправь их; then update the changelog and open a pull request \
                    with a short summary of the root cause конец";

/// One drawn frame, a symbol per cell, and the cursor.
struct Screen {
    cells: Vec<Vec<String>>,
    cursor: (usize, usize),
}

impl Screen {
    fn at(&self, x: usize, y: usize) -> &str {
        &self.cells[y][x]
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The box whose top border holds `title`: left, top, right, bottom.
    fn window(&self, title: &str) -> (usize, usize, usize, usize) {
        for (y, row) in self.cells.iter().enumerate() {
            for x0 in 0..row.len() {
                if row[x0] != "╭" && row[x0] != "┌" {
                    continue;
                }
                let Some(x1) = (x0 + 1..row.len()).find(|&x| row[x] == "╮" || row[x] == "┐")
                else {
                    continue;
                };
                if !row[x0..x1].concat().contains(title) {
                    continue;
                }
                let y1 = (y + 1..self.cells.len())
                    .find(|&yy| ["╰", "└"].contains(&self.cells[yy][x0].as_str()))
                    .unwrap_or_else(|| panic!("{title}: no bottom\n{}", self.text()));
                return (x0, y, x1, y1);
            }
        }
        panic!("no window {title:?} in\n{}", self.text())
    }

    /// Asserts the window titled `title` keeps its sides and a blank
    /// column inside each, on every row.
    fn assert_padded(&self, title: &str, what: &str) {
        let (x0, y0, x1, y1) = self.window(title);
        for y in y0 + 1..y1 {
            let row = self.cells[y].concat();
            assert_eq!(self.at(x0, y), "│", "{what}: left border\n{row}");
            assert_eq!(self.at(x1, y), "│", "{what}: right border\n{row}");
            assert_eq!(self.at(x0 + 1, y), " ", "{what}: left padding\n{row}");
            assert_eq!(self.at(x1 - 1, y), " ", "{what}: right padding\n{row}");
        }
    }

    /// The rows of the text area inside `title` (after `┊`), trimmed.
    fn text_rows(&self, title: &str) -> Vec<String> {
        let (x0, y0, x1, y1) = self.window(title);
        (y0 + 1..y1)
            .filter_map(|y| {
                let inner = self.cells[y][x0 + 1..x1].concat();
                let (_, rest) = inner.split_once('┊')?;
                Some(rest.trim().to_owned())
            })
            .filter(|r| !r.is_empty())
            .collect()
    }
}

/// `app` drawn at `w` × `h` in both styles (doc/0020).
fn screens(app: &mut App, w: u16, h: u16) -> Vec<Screen> {
    [UiStyle::Dashboard, UiStyle::Minimal]
        .into_iter()
        .map(|style| {
            app.style = style;
            draw(app, w, h)
        })
        .collect()
}

fn draw(app: &mut App, w: u16, h: u16) -> Screen {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer();
    let cells = (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
        .collect();
    Screen {
        cells,
        cursor: (usize::from(pos.x), usize::from(pos.y)),
    }
}

const PROJECTS: [&str; 4] = [
    "api",
    "billing-platform-frontend-with-a-very-long-folder-name-that-fills-the-row",
    "проект-оплаты-и-счетов",
    "web",
];

fn dialog() -> (App, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_single();
    app.open_dialog(project_list(root.path(), &PROJECTS));
    (app, root)
}

fn templates() -> TemplateList {
    TemplateList {
        templates: vec![
            Template {
                name: "fix-tests".into(),
                text: "Fix the failing tests in the payments module, then run the suite twice."
                    .into(),
            },
            Template {
                name: "review-the-pull-request-with-a-very-strict-checklist".into(),
                text: "Review the diff against main.".into(),
            },
        ],
        ..TemplateList::default()
    }
}

/// `Ctrl+P` and the templates loaded into the picker it opened.
fn open_picker(app: &mut App) {
    let effects = app.handle_key(ctrl('p'));
    let id = effects
        .iter()
        .find_map(|e| match e {
            Effect::LoadTemplates(load) => Some(load.id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no template load in {effects:?}"));
    app.templates_loaded(id, templates());
}

/// A live, idle background row `ready-done` owned by `main`, with `t`
/// pressed on it.
fn reply() -> App {
    let text = r#"[{ "pid": 4342, "status": "idle", "id": "00000001",
        "cwd": "/home/user/workspace/demo-app", "kind": "background",
        "startedAt": 1767232822505, "sessionId": "00000001-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "name": "ready-done", "state": "done" }]"#;
    let listing = parse_listing(text).unwrap();
    let mut sc = Sidecar::default();
    for s in &listing.sessions {
        if let Some(sid) = &s.session_id {
            sc.record_owner(sid, "main", AccountSource::Started);
        }
    }
    let mut app = app_two_with(sc);
    for account in ["main", "alt"] {
        app.apply_poll(PollUpdate {
            account: account.into(),
            cycle: 0,
            result: Ok(parse_listing(text).unwrap()),
        });
    }
    common::select_name(&mut app, "ready-done");
    app.handle_key(ch('t'));
    assert!(
        app.reply_window().is_some(),
        "{}",
        common::status_text(&app)
    );
    app
}

#[test]
fn the_dialog_keeps_a_padding_column_on_each_side() {
    let (mut app, _root) = dialog();
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, TEXT);
    for (w, h) in SIZES {
        for s in screens(&mut app, w, h) {
            s.assert_padded("New session", &format!("dialog {w}x{h}"));
        }
    }
}

#[test]
fn the_resume_window_keeps_a_padding_column_and_names_esc_once() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app_single();
    let effects = app.handle_key(ch('R'));
    let [Effect::ScanProjects(scan)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    app.projects_found(scan.id, project_list(root.path(), &PROJECTS));
    for (w, h) in SIZES {
        for s in screens(&mut app, w, h) {
            s.assert_padded("Resume a past conversation", &format!("resume {w}x{h}"));
            assert_eq!(s.text().matches("Esc cancel").count(), 1, "{}", s.text());
        }
    }
}

#[test]
fn the_reply_window_keeps_a_padding_column_and_names_esc_once() {
    let mut app = reply();
    type_text(&mut app, TEXT);
    for (w, h) in SIZES {
        for s in screens(&mut app, w, h) {
            s.assert_padded("Reply to ready-done", &format!("reply {w}x{h}"));
            assert_eq!(s.text().matches("Esc cancel").count(), 1, "{}", s.text());
        }
    }
}

#[test]
fn the_picker_is_inset_into_its_host() {
    let (mut dialog, _root) = dialog();
    open_picker(&mut dialog);
    let mut reply = reply();
    open_picker(&mut reply);
    for (app, host) in [
        (&mut dialog, "New session"),
        (&mut reply, "Reply to ready-done"),
    ] {
        for (w, h) in SIZES {
            let what = format!("{host} {w}x{h}");
            for s in screens(app, w, h) {
                s.assert_padded("Templates", &what);
                s.assert_padded(host, &what);
                let (hx0, hy0, hx1, hy1) = s.window(host);
                let (px0, py0, px1, py1) = s.window("Templates");
                assert!(px0 >= hx0 + 2 && px1 + 2 <= hx1, "{what}\n{}", s.text());
                assert!(py0 > hy0 && py1 < hy1, "{what}\n{}", s.text());
                assert_eq!(s.text().matches("Esc close").count(), 1, "{}", s.text());
            }
        }
    }
}

#[test]
fn task_and_reply_texts_wrap_at_words_with_the_cursor_after_the_text() {
    let words: Vec<&str> = TEXT.split(' ').collect();
    let (mut dialog, _root) = dialog();
    dialog.handle_key(key(KeyCode::Tab));
    type_text(&mut dialog, TEXT);
    let mut reply = reply();
    type_text(&mut reply, TEXT);
    for (app, title) in [
        (&mut dialog, "New session"),
        (&mut reply, "Reply to ready-done"),
    ] {
        for (w, h) in SIZES {
            for s in screens(app, w, h) {
                let rows = s.text_rows(title);
                let shown: Vec<&str> = rows.iter().flat_map(|r| r.split(' ')).collect();
                assert_eq!(shown, words, "{title} {w}x{h}: {rows:#?}");
                let (x, y) = s.cursor;
                assert_eq!(s.at(x - 1, y), "ц", "{title} {w}x{h}\n{}", s.text());
                assert_eq!(s.at(x, y), " ", "{title} {w}x{h}");
            }
        }
    }
}
