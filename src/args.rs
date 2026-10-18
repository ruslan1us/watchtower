//! Command-line arguments of `tower` (milestone 1: no subcommands yet).

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start the TUI.
    Run,
    /// `--version` / `-V`.
    Version,
    /// `--help` / `-h`.
    Help,
}

/// Short usage text.
pub const USAGE: &str = "\
Usage: tower [--version | --help]

A terminal UI over Claude Code background sessions.
Run without arguments to start it. Press ? inside for the keys.

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
    match args.as_slice() {
        [] => Ok(Command::Run),
        [one] => match one.as_ref() {
            "--version" | "-V" => Ok(Command::Version),
            "--help" | "-h" => Ok(Command::Help),
            other => Err(format!("unknown argument `{other}`")),
        },
        _ => Err("too many arguments".to_owned()),
    }
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
        assert!(parse(["status"]).is_err());
        assert!(parse(["-V", "-h"]).is_err());
    }
}
