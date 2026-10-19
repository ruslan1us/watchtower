//! Command-line arguments of `tower`.

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start the TUI.
    Run,
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
}

/// Short usage text.
pub const USAGE: &str = "\
Usage: tower [--version | --help]
       tower status [--json]
       tower update [--check | --version vX.Y.Z] [--force]

A terminal UI over Claude Code background sessions.
Run without arguments to start it. Press ? inside for the keys.

Commands:
  status         Poll every account once and print the merged sessions
  status --json  The same as JSON, for scripts and the orchestrator
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
            "not found — built-in defaults apply; see config.example.toml"
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
        ["status", rest @ ..] => parse_status(rest),
        ["update" | "--update", rest @ ..] => parse_update(rest),
        ["hook", rest @ ..] => Ok(parse_hook(rest)),
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
