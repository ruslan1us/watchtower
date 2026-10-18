//! Config parsing, defaults and validation (doc/0002, doc/0003).

use std::path::{Path, PathBuf};

use watchtower::config::{Account, Config, ConfigError};
use watchtower::paths::Dirs;

const HOME: &str = "/home/user";

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::parse(text, Path::new(HOME))
}

fn invalid(text: &str) -> String {
    match parse(text) {
        Err(ConfigError::Invalid(msg)) => msg,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn missing_file_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = Dirs::new(dir.path(), Some(&dir.path().join("xdg")));
    assert!(!dirs.config_file().exists());
    let c = Config::load(&dirs).unwrap();
    assert_eq!(c, Config::defaults(dir.path()));
    assert_eq!(c.default_account, "main");
    assert_eq!(c.poll_interval_secs, 2);
    assert_eq!(c.accounts.len(), 1);
    let main = &c.accounts[0];
    assert_eq!(
        (main.name.as_str(), main.command.as_str()),
        ("main", "claude")
    );
    assert_eq!(main.config_dir, dir.path().join(".claude"));
    assert_eq!(main.argv(), ["claude"]);
    assert_eq!(c.default_account().name, "main");
}

#[test]
fn full_documented_config() {
    let text = r#"
default_account = "alt"
poll_interval_secs = 5
project_roots = ["~/workspace"]
extra_projects = ["~/notes", "/srv/shared"]
default_model = "opus"
default_effort = "high"
default_permission_mode = "manual"
handoff_prompt = "carry on"
some_future_key = { nested = true }

[[accounts]]
name = "main"
command = "claude"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
colour = "blue"
"#;
    let c = parse(text).unwrap();
    assert_eq!(c.default_account, "alt");
    assert_eq!(c.poll_interval_secs, 5);
    assert_eq!(c.poll_interval(), std::time::Duration::from_secs(5));
    assert_eq!(c.project_roots, [PathBuf::from("/home/user/workspace")]);
    assert_eq!(
        c.extra_projects,
        [
            PathBuf::from("/home/user/notes"),
            PathBuf::from("/srv/shared")
        ]
    );
    assert_eq!(c.default_model, "opus");
    assert_eq!(c.default_effort, "high");
    assert_eq!(c.default_permission_mode, "manual");
    assert_eq!(c.handoff_prompt, "carry on");
    assert_eq!(c.accounts.len(), 2);
    assert_eq!(
        c.accounts[0].config_dir,
        PathBuf::from("/home/user/.claude")
    );
    assert_eq!(
        c.account("alt").unwrap().config_dir,
        PathBuf::from("/home/user/.claude-alt")
    );
    assert_eq!(c.default_account().command, "claude-alt");
}

#[test]
fn partial_config_defaults() {
    let c = parse("poll_interval_secs = 3\n").unwrap();
    assert_eq!(c.accounts[0].name, "main");
    assert_eq!(c.default_account, "main");
    assert_eq!(c.default_model, "default");
    assert_eq!(c.default_effort, "default");
    assert_eq!(c.default_permission_mode, "auto");
    assert_eq!(c.handoff_prompt, "continue");

    // Without default_account the first account is the default.
    let c = parse("[[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n").unwrap();
    assert_eq!(c.default_account, "alt");
}

#[test]
fn command_is_split_like_a_shell() {
    let a = Account::new(
        "alt",
        r#"env CLAUDE_CONFIG_DIR="/home/user/my dir" claude"#,
        None,
        Path::new(HOME),
    )
    .unwrap();
    assert_eq!(
        a.argv(),
        ["env", "CLAUDE_CONFIG_DIR=/home/user/my dir", "claude"]
    );
}

#[test]
fn validation_errors() {
    let dup = "[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n\
               [[accounts]]\nname = \"main\"\ncommand = \"claude-alt\"\n";
    assert!(invalid(dup).contains("duplicate"));

    let missing_default =
        "default_account = \"nope\"\n[[accounts]]\nname = \"main\"\ncommand = \"claude\"\n";
    assert!(invalid(missing_default).contains("nope"));

    assert!(invalid("default_account = \"alt\"\n").contains("alt"));
    assert!(invalid("[[accounts]]\nname = \"main\"\ncommand = \"  \"\n").contains("empty"));
    assert!(
        invalid("[[accounts]]\nname = \"main\"\ncommand = \"claude 'oops\"\n").contains("quoted")
    );
    assert!(invalid("[[accounts]]\nname = \"\"\ncommand = \"claude\"\n").contains("name"));
    assert!(invalid("poll_interval_secs = 0\n").contains("poll_interval_secs"));
}

/// Two accounts, `a` and `b`, with the given `config_dir` lines (or none).
fn two_accounts(a_dir: Option<&str>, b_dir: Option<&str>) -> String {
    let line = |d: Option<&str>| {
        d.map(|d| format!("config_dir = \"{d}\"\n"))
            .unwrap_or_default()
    };
    format!(
        "[[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n{}\
         [[accounts]]\nname = \"main\"\ncommand = \"claude\"\n{}",
        line(a_dir),
        line(b_dir)
    )
}

#[test]
fn duplicate_config_dirs_are_rejected() {
    // S4: `alt` forgot its config_dir, so both accounts are ~/.claude and a
    // live process could never be told apart (doc/0017).
    let msg = invalid(&two_accounts(None, None));
    assert!(msg.contains("`alt` and `main`"), "{msg}");
    assert!(msg.contains("same config dir"), "{msg}");
    assert!(msg.contains("/home/user/.claude"), "{msg}");

    // Paths that do not exist compare by expanded path: `~` and a trailing
    // slash make no difference.
    let msg = invalid(&two_accounts(Some("/home/user/.claude/"), None));
    assert!(msg.contains("same config dir"), "{msg}");
    assert!(
        invalid(&two_accounts(
            Some("~/.claude-alt"),
            Some("/home/user/.claude-alt")
        ))
        .contains("same config dir")
    );

    // Distinct dirs are fine, whether they exist or not.
    assert!(parse(&two_accounts(Some("~/.claude-alt"), None)).is_ok());
}

#[test]
fn symlinked_duplicate_config_dirs_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".claude")).unwrap();
    std::os::unix::fs::symlink(home.path().join(".claude"), home.path().join("alias")).unwrap();
    let text = two_accounts(Some("~/alias"), None);
    match Config::parse(&text, home.path()) {
        Err(ConfigError::Invalid(msg)) => assert!(msg.contains("same config dir"), "{msg}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    // A real, separate dir next to it is accepted.
    std::fs::create_dir(home.path().join(".claude-alt")).unwrap();
    let text = two_accounts(Some("~/.claude-alt"), None);
    assert!(Config::parse(&text, home.path()).is_ok());
}

#[test]
fn parse_errors() {
    assert!(matches!(
        parse("not = [toml"),
        Err(ConfigError::Parse { .. })
    ));
    assert!(matches!(
        parse("poll_interval_secs = \"two\""),
        Err(ConfigError::Parse { .. })
    ));
    // An account needs a command.
    assert!(matches!(
        parse("[[accounts]]\nname = \"main\"\n"),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn load_reports_the_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = Dirs::new(dir.path(), Some(dir.path()));
    std::fs::create_dir_all(&dirs.config_dir).unwrap();
    std::fs::write(dirs.config_file(), "poll_interval_secs = [").unwrap();
    let err = Config::load(&dirs).unwrap_err();
    assert!(err.to_string().contains("config.toml"), "{err}");

    std::fs::write(dirs.config_file(), "poll_interval_secs = 7\n").unwrap();
    assert_eq!(Config::load(&dirs).unwrap().poll_interval_secs, 7);
}
