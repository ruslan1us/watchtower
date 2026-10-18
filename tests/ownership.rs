//! Merging listings and resolving ownership with a fake environ reader
//! (doc/0003).

mod common;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

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

#[derive(Default)]
struct FakeEnviron(HashMap<u32, Env>);

impl FakeEnviron {
    fn with(mut self, pid: u32, env: Env) -> Self {
        self.0.insert(pid, env);
        self
    }
}

impl EnvironReader for FakeEnviron {
    fn config_dir_var(&self, pid: u32) -> io::Result<Option<OsString>> {
        match self.0.get(&pid) {
            Some(Env::Var(v)) => Ok(Some(v.into())),
            Some(Env::Missing) => Ok(None),
            Some(Env::Unreadable) | None => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        }
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
        .with(4315, Env::Var("~/.claude-alt".into())) // pick-colour: literal ~ → alt
        .with(4377, Env::Unreadable) // touch-config: falls through
        .with(1201, Env::Var("/elsewhere/.claude-x".into())); // interactive: unmatched
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
    assert_eq!(
        owner(&merged, "pick-colour"),
        known("alt", AccountSource::Proc)
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
    let env = FakeEnviron::default().with(4290, Env::Var(home.sub("alt-link")));
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
    let env = FakeEnviron::default().with(4290, Env::Var(link_home.sub(".claude-alt")));
    let rows = listing("mixed-states.json").sessions;
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

    // An empty successful listing is still evidence the lists are separate.
    let rows = listing("after-stop-all.json").sessions;
    let merged = home.merge(
        &env,
        &[al("main", Some(rows)), al("alt", Some(vec![]))],
        &Sidecar::default(),
    );
    assert_eq!(owner(&merged, "say-ok"), known("main", AccountSource::List));
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
    let env = FakeEnviron::default().with(4290, Env::Var(home.sub(".claude-alt")));
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
fn single_account_owns_everything() {
    let home = Home::new();
    let accounts = vec![home.accounts[0].clone()];
    let rows = listing("after-stop-all.json").sessions;
    let env = FakeEnviron::default().with(4241, Env::Var("/elsewhere".into()));
    let merged = Resolver::new(&accounts, home.path(), &env)
        .merge(&[al("main", Some(rows))], &Sidecar::default());
    assert_eq!(merged.len(), 7);
    for m in &merged {
        assert_eq!(
            m.owner,
            known("main", AccountSource::List),
            "{:?}",
            m.session.name
        );
    }
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
