//! The subagents of sessions tower started (doc/0029).
//!
//! `claude agents --json` has no subagent data, so tower passes Claude Code
//! three observe-only hooks with `--settings` on the sessions it starts
//! (the new-session dialog's dispatch and the orchestrator's start). The
//! pieces:
//!
//! - The settings file, `<config dir>/claude-settings.json`
//!   ([`settings_json`], [`ensure_settings`]): only the hooks
//!   `SubagentStart`, `SubagentStop` and `PostToolUse` (matcher `Agent`),
//!   each running [`hook_command`], i.e. `<running tower> hook --dir <config
//!   dir>`. Written atomically, and only when its content differs.
//! - The hook, `tower hook` ([`run_hook`]): reads one payload from stdin
//!   (at most [`MAX_PAYLOAD`] bytes), turns it into an [`Event`] ([`parse`])
//!   and records it in the session's own file
//!   `<config dir>/subagents/<sessionId>.json` under an exclusive lock with
//!   a short wait and an atomic write ([`record`]). It never prints and
//!   never fails: any error means nothing is recorded.
//! - Reading ([`read`], [`read_many`], [`prune`]): the TUI and `tower
//!   status` read the files of listed sessions; the agents count only while
//!   the session's row is live and `working` ([`is_current`]).
//!
//! Only these files are read or written: never a Claude Code file, never a
//! transcript (doc/0001, doc/0016). The payload's other fields
//! (`transcript_path`, prompts, messages) are ignored and never stored.

use std::collections::{HashMap, HashSet};
use std::fs::DirBuilder;
use std::io::{self, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::listing::{Session, State};
use crate::sidecar;

/// The folder under tower's config dir that holds one file per session.
pub const DIR_NAME: &str = "subagents";

/// The settings file passed with `--settings`, in tower's config dir.
pub const SETTINGS_FILE: &str = "claude-settings.json";

/// The lock file in [`DIR_NAME`], held around every read-modify-write and
/// the pruning. Never removed.
pub const LOCK_FILE: &str = ".lock";

/// The most bytes of stdin the hook reads; a larger payload is ignored.
pub const MAX_PAYLOAD: usize = 1 << 20;

/// The most bytes of an oversized payload the hook drains before it exits,
/// so the writer does not see a broken pipe for an ordinary large message.
const MAX_DRAIN: u64 = 16 << 20;

/// The most agents one file keeps; beyond it the oldest is dropped.
pub const MAX_AGENTS: usize = 64;

/// The most recently stopped agent ids one file remembers, so a launch
/// reported after its agent already stopped is not taken as running. The
/// same number of stopped agents keep their type, description and
/// background flag for a continuation (doc/0035).
pub const MAX_STOPPED: usize = 64;

/// The longest description kept, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 120;

/// The longest agent type kept, in characters.
pub const MAX_TYPE_CHARS: usize = 64;

/// A file larger than this is not read (it is not one tower wrote).
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// How long the hook waits for the lock before it gives up.
pub const LOCK_WAIT: Duration = Duration::from_secs(2);

/// The `timeout` of each hook in the settings file, in seconds.
pub const HOOK_TIMEOUT_SECS: u64 = 10;

/// A file of a session no full listing shows is pruned once it was last
/// written this long ago (the age doc/0017 gives a dispatch whose row never
/// appeared).
pub const PRUNE_UNLISTED_MS: i64 = sidecar::PRUNE_UNSTARTED_MS;

/// Any file last written this long ago is pruned (doc/0017's 14 days).
pub const PRUNE_OLD_MS: i64 = sidecar::PRUNE_UNSEEN_MS;

/// The file format version.
pub const VERSION: u32 = 1;

/// The status of a recorded agent; only running agents are kept.
pub const RUNNING: &str = "running";

/// `<config dir>/subagents`.
pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join(DIR_NAME)
}

/// `<config dir>/claude-settings.json`.
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE)
}

/// `<dir>/<session_id>.json`, or `None` when `session_id` is not a
/// lowercase UUID (so no file name can leave the folder).
pub fn file_path(dir: &Path, session_id: &str) -> Option<PathBuf> {
    is_session_id(session_id).then(|| dir.join(format!("{session_id}.json")))
}

/// Whether `s` is a UUID as `claude agents --json` prints `sessionId`:
/// 8-4-4-4-12 lowercase hex digits.
pub fn is_session_id(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Whether `s` is an agent id: 1 to 64 lowercase ASCII letters or digits.
pub fn is_agent_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// `text` as one printable line: control characters become spaces, runs of
/// whitespace one space, bidirectional and zero-width formatting characters
/// are dropped, trimmed, at most `max` characters (an ellipsis
/// marks a cut). `None` when nothing is left.
pub fn clean(text: &str, max: usize) -> Option<String> {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        if is_format_char(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if out.is_empty() {
        return None;
    }
    if out.chars().count() > max {
        let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
        cut.truncate(cut.trim_end().len());
        cut.push('…');
        out = cut;
    }
    Some(out)
}

/// Bidirectional and zero-width formatting characters, which could reorder
/// or hide text in the terminal.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
}

// ---- the settings file ------------------------------------------------------

/// The path of the running `tower` binary. After `tower update` replaced
/// it, Linux reports the old file as `<path> (deleted)`; the suffix is
/// dropped when the path itself exists again (the new binary).
pub fn running_exe() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let text = exe.to_string_lossy();
    if let Some(stripped) = text.strip_suffix(" (deleted)") {
        let path = PathBuf::from(stripped);
        if path.is_file() {
            return Ok(path);
        }
    }
    Ok(exe)
}

/// The hook's shell command: `<exe> hook --dir <config dir>`, each path one
/// shell word (single-quoted when it holds anything but plain path
/// characters). Refused for a relative path, a path that is not UTF-8, or
/// one with control characters, which cannot be passed safely.
pub fn hook_command(exe: &Path, config_dir: &Path) -> Result<String, String> {
    let word = |what: &str, path: &Path| -> Result<String, String> {
        let text = path
            .to_str()
            .ok_or_else(|| format!("the {what} path is not UTF-8"))?;
        if !path.is_absolute() {
            return Err(format!("the {what} path {text:?} is not absolute"));
        }
        if text.chars().any(|c| c.is_control()) {
            return Err(format!("the {what} path {text:?} holds control characters"));
        }
        Ok(crate::orchestrator::shell_word(text))
    };
    Ok(format!(
        "{} hook --dir {}",
        word("tower binary", exe)?,
        word("config", config_dir)?
    ))
}

/// The settings file's content: only the three hooks, each running
/// `command` ([`hook_command`]). Nothing else is set.
pub fn settings_json(command: &str) -> String {
    let hook = json!([{
        "type": "command",
        "command": command,
        "timeout": HOOK_TIMEOUT_SECS,
    }]);
    let value = json!({
        "hooks": {
            "SubagentStart": [{ "hooks": hook }],
            "SubagentStop": [{ "hooks": hook }],
            "PostToolUse": [{ "matcher": "Agent", "hooks": hook }],
        }
    });
    serde_json::to_string_pretty(&value).expect("the settings always serialise")
}

/// Writes the settings file `settings` (in tower's config dir, its parent)
/// for the hook binary `exe`, atomically and only when its content differs.
/// Creates the config dir (mode `0700`) when missing. Returns whether it
/// wrote; an error says why the file cannot be used.
pub fn ensure_settings(settings: &Path, exe: &Path) -> Result<bool, String> {
    let config_dir = settings
        .parent()
        .ok_or_else(|| format!("{} has no folder", settings.display()))?;
    let command = hook_command(exe, config_dir)?;
    let mut text = settings_json(&command);
    text.push('\n');
    if std::fs::read(settings).is_ok_and(|old| old == text.as_bytes()) {
        return Ok(false);
    }
    if !config_dir.is_dir() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(config_dir)
            .map_err(|e| format!("cannot create {}: {e}", config_dir.display()))?;
    }
    // write_atomic adds the final newline.
    sidecar::write_atomic(
        settings,
        Some(config_dir),
        &text.as_bytes()[..text.len() - 1],
    )
    .map_err(|e| format!("cannot write {}: {e}", settings.display()))?;
    Ok(true)
}

/// Makes the settings file `settings` current for the running binary
/// ([`running_exe`], [`ensure_settings`]). An error says why the file
/// cannot be used; the session then starts without `--settings`.
pub fn prepare_settings(settings: &Path) -> Result<(), String> {
    let exe = running_exe().map_err(|e| format!("cannot find the running tower binary: {e}"))?;
    ensure_settings(settings, &exe).map(|_| ())
}

/// The warning for a settings file that cannot be used (`err` from
/// [`prepare_settings`]). The same text each time, so the status line shows
/// it once ([`crate::app::App::add_warning`]).
pub fn no_hooks_warning(err: &str) -> String {
    format!("subagents: {err}; sessions start without the subagent hooks")
}

// ---- the file format -------------------------------------------------------

/// One session's file (doc/0029).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    /// [`VERSION`].
    #[serde(default)]
    pub version: u32,
    /// The running agents, oldest first.
    #[serde(default)]
    pub agents: Vec<AgentRecord>,
    /// Ids of agents seen stopping, newest last, at most [`MAX_STOPPED`].
    #[serde(default)]
    pub stopped: Vec<String>,
    /// What the agents seen stopping were, newest last, at most
    /// [`MAX_STOPPED`], so one continued with `SendMessage` shows its
    /// description again (doc/0035). Optional and left out when empty: a
    /// reader without the field (tower 0.4.x) ignores it, since unknown
    /// fields are ignored, and a file without it reads as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopped_agents: Vec<StoppedAgent>,
}

/// A stopped agent as the file remembers it (doc/0035).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedAgent {
    /// The agent id.
    pub id: String,
    /// The agent type; empty while unknown.
    #[serde(rename = "type", default)]
    pub agent_type: String,
    /// The description, when known.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether it ran in the background.
    #[serde(default)]
    pub background: bool,
}

/// One running agent as the file stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    /// The agent id (`agent_id` of the hooks).
    pub id: String,
    /// The agent type, e.g. `general-purpose`; empty while unknown.
    #[serde(rename = "type", default)]
    pub agent_type: String,
    /// The description the parent gave the `Agent` call, when known.
    #[serde(default)]
    pub description: Option<String>,
    /// Unix ms when tower's hook first saw the agent.
    pub started_ms: i64,
    /// [`RUNNING`].
    #[serde(default)]
    pub status: String,
    /// Launched in the background (`PostToolUse` said `async_launched`), so
    /// a `SubagentStop`'s `background_tasks` lists it while it runs.
    #[serde(default)]
    pub background: bool,
}

/// What one hook call reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `SubagentStart`.
    Start { id: String, agent_type: String },
    /// `PostToolUse` of `Agent` with `status: async_launched`.
    Launched {
        id: String,
        agent_type: Option<String>,
        description: Option<String>,
    },
    /// `PostToolUse` of `Agent` with any other status: a foreground agent
    /// that has finished.
    Finished { id: String },
    /// `SubagentStop`, with the snapshot of `background_tasks` when given.
    Stop {
        id: String,
        tasks: Option<Vec<Task>>,
    },
}

/// One entry of a `SubagentStop`'s `background_tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    /// False for a task whose status says it ended.
    pub running: bool,
}

/// The payload fields tower looks at; every other field (transcript
/// paths, prompts, messages) is ignored and never stored.
#[derive(Debug, Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<Value>,
    tool_response: Option<Value>,
    background_tasks: Option<Value>,
}

fn text_field<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a str> {
    value?.get(key)?.as_str()
}

/// Task statuses that mean the task is no longer running.
const ENDED: [&str; 7] = [
    "completed",
    "failed",
    "killed",
    "stopped",
    "cancelled",
    "canceled",
    "error",
];

fn parse_tasks(value: &Value) -> Option<Vec<Task>> {
    let tasks = value.as_array()?;
    Some(
        tasks
            .iter()
            .filter_map(|t| {
                let id = t.get("id")?.as_str()?;
                if !is_agent_id(id) {
                    return None;
                }
                let status = t.get("status").and_then(Value::as_str);
                Some(Task {
                    id: id.to_owned(),
                    agent_type: t
                        .get("agent_type")
                        .and_then(Value::as_str)
                        .and_then(|s| clean(s, MAX_TYPE_CHARS)),
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .and_then(|s| clean(s, MAX_DESCRIPTION_CHARS)),
                    running: !status.is_some_and(|s| ENDED.contains(&s)),
                })
            })
            .collect(),
    )
}

/// Reads a hook payload: its session id (a lowercase UUID) and the event.
/// `None` for anything else: another event or tool, invalid ids, an
/// internal helper's start without a type, garbage.
pub fn parse(payload: &[u8]) -> Option<(String, Event)> {
    let p: Payload = serde_json::from_slice(payload).ok()?;
    let session = p.session_id.filter(|s| is_session_id(s))?;
    let agent_id = || p.agent_id.clone().filter(|id| is_agent_id(id));
    let event = match p.hook_event_name.as_deref()? {
        "SubagentStart" => {
            let agent_type = clean(p.agent_type.as_deref().unwrap_or(""), MAX_TYPE_CHARS)?;
            Event::Start {
                id: agent_id()?,
                agent_type,
            }
        }
        "SubagentStop" => Event::Stop {
            id: agent_id()?,
            tasks: p.background_tasks.as_ref().and_then(parse_tasks),
        },
        "PostToolUse" if p.tool_name.as_deref() == Some("Agent") => {
            let response = p.tool_response.as_ref()?;
            let id = text_field(Some(response), "agentId")
                .or_else(|| text_field(Some(response), "agent_id"))
                .filter(|id| is_agent_id(id))?
                .to_owned();
            match text_field(Some(response), "status")? {
                "async_launched" => {
                    let input = p.tool_input.as_ref();
                    let description = text_field(input, "description")
                        .or_else(|| text_field(Some(response), "description"))
                        .and_then(|s| clean(s, MAX_DESCRIPTION_CHARS));
                    let agent_type =
                        text_field(input, "subagent_type").and_then(|s| clean(s, MAX_TYPE_CHARS));
                    Event::Launched {
                        id,
                        agent_type,
                        description,
                    }
                }
                _ => Event::Finished { id },
            }
        }
        _ => return None,
    };
    Some((session, event))
}

impl SessionFile {
    fn position(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|a| a.id == id)
    }

    fn was_stopped(&self, id: &str) -> bool {
        self.stopped.iter().any(|s| s == id)
    }

    /// Remembers `record` as stopped: its id in `stopped`, its type,
    /// description and background flag in `stopped_agents`.
    fn push_stopped(&mut self, record: AgentRecord) {
        self.stopped.retain(|s| *s != record.id);
        self.stopped.push(record.id.clone());
        let extra = self.stopped.len().saturating_sub(MAX_STOPPED);
        self.stopped.drain(..extra);
        self.stopped_agents.retain(|s| s.id != record.id);
        self.stopped_agents.push(StoppedAgent {
            id: record.id,
            agent_type: record.agent_type,
            description: record.description,
            background: record.background,
        });
        let extra = self.stopped_agents.len().saturating_sub(MAX_STOPPED);
        self.stopped_agents.drain(..extra);
    }

    /// Forgets that `id` stopped, returning what was remembered of it.
    fn take_stopped(&mut self, id: &str) -> Option<StoppedAgent> {
        self.stopped.retain(|s| s != id);
        let i = self.stopped_agents.iter().position(|s| s.id == id)?;
        Some(self.stopped_agents.remove(i))
    }

    /// Fills a missing description or type of agents in the file from the
    /// running entries of a `background_tasks` snapshot. Drops nothing.
    fn fill_from(&mut self, running: &HashMap<&str, &Task>) -> bool {
        let mut changed = false;
        for a in &mut self.agents {
            let Some(t) = running.get(a.id.as_str()) else {
                continue;
            };
            if a.description.is_none() && t.description.is_some() {
                a.description = t.description.clone();
                changed = true;
            }
            if a.agent_type.is_empty()
                && let Some(ty) = &t.agent_type
            {
                a.agent_type = ty.clone();
                changed = true;
            }
        }
        changed
    }

    fn add(&mut self, record: AgentRecord) {
        self.agents.push(record);
        while self.agents.len() > MAX_AGENTS {
            let oldest = (0..self.agents.len())
                .min_by_key(|&i| self.agents[i].started_ms)
                .unwrap_or(0);
            self.agents.remove(oldest);
        }
    }

    /// Applies one hook event. Returns whether anything changed.
    ///
    /// - A start or launch adds the agent, or fills in its type and
    ///   description. A launch for an agent seen stopping is ignored (it
    ///   was reported after its agent ended).
    /// - A start for an agent seen stopping is a continuation (`SendMessage`
    ///   resumes a stopped agent under the same id, doc/0035): the agent
    ///   runs again from `now_ms`, with the description and background flag
    ///   it had when it stopped. Claude Code runs `SubagentStart` hooks
    ///   before the agent's turn, so a start never follows the stop of the
    ///   same run.
    /// - A stop of an agent not in the file drops nothing (Claude Code's own
    ///   helpers stop without a start). Otherwise the agent is dropped, and
    ///   when the payload lists `background_tasks`, background agents not
    ///   running there are dropped too.
    /// - Either way, agents in the file running in `background_tasks` get
    ///   a missing description and type from it.
    /// - A finished foreground agent is dropped.
    pub fn apply(&mut self, event: Event, now_ms: i64) -> bool {
        match event {
            Event::Start { id, agent_type } => {
                if self.was_stopped(&id) {
                    let before = self.take_stopped(&id);
                    if let Some(i) = self.position(&id) {
                        // Not expected: a stopped agent is never in `agents`.
                        self.agents.remove(i);
                    }
                    let (description, background) = before
                        .map(|s| (s.description, s.background))
                        .unwrap_or_default();
                    self.add(AgentRecord {
                        id,
                        agent_type,
                        description,
                        started_ms: now_ms,
                        status: RUNNING.to_owned(),
                        background,
                    });
                    return true;
                }
                match self.position(&id) {
                    Some(i) if self.agents[i].agent_type.is_empty() => {
                        self.agents[i].agent_type = agent_type;
                        true
                    }
                    Some(_) => false,
                    None => {
                        self.add(AgentRecord {
                            id,
                            agent_type,
                            description: None,
                            started_ms: now_ms,
                            status: RUNNING.to_owned(),
                            background: false,
                        });
                        true
                    }
                }
            }
            Event::Launched {
                id,
                agent_type,
                description,
            } => {
                if self.was_stopped(&id) {
                    return false;
                }
                match self.position(&id) {
                    Some(i) => {
                        let a = &mut self.agents[i];
                        let before = a.clone();
                        a.background = true;
                        if description.is_some() {
                            a.description = description;
                        }
                        if a.agent_type.is_empty()
                            && let Some(t) = agent_type
                        {
                            a.agent_type = t;
                        }
                        *a != before
                    }
                    None => {
                        self.add(AgentRecord {
                            id,
                            agent_type: agent_type.unwrap_or_default(),
                            description,
                            started_ms: now_ms,
                            status: RUNNING.to_owned(),
                            background: true,
                        });
                        true
                    }
                }
            }
            Event::Finished { id } => {
                let Some(i) = self.position(&id) else {
                    return false;
                };
                let record = self.agents.remove(i);
                self.push_stopped(record);
                true
            }
            Event::Stop { id, tasks } => {
                let known = self.position(&id).map(|i| self.agents.remove(i));
                let stopped = known.is_some();
                if let Some(record) = known {
                    self.push_stopped(record);
                }
                let Some(tasks) = tasks else {
                    return stopped;
                };
                let running: HashMap<&str, &Task> = tasks
                    .iter()
                    .filter(|t| t.running)
                    .map(|t| (t.id.as_str(), t))
                    .collect();
                if stopped {
                    self.agents
                        .retain(|a| !a.background || running.contains_key(a.id.as_str()));
                }
                let filled = self.fill_from(&running);
                stopped || filled
            }
        }
    }
}

/// Reads a session file; a missing, oversized or unparsable one is `None`.
fn load(path: &Path) -> Option<SessionFile> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Records `event` for `session_id` in `dir` ([`DIR_NAME`]): creates the
/// folder (mode `0700`) when missing, takes [`LOCK_FILE`] for at most
/// [`LOCK_WAIT`], re-reads the file (a missing or corrupt one counts as
/// empty), applies the event and writes the file atomically when it
/// changed.
pub fn record(dir: &Path, session_id: &str, event: Event, now_ms: i64) -> io::Result<()> {
    let path = file_path(dir, session_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a session id"))?;
    if !dir.is_dir() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let _lock = sidecar::lock(&dir.join(LOCK_FILE), LOCK_WAIT)?;
    let mut file = load(&path).unwrap_or_default();
    file.version = VERSION;
    if !file.apply(event, now_ms) {
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    sidecar::write_atomic(&path, Some(dir), &json)
}

/// Reads at most [`MAX_PAYLOAD`] bytes. `None` when there is more (the rest
/// is drained, up to a limit, and the payload ignored) or reading fails.
pub fn read_payload(input: impl Read) -> Option<Vec<u8>> {
    let mut input = input;
    let mut bytes = Vec::new();
    (&mut input)
        .take(MAX_PAYLOAD as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PAYLOAD {
        let _ = io::copy(&mut input.take(MAX_DRAIN), &mut io::sink());
        return None;
    }
    Some(bytes)
}

/// `tower hook`: reads one payload from `input` and records it under
/// `config_dir` ([`record`]). Every error is swallowed: the hook must not
/// print, fail or change what Claude Code does.
pub fn run_hook(input: impl Read, config_dir: &Path, now_ms: i64) {
    let Some(payload) = read_payload(input) else {
        return;
    };
    let Some((session, event)) = parse(&payload) else {
        return;
    };
    let _ = record(&dir(config_dir), &session, event, now_ms);
}

// ---- reading ----------------------------------------------------------------

/// A running subagent of a session, as tower shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subagent {
    /// The agent id.
    pub id: String,
    /// The agent type, e.g. `general-purpose`; empty while unknown.
    #[serde(rename = "type")]
    pub agent_type: String,
    /// The description given to the `Agent` call, when known.
    pub description: Option<String>,
    /// Unix ms when tower's hook first saw it.
    #[serde(rename = "startedAt")]
    pub started_ms: i64,
}

/// Whether a row's subagents count: its process is live and its state is
/// `working`. A parent stays `working` while its subagents run (reference
/// doc), so in any other state the file is stale.
pub fn is_current(session: &Session) -> bool {
    is_current_state(session.has_live_process(), session.state.as_ref())
}

/// [`is_current`] from its parts: whether the row has a live process
/// (`pid` or `status`) and its `state`.
pub fn is_current_state(live: bool, state: Option<&State>) -> bool {
    live && state == Some(&State::Working)
}

/// The running agents in the file of `session_id`, oldest first; none when
/// the id is invalid or the file is missing or unreadable. Strings are
/// cleaned again, since the file is only a file.
pub fn read(dir: &Path, session_id: &str) -> Vec<Subagent> {
    let Some(file) = file_path(dir, session_id).and_then(|p| load(&p)) else {
        return Vec::new();
    };
    let mut agents: Vec<Subagent> = file
        .agents
        .into_iter()
        .filter(|a| a.status == RUNNING && is_agent_id(&a.id))
        .take(MAX_AGENTS)
        .map(|a| Subagent {
            id: a.id,
            agent_type: clean(&a.agent_type, MAX_TYPE_CHARS).unwrap_or_default(),
            description: a
                .description
                .as_deref()
                .and_then(|d| clean(d, MAX_DESCRIPTION_CHARS)),
            started_ms: a.started_ms,
        })
        .collect();
    agents.sort_by_key(|a| a.started_ms);
    agents
}

/// [`read`] for every id; ids without running agents are left out.
pub fn read_many<'a>(
    dir: &Path,
    session_ids: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, Vec<Subagent>> {
    session_ids
        .into_iter()
        .filter_map(|id| {
            let agents = read(dir, id);
            (!agents.is_empty()).then(|| (id.to_owned(), agents))
        })
        .collect()
}

/// The subagents of each listed session whose row is current
/// ([`is_current`]), keyed by `sessionId`.
pub fn read_current<'a>(
    dir: &Path,
    sessions: impl IntoIterator<Item = &'a Session>,
) -> HashMap<String, Vec<Subagent>> {
    read_many(
        dir,
        sessions
            .into_iter()
            .filter(|s| is_current(s))
            .filter_map(|s| s.session_id.as_deref()),
    )
}

/// One read of the TUI, run on a worker thread after a poll: the files of
/// the sessions whose rows are current, and now and then a [`prune`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadJob {
    /// The [`DIR_NAME`] folder.
    pub dir: PathBuf,
    /// `sessionId`s of the current rows ([`is_current`]).
    pub current: Vec<String>,
    /// When set, prune with these listed `sessionId`s and whether every
    /// account's latest poll was a full listing.
    pub prune: Option<(HashSet<String>, bool)>,
}

impl ReadJob {
    /// Prunes when asked, then reads ([`read_many`]).
    pub fn run(&self, now: SystemTime) -> HashMap<String, Vec<Subagent>> {
        if let Some((listed, complete)) = &self.prune {
            prune(&self.dir, listed, *complete, now);
        }
        read_many(&self.dir, self.current.iter().map(String::as_str))
    }
}

/// Removes stale files from `dir`, under [`LOCK_FILE`] (skipped when it is
/// busy): a session file last written [`PRUNE_OLD_MS`] ago; one of a
/// session not in `listed`, when `complete` (every account's latest poll
/// was a full listing, doc/0017) and it was last written
/// [`PRUNE_UNLISTED_MS`] ago; and temp files left by a crashed write after
/// [`PRUNE_UNLISTED_MS`]. Returns how many files went.
pub fn prune(dir: &Path, listed: &HashSet<String>, complete: bool, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let Ok(_lock) = sidecar::lock(&dir.join(LOCK_FILE), Duration::ZERO) else {
        return 0;
    };
    let age_ms = |meta: &std::fs::Metadata| -> Option<i64> {
        let modified = meta.modified().ok()?;
        let age = now.duration_since(modified).unwrap_or_default();
        Some(i64::try_from(age.as_millis()).unwrap_or(i64::MAX))
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(age) = age_ms(&meta) else { continue };
        let stale = if let Some(id) = name.strip_suffix(".json") {
            is_session_id(id)
                && (age >= PRUNE_OLD_MS
                    || (complete && !listed.contains(id) && age >= PRUNE_UNLISTED_MS))
        } else {
            name.split_once(".json.tmp.")
                .is_some_and(|(id, _)| is_session_id(id))
                && age >= PRUNE_UNLISTED_MS
        };
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert!(is_session_id("1a2b3c4d-0000-4000-8000-000000000000"));
        assert!(!is_session_id("1A2B3C4D-0000-4000-8000-000000000000"));
        assert!(!is_session_id("../../etc/passwd"));
        assert!(!is_session_id("1a2b3c4d-0000-4000-8000-00000000000"));
        assert!(!is_session_id("1a2b3c4d"));
        assert!(is_agent_id("a0b1c2d3e4f506172"));
        assert!(!is_agent_id(""));
        assert!(!is_agent_id("A1"));
        assert!(!is_agent_id("a/b"));
        assert!(!is_agent_id(&"a".repeat(65)));
    }

    #[test]
    fn cleaning() {
        assert_eq!(clean("  a\n\tb  ", 10).as_deref(), Some("a b"));
        assert_eq!(clean("\u{1b}[31mred", 10).as_deref(), Some("[31mred"));
        assert_eq!(clean("a\u{202e}b", 10).as_deref(), Some("ab"));
        assert_eq!(clean(" \n ", 10), None);
        assert_eq!(clean("abcdefghij", 5).as_deref(), Some("abcd…"));
    }

    fn record(id: &str, description: Option<&str>) -> AgentRecord {
        AgentRecord {
            id: id.into(),
            agent_type: "general-purpose".into(),
            description: description.map(Into::into),
            started_ms: 1,
            status: RUNNING.into(),
            background: true,
        }
    }

    #[test]
    fn stopped_memory_is_pushed_and_taken_together() {
        let mut f = SessionFile::default();
        f.push_stopped(record("a1", Some("one")));
        f.push_stopped(record("b2", None));
        // Stopping again moves it to the end, once.
        f.push_stopped(record("a1", Some("uno")));
        assert_eq!(f.stopped, ["b2", "a1"]);
        let memory: Vec<_> = f.stopped_agents.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(memory, ["b2", "a1"]);
        let a1 = f.take_stopped("a1").unwrap();
        assert_eq!(a1.description.as_deref(), Some("uno"));
        assert!(a1.background);
        assert_eq!(f.stopped, ["b2"]);
        // An id only in `stopped` (a file of the old format) is forgotten
        // there too.
        f.stopped.push("c3".into());
        assert_eq!(f.take_stopped("c3"), None);
        assert_eq!(f.stopped, ["b2"]);
    }

    #[test]
    fn a_continuation_replaces_a_stale_running_copy() {
        let mut f = SessionFile::default();
        f.push_stopped(record("a1", Some("one")));
        f.agents.push(record("a1", None));
        let start = Event::Start {
            id: "a1".into(),
            agent_type: "Explore".into(),
        };
        assert!(f.apply(start, 9));
        assert_eq!(f.agents.len(), 1);
        assert_eq!(f.agents[0].started_ms, 9);
        assert_eq!(f.agents[0].description.as_deref(), Some("one"));
        assert_eq!(f.agents[0].agent_type, "Explore");
    }
}
