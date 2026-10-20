//! The files a node keeps about its own copy of a job: the workspace
//! marker, the harness record and the session the job continues.
//!
//! Invariant: none of these travel and none of them decide ownership; the
//! ledger does. The marker says what the workspace at the root holds, so a
//! restore never writes over work that is not in a published snapshot
//! (ADR 0004), and the harness record names a process by its pid and the
//! kernel's start time, so a supervisor never kills a pid that was reused
//! after the harness exited (ADR 0003). Each file is replaced whole by a
//! durable rename and read back as `None` when it is absent.

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
}

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

impl HarnessRecord {
    /// True when the recorded process still runs: the pid lives and started when it did.
    pub async fn is_alive(&self) -> bool {
        process_started(self.pid).await.as_deref() == Some(self.started.as_str())
    }

    /// Stops the recorded process group: `SIGTERM`, then `SIGKILL` after `grace`.
    ///
    /// A process that is gone, or a pid now owned by another process, is
    /// left alone.
    pub async fn stop(&self, grace: Duration) {
        let Some(pid) = Pid::from_raw(self.pid.get().try_into().unwrap_or(i32::MAX)) else {
            return;
        };
        if !self.is_alive().await {
            return;
        }
        let _ = rustix::process::kill_process_group(pid, Signal::TERM);
        let deadline = tokio::time::Instant::now() + grace;
        while tokio::time::Instant::now() < deadline {
            if !self.is_alive().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if self.is_alive().await {
            tracing::warn!(pid = self.pid, "harness ignored SIGTERM; killing its group");
            let _ = rustix::process::kill_process_group(pid, Signal::KILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_reused_pid_is_not_the_recorded_harness() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = NonZeroU32::new(child.id().unwrap()).unwrap();
        let started = process_started(pid).await.unwrap();
        let record = HarnessRecord {
            epoch: Epoch::FIRST,
            pid,
            started: started.clone(),
        };
        assert!(record.is_alive().await);
        let other = HarnessRecord {
            started: format!("{started} but earlier"),
            ..record.clone()
        };
        assert!(
            !other.is_alive().await,
            "a different start time is another process"
        );
        other.stop(Duration::from_millis(100)).await;
        assert!(
            record.is_alive().await,
            "stop left a process it does not own alone"
        );

        // The test is the parent here, so it reaps; a harness's parent is the CLI.
        let reaper = tokio::spawn(async move { child.wait().await });
        record.stop(Duration::from_secs(5)).await;
        reaper.await.unwrap().unwrap();
        assert!(!record.is_alive().await);
    }
}
