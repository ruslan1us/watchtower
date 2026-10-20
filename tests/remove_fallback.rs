//! The remove fallback (doc/0034) against fake account commands
//! (doc/0015): when the owner's `rm` says its background service may be
//! restarting and the row has no live process, tower retries with the
//! commands of the other accounts the hand-off guard allows (doc/0018), in
//! config order; the first `removed <id>` wins.

mod common;
#[path = "support/fake_claude.rs"]
mod fake_claude;

use std::path::Path;
use std::time::Duration;

use common::{HOME, NoEnviron, ch, status_text};
use fake_claude::{Fake, lock};
use watchtower::actions::{Fallback, RemoveError, remove_with_fallback};
use watchtower::app::{App, Effect, RemoveJob};
use watchtower::cli::CliRunner;
use watchtower::config::Config;
use watchtower::jobs::{JobResult, Jobs};
use watchtower::listing::parse_listing;
use watchtower::poller::PollUpdate;
use watchtower::sidecar::Sidecar;

const ID: &str = "4c8e2a19";

/// What `rm` printed (exit 1) when the owning account's supervisor was not
/// running (reference doc, verified 2026-10-20).
const SERVICE: &str =
    "couldn't remove 4c8e2a19 — the background service may be restarting. Try again in a moment.";

/// A made-up worktree refusal with a suggested flag (doc/0009).
const WORKTREE: &str = "Refusing to remove 4c8e2a19: worktree has unpushed commits\n\
     To remove anyway and lose those commits, run:\n  \
     claude rm 4c8e2a19 --discard-unpushed 1a2b3c4@/home/user/workspace/demo-app/.claude/worktrees/orphaned-turn";

/// A fake that refuses with `text` on stderr, exit 1.
fn refusing(name: &str, text: &str) -> Fake {
    Fake::new(name, &format!("cat >&2 <<'EOF'\n{text}\nEOF\nexit 1"))
}

/// A fake whose `rm <id>` succeeds.
fn removing(name: &str) -> Fake {
    Fake::new(name, "echo \"removed $2\"")
}

/// The row: `state: blocked`, and with `live` a `pid` and `status` too.
fn row(live: bool) -> String {
    let process = if live {
        r#""pid": 4321, "status": "waiting", "waitingFor": "input needed","#
    } else {
        ""
    };
    format!(
        r#"[{{ {process} "id": "{ID}", "cwd": "/home/user/workspace/demo-app",
            "kind": "background", "startedAt": 1767232811369,
            "sessionId": "4c8e2a19-7d3b-4e1a-9c5f-2b8d0e6a1f37",
            "name": "orphaned-turn", "state": "blocked" }}]"#
    )
}

/// An app whose accounts run the given fakes, in this config order, with
/// `group` per account (empty: none) and `extra` top-level keys. Both
/// accounts list the row (shared lists), whose `started` record names
/// `owner`.
fn app_with(accounts: &[(&str, &Fake, &str)], extra: &str, owner: &str, live: bool) -> App {
    let mut toml = format!("{extra}\ndefault_account = \"{}\"\n", accounts[0].0);
    for (name, fake, group) in accounts {
        toml.push_str(&format!(
            "\n[[accounts]]\nname = \"{name}\"\ncommand = \"'{}'\"\nconfig_dir = \"~/.claude-{name}\"\n",
            fake.program().display()
        ));
        if !group.is_empty() {
            toml.push_str(&format!("group = \"{group}\"\n"));
        }
    }
    let home = Path::new(HOME);
    let config = Config::parse(&toml, home).unwrap();
    let mut app = App::new(config, home.into(), Sidecar::default(), Box::new(NoEnviron));
    app.sidecar.record_started(ID, owner);
    for (name, _, _) in accounts {
        app.apply_poll(PollUpdate {
            account: (*name).into(),
            cycle: 0,
            result: Ok(parse_listing(&row(live)).unwrap()),
        });
    }
    common::select_name(&mut app, "orphaned-turn");
    app
}

/// `d`, `y`, then the remove job run by [`Jobs`] and fed back. Returns the
/// job the app planned.
fn remove(app: &mut App) -> RemoveJob {
    assert!(app.handle_key(ch('d')).is_empty(), "asks first");
    let job = app
        .handle_key(ch('y'))
        .into_iter()
        .find_map(|e| match e {
            Effect::Remove(job) => Some(job),
            _ => None,
        })
        .expect("a remove runs");
    let jobs = Jobs::new(CliRunner::new().with_timeout(Duration::from_secs(20)));
    jobs.remove(job.clone());
    let done: JobResult = jobs
        .recv_timeout(Duration::from_secs(60))
        .expect("remove finished");
    assert!(matches!(done, JobResult::Remove(..)), "{done:?}");
    assert_eq!(done.apply(app), [Effect::PollNow], "refreshes the list");
    job
}

/// The status line's message, without the refresh age.
fn message(app: &App) -> String {
    let text = status_text(app);
    text.split(" | ").next().unwrap_or_default().to_owned()
}

fn popup_text(app: &App) -> String {
    app.popup().expect("refusal window").lines.join("\n")
}

#[test]
fn another_account_removes_after_the_service_refusal() {
    let _g = lock();
    let main = removing("claude");
    let alt = refusing("claude-alt", SERVICE);
    let mut app = app_with(&[("main", &main, ""), ("alt", &alt, "")], "", "alt", false);
    let job = remove(&mut app);

    assert_eq!(job.action.account.name, "alt", "the owner goes first");
    assert!(
        matches!(&job.fallback, Fallback::Others(o) if o.len() == 1 && o[0].name == "main"),
        "{:?}",
        job.fallback
    );
    assert_eq!(alt.all_argv(), [["rm", ID]]);
    assert_eq!(main.all_argv(), [["rm", ID]]);
    assert_eq!(
        message(&app),
        "removed orphaned-turn (with main's command; alt's background service was not answering)"
    );
    assert!(app.popup().is_none());
}

#[test]
fn every_refusal_keeps_the_dialog_and_names_the_accounts_tried() {
    let _g = lock();
    let main = refusing("claude", SERVICE);
    let alt = refusing("claude-alt", SERVICE);
    let mut app = app_with(&[("main", &main, ""), ("alt", &alt, "")], "", "alt", false);
    remove(&mut app);

    assert_eq!(alt.all_argv(), [["rm", ID]]);
    assert_eq!(main.all_argv(), [["rm", ID]], "tried once, no flags");
    let all = popup_text(&app);
    assert!(all.contains(SERVICE), "the owner's message: {all}");
    assert!(all.contains("exit code 1"), "{all}");
    assert!(
        all.contains(
            "alt's background service was not answering, so tower also tried main's command; \
             it did not remove orphaned-turn either."
        ),
        "{all}"
    );
    assert!(all.contains("never passes"), "{all}");
    assert!(status_text(&app).contains("orphaned-turn was not removed"));
}

#[test]
fn other_accounts_are_tried_in_config_order_until_one_removes() {
    let _g = lock();
    let main = refusing("claude", SERVICE);
    let alt = refusing("claude-alt", SERVICE);
    let spare = removing("claude-spare");
    let last = removing("claude-last");
    let mut app = app_with(
        &[
            ("main", &main, ""),
            ("alt", &alt, ""),
            ("spare", &spare, ""),
            ("last", &last, ""),
        ],
        "",
        "alt",
        false,
    );
    remove(&mut app);

    assert_eq!(alt.calls(), 1);
    assert_eq!(main.all_argv(), [["rm", ID]], "main first: config order");
    assert_eq!(spare.all_argv(), [["rm", ID]]);
    assert_eq!(last.calls(), 0, "the first success wins");
    assert_eq!(
        message(&app),
        "removed orphaned-turn (with spare's command; alt's background service was not answering)"
    );
}

#[test]
fn the_group_guard_allows_no_fallback_and_the_dialog_says_so() {
    let _g = lock();
    let main = removing("claude");
    let alt = refusing("claude-alt", SERVICE);
    let mut app = app_with(
        &[("main", &main, "work"), ("alt", &alt, "personal")],
        "",
        "alt",
        false,
    );
    let job = remove(&mut app);

    assert_eq!(job.fallback, Fallback::Guarded);
    assert_eq!(alt.calls(), 1);
    assert_eq!(main.calls(), 0, "another group's command is never run");
    let all = popup_text(&app);
    assert!(all.contains(SERVICE), "{all}");
    assert!(
        all.contains(&format!(
            "alt's background service was not answering. Tower tried no other account: the \
             account groups allow none for this session (allow_cross_group_handoff = false). \
             Retry the remove later, or run `rm {ID}` with another account's command yourself."
        )),
        "{all}"
    );
}

#[test]
fn cross_group_handoff_allowed_lets_another_group_retry() {
    let _g = lock();
    let main = removing("claude");
    let alt = refusing("claude-alt", SERVICE);
    let mut app = app_with(
        &[("main", &main, "work"), ("alt", &alt, "personal")],
        "allow_cross_group_handoff = true",
        "alt",
        false,
    );
    remove(&mut app);
    assert_eq!(main.all_argv(), [["rm", ID]]);
    assert!(status_text(&app).starts_with("removed orphaned-turn (with main's command;"));
}

#[test]
fn a_live_row_gets_no_fallback() {
    let _g = lock();
    let main = removing("claude");
    let alt = refusing("claude-alt", SERVICE);
    let mut app = app_with(&[("main", &main, ""), ("alt", &alt, "")], "", "alt", true);
    assert!(app.selected_session().unwrap().session.has_live_process());
    let job = remove(&mut app);

    assert_eq!(job.fallback, Fallback::Final);
    assert_eq!(alt.all_argv(), [["rm", ID]]);
    assert_eq!(main.calls(), 0);
    let all = popup_text(&app);
    assert!(all.contains(SERVICE), "{all}");
    assert!(!all.contains("tower also tried"), "today's window: {all}");
    assert!(!all.contains("Tower tried no other"), "{all}");
}

#[test]
fn a_worktree_refusal_gets_no_fallback() {
    let _g = lock();
    let main = removing("claude");
    let alt = refusing("claude-alt", WORKTREE);
    let mut app = app_with(&[("main", &main, ""), ("alt", &alt, "")], "", "alt", false);
    let job = remove(&mut app);

    assert!(matches!(job.fallback, Fallback::Others(_)), "eligible row");
    assert_eq!(alt.all_argv(), [["rm", ID]]);
    assert_eq!(main.calls(), 0, "only the service refusal falls back");
    let all = popup_text(&app);
    assert!(all.contains("--discard-unpushed 1a2b3c4@"), "{all}");
    assert!(!all.contains("background service"), "{all}");
}

#[test]
fn a_single_account_has_nobody_to_ask() {
    let _g = lock();
    let main = refusing("claude", SERVICE);
    let mut app = app_with(&[("main", &main, "")], "", "main", false);
    let job = remove(&mut app);
    assert_eq!(job.fallback, Fallback::NoOthers);
    assert_eq!(main.calls(), 1);
    assert!(popup_text(&app).contains(
        "main's background service was not answering and no other account is configured. \
             Retry the remove later."
    ));
}

#[test]
fn the_service_refusal_matches_either_stream_in_any_case() {
    let _g = lock();
    let shouted = SERVICE.to_uppercase();
    for body in [
        format!("cat <<'EOF'\n{SERVICE}\nEOF\nexit 1"),
        format!("cat >&2 <<'EOF'\n{shouted}\nEOF\nexit 1"),
    ] {
        let owner_fake = Fake::new("claude-alt", &body);
        let other = removing("claude");
        let owner = owner_fake.account_named("alt", Some("~/.claude-alt"));
        let main = other.account_named("main", None);
        let removal =
            remove_with_fallback(&CliRunner::new(), &owner, &Fallback::Others(vec![main]), ID);
        assert_eq!(removal.result.unwrap(), Some("main".to_owned()), "{body}");
        assert!(removal.tried.is_empty());
        assert_eq!(other.all_argv(), [["rm", ID]]);
    }
}

#[test]
fn other_owner_errors_never_fall_back() {
    let _g = lock();
    let other = removing("claude");
    let main = other.account_named("main", None);
    for body in [
        "echo \"No job matching '4c8e2a19'. Run 'claude agents' to list running sessions.\" >&2\nexit 1",
        "echo 'error: supervisor gone' >&2\nexit 3",
    ] {
        let owner_fake = Fake::new("claude-alt", body);
        let owner = owner_fake.account_named("alt", Some("~/.claude-alt"));
        let removal = remove_with_fallback(
            &CliRunner::new(),
            &owner,
            &Fallback::Others(vec![main.clone()]),
            ID,
        );
        let err = removal.result.unwrap_err();
        assert!(!err.service_not_answering(), "{err:?}");
        assert!(
            matches!(
                err,
                RemoveError::UnknownId { .. } | RemoveError::Refused { .. }
            ),
            "{err:?}"
        );
    }
    assert_eq!(other.calls(), 0);
}

#[test]
fn several_accounts_tried_are_all_named() {
    let _g = lock();
    let main = refusing("claude", SERVICE);
    let alt = refusing("claude-alt", SERVICE);
    let spare = refusing("claude-spare", "error: not logged in");
    let mut app = app_with(
        &[
            ("main", &main, ""),
            ("alt", &alt, ""),
            ("spare", &spare, ""),
        ],
        "",
        "alt",
        false,
    );
    remove(&mut app);
    assert_eq!(
        (alt.calls(), main.calls(), spare.calls()),
        (1, 1, 1),
        "each once"
    );
    let all = popup_text(&app);
    assert!(
        all.contains(
            "alt's background service was not answering, so tower also tried the commands of \
             main, spare; none of them removed orphaned-turn either."
        ),
        "{all}"
    );
    assert!(all.contains(SERVICE), "the owner's message: {all}");
    assert!(!all.contains("not logged in"), "{all}");
}
