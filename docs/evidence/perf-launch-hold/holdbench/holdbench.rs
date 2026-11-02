// The hold micro-bench of docs/evidence/perf-launch-hold/README.md. Not a
// workspace member: build it as its own crate whose Cargo.toml holds
//
//   [package]
//   name = "holdbench"
//   version = "0.1.0"
//   edition = "2024"
//
//   [dependencies]
//   rustix = { version = "=1.1.5", features = ["process"] }
//
// then `cargo build --release` and run `holdbench 200` (line hold) or
// `holdbench 100 terminal` (stop hold behind a 30 ms registration). On
// macOS the M2 head's hold can hang inside `kill` (README, kernel race);
// a second `kill -CONT` frees it, and the pair keeps the time it waited.
//! ABAB micro-bench of the two holds: time from spawn to the harness's exit,
//! with the release sent the moment the hold allows it.
//! old: /bin/sh trampoline that stops itself; the parent polls waitid every
//!      2 ms (as spawn did) and sends SIGCONT once it is stopped.
//! new: /bin/dash trampoline that reads a line; the parent writes it at once.
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::os::unix::process::CommandExt;
use rustix::process::{Pid, WaitId, WaitIdOptions, waitid, kill_process_group, Signal};

fn old() -> Duration {
    let t = Instant::now();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "unset -f kill && kill -STOP $$ && exec \"$@\"", "fabric-hold", "/usr/bin/true"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0).spawn().unwrap();
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    loop {
        match waitid(WaitId::Pid(pid), WaitIdOptions::STOPPED | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG).unwrap() {
            Some(s) if s.stopped() => break,
            Some(_) => panic!("ended"),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    kill_process_group(pid, Signal::CONT).unwrap();
    child.wait().unwrap();
    t.elapsed()
}

fn new() -> Duration {
    let t = Instant::now();
    let (r, mut w) = std::io::pipe().unwrap();
    let mut cmd = Command::new("/bin/dash");
    cmd.args(["-c", "unset -f read && read -r go && exec \"$@\" </dev/null", "fabric-hold", "/usr/bin/true"])
        .stdin(Stdio::from(r)).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn().unwrap();
    drop(cmd);
    w.write_all(b"\n").unwrap();
    drop(w);
    child.wait().unwrap();
    t.elapsed()
}

/// The terminal path as the M2 head ran it: spawn waits for the stop by
/// polling, then the registration (REG), then the SIGCONT.
fn old_terminal() -> Duration {
    let t = Instant::now();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "unset -f kill && kill -STOP $$ && exec \"$@\"", "fabric-hold", "/usr/bin/true"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0).spawn().unwrap();
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    loop {
        match waitid(WaitId::Pid(pid), WaitIdOptions::STOPPED | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG).unwrap() {
            Some(s) if s.stopped() => break,
            Some(_) => panic!("ended"),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    std::thread::sleep(REG);
    kill_process_group(pid, Signal::CONT).unwrap();
    child.wait().unwrap();
    t.elapsed()
}

/// The terminal path now: the registration runs from the spawn, beside a
/// watcher that blocks in waitid and lets the stop settle 10 ms; the
/// SIGCONT goes once both are done.
fn new_terminal() -> Duration {
    let t = Instant::now();
    let mut child = Command::new("/bin/dash")
        .args(["-c", "unset -f kill && kill -STOP $$ && exec \"$@\"", "fabric-hold", "/usr/bin/true"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0).spawn().unwrap();
    let pid = Pid::from_raw(child.id() as i32).unwrap();
    let raw = child.id() as i32;
    let watcher = std::thread::Builder::new().stack_size(128 * 1024).spawn(move || {
        let pid = Pid::from_raw(raw).unwrap();
        loop {
            match waitid(WaitId::Pid(pid), WaitIdOptions::STOPPED | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT) {
                Ok(Some(s)) if s.stopped() => break,
                Ok(Some(_)) => panic!("ended"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }).unwrap();
    std::thread::sleep(REG);
    watcher.join().unwrap();
    kill_process_group(pid, Signal::CONT).unwrap();
    child.wait().unwrap();
    t.elapsed()
}

/// The registration a terminal launch waits for before its release: the
/// whole answer, about 30 ms on the bench Mac (ADR 0006).
const REG: Duration = Duration::from_millis(30);

fn main() {
    if std::env::args().nth(2).as_deref() == Some("terminal") {
        return run(old_terminal, new_terminal);
    }
    run(old, new)
}

fn run(old: fn() -> Duration, new: fn() -> Duration) {
    let pairs: usize = std::env::args().nth(1).map_or(200, |n| n.parse().unwrap());
    for _ in 0..10 { old(); new(); }
    let mut ratios = Vec::new();
    let (mut olds, mut news) = (Vec::new(), Vec::new());
    for i in 0..pairs {
        let (a, b) = if i % 2 == 0 { let a = old(); let b = new(); (a, b) } else { let b = new(); let a = old(); (a, b) };
        olds.push(a.as_secs_f64() * 1e3); news.push(b.as_secs_f64() * 1e3);
        ratios.push(b.as_secs_f64() / a.as_secs_f64());
        println!("pair\t{i}\t{:.3}\t{:.3}", a.as_secs_f64() * 1e3, b.as_secs_f64() * 1e3);
    }
    let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
    let mr = med(&mut ratios.clone());
    // percentile bootstrap of the median ratio, fixed seed
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut boots = Vec::new();
    for _ in 0..10_000 {
        let mut s: Vec<f64> = (0..ratios.len()).map(|_| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; ratios[(seed % ratios.len() as u64) as usize] }).collect();
        boots.push(med(&mut s));
    }
    boots.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("old median {:.3} ms, new median {:.3} ms, ratio {:.3} [{:.3}-{:.3}] over {} pairs",
        med(&mut olds), med(&mut news), mr, boots[250], boots[9750], pairs);
}
