//! `tower`: the Watchtower TUI. All logic lives in the library; this only
//! parses arguments, loads config and state, and starts the TUI, or runs
//! `tower status` without it.

use std::process::ExitCode;

use watchtower::args::{self, Command};
use watchtower::attach::printable;
use watchtower::config::{Config, ConfigSource};
use watchtower::paths::Dirs;
use watchtower::sidecar::Sidecar;

fn main() -> ExitCode {
    match args::parse(std::env::args().skip(1)) {
        Ok(Command::Run) => {}
        Ok(Command::Status { json }) => return status(json),
        Ok(Command::Version) => {
            println!("tower {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Command::Help) => {
            print!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            eprint!("{}", help_text());
            return ExitCode::from(2);
        }
    }

    let Some(dirs) = Dirs::from_env() else {
        eprintln!("tower: cannot determine the home directory (HOME is not set)");
        return ExitCode::FAILURE;
    };
    let (config, source) = match Config::load_with_source(&dirs) {
        Ok(loaded) => loaded,
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
    match watchtower::tui::run(config, source, dirs, loaded) {
        Ok(exit) => ExitCode::from(exit.status()),
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            ExitCode::FAILURE
        }
    }
}

/// `tower status [--json]` (doc/0010): poll every account once, merge, print,
/// with no terminal and no sidecar writes. Exit 0 when at least one account
/// was listed, 1 when none was, 2 for a config error.
fn status(json: bool) -> ExitCode {
    use std::io::Write;
    use watchtower::status;

    let Some(dirs) = Dirs::from_env() else {
        eprintln!("tower: cannot determine the home directory (HOME is not set)");
        return ExitCode::from(status::EXIT_CONFIG);
    };
    let (config, source) = match Config::load_with_source(&dirs) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            return ExitCode::from(status::EXIT_CONFIG);
        }
    };
    let inherited = std::env::var_os(watchtower::cli::CONFIG_DIR_VAR);
    if let Some(w) = watchtower::cli::inherited_config_dir_warning(inherited.as_deref()) {
        eprintln!("tower: {}", shown(&w));
    }
    let (sidecar, warning) = status::read_sidecar(&dirs.state_file());
    if let Some(w) = warning {
        eprintln!("tower: {}", shown(&w));
    }
    let results = status::poll_all(&config.accounts, &watchtower::cli::CliRunner::new());
    let (report, warnings) = status::build(
        &config,
        &source,
        &dirs.home,
        results,
        &sidecar,
        &watchtower::merge::ProcEnviron,
        watchtower::sidecar::now_ms(),
    );
    for w in &warnings {
        eprintln!("tower: {}", shown(w));
    }
    let text = if json {
        status::to_json(&report)
    } else {
        for e in &report.errors {
            eprintln!(
                "tower: account `{}`: {}",
                shown(&e.account),
                shown(&e.error)
            );
        }
        status::to_table(&report, &dirs.home)
    };
    let mut out = std::io::stdout().lock();
    if let Err(err) = out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        if err.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!("tower: cannot write the status: {err}");
        }
        return ExitCode::FAILURE;
    }
    ExitCode::from(status::exit_code(&config, &report))
}

/// The help text with the config file this run would use (doc/0002).
fn help_text() -> String {
    let source = Dirs::from_env().map(|dirs| ConfigSource::probe(&dirs.config_file()));
    args::help(source.as_ref())
}

/// An error message safe for the terminal: each line without control
/// characters (doc/0007), since it can quote an argument or a file.
fn shown(err: &dyn std::fmt::Display) -> String {
    let text = err.to_string();
    let lines: Vec<String> = text.lines().map(printable).collect();
    lines.join("\n")
}
