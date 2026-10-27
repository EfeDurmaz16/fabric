//! One job as this node holds it: the ledger in memory, its file, and the
//! node-local files beside it.
//!
//! Invariant: the in-memory ledger is never ahead of the file. Every record
//! goes through `Ledger::commit`, which writes and flushes before it shows
//! the record, and the derived manifest is rewritten after each append. A
//! job whose files fail to open is reported, never repaired or dropped.
//!
//! The workspace marker, the harness record and the fork record are kept in
//! memory once read, and every write and removal of them goes through this
//! type, which writes the file first and the copy after, so the copy is
//! never ahead of the file and never stale while the daemon, their one
//! writer, runs. A write that fails forgets the copy, so the next read asks
//! the file again; a read that fails keeps nothing.

use std::fs;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;

use fabric_core::harness::HarnessKind;
use fabric_core::id::{Epoch, JobId, NodeId, RecordHash, SnapshotId};
use fabric_core::ledger::{Author, EventKind, Ledger, LedgerError, Record};
use fabric_core::moves::LeaseView;

use crate::clock;
use crate::error::NodeError;
use crate::ledger_file::{self, LedgerFile};
use crate::local::{
    self, ForkRecord, HarnessRecord, Marker, MarkerState, PendingSession, SessionRecord,
};
use crate::paths::JobDir;

/// A job's ledger and files on this node.
#[derive(Debug)]
pub struct Job {
    id: JobId,
    dir: JobDir,
    ledger: Ledger,
    file: LedgerFile,
    marker: Kept<Marker>,
    harness: Kept<HarnessRecord>,
    forked: Kept<ForkRecord>,
}

/// A node-local file kept in memory after its first read.
#[derive(Debug)]
struct Kept<T>(Mutex<Slot<T>>);

/// What `Kept` holds of its file.
#[derive(Debug)]
enum Slot<T> {
    /// Not read yet, or a write failed and the file must be read again.
    Unread,
    /// What the file holds, `None` when it is absent.
    Read(Option<T>),
}

impl<T> Default for Kept<T> {
    fn default() -> Self {
        Self(Mutex::new(Slot::Unread))
    }
}

impl<T: Clone + Serialize + DeserializeOwned> Kept<T> {
    fn slot(&self) -> std::sync::MutexGuard<'_, Slot<T>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the file at `path` holds, read once.
    fn read(&self, path: &Path) -> Result<Option<T>, local::LocalError> {
        let mut slot = self.slot();
        if let Slot::Read(kept) = &*slot {
            return Ok(kept.clone());
        }
        let read = local::read(path)?;
        *slot = Slot::Read(read.clone());
        Ok(read)
    }

    /// Whether the file at `path` holds a value, read once.
    fn is_some(&self, path: &Path) -> Result<bool, local::LocalError> {
        let mut slot = self.slot();
        if let Slot::Read(kept) = &*slot {
            return Ok(kept.is_some());
        }
        let read: Option<T> = local::read(path)?;
        let present = read.is_some();
        *slot = Slot::Read(read);
        Ok(present)
    }

    /// Replaces the file at `path` with `value`, then the copy.
    fn write(&self, path: &Path, value: &T) -> Result<(), local::LocalError> {
        let mut slot = self.slot();
        *slot = Slot::Unread;
        local::write(path, value)?;
        *slot = Slot::Read(Some(value.clone()));
        Ok(())
    }

    /// Removes the file at `path`, flushing its directory when `flushed`, then the copy.
    fn remove(&self, path: &Path, flushed: bool) -> Result<(), local::LocalError> {
        let mut slot = self.slot();
        *slot = Slot::Unread;
        if flushed {
            local::remove(path)?;
        } else {
            local::remove_unflushed(path)?;
        }
        *slot = Slot::Read(None);
        Ok(())
    }
}

impl Job {
    /// Creates job `id` from its first record, which must be its `Created`.
    ///
    /// The record is checked before anything is written, and the ledger
    /// appears whole or not at all, so a refused or failed create leaves
    /// nothing that would stop the job being learned later. That holds when
    /// the derived manifest fails to write after the ledger is durable, as
    /// on a full disk: the job is not held in memory then, and a ledger
    /// left on disk would make every later offer of the record fail on it
    /// until the node restarts.
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
        if let Err(e) = ledger_file::write_manifest(&dir.manifest(), &ledger) {
            drop(file);
            if let Err(removed) = local::remove(&dir.ledger()) {
                tracing::warn!(error = %removed, "a failed job create left its ledger");
            }
            return Err(e.into());
        }
        Ok(Self {
            id: job_id,
            dir,
            ledger,
            file,
            marker: Kept::default(),
            harness: Kept::default(),
            forked: Kept::default(),
        })
    }

    /// Creates a job this node makes and owns, with its workspace at `marker`.
    ///
    /// The marker is written before the ledger: a crash between the two
    /// leaves a marker with no ledger, which start-up skips as no job, and
    /// never an owned ledger with no marker, which no command could run,
    /// suspend or restore. `id` is fresh, so a failure removes the job's
    /// directory with whatever it wrote: a ledger left there after the
    /// derived manifest failed would load at the next start as a job whose
    /// workspace the caller has deleted and whose id it never printed.
    pub fn create_owned(
        id: JobId,
        dir: JobDir,
        created: &Record,
        marker: &Marker,
    ) -> Result<Self, NodeError> {
        let path = dir.dir().to_owned();
        let made = crate::durable::create_dir(&path)
            .map_err(|e| NodeError::io(&path, e))
            .and_then(|()| Ok(local::write(&dir.marker(), marker)?))
            .and_then(|()| Self::create(id, dir, created));
        if made.is_err() {
            match fs::remove_dir_all(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "a failed job create left its directory");
                }
            }
        }
        made
    }

    /// Opens a job this node held before.
    ///
    /// The derived manifest is rewritten only when it is missing or behind
    /// the ledger, the crash window between an append and its manifest
    /// write. One that holds exactly the bytes the ledger derives is
    /// current: it remembers nothing the ledger lacks, so the check that
    /// a manifest is not ahead passes, and a rewrite would change nothing
    /// but cost two full flushes per job at every start.
    pub fn open(id: JobId, dir: JobDir) -> Result<Self, NodeError> {
        let file = LedgerFile::open(&dir.ledger())?;
        let ledger = Ledger::open(id, &file)?;
        let path = dir.manifest();
        if !manifest_is_current(&path, &ledger) {
            ledger_file::check_manifest(&path, &ledger)?;
            ledger_file::write_manifest(&path, &ledger)?;
        }
        Ok(Self {
            id,
            dir,
            ledger,
            file,
            marker: Kept::default(),
            harness: Kept::default(),
            forked: Kept::default(),
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

    /// The snapshot a suspend begun at `head` in the tenure at `epoch`
    /// ended in: the last `Suspended` after `head` with no `Resumed` after
    /// it, when every record after `head` is of that epoch.
    fn suspended_after(&self, head: RecordHash, epoch: Epoch) -> Option<SnapshotId> {
        let mut suspended = None;
        for record in self.ledger.suffix_after(head)? {
            if record.event().epoch != epoch {
                return None;
            }
            match record.event().kind {
                EventKind::Suspended { snapshot } => suspended = Some(snapshot),
                EventKind::Resumed { .. } => suspended = None,
                _ => {}
            }
        }
        suspended
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
        Ok(self.marker.read(&self.dir.marker())?)
    }

    /// Replaces the workspace marker.
    pub fn set_marker(&self, marker: &Marker) -> Result<(), NodeError> {
        Ok(self.marker.write(&self.dir.marker(), marker)?)
    }

    /// Removes the workspace marker.
    pub fn clear_marker(&self) -> Result<(), NodeError> {
        Ok(self.marker.remove(&self.dir.marker(), true)?)
    }

    /// The harness this node launched, if one is recorded.
    pub fn harness_record(&self) -> Result<Option<HarnessRecord>, NodeError> {
        Ok(self.harness.read(&self.dir.harness())?)
    }

    /// Records the harness this node launched.
    pub fn set_harness_record(&self, record: &HarnessRecord) -> Result<(), NodeError> {
        Ok(self.harness.write(&self.dir.harness(), record)?)
    }

    /// Removes the harness record of a harness that was stopped, durably,
    /// before what follows the stop acts on the workspace.
    pub fn clear_harness_record(&self) -> Result<(), NodeError> {
        Ok(self.harness.remove(&self.dir.harness(), true)?)
    }

    /// Removes the harness record of a harness whose group is gone, without
    /// flushing the directory: a record a crash brings back names a process
    /// every reader finds gone (`local::remove_unflushed`).
    pub fn forget_gone_harness(&self) -> Result<(), NodeError> {
        Ok(self.harness.remove(&self.dir.harness(), false)?)
    }

    /// The session the job continues on this node, if known.
    ///
    /// A registration records its session as pending before its `Resumed`
    /// and moves it into place after, so a crash between the two is settled
    /// here: the pending session becomes the job's when the record right
    /// after the head it names is that `Resumed`, and is dropped otherwise.
    pub fn session(&self) -> Result<Option<SessionRecord>, NodeError> {
        let pending_path = self.dir.session_pending();
        if let Some(pending) = local::read::<PendingSession>(&pending_path)? {
            let resumed = self
                .ledger
                .suffix_after(pending.after)
                .and_then(<[Record]>::first)
                .is_some_and(|next| {
                    next.event().epoch == pending.epoch
                        && matches!(
                            &next.event().kind,
                            EventKind::Resumed { harness, .. } if *harness == pending.record.harness
                        )
                });
            if resumed {
                local::write(&self.dir.session(), &pending.record)?;
            }
            local::remove(&pending_path)?;
        }
        Ok(local::read(&self.dir.session())?)
    }

    /// Records the session a registration at `epoch` starts, before its
    /// `Resumed` is appended after the current head; with none, drops one
    /// an earlier registration left, which that `Resumed` would otherwise
    /// seem to confirm.
    pub fn begin_session(
        &self,
        epoch: Epoch,
        record: Option<&SessionRecord>,
    ) -> Result<(), NodeError> {
        match record {
            Some(record) => local::write(
                &self.dir.session_pending(),
                &PendingSession {
                    after: self.ledger.head(),
                    epoch,
                    record: record.clone(),
                },
            )?,
            None => local::remove(&self.dir.session_pending())?,
        }
        Ok(())
    }

    /// Records the session a restore placed, over any a registration left
    /// pending and any fresh Codex launch planned here before.
    pub fn set_session(&self, record: &SessionRecord) -> Result<(), NodeError> {
        local::remove(&self.dir.session_pending())?;
        local::remove(&self.dir.fresh_codex())?;
        local::remove(&self.dir.fresh_codex_planned())?;
        Ok(local::write(&self.dir.session(), record)?)
    }

    /// Forgets the session, a pending one and any fresh Codex launch planned here included.
    pub fn forget_session(&self) -> Result<(), NodeError> {
        local::remove(&self.dir.session_pending())?;
        local::remove(&self.dir.fresh_codex())?;
        local::remove(&self.dir.fresh_codex_planned())?;
        Ok(local::remove(&self.dir.session())?)
    }

    /// The fork record that freezes the job here, if a peer's ledger forked from this one.
    pub fn forked(&self) -> Result<Option<ForkRecord>, NodeError> {
        Ok(self.forked.read(&self.dir.forked())?)
    }

    /// Whether the job is frozen here, without copying the fork record.
    pub fn is_frozen(&self) -> Result<bool, NodeError> {
        Ok(self.forked.is_some(&self.dir.forked())?)
    }

    /// Freezes the job here; a job already frozen keeps the first record.
    pub fn freeze(&self, fork: &ForkRecord) -> Result<bool, NodeError> {
        if self.is_frozen()? {
            return Ok(false);
        }
        self.forked.write(&self.dir.forked(), fork)?;
        Ok(true)
    }

    /// Settles a marker a crash left behind once the ledger says how the
    /// step ended: a suspend whose `Suspended` is durable is clean.
    ///
    /// Only a `Suspended` this node wrote after the head the marker
    /// recorded, in the same tenure, completes that suspend. One from
    /// before it began, or from another node after a steal or a move, says
    /// nothing about this tree.
    pub fn reconcile_marker(&self) -> Result<Option<Marker>, NodeError> {
        let Some(marker) = self.marker()? else {
            return Ok(None);
        };
        if let MarkerState::Suspending { head, tree } = &marker.state
            && let Some(snapshot) = self.suspended_after(*head, marker.epoch)
        {
            let clean = Marker {
                state: MarkerState::Clean {
                    snapshot,
                    tree: *tree,
                },
                ..marker
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

/// True when the manifest at `path` holds exactly the bytes `ledger`
/// derives; false when it is missing or differs.
fn manifest_is_current(path: &Path, ledger: &Ledger) -> bool {
    // A manifest that does not read is left to `check_manifest`, which
    // fails closed on it with the manifest's path, as it always did.
    let Ok(on_disk) = fs::read(path) else {
        return false;
    };
    // A ledger whose manifest does not encode is left to `write_manifest`,
    // which reports it.
    ledger
        .manifest()
        .encode()
        .is_ok_and(|derived| derived == on_disk)
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

    /// A start rewrites a job's derived manifest only when it is missing
    /// or behind the ledger: one that is current is left as it is, since
    /// its rewrite costs two full flushes per job at every start.
    #[test]
    fn an_open_rewrites_the_manifest_only_when_it_is_missing_or_behind() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let me = NodeId::from_bytes([3; 32]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            me,
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let mut job = Job::create(id, state.job(id), &created).unwrap();
        let path = state.job(id).manifest();
        let behind = fs::read(&path).unwrap();
        job.append(me, EventKind::Note { text: "one".into() })
            .unwrap();
        let current = fs::read(&path).unwrap();
        drop(job);
        let inode = |path: &std::path::Path| fs::metadata(path).unwrap().ino();

        let before = inode(&path);
        Job::open(id, state.job(id)).unwrap();
        assert_eq!(inode(&path), before, "a current manifest was rewritten");

        fs::write(&path, &behind).unwrap();
        Job::open(id, state.job(id)).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            current,
            "a manifest behind was kept"
        );

        fs::remove_file(&path).unwrap();
        Job::open(id, state.job(id)).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            current,
            "a missing manifest was not written"
        );
    }

    /// The marker, the harness record and the fork record are read from
    /// disk once and then answered from memory, since the daemon is their
    /// one writer; every write goes to the file before the copy, and a
    /// write that fails leaves the next read to the file.
    #[test]
    fn a_jobs_local_records_are_read_once_and_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let me = NodeId::from_bytes([3; 32]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            me,
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let marker = Marker {
            root: dir.path().join("work"),
            state: MarkerState::Live,
            epoch: Epoch::FIRST,
        };
        let job = Job::create_owned(id, state.job(id), &created, &marker).unwrap();
        let paths = state.job(id);
        assert_eq!(job.marker().unwrap(), Some(marker.clone()));
        assert_eq!(job.harness_record().unwrap(), None);
        assert!(!job.is_frozen().unwrap());
        // Read once: files changed behind the job's back are not read again.
        fs::remove_file(paths.marker()).unwrap();
        fs::write(paths.harness(), "not a record").unwrap();
        assert_eq!(job.marker().unwrap(), Some(marker.clone()));
        assert_eq!(job.harness_record().unwrap(), None);

        // Written through: the file holds what the copy holds.
        let record = HarnessRecord {
            epoch: Epoch::FIRST,
            pid: std::num::NonZeroU32::new(4242).unwrap(),
            started: "then".into(),
        };
        job.set_harness_record(&record).unwrap();
        assert_eq!(local::read(&paths.harness()).unwrap(), Some(record.clone()));
        assert_eq!(job.harness_record().unwrap(), Some(record));
        job.forget_gone_harness().unwrap();
        assert!(!paths.harness().exists());
        assert_eq!(job.harness_record().unwrap(), None);
        let fork = ForkRecord {
            peer: NodeId::from_bytes([7; 32]),
            epoch: Epoch::FIRST,
            head: job.ledger().head(),
        };
        assert!(job.freeze(&fork).unwrap());
        assert_eq!(local::read(&paths.forked()).unwrap(), Some(fork.clone()));
        assert!(
            !job.freeze(&fork).unwrap(),
            "a second freeze replaced the first"
        );

        // A write that fails forgets the copy, so the next read asks the file.
        fs::create_dir_all(paths.marker()).unwrap();
        assert!(job.set_marker(&marker).is_err());
        assert!(job.marker().is_err(), "the copy outlived a failed write");
    }

    /// An owned job whose marker could not be written must leave no
    /// ledger: start-up would load it as a job no command can run.
    #[test]
    fn an_owned_job_whose_marker_fails_leaves_no_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            NodeId::from_bytes([3; 32]),
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let marker = Marker {
            root: dir.path().join("work"),
            state: MarkerState::Live,
            epoch: fabric_core::id::Epoch::FIRST,
        };
        // A directory where the marker goes makes its write fail.
        fs::create_dir_all(state.job(id).marker()).unwrap();
        let failed = Job::create_owned(id, state.job(id), &created, &marker);
        assert!(failed.is_err(), "{failed:?}");
        assert!(!state.job(id).ledger().exists());
        let job = Job::create_owned(id, state.job(id), &created, &marker).unwrap();
        assert_eq!(job.marker().unwrap(), Some(marker));
    }

    /// A registration that died before its `Resumed` leaves a pending
    /// session whose harness the CLI killed, and which may never have been
    /// written. It must not become the job's session: every later capture
    /// would look for it and fail.
    #[test]
    fn a_pending_session_without_its_resumed_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            NodeId::from_bytes([3; 32]),
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let marker = Marker {
            root: dir.path().join("work"),
            state: MarkerState::Live,
            epoch: fabric_core::id::Epoch::FIRST,
        };
        let job = Job::create_owned(id, state.job(id), &created, &marker).unwrap();
        let record = SessionRecord {
            harness: HarnessKind::Codex,
            session: fabric_core::session::SessionId::parse("never-ran").unwrap(),
            known_writer: None,
        };
        job.begin_session(fabric_core::id::Epoch::FIRST, Some(&record))
            .unwrap();
        assert_eq!(job.session().unwrap(), None);
        assert!(!state.job(id).session_pending().exists());
    }

    /// A peer's `Created` can be durable here when the derived manifest
    /// beside it fails to write, as on a full disk. The job is not held
    /// then, so its ledger must go too: left on disk, it would make every
    /// later offer of the record fail until the node restarts.
    #[test]
    fn a_learned_job_whose_manifest_fails_can_be_learned_again() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            NodeId::from_bytes([3; 32]),
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        // A directory where the manifest goes makes its write fail after
        // the ledger is written.
        fs::create_dir_all(state.job(id).manifest().join("in-the-way")).unwrap();
        let failed = Job::create(id, state.job(id), &created);
        assert!(failed.is_err(), "{failed:?}");
        fs::remove_dir_all(state.job(id).manifest()).unwrap();
        Job::create(id, state.job(id), &created).unwrap();
        Job::open(id, state.job(id)).unwrap();
    }

    /// The ledger can be durable when the derived manifest beside it fails
    /// to write, as on a full disk. The job's id was never printed and its
    /// workspace is gone, so the create must not leave that ledger for the
    /// next start to load as an owned job nothing can run.
    #[test]
    fn an_owned_job_whose_manifest_fails_leaves_no_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let id = JobId::from_bytes([1; 16]);
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            NodeId::from_bytes([3; 32]),
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let marker = Marker {
            root: dir.path().join("work"),
            state: MarkerState::Live,
            epoch: fabric_core::id::Epoch::FIRST,
        };
        // A directory where the manifest goes makes its write fail after
        // the ledger is written.
        fs::create_dir_all(state.job(id).manifest().join("in-the-way")).unwrap();
        let failed = Job::create_owned(id, state.job(id), &created, &marker);
        assert!(failed.is_err(), "{failed:?}");
        assert!(
            !state.job(id).ledger().exists(),
            "the ledger stayed without its job"
        );
    }
}
