//! The files a node keeps about its own copy of a job: the workspace
//! marker, the harness record, the session the job continues, and the
//! fork record that freezes the job.
//!
//! Invariant: none of these travel and none of them decide ownership; the
//! ledger does. The marker says what the workspace at the root holds, so a
//! restore never writes over work that is not in a published snapshot
//! (ADR 0004), and the harness record names a process by its pid and the
//! kernel's start time, so a supervisor never kills a pid that was reused
//! after the harness exited (ADR 0003). The record stands for the whole
//! process group the harness leads: it is kept while any process of the
//! group runs, and a stop is done only when the group is empty. Each file
//! is replaced whole by a durable rename and read back as `None` when it is
//! absent.

use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, NodeId, RecordHash, SnapshotId};
use fabric_core::session::SessionId;
use fabric_core::version::HarnessVersion;
use rustix::process::{Pid, Signal};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::durable;
use crate::stamp::TreeStamp;

/// Why a node-local file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file does not hold what this version writes.
    #[error("{path} does not parse: {reason}")]
    Malformed {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The harness group could not be signalled, or outlived `SIGKILL`.
    #[error(
        "the harness group of pid {pid} still runs ({reason}); nothing was captured. Stop it by hand, then run the command again"
    )]
    Survived {
        /// The group's leader.
        pid: NonZeroU32,
        /// What failed.
        reason: String,
    },
    /// The recorded pid leads a group and its start time could not be read.
    #[error(
        "pid {pid} leads a process group and its start time could not be read, so whether it is still the harness is not known and it was not signalled; nothing was captured. Run the command again"
    )]
    Unidentified {
        /// The recorded pid.
        pid: NonZeroU32,
    },
}

/// Whether a recorded harness still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The process the record names runs.
    Leader,
    /// It exited, and processes it started still run in its group.
    Group,
    /// Nothing of it runs.
    Gone,
    /// Its start time could not be read while a group with its pid runs,
    /// so that group may be its own or another process's after the pid
    /// was given out again; nothing signals it.
    Unknown,
}

/// How long a group gets to empty after `SIGKILL`.
const KILL_WAIT: Duration = Duration::from_secs(5);

/// What the workspace at a job's root holds on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerState {
    /// A harness may have changed it since the last snapshot.
    Live,
    /// A suspend stopped the harness and has not recorded `Suspended` yet; `head` was the ledger head then.
    Suspending {
        /// The ledger head when the suspend began.
        head: RecordHash,
        /// The tree as the capture found it; `None` when it could not be measured.
        tree: Option<TreeStamp>,
    },
    /// It held exactly this snapshot when the tree was `tree`.
    ///
    /// A user can edit the tree by hand afterwards, so the snapshot is the
    /// workspace only while the tree still measures `tree`; `None`, when it
    /// could not be measured, never does.
    Clean {
        /// The snapshot.
        snapshot: SnapshotId,
        /// The tree when the snapshot was captured or restored.
        tree: Option<TreeStamp>,
    },
    /// A restore of this snapshot began and has not finished.
    Restoring {
        /// The snapshot.
        snapshot: SnapshotId,
    },
}

/// The workspace marker: where the root is, what it holds, and the tenure
/// that wrote it (ADR 0004).
///
/// A node can lose a job while its marker says `Live`, keep that workspace
/// as an orphan, and own the job again later at a higher epoch. The epoch
/// tells that orphan apart from a workspace of the current tenure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// The workspace root on this node.
    pub root: PathBuf,
    /// What it holds.
    pub state: MarkerState,
    /// The epoch at which this node held the job when it wrote the marker.
    pub epoch: Epoch,
}

impl Marker {
    /// Whether the workspace belongs to this node's tenure at `epoch`.
    ///
    /// It does when the marker was written at that epoch, when it was
    /// restoring exactly `latest`, or when it holds `latest` and the tree is
    /// unchanged since, because any tenure starts from `latest`. Anything
    /// else was left by an earlier tenure and may hold changes the job does
    /// not have, or lack changes it does.
    pub async fn is_current(&self, epoch: Epoch, latest: Option<SnapshotId>) -> bool {
        if self.epoch == epoch {
            return true;
        }
        match self.state {
            MarkerState::Restoring { snapshot } => Some(snapshot) == latest,
            MarkerState::Clean { snapshot, .. } => {
                Some(snapshot) == latest && self.is_unchanged().await
            }
            MarkerState::Live | MarkerState::Suspending { .. } => false,
        }
    }

    /// Whether the workspace is clean and still measures what it did when
    /// the marker was written.
    pub async fn is_unchanged(&self) -> bool {
        match self.state {
            MarkerState::Clean {
                tree: Some(tree), ..
            } => TreeStamp::measure(&self.root).await == Some(tree),
            _ => false,
        }
    }
}

/// A harness this node launched for a job, as the supervisor records it (ADR 0003).
///
/// A process that leaves the group, with `setsid` for one, is out of reach:
/// the supervisor signals the group and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessRecord {
    /// The epoch the harness was launched under; a higher one kills it.
    pub epoch: Epoch,
    /// Its process id, which is also its process group id.
    pub pid: NonZeroU32,
    /// The kernel's start time of the process, as `ps` prints it, and on
    /// Linux its start in clock ticks since boot after it.
    pub started: String,
}

/// A peer's ledger for the job that does not continue this node's, at an
/// epoch at least as high as this node's.
///
/// Its presence freezes the job on this node: the fork rule of ADR 0003 is
/// not built, so the node fails closed and never runs the job again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRecord {
    /// The peer whose ledger forked from this one.
    pub peer: NodeId,
    /// The highest epoch in the peer's ledger.
    pub epoch: Epoch,
    /// The peer's head when the fork was found.
    pub head: RecordHash,
}

/// The harness session a job continues on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The harness that owns the session.
    pub harness: HarnessKind,
    /// The session id the harness minted.
    pub session: SessionId,
    /// For a Codex session, the newest Codex known to have written it: the
    /// version the capture this node restored, or took last, recorded,
    /// raised to the version each launch on the session started with. A
    /// Codex that resumes a thread leaves no version in its rollout, so
    /// this is what keeps an older Codex off a thread a newer one
    /// continued elsewhere (ADR 0006, Settings travel with the session).
    /// `None` for any other harness, whose guard reads the session's own
    /// files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_writer: Option<HarnessVersion>,
}

/// A session a registration recorded before its `Resumed`, until the
/// session file holds it.
///
/// It is the job's session only if the record right after `after` is that
/// `Resumed`: a crash between the two writes then leaves it to be moved
/// into place, and a crash before the `Resumed` leaves one to drop, since
/// the CLI killed the harness that named it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSession {
    /// The ledger head the registration's `Resumed` follows.
    pub after: RecordHash,
    /// The epoch the harness was registered under.
    pub epoch: Epoch,
    /// The session.
    pub record: SessionRecord,
}

/// A snapshot this node keeps for a job although the job's ledger does not
/// name it, with every snapshot its bundle needs (ADR 0003, Orphans).
///
/// The store keeps every snapshot it holds until one is forgotten, and
/// refuses to forget a bundle base a later snapshot needs; a pin says
/// that no pass that forgets snapshots the ledger no longer names may
/// forget these. Nothing forgets snapshots yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// The snapshot.
    pub snapshot: SnapshotId,
    /// It and each bundle base it needs, in order, to its full bundle.
    pub chain: Vec<SnapshotId>,
    /// Why it is kept.
    pub why: PinWhy,
}

/// Why a snapshot is pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinWhy {
    /// `fabric job export` captured an orphan workspace at this root.
    Exported {
        /// The workspace.
        root: PathBuf,
    },
    /// A record of this node's chain published it, and a fork repair took
    /// another chain; the old ledger is kept at `kept`.
    ForkLost {
        /// The old ledger.
        kept: PathBuf,
    },
}

/// Reads a node-local file, or `None` when it does not exist.
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, LocalError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LocalError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| LocalError::Malformed {
            path: path.to_owned(),
            reason: e.to_string(),
        })
}

/// Replaces a node-local file with `value`.
pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), LocalError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| LocalError::Malformed {
        path: path.to_owned(),
        reason: e.to_string(),
    })?;
    durable::write_atomic(path, &bytes).map_err(|source| LocalError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Removes a node-local file; an absent one is already removed.
pub fn remove(path: &Path) -> Result<(), LocalError> {
    let io = |source| LocalError::Io {
        path: path.to_owned(),
        source,
    };
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(e)),
    }
    match path.parent() {
        Some(dir) => durable::sync_dir(dir).map_err(io),
        None => Ok(()),
    }
}

/// Removes a node-local file without flushing its directory, so a crash
/// can bring it back; an absent one is already removed.
///
/// Only for a file whose return after a crash is harmless because every
/// reader checks it again: a harness record, whose process each reader
/// asks for by pid and start time (`HarnessRecord::liveness`), so a record
/// that comes back names a process that is gone. It saves a full flush of
/// the directory at every harness's exit.
pub fn remove_unflushed(path: &Path) -> Result<(), LocalError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LocalError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The program that reads a process's start time.
const PS: &str = "ps";

/// The kernel's start time of `pid` as `ps -o lstart=` prints it, and on
/// Linux its start in clock ticks since boot after it, or `None` when no
/// such process runs or `ps` failed.
///
/// `lstart` has whole seconds, and a pid can be given out again within
/// one on a busy Linux host whose pid space is 32768; the ticks, a hundredth
/// of a second, tell the two processes apart. macOS gives out pids up to
/// 99998 and cannot start that many processes in a second.
pub async fn process_started(pid: NonZeroU32) -> Option<String> {
    match probe_started(PS, pid).await {
        Started::At(text) => Some(text),
        Started::Absent | Started::Unknown => None,
    }
}

/// What `ps` says about a pid's start time.
enum Started {
    /// A process with the pid runs, and started then.
    At(String),
    /// `ps` ran and found no process with the pid: it exits 1 and prints nothing.
    Absent,
    /// `ps` could not be run, or failed in another way.
    Unknown,
}

async fn probe_started(ps: &str, pid: NonZeroU32) -> Started {
    let Ok(output) = tokio::process::Command::new(ps)
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
        .await
    else {
        return Started::Unknown;
    };
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && !text.is_empty() {
        match kernel_start(pid) {
            KernelStart::NotKept => Started::At(text),
            KernelStart::Ticks(ticks) => Started::At(format!("{text} {ticks}")),
            // The process ended, or its pid passed to another, after `ps`
            // read it; which one this is cannot be told.
            KernelStart::Unread => Started::Unknown,
        }
    } else if output.status.code() == Some(1) && text.is_empty() && output.stderr.is_empty() {
        Started::Absent
    } else {
        Started::Unknown
    }
}

/// What the kernel says of a process's start, finer than `ps` does.
enum KernelStart {
    /// This platform keeps nothing finer that Fabric reads.
    #[cfg_attr(
        target_os = "linux",
        expect(dead_code, reason = "Linux keeps the start in ticks")
    )]
    NotKept,
    /// Clock ticks since boot.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "only Linux start ticks are read")
    )]
    Ticks(u64),
    /// The platform keeps it and it could not be read.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "only Linux start ticks are read")
    )]
    Unread,
}

/// The start of `pid` in clock ticks since boot, read from `/proc`.
#[cfg(target_os = "linux")]
fn kernel_start(pid: NonZeroU32) -> KernelStart {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .as_deref()
        .and_then(start_ticks)
        .map_or(KernelStart::Unread, KernelStart::Ticks)
}

#[cfg(not(target_os = "linux"))]
fn kernel_start(_pid: NonZeroU32) -> KernelStart {
    KernelStart::NotKept
}

/// Field 22 of a `/proc/<pid>/stat` line, the process's start in clock
/// ticks since boot. The command name in field 2 is in parentheses and may
/// hold spaces and parentheses itself, so fields are counted from the last
/// `)`.
#[cfg(any(target_os = "linux", test))]
fn start_ticks(stat: &str) -> Option<u64> {
    let (_, after_name) = stat.rsplit_once(')')?;
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// `pid` as a process id that may lead a group Fabric signals: any but 1.
fn leader_pid(pid: NonZeroU32) -> Option<Pid> {
    Pid::from_raw(i32::try_from(pid.get()).ok()?).filter(|pid| pid.as_raw_nonzero().get() > 1)
}

/// True when `pid` leads its own process group, as a launched harness does.
///
/// A harness that already exited and that the CLI, its parent, has not
/// reaped yet counts: a launch can end before the CLI registers it, as a
/// held harness stopped by a signal does, and its group may still hold
/// what it started. Linux's `getpgid` answers for such a process. macOS's
/// does not, and refuses it as absent, so there a process that `kill(pid,
/// 0)` still finds (macOS answers it for a zombie) leads its group when a
/// group with its id exists: a group takes its leader's pid as its id, and
/// that pid is not given out again while the group exists. `kill(-pid,
/// 0)` says the group exists by succeeding, or by `EPERM` when the group
/// holds only processes that cannot be signalled, a zombie among them.
/// Nothing is started to answer, as `ps -o pgid=` was.
pub fn leads_group(pid: NonZeroU32) -> bool {
    let Some(leader) = leader_pid(pid) else {
        return false;
    };
    match rustix::process::getpgid(Some(leader)) {
        Ok(group) => group == leader,
        Err(rustix::io::Errno::SRCH) => {
            rustix::process::test_kill_process(leader).is_ok() && group_id_taken(leader)
        }
        Err(_) => false,
    }
}

/// Whether a process group with id `group` exists, zombies counted:
/// `kill(-group, 0)` succeeds, or is refused with `EPERM`, which says the
/// group exists and holds nothing this process may signal. Liveness asks
/// a narrower question (`HarnessRecord::group_exists`): whether anything
/// of the group still runs.
fn group_id_taken(group: Pid) -> bool {
    matches!(
        rustix::process::test_kill_process_group(group),
        Ok(()) | Err(rustix::io::Errno::PERM)
    )
}

impl HarnessRecord {
    /// Whether the recorded process, or any process left in its group, runs.
    ///
    /// When the pid names another process now, the group is gone: a pid is
    /// not given out again while a process group with that id exists.
    pub async fn liveness(&self) -> Liveness {
        self.liveness_with(PS).await
    }

    async fn liveness_with(&self, ps: &str) -> Liveness {
        // No process holds the pid, not even an unreaped one, so `ps`
        // would find none either: the answer is its `Absent` branch,
        // without starting it, as at a harness's exit, which the CLI
        // reports after it reaped the harness.
        let started = if self.is_absent() {
            Started::Absent
        } else {
            probe_started(ps, self.pid).await
        };
        match started {
            Started::At(started) if started == self.started => Liveness::Leader,
            Started::Absent if self.group_exists() => Liveness::Group,
            // A failed probe cannot tell this group from one another
            // process leads under the same pid after it was given out again.
            Started::Unknown if self.group_exists() => Liveness::Unknown,
            Started::At(_) | Started::Absent | Started::Unknown => Liveness::Gone,
        }
    }

    /// True when no process has the recorded pid: `kill(pid, 0)` finds
    /// none. A zombie still has it, on Linux and on macOS alike.
    fn is_absent(&self) -> bool {
        self.group().is_some_and(|pid| {
            rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
        })
    }

    /// The group to signal; never group 1, because `kill(-1)` signals every process.
    fn group(&self) -> Option<Pid> {
        leader_pid(self.pid)
    }

    fn group_exists(&self) -> bool {
        self.group()
            .is_some_and(|group| rustix::process::test_kill_process_group(group).is_ok())
    }

    /// Stops the recorded process group: `SIGTERM`, then `SIGKILL` after
    /// `grace`, and returns once nothing of the group runs.
    ///
    /// A group that is gone, or a pid now owned by another process, is left
    /// alone. A group that cannot be signalled or outlives `SIGKILL` is an
    /// error, so no caller captures or hands away a workspace it still writes,
    /// and so is a group whose leader's start time cannot be read, which is
    /// never signalled.
    pub async fn stop(&self, grace: Duration) -> Result<(), LocalError> {
        self.stop_with(grace, PS).await
    }

    async fn stop_with(&self, grace: Duration, ps: &str) -> Result<(), LocalError> {
        match self.liveness_with(ps).await {
            Liveness::Gone => return Ok(()),
            Liveness::Unknown => return Err(LocalError::Unidentified { pid: self.pid }),
            Liveness::Leader | Liveness::Group => {}
        }
        let survived = |reason: String| LocalError::Survived {
            pid: self.pid,
            reason,
        };
        let group = self
            .group()
            .ok_or_else(|| survived("the pid is not a process id".to_owned()))?;
        self.signal(group, Signal::TERM).map_err(survived)?;
        if self.wait_gone(grace, ps).await {
            return Ok(());
        }
        tracing::warn!(pid = self.pid, "harness ignored SIGTERM; killing its group");
        self.signal(group, Signal::KILL).map_err(survived)?;
        if self.wait_gone(KILL_WAIT, ps).await {
            Ok(())
        } else {
            Err(survived("it outlived SIGKILL".to_owned()))
        }
    }

    fn signal(&self, group: Pid, signal: Signal) -> Result<(), String> {
        match rustix::process::kill_process_group(group, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(e) => Err(format!("signal {signal:?} to group {}: {e}", self.pid)),
        }
    }

    async fn wait_gone(&self, limit: Duration, ps: &str) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if self.liveness_with(ps).await == Liveness::Gone {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_of(child: &tokio::process::Child, started: String) -> HarnessRecord {
        HarnessRecord {
            epoch: Epoch::FIRST,
            pid: NonZeroU32::new(child.id().unwrap()).unwrap(),
            started,
        }
    }

    /// A command name may hold spaces and parentheses; the start is still
    /// the 22nd field of the line.
    #[test]
    fn the_start_ticks_are_read_past_any_command_name() {
        let stat =
            "4321 (a) b (c)) S 1 4321 4321 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 100";
        assert_eq!(start_ticks(stat), Some(987_654));
        assert_eq!(start_ticks("4321 (sleep) S 1"), None);
    }

    /// Two processes started in the same second have one `lstart`, and a
    /// pid given out again within that second must not make the second
    /// process pass for the first.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn two_processes_started_in_one_second_have_different_starts() {
        let spawn = || {
            tokio::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap()
        };
        let lstart = |pid: u32| {
            String::from_utf8(
                std::process::Command::new("ps")
                    .args(["-o", "lstart=", "-p", &pid.to_string()])
                    .env("LC_ALL", "C")
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
        };
        for _ in 0..20 {
            let mut first = spawn();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut second = spawn();
            let (a, b) = (first.id().unwrap(), second.id().unwrap());
            let same_second = lstart(a) == lstart(b);
            let starts = (
                process_started(NonZeroU32::new(a).unwrap()).await,
                process_started(NonZeroU32::new(b).unwrap()).await,
            );
            first.kill().await.unwrap();
            second.kill().await.unwrap();
            if same_second {
                assert_ne!(starts.0, starts.1);
                return;
            }
        }
        panic!("no two processes started in one second");
    }

    #[tokio::test]
    async fn a_reused_pid_is_not_the_recorded_harness() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = NonZeroU32::new(child.id().unwrap()).unwrap();
        let started = process_started(pid).await.unwrap();
        let record = record_of(&child, started.clone());
        assert_eq!(record.liveness().await, Liveness::Leader);
        let other = HarnessRecord {
            started: format!("{started} but earlier"),
            ..record.clone()
        };
        assert_eq!(
            other.liveness().await,
            Liveness::Gone,
            "a different start time is another process"
        );
        other.stop(Duration::from_millis(100)).await.unwrap();
        assert_eq!(
            record.liveness().await,
            Liveness::Leader,
            "stop left a process it does not own alone"
        );

        // The test is the parent here, so it reaps; a harness's parent is the CLI.
        let reaper = tokio::spawn(async move { child.wait().await });
        record.stop(Duration::from_secs(5)).await.unwrap();
        reaper.await.unwrap().unwrap();
        assert_eq!(record.liveness().await, Liveness::Gone);
    }

    /// A headless harness can finish its turn and exit before the CLI,
    /// which has not reaped it yet, registers it; it still leads its group.
    #[tokio::test]
    async fn an_exited_leader_its_parent_has_not_reaped_still_leads_its_group() {
        use std::os::unix::process::CommandExt;
        let mut leader = std::process::Command::new("true")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = NonZeroU32::new(leader.id()).unwrap();
        while process_started(pid).await.is_some()
            && !std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap()
                .stdout
                .starts_with(b"Z")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(leads_group(pid));
        leader.wait().unwrap();
        let mut member = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(
            !leads_group(NonZeroU32::new(member.id()).unwrap()),
            "a process in the test's group leads no group"
        );
        member.kill().unwrap();
        member.wait().unwrap();
    }

    /// A harness that started a background process and exited: the record
    /// still stands for the group, and a stop ends what is left of it.
    #[tokio::test]
    async fn a_stop_reaches_the_group_after_its_leader_exited() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child");
        let mut leader = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sleep 30 & echo $! > {}", pid_file.display()))
            .process_group(0)
            .spawn()
            .unwrap();
        let started = process_started(NonZeroU32::new(leader.id().unwrap()).unwrap())
            .await
            .unwrap();
        let record = record_of(&leader, started);
        leader.wait().await.unwrap();
        let child: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(record.liveness().await, Liveness::Group);
        record.stop(Duration::from_secs(5)).await.unwrap();
        assert_eq!(record.liveness().await, Liveness::Gone);
        let child = Pid::from_raw(child).unwrap();
        assert!(
            rustix::process::test_kill_process(child).is_err(),
            "the background process outlived the stop"
        );
    }

    /// A harness the CLI reaped holds no pid any more, so its liveness is
    /// read without `ps`: the record stands for the group while a process
    /// it started runs, even where `ps` cannot be run at all.
    #[tokio::test]
    async fn a_reaped_leader_is_read_without_ps() {
        let no_ps = "/nonexistent/fabric-test-ps";
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child");
        let mut leader = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sleep 30 & echo $! > {}", pid_file.display()))
            .process_group(0)
            .spawn()
            .unwrap();
        let started = process_started(NonZeroU32::new(leader.id().unwrap()).unwrap())
            .await
            .unwrap();
        let record = record_of(&leader, started);
        leader.wait().await.unwrap();
        assert_eq!(record.liveness_with(no_ps).await, Liveness::Group);
        record
            .stop_with(Duration::from_secs(5), no_ps)
            .await
            .unwrap();
        assert_eq!(record.liveness_with(no_ps).await, Liveness::Gone);
        let child: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            rustix::process::test_kill_process(Pid::from_raw(child).unwrap()).is_err(),
            "the background process outlived the stop"
        );
    }

    /// A start-time probe that fails says nothing about who holds the pid
    /// now. A stale record whose pid another process took, and leads a
    /// group with, must not have that group signalled on such a failure.
    #[tokio::test]
    async fn a_failed_start_time_probe_signals_nothing() {
        let no_ps = "/nonexistent/fabric-test-ps";
        let mut other = tokio::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let stale = record_of(&other, "the harness that had this pid".to_owned());
        assert_eq!(stale.liveness_with(no_ps).await, Liveness::Unknown);
        let refused = stale.stop_with(Duration::from_millis(100), no_ps).await;
        assert!(
            matches!(refused, Err(LocalError::Unidentified { .. })),
            "{refused:?}"
        );
        assert!(
            other.try_wait().unwrap().is_none(),
            "the other process's group was signalled"
        );
        other.kill().await.unwrap();
    }
}
