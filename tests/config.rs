//! Config parsing, defaults and validation (doc/0002, doc/0003).

use std::path::{Path, PathBuf};

use watchtower::config::{
    Account, ColorDepth, Config, ConfigError, ConfigSource, UiStyle, UiTheme,
};
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
fn an_empty_or_blank_handoff_prompt_is_valid() {
    // doc/0033: empty means no prompt; the default stays `continue`.
    for (line, want) in [
        ("handoff_prompt = \"\"\n", ""),
        ("handoff_prompt = \"   \"\n", "   "),
        ("handoff_prompt = \"go on\"\n", "go on"),
        ("poll_interval_secs = 2\n", "continue"),
    ] {
        let c = parse(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(c.handoff_prompt, want, "{line}");
    }
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
    assert!(
        !c.allow_cross_group_handoff,
        "cross-group hand-off is off by default"
    );
    assert_eq!(c.accounts[0].group, "", "no group by default");
    assert!(!Config::defaults(Path::new(HOME)).allow_cross_group_handoff);

    // Without default_account the first account is the default.
    let c = parse("[[accounts]]\nname = \"alt\"\ncommand = \"claude-alt\"\n").unwrap();
    assert_eq!(c.default_account, "alt");
}

#[test]
fn account_groups_parse() {
    let c = parse(
        r#"
allow_cross_group_handoff = true

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "personal_2-b"

[[accounts]]
name = "spare"
command = "claude-spare"
config_dir = "~/.claude-spare"
"#,
    )
    .unwrap();
    assert!(c.allow_cross_group_handoff);
    let groups: Vec<(&str, &str)> = c
        .accounts
        .iter()
        .map(|a| (a.name.as_str(), a.group.as_str()))
        .collect();
    assert_eq!(
        groups,
        [("main", "work"), ("alt", "personal_2-b"), ("spare", "")]
    );
    assert_eq!(c.accounts[0].label(), "main (work)");
    assert_eq!(c.accounts[2].label(), "spare");
}

#[test]
fn invalid_group_names_are_rejected() {
    for bad in [
        "", "   ", "my work", " work", "work!", "a.b", "groupé", "x/y",
    ] {
        let text =
            format!("[[accounts]]\nname = \"main\"\ncommand = \"claude\"\ngroup = \"{bad}\"\n");
        let msg = invalid(&text);
        assert!(
            msg.contains("main") && msg.contains("group"),
            "{bad:?}: {msg}"
        );
    }
    // Not a string: a parse error.
    assert!(matches!(
        parse("[[accounts]]\nname = \"main\"\ncommand = \"claude\"\ngroup = 1\n"),
        Err(ConfigError::Parse { .. })
    ));
    assert!(matches!(
        parse("allow_cross_group_handoff = \"yes\"\n"),
        Err(ConfigError::Parse { .. })
    ));
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

/// `config.example.toml` at the repository root parses, documents every key
/// of doc/0002, and keeps the built-in defaults where it shows them, so the
/// public example cannot drift from the code.
#[test]
fn example_config_is_valid_and_complete() {
    let text = include_str!("../config.example.toml");
    let c = parse(text).unwrap();
    let defaults = Config::defaults(Path::new(HOME));

    assert_eq!(c.default_account, "main");
    assert_eq!(c.poll_interval_secs, defaults.poll_interval_secs);
    assert_eq!(c.logs_follow_secs, defaults.logs_follow_secs);
    assert_eq!(c.default_model, defaults.default_model);
    assert_eq!(c.default_effort, defaults.default_effort);
    assert_eq!(c.default_permission_mode, defaults.default_permission_mode);
    assert_eq!(c.handoff_prompt, defaults.handoff_prompt);
    assert_eq!(
        c.allow_cross_group_handoff,
        defaults.allow_cross_group_handoff
    );
    assert_eq!(c.project_roots, [PathBuf::from("/home/user/workspace")]);
    assert_eq!(c.extra_projects, [PathBuf::from("/home/user/notes")]);

    let accounts: Vec<(&str, &str, PathBuf, &str)> = c
        .accounts
        .iter()
        .map(|a| {
            (
                a.name.as_str(),
                a.command.as_str(),
                a.config_dir.clone(),
                a.group.as_str(),
            )
        })
        .collect();
    // Both example accounts share a group, so hand-off works between them.
    assert_eq!(
        accounts,
        [
            (
                "main",
                "claude",
                PathBuf::from("/home/user/.claude"),
                "personal"
            ),
            (
                "alt",
                "claude-alt",
                PathBuf::from("/home/user/.claude-alt"),
                "personal"
            ),
        ]
    );
    // The default `main` account is the example's first one.
    assert_eq!(defaults.accounts[0].name, c.accounts[0].name);
    assert_eq!(defaults.accounts[0].command, c.accounts[0].command);

    // Every documented key appears, and nothing else: a new config key
    // must be added to the example (and to this list).
    let table: toml::Table = toml::from_str(text).unwrap();
    let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "accounts",
            "allow_cross_group_handoff",
            "default_account",
            "default_effort",
            "default_model",
            "default_permission_mode",
            "extra_projects",
            "handoff_prompt",
            "logs_follow_secs",
            "poll_interval_secs",
            "project_roots",
            "sessions",
            "ui",
        ]
    );
    let sessions_keys: Vec<&str> = table["sessions"]
        .as_table()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(sessions_keys, ["track_subagents"]);
    assert_eq!(c.track_subagents, defaults.track_subagents);
    let mut ui_keys: Vec<&str> = table["ui"]
        .as_table()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    ui_keys.sort_unstable();
    assert_eq!(ui_keys, ["color_depth", "style", "theme"]);
    assert_eq!(c.ui_style, defaults.ui_style);
    assert_eq!(c.ui_theme, defaults.ui_theme);
    assert_eq!(c.ui_color_depth, defaults.ui_color_depth);
    for account in table["accounts"].as_array().unwrap() {
        let mut keys: Vec<&str> = account
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["command", "config_dir", "group", "name"]);
    }
}

#[test]
fn load_reports_where_it_looked_and_whether_the_file_was_found() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = Dirs::new(dir.path(), None);
    let (config, source) = Config::load_with_source(&dirs).unwrap();
    assert_eq!(config, Config::defaults(dir.path()));
    assert_eq!(
        source,
        ConfigSource {
            path: dirs.config_file(),
            found: false
        }
    );
    assert_eq!(source, ConfigSource::probe(&dirs.config_file()));
    assert_eq!(
        source.line(Some(dir.path())),
        "config: ~/.config/watchtower/config.toml (not found — using defaults)"
    );

    std::fs::create_dir_all(&dirs.config_dir).unwrap();
    std::fs::write(dirs.config_file(), "poll_interval_secs = 5\n").unwrap();
    let (config, source) = Config::load_with_source(&dirs).unwrap();
    assert_eq!(config.poll_interval_secs, 5);
    assert!(source.found);
    assert_eq!(source, ConfigSource::probe(&dirs.config_file()));
    assert_eq!(
        source.line(None),
        format!("config: {}", dirs.config_file().display())
    );
    // A broken file is still an error, not "not found".
    std::fs::write(dirs.config_file(), "poll_interval_secs = [").unwrap();
    assert!(matches!(
        Config::load_with_source(&dirs),
        Err(ConfigError::Parse { .. })
    ));
}

/// `[ui] style` (doc/0020): dashboard by default, minimal on request, and
/// an unknown value is an error that names the choices.
#[test]
fn ui_style_defaults_to_dashboard_and_rejects_unknown_values() {
    assert_eq!(parse("").unwrap().ui_style, UiStyle::Dashboard);
    assert_eq!(parse("[ui]\n").unwrap().ui_style, UiStyle::Dashboard);
    assert_eq!(
        Config::defaults(Path::new(HOME)).ui_style,
        UiStyle::Dashboard
    );
    assert_eq!(
        parse("[ui]\nstyle = \"minimal\"\n").unwrap().ui_style,
        UiStyle::Minimal
    );
    assert_eq!(
        parse("[ui]\nstyle = \"dashboard\"\n").unwrap().ui_style,
        UiStyle::Dashboard
    );
    let err = parse("[ui]\nstyle = \"fancy\"\n").unwrap_err();
    assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("[ui] style `fancy`"), "{text}");
    assert!(text.contains("\"dashboard\" or \"minimal\""), "{text}");
    // A value of the wrong type is a parse error, as for other keys.
    assert!(matches!(
        parse("[ui]\nstyle = 1\n"),
        Err(ConfigError::Parse { .. })
    ));
    assert_eq!(UiStyle::Minimal.as_str(), "minimal");
}

/// `[ui] theme` (doc/0021): neutral by default, each named theme on
/// request, and an unknown value is an error that lists the valid names.
#[test]
fn ui_theme_defaults_to_neutral_and_rejects_unknown_values() {
    assert_eq!(parse("").unwrap().ui_theme, UiTheme::Neutral);
    assert_eq!(parse("[ui]\n").unwrap().ui_theme, UiTheme::Neutral);
    assert_eq!(
        parse("[ui]\nstyle = \"minimal\"\n").unwrap().ui_theme,
        UiTheme::Neutral
    );
    assert_eq!(Config::defaults(Path::new(HOME)).ui_theme, UiTheme::Neutral);
    for (name, theme) in [
        ("neutral", UiTheme::Neutral),
        ("warm", UiTheme::Warm),
        ("terminal", UiTheme::Terminal),
        ("tokyo-night", UiTheme::TokyoNight),
        ("neutral-light", UiTheme::NeutralLight),
        ("warm-light", UiTheme::WarmLight),
        ("ansi16", UiTheme::Ansi16),
    ] {
        let c = parse(&format!("[ui]\ntheme = \"{name}\"\n")).unwrap();
        assert_eq!(c.ui_theme, theme, "{name}");
        assert_eq!(c.ui_style, UiStyle::Dashboard, "{name}");
        assert_eq!(theme.as_str(), name);
    }
    assert_eq!(UiTheme::ALL.len(), 7);
    for bad in [
        "tokyo_night",
        "Neutral",
        "light",
        "neutral_light",
        "ansi",
        "",
    ] {
        let err = parse(&format!("[ui]\ntheme = \"{bad}\"\n")).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains(&format!("[ui] theme `{bad}`")), "{text}");
        assert!(
            text.contains(
                "\"neutral\", \"warm\", \"terminal\", \"tokyo-night\", \"neutral-light\", \
                 \"warm-light\" or \"ansi16\""
            ),
            "{text}"
        );
    }
    assert!(matches!(
        parse("[ui]\ntheme = true\n"),
        Err(ConfigError::Parse { .. })
    ));
}

/// `[ui] color_depth` (doc/0038): `auto` by default, each value on request,
/// anything else (a number included) an error.
#[test]
fn color_depth_defaults_to_auto_and_rejects_unknown_values() {
    assert_eq!(parse("").unwrap().ui_color_depth, ColorDepth::Auto);
    assert_eq!(parse("[ui]\n").unwrap().ui_color_depth, ColorDepth::Auto);
    assert_eq!(
        Config::defaults(Path::new(HOME)).ui_color_depth,
        ColorDepth::Auto
    );
    for (name, depth) in [
        ("auto", ColorDepth::Auto),
        ("truecolor", ColorDepth::Truecolor),
        ("256", ColorDepth::Ansi256),
        ("16", ColorDepth::Ansi16),
    ] {
        let c = parse(&format!("[ui]\ncolor_depth = \"{name}\"\n")).unwrap();
        assert_eq!(c.ui_color_depth, depth, "{name}");
        assert_eq!(c.ui_theme, UiTheme::Neutral, "{name}");
        assert_eq!(depth.as_str(), name);
    }
    for bad in ["24bit", "Auto", "8", ""] {
        let err = parse(&format!("[ui]\ncolor_depth = \"{bad}\"\n")).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains(&format!("[ui] color_depth `{bad}`")),
            "{text}"
        );
        assert!(
            text.contains("\"auto\", \"truecolor\", \"256\" or \"16\""),
            "{text}"
        );
    }
    assert!(matches!(
        parse("[ui]\ncolor_depth = 256\n"),
        Err(ConfigError::Parse { .. })
    ));
}

/// `auto` reads the environment (doc/0038): `COLORTERM` truecolor first;
/// else only a `TERM` known to be limited is 16 colours, and every other
/// `TERM` is full colour as before, so nobody whose terminal is not known
/// to be limited sees a change. 256 is never detected; a set value wins.
#[test]
fn color_depth_detection_from_the_environment() {
    use ColorDepth::{Ansi16, Ansi256, Auto, Truecolor};
    let limited = [
        "",
        "dumb",
        "linux",
        "vt52",
        "vt100",
        "vt102",
        "vt220",
        "vt320",
        "ansi",
        "cons25",
        "cygwin",
        "sun",
        "xterm-color",
        "xterm-16color",
        "rxvt-16color",
        "LINUX",
    ];
    let full = [
        "xterm",
        "screen",
        "tmux",
        "rxvt",
        "rxvt-unicode",
        "xterm-256color",
        "screen-256color",
        "tmux-256color",
        "xterm-direct",
        "vte-256color",
        "xterm-kitty",
        "alacritty",
    ];
    let mut cases: Vec<(Option<&str>, Option<&str>, ColorDepth)> = vec![
        (Some("truecolor"), Some("xterm-256color"), Truecolor),
        (Some("24bit"), Some("xterm"), Truecolor),
        (Some("TrueColor"), None, Truecolor),
        // COLORTERM wins over a limited TERM.
        (Some("truecolor"), Some("linux"), Truecolor),
        (Some("24bit"), Some("dumb"), Truecolor),
        (None, None, Ansi16),
        (Some("yes"), Some("linux"), Ansi16),
        (Some(""), Some("screen"), Truecolor),
    ];
    for t in limited {
        cases.push((None, Some(t), Ansi16));
    }
    for t in full {
        cases.push((None, Some(t), Truecolor));
    }
    for (colorterm, term, want) in cases {
        assert_eq!(
            ColorDepth::detect(colorterm, term),
            want,
            "{colorterm:?} {term:?}"
        );
    }
    for detected in [Truecolor, Ansi256, Ansi16] {
        assert_eq!(Auto.resolve(detected), detected);
        for set in [Truecolor, Ansi256, Ansi16] {
            assert_eq!(set.resolve(detected), set);
        }
    }
}

/// What each theme gives way to (doc/0038): only on a 16-colour terminal,
/// and only a theme that needs more than 16 colours.
#[test]
fn themes_give_way_to_ansi16_on_sixteen_colours() {
    for theme in UiTheme::ALL {
        assert_eq!(theme.shown_at(ColorDepth::Truecolor), theme, "{theme:?}");
        assert_eq!(theme.shown_at(ColorDepth::Ansi256), theme, "{theme:?}");
        let want = match theme {
            UiTheme::Terminal | UiTheme::Ansi16 => theme,
            _ => UiTheme::Ansi16,
        };
        assert_eq!(theme.shown_at(ColorDepth::Ansi16), want, "{theme:?}");
    }
}

#[test]
fn track_subagents_defaults_to_true() {
    assert!(Config::defaults(Path::new(HOME)).track_subagents);
    assert!(parse("").unwrap().track_subagents);
    assert!(parse("[sessions]\n").unwrap().track_subagents);
    assert!(
        !parse("[sessions]\ntrack_subagents = false\n")
            .unwrap()
            .track_subagents
    );
    assert!(parse("[sessions]\ntrack_subagents = \"no\"\n").is_err());
}

/// `logs_follow_secs` (doc/0031): 0 is off, else 5 to 30; missing is 10.
#[test]
fn logs_follow_secs_is_off_or_five_to_thirty() {
    use std::time::Duration;
    let defaults = Config::defaults(Path::new(HOME));
    assert_eq!(defaults.logs_follow_secs, 10);
    assert_eq!(defaults.logs_follow(), Some(Duration::from_secs(10)));
    assert_eq!(parse("").unwrap().logs_follow_secs, 10);
    let off = parse("logs_follow_secs = 0\n").unwrap();
    assert_eq!(off.logs_follow_secs, 0);
    assert_eq!(off.logs_follow(), None);
    for ok in [5, 30] {
        let c = parse(&format!("logs_follow_secs = {ok}\n")).unwrap();
        assert_eq!(c.logs_follow(), Some(Duration::from_secs(ok)));
    }
    for bad in [4, 31] {
        let msg = invalid(&format!("logs_follow_secs = {bad}\n"));
        assert!(
            msg.contains(&format!(
                "logs_follow_secs must be 0 (off) or 5 to 30 seconds, not {bad}"
            )),
            "{msg}"
        );
    }
    assert!(parse("logs_follow_secs = -1\n").is_err());
    assert!(parse("logs_follow_secs = \"off\"\n").is_err());
}
