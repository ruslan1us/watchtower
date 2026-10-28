//! A layout audit of every screen and window (doc/0011, doc/0020,
//! doc/0021, doc/0038): each scenario is drawn with ratatui's
//! `TestBackend` at nine terminal sizes and in four looks (the dashboard in
//! `neutral`, `neutral-light` and under `NO_COLOR`, and the minimal style),
//! with long, realistic data: long session and project names, Cyrillic, a
//! 60-column branch, deep folders, long model and template names and a long
//! hand-off prompt.
//!
//! Every frame is checked for text that is cut, wrapped or overflowing:
//!
//! - `border`: a window whose frame is broken (text over its border);
//! - `pad`: window text touching the window's left or right border;
//! - `hint-row`: a key hint line that wraps onto a second row or is cut;
//! - `cut`: a known string cut at a word boundary without `…`;
//! - `mid-word`: a known string cut inside a word without `…`;
//! - `could-wrap`: a value of a details panel cut (with `…`) while the panel
//!   has free rows below it;
//! - `odd`: odd values (`↑0`, `· ·`, a dangling `·`, `1 sessions`);
//! - `align`: columns of the session list or the details panel out of line,
//!   or a minimal-style row that lost its account column;
//! - `missing`: a string the window must show (its answers, its keys) that
//!   is not on the screen at all.
//!
//! Each invariant runs per family of scenarios (`<check>_<family>`); a few
//! targeted tests cover what the heuristics cannot see. The ones that fail
//! today are ignored with the id of their finding (`F-…`, from the audit
//! report). `audit_report` and `audit_by_subject` (ignored) print every
//! finding grouped; `dump` (ignored) writes every frame to the folder in
//! `UI_AUDIT_DUMP` for review by eye:
//!
//! ```text
//! UI_AUDIT_DUMP=/tmp/frames cargo test --test ui_audit -- --ignored dump
//! cargo test --test ui_audit -- --ignored audit_report --nocapture
//! ```
//!
//! The frames are drawn once per run, on every core, and shared by the
//! tests.

mod common;
mod settings_common;

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use common::{NoEnviron, ch, ctrl, key};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use watchtower::actions::RemoveError;
use watchtower::app::{
    App, DIALOG_KEY_TABLE, Effect, KEY_TABLE, LOGS_KEY_TABLE, Overlay, Severity,
};
use watchtower::commands::{CommandList, Kind as CommandKind, Origin, SlashCommand};
use watchtower::config::{ColorDepth, Config, UiStyle, UiTheme};
use watchtower::config_edit::SettingKey;
use watchtower::dispatch::{DispatchError, MODEL_CHOICES};
use watchtower::gitinfo::{GitJob, GitState, GitSummary, Head, Upstream};
use watchtower::handoff::{HandoffError, Step};
use watchtower::listing::parse_listing;
use watchtower::logview::{self, LogsError};
use watchtower::orchestrator::StartError;
use watchtower::poller::PollUpdate;
use watchtower::projects::{Project, ProjectList};
use watchtower::reply::ReplyError;
use watchtower::sidecar::{AccountSource, Sidecar};
use watchtower::subagents::Subagent;
use watchtower::templates::{Template, TemplateList};
use watchtower::usage::{Clock, ExtraUsage, Limit, LimitKind, PlanUsage, Probed, RateLimited};

// ---- data -------------------------------------------------------------------

/// The wall clock every frame is drawn at (Unix ms).
const NOW_MS: i64 = 1_800_000_000_000;
const HOME: &str = "/home/user";

const LONG_NAME: &str = "refactor-the-authentication-middleware-and-session-store";
const CYR_NAME: &str = "исправить-падающие-тесты-в-модуле-платежей";
const LONG_PROJECT: &str = "billing-platform-frontend-with-a-very-long-folder-name";
const CYR_PROJECT: &str = "проект-оплаты-и-счетов";
const DEEP: &str = "data/pipelines/reports/quarterly/2026/eu-west";
/// 60 columns.
const BRANCH: &str = "feature/move-the-session-store-onto-the-new-cache-with-retry";
const LONG_MODEL: &str = "claude-opus-4-1-20250805-experimental-long-context";
const HANDOFF_PROMPT: &str = "Continue where you left off: re-run the failing integration \
    tests, fix the flaky ones and summarise what changed in the pull request description";
const LONG_TASK: &str = "Найди, почему падают интеграционные тесты платежей после обновления \
    библиотеки, и исправь их; then update the changelog and open a pull request with a short \
    summary of the root cause";
const LONG_TEMPLATE: &str = "review-the-pull-request-with-a-very-strict-checklist";
const LONG_MESSAGE: &str = "started refactor-the-authentication-middleware-and-session-store \
    under main in ~/workspace/billing-platform-frontend-with-a-very-long-folder-name";
const LONG_WARNING: &str = "project root ~/old-projects-from-the-previous-laptop-backup does \
    not exist";
const POLL_ERROR: &str = "claude-alt agents --json failed (exit 1): could not reach the \
    background service; is the supervisor running?";
const REFUSAL: &str = "Refusing to remove refactor-the-authentication-middleware-and-session-\
    store: its worktree ~/workspace/billing-platform-frontend-with-a-very-long-folder-name/.\
    claude/worktrees/refactor-auth has uncommitted changes and 3 commits that are not pushed.\n\
    Commit or stash them, or push the branch, then remove it again.";

/// The config of most scenarios: three accounts, `main` and `alt` in one
/// group, `spare` in another; cross-group hand-offs as given.
fn config_text(cross: bool, prompt: &str) -> String {
    format!(
        r#"
default_account = "main"
handoff_prompt = {prompt:?}
allow_cross_group_handoff = {cross}

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
group = "work"

[[accounts]]
name = "spare"
command = "claude-spare"
config_dir = "~/.claude-spare"
group = "personal"
"#
    )
}

fn sid(id: &str) -> String {
    format!("{id}-0000-4000-8000-000000000000")
}

/// A background row; `process` holds its live part.
fn row(id: &str, name: &str, cwd: &str, state: &str, process: &str, ago_s: i64) -> String {
    format!(
        r#"{{ {process} "id": "{id}", "sessionId": "{}", "cwd": "{HOME}/workspace/{cwd}",
            "kind": "background", "name": "{name}", "state": "{state}",
            "startedAt": {} }}"#,
        sid(id),
        NOW_MS - ago_s * 1000
    )
}

/// The sessions of every scenario.
fn listing_text() -> String {
    let rows = [
        row(
            "a1a1a1a1",
            LONG_NAME,
            LONG_PROJECT,
            "working",
            r#""pid": 4101, "status": "busy","#,
            300,
        ),
        row(
            "b2b2b2b2",
            CYR_NAME,
            CYR_PROJECT,
            "blocked",
            r#""pid": 4102, "status": "waiting", "waitingFor": "permission prompt","#,
            42,
        ),
        row(
            "c3c3c3c3",
            "print-date",
            "api",
            "done",
            r#""pid": 4103, "status": "idle","#,
            720,
        ),
        row("d4d4d4d4", "say-ok", "api", "done", "", 7200),
        row(
            "e5e5e5e5",
            "nightly-report",
            DEEP,
            "done",
            r#""pid": 4105, "status": "busy","#,
            1800,
        ),
        row(
            "f6f6f6f6",
            "pick-colour",
            "web",
            "blocked",
            r#""pid": 4106, "status": "waiting", "waitingFor": "input needed","#,
            90,
        ),
        format!(
            r#"{{ "pid": 4107, "cwd": "{HOME}/workspace/demo-app", "kind": "interactive",
                "startedAt": {}, "sessionId": "{}", "name": "demo-app-3f",
                "status": "idle" }}"#,
            NOW_MS - 60_000,
            sid("a7a7a7a7")
        ),
    ];
    format!("[{}]", rows.join(",\n"))
}

fn poll_of(account: &str, text: &str) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle: 0,
        result: Ok(parse_listing(text).unwrap()),
    }
}

/// The app of most scenarios: every row owned by `main` (unless
/// `owners` is false), listed by all three accounts, the git summaries in.
fn app_with(cross: bool, prompt: &str, owners: bool, text: &str) -> App {
    let config = Config::parse(&config_text(cross, prompt), Path::new(HOME)).unwrap();
    let mut sc = Sidecar::default();
    if owners {
        for s in &parse_listing(text).unwrap().sessions {
            if let Some(id) = &s.session_id {
                sc.record_owner(id, "main", AccountSource::Started);
            }
        }
    }
    let mut app = App::new(config, HOME.into(), sc, Box::new(NoEnviron));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(poll_of(a, text));
    }
    feed_git(&mut app);
    app
}

fn base() -> App {
    app_with(false, HANDOFF_PROMPT, true, &listing_text())
}

// ---- plan usage (doc/0043) ----------------------------------------------------

/// A limit of `kind` at `percent`, resetting `hours` after [`NOW_MS`].
fn limit(kind: &str, percent: f64, hours: i64, severity: &str, model: Option<&str>) -> Limit {
    let kind = LimitKind::from(kind);
    let group = match kind {
        LimitKind::Session => Some("session".to_owned()),
        LimitKind::WeeklyAll | LimitKind::WeeklyScoped => Some("weekly".to_owned()),
        _ => None,
    };
    Limit {
        kind,
        group,
        percent: Some(percent),
        resets_at_ms: Some(NOW_MS + hours * 3_600_000),
        model: model.map(str::to_owned),
        surface: None,
        severity: Some(severity.to_owned()),
        active: Some(true),
    }
}

fn plan(limits: Vec<Limit>, extra: bool) -> Probed {
    Probed {
        usage: Some(PlanUsage {
            limits,
            extra: Some(ExtraUsage {
                enabled: extra,
                monthly_limit: Some(50.0),
                used_credits: Some(12.5),
                utilization: Some(25.0),
                currency: Some("USD".to_owned()),
            }),
        }),
        ..Probed::default()
    }
}

fn plain_plan() -> Probed {
    plan(
        vec![
            limit("session", 24.0, 3, "normal", None),
            limit("weekly_all", 12.0, 100, "normal", None),
            limit("weekly_scoped", 3.0, 100, "normal", Some("Fable")),
        ],
        false,
    )
}

/// The scenarios' app with `[usage] show = true` (doc/0043), local time
/// as UTC.
fn usage_app() -> App {
    let config = Config::parse(
        &format!(
            "{}\n[usage]\nshow = true\n",
            config_text(false, HANDOFF_PROMPT)
        ),
        Path::new(HOME),
    )
    .unwrap();
    let text = listing_text();
    let mut sc = Sidecar::default();
    for s in &parse_listing(&text).unwrap().sessions {
        if let Some(id) = &s.session_id {
            sc.record_owner(id, "main", AccountSource::Started);
        }
    }
    let mut app = App::new(config, HOME.into(), sc, Box::new(NoEnviron));
    for a in ["main", "alt", "spare"] {
        app.apply_poll(poll_of(a, &text));
    }
    feed_git(&mut app);
    app.clock = Clock::UTC;
    app
}

/// `account`'s probe result, received `ago_s` seconds before [`NOW_MS`].
fn usage_in(app: &mut App, account: &str, result: Result<Probed, String>, ago_s: i64) {
    app.usage_received(account, result, Instant::now(), NOW_MS - ago_s * 1000);
}

fn feed_git(app: &mut App) {
    let summary = |head: Head, changed: usize, upstream: Upstream| {
        GitState::Summary(GitSummary {
            head,
            changed,
            upstream,
        })
    };
    let jobs = [
        (
            "api",
            summary(
                Head::Branch(BRANCH.into()),
                0,
                Upstream::Tracking {
                    ahead: 0,
                    behind: 0,
                },
            ),
        ),
        (
            LONG_PROJECT,
            summary(
                Head::Branch("feature/login".into()),
                12,
                Upstream::Tracking {
                    ahead: 3,
                    behind: 1,
                },
            ),
        ),
        (
            CYR_PROJECT,
            summary(Head::Detached("1a2b3c4".into()), 2, Upstream::None),
        ),
        (
            DEEP,
            summary(Head::Branch(BRANCH.into()), 5, Upstream::Gone),
        ),
    ];
    for (cwd, state) in jobs {
        app.git_finished(
            GitJob {
                cwd: format!("{HOME}/workspace/{cwd}"),
            },
            state,
        );
    }
}

/// Projects shown as `~/workspace/<name>`, in folders under `root`.
fn projects(root: &Path, names: &[&str], warnings: &[&str]) -> ProjectList {
    ProjectList {
        projects: names
            .iter()
            .map(|n| {
                let path = root.join(n);
                std::fs::create_dir_all(&path).unwrap();
                Project {
                    canonical: path.clone(),
                    display: format!("~/workspace/{n}"),
                    path,
                    last_used_ms: None,
                    extra: false,
                }
            })
            .collect(),
        warnings: warnings.iter().map(|w| (*w).to_owned()).collect(),
    }
}

const PROJECT_NAMES: [&str; 6] = [
    "api",
    LONG_PROJECT,
    CYR_PROJECT,
    "web",
    "data-pipelines-quarterly-reports-eu-west-2026-archive",
    "demo-app",
];

fn templates() -> TemplateList {
    TemplateList {
        templates: vec![
            Template {
                name: "fix-tests".into(),
                text: "Fix the failing tests in the payments module, then run the whole suite \
                       twice to make sure nothing is flaky any more."
                    .into(),
            },
            Template {
                name: LONG_TEMPLATE.into(),
                text: "Review the diff against main.\nBe strict.".into(),
            },
            Template {
                name: "проверить-миграции".into(),
                text: "Проверь миграции базы данных и убедись, что откат работает.".into(),
            },
        ],
        ..TemplateList::default()
    }
}

const LONG_SKILL: &str = "review-kit:review-the-pull-request-with-a-very-strict-checklist";
const SKILL_HINT: &str = "[ticket-link] [priority]";
const SKILL_DESCRIPTION: &str = "Fix a bug from a ticket: reproduce it, write a failing test, \
    fix it and open a pull request";
const CYR_DESCRIPTION: &str = "Проверь миграции базы данных и убедись, что откат работает";

/// Slash commands from every source (claude.ai's synced skills too), with
/// long names, a long hint and a Cyrillic description (doc/0042).
fn slash_commands() -> CommandList {
    let c = |name: &str, hint: Option<&str>, description: &str, origin: Origin| SlashCommand {
        name: name.into(),
        description: description.into(),
        argument_hint: hint.map(str::to_owned),
        origin,
        kind: CommandKind::Skill,
    };
    let plugin = || Origin::Plugin("review-kit".into());
    watchtower::commands::merge(
        vec![
            c(
                "fix-bug",
                Some(SKILL_HINT),
                SKILL_DESCRIPTION,
                Origin::Personal,
            ),
            c("standup", None, "", Origin::Personal),
        ],
        vec![
            c("проверить-миграции", None, CYR_DESCRIPTION, Origin::Project),
            c(
                "frontend:component",
                Some("<name>"),
                "A new component",
                Origin::Project,
            ),
        ],
        vec![c(
            "fill-forms",
            None,
            "Fill in a form from a document",
            Origin::Synced,
        )],
        vec![
            c(
                LONG_SKILL,
                None,
                "Review the diff against main, strictly",
                plugin(),
            ),
            c(
                "review-kit:code-review",
                None,
                "Review the changes",
                plugin(),
            ),
            c("review-kit:fix-lint", None, "Fix the lint errors", plugin()),
            c(
                "review-kit:fix-types",
                None,
                "Fix the type errors",
                plugin(),
            ),
            c(
                "review-kit:fixtures",
                None,
                "Refresh the test fixtures",
                plugin(),
            ),
        ],
    )
}

/// The slash-command load the app asks for, answered with `list` (none:
/// still loading).
fn with_commands(app: &mut App, list: Option<CommandList>) {
    let effects = app.tick(Instant::now());
    let load = effects
        .iter()
        .find_map(|e| match e {
            Effect::LoadCommands(load) => Some(load.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no command load in {effects:?}"));
    if let Some(list) = list {
        app.commands_loaded(&load, list);
    }
}

fn subagents() -> HashMap<String, Vec<Subagent>> {
    let agent = |id: &str, ty: &str, desc: &str, ago_s: i64| Subagent {
        id: id.into(),
        agent_type: ty.into(),
        description: Some(desc.into()),
        started_ms: NOW_MS - ago_s * 1000,
    };
    HashMap::from([(
        sid("a1a1a1a1"),
        vec![
            agent(
                "aa01",
                "general-purpose",
                "Research how other services rotate their session signing keys",
                130,
            ),
            agent("aa02", "Explore", "Найти все места, где читается токен", 70),
        ],
    )])
}

// ---- scenarios --------------------------------------------------------------

/// A built scenario: the app and whatever must live while it is drawn.
struct Built {
    app: App,
    _keep: Vec<Box<dyn Any>>,
}

fn just(app: App) -> Built {
    Built {
        app,
        _keep: Vec::new(),
    }
}

fn keep(app: App, dir: impl Any) -> Built {
    Built {
        app,
        _keep: vec![Box::new(dir)],
    }
}

fn press(app: &mut App, k: ratatui::crossterm::event::KeyEvent) -> Vec<Effect> {
    app.handle_key(k)
}

fn type_in(app: &mut App, text: &str) {
    for c in text.chars() {
        if c == '\n' {
            app.handle_key(key(KeyCode::Enter));
        } else {
            app.handle_key(ch(c));
        }
    }
}

fn tabs(app: &mut App, n: usize) {
    for _ in 0..n {
        app.handle_key(key(KeyCode::Tab));
    }
}

fn select(app: &mut App, name: &str) {
    common::select_name(app, name);
}

/// The new-session dialog over the long projects.
fn dialog(warnings: &[&str]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let mut app = base();
    app.open_dialog(projects(dir.path(), &PROJECT_NAMES, warnings));
    keep(app, dir)
}

fn dialog_then(f: impl Fn(&mut App)) -> Built {
    let mut b = dialog(&[]);
    f(&mut b.app);
    b
}

/// The dialog with a valid task, `Ctrl+S` pressed: the dispatch job.
fn dialog_started() -> (Built, Option<watchtower::app::DispatchJob>) {
    let mut b = dialog(&[]);
    tabs(&mut b.app, 1);
    type_in(&mut b.app, "do the thing well");
    let job = press(&mut b.app, ctrl('s'))
        .into_iter()
        .find_map(|e| match e {
            Effect::Dispatch(job) => Some(job),
            _ => None,
        });
    (b, job)
}

fn with_templates(app: &mut App, list: Option<TemplateList>) {
    let effects = press(app, ctrl('p'));
    let load = effects
        .iter()
        .find_map(|e| match e {
            Effect::LoadTemplates(load) => Some(load.id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no template load in {effects:?}"));
    if let Some(list) = list {
        app.templates_loaded(load, list);
    }
}

fn resume(warnings: &[&str]) -> Built {
    let dir = tempfile::tempdir().unwrap();
    let mut app = base();
    let effects = press(&mut app, ch('R'));
    let Some(id) = effects.iter().find_map(|e| match e {
        Effect::ScanProjects(scan) => Some(scan.id),
        _ => None,
    }) else {
        panic!("{effects:?}")
    };
    app.projects_found(id, projects(dir.path(), &PROJECT_NAMES, warnings));
    keep(app, dir)
}

/// `a` on `name`, then (with a target picker) `Enter`; the confirmation.
fn handoff(cross: bool, prompt: &str, name: &str) -> App {
    let mut app = app_with(cross, prompt, true, &listing_text());
    select(&mut app, name);
    press(&mut app, ch('a'));
    assert!(
        matches!(app.overlay(), Overlay::Confirm(_) | Overlay::Target(_)),
        "{:?}",
        app.overlay()
    );
    app
}

fn handoff_job(app: &mut App) -> watchtower::app::HandoffJob {
    press(app, ch('y'))
        .into_iter()
        .find_map(|e| match e {
            Effect::Handoff(job) => Some(job),
            _ => None,
        })
        .expect("the hand-off starts")
}

fn logs_open(name: &str, deliver: bool) -> App {
    let mut app = base();
    select(&mut app, name);
    press(&mut app, ch(' '));
    if deliver {
        let target = app.logs().unwrap().in_flight().cloned().unwrap();
        let screen = logview::render(common::fixture("logs/working.ansi").as_bytes());
        app.logs_fetched(target, Ok(screen));
    }
    app
}

fn reply_open(name: &str) -> App {
    let mut app = base();
    select(&mut app, name);
    press(&mut app, ch('t'));
    assert!(
        app.reply_window().is_some(),
        "{}",
        common::status_text(&app)
    );
    app
}

fn reply_job(app: &mut App) -> watchtower::app::ReplyJob {
    press(app, ctrl('s'))
        .into_iter()
        .find_map(|e| match e {
            Effect::Reply(job) => Some(job),
            _ => None,
        })
        .expect("the reply job")
}

fn rename_open(name: &str) -> App {
    let mut app = base();
    select(&mut app, name);
    press(&mut app, ch('e'));
    assert!(
        app.rename_window().is_some(),
        "{}",
        common::status_text(&app)
    );
    app
}

/// The Settings config with long values.
const SETTINGS_CONFIG: &str = r#"# Settings with long values.
default_account = "main"
default_model = "claude-opus-4-1-20250805-experimental-long-context"
handoff_prompt = "Continue where you left off: re-run the failing integration tests, fix the flaky ones and summarise what changed"
project_roots = ["~/workspace", "~/old-projects-from-the-previous-laptop-backup", "~/clients/very-long-client-name-holdings/repositories"]
extra_projects = ["~/notes"]

[ui]
style = "dashboard"
theme = "neutral"

[[accounts]]
name = "main"
command = "claude"
group = "work"

[[accounts]]
name = "alternative-account"
command = "claude-alternative-wrapper"
config_dir = "~/.config/claude-code-alternative-profile"
group = "personal-sandbox"
"#;

fn settings(config: Option<&str>, f: impl Fn(&mut App)) -> Built {
    let env = settings_common::env(config);
    let mut app = settings_common::app_for(&env);
    settings_common::open(&mut app);
    f(&mut app);
    keep(app, env)
}

fn sp(app: &mut App, k: ratatui::crossterm::event::KeyEvent) {
    settings_common::press(app, k);
}

type Builder = Box<dyn Fn() -> Built + Sync + Send>;

/// Every scenario, by name. The family is the name up to the first `-`.
fn scenarios() -> Vec<(&'static str, Builder)> {
    let mut out: Vec<(&'static str, Builder)> = Vec::new();
    macro_rules! s {
        ($name:expr, $body:expr) => {
            out.push(($name, Box::new($body)));
        };
    }

    // The list, its group headers and the details panel or strip.
    s!("list-long-row", || {
        let mut app = base();
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-cyrillic-row", || {
        let mut app = base();
        select(&mut app, CYR_NAME);
        just(app)
    });
    s!("list-git-clean", || {
        let mut app = base();
        select(&mut app, "print-date");
        just(app)
    });
    s!("list-git-deep", || {
        let mut app = base();
        select(&mut app, "nightly-report");
        just(app)
    });
    s!("list-stopped-row", || {
        let mut app = base();
        select(&mut app, "say-ok");
        just(app)
    });
    s!("list-interactive-row", || {
        let mut app = base();
        select(&mut app, "demo-app-3f");
        just(app)
    });
    s!("list-header", || {
        let mut app = base();
        press(&mut app, key(KeyCode::Home));
        just(app)
    });
    s!("list-by-project-header", || {
        let mut app = base();
        press(&mut app, ch('p'));
        press(&mut app, key(KeyCode::Home));
        just(app)
    });
    s!("list-by-project-row", || {
        let mut app = base();
        press(&mut app, ch('p'));
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-by-project-logs", || {
        let mut app = logs_open(LONG_NAME, true);
        press(&mut app, ch('p'));
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-subagents", || {
        let mut app = base();
        app.subagents_read(subagents());
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-unknown-owner", || {
        let mut app = app_with(false, HANDOFF_PROMPT, false, &listing_text());
        select(&mut app, "print-date");
        just(app)
    });
    s!("list-usage", || {
        let mut app = usage_app();
        let mut main = plain_plan();
        if let Some(u) = main.usage.as_mut() {
            u.extra.as_mut().unwrap().enabled = true;
        }
        usage_in(&mut app, "main", Ok(main), 30);
        usage_in(&mut app, "alt", Ok(plain_plan()), 1200);
        usage_in(&mut app, "spare", Ok(Probed::default()), 30);
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-usage-critical", || {
        let mut app = usage_app();
        let mut p = plan(
            vec![
                limit("session", 81.6, 1, "warning", None),
                limit("weekly_all", 97.0, 30, "critical", None),
                limit(
                    "weekly_scoped",
                    40.2,
                    30,
                    "normal",
                    Some("Claude Opus with a very long model display name"),
                ),
                limit("monthly_surface", 7.0, 400, "elevated", None),
            ],
            true,
        );
        p.last_known = true;
        p.rate_limited = Some(RateLimited {
            stated: "1:30pm".to_owned(),
            until_ms: Some(NOW_MS + 3_600_000),
        });
        usage_in(&mut app, "main", Ok(p), 90);
        usage_in(&mut app, "alt", Ok(plain_plan()), 30);
        usage_in(&mut app, "spare", Ok(plain_plan()), 30);
        select(&mut app, "print-date");
        just(app)
    });
    s!("list-usage-waiting", || {
        let mut app = usage_app();
        select(&mut app, LONG_NAME);
        just(app)
    });
    s!("list-usage-failed", || {
        let mut app = usage_app();
        usage_in(&mut app, "main", Err(POLL_ERROR.to_owned()), 10);
        usage_in(&mut app, "alt", Ok(plain_plan()), 4000);
        usage_in(
            &mut app,
            "alt",
            Err("/usage timed out after 20s".to_owned()),
            10,
        );
        select(&mut app, CYR_NAME);
        just(app)
    });
    s!("list-empty", || {
        let mut app = app_with(false, HANDOFF_PROMPT, true, "[]");
        app.apply_poll(poll_of("main", "[]"));
        just(app)
    });
    s!("list-loading", || {
        let config = Config::parse(&config_text(false, "continue"), Path::new(HOME)).unwrap();
        just(App::new(
            config,
            HOME.into(),
            Sidecar::default(),
            Box::new(NoEnviron),
        ))
    });

    // The status line and the filter line.
    s!("status-message", || {
        let mut app = base();
        app.set_message(LONG_MESSAGE, Severity::Info);
        app.add_warning(LONG_WARNING);
        just(app)
    });
    s!("status-poll-error", || {
        let mut app = base();
        app.apply_poll(PollUpdate {
            account: "alt".into(),
            cycle: 1,
            result: Err(POLL_ERROR.into()),
        });
        just(app)
    });
    s!("status-theme-fallback", || {
        let mut app = base();
        app.color_depth = ColorDepth::Ansi16;
        just(app)
    });
    s!("filter-editing", || {
        let mut app = base();
        press(&mut app, ch('/'));
        type_in(&mut app, "исправить-падающие");
        just(app)
    });
    s!("filter-no-match", || {
        let mut app = base();
        press(&mut app, ch('/'));
        type_in(&mut app, "no-such-session-anywhere-in-this-list");
        just(app)
    });
    s!("filter-kept", || {
        let mut app = base();
        press(&mut app, ch('i'));
        press(&mut app, ch('/'));
        type_in(&mut app, "refactor-the-authentication");
        press(&mut app, key(KeyCode::Enter));
        just(app)
    });

    // The logs panel.
    s!("logs-open", || just(logs_open(LONG_NAME, true)));
    s!("logs-focused", || {
        let mut app = logs_open(LONG_NAME, true);
        press(&mut app, key(KeyCode::Right));
        just(app)
    });
    s!("logs-paused", || {
        let mut app = logs_open(LONG_NAME, true);
        press(&mut app, key(KeyCode::Right));
        press(&mut app, key(KeyCode::Up));
        just(app)
    });
    s!("logs-full", || {
        let mut app = logs_open(LONG_NAME, true);
        press(&mut app, key(KeyCode::Right));
        press(&mut app, ch('f'));
        just(app)
    });
    s!("logs-loading", || just(logs_open(CYR_NAME, false)));
    s!("logs-error", || {
        let mut app = logs_open(CYR_NAME, false);
        let target = app.logs().unwrap().in_flight().cloned().unwrap();
        app.logs_fetched(
            target,
            Err(LogsError::Failed {
                code: Some(1),
                stderr: POLL_ERROR.into(),
            }),
        );
        just(app)
    });

    // Help.
    s!("help-list", || {
        let mut app = base();
        press(&mut app, ch('?'));
        just(app)
    });
    s!("help-logs", || {
        let mut app = logs_open(LONG_NAME, true);
        press(&mut app, key(KeyCode::Right));
        press(&mut app, ch('?'));
        just(app)
    });

    // The new-session dialog.
    s!("dialog-project", || dialog(&[]));
    s!("dialog-project-warnings", || dialog(&[LONG_WARNING, "x"]));
    s!("dialog-project-filter", || {
        dialog_then(|app| type_in(app, "billing-platform-frontend-with"))
    });
    s!("dialog-project-no-match", || {
        dialog_then(|app| type_in(app, "no-such-project-anywhere-at-all"))
    });
    s!("dialog-project-none", || {
        let mut app = base();
        app.open_dialog(ProjectList::default());
        just(app)
    });
    s!("dialog-task-empty", || dialog_then(|app| tabs(app, 1)));
    s!("dialog-task-long", || {
        dialog_then(|app| {
            tabs(app, 1);
            type_in(app, LONG_TASK);
            type_in(app, "\nвторая строка");
        })
    });
    s!("dialog-name-empty", || dialog_then(|app| tabs(app, 2)));
    s!("dialog-name-long", || {
        dialog_then(|app| {
            tabs(app, 2);
            type_in(app, LONG_NAME);
        })
    });
    s!("dialog-account", || dialog_then(|app| tabs(app, 3)));
    s!("dialog-model", || dialog_then(|app| tabs(app, 4)));
    s!("dialog-model-long", || {
        dialog_then(|app| {
            tabs(app, 4);
            // The dialog's text fields take no Ctrl keys (doc/0036):
            // the preset is deleted with Backspace.
            while !app.dialog().unwrap().model.text().is_empty() {
                press(app, key(KeyCode::Backspace));
            }
            type_in(app, LONG_MODEL);
        })
    });
    s!("dialog-effort", || dialog_then(|app| tabs(app, 5)));
    s!("dialog-mode", || dialog_then(|app| tabs(app, 6)));
    s!("dialog-haiku-error", || {
        dialog_then(|app| {
            tabs(app, 4);
            while app.dialog().unwrap().model.text() != "haiku" {
                press(app, key(KeyCode::Down));
            }
            press(app, ctrl('s'));
        })
    });
    s!("dialog-discard", || {
        dialog_then(|app| {
            tabs(app, 1);
            type_in(app, "do the thing");
            press(app, key(KeyCode::Esc));
        })
    });
    s!("dialog-quit", || {
        dialog_then(|app| {
            tabs(app, 1);
            type_in(app, "do the thing");
            press(app, ctrl('c'));
        })
    });
    s!("dialog-starting", || dialog_started().0);
    s!("dialog-untrusted", || {
        let (mut b, job) = dialog_started();
        b.app.dispatch_finished(
            job.expect("a dispatch"),
            Err(DispatchError::Untrusted {
                dir: PathBuf::from(format!("{HOME}/workspace/{LONG_PROJECT}")),
                message: "Workspace not trusted. Run `claude` there once and accept the \
                          trust prompt."
                    .into(),
            }),
        );
        b
    });

    // The slash-command menu in the task field (doc/0042).
    s!("dialog-slash-loading", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, None);
            type_in(app, "/");
        })
    });
    s!("dialog-slash-all", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, Some(slash_commands()));
            type_in(app, "/");
        })
    });
    s!("dialog-slash-filtered", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, Some(slash_commands()));
            type_in(app, "/fi");
            press(app, key(KeyCode::Down));
        })
    });
    s!("dialog-slash-long", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, Some(slash_commands()));
            type_in(app, "/review-the");
        })
    });
    s!("dialog-slash-hint", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, Some(slash_commands()));
            type_in(app, "/fix-b");
            press(app, key(KeyCode::Tab));
            type_in(app, "https://tickets.example.com/browse/PAY-1234");
        })
    });
    s!("dialog-slash-lower", || {
        dialog_then(|app| {
            tabs(app, 1);
            with_commands(app, Some(slash_commands()));
            type_in(app, "\n\n\n\n/");
        })
    });
    s!("reply-slash-all", || {
        let mut app = reply_open("print-date");
        with_commands(&mut app, Some(slash_commands()));
        type_in(&mut app, "/");
        just(app)
    });
    s!("reply-slash-hint", || {
        let mut app = reply_open("print-date");
        with_commands(&mut app, Some(slash_commands()));
        type_in(&mut app, "/fix-b");
        press(&mut app, key(KeyCode::Enter));
        type_in(&mut app, "PAY-1234");
        just(app)
    });

    // The template picker, over the dialog and the quick reply window.
    s!("templates-loading", || {
        dialog_then(|app| with_templates(app, None))
    });
    s!("templates-empty", || {
        dialog_then(|app| with_templates(app, Some(TemplateList::default())))
    });
    s!("templates-list", || {
        dialog_then(|app| {
            with_templates(app, Some(templates()));
            press(app, key(KeyCode::Down));
        })
    });
    s!("templates-error", || {
        dialog_then(|app| {
            with_templates(
                app,
                Some(TemplateList {
                    error: Some(
                        "cannot list ~/.config/watchtower/templates: permission denied (os \
                         error 13)"
                            .into(),
                    ),
                    ..TemplateList::default()
                }),
            );
        })
    });
    s!("templates-no-match", || {
        dialog_then(|app| {
            with_templates(app, Some(templates()));
            type_in(app, "nothing-matches-this-filter");
        })
    });
    s!("templates-reply", || {
        let mut app = reply_open("print-date");
        with_templates(&mut app, Some(templates()));
        just(app)
    });

    // The quick reply window.
    s!("reply-live-empty", || just(reply_open("print-date")));
    s!("reply-live-long", || {
        let mut app = reply_open("print-date");
        type_in(&mut app, LONG_TASK);
        just(app)
    });
    s!("reply-stopped", || just(reply_open("say-ok")));
    s!("reply-error", || {
        let mut app = reply_open("print-date");
        type_in(&mut app, "  ");
        press(&mut app, ctrl('s'));
        just(app)
    });
    s!("reply-discard", || {
        let mut app = reply_open("print-date");
        type_in(&mut app, "go on");
        press(&mut app, key(KeyCode::Esc));
        just(app)
    });
    s!("reply-untrusted", || {
        let mut app = reply_open("print-date");
        type_in(&mut app, "go on");
        let job = reply_job(&mut app);
        app.reply_finished(
            job,
            Err(ReplyError::Untrusted {
                dir: format!("{HOME}/workspace/api").into(),
                message: "Workspace not trusted. Run `claude` in ~/workspace/api once.".into(),
            }),
        );
        just(app)
    });

    // The rename window.
    s!("rename-running", || {
        let mut app = rename_open("print-date");
        type_in(&mut app, "-and-a-much-longer-new-name");
        just(app)
    });
    s!("rename-long", || just(rename_open(LONG_NAME)));
    s!("rename-stopped", || just(rename_open("say-ok")));
    s!("rename-error", || {
        let mut app = rename_open("say-ok");
        press(&mut app, ctrl('u'));
        press(&mut app, key(KeyCode::Enter));
        just(app)
    });
    s!("rename-untrusted", || {
        let mut app = rename_open("say-ok");
        type_in(&mut app, "-v2");
        let effects = press(&mut app, key(KeyCode::Enter));
        let [Effect::Rename(job)] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        app.rename_finished(
            job.clone(),
            Err(watchtower::rename::RenameError::Untrusted {
                dir: PathBuf::from(format!("{HOME}/workspace/api")),
                message: "Workspace not trusted. Run `claude` there once.".into(),
            }),
        );
        just(app)
    });

    // Hand-off: the confirmation, the target picker, the trust window, the
    // quit question while it runs.
    s!("handoff-confirm", || {
        just(handoff(false, HANDOFF_PROMPT, "print-date"))
    });
    s!("handoff-no-prompt", || just(handoff(
        false,
        "",
        "print-date"
    )));
    s!("handoff-working", || {
        just(handoff(false, HANDOFF_PROMPT, LONG_NAME))
    });
    s!("handoff-permission", || just(handoff(false, "", CYR_NAME)));
    s!("handoff-target", || {
        just(handoff(true, HANDOFF_PROMPT, LONG_NAME))
    });
    s!("handoff-untrusted", || {
        let mut app = handoff(false, HANDOFF_PROMPT, "print-date");
        let job = handoff_job(&mut app);
        app.handoff_finished(
            job,
            Err(HandoffError::Untrusted {
                dir: PathBuf::from(format!("{HOME}/workspace/api")),
                message: "Workspace not trusted. Run `claude-alt` there once and accept the \
                          trust prompt, then retry."
                    .into(),
            }),
        );
        just(app)
    });
    s!("handoff-quit", || {
        let mut app = handoff(false, HANDOFF_PROMPT, "print-date");
        handoff_job(&mut app);
        app.handoff_step(Step::WaitingForStop);
        press(&mut app, ch('q'));
        just(app)
    });

    // Stop and remove confirmations, the `rm` refusal.
    s!("confirm-stop-long", || {
        let mut app = base();
        select(&mut app, LONG_NAME);
        press(&mut app, ch('s'));
        just(app)
    });
    s!("confirm-stop-cyrillic", || {
        let mut app = base();
        select(&mut app, CYR_NAME);
        press(&mut app, ch('s'));
        just(app)
    });
    s!("confirm-remove-long", || {
        let mut app = base();
        select(&mut app, LONG_NAME);
        press(&mut app, ch('d'));
        just(app)
    });
    s!("popup-refusal", || {
        let mut app = base();
        let action = watchtower::app::SessionAction {
            account: app.config().accounts[0].clone(),
            id: "a1a1a1a1".into(),
            name: LONG_NAME.into(),
        };
        app.remove_finished(
            action.into(),
            Err(RemoveError::Refused {
                code: Some(1),
                message: REFUSAL.into(),
            })
            .into(),
        );
        just(app)
    });

    // The orchestrator's trust window.
    s!("trust-orchestrator", || {
        let mut app = base();
        let effects = press(&mut app, ch('o'));
        let Some(job) = effects.iter().find_map(|e| match e {
            Effect::StartOrchestrator(job) => Some(job.clone()),
            _ => None,
        }) else {
            panic!("{effects:?}")
        };
        app.orchestrator_finished(
            job,
            Err(StartError::Dispatch(DispatchError::Untrusted {
                dir: "/x".into(),
                message: "Workspace not trusted. Run claude there first.".into(),
            })),
        );
        just(app)
    });

    // The account picker of an unknown owner.
    s!("picker-attach", || {
        let mut app = app_with(false, HANDOFF_PROMPT, false, &listing_text());
        select(&mut app, LONG_NAME);
        press(&mut app, key(KeyCode::Enter));
        assert!(
            matches!(app.overlay(), Overlay::Picker(_)),
            "{:?}",
            app.overlay()
        );
        just(app)
    });
    s!("picker-rename", || {
        let mut app = app_with(true, HANDOFF_PROMPT, false, &listing_text());
        select(&mut app, "say-ok");
        press(&mut app, ch('e'));
        type_in(&mut app, "-v2");
        press(&mut app, key(KeyCode::Enter));
        assert!(
            matches!(app.overlay(), Overlay::Picker(_)),
            "{:?}",
            app.overlay()
        );
        just(app)
    });
    s!("picker-reply", || {
        let mut app = app_with(true, HANDOFF_PROMPT, false, &listing_text());
        select(&mut app, "say-ok");
        press(&mut app, ch('t'));
        assert!(
            matches!(app.overlay(), Overlay::Picker(_)),
            "{}",
            common::status_text(&app)
        );
        just(app)
    });

    // The resume window.
    s!("resume-project", || resume(&[]));
    s!("resume-warnings", || resume(&[LONG_WARNING]));
    s!("resume-account", || {
        let mut b = resume(&[]);
        tabs(&mut b.app, 1);
        b
    });

    // The Settings window.
    s!("settings-style", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "style")
        })
    });
    s!("settings-theme-pick", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "theme");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-depth-row", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "colour depth")
        })
    });
    s!("settings-depth-pick", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "colour depth");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-model", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "default model")
        })
    });
    s!("settings-model-pick", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "default model");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-prompt", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "hand-off prompt")
        })
    });
    s!("settings-prompt-input", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "hand-off prompt");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-roots", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "project roots")
        })
    });
    s!("settings-roots-editor", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "project roots");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-usage", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "show plan usage")
        })
    });
    s!("settings-usage-refresh", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "usage refresh");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-follow", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "follow logs in background")
        })
    });
    s!("settings-accounts", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            sp(app, key(KeyCode::End));
        })
    });
    s!("settings-unsaved", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "default effort");
            sp(app, key(KeyCode::Right));
            settings_common::select(app, "track subagents");
            sp(app, key(KeyCode::Right));
            sp(app, key(KeyCode::Esc));
        })
    });
    s!("settings-cross-group", || {
        settings(Some(SETTINGS_CONFIG), |app| {
            settings_common::select(app, "allow cross-group hand-off");
            sp(app, key(KeyCode::Enter));
        })
    });
    s!("settings-help", || {
        settings(Some(SETTINGS_CONFIG), |app| sp(app, ch('?')))
    });
    s!("settings-no-file", || {
        settings(None, |app| settings_common::select(app, "style"))
    });
    out
}

// ---- rendering ----------------------------------------------------------------

const SIZES: [(u16, u16); 9] = [
    (80, 24),
    (90, 28),
    (100, 30),
    (104, 32),
    (120, 36),
    (140, 40),
    (160, 45),
    (200, 50),
    (250, 60),
];

/// The looks: name, style, theme, colour.
const LOOKS: [(&str, UiStyle, UiTheme, bool); 4] = [
    ("dash-neutral", UiStyle::Dashboard, UiTheme::Neutral, true),
    (
        "dash-light",
        UiStyle::Dashboard,
        UiTheme::NeutralLight,
        true,
    ),
    ("dash-nocolor", UiStyle::Dashboard, UiTheme::Neutral, false),
    ("minimal", UiStyle::Minimal, UiTheme::Neutral, true),
];

/// One drawn frame: a character per cell.
struct Frame {
    scenario: &'static str,
    look: &'static str,
    w: u16,
    h: u16,
    rows: Vec<Vec<char>>,
}

impl Frame {
    fn row_text(&self, y: usize) -> String {
        self.rows[y].iter().collect()
    }

    fn text(&self) -> String {
        (0..self.rows.len())
            .map(|y| self.row_text(y).trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn draw(app: &mut App, w: u16, h: u16) -> Vec<Vec<char>> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| watchtower::ui::draw_at(f, app, Instant::now(), NOW_MS))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        })
        .collect()
}

/// Every scenario at every size in every look, drawn once per test run,
/// on as many threads as there are cores.
fn frames() -> &'static [Frame] {
    static FRAMES: OnceLock<Vec<Frame>> = OnceLock::new();
    FRAMES.get_or_init(|| {
        let all = scenarios();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = all.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            let handles: Vec<_> = all
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || part.iter().flat_map(draw_scenario).collect::<Vec<Frame>>())
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| match h.join() {
                    Ok(frames) => frames,
                    Err(e) => std::panic::resume_unwind(e),
                })
                .collect::<Vec<Frame>>()
        })
    })
}

/// One scenario at every size in every look: one build per size.
fn draw_scenario((scenario, build): &(&'static str, Builder)) -> Vec<Frame> {
    let mut out = Vec::new();
    for &(w, h) in &SIZES {
        let mut b = build();
        for &(look, style, theme, color) in &LOOKS {
            b.app.style = style;
            b.app.set_theme(theme);
            b.app.color = color;
            let rows = draw(&mut b.app, w, h);
            out.push(Frame {
                scenario,
                look,
                w,
                h,
                rows,
            });
        }
    }
    out
}

// ---- known strings ---------------------------------------------------------------

/// What a known string is: a value or sentence, or a line of key hints that
/// must fit one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Value,
    Hint,
}

fn known() -> &'static [(String, Kind)] {
    static KNOWN: OnceLock<Vec<(String, Kind)>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        let mut v: Vec<(String, Kind)> = Vec::new();
        let mut value = |s: &str| v.push((s.to_owned(), Kind::Value));
        for s in [
            LONG_NAME,
            CYR_NAME,
            LONG_PROJECT,
            CYR_PROJECT,
            DEEP,
            BRANCH,
            LONG_MODEL,
            LONG_TEMPLATE,
            "data-pipelines-quarterly-reports-eu-west-2026-archive",
            "проверить-миграции",
            "Fix the failing tests in the payments module",
            "Проверь миграции базы данных",
            "Research how other services rotate their session signing keys",
            "Найти все места, где читается токен",
            "Continue where you left off",
            "old-projects-from-the-previous-laptop-backup",
            "clients/very-long-client-name-holdings/repositories",
            "claude-alternative-wrapper",
            "alternative-account",
            "config/claude-code-alternative-profile",
            "personal-sandbox",
            "nightly-report",
            "print-date",
            "no live process",
            "permission prompt",
            "waiting for your reply",
            "starting… (a supervisor may have to start first; this can take a few seconds)",
            "type to filter by name, project, account",
            "The conversation is kept; attaching wakes the session again.",
            "The transcript stays on disk and can still be resumed with claude --resume.",
            "The full conversation and project context will continue under",
            "The pending permission prompt is not shown again after the wake.",
            "The account that owns this session is unknown.",
            "Attaching with another account moves the session to it.",
            "Tower wakes it with that account to rename it, which moves the session to it.",
            "Tower wakes it with that account to send your text, which moves the session to it.",
            "Owner: main. Stop it there and continue under:",
            "your next prompt for the session · Ctrl+P inserts a template",
            "no project matches the filter",
            "no template matches the filter",
            "No templates yet: put .md files in",
            "loading templates…",
            "the dashboard needs",
            "could not reach the background service",
            "accounts: main, alt, spare",
            "in a session: Ctrl+Z returns to tower",
            "loading commands…",
            LONG_SKILL,
            SKILL_DESCRIPTION,
            CYR_DESCRIPTION,
            "Review the diff against main, strictly",
            "plugin review-kit",
        ] {
            value(s);
        }
        // Window and panel titles.
        value(&format!("{LONG_NAME} (main)"));
        value(&format!("Continue {LONG_NAME} under which account?"));
        value(&format!("Which account owns {LONG_NAME}?"));
        value(LONG_MESSAGE);
        value(LONG_WARNING);
        value(POLL_ERROR);
        value(HANDOFF_PROMPT);
        value(&watchtower::ui::resume_note("claude"));
        value("Stops the idle session and wakes it under main with your text as the next prompt.");
        value("Wakes the session under main with your text as the next prompt.");
        value(
            "is stopped. Enter wakes it under main in the background without attaching, runs \
             /rename there (a local command, no model turn), waits until the list shows the new \
             name and stops it again.",
        );
        value("Claude Code renames a running session from inside it. Enter attaches to");
        value("and press Enter, then Ctrl+Z to come back to tower; tower then checks the name.");
        for (k, what) in KEY_TABLE
            .iter()
            .chain(DIALOG_KEY_TABLE)
            .chain(LOGS_KEY_TABLE)
        {
            let _ = k;
            value(what);
        }
        for key in SettingKey::ALL {
            value(key.help());
            value(key.applies());
        }
        for line in REFUSAL.lines() {
            value(line);
        }
        let mut hint = |s: &str| v.push((s.to_owned(), Kind::Hint));
        for s in [
            "describe the task · Enter new line · Ctrl+G opens $VISUAL / $EDITOR · Ctrl+P \
             inserts a template",
            "Tab/Shift+Tab field · Enter next field · Ctrl+S start · Alt+Enter start & attach \
             · Ctrl+G editor · Ctrl+P template",
            "Tab/Shift+Tab field · Enter new line · Ctrl+S start · Alt+Enter start & attach · \
             Ctrl+G editor · Ctrl+P template",
            "Tab/Shift+Tab field · ↑↓ choose · type to filter · Enter open the picker · Esc \
             cancel",
            "Enter rename · Ctrl+U clear · Esc cancel",
            "Enter attach · Ctrl+U clear · Esc cancel",
            "Enter new line · Ctrl+S send · Ctrl+P template · Esc cancel",
            "↑↓ choose · type to filter · Enter insert · Esc close",
            "↑↓ choose · Tab insert · Esc close",
            "Enter keep · Esc clear",
            "Enter continue under this account · Esc cancel",
            "Enter attach with this account · Esc cancel",
            "Enter rename with this account · Esc cancel",
            "Esc or ? closes this help",
            "↑↓/PgUp/PgDn scroll · Esc close",
            "→ scroll · Space/Esc close",
            "↑↓ PgUp/PgDn g/G · f full screen · ← list · Space close",
            "↑↓ PgUp/PgDn g/G · f/← back · Space close",
            "Discard this task?  y discard · n keep editing",
            "Quit tower? The dialog's contents are lost.  y quit · n stay",
            "Discard this text?  y discard · n keep editing",
            "y yes   n/Esc no (default)",
            "y quit   n/Esc keep running (default)",
            "a add   Enter edit   d remove   Esc back to settings",
            "↑↓ preview   Enter keep   Esc revert to",
            "↑↓ move   Enter keep   Esc cancel",
            "Enter pick   ←→ change   Del default",
            // The selected row's hint, with its trailing space.
            "Enter pick   ←→ change ",
        ] {
            hint(s);
        }
        hint(&format!(
            "↑↓ {} · or type a model name",
            MODEL_CHOICES.join(" · ")
        ));
        v
    })
}

// ---- checks -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Check {
    Border,
    Pad,
    HintRow,
    Cut,
    MidWord,
    CouldWrap,
    Odd,
    Align,
    Missing,
}

impl Check {
    fn name(self) -> &'static str {
        match self {
            Check::Border => "border",
            Check::Pad => "pad",
            Check::HintRow => "hint-row",
            Check::Cut => "cut",
            Check::MidWord => "mid-word",
            Check::CouldWrap => "could-wrap",
            Check::Odd => "odd",
            Check::Align => "align",
            Check::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
struct Finding {
    check: Check,
    scenario: &'static str,
    look: &'static str,
    w: u16,
    h: u16,
    /// What was found: the known string, the pattern, the window.
    subject: String,
    /// The rendered line(s).
    line: String,
}

/// A box drawn with border glyphs: corners at (x0, y0) and (x1, y1).
#[derive(Debug, Clone, Copy)]
struct BoxRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

const TL: &[char] = &['╭', '┌', '┏'];
const TR: &[char] = &['╮', '┐', '┓'];
const BL: &[char] = &['╰', '└', '┗'];
const BR: &[char] = &['╯', '┘', '┛'];
const VL: &[char] = &['│', '┃', '├', '┣'];
const VR: &[char] = &['│', '┃', '┤', '┫'];
const BORDER: &[char] = &[
    '│', '┃', '╮', '┐', '┓', '╯', '┘', '┛', '┤', '┫', '╭', '┌', '┏', '╰', '└', '┗', '├', '┣',
];

/// The boxes of a frame, and the top-left corners whose box is broken.
fn boxes(f: &Frame) -> (Vec<BoxRect>, Vec<(usize, usize)>) {
    let (w, h) = (usize::from(f.w), usize::from(f.h));
    let at = |x: usize, y: usize| f.rows[y][x];
    let mut out = Vec::new();
    let mut broken = Vec::new();
    for y0 in 0..h {
        for x0 in 0..w {
            if !TL.contains(&at(x0, y0)) {
                continue;
            }
            let mut y = y0 + 1;
            while y < h && VL.contains(&at(x0, y)) {
                y += 1;
            }
            if y >= h || !BL.contains(&at(x0, y)) {
                broken.push((x0, y0));
                continue;
            }
            let y1 = y;
            let x1 = (x0 + 1..w).find(|&x| {
                TR.contains(&at(x, y0))
                    && BR.contains(&at(x, y1))
                    && (y0 + 1..y1).all(|y| VR.contains(&at(x, y)))
            });
            match x1 {
                Some(x1) => out.push(BoxRect { x0, y0, x1, y1 }),
                None => broken.push((x0, y0)),
            }
        }
    }
    (out, broken)
}

/// The innermost box holding (x, y), border included.
fn box_at(bs: &[BoxRect], x: usize, y: usize) -> Option<BoxRect> {
    bs.iter()
        .filter(|b| b.x0 <= x && x <= b.x1 && b.y0 <= y && y <= b.y1)
        .min_by_key(|b| (b.x1 - b.x0) * (b.y1 - b.y0))
        .copied()
}

/// Scenarios without a window over the screen: every box is a panel.
fn is_screen(scenario: &str) -> bool {
    ["list-", "status-", "filter-", "logs-"]
        .iter()
        .any(|p| scenario.starts_with(p))
        || [
            "settings-style",
            "settings-depth-row",
            "settings-model",
            "settings-prompt",
            "settings-prompt-input",
            "settings-roots",
            "settings-follow",
            "settings-accounts",
            "settings-no-file",
        ]
        .contains(&scenario)
}

/// A window over the screen, not a panel of it: it touches neither side.
fn is_window(f: &Frame, b: &BoxRect) -> bool {
    b.x0 > 0 && b.x1 + 1 < usize::from(f.w)
}

/// Labels of the details panels' `label  value` lines.
const KV_LABELS: &[&str] = &[
    "group",
    "state",
    "status",
    "changed",
    "folder",
    "git",
    "account",
    "id",
    "kind",
    "hand-off",
    "sessions",
    "attention",
    "accounts",
    "states",
    "usage",
];

/// A details panel: a box with a ` folder    ` or ` sessions  ` line.
fn is_kv_panel(f: &Frame, b: &BoxRect) -> bool {
    (b.y0 + 1..b.y1).any(|y| {
        let inner: String = f.rows[y][b.x0 + 1..b.x1].iter().collect();
        inner.starts_with(" folder    ") || inner.starts_with(" sessions  ")
    })
}

fn blank_rows(f: &Frame, b: &BoxRect) -> usize {
    (b.y0 + 1..b.y1)
        .filter(|&y| f.rows[y][b.x0 + 1..b.x1].iter().all(|c| *c == ' '))
        .count()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Where `s` matches row `y` from column `x`: the number of chars.
fn match_len(row: &[char], x: usize, s: &[char]) -> usize {
    row[x..].iter().zip(s).take_while(|(a, b)| a == b).count()
}

/// The columns where `needle` starts in `row`.
fn positions(row: &[char], needle: &[char]) -> Vec<usize> {
    let row_s: String = row.iter().collect();
    let needle_s: String = needle.iter().collect();
    find_all(&row_s, &needle_s)
}

/// The columns (chars) where `needle` starts in `row`.
fn find_all(row: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = row[from..].find(needle) {
        let byte = from + at;
        out.push(row[..byte].chars().count());
        from = byte + row[byte..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

fn contains(row: &[char], needle: &[char]) -> bool {
    let row_s: String = row.iter().collect();
    let needle_s: String = needle.iter().collect();
    row_s.contains(&needle_s)
}

fn finding(
    f: &Frame,
    check: Check,
    subject: impl Into<String>,
    line: impl Into<String>,
) -> Finding {
    Finding {
        check,
        scenario: f.scenario,
        look: f.look,
        w: f.w,
        h: f.h,
        subject: subject.into(),
        line: line.into(),
    }
}

/// How many leading characters of a known string must match before a
/// shorter match counts as a cut of it.
const HEAD: usize = 14;

/// The strings each scenario must show whole on one row, by scenario name
/// prefix.
const MUST: &[(&str, &str)] = &[
    ("dialog-", "Esc cancel"),
    ("dialog-discard", "y discard"),
    ("dialog-quit", "y quit"),
    ("resume-", "Esc cancel"),
    ("rename-running", "Esc cancel"),
    ("rename-stopped", "Esc cancel"),
    ("rename-long", "Esc cancel"),
    ("rename-error", "the name must not be empty"),
    ("reply-live", "Esc cancel"),
    ("reply-stopped", "Esc cancel"),
    ("reply-discard", "y discard"),
    ("templates-", "Esc close"),
    ("handoff-confirm", "n/Esc no (default)"),
    ("handoff-no-prompt", "n/Esc no (default)"),
    ("handoff-working", "n/Esc no (default)"),
    ("handoff-permission", "n/Esc no (default)"),
    ("handoff-target", "Esc cancel"),
    ("handoff-untrusted", "Esc: leave it stopped."),
    ("handoff-quit", "keep running (default)"),
    ("confirm-", "n/Esc no (default)"),
    ("popup-", "Esc close"),
    ("trust-", "Esc: do not start it."),
    ("picker-", "Esc cancel"),
    ("help-list", "Esc or ? closes this help"),
    ("help-list", "Ctrl+Z"),
    ("help-list", "Esc                    cancel"),
    ("help-logs", "Esc or ? closes this help"),
    ("help-logs", "Space            close the logs panel"),
    ("settings-", "? help"),
    ("list-", "? help"),
    ("filter-editing", "Enter keep · Esc clear"),
];

fn check_frame(f: &Frame) -> Vec<Finding> {
    let (bs, _) = boxes(f);
    let screen = is_screen(f.scenario);
    check_frame_all(f)
        .into_iter()
        .filter(|(fd, at)| {
            // With a window open, only what is in a window: the screen
            // under it has its own scenarios.
            screen
                || fd.check == Check::Missing
                || at.is_some_and(|(x, y)| box_at(&bs, x, y).is_some_and(|b| is_window(f, &b)))
        })
        .map(|(fd, _)| fd)
        .collect()
}

/// Every finding of a frame, with where it was found.
fn check_frame_all(f: &Frame) -> Vec<(Finding, Option<(usize, usize)>)> {
    let mut out: Vec<(Finding, Option<(usize, usize)>)> = Vec::new();
    let (bs, broken) = boxes(f);
    let h = usize::from(f.h);

    let screen = is_screen(f.scenario);
    // (a) Broken frames (on screens without a window over them, where
    // every box is whole), and window text touching its border.
    for (x0, y0) in broken.into_iter().filter(|_| screen) {
        out.push((
            finding(
                f,
                Check::Border,
                format!("box at {x0},{y0}"),
                f.row_text(y0).trim_end(),
            ),
            Some((x0, y0)),
        ));
    }
    for b in bs.iter().filter(|b| is_window(f, b)) {
        for y in b.y0 + 1..b.y1 {
            let row = &f.rows[y];
            let left = row[b.x0 + 1];
            let right = row[b.x1 - 1];
            let allowed = [' ', '▌', '▶', '─', '├', '┤', '│'];
            if !allowed.contains(&left) || (right != ' ' && right != '─') {
                let side = if allowed.contains(&left) {
                    "right"
                } else {
                    "left"
                };
                let inner: String = row[b.x0..=b.x1].iter().collect();
                let title: String = f.rows[b.y0][b.x0 + 1..b.x1]
                    .iter()
                    .collect::<String>()
                    .trim_matches(|c: char| c == '─' || c == ' ')
                    .to_owned();
                out.push((
                    finding(f, Check::Pad, format!("{side} border of {title:?}"), inner),
                    Some((b.x0 + 1, y)),
                ));
            }
        }
    }

    // (b), (c), (d), (e): known strings.
    let known = known();
    let chars: Vec<Vec<char>> = known.iter().map(|(s, _)| s.chars().collect()).collect();
    let heads: Vec<String> = chars
        .iter()
        .map(|s| s.iter().take(HEAD).collect())
        .collect();
    for y in 0..h {
        let row = &f.rows[y];
        let row_s = f.row_text(y);
        // Every match of a known string's first [`HEAD`] characters.
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        for (i, s) in chars.iter().enumerate() {
            for x in find_all(&row_s, &heads[i]) {
                hits.push((x, i, match_len(row, x, s)));
            }
        }
        for &(x, i, k) in &hits {
            let s = &chars[i];
            if k >= s.len() {
                continue;
            }
            // Another known string explains this text better.
            if hits.iter().any(|&(x2, i2, k2)| {
                // It holds this match: a longer text, or the same text
                // as a shorter string.
                i2 != i
                    && x2 <= x
                    && x2 + k2 >= x + k
                    && (x2 < x || k2 > k || chars[i2].len() < s.len())
            }) {
                continue;
            }
            let rest: String = s[k..].iter().collect();
            let rest_t = rest.trim_start();
            if rest_t.is_empty() {
                continue;
            }
            let (text, kind) = (&known[i].0, known[i].1);
            let end = x + k;
            let next = row.get(end).copied();
            let line = f.row_text(y).trim_end().to_owned();
            // Where the text's room ends: the right border of the box it
            // is in (its title on the border, too), else the screen's edge.
            let b = box_at(&bs, x, y);
            let edge = b.map_or(row.len(), |b| b.x1);
            if next == Some('…') {
                // A title on the top border is no value: the panel shows
                // a shortened title's whole text in its body (doc/0041).
                if let Some(b) = b
                    && y > b.y0
                    && is_kv_panel(f, &b)
                    && blank_rows(f, &b) > 0
                {
                    out.push((
                        finding(f, Check::CouldWrap, text.clone(), line.clone()),
                        Some((x, y)),
                    ));
                }
                if kind == Kind::Hint {
                    out.push((finding(f, Check::HintRow, text.clone(), line), Some((x, y))));
                }
                continue;
            }
            // Only spaces up to the edge: the text goes on below (a wrap)
            // or stops here.
            let blank_after = row[end.min(edge)..edge].iter().all(|c| *c == ' ');
            // The next row of the same box goes on with the rest, after
            // its border, indent and gutter.
            let rest_head: String = rest_t.chars().take(8).collect();
            let wrapped = y + 1 < h && {
                let from = b.map_or(0, |b| b.x0);
                let below: String = f.rows[y + 1][from..edge.min(f.rows[y + 1].len())]
                    .iter()
                    .collect();
                below
                    .trim_start_matches(|c: char| c == ' ' || c == '┊' || BORDER.contains(&c))
                    .starts_with(&rest_head)
            };
            if blank_after && wrapped {
                if kind == Kind::Hint {
                    let next_line = f.row_text(y + 1).trim_end().to_owned();
                    out.push((
                        finding(
                            f,
                            Check::HintRow,
                            text.clone(),
                            format!("{line}\n{next_line}"),
                        ),
                        Some((x, y)),
                    ));
                }
                continue;
            }
            // Cut: the text runs into its edge (or its padding column, or
            // the corner of the border it is a title on). Text that stops
            // before the edge was shortened on purpose or is covered by a
            // window over it.
            let at_edge = end == edge || (end + 1 == edge && next == Some(' '));
            if !at_edge {
                continue;
            }
            let mid = k > 0 && is_word(s[k - 1]) && is_word(s[k]);
            let check = if mid { Check::MidWord } else { Check::Cut };
            out.push((finding(f, check, text.clone(), line.clone()), Some((x, y))));
            if kind == Kind::Hint {
                out.push((finding(f, Check::HintRow, text.clone(), line), Some((x, y))));
            }
        }
    }

    // (f) Odd values.
    for y in 0..h {
        let row = &f.rows[y];
        let line = f.row_text(y).trim_end().to_owned();
        let n = row.len();
        for x in 0..n {
            let c = row[x];
            let next = row.get(x + 1).copied().unwrap_or(' ');
            let after = row.get(x + 2).copied().unwrap_or(' ');
            if (c == '↑' || c == '↓') && next == '0' && !after.is_ascii_digit() {
                out.push((
                    finding(f, Check::Odd, format!("{c}0"), line.clone()),
                    Some((x, y)),
                ));
            }
            if c == '·' {
                // `· ·` with only spaces between.
                let mut z = x + 1;
                while z < n && row[z] == ' ' {
                    z += 1;
                }
                if z < n && z > x + 1 && row[z] == '·' {
                    out.push((
                        finding(f, Check::Odd, "empty separator", line.clone()),
                        Some((x, y)),
                    ));
                }
                // A dangling `·` before a border or the end of the row.
                if z >= n || BORDER.contains(&row[z]) {
                    out.push((
                        finding(f, Check::Odd, "trailing separator", line.clone()),
                        Some((x, y)),
                    ));
                }
                // A leading `·` after a border or the start of the row.
                let mut a = x;
                while a > 0 && row[a - 1] == ' ' {
                    a -= 1;
                }
                if a == 0 || BORDER.contains(&row[a - 1]) {
                    out.push((
                        finding(f, Check::Odd, "leading separator", line.clone()),
                        Some((x, y)),
                    ));
                }
            }
            if c == '(' && (next == ')' || (next == ' ' && after == ')')) {
                out.push((
                    finding(f, Check::Odd, "empty brackets", line.clone()),
                    Some((x, y)),
                ));
            }
            if c == '1' && next == ' ' && (x == 0 || !row[x - 1].is_ascii_digit()) && x + 2 < n {
                let word: String = row[x + 2..]
                    .iter()
                    .take_while(|c| c.is_alphabetic())
                    .collect();
                if [
                    "sessions",
                    "folders",
                    "matches",
                    "changes",
                    "accounts",
                    "windows",
                    "settings",
                    "templates",
                    "rows",
                ]
                .contains(&word.as_str())
                {
                    out.push((
                        finding(f, Check::Odd, format!("1 {word}"), line.clone()),
                        Some((x, y)),
                    ));
                }
            }
        }
    }

    // (g) Columns: the dashboard list against its header, the details
    // panel's values against their labels.
    for b in bs.iter().filter(|_| screen) {
        let inner = |y: usize| -> &[char] { &f.rows[y][b.x0 + 1..b.x1] };
        let header = (b.y0 + 1..b.y1).find(|&y| {
            let t: String = inner(y).iter().collect();
            t.trim_start().starts_with("NAME") && t.contains("PROJECT") && t.contains("ACCT")
        });
        if let Some(hy) = header {
            let head = inner(hy);
            let cols: Vec<usize> = ["PROJECT", "STATE", "ACCT"]
                .iter()
                .filter_map(|n| {
                    let n: Vec<char> = n.chars().collect();
                    positions(head, &n).first().copied()
                })
                .collect();
            for y in hy + 1..b.y1 {
                let r = inner(y);
                let glyph = r.get(3).copied().unwrap_or(' ');
                let is_row = glyph == '▪' || ('\u{2800}'..='\u{28ff}').contains(&glyph);
                if !is_row {
                    continue;
                }
                for &c in &cols {
                    if c >= 2 && (r[c - 1] != ' ' || r[c - 2] != ' ') {
                        let t: String = r.iter().collect();
                        let h: String = head.iter().collect();
                        out.push((
                            finding(
                                f,
                                Check::Align,
                                "list column",
                                format!("{}\n{}", h.trim_end(), t.trim_end()),
                            ),
                            Some((b.x0 + 1, y)),
                        ));
                        break;
                    }
                }
            }
        }
        if is_kv_panel(f, b) {
            for y in b.y0 + 1..b.y1 {
                let r = inner(y);
                let t: String = r.iter().collect();
                let label = t.trim_start().split(' ').next().unwrap_or("");
                if !t.starts_with(' ') || t.starts_with("  ") || !KV_LABELS.contains(&label) {
                    continue;
                }
                // ` label     value`: the value at column 11; the account
                // chip starts with its painted space.
                let at = if label == "account" && r.get(11) == Some(&' ') {
                    12
                } else {
                    11
                };
                if r.len() > at && (r[10] != ' ' || r[at] == ' ') {
                    out.push((
                        finding(f, Check::Align, "details value", t.trim_end()),
                        Some((b.x0 + 1, y)),
                    ));
                }
            }
        }
    }

    // The minimal list keeps every column of a row: each session row
    // ends with its account (`main`, or `?` when unknown).
    if screen && f.look == "minimal" && f.scenario.starts_with("list-") {
        for y in 1..h.saturating_sub(2) {
            let t = f.row_text(y);
            let body = t.trim_end();
            let lead: String = t.chars().take(4).collect();
            let is_row = ["  ● ", "    ", "▶   ", "▶ ● "].contains(&lead.as_str())
                && !body.trim_start().starts_with(['├', '└', '▾', '▸'])
                && !body.trim().is_empty()
                && !t.contains('┃')
                && !t.contains('│');
            if is_row && !(body.ends_with(" main") || body.ends_with(" ?")) {
                out.push((
                    finding(f, Check::Align, "minimal row without its account", body),
                    Some((0, y)),
                ));
            }
        }
    }

    // Missing: strings the window must show.
    let all: Vec<String> = (0..h).map(|y| f.row_text(y)).collect();
    for (prefix, must) in MUST {
        if !f.scenario.starts_with(prefix) {
            continue;
        }
        let m: Vec<char> = must.chars().collect();
        if !f.rows.iter().any(|r| contains(r, &m)) {
            let head: Vec<char> = m.iter().take(10).copied().collect();
            let partly = m.len() > 10 && f.rows.iter().any(|r| contains(r, &head));
            if !partly {
                out.push((finding(f, Check::Missing, *must, all.join("\n")), None));
            }
        }
    }
    out
}

fn findings() -> &'static [Finding] {
    static FINDINGS: OnceLock<Vec<Finding>> = OnceLock::new();
    FINDINGS.get_or_init(|| {
        // Frames with the same text (the themes draw the same text) are
        // checked once; the frames of one scenario are next to each other.
        let check_part = |part: &'static [Frame]| -> Vec<Finding> {
            let mut seen: HashMap<(u16, u16, String), Vec<Finding>> = HashMap::new();
            let mut out = Vec::new();
            for f in part {
                let found = seen
                    .entry((f.w, f.h, f.text()))
                    .or_insert_with(|| check_frame(f));
                out.extend(found.iter().map(|fd| Finding {
                    look: f.look,
                    ..fd.clone()
                }));
            }
            out
        };
        let all = frames();
        let per = SIZES.len() * LOOKS.len();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = (all.len() / per).div_ceil(threads).max(1) * per;
        std::thread::scope(|scope| {
            let handles: Vec<_> = all
                .chunks(chunk)
                .map(|part| scope.spawn(move || check_part(part)))
                .collect();
            handles
                .into_iter()
                .flat_map(|h| match h.join() {
                    Ok(found) => found,
                    Err(e) => std::panic::resume_unwind(e),
                })
                .collect()
        })
    })
}

/// The findings of `check` in scenarios named `prefix…`, grouped.
fn grouped(
    check: Option<Check>,
    prefix: &str,
) -> BTreeMap<(Check, &'static str, String), Vec<&'static Finding>> {
    let mut groups: BTreeMap<(Check, &'static str, String), Vec<&Finding>> = BTreeMap::new();
    for fd in findings() {
        if check.is_some_and(|c| c != fd.check) || !fd.scenario.starts_with(prefix) {
            continue;
        }
        groups
            .entry((fd.check, fd.scenario, fd.subject.clone()))
            .or_default()
            .push(fd);
    }
    groups
}

fn describe(groups: &BTreeMap<(Check, &'static str, String), Vec<&Finding>>) -> String {
    let mut out = String::new();
    for ((check, scenario, subject), list) in groups {
        let mut sizes: Vec<(u16, u16)> = list.iter().map(|f| (f.w, f.h)).collect();
        sizes.sort();
        sizes.dedup();
        let mut looks: Vec<&str> = list.iter().map(|f| f.look).collect();
        looks.sort();
        looks.dedup();
        let small = list.iter().min_by_key(|f| (f.w, f.h)).unwrap();
        let large = list.iter().max_by_key(|f| (f.w, f.h)).unwrap();
        let (sw, sh) = sizes[0];
        let (lw, lh) = sizes[sizes.len() - 1];
        out.push_str(&format!(
            "[{}] {scenario}: {subject:?}\n  sizes {sw}x{sh} .. {lw}x{lh} ({} sizes), looks {}\n",
            check.name(),
            sizes.len(),
            looks.join(", ")
        ));
        if *check != Check::Missing {
            out.push_str(&format!(
                "  at {}x{} ({}):\n    {}\n",
                small.w,
                small.h,
                small.look,
                small.line.replace('\n', "\n    ")
            ));
            if large.w != small.w {
                out.push_str(&format!(
                    "  at {}x{} ({}):\n    {}\n",
                    large.w,
                    large.h,
                    large.look,
                    large.line.replace('\n', "\n    ")
                ));
            }
        } else {
            out.push_str(&format!("  at {}x{} ({})\n", small.w, small.h, small.look));
        }
    }
    out
}

/// Asserts that no frame of the scenarios named `prefix…` has a finding
/// of `check`.
fn assert_clean(check: Check, prefix: &str) {
    let groups = grouped(Some(check), prefix);
    assert!(
        groups.is_empty(),
        "{} finding(s) of {}:\n{}",
        groups.len(),
        check.name(),
        describe(&groups)
    );
}

// ---- the invariants -----------------------------------------------------------

/// One test per check and family of scenarios: `<check>_<family>` asserts
/// that no frame of the family's scenarios has a finding of that check.
macro_rules! invariants {
    ($( $(#[$m:meta])* $name:ident: $check:ident, $prefix:literal; )*) => {
        $(
            $(#[$m])*
            #[test]
            fn $name() {
                assert_clean(Check::$check, $prefix);
            }
        )*
    };
}

invariants! {
    border_list: Border, "list-";
    border_status: Border, "status-";
    border_filter: Border, "filter-";
    border_logs: Border, "logs-";
    border_help: Border, "help-";
    border_dialog: Border, "dialog-";
    border_templates: Border, "templates-";
    border_reply: Border, "reply-";
    border_rename: Border, "rename-";
    border_handoff: Border, "handoff-";
    border_confirm: Border, "confirm-";
    border_popup: Border, "popup-";
    border_trust: Border, "trust-";
    border_picker: Border, "picker-";
    border_resume: Border, "resume-";
    border_settings: Border, "settings-";
    pad_list: Pad, "list-";
    pad_status: Pad, "status-";
    pad_filter: Pad, "filter-";
    pad_logs: Pad, "logs-";
    pad_help: Pad, "help-";
    pad_dialog: Pad, "dialog-";
    pad_templates: Pad, "templates-";
    pad_reply: Pad, "reply-";
    pad_rename: Pad, "rename-";
    pad_handoff: Pad, "handoff-";
    pad_confirm: Pad, "confirm-";
    pad_popup: Pad, "popup-";
    pad_trust: Pad, "trust-";
    pad_picker: Pad, "picker-";
    pad_resume: Pad, "resume-";
    pad_settings: Pad, "settings-";
    hint_row_list: HintRow, "list-";
    hint_row_status: HintRow, "status-";
    hint_row_filter: HintRow, "filter-";
    hint_row_logs: HintRow, "logs-";
    hint_row_help: HintRow, "help-";
    hint_row_dialog: HintRow, "dialog-";
    hint_row_templates: HintRow, "templates-";
    hint_row_reply: HintRow, "reply-";
    hint_row_rename: HintRow, "rename-";
    hint_row_handoff: HintRow, "handoff-";
    hint_row_confirm: HintRow, "confirm-";
    hint_row_popup: HintRow, "popup-";
    hint_row_trust: HintRow, "trust-";
    hint_row_picker: HintRow, "picker-";
    hint_row_resume: HintRow, "resume-";
    hint_row_settings: HintRow, "settings-";
    cut_list: Cut, "list-";
    cut_status: Cut, "status-";
    cut_filter: Cut, "filter-";
    cut_logs: Cut, "logs-";
    cut_help: Cut, "help-";
    cut_dialog: Cut, "dialog-";
    cut_templates: Cut, "templates-";
    cut_reply: Cut, "reply-";
    cut_rename: Cut, "rename-";
    cut_handoff: Cut, "handoff-";
    cut_confirm: Cut, "confirm-";
    cut_popup: Cut, "popup-";
    cut_trust: Cut, "trust-";
    cut_picker: Cut, "picker-";
    cut_resume: Cut, "resume-";
    cut_settings: Cut, "settings-";
    mid_word_list: MidWord, "list-";
    mid_word_status: MidWord, "status-";
    mid_word_filter: MidWord, "filter-";
    mid_word_logs: MidWord, "logs-";
    mid_word_help: MidWord, "help-";
    mid_word_dialog: MidWord, "dialog-";
    mid_word_templates: MidWord, "templates-";
    mid_word_reply: MidWord, "reply-";
    mid_word_rename: MidWord, "rename-";
    mid_word_handoff: MidWord, "handoff-";
    mid_word_confirm: MidWord, "confirm-";
    mid_word_popup: MidWord, "popup-";
    mid_word_trust: MidWord, "trust-";
    mid_word_picker: MidWord, "picker-";
    mid_word_resume: MidWord, "resume-";
    mid_word_settings: MidWord, "settings-";
    could_wrap_list: CouldWrap, "list-";
    could_wrap_status: CouldWrap, "status-";
    could_wrap_filter: CouldWrap, "filter-";
    could_wrap_logs: CouldWrap, "logs-";
    could_wrap_help: CouldWrap, "help-";
    could_wrap_dialog: CouldWrap, "dialog-";
    could_wrap_templates: CouldWrap, "templates-";
    could_wrap_reply: CouldWrap, "reply-";
    could_wrap_rename: CouldWrap, "rename-";
    could_wrap_handoff: CouldWrap, "handoff-";
    could_wrap_confirm: CouldWrap, "confirm-";
    could_wrap_popup: CouldWrap, "popup-";
    could_wrap_trust: CouldWrap, "trust-";
    could_wrap_picker: CouldWrap, "picker-";
    could_wrap_resume: CouldWrap, "resume-";
    could_wrap_settings: CouldWrap, "settings-";
    odd_list: Odd, "list-";
    odd_status: Odd, "status-";
    odd_filter: Odd, "filter-";
    odd_logs: Odd, "logs-";
    odd_help: Odd, "help-";
    odd_dialog: Odd, "dialog-";
    odd_templates: Odd, "templates-";
    odd_reply: Odd, "reply-";
    odd_rename: Odd, "rename-";
    odd_handoff: Odd, "handoff-";
    odd_confirm: Odd, "confirm-";
    odd_popup: Odd, "popup-";
    odd_trust: Odd, "trust-";
    odd_picker: Odd, "picker-";
    odd_resume: Odd, "resume-";
    odd_settings: Odd, "settings-";
    align_list: Align, "list-";
    align_status: Align, "status-";
    align_filter: Align, "filter-";
    align_logs: Align, "logs-";
    align_help: Align, "help-";
    align_dialog: Align, "dialog-";
    align_templates: Align, "templates-";
    align_reply: Align, "reply-";
    align_rename: Align, "rename-";
    align_handoff: Align, "handoff-";
    align_confirm: Align, "confirm-";
    align_popup: Align, "popup-";
    align_trust: Align, "trust-";
    align_picker: Align, "picker-";
    align_resume: Align, "resume-";
    align_settings: Align, "settings-";
    missing_list: Missing, "list-";
    missing_status: Missing, "status-";
    missing_filter: Missing, "filter-";
    missing_logs: Missing, "logs-";
    missing_help: Missing, "help-";
    missing_dialog: Missing, "dialog-";
    missing_templates: Missing, "templates-";
    missing_reply: Missing, "reply-";
    missing_rename: Missing, "rename-";
    missing_handoff: Missing, "handoff-";
    missing_confirm: Missing, "confirm-";
    missing_popup: Missing, "popup-";
    missing_trust: Missing, "trust-";
    missing_picker: Missing, "picker-";
    missing_resume: Missing, "resume-";
    missing_settings: Missing, "settings-";
}

// ---- targeted invariants ---------------------------------------------------

/// The frame of `scenario` in `look` at `w` × `h`.
fn frame(scenario: &str, look: &str, w: u16, h: u16) -> &'static Frame {
    frames()
        .iter()
        .find(|f| f.scenario == scenario && f.look == look && f.w == w && f.h == h)
        .unwrap_or_else(|| panic!("no frame {scenario} {look} {w}x{h}"))
}

fn rows_of(f: &Frame) -> Vec<String> {
    (0..f.rows.len()).map(|y| f.row_text(y)).collect()
}

#[test]
fn the_light_theme_draws_the_same_text_as_neutral() {
    for f in frames().iter().filter(|f| f.look == "dash-light") {
        let neutral = frame(f.scenario, "dash-neutral", f.w, f.h);
        assert_eq!(f.text(), neutral.text(), "{} {}x{}", f.scenario, f.w, f.h);
    }
}

#[test]
fn the_details_keys_row_keeps_every_key_that_fits() {
    // print-date offers Enter, Space, a, s and ! (doc/0020, doc/0037).
    for (w, h) in [(140, 40), (160, 45), (250, 60)] {
        let rows = rows_of(frame("list-git-clean", "dash-neutral", w, h));
        let keys = rows
            .iter()
            .find(|r| r.contains("Enter attach  Space logs"))
            .expect("the keys row");
        assert!(
            keys.contains("s stop") && keys.contains("! shell"),
            "{keys}"
        );
    }
}

#[test]
fn the_dialog_model_field_shows_a_long_model_name() {
    let rows = rows_of(frame("dialog-model-long", "dash-neutral", 80, 24));
    let model = rows
        .iter()
        .find(|r| r.contains("▶ Model"))
        .expect("the model row");
    assert!(model.contains("experimental-long-context"), "{model}");
}

#[test]
fn settings_values_use_the_width_of_their_row() {
    for (w, h) in [(80, 24), (140, 40), (250, 60)] {
        let rows = rows_of(frame("settings-style", "dash-neutral", w, h));
        let row = rows
            .iter()
            .find(|r| r.contains("hand-off prompt  "))
            .expect("the hand-off prompt row");
        // A value cut with `…` leaves no run of free columns after it.
        let after = row.split('…').nth(1).unwrap_or("");
        let free = after.chars().take_while(|c| *c == ' ').count();
        assert!(
            free < 4,
            "{w}x{h}: {free} free columns after the cut: {row}"
        );
    }
}

#[test]
fn settings_account_rows_show_whole_values_when_there_is_room() {
    let rows = rows_of(frame("settings-accounts", "dash-neutral", 160, 45));
    let row = rows
        .iter()
        .find(|r| r.contains("altern"))
        .expect("the second account");
    for value in [
        "alternative-account",
        "claude-alternative-wrapper",
        "personal-sandbox",
    ] {
        assert!(row.contains(value), "{value}: {row}");
    }
}

#[test]
fn the_folder_list_keeps_a_gap_after_each_folder() {
    for (w, h) in [(80, 24), (120, 36), (250, 60)] {
        for row in rows_of(frame("settings-roots-editor", "dash-neutral", w, h)) {
            let cs: Vec<char> = row.chars().collect();
            let glued = cs.windows(2).any(|p| p[0] == '…' && p[1].is_alphanumeric());
            assert!(!glued, "{w}x{h}: {row}");
        }
    }
}

/// The checks themselves, on a hand-made frame: a window with a hint cut
/// mid-word, a wrapped hint, `↑0` and a value cut with `…`.
#[test]
fn the_checks_find_cuts_wraps_and_odd_values() {
    let placeholder = "describe the task · Enter new line · Ctrl+G opens $VISUAL / $EDITOR · \
                       Ctrl+P inserts a template";
    let footer = "Tab/Shift+Tab field · Enter next field · Ctrl+S start · Alt+Enter start & \
                  attach · Ctrl+G editor · Ctrl+P template";
    let inner = 44;
    let row = |text: &str| -> Vec<char> {
        let mut r = vec![' ', '│'];
        let mut t: Vec<char> = text.chars().collect();
        t.resize(inner, ' ');
        r.extend(t);
        r.extend(['│', ' ']);
        r
    };
    let edge = |l: char, r: char| -> Vec<char> {
        let mut v = vec![' ', l];
        v.extend(std::iter::repeat_n('─', inner));
        v.extend([r, ' ']);
        v
    };
    // Clipped at the border inside `Ctrl`.
    let cut: String = placeholder.chars().take(inner - 4).collect();
    // Wrapped at a space, as `wrap` does: the `·` ends the first row.
    let (first, second) = footer.split_at(footer.find(" Ctrl+S").unwrap());
    let rows = vec![
        edge('╭', '╮'),
        row(&format!("    {cut}")),
        row(&format!(" {first}")),
        row(&format!(" {}", &second.trim_start()[..24])),
        row(" git  main · clean · ↑0"),
        edge('╰', '╯'),
    ];
    let f = Frame {
        scenario: "selftest",
        look: "dash-neutral",
        w: (inner + 4) as u16,
        h: rows.len() as u16,
        rows,
    };
    let found = check_frame(&f);
    let has = |check: Check, subject: &str| {
        found
            .iter()
            .any(|fd| fd.check == check && fd.subject.starts_with(subject))
    };
    assert!(has(Check::MidWord, "describe the task"), "{found:#?}");
    assert!(has(Check::HintRow, "describe the task"), "{found:#?}");
    assert!(
        has(Check::HintRow, "Tab/Shift+Tab field · Enter next"),
        "{found:#?}"
    );
    assert!(
        !has(Check::Cut, "Tab/Shift+Tab field · Enter next"),
        "{found:#?}"
    );
    assert!(
        !has(Check::MidWord, "Enter new line · Ctrl+S"),
        "{found:#?}"
    );
    // Only the cut placeholder runs into the right border.
    let pads: Vec<&Finding> = found.iter().filter(|fd| fd.check == Check::Pad).collect();
    assert!(
        pads.len() == 1 && pads[0].subject.starts_with("right border"),
        "{pads:#?}"
    );
    assert!(has(Check::Odd, "↑0"), "{found:#?}");
}

#[test]
fn every_scenario_draws_at_every_size_and_look() {
    let n = scenarios().len();
    assert_eq!(frames().len(), n * SIZES.len() * LOOKS.len());
}

#[test]
#[ignore = "prints every finding; run with --ignored --nocapture"]
fn audit_report() {
    let t = Instant::now();
    let n = frames().len();
    println!("{n} frames drawn in {:?}", t.elapsed());
    let t = Instant::now();
    let m = findings().len();
    println!("{m} findings in {:?}", t.elapsed());
    let groups = grouped(None, "");
    println!("{} finding groups\n\n{}", groups.len(), describe(&groups));
}

/// Every finding grouped by check and subject, with the scenarios.
#[test]
#[ignore = "prints the findings by subject; run with --ignored --nocapture"]
fn audit_by_subject() {
    let mut groups: BTreeMap<(Check, String), Vec<&Finding>> = BTreeMap::new();
    for fd in findings() {
        groups
            .entry((fd.check, fd.subject.clone()))
            .or_default()
            .push(fd);
    }
    for ((check, subject), list) in &groups {
        let mut scen: Vec<&str> = list.iter().map(|f| f.scenario).collect();
        scen.sort();
        scen.dedup();
        let mut looks: Vec<&str> = list.iter().map(|f| f.look).collect();
        looks.sort();
        looks.dedup();
        let small = list.iter().min_by_key(|f| (f.w, f.h)).unwrap();
        let large = list.iter().max_by_key(|f| (f.w, f.h)).unwrap();
        println!(
            "[{}] {subject:?}\n  {} frames; sizes {}x{} .. {}x{}; looks {}\n  scenarios: {}",
            check.name(),
            list.len(),
            small.w,
            small.h,
            large.w,
            large.h,
            looks.join(", "),
            scen.join(", ")
        );
        if *check != Check::Missing {
            println!(
                "  e.g. {} {} {}x{}:\n    {}",
                small.scenario,
                small.look,
                small.w,
                small.h,
                small.line.replace('\n', "\n    ")
            );
        }
    }
}

/// Writes every frame to `$UI_AUDIT_DUMP/<scenario>/<look>-<w>x<h>.txt`.
#[test]
#[ignore = "writes every frame for review; set UI_AUDIT_DUMP"]
fn dump() {
    let Some(dir) = std::env::var_os("UI_AUDIT_DUMP") else {
        panic!("set UI_AUDIT_DUMP to a folder");
    };
    let dir = PathBuf::from(dir);
    for f in frames() {
        let sub = dir.join(f.scenario);
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join(format!("{}-{}x{}.txt", f.look, f.w, f.h));
        std::fs::write(path, f.text() + "\n").unwrap();
    }
}
