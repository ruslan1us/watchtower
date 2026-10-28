//! Slash-command suggestions (doc/0042): the frontmatter reader, the
//! personal, project and plugin scans from temp folders (plugins listed by
//! a fake `claude`), precedence and plugin names, the menu's keys in the
//! new-session dialog and the quick reply window, the app's loads and
//! cache, and the menu drawn in both styles.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use common::{app_single, ch, key, project_list};
use fake_claude::{Fake, lock};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use watchtower::app::{App, Effect};
use watchtower::cli::CliRunner;
use watchtower::commands::{
    self, CommandList, CommandLoad, Commands, Kind, MAX_HEAD_BYTES, MenuView, Origin, SlashCommand,
    SlashMenu, matches, parse_frontmatter, parse_head, parse_plugin_list, read_head, trigger,
};
use watchtower::config::{Account, UiStyle, UiTheme};
use watchtower::dialog::{Field, TextArea};

// ---- helpers ---------------------------------------------------------------------

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str, text: &str) {
    write(&dir.join("skills").join(name).join("SKILL.md"), text);
}

fn names(list: &[SlashCommand]) -> Vec<&str> {
    list.iter().map(|c| c.name.as_str()).collect()
}

fn by_name<'a>(list: &'a [SlashCommand], name: &str) -> &'a SlashCommand {
    list.iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", names(list)))
}

fn cmd(name: &str, hint: Option<&str>, origin: Origin) -> SlashCommand {
    SlashCommand {
        name: name.into(),
        description: format!("about {name}"),
        argument_hint: hint.map(str::to_owned),
        origin,
        kind: Kind::Skill,
    }
}

fn list_of(items: Vec<SlashCommand>) -> CommandList {
    commands::merge(items, Vec::new(), Vec::new())
}

fn text_at_end(s: &str) -> TextArea {
    TextArea::new(s)
}

// ---- the frontmatter ---------------------------------------------------------------

#[test]
fn frontmatter_fields_quotes_and_blocks_are_read() {
    let fm = parse_frontmatter(&[
        "name: fix-bug",
        "description: \"Fix a bug: from a \\\"ticket\\\"\"",
        "argument-hint: [ticket-link]",
        "allowed-tools:",
        "  - Read",
        "  - Grep",
        "metadata:",
        "  owner: someone",
        "disable-model-invocation: true",
    ]);
    assert!(!fm.malformed);
    assert_eq!(fm.name.as_deref(), Some("fix-bug"));
    assert_eq!(
        fm.description.as_deref(),
        Some("Fix a bug: from a \"ticket\"")
    );
    // A flow sequence shows as written.
    assert_eq!(fm.argument_hint.as_deref(), Some("[ticket-link]"));
    assert_eq!(fm.user_invocable, None);

    let fm = parse_frontmatter(&[
        "description: >",
        "  Folded over",
        "  two lines",
        "argument-hint: '[file] [it''s format]'",
        "user-invocable: false # hidden",
    ]);
    assert_eq!(fm.description.as_deref(), Some("Folded over two lines"));
    assert_eq!(fm.argument_hint.as_deref(), Some("[file] [it's format]"));
    assert_eq!(fm.user_invocable, Some(false));

    // A plain value over several lines, and a comment.
    let fm = parse_frontmatter(&["# a comment", "description: first", "  and second # note"]);
    assert_eq!(fm.description.as_deref(), Some("first and second"));

    // Only an unquoted boolean counts.
    let fm = parse_frontmatter(&["user-invocable: \"false\""]);
    assert_eq!(fm.user_invocable, None);
}

#[test]
fn malformed_frontmatter_has_empty_fields() {
    for lines in [
        vec!["description: ok", "this is not yaml"],
        vec!["description: one", "description: two"],
        vec!["description: \"never closed"],
    ] {
        let fm = parse_frontmatter(&lines);
        // A quote that does not close only loses that field.
        if lines[0].contains("never closed") {
            assert_eq!(fm.description, None, "{lines:?}");
            continue;
        }
        assert!(fm.malformed, "{lines:?}");
        assert_eq!(fm.description, None, "{lines:?}");
    }
}

#[test]
fn the_head_falls_back_to_the_first_line_and_needs_a_closed_frontmatter() {
    let head = parse_head("---\nname: x\n---\n\n# Deploy the app\nmore").unwrap();
    assert_eq!(head.first_line.as_deref(), Some("# Deploy the app"));
    let head = parse_head("\n\nJust a prompt file\n").unwrap();
    assert!(head.frontmatter.is_none());
    assert_eq!(head.first_line.as_deref(), Some("Just a prompt file"));
    // With a description the body is not looked at.
    let head = parse_head("---\ndescription: d\n---\nbody").unwrap();
    assert_eq!(head.first_line, None);
    // The opening `---` must be the first line; one that does not close is
    // unreadable.
    assert!(parse_head("---\nname: x\nno end").is_none());
    let head = parse_head("text\n---\nname: x\n---").unwrap();
    assert!(head.frontmatter.is_none());
}

#[test]
fn files_are_read_up_to_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    // A frontmatter that does not close within the cap: skipped.
    let long = format!(
        "---\ndescription: d\nfiller: {}\n---\n",
        "x".repeat(MAX_HEAD_BYTES)
    );
    write(&dir.path().join("long.md"), &long);
    assert!(read_head(&dir.path().join("long.md")).is_none());
    // A big body does not matter: reading stops after the frontmatter.
    let big = format!("---\ndescription: d\n---\n{}", "y\n".repeat(100_000));
    write(&dir.path().join("big.md"), &big);
    let head = read_head(&dir.path().join("big.md")).unwrap();
    assert_eq!(head.frontmatter.unwrap().description.as_deref(), Some("d"));
    // Not UTF-8, a folder, a missing file: skipped.
    std::fs::write(dir.path().join("bin.md"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
    assert!(read_head(&dir.path().join("bin.md")).is_none());
    assert!(read_head(dir.path()).is_none());
    assert!(read_head(&dir.path().join("missing.md")).is_none());
}

// ---- the scans ------------------------------------------------------------------------

#[test]
fn personal_skills_and_commands_follow_the_naming_rules() {
    let config = tempfile::tempdir().unwrap();
    let c = config.path();
    skill(
        c,
        "deploy-staging",
        "---\nname: deploy\ndescription: Ship it\n---\n",
    );
    skill(
        c,
        "fix-bug",
        "---\nargument-hint: \"[ticket]\"\n---\n# Fix a bug\n",
    );
    skill(
        c,
        "hidden",
        "---\nuser-invocable: false\ndescription: x\n---\n",
    );
    skill(
        c,
        "broken",
        "---\ndescription: d\nnot yaml here\n---\nThe body line\n",
    );
    skill(c, ".dotted", "---\ndescription: x\n---\n");
    skill(c, "synced", "---\ndescription: x\n---\n");
    write(
        &c.join("skills/synced/other/SKILL.md"),
        "---\ndescription: x\n---\n",
    );
    write(&c.join("skills/no-skill-file/README.md"), "nothing");
    write(&c.join("commands/review.md"), "Review the diff\n");
    write(
        &c.join("commands/frontend/component.md"),
        "---\nname: ignored\ndescription: A component\n---\n",
    );
    write(&c.join("commands/a/b/c/deep.md"), "deep");
    write(&c.join("commands/a/b/c/d/too-deep.md"), "too deep");
    write(&c.join("commands/notes.txt"), "not a command");
    // A skill folder may be a link to a folder elsewhere.
    let elsewhere = tempfile::tempdir().unwrap();
    write(
        &elsewhere.path().join("linked/SKILL.md"),
        "---\ndescription: Linked\n---\n",
    );
    std::os::unix::fs::symlink(elsewhere.path().join("linked"), c.join("skills/linked")).unwrap();

    let found = commands::personal(c);
    let mut got = names(&found);
    got.sort();
    assert_eq!(
        got,
        [
            "a:b:c:deep",
            "broken",
            "deploy",
            "fix-bug",
            "frontend:component",
            "linked",
            "review"
        ]
    );
    let fix = by_name(&found, "fix-bug");
    assert_eq!(fix.description, "Fix a bug");
    assert_eq!(fix.argument_hint.as_deref(), Some("[ticket]"));
    assert_eq!(fix.kind, Kind::Skill);
    assert_eq!(fix.origin, Origin::Personal);
    // Malformed YAML: empty metadata, the body's first line.
    assert_eq!(by_name(&found, "broken").description, "The body line");
    assert_eq!(by_name(&found, "review").kind, Kind::Command);
    assert_eq!(
        by_name(&found, "frontend:component").description,
        "A component"
    );
}

#[test]
fn project_entries_come_from_the_folder_and_its_parents_up_to_the_repository() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let sub = repo.join("packages/web");
    skill(
        &repo.join(".claude"),
        "shared",
        "---\ndescription: root one\n---\n",
    );
    skill(
        &sub.join(".claude"),
        "shared",
        "---\ndescription: nested one\n---\n",
    );
    skill(
        &sub.join(".claude"),
        "web-only",
        "---\ndescription: w\n---\n",
    );
    write(&repo.join(".claude/commands/release.md"), "Release\n");
    // Above the repository: not read.
    skill(&root.path().join(".claude"), "outside", "x\n");
    let config = tempfile::tempdir().unwrap();

    assert_eq!(
        commands::project_folders(&sub),
        [repo.clone(), repo.join("packages"), sub.clone()]
    );
    let found = commands::project(&sub, config.path());
    let list = commands::merge(Vec::new(), found, Vec::new());
    assert_eq!(names(&list.commands), ["release", "shared", "web-only"]);
    // The repository root's wins.
    assert_eq!(by_name(&list.commands, "shared").description, "root one");
    assert!(list.commands.iter().all(|c| c.origin == Origin::Project));

    // Outside a repository, only the folder itself.
    let lone = root.path().join("lone");
    std::fs::create_dir_all(&lone).unwrap();
    assert_eq!(commands::project_folders(&lone), vec![lone]);
}

#[test]
fn a_project_in_the_home_folder_does_not_count_the_personal_folder_twice() {
    let home = tempfile::tempdir().unwrap();
    skill(&home.path().join(".claude"), "mine", "x\n");
    let found = commands::project(home.path(), &home.path().join(".claude"));
    assert!(found.is_empty(), "{:?}", names(&found));
}

#[test]
fn precedence_skill_over_command_then_personal_over_project() {
    let mk = |name: &str, kind: Kind, origin: Origin, d: &str| SlashCommand {
        name: name.into(),
        description: d.into(),
        argument_hint: None,
        origin,
        kind,
    };
    let personal = vec![
        mk("deploy", Kind::Skill, Origin::Personal, "personal skill"),
        mk(
            "review",
            Kind::Command,
            Origin::Personal,
            "personal command",
        ),
    ];
    let project = vec![
        mk("deploy", Kind::Skill, Origin::Project, "project skill"),
        mk("review", Kind::Skill, Origin::Project, "project skill"),
        mk("Alpha", Kind::Command, Origin::Project, "a"),
    ];
    let plugins = vec![mk(
        "review-kit:deploy",
        Kind::Skill,
        Origin::Plugin("review-kit".into()),
        "plugin",
    )];
    let list = commands::merge(personal, project, plugins);
    assert_eq!(
        names(&list.commands),
        ["Alpha", "deploy", "review", "review-kit:deploy"]
    );
    assert_eq!(
        by_name(&list.commands, "deploy").description,
        "personal skill"
    );
    assert_eq!(
        by_name(&list.commands, "review").description,
        "project skill"
    );
}

/// A plugin folder with the default layout and a manifest naming it.
fn plugin_dir(root: &Path, manifest: Option<&str>) -> PathBuf {
    let dir = root.join("cache/example-market/review-kit/1.0.0");
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(m) = manifest {
        write(&dir.join(".claude-plugin/plugin.json"), m);
    }
    dir
}

#[test]
fn plugins_are_namespaced_by_the_manifest_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        tmp.path(),
        Some(r#"{ "name": "rk", "skills": ["./extra/", "./one"] }"#),
    );
    skill(
        &dir,
        "code-review",
        "---\nname: fancy\ndescription: Review\n---\n",
    );
    skill(&dir, "quiet", "---\nuser-invocable: false\n---\n");
    write(
        &dir.join("commands/db/migrate.md"),
        "---\nargument-hint: <n>\n---\nMigrate\n",
    );
    write(&dir.join("commands/about.md"), "About it\n");
    write(
        &dir.join("extra/more/SKILL.md"),
        "---\ndescription: More\n---\n",
    );
    write(&dir.join("one/SKILL.md"), "---\ndescription: Single\n---\n");
    // A link that leaves the plugin's folder is not followed.
    let outside = tempfile::tempdir().unwrap();
    write(
        &outside.path().join("SKILL.md"),
        "---\ndescription: out\n---\n",
    );
    std::os::unix::fs::symlink(outside.path(), dir.join("skills/escape")).unwrap();

    let found = commands::plugin(&dir, "review-kit");
    let mut got = names(&found);
    got.sort();
    assert_eq!(
        got,
        ["rk:about", "rk:db:migrate", "rk:fancy", "rk:more", "rk:one"]
    );
    assert_eq!(
        by_name(&found, "rk:db:migrate").argument_hint.as_deref(),
        Some("<n>")
    );
    assert!(
        found
            .iter()
            .all(|c| c.origin == Origin::Plugin("rk".into()))
    );
    assert_eq!(Origin::Plugin("rk".into()).tag(), "plugin rk");

    // Without a manifest the listed name is the namespace.
    let tmp = tempfile::tempdir().unwrap();
    let dir = plugin_dir(tmp.path(), None);
    skill(&dir, "code-review", "x\n");
    assert_eq!(
        names(&commands::plugin(&dir, "review-kit")),
        ["review-kit:code-review"]
    );
}

#[test]
fn the_manifest_commands_key_replaces_the_default_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        tmp.path(),
        Some(
            r#"{ "name": "rk", "commands": {
                "status": { "source": "./cmds/status.md", "argumentHint": "[env]" },
                "about": { "content": "Explain this plugin.", "description": "Describe it" },
                "bad": { "source": "../outside.md" }
            } }"#,
        ),
    );
    write(&dir.join("commands/ignored.md"), "x\n");
    write(
        &dir.join("cmds/status.md"),
        "---\ndescription: Status\n---\n",
    );
    let found = commands::plugin(&dir, "review-kit");
    let mut got = names(&found);
    got.sort();
    assert_eq!(got, ["rk:about", "rk:status"]);
    assert_eq!(by_name(&found, "rk:about").description, "Describe it");
    let status = by_name(&found, "rk:status");
    assert_eq!(status.description, "Status");
    assert_eq!(status.argument_hint.as_deref(), Some("[env]"));

    // A list of paths: files and folders.
    let tmp = tempfile::tempdir().unwrap();
    let dir = plugin_dir(
        tmp.path(),
        Some(r#"{ "name": "rk", "commands": ["./cmds/one.md", "./more"] }"#),
    );
    write(&dir.join("cmds/one.md"), "One\n");
    write(&dir.join("more/two.md"), "Two\n");
    write(&dir.join("commands/ignored.md"), "x\n");
    let mut got: Vec<String> = commands::plugin(&dir, "x")
        .into_iter()
        .map(|c| c.name)
        .collect();
    got.sort();
    assert_eq!(got, ["rk:one", "rk:two"]);
}

#[test]
fn a_root_skill_counts_only_without_a_skills_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = plugin_dir(tmp.path(), Some(r#"{ "name": "rk" }"#));
    write(
        &dir.join("SKILL.md"),
        "---\nname: solo\ndescription: S\n---\n",
    );
    assert_eq!(names(&commands::plugin(&dir, "x")), ["rk:solo"]);
    skill(&dir, "other", "x\n");
    assert_eq!(names(&commands::plugin(&dir, "x")), ["rk:other"]);
}

#[test]
fn the_plugin_list_keeps_enabled_plugins_of_this_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let c = tmp.path().join("c");
    for d in [&a, &b, &c] {
        std::fs::create_dir_all(d).unwrap();
    }
    let here = tmp.path().join("project");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&here).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let json = format!(
        r#"[
          {{"id": "review-kit@example-market", "version": "1.0.0", "scope": "user",
            "enabled": true, "installPath": "{a}", "installedAt": "x", "lastUpdated": "y",
            "projectEnabled": false, "unknown": 1}},
          {{"id": "off@example-market", "scope": "user", "enabled": false, "installPath": "{b}"}},
          {{"id": "theirs@example-market", "scope": "project", "enabled": true,
            "installPath": "{b}", "projectPath": "{other}"}},
          {{"id": "ours@example-market", "scope": "local", "enabled": true,
            "installPath": "{b}", "projectPath": "{here}"}},
          {{"id": "in-place@example-market", "scope": "user", "enabled": true,
            "installPath": "/no/such/folder", "readFromFolder": "{c}"}},
          {{"id": "gone@example-market", "scope": "user", "enabled": true,
            "installPath": "/no/such/folder"}},
          {{"id": "relative@example-market", "scope": "user", "enabled": true,
            "installPath": "relative/path"}},
          {{"id": "dup@example-market", "scope": "user", "enabled": true, "installPath": "{a}"}},
          {{"scope": "user", "enabled": true, "installPath": "{a}"}},
          "not an object"
        ]"#,
        a = a.display(),
        b = b.display(),
        c = c.display(),
        here = here.display(),
        other = other.display(),
    );
    let got = parse_plugin_list(&json, &here);
    let ids: Vec<&str> = got.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "review-kit@example-market",
            "ours@example-market",
            "in-place@example-market"
        ]
    );
    assert_eq!(got[0].name, "review-kit");
    assert_eq!(got[0].root, a);
    assert_eq!(got[2].root, c);
    assert!(parse_plugin_list("[]", &here).is_empty());
    let noisy = format!("Starting background service…\n{json}");
    assert_eq!(parse_plugin_list(&noisy, &here).len(), 3);
    assert!(parse_plugin_list("not json", &here).is_empty());
}

#[test]
fn a_load_reads_every_source_and_lists_plugins_with_the_accounts_command() {
    let _guard = lock();
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config");
    skill(&config, "fix-bug", "---\ndescription: Personal fix\n---\n");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    skill(
        &project.join(".claude"),
        "fix-bug",
        "---\ndescription: Project fix\n---\n",
    );
    skill(
        &project.join(".claude"),
        "triage",
        "---\ndescription: Triage\n---\n",
    );
    let plugin = plugin_dir(tmp.path(), Some(r#"{"name": "review-kit"}"#));
    skill(&plugin, "code-review", "---\ndescription: Review\n---\n");

    let fake = Fake::new("claude", "cat \"$FAKE_DIR/plugins.json\"");
    write(
        &fake.dir.path().join("plugins.json"),
        &format!(
            r#"[{{"id": "review-kit@example-market", "version": "1.0.0", "scope": "user",
                "enabled": true, "installPath": "{}"}}]"#,
            plugin.display()
        ),
    );
    let mut account = fake.account();
    account.config_dir = config.clone();
    let load = CommandLoad {
        id: 1,
        account,
        folder: project.clone(),
    };
    let list = load.run(&CliRunner::new());
    assert_eq!(
        names(&list.commands),
        ["fix-bug", "review-kit:code-review", "triage"]
    );
    assert_eq!(
        by_name(&list.commands, "fix-bug").description,
        "Personal fix"
    );
    assert_eq!(fake.argv(), ["plugin", "list", "--json"]);
    assert_eq!(
        fake.cwd(),
        std::fs::canonicalize(&project).unwrap(),
        "run in the project folder"
    );

    // A failing listing only leaves the plugins out.
    let failing = Fake::new("claude", "echo boom >&2; exit 1");
    let mut account = failing.account();
    account.config_dir = config;
    let load = CommandLoad {
        id: 2,
        account,
        folder: project,
    };
    assert_eq!(
        names(&load.run(&CliRunner::new()).commands),
        ["fix-bug", "triage"]
    );
}

// ---- the menu -----------------------------------------------------------------------------

fn ready(items: Vec<SlashCommand>) -> Commands {
    Commands::Ready(Arc::new(list_of(items)))
}

fn menu_with(items: Vec<SlashCommand>) -> SlashMenu {
    let mut m = SlashMenu::default();
    m.set_source(None, ready(items));
    m
}

fn view_names(menu: &SlashMenu, text: &TextArea) -> Option<Vec<String>> {
    match menu.view(text)? {
        MenuView::Loading => Some(vec!["…".into()]),
        MenuView::List { items, .. } => Some(items.iter().map(|c| c.name.clone()).collect()),
    }
}

fn sample() -> Vec<SlashCommand> {
    vec![
        cmd("fix-bug", Some("[ticket]"), Origin::Project),
        cmd("review", None, Origin::Personal),
        cmd(
            "review-kit:code-review",
            None,
            Origin::Plugin("review-kit".into()),
        ),
        cmd("prefix-fix", None, Origin::Personal),
    ]
}

#[test]
fn the_trigger_is_only_the_first_word_with_the_cursor_inside() {
    assert!(trigger(&text_at_end("/fi")).is_some());
    assert!(trigger(&text_at_end("  /fi")).is_some());
    assert!(trigger(&text_at_end("\n  \n/fi")).is_some());
    assert_eq!(trigger(&text_at_end("/")).unwrap().query, "");
    assert!(trigger(&text_at_end("fix /fi")).is_none());
    assert!(
        trigger(&text_at_end("/fix-bug ")).is_none(),
        "past the word"
    );
    assert!(trigger(&text_at_end("/home/user")).is_none(), "a path");
    let mut t = text_at_end("/fix-bug some link");
    t.set_cursor(0, 3);
    let tr = trigger(&t).unwrap();
    assert_eq!((tr.start, tr.end, tr.query.as_str()), (0, 8, "fix-bug"));
    t.set_cursor(0, 0);
    assert!(trigger(&t).is_none(), "before the slash");
}

#[test]
fn matching_puts_prefixes_first_then_segments_then_substrings() {
    let list = list_of(sample());
    let got: Vec<&str> = matches(&list, "fix")
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(got, ["fix-bug", "prefix-fix"]);
    let got: Vec<&str> = matches(&list, "CODE")
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(got, ["review-kit:code-review"]);
    let got: Vec<&str> = matches(&list, "rev")
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(got, ["review", "review-kit:code-review"]);
    assert_eq!(matches(&list, "").len(), 4);
    assert!(matches(&list, "zzz").is_empty());
}

#[test]
fn the_menu_opens_filters_and_closes() {
    let menu = menu_with(sample());
    assert_eq!(
        view_names(&menu, &text_at_end("/f")).unwrap(),
        ["fix-bug", "prefix-fix"]
    );
    assert_eq!(
        view_names(&menu, &text_at_end("/zzz")),
        None,
        "no match closes it"
    );
    assert_eq!(view_names(&menu, &text_at_end("x /f")), None);
    // Nothing found: no menu at all; loading: a loading row.
    let empty = menu_with(Vec::new());
    assert_eq!(view_names(&empty, &text_at_end("/")), None);
    let mut loading = SlashMenu::default();
    loading.set_source(None, Commands::Loading);
    assert_eq!(view_names(&loading, &text_at_end("/")).unwrap(), ["…"]);
    assert_eq!(view_names(&SlashMenu::default(), &text_at_end("/")), None);
}

fn press(menu: &mut SlashMenu, text: &mut TextArea, k: KeyEvent) -> bool {
    let took = menu.handle_key(text, &k);
    if !took {
        text.key(&k);
    }
    menu.sync(text);
    took
}

#[test]
fn keys_choose_accept_and_escape() {
    let mut menu = menu_with(sample());
    let mut text = TextArea::default();
    for c in "/re".chars() {
        press(&mut menu, &mut text, ch(c));
    }
    let selected = |m: &SlashMenu, t: &TextArea| match m.view(t) {
        Some(MenuView::List { selected, items }) => items[selected].name.clone(),
        other => panic!("{other:?}"),
    };
    // Prefix matches, then `prefix-fix`, which only contains `re`.
    assert_eq!(selected(&menu, &text), "review");
    assert!(press(&mut menu, &mut text, key(KeyCode::Down)));
    assert_eq!(selected(&menu, &text), "review-kit:code-review");
    assert!(press(&mut menu, &mut text, key(KeyCode::Down)));
    assert_eq!(selected(&menu, &text), "prefix-fix");
    // Wraps around.
    assert!(press(&mut menu, &mut text, key(KeyCode::Down)));
    assert_eq!(selected(&menu, &text), "review");
    assert!(press(&mut menu, &mut text, key(KeyCode::Up)));
    assert_eq!(selected(&menu, &text), "prefix-fix");
    // Typing keeps filtering and starts at the top again.
    press(&mut menu, &mut text, ch('v'));
    assert_eq!(selected(&menu, &text), "review");
    // Tab accepts: the word becomes `/name ` and the menu closes.
    assert!(press(&mut menu, &mut text, key(KeyCode::Tab)));
    assert_eq!(text.text(), "/review ");
    assert_eq!(text.cursor(), (0, 8));
    assert!(menu.view(&text).is_none());
    assert_eq!(menu.argument_hint(&text), None, "no hint for review");

    // Enter accepts too, inside a word with text after it.
    let mut text = TextArea::new("/fi link");
    text.set_cursor(0, 3);
    menu.sync(&text);
    assert!(press(&mut menu, &mut text, key(KeyCode::Enter)));
    assert_eq!(text.text(), "/fix-bug link");
    assert_eq!(text.cursor(), (0, 9), "after the space that was there");
    assert_eq!(
        menu.argument_hint(&text),
        Some(("/fix-bug".to_owned(), "[ticket]".to_owned()))
    );
    // The hint goes once the command does.
    let changed = TextArea::new("/review link");
    assert_eq!(menu.argument_hint(&changed), None);

    // Esc closes only the menu; typing opens it again.
    let mut text = TextArea::default();
    for c in "/f".chars() {
        press(&mut menu, &mut text, ch(c));
    }
    assert!(press(&mut menu, &mut text, key(KeyCode::Esc)));
    assert!(menu.view(&text).is_none());
    assert_eq!(text.text(), "/f");
    // With the menu closed the keys are the field's: Enter is not taken.
    assert!(!menu.handle_key(&mut text, &key(KeyCode::Enter)));
    press(&mut menu, &mut text, ch('i'));
    assert!(menu.view(&text).is_some(), "typing reopens it");
    // Modified keys are never the menu's.
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    assert!(!menu.handle_key(&mut text, &alt_enter));
    let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
    assert!(!menu.handle_key(&mut text, &shift_tab));
}

#[test]
fn while_loading_only_esc_is_taken() {
    let mut menu = SlashMenu::default();
    menu.set_source(None, Commands::Loading);
    let mut text = TextArea::new("/x");
    assert!(!menu.handle_key(&mut text, &key(KeyCode::Enter)));
    assert!(!menu.handle_key(&mut text, &key(KeyCode::Down)));
    assert!(menu.handle_key(&mut text, &key(KeyCode::Esc)));
    assert!(menu.view(&text).is_none());
}

// ---- in the app -----------------------------------------------------------------------------

/// The dialog over two projects, focus on the task field; returns the
/// project root.
fn dialog(app: &mut App) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    app.open_dialog(project_list(root.path(), &["api", "web"]));
    while app.dialog().unwrap().field != Field::Task {
        app.handle_key(key(KeyCode::Tab));
    }
    root
}

fn loads(effects: &[Effect]) -> Vec<&CommandLoad> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::LoadCommands(l) => Some(l),
            _ => None,
        })
        .collect()
}

/// The next load the app asks for, answered with `items`.
fn answer(app: &mut App, items: Vec<SlashCommand>) -> CommandLoad {
    let effects = app.tick(Instant::now());
    let load = loads(&effects)
        .first()
        .map(|l| (*l).clone())
        .unwrap_or_else(|| panic!("no load in {effects:?}"));
    app.commands_loaded(&load, list_of(items));
    load
}

fn type_in(app: &mut App, s: &str) {
    for c in s.chars() {
        app.handle_key(ch(c));
    }
}

#[test]
fn the_dialog_loads_for_its_account_and_project_and_completes() {
    let mut app = app_single();
    let root = dialog(&mut app);
    let load = answer(&mut app, sample());
    assert_eq!(load.account.name, "main");
    assert_eq!(
        load.folder,
        app.dialog().unwrap().selected_project().unwrap().path
    );
    assert!(load.folder.starts_with(root.path()));
    // No second load while nothing changed.
    assert!(loads(&app.tick(Instant::now())).is_empty());

    type_in(&mut app, "/fi");
    assert!(app.dialog().unwrap().slash_open());
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    let d = app.dialog().unwrap();
    assert_eq!(d.task.text(), "/fix-bug ");
    assert!(!d.slash_open());
    // With the menu closed, Enter is a new line again.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.dialog().unwrap().task.text(), "/fix-bug \n");

    // Esc closes the menu, not the dialog.
    let mut app = app_single();
    let _root = dialog(&mut app);
    answer(&mut app, sample());
    type_in(&mut app, "/");
    app.handle_key(key(KeyCode::Esc));
    let d = app.dialog().expect("the dialog stays open");
    assert!(!d.slash_open());
    assert_eq!(d.task.text(), "/");
    // A second Esc asks about the typed task, as without the menu.
    app.handle_key(key(KeyCode::Esc));
    assert!(app.dialog().unwrap().prompt.is_some());
}

#[test]
fn tab_with_the_menu_closed_still_moves_to_the_next_field() {
    let mut app = app_single();
    let _root = dialog(&mut app);
    answer(&mut app, sample());
    type_in(&mut app, "/zzz");
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.dialog().unwrap().field, Field::Name);
}

#[test]
fn a_new_project_loads_again_and_the_cache_shows_meanwhile() {
    let mut app = app_single();
    let _root = dialog(&mut app);
    let first = answer(&mut app, sample());
    // Another project: a new load, the menu waits for it.
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    app.handle_key(key(KeyCode::Down));
    let effects = app.tick(Instant::now());
    let second = loads(&effects)[0].clone();
    assert_ne!(second.folder, first.folder);
    assert_eq!(app.dialog().unwrap().slash.commands(), &Commands::Loading);
    app.commands_loaded(
        &second,
        list_of(vec![cmd("web-only", None, Origin::Project)]),
    );
    // Back to the first project: its cached list shows at once, and is
    // refreshed.
    app.handle_key(key(KeyCode::Up));
    let effects = app.tick(Instant::now());
    let third = loads(&effects)[0].clone();
    assert_eq!(third.folder, first.folder);
    let Commands::Ready(list) = app.dialog().unwrap().slash.commands() else {
        panic!("the cached list shows");
    };
    assert_eq!(list.commands.len(), 4);
    // A result for an older load still updates the list.
    app.commands_loaded(&first, list_of(vec![cmd("only", None, Origin::Project)]));
    let Commands::Ready(list) = app.dialog().unwrap().slash.commands() else {
        panic!();
    };
    assert_eq!(names(&list.commands), ["only"]);
}

#[test]
fn the_reply_window_loads_for_the_owner_and_the_sessions_folder() {
    let mut app = app_single();
    app.apply_poll(common::poll("main", "after-stop-all.json"));
    common::select_name(&mut app, "say-ok");
    app.handle_key(ch('t'));
    let r = app.reply_window().expect("the reply window");
    assert!(r.folder.is_some());
    let load = answer(&mut app, sample());
    assert_eq!(load.account.name, "main");
    assert_eq!(
        Some(&load.folder),
        app.reply_window().unwrap().folder.as_ref()
    );
    type_in(&mut app, "/rev");
    assert!(app.reply_window().unwrap().slash_open());
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.reply_window().unwrap().text.text(), "/review ");
    // Enter is a new line once the menu is closed.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.reply_window().unwrap().text.text(), "/review \n");
    // Esc with the menu open keeps the window.
    let mut app2 = app_single();
    app2.apply_poll(common::poll("main", "after-stop-all.json"));
    common::select_name(&mut app2, "say-ok");
    app2.handle_key(ch('t'));
    answer(&mut app2, sample());
    type_in(&mut app2, "/");
    app2.handle_key(key(KeyCode::Esc));
    assert!(app2.reply_window().is_some());
    assert!(!app2.reply_window().unwrap().slash_open());
}

#[test]
fn an_account_command_line_is_the_owners() {
    // The load carries the configured account, so its command runs.
    let mut app = common::app_two();
    let _root = dialog(&mut app);
    answer(&mut app, Vec::new());
    // Account field: three Tabs from the task field (name, account).
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Right));
    let effects = app.tick(Instant::now());
    let load: &Account = &loads(&effects)[0].account;
    assert_eq!(load.name, "alt");
    assert_eq!(load.command, "claude-alt");
}

// ---- drawing -----------------------------------------------------------------------------------

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw(f, app, Instant::now()))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn the_menu_draws_in_both_styles() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        for (w, h) in [(80, 24), (120, 36)] {
            let what = format!("{style:?} {w}x{h}");
            let mut app = app_single();
            app.style = style;
            app.set_theme(UiTheme::Neutral);
            let _root = dialog(&mut app);
            answer(&mut app, sample());
            type_in(&mut app, "/");
            let lines = render(&mut app, w, h);
            let all = lines.join("\n");
            assert!(all.contains("Commands"), "{what}\n{all}");
            assert!(all.contains("▶ /fix-bug [ticket]"), "{what}\n{all}");
            assert!(all.contains("/review-kit:code-review"), "{what}\n{all}");
            assert!(all.contains("plugin review-kit"), "{what}\n{all}");
            assert!(all.contains("Tab insert"), "{what}\n{all}");
            // The description shows where there is room.
            assert!(all.contains("about fix-bug"), "{what}\n{all}");

            // Accepted: the argument hint on the task's label row.
            app.handle_key(key(KeyCode::Tab));
            let lines = render(&mut app, w, h);
            let all = lines.join("\n");
            assert!(!all.contains("Tab insert"), "{what}: closed\n{all}");
            let row = lines
                .iter()
                .find(|l| l.contains("Task"))
                .unwrap_or_else(|| panic!("{what}\n{all}"));
            assert!(row.contains("/fix-bug [ticket]"), "{what}\n{all}");

            // Loading.
            let mut app = app_single();
            app.style = style;
            let _root = dialog(&mut app);
            app.tick(Instant::now());
            type_in(&mut app, "/");
            let all = render(&mut app, w, h).join("\n");
            assert!(all.contains(commands::LOADING_TEXT), "{what}\n{all}");
        }
    }
}

#[test]
fn the_reply_window_grows_to_hold_the_menu() {
    for style in [UiStyle::Minimal, UiStyle::Dashboard] {
        let mut app = app_single();
        app.style = style;
        app.apply_poll(common::poll("main", "after-stop-all.json"));
        common::select_name(&mut app, "say-ok");
        app.handle_key(ch('t'));
        answer(&mut app, sample());
        type_in(&mut app, "/");
        let lines = render(&mut app, 80, 24);
        let all = lines.join("\n");
        for name in [
            "/fix-bug",
            "/prefix-fix",
            "/review",
            "/review-kit:code-review",
        ] {
            assert!(all.contains(name), "{style:?}: {name}\n{all}");
        }
        app.handle_key(key(KeyCode::Enter));
        let all = render(&mut app, 80, 24).join("\n");
        assert!(
            all.contains("/fix-bug [ticket]"),
            "{style:?}: the hint row\n{all}"
        );
    }
}
