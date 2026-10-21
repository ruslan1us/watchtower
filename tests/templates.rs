//! Prompt templates (doc/0036): loading `<config dir>/templates` from temp
//! folders, the reusable picker's keys and notes, `Ctrl+P` in the
//! new-session dialog with the insertion into the task field, and the
//! picker drawn in both styles at a narrow and a wide size.

mod common;

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{HOME, app_single, ch, ctrl, key, project_list, status_text, type_text};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::{App, Effect};
use watchtower::config::{UiStyle, UiTheme};
use watchtower::dialog::{DialogPrompt, DialogStatus, Field};
use watchtower::templates::{
    self, MAX_TEMPLATE_BYTES, PickerAction, SkipReason, Skipped, Template, TemplateList,
    TemplateLoad, TemplatePicker,
};

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

fn names(list: &TemplateList) -> Vec<&str> {
    list.templates.iter().map(|t| t.name.as_str()).collect()
}

fn reason(list: &TemplateList, file: &str) -> SkipReason {
    list.skipped
        .iter()
        .find(|s| s.file == file)
        .unwrap_or_else(|| panic!("{file} not skipped: {:?}", list.skipped))
        .reason
        .clone()
}

fn template(name: &str, text: &str) -> Template {
    Template {
        name: name.into(),
        text: text.into(),
    }
}

fn list_of(items: &[(&str, &str)]) -> TemplateList {
    TemplateList {
        templates: items.iter().map(|(n, t)| template(n, t)).collect(),
        ..TemplateList::default()
    }
}

// ---- loading ---------------------------------------------------------------

#[test]
fn the_folder_is_the_config_dirs_templates_folder() {
    assert_eq!(
        templates::dir(Path::new("/home/user/.config/watchtower")),
        PathBuf::from("/home/user/.config/watchtower/templates")
    );
}

#[test]
fn templates_are_md_files_sorted_by_name_ignoring_case() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for name in [
        "review.md",
        "Alpha.md",
        "beta.md",
        "alpha2.md",
        "Review-deep.md",
    ] {
        write(d, name, "text");
    }
    write(d, "notes.txt", "not a template");
    write(d, "draft.md.bak", "not a template");
    std::fs::create_dir(d.join("nested")).unwrap();
    write(&d.join("nested"), "inner.md", "not listed: no recursion");
    let list = templates::load(d);
    assert_eq!(
        names(&list),
        ["Alpha", "alpha2", "beta", "review", "Review-deep"]
    );
    assert!(list.skipped.is_empty(), "{:?}", list.skipped);
    assert_eq!(list.error, None);
    assert_eq!(list.skipped_note(), None);
}

#[test]
fn the_text_keeps_its_start_and_loses_trailing_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "review.md",
        "\n  Review the diff.\r\n\nCheck tests.  \n\n\t\n",
    );
    let list = templates::load(dir.path());
    let t = &list.templates[0];
    assert_eq!(t.text, "\n  Review the diff.\r\n\nCheck tests.");
    assert_eq!(t.preview(), "Review the diff.");
}

#[test]
fn a_missing_folder_means_no_templates_and_no_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("templates");
    let list = templates::load(&folder);
    assert_eq!(list, TemplateList::default());
    // Created empty and private, so the user can put files there.
    let meta = std::fs::metadata(&folder).expect("folder created");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    let load = TemplateLoad {
        id: 3,
        folder: dir.path().join("missing"),
    };
    assert_eq!(load.run(), TemplateList::default());
}

#[test]
fn a_missing_parent_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("no-config").join("templates");
    assert_eq!(templates::load(&folder), TemplateList::default());
    assert!(!dir.path().join("no-config").exists());
}

#[test]
fn a_folder_that_is_a_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "templates", "a file");
    let list = templates::load(&dir.path().join("templates"));
    assert!(list.templates.is_empty());
    let err = list.error.expect("an error");
    assert!(err.starts_with("cannot read "), "{err}");
}

#[test]
fn hidden_files_are_ignored_without_a_report() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".secret.md", "hidden");
    write(dir.path(), ".md", "no name");
    write(dir.path(), "shown.md", "shown");
    let list = templates::load(dir.path());
    assert_eq!(names(&list), ["shown"]);
    assert!(list.skipped.is_empty(), "{:?}", list.skipped);
}

#[test]
fn symlinks_and_other_non_regular_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(d, "real.md", "real");
    write(outside.path(), "elsewhere.md", "outside");
    std::os::unix::fs::symlink(d.join("real.md"), d.join("inside-link.md")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("elsewhere.md"), d.join("out-link.md")).unwrap();
    std::os::unix::fs::symlink(d.join("gone.md"), d.join("dangling.md")).unwrap();
    std::fs::create_dir(d.join("folder.md")).unwrap();
    let fifo = std::ffi::CString::new(d.join("pipe.md").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let list = templates::load(d);
    assert_eq!(names(&list), ["real"]);
    assert_eq!(reason(&list, "inside-link.md"), SkipReason::Symlink);
    assert_eq!(reason(&list, "out-link.md"), SkipReason::Symlink);
    assert_eq!(reason(&list, "dangling.md"), SkipReason::Symlink);
    assert_eq!(reason(&list, "folder.md"), SkipReason::NotRegular);
    assert_eq!(reason(&list, "pipe.md"), SkipReason::NotRegular);
    assert_eq!(list.skipped.len(), 5);
}

#[test]
fn a_symlinked_templates_folder_is_followed() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles");
    std::fs::create_dir(&real).unwrap();
    write(&real, "review.md", "Review it.");
    let link = dir.path().join("templates");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(names(&templates::load(&link)), ["review"]);
}

#[test]
fn large_empty_and_non_utf8_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let max = MAX_TEMPLATE_BYTES as usize;
    write(d, "at-limit.md", &"a".repeat(max));
    write(d, "over-limit.md", &"a".repeat(max + 1));
    write(d, "empty.md", "");
    write(d, "blank.md", "  \n\n\t \n");
    std::fs::write(d.join("latin1.md"), b"caf\xe9\n").unwrap();
    let bad_name = std::ffi::OsStr::from_bytes(b"bad-\xff.md");
    std::fs::write(d.join(bad_name), "valid text").unwrap();
    let list = templates::load(d);
    assert_eq!(names(&list), ["at-limit"]);
    assert_eq!(list.templates[0].text.len(), max);
    assert_eq!(reason(&list, "over-limit.md"), SkipReason::TooLarge);
    assert_eq!(reason(&list, "empty.md"), SkipReason::Empty);
    assert_eq!(reason(&list, "blank.md"), SkipReason::Empty);
    assert_eq!(reason(&list, "latin1.md"), SkipReason::NotUtf8);
    assert_eq!(reason(&list, "bad-\u{fffd}.md"), SkipReason::NotUtf8);
}

#[test]
fn the_skipped_note_gives_the_count_and_the_first_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b-empty.md", "");
    let list = templates::load(dir.path());
    assert_eq!(
        list.skipped_note().as_deref(),
        Some("skipped template b-empty.md: empty")
    );
    write(
        dir.path(),
        "a-big.md",
        &"x".repeat(MAX_TEMPLATE_BYTES as usize + 10),
    );
    write(dir.path(), "c-empty.md", "\n");
    let list = templates::load(dir.path());
    assert_eq!(
        list.skipped_note().as_deref(),
        Some("skipped 3 template files: a-big.md (over 64 KiB) and 2 more")
    );
}

// ---- the picker --------------------------------------------------------------

fn picker() -> TemplatePicker {
    TemplatePicker::new(
        7,
        PathBuf::from(format!("{HOME}/.config/watchtower/templates")),
        Path::new(HOME),
    )
}

fn loaded() -> TemplatePicker {
    let mut p = picker();
    p.set_list(list_of(&[
        ("fix-tests", "Fix the failing tests."),
        ("review", "Review the diff.\nBe strict."),
        ("Review-security", "Look for injection."),
        ("summary", "Summarise the change."),
    ]));
    p
}

fn type_into(p: &mut TemplatePicker, text: &str) {
    for c in text.chars() {
        assert_eq!(p.handle_key(ch(c)), PickerAction::None);
    }
}

fn shown(p: &TemplatePicker) -> Vec<&str> {
    p.filtered().iter().map(|t| t.name.as_str()).collect()
}

#[test]
fn the_picker_waits_for_its_load() {
    let mut p = picker();
    assert_eq!(p.load_id(), 7);
    assert!(p.is_loading());
    assert_eq!(p.shown_folder(), "~/.config/watchtower/templates");
    assert_eq!(p.empty_text().as_deref(), Some("loading templates…"));
    assert_eq!(p.handle_key(key(KeyCode::Enter)), PickerAction::None);
    // A filter typed while loading is kept.
    type_into(&mut p, "rev");
    p.set_list(loaded().list().unwrap().clone());
    assert!(!p.is_loading());
    assert_eq!(shown(&p), ["review", "Review-security"]);
}

#[test]
fn the_empty_state_names_the_folder() {
    let mut p = picker();
    p.set_list(TemplateList::default());
    assert_eq!(
        p.empty_text().as_deref(),
        Some("No templates yet: put .md files in ~/.config/watchtower/templates")
    );
    assert_eq!(p.selected(), None);
    assert_eq!(p.handle_key(key(KeyCode::Enter)), PickerAction::None);
    let mut p = picker();
    p.set_list(TemplateList {
        error: Some("cannot read /x: denied".into()),
        ..TemplateList::default()
    });
    assert_eq!(p.empty_text().as_deref(), Some("cannot read /x: denied"));
}

#[test]
fn typing_filters_by_name_ignoring_case() {
    let mut p = loaded();
    assert_eq!(p.empty_text(), None);
    assert_eq!(shown(&p).len(), 4);
    type_into(&mut p, "REV");
    assert_eq!(shown(&p), ["review", "Review-security"]);
    type_into(&mut p, " sec");
    assert_eq!(shown(&p), ["Review-security"]);
    p.handle_key(key(KeyCode::Backspace));
    p.handle_key(key(KeyCode::Backspace));
    p.handle_key(key(KeyCode::Backspace));
    assert_eq!(shown(&p), ["review", "Review-security"]);
    type_into(&mut p, "zzz");
    assert!(shown(&p).is_empty());
    assert_eq!(
        p.empty_text().as_deref(),
        Some("no template matches the filter")
    );
    assert_eq!(p.handle_key(ctrl('u')), PickerAction::None);
    assert!(p.filter().is_empty());
    assert_eq!(shown(&p).len(), 4);
}

#[test]
fn arrows_choose_and_enter_picks_the_text() {
    let mut p = loaded();
    assert_eq!(p.selected().unwrap().name, "fix-tests");
    p.handle_key(key(KeyCode::Up));
    assert_eq!(p.selected_index(), 0);
    p.handle_key(key(KeyCode::Down));
    assert_eq!(p.selected().unwrap().name, "review");
    p.handle_key(key(KeyCode::PageDown));
    assert_eq!(p.selected().unwrap().name, "summary", "clamped to the last");
    p.handle_key(key(KeyCode::Down));
    assert_eq!(p.selected_index(), 3);
    p.handle_key(key(KeyCode::PageUp));
    assert_eq!(p.selected_index(), 0);
    p.handle_key(key(KeyCode::Down));
    assert_eq!(
        p.handle_key(key(KeyCode::Enter)),
        PickerAction::Pick("Review the diff.\nBe strict.".into())
    );
    // Typing starts the choice over among the matches.
    type_into(&mut p, "s");
    assert_eq!(p.selected_index(), 0);
}

#[test]
fn esc_closes_and_ctrl_c_quits() {
    let mut p = loaded();
    assert_eq!(p.handle_key(key(KeyCode::Esc)), PickerAction::Close);
    assert_eq!(p.handle_key(ctrl('c')), PickerAction::Quit);
    // Ctrl+С under a Cyrillic layout is Ctrl+C too.
    assert_eq!(p.handle_key(ctrl('с')), PickerAction::Quit);
}

#[test]
fn cyrillic_text_is_typed_as_is() {
    let mut p = picker();
    p.set_list(list_of(&[("ревью", "Проверь."), ("review", "Review.")]));
    type_into(&mut p, "рев");
    assert_eq!(shown(&p), ["ревью"]);
}

#[test]
fn previews_are_the_first_non_blank_line() {
    assert_eq!(template("a", "first\nsecond").preview(), "first");
    assert_eq!(template("a", "\n\n   second  \nthird").preview(), "second");
}

// ---- the new-session dialog ----------------------------------------------------

fn open(app: &mut App) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    app.open_dialog(project_list(root.path(), &["api", "web"]));
    root
}

fn to_task(app: &mut App) {
    while app.dialog().unwrap().field != Field::Task {
        app.handle_key(key(KeyCode::Tab));
    }
}

/// `Ctrl+P` and the load it asks for, run against `list`.
fn ctrl_p(app: &mut App, list: TemplateList) {
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(load)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_eq!(load.folder, app.templates_dir());
    let picker = app.dialog().unwrap().templates.as_ref().expect("open");
    assert!(picker.is_loading());
    assert_eq!(picker.load_id(), load.id);
    assert!(app.templates_loaded(load.id, list).is_empty());
}

fn task(app: &App) -> String {
    app.dialog().unwrap().task.text()
}

fn reviews() -> TemplateList {
    list_of(&[
        ("review", "Review the diff.\nBe strict."),
        ("tests", "Add tests."),
    ])
}

#[test]
fn the_templates_folder_follows_the_config_dir() {
    let mut app = app_single();
    assert_eq!(
        app.templates_dir(),
        Path::new("/home/user/.config/watchtower/templates")
    );
    app.set_config_dir(Path::new("/xdg/watchtower"));
    assert_eq!(app.templates_dir(), Path::new("/xdg/watchtower/templates"));
}

#[test]
fn a_pick_fills_an_empty_task_field() {
    let mut app = app_single();
    let _root = open(&mut app);
    to_task(&mut app);
    ctrl_p(&mut app, reviews());
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let d = app.dialog().unwrap();
    assert!(d.templates.is_none(), "the picker closes");
    assert_eq!(d.task.text(), "Review the diff.\nBe strict.");
    assert_eq!(d.task.cursor(), (1, 10));
    assert_eq!(d.field, Field::Task);
}

#[test]
fn a_blank_task_field_is_replaced() {
    let mut app = app_single();
    let _root = open(&mut app);
    to_task(&mut app);
    type_text(&mut app, "  ");
    app.handle_key(key(KeyCode::Enter));
    ctrl_p(&mut app, reviews());
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(task(&app), "Add tests.");
}

#[test]
fn a_pick_goes_in_at_the_cursor() {
    let mut app = app_single();
    let _root = open(&mut app);
    to_task(&mut app);
    type_text(&mut app, "abc def");
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Left));
    }
    ctrl_p(&mut app, reviews());
    app.handle_key(key(KeyCode::Enter));
    let d = app.dialog().unwrap();
    assert_eq!(d.task.text(), "abc Review the diff.\nBe strict.def");
    assert_eq!(d.task.cursor(), (1, 10), "after the inserted text");
    // Typing goes on at the cursor.
    type_text(&mut app, "!");
    assert_eq!(task(&app), "abc Review the diff.\nBe strict.!def");
}

#[test]
fn ctrl_p_from_another_field_inserts_into_the_task() {
    let mut app = app_single();
    let _root = open(&mut app);
    assert_eq!(app.dialog().unwrap().field, Field::Project);
    ctrl_p(&mut app, reviews());
    // Typed text filters the picker, not the project list.
    type_text(&mut app, "tes");
    assert!(app.dialog().unwrap().filter.is_empty());
    app.handle_key(key(KeyCode::Enter));
    let d = app.dialog().unwrap();
    assert_eq!(d.task.text(), "Add tests.");
    assert_eq!(d.field, Field::Task);
}

#[test]
fn esc_closes_only_the_picker() {
    let mut app = app_single();
    let _root = open(&mut app);
    to_task(&mut app);
    type_text(&mut app, "keep me");
    ctrl_p(&mut app, reviews());
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    let d = app.dialog().expect("the dialog stays open");
    assert!(d.templates.is_none());
    assert_eq!(d.task.text(), "keep me");
    assert_eq!(d.prompt, None);
}

#[test]
fn ctrl_c_in_the_picker_asks_the_dialogs_question() {
    let mut app = app_single();
    let _root = open(&mut app);
    ctrl_p(&mut app, reviews());
    assert!(app.handle_key(ctrl('c')).is_empty());
    let d = app.dialog().unwrap();
    assert!(d.templates.is_none());
    assert_eq!(d.prompt, Some(DialogPrompt::Quit));
}

#[test]
fn ctrl_p_under_a_cyrillic_layout_opens_the_picker() {
    let mut app = app_single();
    let _root = open(&mut app);
    let effects = app.handle_key(KeyEvent::new(KeyCode::Char('з'), KeyModifiers::CONTROL));
    assert!(matches!(effects.as_slice(), [Effect::LoadTemplates(_)]));
}

#[test]
fn ctrl_p_does_nothing_while_starting() {
    let mut app = app_single();
    let _root = open(&mut app);
    to_task(&mut app);
    type_text(&mut app, "fix the flaky test");
    let effects = app.handle_key(ctrl('s'));
    assert!(matches!(effects.as_slice(), [Effect::Dispatch(_)]));
    assert!(matches!(
        app.dialog().unwrap().status,
        DialogStatus::Starting { .. }
    ));
    assert!(app.handle_key(ctrl('p')).is_empty());
    assert!(app.dialog().unwrap().templates.is_none());
}

#[test]
fn stale_loads_are_dropped() {
    let mut app = app_single();
    let _root = open(&mut app);
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(first)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    let first = first.id;
    app.handle_key(key(KeyCode::Esc));
    let effects = app.handle_key(ctrl('p'));
    let [Effect::LoadTemplates(second)] = effects.as_slice() else {
        panic!("{effects:?}")
    };
    assert_ne!(first, second.id);
    let second = second.id;
    app.templates_loaded(first, reviews());
    assert!(
        app.dialog()
            .unwrap()
            .templates
            .as_ref()
            .unwrap()
            .is_loading()
    );
    app.templates_loaded(second, reviews());
    assert!(
        !app.dialog()
            .unwrap()
            .templates
            .as_ref()
            .unwrap()
            .is_loading()
    );
    // A load for a closed picker changes nothing.
    app.handle_key(key(KeyCode::Esc));
    app.templates_loaded(second + 1, reviews());
    assert!(app.dialog().unwrap().templates.is_none());
}

#[test]
fn skipped_files_are_reported_in_the_status_line() {
    let mut app = app_single();
    let _root = open(&mut app);
    let mut list = reviews();
    list.skipped = vec![
        Skipped {
            file: "big.md".into(),
            reason: SkipReason::TooLarge,
        },
        Skipped {
            file: "empty.md".into(),
            reason: SkipReason::Empty,
        },
    ];
    ctrl_p(&mut app, list);
    let status = status_text(&app);
    assert!(
        status.contains("skipped 2 template files: big.md (over 64 KiB) and 1 more"),
        "{status}"
    );
}

// ---- rendering -----------------------------------------------------------------

fn render(app: &mut App, w: u16, h: u16) -> (Vec<String>, (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer();
    let lines = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect();
    (lines, (pos.x, pos.y))
}

fn find(lines: &[String], needle: &str) -> (usize, usize) {
    let y = lines
        .iter()
        .rposition(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?} in\n{}", lines.join("\n")));
    let x = unicode_width::UnicodeWidthStr::width(&lines[y][..lines[y].rfind(needle).unwrap()]);
    (x, y)
}

fn picker_app(list: TemplateList, style: UiStyle) -> (App, tempfile::TempDir) {
    let mut app = app_single();
    app.style = style;
    app.set_theme(UiTheme::Warm);
    let root = open(&mut app);
    ctrl_p(&mut app, list);
    (app, root)
}

const SIZES: [(u16, u16); 2] = [(60, 20), (120, 40)];

#[test]
fn the_picker_lists_names_and_previews_in_both_styles() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        for (w, h) in SIZES {
            let what = format!("{style:?} {w}x{h}");
            let (mut app, _root) = picker_app(reviews(), style);
            let (lines, cursor) = render(&mut app, w, h);
            let all = lines.join("\n");
            let rounded = style == UiStyle::Dashboard && w >= 80;
            let corner = if rounded {
                "╭ Templates "
            } else {
                "┌ Templates "
            };
            assert!(all.contains(corner), "{what}\n{all}");
            let (x, y) = find(&lines, "▶ review");
            assert!(lines[y].contains("Review the diff."), "{what}: preview");
            assert!(!all.contains("Be strict."), "{what}: first line only");
            assert!(lines[y + 1].contains("  tests"), "{what}");
            assert!(all.contains("Esc close"), "{what}");
            // The cursor is in the filter line, after `› `.
            let (fx, fy) = find(&lines, "› type to filter");
            assert_eq!(cursor, (fx as u16 + 2, fy as u16), "{what}");
            assert!(fy < y, "{what}: the filter above the list");
            assert!(x > 0);
        }
    }
}

#[test]
fn the_picker_shows_the_empty_state_with_the_folder() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        for (w, h) in SIZES {
            let what = format!("{style:?} {w}x{h}");
            let (mut app, _root) = picker_app(TemplateList::default(), style);
            let (lines, _) = render(&mut app, w, h);
            // Wrapped at the narrow size: join the rows' text.
            let words: Vec<&str> = lines
                .iter()
                .flat_map(|l| l.split(|c: char| c.is_whitespace() || c == '│'))
                .filter(|w| !w.is_empty())
                .collect();
            let text = words.join(" ");
            assert!(
                text.contains("No templates yet: put .md files in ~/.config/watchtower/templates"),
                "{what}\n{}",
                lines.join("\n")
            );
        }
    }
}

#[test]
fn the_dialog_shows_the_key() {
    let mut app = app_single();
    let _root = open(&mut app);
    let (lines, _) = render(&mut app, 120, 40);
    let all = lines.join("\n");
    assert!(all.contains("Ctrl+P template"), "{all}");
    // The help lists it among the dialog's keys.
    let mut app = app_single();
    app.handle_key(ch('?'));
    let (lines, _) = render(&mut app, 100, 30);
    assert!(
        lines.join("\n").contains("insert a template"),
        "{}",
        lines.join("\n")
    );
}

#[test]
fn a_long_list_scrolls_with_the_choice() {
    let items: Vec<(String, String)> = (0..30)
        .map(|i| (format!("t{i:02}"), format!("text {i}")))
        .collect();
    let refs: Vec<(&str, &str)> = items
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let (mut app, _root) = picker_app(list_of(&refs), UiStyle::Minimal);
    for _ in 0..25 {
        app.handle_key(key(KeyCode::Down));
    }
    let (lines, _) = render(&mut app, 60, 20);
    let all = lines.join("\n");
    assert!(all.contains("▶ t25"), "{all}");
    assert!(!all.contains(" t00 "), "{all}");
    assert!(all.contains("30 of 30"), "{all}");
}
