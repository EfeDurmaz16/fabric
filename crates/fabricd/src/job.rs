//! One job as this node holds it: the ledger in memory, its file, and the
//! node-local files beside it.
//!
//! Invariant: the in-memory ledger is never ahead of the file. Every record
//! goes through `Ledger::commit`, which writes and flushes before it shows
//! the record, and the derived manifest is rewritten after each append. A
//! job whose files fail to open is reported, never repaired or dropped.

use std::fs;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::ledger::{Author, EventKind, Ledger, LedgerError, Record};
use fabric_core::moves::LeaseView;

use crate::clock;
use crate::error::NodeError;
use crate::ledger_file::{self, LedgerFile};
use crate::local::{self, ForkRecord, HarnessRecord, Marker, MarkerState, SessionRecord};
use crate::paths::JobDir;

/// A job's ledger and files on this node.
#[derive(Debug)]
pub struct Job {
    id: JobId,
    dir: JobDir,
    ledger: Ledger,
    file: LedgerFile,
}

impl Job {
    /// Creates job `id` from its first record, which must be its `Created`.
    ///
    /// The record is checked before anything is written, and the ledger
    /// appears whole or not at all, so a refused or failed create leaves
    /// nothing that would stop the job being learned later.
    pub fn create(id: JobId, dir: JobDir, created: &Record) -> Result<Self, NodeError> {
        let EventKind::Created { job_id, .. } = created.event().kind else {
            return Err(LedgerError::FirstNotCreated(created.event().kind.name()).into());
        };
        if job_id != id {
            return Err(LedgerError::WrongJob {
                found: job_id,
                expected: id,
            }
            .into());
        }
        let ledger = Ledger::replay(job_id, [created.clone()])?;
        crate::durable::create_dir(dir.dir()).map_err(|e| NodeError::io(dir.dir(), e))?;
        let file = LedgerFile::create(&dir.ledger(), created)?;
        ledger_file::write_manifest(&dir.manifest(), &ledger)?;
        Ok(Self {
            id: job_id,
            dir,
            ledger,
            file,
        })
    }

    /// Opens a job this node held before.
    pub fn open(id: JobId, dir: JobDir) -> Result<Self, NodeError> {
        let file = LedgerFile::open(&dir.ledger())?;
        let ledger = Ledger::open(id, &file)?;
        ledger_file::check_manifest(&dir.manifest(), &ledger)?;
        ledger_file::write_manifest(&dir.manifest(), &ledger)?;
        Ok(Self {
            id,
            dir,
            ledger,
            file,
        })
    }

    /// The job's id.
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The job's node-local directory.
    #[must_use]
    pub fn dir(&self) -> &JobDir {
        &self.dir
    }

    /// The ledger.
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Stores `record` durably, then shows it, then rewrites the derived manifest.
    pub fn commit(&mut self, record: Record, author: Author) -> Result<(), NodeError> {
        self.ledger.commit(&mut self.file, record, author)?;
        ledger_file::write_manifest(&self.dir.manifest(), &self.ledger)?;
        Ok(())
    }

    /// Proposes a record `me` writes now and commits it.
    pub fn append(&mut self, me: NodeId, kind: EventKind) -> Result<Record, NodeError> {
        let record = self
            .ledger
            .propose(me, kind, clock::event_id(), clock::now_ms())?;
        self.commit(record.clone(), Author::Node(me))?;
        Ok(record)
    }

    /// Commits a record a peer sent, unless this ledger already holds it.
    ///
    /// The record is checked first as the peer's own, then as one the peer
    /// relays from its writer. A record that names this node as its writer
    /// and that this node does not hold is refused: this node writes its own
    /// records and never learns them from a peer. Returns whether the
    /// ledger grew.
    pub fn accept(&mut self, bytes: &[u8], peer: NodeId, me: NodeId) -> Result<bool, NodeError> {
        let hash = fabric_core::id::RecordHash::of(bytes);
        if self.ledger.records().iter().any(|r| r.hash() == hash) {
            return Ok(false);
        }
        match self.ledger.verify(bytes, Author::Node(peer)) {
            Ok(record) => self.commit(record, Author::Node(peer))?,
            Err(LedgerError::NotAuthor { expected }) if expected != me => {
                let record = self.ledger.verify(bytes, Author::Relayed)?;
                self.commit(record, Author::Relayed)?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(true)
    }

    /// How this node relates to the lease.
    #[must_use]
    pub fn view(&self, me: NodeId) -> LeaseView {
        self.ledger.view(me)
    }

    /// The latest published snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<SnapshotId> {
        self.ledger.manifest().latest_snapshot
    }

    /// The harness the job currently runs under.
    #[must_use]
    pub fn harness(&self) -> &HarnessKind {
        &self.ledger.manifest().harness
    }

    /// The snapshot the owner suspended in, when no `Resumed` followed it.
    #[must_use]
    pub fn suspended_in(&self) -> Option<SnapshotId> {
        self.ledger
            .records()
            .iter()
            .rev()
            .find_map(|r| match r.event().kind {
                EventKind::Suspended { snapshot } => Some(Some(snapshot)),
                EventKind::Resumed { .. } => Some(None),
                _ => None,
            })
            .flatten()
            .filter(|s| Some(*s) == self.latest())
    }

    /// The workspace marker, if this node has a workspace for the job.
    pub fn marker(&self) -> Result<Option<Marker>, NodeError> {
        Ok(local::read(&self.dir.marker())?)
    }

    /// Replaces the workspace marker.
    pub fn set_marker(&self, marker: &Marker) -> Result<(), NodeError> {
        Ok(local::write(&self.dir.marker(), marker)?)
    }

    /// Removes the workspace marker.
    pub fn clear_marker(&self) -> Result<(), NodeError> {
        Ok(local::remove(&self.dir.marker())?)
    }

    /// The harness this node launched, if one is recorded.
    pub fn harness_record(&self) -> Result<Option<HarnessRecord>, NodeError> {
        Ok(local::read(&self.dir.harness())?)
    }

    /// The session the job continues on this node, if known.
    pub fn session(&self) -> Result<Option<SessionRecord>, NodeError> {
        Ok(local::read(&self.dir.session())?)
    }

    /// The fork record that freezes the job here, if a peer's ledger forked from this one.
    pub fn forked(&self) -> Result<Option<ForkRecord>, NodeError> {
        Ok(local::read(&self.dir.forked())?)
    }

    /// Freezes the job here; a job already frozen keeps the first record.
    pub fn freeze(&self, fork: &ForkRecord) -> Result<bool, NodeError> {
        if self.forked()?.is_some() {
            return Ok(false);
        }
        local::write(&self.dir.forked(), fork)?;
        Ok(true)
    }

    /// Settles a marker a crash left behind once the ledger says how the
    /// step ended: a suspend whose `Suspended` is durable is clean.
    pub fn reconcile_marker(&self) -> Result<Option<Marker>, NodeError> {
        let Some(marker) = self.marker()? else {
            return Ok(None);
        };
        if let (MarkerState::Suspending { .. }, Some(snapshot)) =
            (&marker.state, self.suspended_in())
        {
            let clean = Marker {
                root: marker.root,
                state: MarkerState::Clean { snapshot },
                epoch: marker.epoch,
            };
            self.set_marker(&clean)?;
            return Ok(Some(clean));
        }
        Ok(Some(marker))
    }

    /// Removes the harness state staging directory, so a capture or a restore starts empty.
    pub fn clear_harness_state(&self) -> Result<(), NodeError> {
        let dir = self.dir.harness_state();
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(NodeError::io(dir, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::harness::HarnessKind;
    use fabric_core::id::EventId;

    use super::*;
    use crate::paths::StateDir;

    #[test]
    fn a_created_record_for_another_job_leaves_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let (asked, other) = (JobId::from_bytes([1; 16]), JobId::from_bytes([2; 16]));
        let created = Ledger::created(
            other,
            "goal".into(),
            HarnessKind::Codex,
            NodeId::from_bytes([3; 32]),
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let refused = Job::create(asked, state.job(asked), &created);
        assert!(
            matches!(
                refused,
                Err(NodeError::Ledger(LedgerError::WrongJob { .. }))
            ),
            "{refused:?}"
        );
        assert!(!state.job(asked).dir().exists());
        Job::create(other, state.job(other), &created).unwrap();
        Job::open(other, state.job(other)).unwrap();
    }
}
