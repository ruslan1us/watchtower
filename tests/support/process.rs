//! What tests ask about a process: from `/proc` on Linux, from `ps` elsewhere,
//! where there is no `/proc` (doc/0049). A test that only needs the answer works
//! on both, so the terminal tests still run on macOS.
#![allow(dead_code)]

use std::fs::File;

/// A shell command printing the process group of the shell running it.
#[cfg(target_os = "linux")]
pub const PRINT_OWN_PGID: &str = "cut -d' ' -f5 /proc/$$/stat";
/// A shell command printing the process group of the shell running it.
#[cfg(not(target_os = "linux"))]
pub const PRINT_OWN_PGID: &str = "ps -o pgid= -p $$";

/// Whether the process is there and not a zombie.
#[cfg(target_os = "linux")]
pub fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .and_then(|(_, rest)| rest.chars().next())
        })
        .is_some_and(|state| state != 'Z')
}

/// Every child of `parent`, with its state.
#[cfg(target_os = "linux")]
pub fn children_of(parent: i32) -> Vec<(i32, char)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, rest)) = stat.rsplit_once(") ") else {
            continue;
        };
        let mut fields = rest.split(' ');
        let state = fields.next().and_then(|s| s.chars().next()).unwrap_or('?');
        let ppid: i32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if ppid == parent {
            out.push((pid, state));
        }
    }
    out
}

/// A process's CPU time in clock ticks (user + system).
#[cfg(target_os = "linux")]
pub fn cpu_ticks(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

/// A process's resident memory in KiB.
#[cfg(target_os = "linux")]
pub fn rss_kib(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap()
}

/// The terminal a process has as its stdin.
#[cfg(target_os = "linux")]
pub fn terminal_of(pid: i32) -> Option<File> {
    File::open(format!("/proc/{pid}/fd/0")).ok()
}

/// What `ps -p <pid> -o <column>=` prints, trimmed; empty for no process.
#[cfg(not(target_os = "linux"))]
fn ps_column(pid: u32, column: &str) -> String {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", &format!("{column}=")])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Whether the process is there and not a zombie.
#[cfg(not(target_os = "linux"))]
pub fn alive(pid: i32) -> bool {
    let state = ps_column(pid as u32, "stat");
    !state.is_empty() && !state.starts_with('Z')
}

/// Every child of `parent`, with its state.
#[cfg(not(target_os = "linux"))]
pub fn children_of(parent: i32) -> Vec<(i32, char)> {
    // One `-o` per column: `pid=,ppid=` would make the rest a column's title.
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "stat="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let pid = columns.next()?.parse().ok()?;
            let ppid: i32 = columns.next()?.parse().ok()?;
            let state = columns.next()?.chars().next()?;
            (ppid == parent).then_some((pid, state))
        })
        .collect()
}

/// A process's CPU time (user + system) in hundredths of a second, the unit of
/// Linux's clock ticks. `ps` prints `[[dd-]hh:]mm:ss.cc`.
#[cfg(not(target_os = "linux"))]
pub fn cpu_ticks(pid: u32) -> u64 {
    let time = ps_column(pid, "time");
    let (whole, hundredths) = time.split_once('.').unwrap_or((&time, "0"));
    let seconds = whole
        .replace('-', ":")
        .split(':')
        .fold(0u64, |total, part| {
            total * 60 + part.parse::<u64>().unwrap()
        });
    seconds * 100 + hundredths.parse::<u64>().unwrap()
}

/// A process's resident memory in KiB.
#[cfg(not(target_os = "linux"))]
pub fn rss_kib(pid: u32) -> u64 {
    ps_column(pid, "rss").parse().unwrap()
}

/// The terminal a process has as its stdin.
#[cfg(not(target_os = "linux"))]
pub fn terminal_of(pid: i32) -> Option<File> {
    let tty = ps_column(pid as u32, "tty");
    File::open(format!("/dev/{tty}")).ok()
}
