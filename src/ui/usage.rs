//! The plan usage texts of the header, the details panel and the minimal
//! title (doc/0043): only the numbers Claude Code reports, coloured by
//! their `severity`, dimmed with their age when stale. Drawn only while
//! `[usage] show` is on; never in a window.

use ratatui::style::{Modifier, Style};

use super::theme::{Lb, Theme};
use crate::app::App;
use crate::usage::{self, Level, Limit, LimitKind, Shown};

/// How much of an account's usage the header shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum HeaderUsage {
    /// Nothing.
    None,
    /// The session's number only, without the age of a stale value.
    Bare,
    /// The session's number only.
    Session,
    /// The session's and the week's.
    Both,
}

/// The style of a number of `level`, or faint when stale.
fn level_style(level: Level, stale: bool, th: Theme) -> Style {
    if stale {
        return th.faint();
    }
    match level {
        Level::Normal => th.text(),
        Level::Warning => th.hue(th.p.amber),
        Level::Critical => th.hue(th.p.red),
    }
}

/// ` 5h 24% · 7d 12%` after an account's chip, with ` · 12m ago` when
/// stale; ` …` before the first answer and ` —` without plan data.
/// `None` when nothing is to be drawn.
pub(super) fn header_piece(
    app: &App,
    account: &str,
    now_ms: i64,
    how: HeaderUsage,
    th: Theme,
) -> Option<Lb> {
    if how == HeaderUsage::None {
        return None;
    }
    let shown = app.usage_shown(account, now_ms)?;
    let mut l = Lb::new();
    match shown {
        Shown::Waiting => {
            l.push(" …", th.faint());
        }
        Shown::NoPlan | Shown::Failed(_) => {
            l.push(" —", th.faint());
        }
        Shown::Plan {
            snapshot,
            stale,
            age_ms,
            ..
        } => {
            let mut limits: Vec<&Limit> = snapshot.usage.session().into_iter().collect();
            if how == HeaderUsage::Both {
                limits.extend(snapshot.usage.weekly());
            }
            if limits.is_empty() {
                l.push(" —", th.faint());
                return Some(l);
            }
            for (k, limit) in limits.into_iter().enumerate() {
                l.push(if k == 0 { " " } else { " · " }, th.faint());
                l.push(
                    format!("{} {}", limit.window(), limit.percent_text()),
                    level_style(limit.level(), stale, th),
                );
            }
            if stale && how != HeaderUsage::Bare {
                l.push(format!(" · {} ago", usage::age_text(age_ms)), th.faint());
            }
        }
    }
    Some(l)
}

/// One row of the details panel's `usage` value, with how it is drawn.
pub(super) struct UsageRow {
    pub text: String,
    pub style: Style,
    /// The most rows the text may wrap onto.
    pub rows: usize,
}

/// `5h 24% · resets 22:40`, `Fable 7d 0% · resets Tue 09:00`.
fn limit_text(app: &App, limit: &Limit, now_ms: i64) -> String {
    let mut text = match limit.scope_label() {
        Some(scope) => format!("{scope} {} {}", limit.window(), limit.percent_text()),
        None => format!("{} {}", limit.window(), limit.percent_text()),
    };
    let with_day = limit.kind != LimitKind::Session && limit.group.as_deref() != Some("session");
    if let Some(at) = limit.resets_at_ms
        && let Some(when) = usage::reset_text(at, now_ms, with_day, app.clock)
    {
        text.push_str(" · resets ");
        text.push_str(&when);
    }
    text
}

/// The details panel's `usage` rows for `account`: each limit with its
/// reset time (the session, the week, the scoped ones, any other kind),
/// extra usage when it is on, and the age when stale; `None` while
/// `[usage] show` is off.
pub(super) fn detail_rows(
    app: &App,
    account: &str,
    now_ms: i64,
    th: Theme,
) -> Option<Vec<UsageRow>> {
    let shown = app.usage_shown(account, now_ms)?;
    let row = |text: String, style: Style, rows: usize| UsageRow { text, style, rows };
    Some(match shown {
        Shown::Waiting => vec![row("checking…".to_owned(), th.muted(), 1)],
        Shown::NoPlan => vec![row(format!("— {}", usage::NO_PLAN), th.muted(), 2)],
        Shown::Failed(error) => vec![row(
            format!("— {}", crate::attach::printable(error)),
            th.muted(),
            3,
        )],
        Shown::Plan {
            snapshot,
            stale,
            age_ms,
            error,
            rate_limited,
        } => {
            let u = &snapshot.usage;
            let mut limits: Vec<&Limit> = Vec::new();
            limits.extend(u.session());
            limits.extend(u.weekly());
            limits.extend(
                u.limits
                    .iter()
                    .filter(|l| !matches!(l.kind, LimitKind::Session | LimitKind::WeeklyAll)),
            );
            let mut rows: Vec<UsageRow> = limits
                .into_iter()
                .map(|l| {
                    row(
                        limit_text(app, l, now_ms),
                        level_style(l.level(), stale, th),
                        2,
                    )
                })
                .collect();
            if rows.is_empty() {
                rows.push(row("— no limits listed".to_owned(), th.muted(), 1));
            }
            if let Some(extra) = u.extra.as_ref().filter(|e| e.enabled) {
                let style = if stale { th.faint() } else { th.muted() };
                rows.push(row(format!("extra {}", extra.text()), style, 1));
            }
            if stale {
                let mut age = format!("as of {} ago", usage::age_text(age_ms));
                if snapshot.last_known {
                    age = format!("last known · {age}");
                }
                rows.push(row(age, th.faint(), 1));
            }
            if let Some(r) = rate_limited {
                rows.push(row(
                    format!("rate limited until {}", r.stated),
                    th.faint(),
                    1,
                ));
            } else if let Some(error) = error {
                rows.push(row(crate::attach::printable(error), th.faint(), 3));
            }
            rows
        }
    })
}

/// The minimal title's `main 24%/12%` for `account`, dimmed when stale:
/// `main —` without plan data, `main …` before the first answer.
pub(super) fn compact(app: &App, account: &str, now_ms: i64) -> Option<(String, Style)> {
    let shown = app.usage_shown(account, now_ms)?;
    let name = crate::attach::printable(account);
    Some(match shown {
        Shown::Waiting => (format!("{name} …"), Style::new()),
        Shown::NoPlan | Shown::Failed(_) => (format!("{name} —"), Style::new()),
        Shown::Plan {
            snapshot, stale, ..
        } => {
            let pct = |l: Option<&Limit>| l.map_or("?".to_owned(), Limit::percent_text);
            let text = format!(
                "{name} {}/{}",
                pct(snapshot.usage.session()),
                pct(snapshot.usage.weekly())
            );
            let style = if stale {
                Style::new().add_modifier(Modifier::DIM)
            } else {
                Style::new()
            };
            (text, style)
        }
    })
}
