//! The files a node keeps about its own copy of a job: the workspace
//! marker, the harness record and the session the job continues.
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
use fabric_core::id::{Epoch, RecordHash, SnapshotId};
use rustix::process::{Pid, Signal};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::durable;

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
    },
    /// It holds exactly this snapshot.
    Clean {
        /// The snapshot.
        snapshot: SnapshotId,
    },
    /// A restore of this snapshot began and has not finished.
    Restoring {
        /// The snapshot.
        snapshot: SnapshotId,
    },
}

/// The workspace marker: where the root is and what it holds (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// The workspace root on this node.
    pub root: PathBuf,
    /// What it holds.
    pub state: MarkerState,
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
    /// The kernel's start time of the process, as `ps` prints it.
    pub started: String,
}

/// The harness session a job continues on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The harness that owns the session.
    pub harness: HarnessKind,
    /// The session id the harness minted.
    pub session: String,
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

/// The kernel's start time of `pid` as `ps -o lstart=` prints it, or `None` when no such process runs.
pub async fn process_started(pid: NonZeroU32) -> Option<String> {
    let output = tokio::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// `pid` as a process id that may lead a group Fabric signals: any but 1.
fn leader_pid(pid: NonZeroU32) -> Option<Pid> {
    Pid::from_raw(i32::try_from(pid.get()).ok()?).filter(|pid| pid.as_raw_nonzero().get() > 1)
}

/// True when `pid` runs and leads its own process group, as a launched harness does.
pub fn leads_group(pid: NonZeroU32) -> bool {
    leader_pid(pid).is_some_and(|pid| rustix::process::getpgid(Some(pid)) == Ok(pid))
}

impl HarnessRecord {
    /// Whether the recorded process, or any process left in its group, runs.
    ///
    /// When the pid names another process now, the group is gone: a pid is
    /// not given out again while a process group with that id exists.
    pub async fn liveness(&self) -> Liveness {
        match process_started(self.pid).await {
            Some(started) if started == self.started => Liveness::Leader,
            None if self.group_exists() => Liveness::Group,
            Some(_) | None => Liveness::Gone,
        }
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
    /// error, so no caller captures or hands away a workspace it still writes.
    pub async fn stop(&self, grace: Duration) -> Result<(), LocalError> {
        if self.liveness().await == Liveness::Gone {
            return Ok(());
        }
        let survived = |reason: String| LocalError::Survived {
            pid: self.pid,
            reason,
        };
        let group = self
            .group()
            .ok_or_else(|| survived("the pid is not a process id".to_owned()))?;
        self.signal(group, Signal::TERM).map_err(survived)?;
        if self.wait_gone(grace).await {
            return Ok(());
        }
        tracing::warn!(pid = self.pid, "harness ignored SIGTERM; killing its group");
        self.signal(group, Signal::KILL).map_err(survived)?;
        if self.wait_gone(KILL_WAIT).await {
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

    async fn wait_gone(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if self.liveness().await == Liveness::Gone {
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
}
