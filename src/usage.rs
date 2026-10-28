//! Each account's plan usage, shown on request (doc/0043): opt-in,
//! informational, only the numbers Claude Code itself reports.
//!
//! **The probe.** `<account command> -p --output-format stream-json
//! --verbose --no-session-persistence --safe-mode /usage` runs Claude
//! Code's `/usage` as a local command: no model turn, no tokens, nothing
//! saved, and with `--safe-mode` none of the user's hooks or MCP servers
//! (reference/claude-code-cli.md, "Plan usage limits"). It runs like every
//! background call ([`CliRunner::run_line`]: stdin null, captured output, a
//! process group of its own, no inherited `CLAUDE_CONFIG_DIR`), killed after
//! [`PROBE_TIMEOUT`], in tower's own empty folder `<config dir>/usage/`
//! ([`dir`], created `0700`). The running command's config dir decides the
//! account, as for every other call (doc/0003).
//!
//! **The parser** ([`parse_probe`]) reads the `assistant` line's
//! `usage_report` leniently: unknown `kind` and `severity` values are kept
//! as-is, numbers may be integers or floats, missing fields are absent. A
//! `usage_report` of `null` means there is no plan data (logged out, an API
//! key, a setup token or usage billing). The text's "Showing last-known
//! usage" and "rate limited until …" are kept as signals.
//!
//! **The probe's own row.** While it runs (about a second) the probe is
//! listed by `claude agents --json` as an `interactive` row in its folder;
//! [`ProbeFolder::hide`] drops such rows from every listing tower shows.
//!
//! **The schedule** ([`UsageBoard`]) is pure state, driven by the app's
//! tick: only while `[usage] show = true`; a probe per account every
//! `refresh_secs`, the first at once; one probe per account at a time and
//! at most [`MAX_RUNNING`] in all; `r` refreshes accounts whose last probe
//! is older than [`REFRESH_AFTER`]; a stated "rate limited until" time
//! holds the next probe back. The last value is kept with its time; it is
//! stale once older than twice the interval, when the last probe failed,
//! or when Claude Code showed its last-known numbers.
//!
//! Nothing here feeds the hand-off, the orchestrator or any automatic
//! action (doc/0043): the values are only drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::{CliError, CliRunner, CommandLine};
use crate::config::Account;
use crate::listing::{Listing, Session};

/// Tower's folder the probe runs in, under its config dir.
pub const USAGE_DIR: &str = "usage";

/// The probe's arguments after the account command (doc/0043). Verified
/// with Claude Code 2.1.312 in this order.
pub const PROBE_ARGS: [&str; 7] = [
    "-p",
    "--output-format",
    "stream-json",
    "--verbose",
    "--no-session-persistence",
    "--safe-mode",
    "/usage",
];

/// How long one probe may take before it is killed.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Default `[usage] refresh_secs`.
pub const DEFAULT_REFRESH_SECS: u64 = 300;

/// The allowed `[usage] refresh_secs`, in seconds.
pub const REFRESH_RANGE: std::ops::RangeInclusive<u64> = 60..=3600;

/// The `[usage] refresh_secs` values the Settings window steps through.
pub const REFRESH_STEPS: [u64; 5] = [60, 120, 300, 600, 900];

/// `r` probes an account again only when its last probe started longer
/// ago than this.
pub const REFRESH_AFTER: Duration = Duration::from_secs(60);

/// The most probes running at once, over all accounts: each one is a
/// short-lived Claude Code process of a few hundred MB.
pub const MAX_RUNNING: usize = 1;

/// The wait after "rate limited until" when the stated time cannot be
/// read.
pub const RATE_LIMIT_FALLBACK: Duration = Duration::from_secs(15 * 60);

/// The longest a stated "rate limited until" time holds probes back.
pub const RATE_LIMIT_MAX: Duration = Duration::from_secs(24 * 3600);

/// Why an account has no plan data, as the details panel says it.
pub const NO_PLAN: &str = "no plan data (logged out, API key or usage billing)";

/// `<config dir>/usage`.
pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join(USAGE_DIR)
}

/// Creates `dir` (and its parents) when missing, the folder itself with
/// mode `0700`, and returns its canonical path.
pub fn prepare_dir(dir: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    if !dir.is_dir() {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    std::fs::canonicalize(dir)
}

/// The probe's command line: `<account command> -p … /usage` in `dir`.
pub fn probe_command(account: &Account, dir: &Path) -> CommandLine {
    CommandLine::for_account(account, PROBE_ARGS).with_cwd(dir)
}

// ---- the probe's own row -----------------------------------------------------

/// Tower's usage folder, to recognise the probe's own transient row in a
/// listing: an `interactive` row whose `cwd` is this folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeFolder {
    /// The folder as tower names it.
    pub path: PathBuf,
    /// Its canonical path (symlinks resolved), once known.
    pub canonical: Option<PathBuf>,
}

impl ProbeFolder {
    /// The folder `<config dir>/usage`; its canonical path is that of the
    /// config dir joined with `usage` when the config dir exists. Looks at
    /// the disk once.
    pub fn for_config_dir(config_dir: &Path) -> ProbeFolder {
        ProbeFolder {
            path: dir(config_dir),
            canonical: std::fs::canonicalize(config_dir)
                .ok()
                .map(|d| d.join(USAGE_DIR)),
        }
    }

    /// Whether `session` is a probe's own row.
    pub fn is_probe_row(&self, session: &Session) -> bool {
        if !session.is_interactive() || session.cwd.is_empty() {
            return false;
        }
        let cwd = Path::new(&session.cwd);
        cwd == self.path || self.canonical.as_deref() == Some(cwd)
    }

    /// Drops the probe's own rows from `listing`.
    pub fn hide(&self, listing: &mut Listing) {
        listing.sessions.retain(|s| !self.is_probe_row(s));
    }
}

// ---- local time ------------------------------------------------------------------

/// A wall-clock time broken down: in the local time zone
/// ([`Clock::LOCAL`]) or in UTC ([`Clock::UTC`], for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    /// 0 is Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Converts between Unix seconds and wall-clock times.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub to_wall: fn(i64) -> Option<WallTime>,
    pub from_wall: fn(&WallTime) -> Option<i64>,
}

impl Clock {
    /// The local time zone, through the C library (`localtime_r`,
    /// `mktime`).
    pub const LOCAL: Clock = Clock {
        to_wall: local_wall,
        from_wall: local_unix,
    };

    /// UTC, independent of the machine (tests).
    pub const UTC: Clock = Clock {
        to_wall: utc_wall,
        from_wall: utc_unix,
    };
}

fn local_wall(unix_s: i64) -> Option<WallTime> {
    // `time_t` is 64-bit on x86_64 glibc and musl alike (doc/0012).
    let t = unix_s;
    // SAFETY: `tm` is plain data; `localtime_r` fills it or returns null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let done = unsafe { libc::localtime_r(&t, &mut tm) };
    if done.is_null() {
        return None;
    }
    Some(WallTime {
        year: tm.tm_year + 1900,
        month: u32::try_from(tm.tm_mon + 1).ok()?,
        day: u32::try_from(tm.tm_mday).ok()?,
        weekday: u32::try_from(tm.tm_wday).ok()?,
        hour: u32::try_from(tm.tm_hour).ok()?,
        minute: u32::try_from(tm.tm_min).ok()?,
    })
}

fn local_unix(w: &WallTime) -> Option<i64> {
    // SAFETY: `tm` is plain data; `mktime` only reads and normalises it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = w.year - 1900;
    tm.tm_mon = i32::try_from(w.month).ok()? - 1;
    tm.tm_mday = i32::try_from(w.day).ok()?;
    tm.tm_hour = i32::try_from(w.hour).ok()?;
    tm.tm_min = i32::try_from(w.minute).ok()?;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t)
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn utc_wall(unix_s: i64) -> Option<WallTime> {
    let days = unix_s.div_euclid(86_400);
    let secs = unix_s.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(WallTime {
        year: i32::try_from(year).ok()?,
        month,
        day,
        weekday: u32::try_from((days + 4).rem_euclid(7)).ok()?,
        hour: (secs / 3600) as u32,
        minute: ((secs % 3600) / 60) as u32,
    })
}

fn utc_unix(w: &WallTime) -> Option<i64> {
    if !(1..=12).contains(&w.month) || !(1..=31).contains(&w.day) || w.hour > 23 || w.minute > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(w.year), w.month, w.day);
    Some(days * 86_400 + i64::from(w.hour) * 3600 + i64::from(w.minute) * 60)
}

/// Parses an ISO 8601 date and time (`2026-10-28T21:40:00.177181+00:00`,
/// `…Z`, no fraction, no seconds; no offset means UTC) into Unix ms.
pub fn parse_iso8601(text: &str) -> Option<i64> {
    let text = text.trim();
    let num = |s: &str| -> Option<i64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (date, time) = text.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let year = num(d.next()?)?;
    let month = u32::try_from(num(d.next()?)?).ok()?;
    let day = u32::try_from(num(d.next()?)?).ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // The offset: `Z`, `+HH:MM`, `-HHMM`, or none.
    let (clock, offset_s) = if let Some(c) = time.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else if let Some(at) = time.rfind(['+', '-']) {
        let (c, off) = time.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let digits: String = off[1..].chars().filter(|c| *c != ':').collect();
        if digits.len() != 4 {
            return None;
        }
        let h = num(&digits[..2])?;
        let m = num(&digits[2..])?;
        (c, sign * (h * 3600 + m * 60))
    } else {
        (time, 0)
    };
    let mut parts = clock.split(':');
    let hour = num(parts.next()?)?;
    let minute = num(parts.next()?)?;
    let (sec, frac_ms) = match parts.next() {
        Some(s) => {
            let (whole, frac) = s.split_once(['.', ',']).unwrap_or((s, ""));
            let ms = if frac.is_empty() {
                0
            } else {
                let three: String = frac.chars().chain("000".chars()).take(3).collect();
                num(&three)?
            };
            (num(whole)?, ms)
        }
        None => (0, 0),
    };
    if hour > 23 || minute > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + sec - offset_s;
    Some(secs * 1000 + frac_ms)
}

// ---- the report ---------------------------------------------------------------------

string_enum! {
    /// A limit's `kind`.
    pub enum LimitKind {
        /// The current five-hour session.
        Session => "session",
        /// The week, all models.
        WeeklyAll => "weekly_all",
        /// The week, one model or surface (`scope`).
        WeeklyScoped => "weekly_scoped",
    }
}

/// How a limit's `severity` is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// `normal`, or no severity.
    Normal,
    /// Any other value that does not read as critical.
    Warning,
    /// A value that reads as critical or exceeded.
    Critical,
}

/// One limit of `usage_report.rate_limits.limits`.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub kind: LimitKind,
    /// `group`: `session` or `weekly` so far.
    pub group: Option<String>,
    /// `percent` used, an integer or a float.
    pub percent: Option<f64>,
    /// `resets_at`, Unix ms.
    pub resets_at_ms: Option<i64>,
    /// `scope.model.display_name`.
    pub model: Option<String>,
    /// `scope.surface`, when it is a string or names itself.
    pub surface: Option<String>,
    /// `severity` as Claude Code spells it (`normal`, …).
    pub severity: Option<String>,
    /// `is_active`.
    pub active: Option<bool>,
}

impl Limit {
    /// How the severity is drawn: `normal` (or none) is normal; a value
    /// that names a critical or exceeded limit is critical; any other is a
    /// warning.
    pub fn level(&self) -> Level {
        let Some(s) = self.severity.as_deref() else {
            return Level::Normal;
        };
        let s = s.to_ascii_lowercase();
        if s.is_empty() || s == "normal" || s == "ok" || s == "none" {
            Level::Normal
        } else if [
            "crit", "exceed", "error", "severe", "over", "max", "block", "high",
        ]
        .iter()
        .any(|w| s.contains(w))
        {
            Level::Critical
        } else {
            Level::Warning
        }
    }

    /// The window's short label: `5h` for the session, `7d` for a week,
    /// else the raw kind.
    pub fn window(&self) -> String {
        match (&self.kind, self.group.as_deref()) {
            (LimitKind::Session, _) | (_, Some("session")) => "5h".to_owned(),
            (LimitKind::WeeklyAll | LimitKind::WeeklyScoped, _) | (_, Some("weekly")) => {
                "7d".to_owned()
            }
            (kind, _) => crate::attach::printable(kind.as_str()),
        }
    }

    /// What the limit is for: the model or surface of a scoped limit.
    pub fn scope_label(&self) -> Option<String> {
        self.model
            .as_deref()
            .or(self.surface.as_deref())
            .map(crate::attach::printable)
    }

    /// `24%`; `?%` without a number.
    pub fn percent_text(&self) -> String {
        match self.percent {
            Some(p) if p.is_finite() => format!("{}%", p.round().max(0.0) as u64),
            _ => "?%".to_owned(),
        }
    }
}

/// `usage_report.rate_limits.extra_usage`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraUsage {
    pub enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

impl ExtraUsage {
    /// `12.50 of 50.00 USD`, `40% used` or `on`.
    pub fn text(&self) -> String {
        let money = |v: f64| {
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.2}")
            }
        };
        let cur = self
            .currency
            .as_deref()
            .map(|c| format!(" {}", crate::attach::printable(c)))
            .unwrap_or_default();
        match (self.used_credits, self.monthly_limit, self.utilization) {
            (Some(used), Some(limit), _) => format!("{} of {}{cur}", money(used), money(limit)),
            (Some(used), None, _) => format!("{}{cur} used", money(used)),
            (None, _, Some(u)) => format!("{}% used", u.round().max(0.0) as u64),
            _ => "on".to_owned(),
        }
    }
}

/// The plan's usage as Claude Code reported it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanUsage {
    /// In Claude Code's order.
    pub limits: Vec<Limit>,
    pub extra: Option<ExtraUsage>,
}

impl PlanUsage {
    /// The current session's limit.
    pub fn session(&self) -> Option<&Limit> {
        self.limits.iter().find(|l| l.kind == LimitKind::Session)
    }

    /// The week's limit over all models.
    pub fn weekly(&self) -> Option<&Limit> {
        self.limits.iter().find(|l| l.kind == LimitKind::WeeklyAll)
    }
}

/// "rate limited until …" in `/usage`'s text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimited {
    /// The time as stated, e.g. `10:40pm`.
    pub stated: String,
    /// It, read in the local time zone, as Unix ms; `None` when it cannot
    /// be read.
    pub until_ms: Option<i64>,
}

/// What one probe found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probed {
    /// The plan's usage; `None`: `usage_report` was `null` or had no
    /// `rate_limits`, i.e. no plan data.
    pub usage: Option<PlanUsage>,
    /// The text said "Showing last-known usage".
    pub last_known: bool,
    /// The text said "rate limited until …".
    pub rate_limited: Option<RateLimited>,
    /// `claude_code_version` of the `system` line.
    pub version: Option<String>,
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    }
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn parse_limit(v: &Value) -> Option<Limit> {
    let o = v.as_object()?;
    let scope = o.get("scope").filter(|s| !s.is_null());
    let named = |v: Option<&Value>| -> Option<String> {
        let v = v.filter(|v| !v.is_null())?;
        text_of(v).or_else(|| {
            ["display_name", "name"]
                .iter()
                .find_map(|k| v.get(k).and_then(text_of))
        })
    };
    Some(Limit {
        kind: LimitKind::from(o.get("kind").and_then(Value::as_str).unwrap_or("")),
        group: o.get("group").and_then(text_of),
        percent: o.get("percent").and_then(number),
        resets_at_ms: o
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(parse_iso8601),
        model: named(scope.and_then(|s| s.get("model"))),
        surface: named(scope.and_then(|s| s.get("surface"))),
        severity: o.get("severity").and_then(text_of),
        active: o.get("is_active").and_then(Value::as_bool),
    })
}

fn parse_extra(v: &Value) -> Option<ExtraUsage> {
    let o = v.as_object()?;
    Some(ExtraUsage {
        enabled: o
            .get("is_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        monthly_limit: o.get("monthly_limit").and_then(number),
        used_credits: o.get("used_credits").and_then(number),
        utilization: o.get("utilization").and_then(number),
        currency: o.get("currency").and_then(text_of),
    })
}

/// `usage_report`; `None` when it is `null` or has no `rate_limits`.
fn parse_report(report: &Value) -> Option<PlanUsage> {
    let limits = report.get("rate_limits").filter(|r| !r.is_null())?;
    Some(PlanUsage {
        limits: limits
            .get("limits")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(parse_limit).collect())
            .unwrap_or_default(),
        extra: limits.get("extra_usage").and_then(parse_extra),
    })
}

/// The text of an `assistant` line's content.
fn assistant_text(line: &Value) -> String {
    line.pointer("/message/content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Parses the probe's stdout, one JSON value per line (doc/0043). Lines
/// that are not JSON are skipped. `now_ms` and `clock` read a stated
/// "rate limited until" time.
///
/// An error: no `assistant` or `result` line, or a `result` with
/// `is_error: true` (its text, first line).
pub fn parse_probe(stdout: &str, now_ms: i64, clock: Clock) -> Result<Probed, String> {
    let mut probed = Probed::default();
    let mut texts = Vec::new();
    let mut seen = false;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("system") => {
                probed.version = v.get("claude_code_version").and_then(text_of);
            }
            Some("assistant") => {
                seen = true;
                texts.push(assistant_text(&v));
                if let Some(usage) = v.get("usage_report").and_then(parse_report) {
                    probed.usage = Some(usage);
                }
            }
            Some("result") => {
                seen = true;
                let text = v
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if v.get("is_error").and_then(Value::as_bool) == Some(true) {
                    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    return Err(if first.is_empty() {
                        "/usage reported an error".to_owned()
                    } else {
                        format!("/usage: {}", crate::attach::printable(first.trim()))
                    });
                }
                texts.push(text);
            }
            _ => {}
        }
    }
    if !seen {
        return Err("no /usage output".to_owned());
    }
    let text = texts.join("\n");
    probed.last_known = text
        .to_ascii_lowercase()
        .contains("showing last-known usage");
    probed.rate_limited = rate_limited(&text, now_ms, clock);
    Ok(probed)
}

/// "rate limited until <time>" in `text`, the time read as a local one.
fn rate_limited(text: &str, now_ms: i64, clock: Clock) -> Option<RateLimited> {
    const MARK: &str = "rate limited until";
    let lower = text.to_ascii_lowercase();
    let at = lower.find(MARK)?;
    let rest = &text[at + MARK.len()..];
    let stated: String = rest
        .lines()
        .next()
        .unwrap_or("")
        .split(['(', '·'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches(['.', ',', ';'])
        .trim()
        .to_owned();
    let until_ms = stated_time(&stated, now_ms, clock);
    Some(RateLimited {
        stated: crate::attach::printable(&stated),
        until_ms,
    })
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// A stated local time as Unix ms: `10:40pm`, `10pm`, `22:40`, optionally
/// after a date `Oct 28,`. Without a date, the next such time from `now`.
fn stated_time(stated: &str, now_ms: i64, clock: Clock) -> Option<i64> {
    let lower = stated.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    let now_s = now_ms.div_euclid(1000);
    let today = (clock.to_wall)(now_s)?;
    let (date, time) = match words.as_slice() {
        [time] => (None, *time),
        [month, day, time, ..] => {
            let m = MONTHS.iter().position(|x| month.starts_with(x))? as u32 + 1;
            let d: u32 = day.parse().ok()?;
            (Some((m, d)), *time)
        }
        _ => return None,
    };
    let (clock_part, pm) = if let Some(t) = time.strip_suffix("pm") {
        (t, Some(true))
    } else if let Some(t) = time.strip_suffix("am") {
        (t, Some(false))
    } else {
        (time, None)
    };
    let (h, m) = clock_part.split_once(':').unwrap_or((clock_part, "0"));
    let mut hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    match pm {
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    let (month, day) = date.unwrap_or((today.month, today.day));
    let wall = WallTime {
        year: today.year,
        month,
        day,
        weekday: 0,
        hour,
        minute,
    };
    let mut at = (clock.from_wall)(&wall)?;
    if at <= now_s {
        if date.is_none() {
            at += 86_400;
        } else if at + 86_400 < now_s {
            let next = WallTime {
                year: today.year + 1,
                ..wall
            };
            at = (clock.from_wall)(&next)?;
        }
    }
    Some(at * 1000)
}

/// Runs one probe for `account` in `dir` (created when missing) and parses
/// it. Returns the folder's canonical path with the result, so the
/// listing can hide the probe's row (doc/0043).
pub fn run_probe(
    runner: &CliRunner,
    account: &Account,
    dir: &Path,
    clock: Clock,
) -> (Option<PathBuf>, Result<Probed, String>) {
    let canonical = match prepare_dir(dir) {
        Ok(c) => Some(c),
        Err(err) => {
            return (None, Err(format!("cannot create {}: {err}", dir.display())));
        }
    };
    let runner = runner.clone().with_timeout(PROBE_TIMEOUT);
    let out = match runner.run_line(&probe_command(account, dir)) {
        Ok(out) => out,
        Err(CliError::Timeout { timeout, .. }) => {
            return (
                canonical,
                Err(format!("/usage timed out after {}s", timeout.as_secs())),
            );
        }
        Err(err) => return (canonical, Err(err.to_string())),
    };
    if !out.success() {
        let first = out
            .stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned();
        let code = out.code.map_or("signal".to_owned(), |c| c.to_string());
        let text = if first.is_empty() {
            format!("/usage failed (exit {code})")
        } else {
            format!(
                "/usage failed (exit {code}): {}",
                crate::attach::printable(&first)
            )
        };
        return (canonical, Err(text));
    }
    let result = parse_probe(&out.stdout_text(), crate::sidecar::now_ms(), clock);
    (canonical, result)
}

// ---- the schedule ---------------------------------------------------------------------

/// A probe to run (doc/0043).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageJob {
    pub account: Account,
    /// The folder it runs in.
    pub dir: PathBuf,
}

/// The last plan data of an account and when it came.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub usage: PlanUsage,
    /// Wall-clock time of the probe's result, Unix ms.
    pub at_ms: i64,
    /// Claude Code showed its last-known numbers.
    pub last_known: bool,
}

/// One account's usage and its probes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountUsage {
    /// The last plan data.
    pub latest: Option<Snapshot>,
    /// The last probe found no plan data.
    pub no_plan: bool,
    /// Why the last probe failed; `None` when it worked.
    pub error: Option<String>,
    /// The last probe said "rate limited until …".
    pub rate_limited: Option<RateLimited>,
    /// A probe's result has come in since the board started or was reset.
    pub answered: bool,
    started: Option<Instant>,
    running: bool,
    forced: bool,
    not_before: Option<Instant>,
}

/// What the UI shows for one account.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown<'a> {
    /// No probe has answered yet.
    Waiting,
    /// No plan data: logged out, an API key or usage billing.
    NoPlan,
    /// The probe failed and there is no earlier value.
    Failed(&'a str),
    /// The numbers, with their age and whether they are stale.
    Plan {
        snapshot: &'a Snapshot,
        stale: bool,
        age_ms: i64,
        /// The last probe failed (its error), while an older value is
        /// shown.
        error: Option<&'a str>,
        rate_limited: Option<&'a RateLimited>,
    },
}

impl AccountUsage {
    /// Whether a probe is running for the account.
    pub fn running(&self) -> bool {
        self.running
    }

    /// What to show at wall-clock time `now_ms` with probes every
    /// `interval`: a value older than twice the interval, one Claude Code
    /// called last-known, or one kept after a failed probe is stale.
    pub fn shown(&self, now_ms: i64, interval: Duration) -> Shown<'_> {
        match (&self.latest, self.error.as_deref()) {
            (Some(snapshot), error) => {
                let age_ms = (now_ms - snapshot.at_ms).max(0);
                let limit = i64::try_from(interval.as_millis()).unwrap_or(i64::MAX) * 2;
                let stale = error.is_some() || snapshot.last_known || age_ms > limit;
                Shown::Plan {
                    snapshot,
                    stale,
                    age_ms,
                    error,
                    rate_limited: self.rate_limited.as_ref(),
                }
            }
            (None, Some(error)) => Shown::Failed(error),
            (None, None) if self.no_plan => Shown::NoPlan,
            (None, None) => Shown::Waiting,
        }
    }
}

/// Every account's usage and the probe schedule. Pure state: the app asks
/// it on every tick which probes to start ([`UsageBoard::due`]).
#[derive(Debug, Clone, Default)]
pub struct UsageBoard {
    accounts: HashMap<String, AccountUsage>,
}

impl UsageBoard {
    /// The account's usage, once it has an entry.
    pub fn get(&self, account: &str) -> Option<&AccountUsage> {
        self.accounts.get(account)
    }

    /// Whether any probe is running.
    pub fn any_running(&self) -> bool {
        self.accounts.values().any(|a| a.running)
    }

    /// The accounts of `names` to probe now, marked as running: those
    /// never probed, those whose last probe started at least `interval`
    /// ago, and those `r` asked for ([`UsageBoard::refresh`]); never one
    /// already running or held back by a rate limit, and at most
    /// [`MAX_RUNNING`] at a time, in `names`' order.
    pub fn due(&mut self, names: &[&str], now: Instant, interval: Duration) -> Vec<String> {
        let mut running = self.accounts.values().filter(|a| a.running).count();
        let mut out = Vec::new();
        for name in names {
            if running >= MAX_RUNNING {
                break;
            }
            let a = self.accounts.entry((*name).to_owned()).or_default();
            if a.running || a.not_before.is_some_and(|t| now < t) {
                continue;
            }
            let due = a.forced
                || a.started
                    .is_none_or(|s| now.saturating_duration_since(s) >= interval);
            if due {
                a.running = true;
                a.forced = false;
                a.started = Some(now);
                running += 1;
                out.push((*name).to_owned());
            }
        }
        out
    }

    /// `r`: probes again, on the next [`UsageBoard::due`], every account of
    /// `names` whose last probe started longer than [`REFRESH_AFTER`] ago.
    /// A running probe, a fresh one or a rate limit's wait is left alone.
    pub fn refresh(&mut self, names: &[&str], now: Instant) {
        for name in names {
            let a = self.accounts.entry((*name).to_owned()).or_default();
            let old = a
                .started
                .is_none_or(|s| now.saturating_duration_since(s) > REFRESH_AFTER);
            if !a.running && old {
                a.forced = true;
            }
        }
    }

    /// Records a probe's result for `account`, at `now` and wall-clock
    /// `now_ms`. A stated rate limit holds the next probe back until its
    /// time (at least `interval`, at most [`RATE_LIMIT_MAX`]; an unreadable
    /// time waits [`RATE_LIMIT_FALLBACK`]).
    pub fn finished(
        &mut self,
        account: &str,
        result: Result<Probed, String>,
        now: Instant,
        now_ms: i64,
        interval: Duration,
    ) {
        let a = self.accounts.entry(account.to_owned()).or_default();
        a.running = false;
        a.answered = true;
        match result {
            Ok(p) => {
                a.rate_limited = p.rate_limited.clone();
                a.not_before = p.rate_limited.as_ref().map(|r| {
                    let wait = r.until_ms.map_or(RATE_LIMIT_FALLBACK, |until| {
                        Duration::from_millis(u64::try_from(until - now_ms).unwrap_or(0))
                    });
                    now + wait.clamp(interval.min(RATE_LIMIT_MAX), RATE_LIMIT_MAX)
                });
                match p.usage {
                    Some(usage) => {
                        a.latest = Some(Snapshot {
                            usage,
                            at_ms: now_ms,
                            last_known: p.last_known,
                        });
                        a.no_plan = false;
                        a.error = None;
                    }
                    None if p.rate_limited.is_some() => {
                        let stated = p.rate_limited.map(|r| r.stated).unwrap_or_default();
                        a.error = Some(format!("rate limited until {stated}"));
                    }
                    None => {
                        a.latest = None;
                        a.no_plan = true;
                        a.error = None;
                    }
                }
            }
            Err(err) => a.error = Some(err),
        }
    }

    /// Forgets every value and schedule (`[usage] show` turned off), so a
    /// later start probes at once. A running probe stays marked as
    /// running until its result comes, so a second one never starts
    /// beside it.
    pub fn reset(&mut self) {
        self.accounts.retain(|_, a| a.running);
        for a in self.accounts.values_mut() {
            *a = AccountUsage {
                running: true,
                started: a.started,
                ..AccountUsage::default()
            };
        }
    }
}

// ---- texts -------------------------------------------------------------------------------

/// `12m`, `3h`, `2d`: an age for a stale value.
pub fn age_text(age_ms: i64) -> String {
    let s = age_ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// When a limit resets, in local time: `22:40` today, else `Tue 09:00`;
/// the week's always with its weekday.
pub fn reset_text(at_ms: i64, now_ms: i64, with_day: bool, clock: Clock) -> Option<String> {
    if at_ms <= now_ms {
        return Some("now".to_owned());
    }
    let at = (clock.to_wall)(at_ms.div_euclid(1000))?;
    let now = (clock.to_wall)(now_ms.div_euclid(1000))?;
    let same_day = (at.year, at.month, at.day) == (now.year, now.month, now.day);
    let time = format!("{:02}:{:02}", at.hour, at.minute);
    if same_day && !with_day {
        Some(time)
    } else {
        Some(format!("{} {time}", WEEKDAYS[at.weekday as usize % 7]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        for days in [-1, 0, 1, 59, 365, 11_016, 20_754, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(utc_wall(0).unwrap().weekday, 4, "1970-01-01 was a Thursday");
    }

    #[test]
    fn local_time_round_trips_to_the_minute() {
        // Noon UTC is never inside a DST change of a real time zone.
        for s in [
            1_793_188_800_i64,
            1_800_000_000 - 1_800_000_000 % 86_400 + 43_200,
        ] {
            let wall = local_wall(s).unwrap();
            assert_eq!(local_unix(&wall), Some(s - s % 60));
            assert_eq!(utc_unix(&utc_wall(s).unwrap()), Some(s - s % 60));
        }
    }
}
