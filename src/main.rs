//! `tower`: the Watchtower TUI. All logic lives in the library; this only
//! parses arguments, loads config and state, and starts the TUI (in the
//! welcome window with no config file, or for `tower setup`), or runs
//! `tower status`, `tower dispatch`, `tower wake` or `tower update` without
//! it.

use std::process::ExitCode;

use watchtower::args::{self, Command};
use watchtower::attach::printable;
use watchtower::config::{Config, ConfigSource};
use watchtower::paths::Dirs;
use watchtower::sidecar::Sidecar;
use watchtower::update;
use watchtower::welcome::Origin;

fn main() -> ExitCode {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|a| a == "hook") {
        return hook(&raw);
    }
    let setup = match args::parse(std::env::args().skip(1)) {
        Ok(Command::Run) => false,
        Ok(Command::Setup) => true,
        // Handled above, before arguments that are not UTF-8 could panic.
        Ok(Command::Hook { .. }) => return ExitCode::SUCCESS,
        Ok(Command::Status { json }) => return status(json),
        Ok(Command::Dispatch(args)) => {
            return control(|dirs, config, runner, out, err| {
                watchtower::control::dispatch(dirs, config, &args, runner, out, err)
            });
        }
        Ok(Command::Wake(args)) => {
            return control(|dirs, config, runner, out, err| {
                let environ = watchtower::merge::ProcEnviron;
                watchtower::control::wake(dirs, config, &args, runner, &environ, out, err)
            });
        }
        Ok(Command::Update {
            check,
            version,
            force,
        }) => return update(check, version, force),
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
    };

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
    let welcome = Origin::for_start(setup, source.found);
    match watchtower::tui::run(config, source, dirs, loaded, welcome) {
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
    // A running tower's usage probe is no session (doc/0043).
    let probes = watchtower::usage::ProbeFolder::for_config_dir(&dirs.config_dir);
    let results = status::poll_all(&config.accounts, &watchtower::cli::CliRunner::new())
        .into_iter()
        .map(|result| {
            result.map(|mut listing| {
                probes.hide(&mut listing);
                listing
            })
        })
        .collect();
    let (mut report, warnings) = status::build(
        &config,
        &source,
        &dirs.home,
        results,
        &sidecar,
        &watchtower::merge::ProcEnviron,
        watchtower::sidecar::now_ms(),
    );
    if config.track_subagents {
        status::add_subagents(&mut report, &watchtower::subagents::dir(&dirs.config_dir));
    }
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

/// `tower dispatch` and `tower wake` (doc/0044): load the config, warn
/// about an inherited `CLAUDE_CONFIG_DIR`, and run `work` with no terminal.
/// A config error exits with [`watchtower::control::EXIT_USAGE`].
fn control(
    work: impl FnOnce(
        &Dirs,
        &Config,
        &watchtower::cli::CliRunner,
        &mut dyn std::io::Write,
        &mut dyn std::io::Write,
    ) -> u8,
) -> ExitCode {
    use watchtower::control::EXIT_USAGE;
    let Some(dirs) = Dirs::from_env() else {
        eprintln!("tower: cannot determine the home directory (HOME is not set)");
        return ExitCode::from(EXIT_USAGE);
    };
    let config = match Config::load(&dirs) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let inherited = std::env::var_os(watchtower::cli::CONFIG_DIR_VAR);
    if let Some(w) = watchtower::cli::inherited_config_dir_warning(inherited.as_deref()) {
        eprintln!("tower: {}", shown(&w));
    }
    let runner = watchtower::cli::CliRunner::new();
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    ExitCode::from(work(&dirs, &config, &runner, &mut out, &mut err))
}

/// `tower hook` (doc/0029): records one Claude Code hook payload from stdin
/// in tower's subagent files. Prints nothing, not even a panic, and always
/// exits 0, so it never changes what the session does; on any problem it
/// records nothing.
fn hook(raw: &[std::ffi::OsString]) -> ExitCode {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        let Some(words) = raw
            .iter()
            .map(|a| a.to_str())
            .collect::<Option<Vec<&str>>>()
        else {
            return;
        };
        let Ok(Command::Hook { dir, valid: true }) = args::parse(words) else {
            return;
        };
        let Some(dir) = dir.or_else(|| Dirs::from_env().map(|d| d.config_dir)) else {
            return;
        };
        if dir.is_absolute() {
            watchtower::subagents::run_hook(
                std::io::stdin().lock(),
                &dir,
                watchtower::sidecar::now_ms(),
            );
        }
    });
    ExitCode::SUCCESS
}

/// `tower update` (doc/0023). With `check`, prints whether a newer release
/// exists: exit 0 up to date, 10 available, 1 error. Otherwise replaces the
/// running binary: exit 0 when updated or already up to date, 1 on error.
fn update(check: bool, version: Option<update::Version>, force: bool) -> ExitCode {
    let updater = update::Updater::new();
    if check {
        return match updater.check() {
            Ok(result) => {
                println!("{}", result.line());
                ExitCode::from(result.exit_code())
            }
            Err(err) => {
                eprintln!("tower: {}", shown(&err));
                ExitCode::FAILURE
            }
        };
    }
    let result = update::current_target().and_then(|target| {
        let request = update::Request { version, force };
        updater.update(&target, &request, &mut std::io::stdout())
    });
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tower: {}", shown(&err));
            ExitCode::FAILURE
        }
    }
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
