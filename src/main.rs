//! `tower`: the Watchtower TUI. All logic lives in the library; this only
//! parses arguments, loads config and state, and starts the TUI.

use std::process::ExitCode;

use watchtower::args::{self, Command};
use watchtower::attach::printable;
use watchtower::config::Config;
use watchtower::paths::Dirs;
use watchtower::sidecar::Sidecar;

fn main() -> ExitCode {
    match args::parse(std::env::args().skip(1)) {
        Ok(Command::Run) => {}
        Ok(Command::Version) => {
            println!("tower {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Command::Help) => {
            print!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            eprint!("{}", args::USAGE);
            return ExitCode::from(2);
        }
    }

    let Some(dirs) = Dirs::from_env() else {
        eprintln!("tower: cannot determine the home directory (HOME is not set)");
        return ExitCode::FAILURE;
    };
    let config = match Config::load(&dirs) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            eprintln!(
                "tower: fix {} (or move it away to use the defaults) and start again",
                dirs.config_file().display()
            );
            return ExitCode::FAILURE;
        }
    };
    let loaded = Sidecar::load(&dirs.state_file());
    match watchtower::tui::run(config, dirs, loaded) {
        Ok(exit) => ExitCode::from(exit.status()),
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            ExitCode::FAILURE
        }
    }
}

/// An error message safe for the terminal: each line without control
/// characters (doc/0007), since it can quote an argument or a file.
fn shown(err: &dyn std::fmt::Display) -> String {
    let text = err.to_string();
    let lines: Vec<String> = text.lines().map(printable).collect();
    lines.join("\n")
}
