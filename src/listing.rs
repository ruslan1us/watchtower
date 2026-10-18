//! Model and lenient parser for `claude agents --json [--all]`.
//!
//! This is the one module that knows the JSON shape (doc/0001). Field list
//! and observed values: `doc/reference/claude-code-cli.md`.
//!
//! Parsing rules (doc/0001): unknown fields are ignored, missing fields get
//! defaults, unknown enum values become `Unknown(raw)`, and a malformed row is
//! skipped with a warning instead of failing the whole listing.

use serde::{Deserialize, Serialize};

string_enum! {
    /// `kind`: how the session runs.
    pub enum Kind {
        /// Running in some other terminal; cannot be attached.
        Interactive => "interactive",
        /// A background session (`claude --bg`).
        Background => "background",
    }
}

string_enum! {
    /// `state`, present on background rows. After `claude stop` a row keeps
    /// its last value, so `state` alone does not say whether the process is
    /// alive; see [`Session::has_live_process`].
    pub enum State {
        Working => "working",
        Blocked => "blocked",
        Done => "done",
        Failed => "failed",
        Stopped => "stopped",
    }
}

string_enum! {
    /// `status`, present only while the process is alive.
    pub enum Status {
        Busy => "busy",
        Waiting => "waiting",
        Idle => "idle",
    }
}

string_enum! {
    /// `waitingFor`, present when `status` is `waiting`.
    pub enum WaitingFor {
        PermissionPrompt => "permission prompt",
        InputNeeded => "input needed",
        SandboxRequest => "sandbox request",
        WorkerRequest => "worker request",
        DialogOpen => "dialog open",
    }
}

impl Default for Kind {
    /// A row without `kind` is kept; its kind shows as an empty unknown value.
    fn default() -> Self {
        Kind::Unknown(String::new())
    }
}

/// One row of the listing. Every field except `kind` and `cwd` is optional,
/// because the CLI omits fields that do not apply (see the reference doc).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// `interactive` or `background`.
    #[serde(default)]
    pub kind: Kind,
    /// Short id used by `attach`, `logs`, `stop`, `rm`. Background rows only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Full UUID, used by `--resume`; the primary merge key (doc/0003).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Display name, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Working directory of the session; empty when missing.
    #[serde(default)]
    pub cwd: String,
    /// Start of the *current process*, Unix ms. Reset on every wake, so it is
    /// not a creation or activity time (doc/0006).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// OS process id, while the process is alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Process status, while the process is alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// Background state; absent on interactive rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
    /// What a waiting session waits for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<WaitingFor>,
}

impl Session {
    /// True when the row has a live process: `pid` or `status` present.
    /// Rows without either are the ones only `--all` returns, e.g. after
    /// `claude stop` (doc/0006).
    pub fn has_live_process(&self) -> bool {
        self.pid.is_some() || self.status.is_some()
    }

    /// True for `kind: interactive`.
    pub fn is_interactive(&self) -> bool {
        self.kind == Kind::Interactive
    }

    /// True when `claude attach <id>` can be used: not interactive and a short
    /// id is present (doc/0005).
    pub fn is_attachable(&self) -> bool {
        !self.is_interactive() && self.id.is_some()
    }

    /// Key for tower's own records: `sessionId`, or the short `id` when it is
    /// missing (doc/0002, doc/0006).
    pub fn key(&self) -> Option<&str> {
        self.session_id.as_deref().or(self.id.as_deref())
    }

    /// Name for display: `name`, else the short id, else the first 8
    /// characters of `sessionId`, else `pid <n>` (doc/0006).
    pub fn display_name(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_owned();
        }
        if let Some(id) = &self.id {
            return id.clone();
        }
        if let Some(sid) = &self.session_id {
            return sid.chars().take(8).collect();
        }
        match self.pid {
            Some(pid) => format!("pid {pid}"),
            None => "(unnamed)".to_owned(),
        }
    }
}

/// Parsed rows plus warnings about rows that were skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Rows that parsed, in the CLI's order.
    pub sessions: Vec<Session>,
    /// One message per skipped row (or other non-fatal oddity). Never printed
    /// by the library; the UI decides how to show them.
    pub warnings: Vec<String>,
}

/// The listing as a whole could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The output is not JSON at all.
    #[error("listing is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The output is JSON but not an array of rows.
    #[error("listing is not a JSON array")]
    NotAnArray,
}

/// Parses the stdout of `claude agents --json [--all]`.
///
/// Whitespace-only output is treated as an empty listing with a warning.
/// A row that is not an object or has a field of the wrong JSON type is
/// skipped and reported in [`Listing::warnings`].
pub fn parse_listing(text: &str) -> Result<Listing, ParseError> {
    if text.trim().is_empty() {
        return Ok(Listing {
            sessions: Vec::new(),
            warnings: vec!["empty listing output; treated as no sessions".to_owned()],
        });
    }
    let value: serde_json::Value = serde_json::from_str(text)?;
    let serde_json::Value::Array(rows) = value else {
        return Err(ParseError::NotAnArray);
    };
    let mut listing = Listing::default();
    for (index, row) in rows.into_iter().enumerate() {
        if !row.is_object() {
            listing
                .warnings
                .push(format!("skipped listing row {index}: not a JSON object"));
            continue;
        }
        match serde_json::from_value::<Session>(row) {
            Ok(session) => listing.sessions.push(session),
            Err(err) => listing
                .warnings
                .push(format!("skipped listing row {index}: {err}")),
        }
    }
    Ok(listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_round_trip() {
        assert_eq!(State::from("blocked"), State::Blocked);
        assert_eq!(State::from("paused"), State::Unknown("paused".into()));
        assert_eq!(State::Unknown("paused".into()).as_str(), "paused");
        assert_eq!(WaitingFor::from("dialog open"), WaitingFor::DialogOpen);
        assert_eq!(
            WaitingFor::PermissionPrompt.to_string(),
            "permission prompt"
        );
        assert_eq!(
            serde_json::to_string(&Status::Unknown("napping".into())).unwrap(),
            "\"napping\""
        );
    }

    #[test]
    fn display_name_fallbacks() {
        let mut s = Session {
            session_id: Some("0f0e0d0c-0000-4000-8000-000000000000".into()),
            ..Default::default()
        };
        assert_eq!(s.display_name(), "0f0e0d0c");
        s.id = Some("1a2b3c4d".into());
        assert_eq!(s.display_name(), "1a2b3c4d");
        s.name = Some("fix-flaky-test".into());
        assert_eq!(s.display_name(), "fix-flaky-test");
        let p = Session {
            pid: Some(4242),
            ..Default::default()
        };
        assert_eq!(p.display_name(), "pid 4242");
    }
}
