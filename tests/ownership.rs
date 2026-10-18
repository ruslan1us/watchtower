//! Merging listings and resolving ownership with a fake environ reader
//! (doc/0003).

mod common;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use common::listing;
use watchtower::config::Account;
use watchtower::listing::{Session, parse_listing};
use watchtower::merge::{
    AccountListing, EnvironReader, MergedSession, Ownership, Resolver, find_var, same_session,
};
use watchtower::sidecar::{AccountSource, Sidecar};

/// What the fake returns for a pid.
#[derive(Clone)]
enum Env {
    Var(String),
    Missing,
    Unreadable,
}

/// The command line of a Claude Code native binary started from `PATH`.
const CLAUDE_CMDLINE: &[u8] = b"claude\0--bg\0";
/// Where the native binary's `exe` link points: named after the version.
const CLAUDE_EXE: &str = "/home/user/.local/share/claude/versions/1.0.0";

/// A process as the fake `/proc` shows it; `None` is unreadable.
#[derive(Clone, Default)]
struct Proc {
    cmdline: Option<Vec<u8>>,
    exe: Option<PathBuf>,
    ticks: Option<u64>,
}

/// `CLAUDE_CONFIG_DIR`, command line, executable and raw start ticks per
/// pid. A pid without a process has unreadable `cmdline`, `exe` and
/// `stat`. Processes can change between merges (pid reuse) through the
/// `&self` setters.
#[derive(Default)]
struct FakeEnviron {
    env: RefCell<HashMap<u32, Env>>,
    procs: RefCell<HashMap<u32, Proc>>,
}

impl FakeEnviron {
    fn with(self, pid: u32, env: Env) -> Self {
        self.set_env(pid, env);
        self
    }

    /// The process holding `pid` now has `env`.
    fn set_env(&self, pid: u32, env: Env) {
        self.env.borrow_mut().insert(pid, env);
    }

    /// `pid` is a Claude Code process that started at `ticks`.
    fn claude(self, pid: u32, ticks: u64) -> Self {
        self.process(pid, CLAUDE_CMDLINE, Some(CLAUDE_EXE), Some(ticks))
    }

    /// `pid` runs `cmdline` (NUL-separated) from `exe`, started at `ticks`.
    fn process(self, pid: u32, cmdline: &[u8], exe: Option<&str>, ticks: Option<u64>) -> Self {
        self.procs.borrow_mut().insert(
            pid,
            Proc {
                cmdline: Some(cmdline.to_vec()),
                exe: exe.map(PathBuf::from),
                ticks,
            },
        );
        self
    }

    /// Every listed process is Claude Code: the pids are the rows' own
    /// processes, not reused ones.
    fn matching(self, rows: &[Session]) -> Self {
        rows.iter()
            .filter_map(|s| s.pid)
            .fold(self, |env, pid| env.claude(pid, u64::from(pid) * 10))
    }

    /// Another process now holds `pid`: same reads, other start ticks.
    fn set_ticks(&self, pid: u32, ticks: Option<u64>) {
        self.procs.borrow_mut().entry(pid).or_default().ticks = ticks;
    }

    /// The process holding `pid` now shows `cmdline` and no `exe`.
    fn set_cmdline(&self, pid: u32, cmdline: Option<&[u8]>) {
        let mut procs = self.procs.borrow_mut();
        let p = procs.entry(pid).or_default();
        p.cmdline = cmdline.map(<[u8]>::to_vec);
        p.exe = None;
    }

    fn read<T>(&self, pid: u32, f: impl Fn(&Proc) -> Option<T>) -> io::Result<T> {
        self.procs
            .borrow()
            .get(&pid)
            .and_then(f)
            .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

impl EnvironReader for FakeEnviron {
    fn config_dir_var(&self, pid: u32) -> io::Result<Option<OsString>> {
        match self.env.borrow().get(&pid) {
            Some(Env::Var(v)) => Ok(Some(v.into())),
            Some(Env::Missing) => Ok(None),
            Some(Env::Unreadable) | None => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        }
    }

    fn cmdline(&self, pid: u32) -> io::Result<Vec<u8>> {
        self.read(pid, |p| p.cmdline.clone())
    }

    fn exe(&self, pid: u32) -> io::Result<PathBuf> {
        self.read(pid, |p| p.exe.clone())
    }

    fn start_ticks(&self, pid: u32) -> io::Result<u64> {
        self.read(pid, |p| p.ticks)
    }
}

/// A fake home with real `~/.claude` and `~/.claude-alt` directories, so
/// canonicalization has something to resolve.
struct Home {
    dir: tempfile::TempDir,
    accounts: Vec<Account>,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::create_dir(dir.path().join(".claude-alt")).unwrap();
        let accounts = vec![
            Account::new("main", "claude", None, dir.path()).unwrap(),
            Account::new("alt", "claude-alt", Some("~/.claude-alt"), dir.path()).unwrap(),
        ];
        Home { dir, accounts }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn sub(&self, rel: &str) -> String {
        self.path().join(rel).to_string_lossy().into_owned()
    }

    fn merge(
        &self,
        env: &FakeEnviron,
        listings: &[AccountListing],
        sidecar: &Sidecar,
    ) -> Vec<MergedSession> {
        Resolver::new(&self.accounts, self.path(), env).merge(listings, sidecar)
    }
}

fn al(account: &str, sessions: Option<Vec<Session>>) -> AccountListing {
    match sessions {
        Some(rows) => AccountListing::fresh(account, rows),
        None => AccountListing::missing(account),
    }
}

fn find<'a>(merged: &'a [MergedSession], name: &str) -> &'a MergedSession {
    merged
        .iter()
        .find(|m| m.session.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no {name}"))
}

fn owner(merged: &[MergedSession], name: &str) -> Ownership {
    find(merged, name).owner.clone()
}

fn known(account: &str, source: AccountSource) -> Ownership {
    Ownership::Known {
        account: account.into(),
        source,
    }
}

#[test]
fn shared_lists_are_deduplicated_and_resolved_by_proc() {
    let home = Home::new();
    let rows = listing("after-stop-all.json").sessions;
    let env = FakeEnviron::default()
        .with(4241, Env::Missing) // bad-model: default ~/.claude → main
        .with(4240, Env::Var(home.sub(".claude"))) // run-test-suite → main
        .with(4290, Env::Var(home.sub(".claude-alt"))) // print-date → alt
        .with(4315, Env::Var("~/.claude-alt".into())) // pick-colour: literal ~, not a path
        .with(4377, Env::Unreadable) // touch-config: falls through
        .with(1201, Env::Var("/elsewhere/.claude-x".into())) // interactive: unmatched
        .matching(&rows);
    let listings = [
        al("main", Some(rows.clone())),
        al("alt", Some(rows.clone())),
    ];
    let merged = home.merge(&env, &listings, &Sidecar::default());

    assert_eq!(merged.len(), rows.len(), "shared rows merged once");
    for m in &merged {
        assert_eq!(m.seen_in, ["main", "alt"]);
    }
    assert_eq!(
        owner(&merged, "bad-model"),
        known("main", AccountSource::Proc)
    );
    assert_eq!(
        owner(&merged, "run-test-suite"),
        known("main", AccountSource::Proc)
    );
    assert_eq!(
        owner(&merged, "print-date"),
        known("alt", AccountSource::Proc)
    );
    // A literal `~` is not expanded by the process: relative, never guessed.
    assert_eq!(
        owner(&merged, "pick-colour"),
        Ownership::Unknown {
            config_dir: Some(PathBuf::from("~/.claude-alt"))
        }
    );
    // Unreadable environ, shared list, no sidecar: unknown.
    assert_eq!(
        owner(&merged, "touch-config"),
        Ownership::Unknown { config_dir: None }
    );
    // Unmatched config dir: unknown, raw path kept.
    assert_eq!(
        owner(&merged, "demo-app-3f"),
        Ownership::Unknown {
            config_dir: Some(PathBuf::from("/elsewhere/.claude-x"))
        }
    );
    assert_eq!(find(&merged, "demo-app-3f").owner.label(), "?");
    // Stopped row, shared list, no sidecar: unknown.
    assert_eq!(
        owner(&merged, "say-ok"),
        Ownership::Unknown { config_dir: None }
    );
}

#[test]
fn symlinked_config_dir_matches() {
    let home = Home::new();
    std::os::unix::fs::symlink(
        home.path().join(".claude-alt"),
        home.path().join("alt-link"),
    )
    .unwrap();
    let rows = listing("mixed-states.json").sessions;
    let env = FakeEnviron::default()
        .with(4290, Env::Var(home.sub("alt-link")))
        .matching(&rows);
    let merged = home.merge(
        &env,
        &[al("main", Some(rows.clone())), al("alt", Some(rows))],
        &Sidecar::default(),
    );
    assert_eq!(
        owner(&merged, "print-date"),
        known("alt", AccountSource::Proc)
    );

    // And the other way round: the configured dir is the symlink.
    let link_home = Home::new();
    std::os::unix::fs::symlink(
        link_home.path().join(".claude-alt"),
        link_home.path().join("alt-link"),
    )
    .unwrap();
    let accounts = vec![
        link_home.accounts[0].clone(),
        Account::new("alt", "claude-alt", Some("~/alt-link"), link_home.path()).unwrap(),
    ];
    let rows = listing("mixed-states.json").sessions;
    let env = FakeEnviron::default()
        .with(4290, Env::Var(link_home.sub(".claude-alt")))
        .matching(&rows);
    let merged = Resolver::new(&accounts, link_home.path(), &env).merge(
        &[al("main", Some(rows.clone())), al("alt", Some(rows))],
        &Sidecar::default(),
    );
    assert_eq!(
        owner(&merged, "print-date"),
        known("alt", AccountSource::Proc)
    );
}

#[test]
fn separate_lists_resolve_by_list() {
    let home = Home::new();
    let all = listing("after-stop-all.json").sessions;
    let (main_rows, alt_rows): (Vec<_>, Vec<_>) = all.into_iter().partition(|s| {
        matches!(
            s.name.as_deref(),
            Some("say-ok" | "bad-model" | "demo-app-3f")
        )
    });
    // No environ readable at all.
    let env = FakeEnviron::default();
    let merged = home.merge(
        &env,
        &[al("main", Some(main_rows)), al("alt", Some(alt_rows))],
        &Sidecar::default(),
    );
    assert_eq!(owner(&merged, "say-ok"), known("main", AccountSource::List));
    assert_eq!(
        owner(&merged, "bad-model"),
        known("main", AccountSource::List)
    );
    assert_eq!(
        owner(&merged, "touch-config"),
        known("alt", AccountSource::List)
    );
    assert_eq!(find(&merged, "touch-config").seen_in, ["alt"]);

    // An empty successful listing alone is no evidence (doc/0017): the
    // supervisor may be starting, or the listing may be a moment old.
    let rows = listing("after-stop-all.json").sessions;
    let merged = home.merge(
        &env,
        &[al("main", Some(rows)), al("alt", Some(vec![]))],
        &Sidecar::default(),
    );
    assert_eq!(
        owner(&merged, "say-ok"),
        Ownership::Unknown { config_dir: None }
    );
}

#[test]
fn failed_poll_is_not_evidence_and_sidecar_is_used() {
    let home = Home::new();
    let rows = listing("after-stop-all.json").sessions;
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "alt",
        AccountSource::Started,
    );
    let env = FakeEnviron::default();
    let merged = home.merge(&env, &[al("main", Some(rows)), al("alt", None)], &sidecar);
    assert_eq!(
        owner(&merged, "say-ok"),
        known("alt", AccountSource::Started)
    );
    assert_eq!(
        owner(&merged, "print-date"),
        Ownership::Unknown { config_dir: None }
    );

    // A missing listing entry for a configured account counts as failed too.
    let rows = listing("after-stop-all.json").sessions;
    let merged = home.merge(&env, &[al("main", Some(rows))], &sidecar);
    assert_eq!(
        owner(&merged, "print-date"),
        Ownership::Unknown { config_dir: None }
    );
}

#[test]
fn proc_beats_sidecar_and_list() {
    let home = Home::new();
    let rows = listing("mixed-states.json").sessions;
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(
        "7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93",
        "main",
        AccountSource::Started,
    );
    let env = FakeEnviron::default()
        .with(4290, Env::Var(home.sub(".claude-alt")))
        .matching(&rows);
    // Listed by main only, but the process runs under alt (woken by hand).
    let merged = home.merge(
        &env,
        &[al("main", Some(rows)), al("alt", Some(vec![]))],
        &sidecar,
    );
    assert_eq!(
        owner(&merged, "print-date"),
        known("alt", AccountSource::Proc)
    );

    assert!(sidecar.record_detections(&merged));
    assert_eq!(
        sidecar.owner("7b4d9f02-3c1e-4a7b-b5d6-2e8f0a1c4b93"),
        Some(("alt", &AccountSource::Proc))
    );
}

#[test]
fn sidecar_naming_unconfigured_account_is_ignored() {
    let home = Home::new();
    let rows = listing("after-stop-all.json").sessions;
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "gone",
        AccountSource::Asked,
    );
    let merged = home.merge(
        &FakeEnviron::default(),
        &[al("main", Some(rows.clone())), al("alt", Some(rows))],
        &sidecar,
    );
    assert_eq!(
        owner(&merged, "say-ok"),
        Ownership::Unknown { config_dir: None }
    );
}

#[test]
fn single_account_owns_everything_implicitly() {
    let home = Home::new();
    let accounts = vec![home.accounts[0].clone()];
    let rows = listing("after-stop-all.json").sessions;
    // No environ readable: every row falls to the implicit owner.
    let merged = Resolver::new(&accounts, home.path(), &FakeEnviron::default())
        .merge(&[al("main", Some(rows))], &Sidecar::default());
    assert_eq!(merged.len(), 7);
    for m in &merged {
        assert_eq!(
            m.owner,
            known("main", AccountSource::Implicit),
            "{:?}",
            m.session.name
        );
    }
    // The implicit owner is not evidence and is never persisted, so a second
    // account with a shared listing added later finds no stale records.
    let mut sidecar = Sidecar::default();
    assert!(!sidecar.record_detections(&merged));
    assert!(sidecar.sessions.is_empty());
}

#[test]
fn single_account_still_applies_rule_one() {
    let home = Home::new();
    let accounts = vec![home.accounts[0].clone()];
    let rows = listing("after-stop-all.json").sessions;
    let env = FakeEnviron::default()
        .with(4241, Env::Var("/elsewhere/.claude-x".into())) // bad-model: not main's dir
        .with(4240, Env::Missing) // run-test-suite: default ~/.claude = main
        .matching(&rows);
    let mut sidecar = Sidecar::default();
    // A record naming the only account is still honoured over the implicit owner.
    sidecar.record_owner(
        "b2f8d064-6a3c-4e1f-9d58-0c7a2b4e6f81",
        "main",
        AccountSource::Started,
    );
    let merged =
        Resolver::new(&accounts, home.path(), &env).merge(&[al("main", Some(rows))], &sidecar);
    // A live process under another config dir is not silently the only account.
    assert_eq!(
        owner(&merged, "bad-model"),
        Ownership::Unknown {
            config_dir: Some(PathBuf::from("/elsewhere/.claude-x"))
        }
    );
    assert_eq!(
        owner(&merged, "run-test-suite"),
        known("main", AccountSource::Proc)
    );
    assert_eq!(
        owner(&merged, "say-ok"),
        known("main", AccountSource::Started)
    );
    assert_eq!(
        owner(&merged, "print-date"),
        known("main", AccountSource::Implicit)
    );
    // Only the genuine detection is recorded: no `list` records at all.
    assert!(sidecar.record_detections(&merged));
    let sources: Vec<_> = sidecar
        .sessions
        .values()
        .filter_map(|r| r.account_source.clone())
        .collect();
    assert_eq!(sources.len(), 2, "{sources:?}");
    assert!(sources.contains(&AccountSource::Proc));
    assert!(sources.contains(&AccountSource::Started));
    assert!(!sources.contains(&AccountSource::List));
}

#[test]
fn live_row_wins_over_stopped_duplicate() {
    let home = Home::new();
    let stopped = listing("after-stop-all.json").sessions;
    let live = listing("mixed-states.json").sessions;
    let merged = home.merge(
        &FakeEnviron::default(),
        &[al("main", Some(stopped)), al("alt", Some(live))],
        &Sidecar::default(),
    );
    let say_ok = find(&merged, "say-ok");
    assert_eq!(say_ok.session.pid, Some(4342));
    assert_eq!(say_ok.seen_in, ["main", "alt"]);
}

#[test]
fn dedup_falls_back_to_id_then_pid() {
    let a = parse_listing(r#"[
        { "kind": "background", "id": "1a2b3c4d", "name": "x" },
        { "kind": "interactive", "pid": 4242, "name": "y" },
        { "kind": "background", "sessionId": "5e6f7a8b-0000-4000-8000-000000000000", "id": "5e6f7a8b", "name": "z" }
    ]"#).unwrap().sessions;
    let b = parse_listing(r#"[
        { "kind": "background", "id": "1a2b3c4d", "sessionId": "1a2b3c4d-0000-4000-8000-000000000000", "name": "x" },
        { "kind": "interactive", "pid": 4242, "name": "y" },
        { "kind": "background", "sessionId": "9c8d7e6f-0000-4000-8000-000000000000", "id": "5e6f7a8b", "name": "z2" }
    ]"#).unwrap().sessions;
    assert!(same_session(&a[0], &b[0]), "by id when one lacks sessionId");
    assert!(same_session(&a[1], &b[1]), "by pid");
    assert!(
        !same_session(&a[2], &b[2]),
        "different sessionIds differ even with equal id"
    );
    let home = Home::new();
    let merged = home.merge(
        &FakeEnviron::default(),
        &[al("main", Some(a)), al("alt", Some(b))],
        &Sidecar::default(),
    );
    assert_eq!(merged.len(), 4);
}

#[test]
fn environ_block_parsing() {
    let block =
        b"PATH=/usr/bin\0CLAUDE_CONFIG_DIR=/home/user/.claude-alt\0X_CLAUDE_CONFIG_DIR=no\0";
    assert_eq!(
        find_var(block, "CLAUDE_CONFIG_DIR"),
        Some(OsString::from("/home/user/.claude-alt"))
    );
    assert_eq!(find_var(b"PATH=/usr/bin\0", "CLAUDE_CONFIG_DIR"), None);
    assert_eq!(find_var(b"", "CLAUDE_CONFIG_DIR"), None);
}

#[test]
fn proc_environ_reads_own_process() {
    use watchtower::merge::ProcEnviron;
    // Our own process is always readable; whatever the variable is, no error.
    assert!(ProcEnviron.config_dir_var(std::process::id()).is_ok());
    // A pid that cannot exist.
    assert!(ProcEnviron.config_dir_var(u32::MAX).is_err());
}

#[test]
fn stale_rows_are_shown_but_not_evidence() {
    let home = Home::new();
    let all = listing("after-stop-all.json").sessions;
    let (main_rows, alt_rows): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|s| matches!(s.name.as_deref(), Some("say-ok" | "bad-model")));
    // alt's latest poll failed; its last good rows are kept, marked stale.
    let merged = home.merge(
        &FakeEnviron::default(),
        &[
            AccountListing::fresh("main", main_rows),
            AccountListing::stale("alt", alt_rows),
        ],
        &Sidecar::default(),
    );
    assert_eq!(merged.len(), 7, "stale rows stay visible");
    // Rule 2 needs every account to be freshly polled.
    assert_eq!(
        owner(&merged, "say-ok"),
        Ownership::Unknown { config_dir: None }
    );
    assert_eq!(
        owner(&merged, "touch-config"),
        Ownership::Unknown { config_dir: None }
    );
    assert!(!AccountListing::stale("alt", vec![]).is_fresh());
    assert!(!AccountListing::missing("alt").is_fresh());
    assert!(AccountListing::fresh("alt", vec![]).is_fresh());
}

#[test]
fn fresh_row_replaces_stale_duplicate() {
    let home = Home::new();
    let live = listing("mixed-states.json").sessions;
    let stopped = listing("after-stop-all.json").sessions;
    // The stale snapshot (first in config order) still shows say-ok live;
    // the fresh poll says it has stopped. The fresh row wins.
    let merged = home.merge(
        &FakeEnviron::default(),
        &[
            AccountListing::stale("main", live),
            AccountListing::fresh("alt", stopped),
        ],
        &Sidecar::default(),
    );
    let say_ok = find(&merged, "say-ok");
    assert_eq!(say_ok.session.pid, None);
    assert_eq!(say_ok.seen_in, ["main", "alt"]);
}

// ---- doc/0017: evidence rules ------------------------------------------

const SID_A: &str = "11111111-0000-4000-8000-000000000000";
const SID_B: &str = "22222222-0000-4000-8000-000000000000";
const SID_C: &str = "33333333-0000-4000-8000-000000000000";

/// One row from JSON.
fn row(json: &str) -> Session {
    parse_listing(&format!("[{json}]"))
        .unwrap()
        .sessions
        .remove(0)
}

/// A stopped background row (no pid).
fn stopped(name: &str, id: &str, sid: &str) -> Session {
    row(&format!(
        r#"{{"id": "{id}", "sessionId": "{sid}", "name": "{name}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "state": "done", "startedAt": 1790000000000}}"#
    ))
}

/// A live background row.
fn live(name: &str, id: &str, sid: &str, pid: u32, started_at: i64) -> Session {
    row(&format!(
        r#"{{"id": "{id}", "sessionId": "{sid}", "name": "{name}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "state": "working", "status": "busy", "pid": {pid},
            "startedAt": {started_at}}}"#
    ))
}

#[test]
fn rule_two_needs_one_poll_cycle_and_complete_non_empty_lists() {
    let home = Home::new();
    let env = FakeEnviron::default();
    let a = stopped("a", "11111111", SID_A);
    let b = stopped("b", "22222222", SID_B);
    let lists = |main_cycle, alt_cycle| {
        [
            AccountListing::fresh("main", vec![a.clone()]).with_cycle(main_cycle),
            AccountListing::fresh("alt", vec![b.clone()]).with_cycle(alt_cycle),
        ]
    };
    // Same cycle, separate non-empty lists: rule 2.
    let merged = home.merge(&env, &lists(4, 4), &Sidecar::default());
    assert_eq!(owner(&merged, "a"), known("main", AccountSource::List));
    assert_eq!(owner(&merged, "b"), known("alt", AccountSource::List));
    // Snapshots from different cycles are not compared.
    let merged = home.merge(&env, &lists(4, 5), &Sidecar::default());
    assert_eq!(owner(&merged, "a"), Ownership::Unknown { config_dir: None });
    assert_eq!(owner(&merged, "b"), Ownership::Unknown { config_dir: None });
    // A listing whose parser skipped a row is no evidence.
    let [m, l] = lists(4, 4);
    let merged = home.merge(&env, &[m, l.with_partial(true)], &Sidecar::default());
    assert_eq!(owner(&merged, "a"), Ownership::Unknown { config_dir: None });
    // Nor is an empty listing, even from the same cycle.
    let merged = home.merge(
        &env,
        &[
            AccountListing::fresh("main", vec![a.clone(), b.clone()]).with_cycle(4),
            AccountListing::fresh("alt", vec![]).with_cycle(4),
        ],
        &Sidecar::default(),
    );
    assert_eq!(owner(&merged, "a"), Ownership::Unknown { config_dir: None });
}

#[test]
fn rule_two_is_skipped_when_any_row_is_listed_twice() {
    // A removed row still cached in one list, or a shared list: one row
    // seen in both listings proves the lists are not separate, so no row of
    // the merge gets rule 2.
    let home = Home::new();
    let a = stopped("a", "11111111", SID_A);
    let b = stopped("b", "22222222", SID_B);
    let c = stopped("c", "33333333", SID_C);
    let merged = home.merge(
        &FakeEnviron::default(),
        &[
            al("main", Some(vec![a.clone(), c.clone()])),
            al("alt", Some(vec![b, c])),
        ],
        &Sidecar::default(),
    );
    for name in ["a", "b", "c"] {
        assert_eq!(
            owner(&merged, name),
            Ownership::Unknown { config_dir: None },
            "{name}"
        );
    }
}

#[test]
fn rule_two_yields_to_a_non_list_record() {
    let home = Home::new();
    let a = stopped("a", "11111111", SID_A);
    let b = stopped("b", "22222222", SID_B);
    let listings = [
        al("main", Some(vec![a.clone()])),
        al("alt", Some(vec![b.clone()])),
    ];
    for source in [
        AccountSource::Started,
        AccountSource::Handoff,
        AccountSource::Asked,
        AccountSource::Proc,
    ] {
        let mut sidecar = Sidecar::default();
        sidecar.record_owner(SID_A, "alt", source.clone());
        let merged = home.merge(&FakeEnviron::default(), &listings, &sidecar);
        assert_eq!(owner(&merged, "a"), known("alt", source.clone()));
        assert!(!sidecar.record_detections(&merged[..1]), "{source:?}");
        assert_eq!(sidecar.owner(SID_A), Some(("alt", &source)));
    }
    // An older `list` record is replaced by the current rule-2 detection.
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_A, "alt", AccountSource::List);
    let merged = home.merge(&FakeEnviron::default(), &listings, &sidecar);
    assert_eq!(owner(&merged, "a"), known("main", AccountSource::List));
    assert!(sidecar.record_detections(&merged));
    assert_eq!(sidecar.owner(SID_A), Some(("main", &AccountSource::List)));
}

#[test]
fn list_detection_never_overwrites_another_source() {
    // Even if a merge did produce `list` for a session with a stronger
    // record, `record_detections` keeps the record.
    let a = stopped("a", "11111111", SID_A);
    let merged = [MergedSession {
        session: a,
        seen_in: vec!["main".into()],
        owner: known("main", AccountSource::List),
    }];
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_A, "alt", AccountSource::Handoff);
    assert!(!sidecar.record_detections(&merged));
    assert_eq!(sidecar.owner(SID_A), Some(("alt", &AccountSource::Handoff)));
    // `proc` is the live process's own word and does replace it.
    let merged = [MergedSession {
        owner: known("main", AccountSource::Proc),
        ..merged[0].clone()
    }];
    assert!(sidecar.record_detections(&merged));
    assert_eq!(sidecar.owner(SID_A), Some(("main", &AccountSource::Proc)));
}

#[test]
fn rule_one_uses_only_fresh_rows() {
    let home = Home::new();
    let env = FakeEnviron::default()
        .with(4242, Env::Var(home.sub(".claude")))
        .claude(4242, 1000);
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_C, "alt", AccountSource::Handoff);
    let row = live("c", "33333333", SID_C, 4242, 1790000000000);
    // Only alt's stale snapshot still lists the row: no `/proc` evidence.
    let merged = home.merge(
        &env,
        &[
            al("main", Some(vec![])),
            AccountListing::stale("alt", vec![row.clone()]),
        ],
        &sidecar,
    );
    assert_eq!(owner(&merged, "c"), known("alt", AccountSource::Handoff));
    assert!(!sidecar.record_detections(&merged));
    // From a fresh listing the same process is evidence.
    let merged = home.merge(
        &env,
        &[al("main", Some(vec![])), al("alt", Some(vec![row]))],
        &sidecar,
    );
    assert_eq!(owner(&merged, "c"), known("main", AccountSource::Proc));
}

/// One fresh row `c` (pid 4242) listed by main, with an `alt` hand-off
/// record, so "no rule-1 evidence" shows as the record.
fn reuse_case() -> (Vec<AccountListing>, Sidecar) {
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_C, "alt", AccountSource::Handoff);
    let row = live("c", "33333333", SID_C, 4242, 1790000000000);
    (vec![al("main", Some(vec![row]))], sidecar)
}

fn handoff() -> Ownership {
    known("alt", AccountSource::Handoff)
}

fn proc_main() -> Ownership {
    known("main", AccountSource::Proc)
}

#[test]
fn rule_one_accepts_a_first_sighting_only_for_claude_code() {
    let home = Home::new();
    let (listings, sidecar) = reuse_case();
    let resolve = |cmdline: &[u8], exe: Option<&str>| {
        let env =
            FakeEnviron::default()
                .with(4242, Env::Missing)
                .process(4242, cmdline, exe, Some(777));
        owner(&home.merge(&env, &listings, &sidecar), "c")
    };
    let native = "/home/user/.local/share/claude/versions/1.0.0";
    // Claude Code, in the forms it runs as.
    for (cmdline, exe) in [
        // `claude` on PATH: argv[0] is the bare name.
        (&b"claude\0"[..], Some(native)),
        (b"claude\0\0", None),
        // The native binary by its path, also without a readable exe.
        (
            b"/home/user/.local/share/claude/versions/1.0.0\0--bg\0",
            None,
        ),
        (b"/home/user/.local/bin/claude\0", None),
        // argv[0] renamed (a wrapper's symlink): the exe gives it away,
        // also when the version was replaced by an update.
        (b"claude-alt\0", Some(native)),
        (
            b"claude-alt\0",
            Some("/home/user/.local/share/claude/versions/1.0.0 (deleted)"),
        ),
        // node with the npm package's script.
        (
            b"node\0/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js\0--bg\0",
            Some("/usr/bin/node"),
        ),
        (
            b"/usr/bin/node\0--no-warnings\0/home/user/.npm-global/bin/claude\0",
            None,
        ),
        (
            b"node-wrapper\0/opt/x/@anthropic-ai/claude-code/cli.js\0",
            Some("/usr/bin/nodejs"),
        ),
    ] {
        assert_eq!(
            resolve(cmdline, exe),
            proc_main(),
            "{:?} {exe:?}",
            String::from_utf8_lossy(cmdline)
        );
    }
    // Anything else gives no evidence; the record stands.
    for (cmdline, exe) in [
        (
            &b"/usr/bin/python3\0-m\0http.server\0"[..],
            Some("/usr/bin/python3.12"),
        ),
        (b"bash\0-c\0claude\0", Some("/usr/bin/bash")),
        (b"vim\0/home/user/claude\0", Some("/usr/bin/vim")),
        (
            b"node\0/home/user/app/server.js\0claude\0",
            Some("/usr/bin/node"),
        ),
        (b"node\0--require\0", Some("/usr/bin/node")),
        (b"/home/user/claude-tools/run\0", None),
        (b"/home/user/claude/other/1.0.0\0", None),
        (b"", None),
        (b"\0", Some("/usr/bin/sleep")),
    ] {
        assert_eq!(
            resolve(cmdline, exe),
            handoff(),
            "{:?} {exe:?}",
            String::from_utf8_lossy(cmdline)
        );
    }
}

#[test]
fn rule_one_needs_a_readable_cmdline_and_stat() {
    let home = Home::new();
    let (listings, sidecar) = reuse_case();
    let resolver_owner = |env: &FakeEnviron| owner(&home.merge(env, &listings, &sidecar), "c");
    // Nothing readable.
    let env = FakeEnviron::default().with(4242, Env::Missing);
    assert_eq!(resolver_owner(&env), handoff());
    // A readable stat but no cmdline (nor exe).
    let env = FakeEnviron::default()
        .with(4242, Env::Missing)
        .claude(4242, 777);
    env.set_cmdline(4242, None);
    assert_eq!(resolver_owner(&env), handoff());
    // A Claude Code cmdline but no stat.
    let env = FakeEnviron::default().with(4242, Env::Missing).process(
        4242,
        CLAUDE_CMDLINE,
        Some(CLAUDE_EXE),
        None,
    );
    assert_eq!(resolver_owner(&env), handoff());
    let mut resolver = Resolver::new(&home.accounts, home.path(), &env);
    resolver.merge(&listings, &sidecar);
    assert!(resolver.known().is_empty());
}

#[test]
fn rule_one_compares_start_ticks_on_later_polls() {
    let home = Home::new();
    let (listings, sidecar) = reuse_case();
    let env = FakeEnviron::default()
        .with(4242, Env::Missing)
        .claude(4242, 777);
    let mut resolver = Resolver::new(&home.accounts, home.path(), &env);
    // First sighting: Claude Code, ticks remembered.
    assert_eq!(
        owner(&resolver.merge(&listings, &sidecar), "c"),
        proc_main()
    );
    assert_eq!(resolver.known().get(SID_C, 4242), Some(777));
    // Later polls with the same ticks: the same process. The cmdline is no
    // longer looked at.
    env.set_cmdline(4242, Some(b"renamed\0"));
    for _ in 0..3 {
        assert_eq!(
            owner(&resolver.merge(&listings, &sidecar), "c"),
            proc_main()
        );
    }
    // The pid now belongs to another process (different ticks), even one
    // that looks like Claude Code: no evidence, and the pair is forgotten.
    env.set_cmdline(4242, Some(CLAUDE_CMDLINE));
    env.set_ticks(4242, Some(9_999));
    assert_eq!(owner(&resolver.merge(&listings, &sidecar), "c"), handoff());
    assert_eq!(resolver.known().get(SID_C, 4242), None);
    // The next poll is a first sighting again: accepted as Claude Code.
    assert_eq!(
        owner(&resolver.merge(&listings, &sidecar), "c"),
        proc_main()
    );
    assert_eq!(resolver.known().get(SID_C, 4242), Some(9_999));
    // ... or not, when the new process is something else.
    env.set_ticks(4242, Some(12_345));
    env.set_cmdline(4242, Some(b"/usr/bin/python3\0"));
    assert_eq!(owner(&resolver.merge(&listings, &sidecar), "c"), handoff());
    assert_eq!(owner(&resolver.merge(&listings, &sidecar), "c"), handoff());
    assert!(resolver.known().is_empty());
    // An unreadable stat on a later poll: no evidence, forgotten.
    let env = FakeEnviron::default()
        .with(4242, Env::Missing)
        .claude(4242, 777);
    let mut resolver = Resolver::new(&home.accounts, home.path(), &env);
    assert_eq!(
        owner(&resolver.merge(&listings, &sidecar), "c"),
        proc_main()
    );
    env.set_ticks(4242, None);
    assert_eq!(owner(&resolver.merge(&listings, &sidecar), "c"), handoff());
    assert!(resolver.known().is_empty());
}

#[test]
fn remembered_processes_are_forgotten_with_their_row_or_pid() {
    let home = Home::new();
    let sidecar = Sidecar::default();
    let env = FakeEnviron::default()
        .with(4242, Env::Missing)
        .with(5000, Env::Missing)
        .claude(4242, 777)
        .claude(5000, 888);
    let mut resolver = Resolver::new(&home.accounts, home.path(), &env);
    let c = live("c", "33333333", SID_C, 4242, 1790000000000);
    let b = live("b", "22222222", SID_B, 5000, 1790000000000);
    resolver.merge(&[al("main", Some(vec![c.clone(), b.clone()]))], &sidecar);
    assert_eq!(resolver.known().len(), 2);
    // Kept only in a stale snapshot: no evidence, but the pair stays.
    resolver.merge(
        &[AccountListing::stale("main", vec![c.clone(), b.clone()])],
        &sidecar,
    );
    assert_eq!(resolver.known().len(), 2);
    // b disappears, c is woken under another pid.
    let mut woken = c.clone();
    woken.pid = Some(5000);
    resolver.merge(&[al("main", Some(vec![woken]))], &sidecar);
    assert_eq!(resolver.known().get(SID_B, 5000), None);
    assert_eq!(resolver.known().get(SID_C, 4242), None);
    assert_eq!(resolver.known().get(SID_C, 5000), Some(888));
    assert_eq!(resolver.known().len(), 1);
    // The same pid under another session is another pair: a first sighting.
    assert!(resolver.into_known().get(SID_B, 4242).is_none());
}

#[test]
fn rule_one_ignores_wall_clock_skew() {
    // Regression: a suspended host (a WSL2 VM paused while Windows sleeps)
    // or a stepped wall clock put hours between `startedAt` and the boot
    // clock, and every live session lost rule-1 evidence. No wall-clock
    // time is an input any more.
    let home = Home::new();
    let env = FakeEnviron::default()
        .with(4242, Env::Missing)
        .claude(4242, 777);
    let mut resolver = Resolver::new(&home.accounts, home.path(), &env);
    let sidecar = Sidecar::default();
    for started_at in [
        1790000000000,
        1790000000000 + 10_404_000,
        1790000000000 - 10_404_000,
        0,
        i64::MAX,
    ] {
        let row = live("c", "33333333", SID_C, 4242, started_at);
        let merged = resolver.merge(&[al("main", Some(vec![row]))], &sidecar);
        assert_eq!(owner(&merged, "c"), proc_main(), "{started_at}");
    }
    // A row without `startedAt` is no different.
    let mut row = live("c", "33333333", SID_C, 4242, 0);
    row.started_at = None;
    let merged = resolver.merge(&[al("main", Some(vec![row.clone()]))], &sidecar);
    assert_eq!(owner(&merged, "c"), proc_main());
    let merged = Resolver::new(&home.accounts, home.path(), &env)
        .merge(&[al("main", Some(vec![row]))], &sidecar);
    assert_eq!(owner(&merged, "c"), proc_main());
}

#[test]
fn the_app_remembers_processes_between_polls() {
    // pid 4242 is Claude Code under alt's config dir. Then another Claude
    // Code process, under main's, gets the pid: the app still has the first
    // one's ticks, so the swap is no evidence and the `proc` record stands.
    // The poll after accepts the new process as a first sighting.
    let home = Home::new();
    let env = Rc::new(
        FakeEnviron::default()
            .with(4242, Env::Var(home.sub(".claude-alt")))
            .claude(4242, 777),
    );
    let mut app = app_with_reader(
        &home,
        Sidecar::default(),
        Box::new(SharedEnviron(Rc::clone(&env))),
    );
    let row = [live("c", "33333333", SID_C, 4242, 1790000000000)];
    app.apply_poll(update("main", 1, &row));
    app.apply_poll(update("alt", 1, &row));
    let proc_alt = known("alt", AccountSource::Proc);
    assert_eq!(app_owner(&app, SID_C), proc_alt);
    app.apply_poll(update("main", 2, &row));
    assert_eq!(app_owner(&app, SID_C), proc_alt);
    env.set_ticks(4242, Some(9_999));
    env.set_env(4242, Env::Var(home.sub(".claude")));
    app.apply_poll(update("alt", 2, &row));
    assert_eq!(app_owner(&app, SID_C), proc_alt);
    assert_eq!(
        app.sidecar.owner(SID_C),
        Some(("alt", &AccountSource::Proc))
    );
    app.apply_poll(update("main", 3, &row));
    assert_eq!(app_owner(&app, SID_C), known("main", AccountSource::Proc));
}

/// A [`FakeEnviron`] the test keeps changing after the app took it.
struct SharedEnviron(Rc<FakeEnviron>);

impl EnvironReader for SharedEnviron {
    fn config_dir_var(&self, pid: u32) -> io::Result<Option<OsString>> {
        self.0.config_dir_var(pid)
    }

    fn cmdline(&self, pid: u32) -> io::Result<Vec<u8>> {
        self.0.cmdline(pid)
    }

    fn exe(&self, pid: u32) -> io::Result<PathBuf> {
        self.0.exe(pid)
    }

    fn start_ticks(&self, pid: u32) -> io::Result<u64> {
        self.0.start_ticks(pid)
    }
}

#[test]
fn the_most_recently_received_row_wins_the_merge() {
    let home = Home::new();
    let woken = live("c", "33333333", SID_C, 5000, 1790000000000);
    let old = stopped("c", "33333333", SID_C);
    let env = FakeEnviron::default()
        .with(5000, Env::Var(home.sub(".claude-alt")))
        .claude(5000, 1000);
    // alt's listing arrived last and shows the live process: it is kept.
    let merged = home.merge(
        &env,
        &[
            AccountListing::fresh("main", vec![old.clone()]).with_received(1),
            AccountListing::fresh("alt", vec![woken.clone()]).with_received(2),
        ],
        &Sidecar::default(),
    );
    assert_eq!(find(&merged, "c").session.pid, Some(5000));
    assert_eq!(owner(&merged, "c"), known("alt", AccountSource::Proc));
    // main's listing arrived last and shows it stopped: the older live row
    // is not used for rule 1.
    let merged = home.merge(
        &env,
        &[
            AccountListing::fresh("main", vec![old]).with_received(3),
            AccountListing::fresh("alt", vec![woken]).with_received(2),
        ],
        &Sidecar::default(),
    );
    assert_eq!(find(&merged, "c").session.pid, None);
    assert_eq!(owner(&merged, "c"), Ownership::Unknown { config_dir: None });
}

#[test]
fn config_dir_var_corner_cases() {
    let home = Home::new();
    let started_at = 1790000000000;
    let resolve = |pid: u32, env: Env| {
        let row = live("c", "33333333", SID_C, pid, started_at);
        let env = FakeEnviron::default().with(pid, env).claude(pid, 1000);
        owner(
            &home.merge(&env, &[al("main", Some(vec![row]))], &Sidecar::default()),
            "c",
        )
    };
    // Empty is unset: the default ~/.claude, main.
    assert_eq!(
        resolve(1, Env::Var(String::new())),
        known("main", AccountSource::Proc)
    );
    assert_eq!(resolve(2, Env::Missing), known("main", AccountSource::Proc));
    // Relative values depend on the process's cwd: `?` with the raw value.
    for raw in [".claude-alt", "./x/.claude-alt", "~/.claude-alt"] {
        assert_eq!(
            resolve(3, Env::Var(raw.into())),
            Ownership::Unknown {
                config_dir: Some(PathBuf::from(raw))
            },
            "{raw}"
        );
    }
    // A trailing slash still matches, also for a dir that does not exist.
    assert_eq!(
        resolve(4, Env::Var(format!("{}/", home.sub(".claude-alt")))),
        known("alt", AccountSource::Proc)
    );
    let missing_home = Path::new("/nonexistent-home");
    let accounts = vec![
        Account::new("main", "claude", None, missing_home).unwrap(),
        Account::new("alt", "claude-alt", Some("~/.claude-alt"), missing_home).unwrap(),
    ];
    let env = FakeEnviron::default()
        .with(5, Env::Var("/nonexistent-home/.claude-alt/".into()))
        .claude(5, 1000);
    let merged = Resolver::new(&accounts, missing_home, &env).merge(
        &[al(
            "main",
            Some(vec![live("c", "33333333", SID_C, 5, started_at)]),
        )],
        &Sidecar::default(),
    );
    assert_eq!(owner(&merged, "c"), known("alt", AccountSource::Proc));
}

#[test]
fn proc_stat_parsing() {
    use watchtower::merge::parse_start_ticks;
    // The command name may hold spaces and parentheses.
    let stat = "4242 (claude (x) y) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 7 0 \
                123456 1000 200 18446744073709551615";
    assert_eq!(parse_start_ticks(stat), Some(123456));
    assert_eq!(parse_start_ticks("4242 (short) S 1"), None);
    assert_eq!(parse_start_ticks("garbage"), None);
}

#[test]
fn proc_reads_of_own_process() {
    use watchtower::merge::{ProcEnviron, looks_like_claude};
    let pid = std::process::id();
    // The start ticks are stable: the same process reads the same value.
    let ticks = ProcEnviron.start_ticks(pid).unwrap();
    assert_eq!(ProcEnviron.start_ticks(pid).unwrap(), ticks);
    let cmdline = ProcEnviron.cmdline(pid).unwrap();
    let exe = ProcEnviron.exe(pid).unwrap();
    assert!(!cmdline.is_empty());
    // The test binary is not Claude Code.
    assert!(!looks_like_claude(&cmdline, Some(&exe)));
    assert!(ProcEnviron.start_ticks(u32::MAX).is_err());
    assert!(ProcEnviron.cmdline(u32::MAX).is_err());
    assert!(ProcEnviron.exe(u32::MAX).is_err());
}

// ---- the reviewer's scenarios through the app ----------------------------

use watchtower::app::App;
use watchtower::config::Config;
use watchtower::listing::Listing;
use watchtower::poller::PollUpdate;

fn app_with(home: &Home, sidecar: Sidecar, env: FakeEnviron) -> App {
    app_with_reader(home, sidecar, Box::new(env))
}

fn app_with_reader(home: &Home, sidecar: Sidecar, env: Box<dyn EnvironReader>) -> App {
    let config = Config::parse(
        r#"
default_account = "main"
[[accounts]]
name = "main"
command = "claude"
[[accounts]]
name = "alt"
command = "claude-alt"
config_dir = "~/.claude-alt"
"#,
        home.path(),
    )
    .unwrap();
    App::new(config, home.path().into(), sidecar, env)
}

fn update(account: &str, cycle: u64, rows: &[Session]) -> PollUpdate {
    PollUpdate {
        account: account.into(),
        cycle,
        result: Ok(Listing {
            sessions: rows.to_vec(),
            warnings: vec![],
        }),
    }
}

fn app_owner(app: &App, key: &str) -> Ownership {
    app.sessions()
        .iter()
        .find(|m| m.session.key() == Some(key))
        .map(|m| m.owner.clone())
        .unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn s1_an_empty_listing_does_not_move_a_handed_off_session() {
    let home = Home::new();
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_A, "alt", AccountSource::Handoff);
    let mut app = app_with(&home, sidecar, FakeEnviron::default());
    let a = [stopped("a", "11111111", SID_A)];
    let handoff = known("alt", AccountSource::Handoff);
    app.apply_poll(update("main", 1, &a));
    app.apply_poll(update("alt", 1, &a));
    assert_eq!(app_owner(&app, SID_A), handoff);
    // alt briefly answers with an empty list; main's poll of the same cycle
    // still lists the row.
    app.apply_poll(update("alt", 2, &[]));
    app.apply_poll(update("main", 2, &a));
    assert_eq!(app_owner(&app, SID_A), handoff);
    assert_eq!(
        app.sidecar.owner(SID_A),
        Some(("alt", &AccountSource::Handoff))
    );
    app.apply_poll(update("alt", 3, &a));
    assert_eq!(app_owner(&app, SID_A), handoff);
    assert_eq!(
        app.sidecar.owner(SID_A),
        Some(("alt", &AccountSource::Handoff))
    );
}

#[test]
fn s2_a_dispatch_race_keeps_the_started_owner() {
    let home = Home::new();
    let mut sidecar = Sidecar::default();
    sidecar.record_started("22222222", "alt");
    let mut app = app_with(&home, sidecar, FakeEnviron::default());
    // The new row (no readable pid) shows up first in main's fresh poll;
    // alt's listing of the same cycle was taken a moment earlier and holds
    // only an older session.
    let new = [
        stopped("a", "11111111", SID_A),
        live("b", "22222222", SID_B, 4343, 1790000100000),
    ];
    let other = [stopped("c", "33333333", SID_C)];
    app.apply_poll(update("alt", 1, &other));
    app.apply_poll(update("main", 1, &new));
    let started = known("alt", AccountSource::Started);
    assert_eq!(app_owner(&app, SID_B), started);
    assert_eq!(
        app.sidecar.owner(SID_B),
        Some(("alt", &AccountSource::Started))
    );
    // The lists catch up.
    let both: Vec<Session> = new.iter().chain(&other).cloned().collect();
    app.apply_poll(update("alt", 2, &both));
    app.apply_poll(update("main", 2, &both));
    assert_eq!(app_owner(&app, SID_B), started);
}

#[test]
fn s2_a_list_record_made_before_the_dispatch_is_recorded_yields() {
    let home = Home::new();
    let mut app = app_with(&home, Sidecar::default(), FakeEnviron::default());
    let new = [live("b", "22222222", SID_B, 4343, 1790000100000)];
    let other = [stopped("c", "33333333", SID_C)];
    // The poll lands before `dispatch_finished` records `started`: rule 2
    // records main.
    app.apply_poll(update("alt", 1, &other));
    app.apply_poll(update("main", 1, &new));
    assert_eq!(
        app.sidecar.owner(SID_B),
        Some(("main", &AccountSource::List))
    );
    app.sidecar.record_started("22222222", "alt");
    // On the next poll the `started` record moves to the sessionId and
    // replaces the weaker `list` record.
    app.apply_poll(update("main", 2, &new));
    assert_eq!(app.sidecar.owner("22222222"), None);
    assert_eq!(
        app.sidecar.owner(SID_B),
        Some(("alt", &AccountSource::Started))
    );
    app.apply_poll(update("alt", 2, &other));
    assert_eq!(app_owner(&app, SID_B), known("alt", AccountSource::Started));
}

#[test]
fn s3_a_stale_row_with_a_reused_pid_is_no_evidence() {
    let home = Home::new();
    let started_at = 1790000000000;
    // pid 4242 now belongs to an unrelated process without
    // CLAUDE_CONFIG_DIR (it would map to main).
    let env = FakeEnviron::default().with(4242, Env::Missing).process(
        4242,
        b"/usr/bin/python3\0-m\0http.server\0",
        Some("/usr/bin/python3.12"),
        Some(5000),
    );
    let mut sidecar = Sidecar::default();
    sidecar.record_owner(SID_C, "alt", AccountSource::Handoff);
    let mut app = app_with(&home, sidecar, env);
    let row = [live("c", "33333333", SID_C, 4242, started_at)];
    app.apply_poll(update("main", 1, &[]));
    app.apply_poll(update("alt", 1, &row));
    let handoff = known("alt", AccountSource::Handoff);
    assert_eq!(app_owner(&app, SID_C), handoff, "fresh row, reused pid");
    app.apply_poll(PollUpdate {
        account: "alt".into(),
        cycle: 2,
        result: Err("timed out".into()),
    });
    assert_eq!(app_owner(&app, SID_C), handoff, "stale row");
    assert_eq!(
        app.sidecar.owner(SID_C),
        Some(("alt", &AccountSource::Handoff))
    );
}
