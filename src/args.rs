//! Command-line arguments of `tower`.

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start the TUI.
    Run,
    /// `tower setup`: start the TUI in the welcome window, to change the
    /// style, theme, project folders and options (doc/0048).
    Setup,
    /// `tower status [--json]`: print one merged snapshot and exit
    /// (doc/0010). `json` selects the JSON output over the short table.
    Status { json: bool },
    /// `--version` / `-V`.
    Version,
    /// `--help` / `-h`.
    Help,
    /// `tower update [--check | --version vX.Y.Z] [--force]`, also spelled
    /// `tower --update …` (doc/0023).
    Update {
        /// `--check`: only compare with the latest release.
        check: bool,
        /// `--version`: install exactly this release.
        version: Option<crate::update::Version>,
        /// `--force`: replace a cargo build output or the same version.
        force: bool,
    },
    /// `tower hook [--dir <config dir>]`: internal, run by the Claude Code
    /// hooks tower passes to the sessions it starts (doc/0029). Reads one
    /// payload from stdin; never prints and always exits 0. `dir` is
    /// tower's config dir (default: the usual one); arguments it does not
    /// understand make it do nothing (`valid: false`), still silently.
    Hook {
        /// `--dir`: tower's config dir.
        dir: Option<std::path::PathBuf>,
        /// False when the arguments were not understood.
        valid: bool,
    },
    /// `tower dispatch --project <dir> … [--] <task>`: start a background
    /// session the way the new-session dialog does (doc/0044).
    Dispatch(DispatchArgs),
    /// `tower wake <id or name> [--account A] [--text T | -- <text>]`: wake
    /// a session that has no live process (doc/0044).
    Wake(WakeArgs),
}

/// The arguments of `tower dispatch` (doc/0044). Options left out take the
/// config's defaults, as the dialog's fields do (doc/0008).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchArgs {
    /// `--project`: the folder the session starts in.
    pub project: String,
    /// `--account`: the account by name; `default_account` when absent.
    pub account: Option<String>,
    /// `--name`: the session name.
    pub name: Option<String>,
    /// `--model`; `default_model` when absent.
    pub model: Option<String>,
    /// `--effort`; `default_effort` when absent.
    pub effort: Option<String>,
    /// `--mode`: the permission mode; `default_permission_mode` when absent.
    pub mode: Option<String>,
    /// `--json`: print `{id, sessionId, name, account}`.
    pub json: bool,
    /// The task: the words after the options (or after `--`), joined by
    /// single spaces.
    pub task: String,
}

/// The arguments of `tower wake` (doc/0044).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WakeArgs {
    /// The session: its short id, its `sessionId` or its exact name.
    pub session: String,
    /// `--account`: the account for a session whose owner is unknown.
    pub account: Option<String>,
    /// The text for the next turn (`--text`, or the words after `--`);
    /// `None` wakes the session without a prompt (doc/0033).
    pub text: Option<String>,
}

/// Short usage text.
pub const USAGE: &str = "\
Usage: tower [--version | --help]
       tower setup
       tower status [--json]
       tower dispatch --project <dir> [--account <name>] [--name <name>]
                      [--model <m>] [--effort <e>] [--mode <p>] [--json] [--] <task>
       tower wake <id or name> [--account <name>] [--text <text> | -- <text>]
       tower update [--check | --version vX.Y.Z] [--force]

A terminal UI over Claude Code background sessions.
Run without arguments to start it. Press ? inside for the keys.

Commands:
  setup          The welcome window at any time: style, theme, project folders
                 and options, starting from the config's values. Saves only
                 what you change; Esc cancels without writing
  status         Poll every account once and print the merged sessions
  status --json  The same as JSON, for scripts and the orchestrator
  dispatch       Start a background session as the new-session dialog does
                 (`n`): the account's command in the project folder, the
                 config's model, effort and mode unless given, tower's
                 subagent hooks. Prints `started <id> · <name> under <account>`
                 (`--json`: id, sessionId, name, account). A task starting
                 with `-` goes after `--`. Exit 0 started, 1 claude failed,
                 2 bad arguments or config, 3 folder not trusted
  wake           Wake a session that has no live process, with the command
                 of the account that owns it: without a text it waits for
                 your prompt, with `--text` (or after `--`) the text is its
                 next prompt. `--account` only for an unknown owner. Exit 0
                 woken, 1 claude failed or started a copy, 2 bad arguments
                 or config, 3 folder not trusted, 4 refused (not found,
                 still running, owner unknown or another account)
  update         Replace this binary with the latest GitHub release, after
                 checking its checksum and that it runs (also `--update`)
    --check            Only report: exit 0 up to date, 10 newer release, 1 error
    --version vX.Y.Z   Install that release, also to reinstall or downgrade
    --force            Also replace a cargo build output or the same version
                 Only `update` uses the network (through curl), never on its own.
  hook           Internal: run by the hooks tower passes to the sessions it
                 starts, to list their subagents; prints nothing

Config: $XDG_CONFIG_HOME/watchtower/config.toml (default ~/.config/watchtower/config.toml)

Options:
  -V, --version  Print the version
  -h, --help     Print this help
";

/// The help text: [`USAGE`] followed by the config file this run would use
/// and whether it exists, when it can be determined (no `HOME`: `None`).
pub fn help(config: Option<&crate::config::ConfigSource>) -> String {
    let mut text = USAGE.to_owned();
    if let Some(source) = config {
        let state = if source.found {
            "found"
        } else {
            "not found — built-in defaults apply; `tower` asks and writes it when it starts"
        };
        text.push_str(&format!(
            "\nConfig in use: {} ({state})\n",
            crate::attach::printable(&source.shown(None))
        ));
    }
    text
}

/// Parses the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<S> = args.into_iter().collect();
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    match args.as_slice() {
        [] => Ok(Command::Run),
        ["--version" | "-V"] => Ok(Command::Version),
        ["--help" | "-h"] => Ok(Command::Help),
        ["setup"] => Ok(Command::Setup),
        ["setup", "--help" | "-h"] => Ok(Command::Help),
        ["setup", other, ..] => Err(format!("unknown argument `{other}` for `setup`")),
        ["status", rest @ ..] => parse_status(rest),
        ["update" | "--update", rest @ ..] => parse_update(rest),
        ["hook", rest @ ..] => Ok(parse_hook(rest)),
        ["dispatch", rest @ ..] => parse_dispatch(rest),
        ["wake", rest @ ..] => parse_wake(rest),
        [one] => Err(format!("unknown argument `{one}`")),
        _ => Err("too many arguments".to_owned()),
    }
}

fn parse_status(rest: &[&str]) -> Result<Command, String> {
    let mut json = false;
    for arg in rest {
        match *arg {
            "--json" if !json => json = true,
            "--json" => return Err("`--json` given twice".to_owned()),
            "--help" | "-h" => return Ok(Command::Help),
            other => return Err(format!("unknown argument `{other}` for `status`")),
        }
    }
    Ok(Command::Status { json })
}

/// `--flag value` or `--flag=value` for one of `flags`: the flag and its
/// value, the value taken from `rest` (advancing it) when not inline.
/// `None` when `arg` is none of the flags.
fn option_value<'a>(
    arg: &'a str,
    flags: &[&'static str],
    rest: &mut std::slice::Iter<'_, &'a str>,
) -> Option<Result<(&'static str, &'a str), String>> {
    for flag in flags {
        if arg == *flag {
            return Some(
                rest.next()
                    .map(|v| (*flag, *v))
                    .ok_or_else(|| format!("`{flag}` needs a value")),
            );
        }
        if let Some(value) = arg.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
            return Some(Ok((*flag, value)));
        }
    }
    None
}

/// Stores `value` in `slot`, refusing a second `flag`.
fn set_once(slot: &mut Option<String>, flag: &str, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("`{flag}` given twice"));
    }
    *slot = Some(value.to_owned());
    Ok(())
}

/// `tower dispatch` (doc/0044): options first; the first word that is not
/// an option, or everything after `--`, is the task.
fn parse_dispatch(rest: &[&str]) -> Result<Command, String> {
    const FLAGS: [&str; 6] = [
        "--project",
        "--account",
        "--name",
        "--model",
        "--effort",
        "--mode",
    ];
    let mut args = DispatchArgs::default();
    let mut project = None;
    let mut task: &[&str] = &[];
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match *arg {
            "--" => {
                task = iter.as_slice();
                break;
            }
            "--json" if !args.json => args.json = true,
            "--json" => return Err("`--json` given twice".to_owned()),
            "--help" | "-h" => return Ok(Command::Help),
            other => match option_value(other, &FLAGS, &mut iter) {
                Some(found) => {
                    let (flag, value) = found?;
                    let slot = match flag {
                        "--project" => &mut project,
                        "--account" => &mut args.account,
                        "--name" => &mut args.name,
                        "--model" => &mut args.model,
                        "--effort" => &mut args.effort,
                        _ => &mut args.mode,
                    };
                    set_once(slot, flag, value)?;
                }
                None if other.starts_with('-') => {
                    return Err(format!(
                        "unknown option `{other}` for `dispatch` (a task that starts with \
                         `-` goes after `--`)"
                    ));
                }
                None => {
                    // The task: this word and every one after it.
                    let at = rest.len() - iter.as_slice().len() - 1;
                    task = &rest[at..];
                    break;
                }
            },
        }
    }
    args.project = project.ok_or("`dispatch` needs `--project <dir>`")?;
    args.task = task.join(" ");
    if args.task.trim().is_empty() {
        return Err(
            "`dispatch` needs a task, e.g. `tower dispatch --project <dir> -- <task>`".to_owned(),
        );
    }
    Ok(Command::Dispatch(args))
}

/// `tower wake` (doc/0044): one session, options in any order before `--`;
/// everything after `--` is the text.
fn parse_wake(rest: &[&str]) -> Result<Command, String> {
    let mut args = WakeArgs::default();
    let mut session: Option<String> = None;
    let mut after_dashes: Option<&[&str]> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match *arg {
            "--" => {
                after_dashes = Some(iter.as_slice());
                break;
            }
            "--help" | "-h" => return Ok(Command::Help),
            other => match option_value(other, &["--account", "--text"], &mut iter) {
                Some(found) => {
                    let (flag, value) = found?;
                    let slot = if flag == "--account" {
                        &mut args.account
                    } else {
                        &mut args.text
                    };
                    set_once(slot, flag, value)?;
                }
                None if other.starts_with('-') => {
                    return Err(format!("unknown option `{other}` for `wake`"));
                }
                None if session.is_some() => {
                    return Err(format!(
                        "unexpected argument `{other}` for `wake`; pass the text with \
                         `--text <text>` or after `--`"
                    ));
                }
                None => session = Some(other.to_owned()),
            },
        }
    }
    if let Some(words) = after_dashes {
        if args.text.is_some() {
            return Err("give the text either with `--text` or after `--`, not both".to_owned());
        }
        if words.is_empty() {
            return Err("nothing after `--`; leave it out to wake without a text".to_owned());
        }
        args.text = Some(words.join(" "));
    }
    if args.text.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err("the text is empty; leave it out to wake without a text".to_owned());
    }
    args.session = session
        .filter(|s| !s.is_empty())
        .ok_or("`wake` needs a session: its short id or its name")?;
    Ok(Command::Wake(args))
}

/// Never an error: `tower hook` must stay silent (doc/0029).
fn parse_hook(rest: &[&str]) -> Command {
    match rest {
        [] => Command::Hook {
            dir: None,
            valid: true,
        },
        ["--dir", dir] if !dir.is_empty() => Command::Hook {
            dir: Some(std::path::PathBuf::from(dir)),
            valid: true,
        },
        _ => Command::Hook {
            dir: None,
            valid: false,
        },
    }
}

fn parse_update(rest: &[&str]) -> Result<Command, String> {
    use crate::update::Version;
    let (mut check, mut force) = (false, false);
    let mut version: Option<Version> = None;
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let value = match *arg {
            "--check" if !check => {
                check = true;
                continue;
            }
            "--force" if !force => {
                force = true;
                continue;
            }
            "--check" | "--force" => return Err(format!("`{arg}` given twice")),
            "--help" | "-h" => return Ok(Command::Help),
            "--version" => match args.next() {
                Some(value) => *value,
                None => return Err("`--version` needs a version, e.g. v1.2.3".to_owned()),
            },
            other => match other.strip_prefix("--version=") {
                Some(value) => value,
                None => return Err(format!("unknown argument `{other}` for `update`")),
            },
        };
        if version.is_some() {
            return Err("`--version` given twice".to_owned());
        }
        version = Some(
            Version::parse(value)
                .ok_or_else(|| format!("`{value}` is not a version (e.g. v1.2.3)"))?,
        );
    }
    if check && (version.is_some() || force) {
        return Err("`--check` takes no other options".to_owned());
    }
    Ok(Command::Update {
        check,
        version,
        force,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags() {
        assert_eq!(parse(Vec::<String>::new()), Ok(Command::Run));
        assert_eq!(parse(["-V"]), Ok(Command::Version));
        assert_eq!(parse(["--version"]), Ok(Command::Version));
        assert_eq!(parse(["--help"]), Ok(Command::Help));
        assert_eq!(parse(["-h"]), Ok(Command::Help));
        assert!(parse(["-V", "-h"]).is_err());
        assert!(parse(["stat"]).is_err());
    }

    #[test]
    fn parses_status() {
        assert_eq!(parse(["status"]), Ok(Command::Status { json: false }));
        assert_eq!(
            parse(["status", "--json"]),
            Ok(Command::Status { json: true })
        );
        assert_eq!(parse(["status", "--help"]), Ok(Command::Help));
        assert!(parse(["status", "--json", "--json"]).is_err());
        assert!(parse(["status", "--yaml"]).is_err());
        assert!(parse(["--json", "status"]).is_err());
    }

    #[test]
    fn parses_update() {
        use crate::update::Version;
        let plain = Command::Update {
            check: false,
            version: None,
            force: false,
        };
        assert_eq!(parse(["update"]), Ok(plain.clone()));
        assert_eq!(parse(["--update"]), Ok(plain));
        assert_eq!(
            parse(["update", "--version", "v1.2.3", "--force"]),
            Ok(Command::Update {
                check: false,
                version: Version::parse("1.2.3"),
                force: true,
            })
        );
        assert_eq!(
            parse(["--update", "--version=1.2.3"]),
            parse(["update", "--version", "v1.2.3"])
        );
        assert_eq!(parse(["update", "-h"]), Ok(Command::Help));
        assert!(parse(["update", "--version"]).is_err());
        assert!(parse(["update", "--version", "latest"]).is_err());
        assert!(parse(["update", "--check", "--force"]).is_err());
        assert!(parse(["update", "--check", "--check"]).is_err());
        assert!(parse(["update", "--now"]).is_err());
    }

    #[test]
    fn parses_hook() {
        assert_eq!(
            parse(["hook"]),
            Ok(Command::Hook {
                dir: None,
                valid: true
            })
        );
        assert_eq!(
            parse(["hook", "--dir", "/home/user/.config/watchtower"]),
            Ok(Command::Hook {
                dir: Some("/home/user/.config/watchtower".into()),
                valid: true
            })
        );
        for bad in [
            &["hook", "--dir"][..],
            &["hook", "--dir", ""],
            &["hook", "x"],
        ] {
            assert_eq!(
                parse(bad.iter().copied()),
                Ok(Command::Hook {
                    dir: None,
                    valid: false
                }),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parses_dispatch() {
        let Ok(Command::Dispatch(args)) = parse([
            "dispatch",
            "--project",
            "~/workspace/demo",
            "--account=alt",
            "--name",
            "n1",
            "--model",
            "opus",
            "--effort",
            "high",
            "--mode",
            "plan",
            "--json",
            "fix",
            "the",
            "test",
        ]) else {
            panic!("not a dispatch");
        };
        assert_eq!(
            args,
            DispatchArgs {
                project: "~/workspace/demo".into(),
                account: Some("alt".into()),
                name: Some("n1".into()),
                model: Some("opus".into()),
                effort: Some("high".into()),
                mode: Some("plan".into()),
                json: true,
                task: "fix the test".into(),
            }
        );
        // After `--` everything is the task, dashes included; options come
        // first, so a later `--json` is task text.
        let task = |words: &[&str]| match parse(words.iter().copied()) {
            Ok(Command::Dispatch(a)) => a.task,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            task(&["dispatch", "--project", "/p", "--", "-v", "--json"]),
            "-v --json"
        );
        assert_eq!(
            task(&["dispatch", "--project", "/p", "go", "--json"]),
            "go --json"
        );
        assert_eq!(parse(["dispatch", "--help"]), Ok(Command::Help));
        for bad in [
            &["dispatch"][..],
            &["dispatch", "--project"],
            &["dispatch", "--project", "/p"],
            &["dispatch", "--project", "/p", "--"],
            &["dispatch", "--project", "/p", "--", "  "],
            &["dispatch", "--project", "/p", "-v"],
            &["dispatch", "--project", "/p", "--json", "--json", "x"],
            &[
                "dispatch",
                "--name",
                "a",
                "--name",
                "b",
                "--project",
                "/p",
                "x",
            ],
            &["dispatch", "--task", "x", "--project", "/p"],
        ] {
            assert!(parse(bad.iter().copied()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_wake() {
        let wake = |words: &[&str]| parse(words.iter().copied());
        assert_eq!(
            wake(&["wake", "1a2b3c4d"]),
            Ok(Command::Wake(WakeArgs {
                session: "1a2b3c4d".into(),
                account: None,
                text: None,
            }))
        );
        assert_eq!(
            wake(&["wake", "--account", "alt", "fix-tests", "--text", "-x go"]),
            Ok(Command::Wake(WakeArgs {
                session: "fix-tests".into(),
                account: Some("alt".into()),
                text: Some("-x go".into()),
            }))
        );
        assert_eq!(
            wake(&["wake", "fix-tests", "--", "-v", "--help"]),
            Ok(Command::Wake(WakeArgs {
                session: "fix-tests".into(),
                account: None,
                text: Some("-v --help".into()),
            }))
        );
        assert_eq!(wake(&["wake", "-h"]), Ok(Command::Help));
        for bad in [
            &["wake"][..],
            &["wake", "a", "b"],
            &["wake", "a", "--text"],
            &["wake", "a", "--text", ""],
            &["wake", "a", "--text", "x", "--", "y"],
            &["wake", "a", "--"],
            &["wake", "a", "--account", "m", "--account", "n"],
            &["wake", "a", "--all"],
            &["wake", "--", "text"],
        ] {
            assert!(wake(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn setup_is_a_command_and_takes_only_help() {
        assert_eq!(parse(["setup"]), Ok(Command::Setup));
        assert_eq!(parse(["setup", "-h"]), Ok(Command::Help));
        assert!(parse(["setup", "--bogus"]).is_err());
        assert!(USAGE.contains("tower setup"));
    }

    #[test]
    fn help_names_the_config_in_use() {
        use crate::config::ConfigSource;
        use std::path::PathBuf;
        assert_eq!(help(None), USAGE);
        let path = PathBuf::from("/home/user/.config/watchtower/config.toml");
        let found = help(Some(&ConfigSource {
            path: path.clone(),
            found: true,
        }));
        assert!(found.starts_with(USAGE));
        assert!(
            found.ends_with("Config in use: /home/user/.config/watchtower/config.toml (found)\n"),
            "{found}"
        );
        let missing = help(Some(&ConfigSource { path, found: false }));
        assert!(
            missing.contains("config.toml (not found — built-in defaults apply"),
            "{missing}"
        );
    }
}
