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
}

/// Short usage text.
pub const USAGE: &str = "\
Usage: tower [--version | --help]
       tower status [--json]

A terminal UI over Claude Code background sessions.
Run without arguments to start it. Press ? inside for the keys.

Commands:
  status         Poll every account once and print the merged sessions
  status --json  The same as JSON, for scripts and the orchestrator

Config: $XDG_CONFIG_HOME/watchtower/config.toml (default ~/.config/watchtower/config.toml)

Options:
  -V, --version  Print the version
  -h, --help     Print this help
";

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
}
