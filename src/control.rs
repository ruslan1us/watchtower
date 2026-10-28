//! `tower dispatch` and `tower wake`: starting and waking background
//! sessions from the command line, for the user and for the orchestrator
//! when the user asks it to (doc/0044).
//!
//! Both take tower's own paths, never a raw `claude --bg` or `--resume`
//! typed by someone else:
//!
//! - [`dispatch`] builds the [`DispatchRequest`] the new-session dialog
//!   builds (doc/0008): the account's command (`default_account` unless
//!   `--account`), the config's model, effort and permission mode unless
//!   given, `--settings` with tower's subagent hooks while tracking is on
//!   (doc/0029), the Haiku warning as text. It runs [`dispatch::run`],
//!   records the owner (`started`) and the project in `recent_projects`,
//!   and prints the short id and name. An untrusted folder is no trust
//!   flow here: the exit code and the message say to start it once from
//!   tower.
//! - [`wake`] polls every account once as `tower status` does, finds the
//!   row and plans the wake ([`crate::wake::plan`]), compares the session's
//!   record in the state file right before it wakes it (doc/0017 section
//!   8), and runs [`crate::wake::run`].
//!
//! Messages go to `err` with a `tower: ` prefix, made printable; the result
//! line goes to `out`. The exit codes are the `EXIT_*` constants.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::args::{DispatchArgs, WakeArgs};
use crate::attach::printable;
use crate::cli::CliRunner;
use crate::config::{Account, Config};
use crate::dispatch::{self, DispatchError, DispatchRequest, Dispatched};
use crate::merge::EnvironReader;
use crate::paths::{self, Dirs};
use crate::sidecar::{self, AccountSource, Sidecar};
use crate::status;
use crate::subagents;
use crate::wake::{self, WakeError};

/// Success: the session started or woke under its own id.
pub const EXIT_OK: u8 = 0;
/// `claude` ran and failed, timed out, printed no id, or (wake) started a
/// copy.
pub const EXIT_FAILED: u8 = 1;
/// Bad arguments, an unusable config, or a request tower refuses before
/// running anything (no such folder, unknown account, invalid option).
pub const EXIT_USAGE: u8 = 2;
/// The account has not trusted the folder; nothing was started or woken.
pub const EXIT_UNTRUSTED: u8 = 3;
/// `tower wake` refused: no such session, several with the name, a live
/// process, an unknown or another owner, another tower's newer record, or
/// no account could be listed.
pub const EXIT_REFUSED: u8 = 4;

/// How often [`dispatch`] lists the account to learn the new session's
/// `sessionId`, and the pause between tries.
const ID_LOOKUPS: usize = 3;
const ID_LOOKUP_PAUSE: Duration = Duration::from_millis(300);

/// Writes `tower: <text>` to `err`, every line printable.
fn say(err: &mut dyn Write, text: &str) {
    let lines: Vec<String> = text.lines().map(printable).collect();
    let _ = writeln!(err, "tower: {}", lines.join("\ntower: "));
}

/// The account named `name`, or a message naming the configured ones.
fn account_named<'a>(config: &'a Config, name: &str) -> Result<&'a Account, String> {
    config.account(name).ok_or_else(|| {
        let names: Vec<&str> = config.accounts.iter().map(|a| a.name.as_str()).collect();
        format!(
            "no account named `{name}`; the configured accounts are {}",
            names.join(", ")
        )
    })
}

/// The project folder as given: `~` expanded, relative to the current
/// directory when relative. `..` is resolved through the file system.
pub fn project_path(given: &str, home: &Path) -> PathBuf {
    let expanded = paths::expand_tilde(given.trim(), home);
    let absolute = std::path::absolute(&expanded).unwrap_or(expanded);
    let dotted = absolute
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir));
    if dotted && let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }
    absolute
}

/// The `--json` output of a dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    /// The short id.
    pub id: String,
    /// The `sessionId`, when a listing right after the start showed it.
    pub session_id: Option<String>,
    /// The session name, when `backgrounded · …` printed one.
    pub name: Option<String>,
    /// The account it runs under.
    pub account: String,
}

/// The request `args` describe, with the config's defaults (doc/0008).
pub fn dispatch_request(
    config: &Config,
    dirs: &Dirs,
    args: &DispatchArgs,
) -> Result<DispatchRequest, String> {
    let account = match &args.account {
        Some(name) => account_named(config, name)?,
        None => config.default_account(),
    };
    let project = project_path(&args.project, &dirs.home);
    if !project.is_dir() {
        return Err(if project.exists() {
            format!("project {} is not a folder", project.display())
        } else {
            format!("project folder {} does not exist", project.display())
        });
    }
    let mut request = DispatchRequest::new(project, args.task.as_str(), account.clone());
    request.name = args.name.clone().filter(|n| !n.trim().is_empty());
    request.model = Some(args.model.clone().unwrap_or(config.default_model.clone()));
    request.effort = Some(args.effort.clone().unwrap_or(config.default_effort.clone()));
    request.permission_mode = args
        .mode
        .clone()
        .unwrap_or(config.default_permission_mode.clone());
    request.end_options = true;
    if config.track_subagents {
        request.settings = Some(subagents::settings_path(&dirs.config_dir));
    }
    Ok(request)
}

/// `tower dispatch` (doc/0044). See the module docs.
pub fn dispatch(
    dirs: &Dirs,
    config: &Config,
    args: &DispatchArgs,
    runner: &CliRunner,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let mut request = match dispatch_request(config, dirs, args) {
        Ok(request) => request,
        Err(e) => {
            say(err, &e);
            return EXIT_USAGE;
        }
    };
    if let Err(e) = request.validate() {
        say(err, &e.to_string());
        return EXIT_USAGE;
    }
    if let Some(warning) = request.haiku_warning() {
        say(err, &format!("warning: {warning}"));
    }
    if let Some(path) = &request.settings
        && let Err(e) = subagents::prepare_settings(path)
    {
        say(
            err,
            &format!("warning: {}", subagents::no_hooks_warning(&e)),
        );
        request.settings = None;
    }
    let started = match dispatch::run(runner, &request) {
        Ok(started) => started,
        Err(DispatchError::Untrusted { dir, message }) => {
            say(err, &message);
            say(
                err,
                &format!(
                    "{} has not trusted {}; start a session there once from tower (`n`, then \
                     `Ctrl+T` to trust the folder), then try again",
                    request.account.name,
                    dir.display()
                ),
            );
            return EXIT_UNTRUSTED;
        }
        Err(DispatchError::Invalid(e)) => {
            say(err, &e.to_string());
            return EXIT_USAGE;
        }
        Err(e) => {
            say(err, &e.to_string());
            return EXIT_FAILED;
        }
    };
    let session_id = lookup_session_id(runner, &request.account, &started.id);
    record_start(dirs, &request, &started, session_id.as_deref(), err);
    let line = if args.json {
        let json = Started {
            id: started.id.clone(),
            session_id,
            name: started.name.clone(),
            account: request.account.name.clone(),
        };
        serde_json::to_string(&json).expect("the result always serialises")
    } else {
        let name = started
            .name
            .as_deref()
            .map(|n| format!(" · {n}"))
            .unwrap_or_default();
        printable(&format!(
            "started {}{name} under {}",
            started.id, request.account.name
        ))
    };
    if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
        return EXIT_FAILED;
    }
    EXIT_OK
}

/// The `sessionId` of the row with short id `id`, from up to
/// [`ID_LOOKUPS`] listings of `account`; `None` when none shows it.
fn lookup_session_id(runner: &CliRunner, account: &Account, id: &str) -> Option<String> {
    for attempt in 0..ID_LOOKUPS {
        if attempt > 0 {
            std::thread::sleep(ID_LOOKUP_PAUSE);
        }
        let Ok(listing) = runner.list_sessions(account) else {
            continue;
        };
        if let Some(sid) = listing
            .sessions
            .iter()
            .find(|s| s.id.as_deref() == Some(id))
            .and_then(|s| s.session_id.clone())
        {
            return Some(sid);
        }
    }
    None
}

/// Records a started session as the dialog does ([`dispatch::record_dispatch`])
/// and, when its `sessionId` is known, moves the record there at once. A
/// state file that cannot be saved is a warning: the session runs anyway.
fn record_start(
    dirs: &Dirs,
    request: &DispatchRequest,
    started: &Dispatched,
    session_id: Option<&str>,
    err: &mut dyn Write,
) {
    let path = dirs.state_file();
    let mut loaded = Sidecar::load(&path);
    if let Some(w) = loaded.warning.take() {
        say(err, &format!("warning: {w}"));
    }
    let mut sidecar = loaded.sidecar;
    let mut changed = dispatch::record_dispatch(
        &mut sidecar,
        request,
        started,
        &dirs.home,
        sidecar::now_ms(),
    );
    if let Some(sid) = session_id {
        let row = crate::listing::Session {
            id: Some(started.id.clone()),
            session_id: Some(sid.to_owned()),
            ..Default::default()
        };
        changed |= sidecar.adopt_short_id_records([&row]);
    }
    if changed && let Err(e) = sidecar.save(&path) {
        say(
            err,
            &format!(
                "warning: cannot record the session in {}: {e}; tower will ask for its \
                 account once it stops",
                path.display()
            ),
        );
    }
}

/// `tower wake` (doc/0044). See the module docs.
pub fn wake(
    dirs: &Dirs,
    config: &Config,
    args: &WakeArgs,
    runner: &CliRunner,
    environ: &dyn EnvironReader,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let account = match args.account.as_deref().map(|n| account_named(config, n)) {
        Some(Err(e)) => {
            say(err, &e);
            return EXIT_USAGE;
        }
        Some(Ok(a)) => Some(a),
        None => None,
    };
    let text = match &args.text {
        Some(t) => match crate::reply::text_to_send(t) {
            Some(t) => Some(t),
            None => {
                say(
                    err,
                    "the text is empty; leave it out to wake without a text",
                );
                return EXIT_USAGE;
            }
        },
        None => None,
    };
    // Read before the listings, so a record another tower writes meanwhile
    // shows as a change right before the wake (doc/0017 section 8).
    let state_file = dirs.state_file();
    let (sidecar, warning) = status::read_sidecar(&state_file);
    if let Some(w) = warning {
        say(err, &w);
    }
    let probes = crate::usage::ProbeFolder::for_config_dir(&dirs.config_dir);
    let results = status::poll_all(&config.accounts, runner)
        .into_iter()
        .map(|result| {
            result.map(|mut listing| {
                probes.hide(&mut listing);
                listing
            })
        })
        .collect();
    let merged = status::merge(config, &dirs.home, results, &sidecar, environ);
    for w in &merged.warnings {
        say(err, w);
    }
    for e in &merged.errors {
        say(err, &format!("account `{}`: {}", e.account, e.error));
    }
    if merged.errors.len() == config.accounts.len() {
        say(err, "no account could be listed; nothing was woken");
        return EXIT_REFUSED;
    }
    let planned = wake::find(&merged.sessions, &args.session)
        .and_then(|m| wake::plan(config, m, account, text));
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => {
            say(err, &refusal);
            if !merged.errors.is_empty() {
                say(err, "(some accounts could not be listed; see above)");
            }
            return EXIT_REFUSED;
        }
    };
    let request = &plan.request;
    // The record as this run planned with: the file's, with a short-id
    // record moved to the sessionId as the merge saw it.
    let mut adopted = sidecar.clone();
    adopted.adopt_short_id_records(merged.sessions.iter().map(|m| &m.session));
    let recorded = adopted.owner_record(&request.session_id);
    let mut check = || -> Result<(), WakeError> {
        let Ok(now) = sidecar::read_owner(&state_file, &request.session_id) else {
            return Ok(());
        };
        if now == recorded {
            return Ok(());
        }
        match now.and_then(|r| r.account) {
            Some(account) if account != request.owner.name => {
                Err(WakeError::OwnerChanged { account })
            }
            _ => Ok(()),
        }
    };
    match wake::run(runner, request, &mut check) {
        Ok(woke) => {
            if !plan.owner_known {
                save_owner(
                    &state_file,
                    &request.session_id,
                    &request.owner.name,
                    AccountSource::Asked,
                    err,
                );
            }
            let name = woke.name.as_deref().unwrap_or(&request.name);
            let what = if request.text.is_some() {
                "with your text as its next prompt"
            } else {
                "waiting for your prompt"
            };
            let line = printable(&format!(
                "woke {} · {name} under {} · {what}",
                request.id, request.owner.name
            ));
            if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                return EXIT_FAILED;
            }
            EXIT_OK
        }
        Err(WakeError::Copied { new_id, notes }) => {
            save_owner(
                &state_file,
                &new_id,
                &request.owner.name,
                AccountSource::Started,
                err,
            );
            for n in &notes {
                say(err, n);
            }
            say(
                err,
                &format!(
                    "--resume started a copy {new_id} instead of waking {} ({}); the copy is \
                     left as it is",
                    request.name, request.id
                ),
            );
            EXIT_FAILED
        }
        Err(WakeError::Untrusted { dir, message }) => {
            say(err, &message);
            say(
                err,
                &format!(
                    "{} has not trusted {}; start a session there once from tower (`n`, then \
                     `Ctrl+T` to trust the folder), then try again",
                    request.owner.name,
                    dir.display()
                ),
            );
            EXIT_UNTRUSTED
        }
        Err(e @ WakeError::Invalid(_)) => {
            say(err, &e.to_string());
            EXIT_USAGE
        }
        Err(e @ (WakeError::CwdMissing(_) | WakeError::OwnerChanged { .. })) => {
            say(err, &e.to_string());
            EXIT_REFUSED
        }
        Err(e) => {
            say(err, &e.to_string());
            EXIT_FAILED
        }
    }
}

/// Records `account` as the owner of `key` in the state file, with a
/// warning when it cannot be saved.
fn save_owner(path: &Path, key: &str, account: &str, source: AccountSource, err: &mut dyn Write) {
    let mut loaded = Sidecar::load(path);
    if let Some(w) = loaded.warning.take() {
        say(err, &format!("warning: {w}"));
    }
    let mut sidecar = loaded.sidecar;
    if sidecar.record_owner(key, account, source)
        && let Err(e) = sidecar.save(path)
    {
        say(
            err,
            &format!(
                "warning: cannot record the owner in {}: {e}",
                path.display()
            ),
        );
    }
}
