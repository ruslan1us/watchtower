//! The background poller with a fake listing source (doc/0001): immediate
//! first poll, every account polled, slow polls skipped instead of piling
//! up, and a forced poll re-run after a running one.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use watchtower::config::Account;
use watchtower::listing::Listing;
use watchtower::poller::{ListSource, Poller};

fn account(name: &str) -> Account {
    Account::new(name, "claude", None, Path::new("/home/user")).unwrap()
}

/// Counts calls per account; `slow` blocks until released.
#[derive(Default)]
struct FakeSource {
    calls: Mutex<Vec<String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    gate: Mutex<bool>,
    open: Condvar,
}

impl FakeSource {
    fn calls(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| *c == name)
            .count()
    }
    fn release(&self) {
        *self.gate.lock().unwrap() = true;
        self.open.notify_all();
    }
}

impl ListSource for FakeSource {
    fn list(&self, account: &Account) -> Result<Listing, String> {
        self.calls.lock().unwrap().push(account.name.clone());
        if account.name == "slow" {
            let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(n, Ordering::SeqCst);
            let mut open = self.gate.lock().unwrap();
            while !*open {
                open = self.open.wait(open).unwrap();
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return Err("slow account failed".into());
        }
        Ok(Listing::default())
    }
}

#[test]
fn polls_all_accounts_immediately_and_on_ticks() {
    let source = Arc::new(FakeSource::default());
    let poller = Poller::start(
        vec![account("main"), account("alt")],
        source.clone(),
        Duration::from_millis(50),
    );
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let u = poller.recv_timeout(Duration::from_secs(5)).expect("result");
        assert!(u.result.is_ok());
        // The immediate first poll is cycle 1 for every account.
        assert_eq!(u.cycle, 1, "{}", u.account);
        seen.push(u.account);
    }
    seen.sort();
    assert_eq!(seen, ["alt", "main"]);
    // Ticks keep polling, each tick a later cycle.
    let mut last = 1;
    for _ in 0..4 {
        let u = poller
            .recv_timeout(Duration::from_secs(5))
            .expect("tick result");
        assert!(u.cycle >= last, "cycles never go back");
        last = u.cycle;
    }
    assert!(last > 1);
    assert!(source.calls("main") >= 2);
}

#[test]
fn slow_account_is_skipped_not_piled_up() {
    let source = Arc::new(FakeSource::default());
    let poller = Poller::start(
        vec![account("slow"), account("main")],
        source.clone(),
        Duration::from_millis(20),
    );
    // main keeps reporting while slow is stuck.
    let mut main_cycle = 0;
    for _ in 0..5 {
        let u = poller.recv_timeout(Duration::from_secs(5)).expect("result");
        assert_eq!(u.account, "main");
        main_cycle = u.cycle;
    }
    assert!(main_cycle > 1);
    assert_eq!(source.calls("slow"), 1, "no new poll while one is running");
    assert_eq!(source.max_in_flight.load(Ordering::SeqCst), 1);

    // A forced poll while slow is running is remembered as one re-run.
    poller.poll_now();
    poller.poll_now();
    std::thread::sleep(Duration::from_millis(50));
    source.release();
    let mut slow_cycles = Vec::new();
    for _ in 0..50 {
        match poller.recv_timeout(Duration::from_millis(200)) {
            Some(u) if u.account == "slow" => {
                assert_eq!(u.result, Err("slow account failed".to_owned()));
                slow_cycles.push(u.cycle);
                if slow_cycles.len() == 2 {
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(slow_cycles.len() >= 2, "re-run after the forced poll");
    // The stuck poll reports the cycle it started in; the re-run reports
    // the forced cycle that asked for it, which is newer than main's
    // earlier ticks.
    assert_eq!(slow_cycles[0], 1);
    assert!(
        slow_cycles[1] > main_cycle,
        "{slow_cycles:?} vs {main_cycle}"
    );
    assert_eq!(source.max_in_flight.load(Ordering::SeqCst), 1);
}
