//! The job ledger (ADR 0002) and the lease transitions it drives (ADR 0003).
//!
//! Invariant: a `Ledger` holds a hash-chained sequence of records that starts
//! with `Created`, every record links to the one before it, every record
//! carries the epoch the rules require, and the lease derived from the
//! records is always `Owned` by one node or `Departing` from one node toward
//! one target. A record that would break any of this is refused before it is
//! stored, so a persisted ledger never needs repair to be replayed.

use std::convert::Infallible;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::codec::{self, CodecError, VersionedError};
use crate::harness::HarnessKind;
use crate::id::{EnvSpecId, Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use crate::lease::{FenceError, FencingToken, LeaseState};

/// The record format version this crate writes.
pub const RECORD_FORMAT_VERSION: u8 = 1;

/// The job manifest format version this crate writes; the first varint of every encoded manifest.
pub const JOB_MANIFEST_VERSION: u16 = 1;

/// Bytes of blake3 digest that end every record on disk.
pub const CHECKSUM_LEN: usize = 16;

/// The most bytes one record may occupy; a memory bound against a hostile
/// peer and the size of the largest control frame (ADR 0005).
pub const MAX_RECORD_LEN: usize = 1 << 20;

/// Bytes of blake3 digest over the length field that follow it.
pub const HEADER_CHECK_LEN: usize = 4;

const LENGTH_LEN: usize = 4;
const HEADER_LEN: usize = LENGTH_LEN + HEADER_CHECK_LEN;
const VERSION_LEN: usize = 1;
const MIN_BODY_LEN: usize = VERSION_LEN + CHECKSUM_LEN;

fn header_check(length: u32) -> [u8; HEADER_CHECK_LEN] {
    let digest = blake3::hash(&length.to_le_bytes());
    let mut out = [0; HEADER_CHECK_LEN];
    out.copy_from_slice(&digest.as_bytes()[..HEADER_CHECK_LEN]);
    out
}

/// One event in a job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The record's own id.
    pub id: EventId,
    /// The hash of the previous record's bytes; `None` only for `Created`.
    pub prev: Option<RecordHash>,
    /// The lease epoch the record was written under.
    pub epoch: Epoch,
    /// Unix milliseconds; informational, never used for ordering.
    pub at: u64,
    /// What happened.
    pub kind: EventKind,
}

/// What a ledger record says happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// The job was created; `created_by` owns it at epoch one.
    Created {
        /// The job this ledger belongs to; every replay checks it.
        job_id: JobId,
        /// The goal in the user's words.
        goal: String,
        /// The harness the job started under.
        harness: HarnessKind,
        /// The node that created the job.
        created_by: NodeId,
    },
    /// The owner committed to a move; the job is not runnable until the target decides.
    MoveStarted {
        /// The move's durable id.
        move_id: MoveId,
        /// The node that alone decides the move.
        target: NodeId,
    },
    /// The target imported the move and owns the job at the next epoch.
    MoveImported {
        /// The move decided.
        move_id: MoveId,
        /// The node the job came from.
        from: NodeId,
    },
    /// The target declined the move; the source owns the job again at the same epoch.
    MoveAborted {
        /// The move decided.
        move_id: MoveId,
    },
    /// `by` took the job from `from` without consent, at the next epoch.
    Stolen {
        /// The new owner.
        by: NodeId,
        /// The owner of record at the time.
        from: NodeId,
        /// The user's reason, or empty.
        reason: String,
    },
    /// A snapshot is complete in the owner's store.
    SnapshotPublished {
        /// The snapshot.
        snapshot: SnapshotId,
        /// The snapshot it follows, if any.
        parent: Option<SnapshotId>,
    },
    /// The harness stopped and the workspace is in `snapshot`.
    Suspended {
        /// The published snapshot the workspace matches.
        snapshot: SnapshotId,
    },
    /// A harness was launched on `node`.
    Resumed {
        /// The node that launched it; always the owner.
        node: NodeId,
        /// The snapshot restored first; `None` only before the first snapshot exists.
        snapshot: Option<SnapshotId>,
        /// The harness launched.
        harness: HarnessKind,
    },
    /// The job now references another environment spec.
    EnvSpecChanged {
        /// The spec's identity.
        env_spec: EnvSpecId,
    },
    /// A decision the user or the agent made, in words.
    DecisionRecorded {
        /// The decision.
        text: String,
    },
    /// Free text.
    Note {
        /// The note.
        text: String,
    },
}

impl EventKind {
    /// Returns true for the two kinds that raise the epoch by one.
    #[must_use]
    pub const fn bumps_epoch(&self) -> bool {
        matches!(
            self,
            EventKind::MoveImported { .. } | EventKind::Stolen { .. }
        )
    }

    /// Returns the kind's name for messages.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            EventKind::Created { .. } => "Created",
            EventKind::MoveStarted { .. } => "MoveStarted",
            EventKind::MoveImported { .. } => "MoveImported",
            EventKind::MoveAborted { .. } => "MoveAborted",
            EventKind::Stolen { .. } => "Stolen",
            EventKind::SnapshotPublished { .. } => "SnapshotPublished",
            EventKind::Suspended { .. } => "Suspended",
            EventKind::Resumed { .. } => "Resumed",
            EventKind::EnvSpecChanged { .. } => "EnvSpecChanged",
            EventKind::DecisionRecorded { .. } => "DecisionRecorded",
            EventKind::Note { .. } => "Note",
        }
    }
}

/// Why record bytes were refused.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The record carries a format version this crate does not know.
    #[error("record format version {version} is not supported")]
    UnknownVersion {
        /// The version byte found.
        version: u8,
    },
    /// A record that is not the tail failed its checksum or framing.
    #[error("ledger corrupt at byte offset {offset}")]
    Corrupt {
        /// Byte offset of the failing record's length field.
        offset: usize,
    },
    /// The record is longer than `MAX_RECORD_LEN`.
    #[error("record is {len} bytes, above the {MAX_RECORD_LEN} byte bound")]
    TooLarge {
        /// The byte length that was offered or asked for.
        len: usize,
    },
    /// The frame checks out but the event inside does not decode, or an event does not encode.
    #[error("record codec: {0}")]
    Codec(#[from] CodecError),
}

/// One event with the exact bytes it occupies on disk and their hash.
#[derive(Clone, PartialEq, Eq)]
pub struct Record {
    event: Event,
    bytes: Vec<u8>,
    hash: RecordHash,
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("event", &self.event)
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

impl Record {
    /// Frames `event` as `u32 length | u32 header_check | u8 version | postcard(event) | checksum`.
    ///
    /// `length` counts the bytes after the header. `header_check` is blake3
    /// of the length field, so a damaged length is corruption and never a
    /// torn tail. `checksum` is blake3 of everything before it.
    pub fn encode(event: Event) -> Result<Record, RecordError> {
        let payload = codec::encode(&event)?;
        let body_len = VERSION_LEN + payload.len() + CHECKSUM_LEN;
        let frame_len = HEADER_LEN + body_len;
        if frame_len > MAX_RECORD_LEN {
            return Err(RecordError::TooLarge { len: frame_len });
        }
        let length =
            u32::try_from(body_len).map_err(|_| RecordError::TooLarge { len: frame_len })?;
        let mut bytes = Vec::with_capacity(frame_len);
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(&header_check(length));
        bytes.push(RECORD_FORMAT_VERSION);
        bytes.extend_from_slice(&payload);
        let digest = blake3::hash(&bytes);
        bytes.extend_from_slice(&digest.as_bytes()[..CHECKSUM_LEN]);
        let hash = RecordHash::of(&bytes);
        Ok(Record { event, bytes, hash })
    }

    /// Decodes exactly one whole frame.
    pub fn decode(frame: &[u8]) -> Result<Record, RecordError> {
        match Self::read_one(frame, 0)? {
            Some((record, consumed)) if consumed == frame.len() => Ok(record),
            _ => Err(RecordError::Corrupt { offset: 0 }),
        }
    }

    /// Returns the event.
    #[must_use]
    pub fn event(&self) -> &Event {
        &self.event
    }

    /// Returns the exact bytes of the frame.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the blake3 hash of the frame, which the next record links to.
    #[must_use]
    pub fn hash(&self) -> RecordHash {
        self.hash
    }

    /// Reads the record at `offset`; `Ok(None)` means a torn tail by the ADR 0002 rule.
    ///
    /// A torn tail is the last thing in the buffer: every byte from `offset`
    /// is zero, the header is cut short, or the header verifies and the
    /// declared end lies at or past the end of the buffer. A header that does
    /// not verify, or a checksum failure before the end, is `Corrupt`.
    fn read_one(buf: &[u8], offset: usize) -> Result<Option<(Record, usize)>, RecordError> {
        let rest = &buf[offset..];
        if rest.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        let Some(header) = rest.get(..HEADER_LEN) else {
            return Ok(None);
        };
        let (length_bytes, check) = header.split_at(LENGTH_LEN);
        let length = u32::from_le_bytes([
            length_bytes[0],
            length_bytes[1],
            length_bytes[2],
            length_bytes[3],
        ]);
        if header_check(length) != *check {
            return Err(RecordError::Corrupt { offset });
        }
        let length = length as usize;
        if length < MIN_BODY_LEN {
            return Err(RecordError::Corrupt { offset });
        }
        if HEADER_LEN + length > MAX_RECORD_LEN {
            return Err(RecordError::TooLarge {
                len: HEADER_LEN + length,
            });
        }
        let Some(frame) = rest.get(..HEADER_LEN + length) else {
            return Ok(None);
        };
        let (hashed, checksum) = frame.split_at(frame.len() - CHECKSUM_LEN);
        if blake3::hash(hashed).as_bytes()[..CHECKSUM_LEN] != *checksum {
            if frame.len() == rest.len() {
                return Ok(None);
            }
            return Err(RecordError::Corrupt { offset });
        }
        let version = hashed[HEADER_LEN];
        if version != RECORD_FORMAT_VERSION {
            return Err(RecordError::UnknownVersion { version });
        }
        let event: Event = match codec::decode(&hashed[HEADER_LEN + VERSION_LEN..]) {
            Ok(event) => event,
            Err(CodecError::TrailingBytes(_)) => return Err(RecordError::Corrupt { offset }),
            Err(err) => return Err(RecordError::Codec(err)),
        };
        let bytes = frame.to_vec();
        let hash = RecordHash::of(&bytes);
        Ok(Some((Record { event, bytes, hash }, frame.len())))
    }
}

/// The result of walking a ledger file's bytes.
#[derive(Debug, Default)]
pub struct Scan {
    /// Every complete record, in file order.
    pub records: Vec<Record>,
    /// The offset of a torn tail to truncate at, if the bytes end mid-record.
    pub torn_tail: Option<usize>,
}

/// Walks `bytes` record by record.
///
/// A record that fails is a torn tail only when it is the last thing in the
/// buffer: its header verifies and its declared end lies at or past the end,
/// its header is cut short, or every byte from its start is zero. Any other
/// failure, a damaged length included, is `RecordError::Corrupt` with the
/// offset, and nothing after it is returned. The caller decides what to do
/// with a torn tail; `scan` never drops a record that is followed by another.
pub fn scan(bytes: &[u8]) -> Result<Scan, RecordError> {
    let mut out = Scan::default();
    let mut offset = 0;
    while offset < bytes.len() {
        let Some((record, consumed)) = Record::read_one(bytes, offset)? else {
            out.torn_tail = Some(offset);
            break;
        };
        out.records.push(record);
        offset += consumed;
    }
    Ok(out)
}

/// How a record reached this node, which decides how much the ledger can check.
///
/// Every record is checked against the chain: it must link to the head,
/// carry the epoch the rules require, and name a writer, snapshot and move
/// the chain allows. `Node` adds the sender rule: the transport-proved
/// sender must be that writer. `Relayed` cannot add it, because records
/// carry no signature yet (ADR 0002, Open questions), so a relayed suffix
/// is trusted as far as the paired peer that sent it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    /// A peer whose identity the transport proved and who must be the writer.
    Node(NodeId),
    /// A record fetched from a paired peer that may not be its writer; the sender rule is skipped.
    Relayed,
}

impl Author {
    const fn sender(self) -> Option<NodeId> {
        match self {
            Author::Node(node) => Some(node),
            Author::Relayed => None,
        }
    }
}

/// Why a record was refused by the ledger rules.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// A ledger must start with `Created`.
    #[error("ledger has no records")]
    Empty,
    /// The `Created` record names another job.
    #[error("ledger is for job {found}, not {expected}")]
    WrongJob {
        /// The job the record names.
        found: JobId,
        /// The job the caller expected.
        expected: JobId,
    },
    /// The first record is not `Created`.
    #[error("first record is {0}, not Created")]
    FirstNotCreated(&'static str),
    /// A `Created` record after the first.
    #[error("Created after the first record")]
    CreatedTwice,
    /// `Created` must carry no `prev` and epoch one.
    #[error("Created must have no prev and epoch 1")]
    BadCreated,
    /// The record does not link to this ledger's head.
    #[error("record does not link to head {head}")]
    PrevMismatch {
        /// The hash a record must carry as `prev` to be accepted now.
        head: RecordHash,
    },
    /// The record carries an epoch the rules do not allow at this point.
    #[error("stale epoch {yours}, current is {current}")]
    StaleEpoch {
        /// The record's epoch.
        yours: Epoch,
        /// The epoch the ledger holds.
        current: Epoch,
    },
    /// The epoch counter cannot grow.
    #[error("epoch counter exhausted")]
    EpochExhausted,
    /// The writer is not the node allowed to write this kind now.
    #[error("record must be written by {expected}")]
    NotAuthor {
        /// The node that may write it.
        expected: NodeId,
    },
    /// Only the owner may do this.
    #[error("job is owned by {owner}")]
    NotOwner {
        /// The owner of record.
        owner: NodeId,
    },
    /// The job is departing and only the target's decision is accepted.
    #[error("job is departing toward {target} under move {move_id}")]
    Departing {
        /// The open move.
        move_id: MoveId,
        /// The node that decides it.
        target: NodeId,
    },
    /// A decision arrived while no move is open.
    #[error("no move is open")]
    NoOpenMove,
    /// A decision names a move other than the open one.
    #[error("move {got} is not the open move {open}")]
    WrongMove {
        /// The open move.
        open: MoveId,
        /// The move the record names.
        got: MoveId,
    },
    /// A record names the wrong source node.
    #[error("record must name {expected} as the source")]
    WrongSource {
        /// The node the record must name.
        expected: NodeId,
    },
    /// A move toward the owner itself.
    #[error("cannot move a job to its owner")]
    MoveToSelf,
    /// A steal by the owner itself.
    #[error("the owner cannot steal its own job")]
    StealBySelf,
    /// A move started while the head is not `Suspended`.
    #[error("job must be suspended before it moves")]
    NotSuspended,
    /// A record names a snapshot the ledger has not published.
    #[error("snapshot {snapshot} is not the latest published snapshot")]
    UnknownSnapshot {
        /// The snapshot named.
        snapshot: SnapshotId,
    },
    /// `Resumed` without a snapshot after one exists.
    #[error("Resumed must name a snapshot once one is published")]
    ResumedWithoutSnapshot,
    /// A steal without `--force`.
    #[error("steal needs --force")]
    StealNeedsForce,
    /// A steal from a node whose store lacks the latest snapshot.
    #[error("steal refused: the latest snapshot is not complete in the local store")]
    StealWithoutSnapshot,
    /// This node is not the target the open move names.
    #[error("this node is not the target of move {move_id}")]
    NotTarget {
        /// The open move.
        move_id: MoveId,
    },
    /// The move id is not open and was never decided here.
    #[error("move {0} is unknown here")]
    UnknownMove(MoveId),
    /// The bytes did not decode as a record.
    #[error(transparent)]
    Record(#[from] RecordError),
}

/// The derived summary of a ledger that any node can rebuild from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobManifest {
    /// The job.
    pub job_id: JobId,
    /// The goal in the user's words.
    pub goal: String,
    /// The harness the job currently runs under.
    pub harness: HarnessKind,
    /// The environment spec the job references, once one was set.
    pub env_spec: Option<EnvSpecId>,
    /// The last snapshot published.
    pub latest_snapshot: Option<SnapshotId>,
    /// Who may run the job.
    pub lease: LeaseState,
    /// The hash of the last record.
    pub head: RecordHash,
    /// How many records the ledger holds.
    pub event_count: u64,
    /// The `at` of the `Created` record.
    pub created_at: u64,
    /// The `at` of the last record.
    pub updated_at: u64,
}

impl JobManifest {
    /// Returns the bytes to persist: the version, then the fields.
    pub fn encode(&self) -> Result<Vec<u8>, JobManifestError> {
        Ok(codec::encode_versioned(JOB_MANIFEST_VERSION, self)?)
    }

    /// Decodes bytes written by `encode`, refusing other versions.
    pub fn decode(bytes: &[u8]) -> Result<Self, JobManifestError> {
        Ok(codec::decode_versioned(bytes, JOB_MANIFEST_VERSION)?)
    }
}

/// Why job manifest bytes were refused.
#[derive(Debug, thiserror::Error)]
pub enum JobManifestError {
    /// The bytes carry a version this crate does not know.
    #[error("job manifest version {0} is not supported")]
    UnknownVersion(u16),
    /// The bytes do not decode, or the value does not encode.
    #[error("job manifest codec: {0}")]
    Codec(#[from] CodecError),
}

impl From<VersionedError> for JobManifestError {
    fn from(err: VersionedError) -> Self {
        match err {
            VersionedError::UnknownVersion(v) => JobManifestError::UnknownVersion(v),
            VersionedError::Codec(c) => JobManifestError::Codec(c),
        }
    }
}

/// Where records live between runs.
///
/// `append` must make the record durable before it returns; the ledger
/// shows a record to callers only after the store accepted it.
pub trait LedgerStore {
    /// The store's own failure type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Appends one record and makes it durable.
    fn append(&mut self, record: &Record) -> Result<(), Self::Error>;

    /// Returns every record in the order it was appended.
    fn records(&self) -> Result<Vec<Record>, Self::Error>;
}

/// A store that keeps records in memory; for tests and in-memory replicas.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    records: Vec<Record>,
}

impl LedgerStore for MemoryStore {
    type Error = Infallible;

    fn append(&mut self, record: &Record) -> Result<(), Infallible> {
        self.records.push(record.clone());
        Ok(())
    }

    fn records(&self) -> Result<Vec<Record>, Infallible> {
        Ok(self.records.clone())
    }
}

/// A failure of a ledger operation that also touches a store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError<E: std::error::Error + 'static> {
    /// The store failed.
    #[error("ledger store: {0}")]
    Store(#[source] E),
    /// The ledger rules refused the record.
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

/// A job's hash-chained history and the lease derived from it.
#[derive(Debug, Clone)]
pub struct Ledger {
    records: Vec<Record>,
    manifest: JobManifest,
}

impl Ledger {
    /// Builds the first record of a new job.
    pub fn created(
        job_id: JobId,
        goal: String,
        harness: HarnessKind,
        created_by: NodeId,
        id: EventId,
        at: u64,
    ) -> Result<Record, RecordError> {
        Record::encode(Event {
            id,
            prev: None,
            epoch: Epoch::FIRST,
            at,
            kind: EventKind::Created {
                job_id,
                goal,
                harness,
                created_by,
            },
        })
    }

    /// Rebuilds a ledger from records this node wrote or accepted earlier.
    ///
    /// `job_id` is the job the caller expects, from the directory or the
    /// message envelope; a chain whose `Created` names another job is
    /// refused with `WrongJob`.
    pub fn replay(
        job_id: JobId,
        records: impl IntoIterator<Item = Record>,
    ) -> Result<Ledger, LedgerError> {
        let mut records = records.into_iter();
        let first = records.next().ok_or(LedgerError::Empty)?;
        let mut ledger = Ledger::from_created(job_id, first)?;
        for record in records {
            ledger.check(record.event(), None)?;
            ledger.apply(&record);
            ledger.records.push(record);
        }
        Ok(ledger)
    }

    /// Loads a ledger from `store`.
    pub fn open<S: LedgerStore>(job_id: JobId, store: &S) -> Result<Ledger, StoreError<S::Error>> {
        let records = store.records().map_err(StoreError::Store)?;
        Ok(Ledger::replay(job_id, records)?)
    }

    fn from_created(job_id: JobId, record: Record) -> Result<Ledger, LedgerError> {
        let event = record.event();
        let EventKind::Created {
            job_id: found,
            goal,
            harness,
            created_by,
        } = &event.kind
        else {
            return Err(LedgerError::FirstNotCreated(event.kind.name()));
        };
        if *found != job_id {
            return Err(LedgerError::WrongJob {
                found: *found,
                expected: job_id,
            });
        }
        if event.prev.is_some() || event.epoch != Epoch::FIRST {
            return Err(LedgerError::BadCreated);
        }
        let manifest = JobManifest {
            job_id,
            goal: goal.clone(),
            harness: harness.clone(),
            env_spec: None,
            latest_snapshot: None,
            lease: LeaseState::Owned {
                node: *created_by,
                epoch: Epoch::FIRST,
            },
            head: record.hash(),
            event_count: 1,
            created_at: event.at,
            updated_at: event.at,
        };
        Ok(Ledger {
            records: vec![record],
            manifest,
        })
    }

    /// Returns the job id.
    #[must_use]
    pub fn job_id(&self) -> JobId {
        self.manifest.job_id
    }

    /// Returns the derived summary.
    #[must_use]
    pub fn manifest(&self) -> &JobManifest {
        &self.manifest
    }

    /// Returns the lease.
    #[must_use]
    pub fn lease(&self) -> LeaseState {
        self.manifest.lease
    }

    /// Returns the hash of the last record.
    #[must_use]
    pub fn head(&self) -> RecordHash {
        self.manifest.head
    }

    /// Returns every record in order.
    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Returns the records after the one whose hash is `after`, if it is in the chain.
    #[must_use]
    pub fn suffix_after(&self, after: RecordHash) -> Option<&[Record]> {
        self.records
            .iter()
            .position(|r| r.hash() == after)
            .map(|i| &self.records[i + 1..])
    }

    /// Returns the token `node` must present to run the job, if it may run it.
    #[must_use]
    pub fn runnable_by(&self, node: NodeId) -> Option<FencingToken> {
        self.manifest
            .lease
            .runnable_by(node)
            .then_some(FencingToken {
                job_id: self.manifest.job_id,
                epoch: self.manifest.lease.epoch(),
            })
    }

    /// Checks a token against this ledger's job and epoch.
    pub fn fence(&self, token: FencingToken) -> Result<(), FenceError> {
        token.check(self.manifest.job_id, self.manifest.lease.epoch())
    }

    /// Builds a record that `author` wants to append now, checked against the rules.
    ///
    /// The record is not part of the ledger until `accept` or `commit` takes it.
    pub fn propose(
        &self,
        author: NodeId,
        kind: EventKind,
        id: EventId,
        at: u64,
    ) -> Result<Record, LedgerError> {
        let current = self.manifest.lease.epoch();
        let epoch = if kind.bumps_epoch() {
            current.next().ok_or(LedgerError::EpochExhausted)?
        } else {
            current
        };
        let event = Event {
            id,
            prev: Some(self.manifest.head),
            epoch,
            at,
            kind,
        };
        self.check(&event, Some(author))?;
        Ok(Record::encode(event)?)
    }

    /// Decodes a frame a peer sent and checks that `from` may append it here.
    pub fn verify(&self, frame: &[u8], from: NodeId) -> Result<Record, LedgerError> {
        let record = Record::decode(frame)?;
        self.check(record.event(), Some(from))?;
        Ok(record)
    }

    /// Appends a record after checking it against the current head and lease.
    pub fn accept(&mut self, record: Record, author: Author) -> Result<&Record, LedgerError> {
        self.check(record.event(), author.sender())?;
        self.apply(&record);
        self.records.push(record);
        Ok(&self.records[self.records.len() - 1])
    }

    /// Checks, stores durably, then appends; the order that keeps the file ahead of memory.
    pub fn commit<S: LedgerStore>(
        &mut self,
        store: &mut S,
        record: Record,
        author: Author,
    ) -> Result<&Record, StoreError<S::Error>> {
        self.check(record.event(), author.sender())?;
        store.append(&record).map_err(StoreError::Store)?;
        Ok(self.accept(record, author)?)
    }

    fn check(&self, event: &Event, sender: Option<NodeId>) -> Result<(), LedgerError> {
        if matches!(event.kind, EventKind::Created { .. }) {
            return Err(LedgerError::CreatedTwice);
        }
        if event.prev != Some(self.manifest.head) {
            return Err(LedgerError::PrevMismatch {
                head: self.manifest.head,
            });
        }
        let current = self.manifest.lease.epoch();
        let expected = if event.kind.bumps_epoch() {
            current.next().ok_or(LedgerError::EpochExhausted)?
        } else {
            current
        };
        if event.epoch != expected {
            return Err(LedgerError::StaleEpoch {
                yours: event.epoch,
                current,
            });
        }
        let writer = self.writer_for(&event.kind)?;
        if let Some(node) = sender
            && node != writer
        {
            return Err(LedgerError::NotAuthor { expected: writer });
        }
        self.check_content(&event.kind)
    }

    /// Returns the one node allowed to write `kind` at this point of the chain.
    fn writer_for(&self, kind: &EventKind) -> Result<NodeId, LedgerError> {
        match self.manifest.lease {
            LeaseState::Owned { node, .. } => match kind {
                EventKind::MoveStarted { target, .. } => {
                    if *target == node {
                        return Err(LedgerError::MoveToSelf);
                    }
                    Ok(node)
                }
                EventKind::MoveImported { .. } | EventKind::MoveAborted { .. } => {
                    Err(LedgerError::NoOpenMove)
                }
                EventKind::Stolen { by, from, .. } => steal_writer(node, *by, *from),
                EventKind::Created { .. } => Err(LedgerError::CreatedTwice),
                EventKind::SnapshotPublished { .. }
                | EventKind::Suspended { .. }
                | EventKind::Resumed { .. }
                | EventKind::EnvSpecChanged { .. }
                | EventKind::DecisionRecorded { .. }
                | EventKind::Note { .. } => Ok(node),
            },
            LeaseState::Departing {
                node,
                move_id,
                target,
                ..
            } => match kind {
                EventKind::MoveImported { move_id: got, from } => {
                    if *got != move_id {
                        return Err(LedgerError::WrongMove {
                            open: move_id,
                            got: *got,
                        });
                    }
                    if *from != node {
                        return Err(LedgerError::WrongSource { expected: node });
                    }
                    Ok(target)
                }
                EventKind::MoveAborted { move_id: got } => {
                    if *got != move_id {
                        return Err(LedgerError::WrongMove {
                            open: move_id,
                            got: *got,
                        });
                    }
                    Ok(target)
                }
                EventKind::Stolen { by, from, .. } => steal_writer(node, *by, *from),
                EventKind::Created { .. } => Err(LedgerError::CreatedTwice),
                _ => Err(LedgerError::Departing { move_id, target }),
            },
        }
    }

    fn check_content(&self, kind: &EventKind) -> Result<(), LedgerError> {
        match kind {
            EventKind::MoveStarted { .. } => {
                let head_kind = &self.records[self.records.len() - 1].event().kind;
                if !matches!(head_kind, EventKind::Suspended { .. }) {
                    return Err(LedgerError::NotSuspended);
                }
                Ok(())
            }
            EventKind::Suspended { snapshot } => {
                if self.manifest.latest_snapshot != Some(*snapshot) {
                    return Err(LedgerError::UnknownSnapshot {
                        snapshot: *snapshot,
                    });
                }
                Ok(())
            }
            EventKind::Resumed { node, snapshot, .. } => {
                if *node != self.manifest.lease.holder() {
                    return Err(LedgerError::NotOwner {
                        owner: self.manifest.lease.holder(),
                    });
                }
                if snapshot.is_none() && self.manifest.latest_snapshot.is_some() {
                    return Err(LedgerError::ResumedWithoutSnapshot);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn apply(&mut self, record: &Record) {
        let event = record.event();
        let m = &mut self.manifest;
        match &event.kind {
            EventKind::MoveStarted { move_id, target } => {
                m.lease = LeaseState::Departing {
                    node: m.lease.holder(),
                    epoch: m.lease.epoch(),
                    move_id: *move_id,
                    target: *target,
                };
            }
            EventKind::MoveImported { .. } => {
                if let LeaseState::Departing { target, .. } = m.lease {
                    m.lease = LeaseState::Owned {
                        node: target,
                        epoch: event.epoch,
                    };
                }
            }
            EventKind::MoveAborted { .. } => {
                m.lease = LeaseState::Owned {
                    node: m.lease.holder(),
                    epoch: m.lease.epoch(),
                };
            }
            EventKind::Stolen { by, .. } => {
                m.lease = LeaseState::Owned {
                    node: *by,
                    epoch: event.epoch,
                };
            }
            EventKind::SnapshotPublished { snapshot, .. } => m.latest_snapshot = Some(*snapshot),
            EventKind::Resumed { harness, .. } => m.harness = harness.clone(),
            EventKind::EnvSpecChanged { env_spec } => m.env_spec = Some(*env_spec),
            EventKind::Created { .. }
            | EventKind::Suspended { .. }
            | EventKind::DecisionRecorded { .. }
            | EventKind::Note { .. } => {}
        }
        m.head = record.hash();
        m.event_count += 1;
        m.updated_at = event.at;
    }
}

fn steal_writer(owner: NodeId, by: NodeId, from: NodeId) -> Result<NodeId, LedgerError> {
    if from != owner {
        return Err(LedgerError::WrongSource { expected: owner });
    }
    if by == owner {
        return Err(LedgerError::StealBySelf);
    }
    Ok(by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::EnvSpecId;

    fn node(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    fn eid() -> EventId {
        EventId::generate()
    }

    fn job() -> JobId {
        JobId::from_bytes([3; 16])
    }

    fn snap(b: u8) -> SnapshotId {
        SnapshotId::of(&[b])
    }

    fn new_ledger(owner: NodeId) -> Ledger {
        let created = Ledger::created(
            job(),
            "goal".into(),
            HarnessKind::ClaudeCode,
            owner,
            eid(),
            10,
        )
        .unwrap();
        Ledger::replay(job(), [created]).unwrap()
    }

    fn append(
        ledger: &mut Ledger,
        author: NodeId,
        kind: EventKind,
    ) -> Result<RecordHash, LedgerError> {
        let record = ledger.propose(author, kind, eid(), 20)?;
        Ok(ledger.accept(record, Author::Node(author))?.hash())
    }

    fn suspended_ledger(owner: NodeId) -> Ledger {
        let mut ledger = new_ledger(owner);
        append(
            &mut ledger,
            owner,
            EventKind::SnapshotPublished {
                snapshot: snap(1),
                parent: None,
            },
        )
        .unwrap();
        append(
            &mut ledger,
            owner,
            EventKind::Suspended { snapshot: snap(1) },
        )
        .unwrap();
        ledger
    }

    fn departing_ledger(owner: NodeId, target: NodeId, move_id: MoveId) -> Ledger {
        let mut ledger = suspended_ledger(owner);
        append(
            &mut ledger,
            owner,
            EventKind::MoveStarted { move_id, target },
        )
        .unwrap();
        ledger
    }

    #[test]
    fn created_makes_the_creator_owner_at_epoch_one() {
        let ledger = new_ledger(node(1));
        assert_eq!(
            ledger.lease(),
            LeaseState::Owned {
                node: node(1),
                epoch: Epoch::FIRST
            }
        );
        assert_eq!(ledger.manifest().event_count, 1);
        assert_eq!(ledger.manifest().created_at, 10);
        assert!(ledger.runnable_by(node(1)).is_some());
        assert!(ledger.runnable_by(node(2)).is_none());
    }

    #[test]
    fn first_record_must_be_created() {
        let note = Record::encode(Event {
            id: eid(),
            prev: None,
            epoch: Epoch::FIRST,
            at: 1,
            kind: EventKind::Note { text: "x".into() },
        })
        .unwrap();
        assert!(matches!(
            Ledger::replay(job(), [note]),
            Err(LedgerError::FirstNotCreated("Note"))
        ));
        assert!(matches!(Ledger::replay(job(), []), Err(LedgerError::Empty)));
    }

    #[test]
    fn created_with_prev_or_other_epoch_is_refused() {
        let bad = Record::encode(Event {
            id: eid(),
            prev: Some(RecordHash::of(b"x")),
            epoch: Epoch::FIRST,
            at: 1,
            kind: EventKind::Created {
                job_id: job(),
                goal: "g".into(),
                harness: HarnessKind::Codex,
                created_by: node(1),
            },
        })
        .unwrap();
        assert!(matches!(
            Ledger::replay(job(), [bad]),
            Err(LedgerError::BadCreated)
        ));
    }

    #[test]
    fn replay_under_another_job_id_is_refused() {
        let ledger = new_ledger(node(1));
        let other = JobId::from_bytes([4; 16]);
        let err = Ledger::replay(other, ledger.records().iter().cloned()).unwrap_err();
        assert!(matches!(
            err,
            LedgerError::WrongJob { found, expected } if found == job() && expected == other
        ));
        assert_eq!(ledger.job_id(), job());
    }

    #[test]
    fn a_second_created_is_refused() {
        let mut ledger = new_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::Created {
                job_id: job(),
                goal: "again".into(),
                harness: HarnessKind::Codex,
                created_by: node(1),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::CreatedTwice));
    }

    #[test]
    fn a_record_that_does_not_link_to_head_is_refused() {
        let mut ledger = new_ledger(node(1));
        let stale_head = ledger.head();
        append(&mut ledger, node(1), EventKind::Note { text: "a".into() }).unwrap();
        let unlinked = Record::encode(Event {
            id: eid(),
            prev: Some(stale_head),
            epoch: Epoch::FIRST,
            at: 1,
            kind: EventKind::Note { text: "b".into() },
        })
        .unwrap();
        let err = ledger.accept(unlinked, Author::Relayed).unwrap_err();
        assert!(matches!(err, LedgerError::PrevMismatch { head } if head == ledger.head()));
    }

    #[test]
    fn a_record_with_the_wrong_epoch_is_stale() {
        let mut ledger = new_ledger(node(1));
        let two = Epoch::FIRST.next().unwrap();
        let ahead = Record::encode(Event {
            id: eid(),
            prev: Some(ledger.head()),
            epoch: two,
            at: 1,
            kind: EventKind::Note { text: "b".into() },
        })
        .unwrap();
        let err = ledger.accept(ahead, Author::Relayed).unwrap_err();
        assert!(matches!(
            err,
            LedgerError::StaleEpoch { yours, current } if yours == two && current == Epoch::FIRST
        ));
    }

    #[test]
    fn only_the_owner_writes_while_owned() {
        let mut ledger = new_ledger(node(1));
        let err = append(&mut ledger, node(2), EventKind::Note { text: "x".into() }).unwrap_err();
        assert!(matches!(err, LedgerError::NotAuthor { expected } if expected == node(1)));
    }

    #[test]
    fn move_needs_a_suspended_head_and_another_node() {
        let mut ledger = new_ledger(node(1));
        let move_id = MoveId::generate();
        let err = append(
            &mut ledger,
            node(1),
            EventKind::MoveStarted {
                move_id,
                target: node(2),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::NotSuspended));
        let mut ledger = suspended_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::MoveStarted {
                move_id,
                target: node(1),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::MoveToSelf));
    }

    #[test]
    fn departing_accepts_only_the_targets_decision_for_the_open_move() {
        let move_id = MoveId::generate();
        let mut ledger = departing_ledger(node(1), node(2), move_id);
        assert!(!ledger.lease().runnable_by(node(1)));
        assert!(!ledger.lease().runnable_by(node(2)));

        let err = append(&mut ledger, node(1), EventKind::Note { text: "x".into() }).unwrap_err();
        assert!(matches!(err, LedgerError::Departing { target, .. } if target == node(2)));

        let other = MoveId::generate();
        let err = append(
            &mut ledger,
            node(2),
            EventKind::MoveAborted { move_id: other },
        )
        .unwrap_err();
        assert!(
            matches!(err, LedgerError::WrongMove { open, got } if open == move_id && got == other)
        );

        let err = append(
            &mut ledger,
            node(2),
            EventKind::MoveImported {
                move_id,
                from: node(9),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::WrongSource { expected } if expected == node(1)));

        let err = append(
            &mut ledger,
            node(3),
            EventKind::MoveImported {
                move_id,
                from: node(1),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::NotAuthor { expected } if expected == node(2)));
    }

    #[test]
    fn a_relayed_record_skips_only_the_sender_rule() {
        let move_id = MoveId::from_bytes([5; 16]);
        let on_target = departing_ledger(node(1), node(2), move_id);
        let mut replica = on_target.clone();
        let imported = on_target
            .propose(
                node(2),
                EventKind::MoveImported {
                    move_id,
                    from: node(1),
                },
                eid(),
                30,
            )
            .unwrap();
        let err = replica
            .accept(imported.clone(), Author::Node(node(3)))
            .unwrap_err();
        assert!(matches!(err, LedgerError::NotAuthor { expected } if expected == node(2)));
        replica.accept(imported, Author::Relayed).unwrap();
        assert_eq!(replica.lease().holder(), node(2));
        let unlinked = Record::encode(Event {
            id: eid(),
            prev: None,
            epoch: replica.lease().epoch(),
            at: 31,
            kind: EventKind::Note { text: "x".into() },
        })
        .unwrap();
        assert!(matches!(
            replica.accept(unlinked, Author::Relayed),
            Err(LedgerError::PrevMismatch { .. })
        ));
    }

    #[test]
    fn import_hands_the_job_to_the_target_at_the_next_epoch() {
        let move_id = MoveId::generate();
        let mut ledger = departing_ledger(node(1), node(2), move_id);
        append(
            &mut ledger,
            node(2),
            EventKind::MoveImported {
                move_id,
                from: node(1),
            },
        )
        .unwrap();
        assert_eq!(
            ledger.lease(),
            LeaseState::Owned {
                node: node(2),
                epoch: Epoch::FIRST.next().unwrap()
            }
        );
    }

    #[test]
    fn abort_returns_the_job_to_the_source_at_the_same_epoch() {
        let move_id = MoveId::generate();
        let mut ledger = departing_ledger(node(1), node(2), move_id);
        append(&mut ledger, node(2), EventKind::MoveAborted { move_id }).unwrap();
        assert_eq!(
            ledger.lease(),
            LeaseState::Owned {
                node: node(1),
                epoch: Epoch::FIRST
            }
        );
    }

    #[test]
    fn a_decision_without_an_open_move_is_refused() {
        let mut ledger = new_ledger(node(1));
        let err = append(
            &mut ledger,
            node(2),
            EventKind::MoveAborted {
                move_id: MoveId::generate(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::NoOpenMove));
    }

    #[test]
    fn stolen_bumps_the_epoch_and_names_the_owner_of_record() {
        let mut ledger = new_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::Stolen {
                by: node(1),
                from: node(1),
                reason: String::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::StealBySelf));
        let err = append(
            &mut ledger,
            node(2),
            EventKind::Stolen {
                by: node(2),
                from: node(7),
                reason: String::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::WrongSource { expected } if expected == node(1)));
        append(
            &mut ledger,
            node(2),
            EventKind::Stolen {
                by: node(2),
                from: node(1),
                reason: "pro asleep".into(),
            },
        )
        .unwrap();
        assert_eq!(
            ledger.lease(),
            LeaseState::Owned {
                node: node(2),
                epoch: Epoch::FIRST.next().unwrap()
            }
        );
    }

    #[test]
    fn stolen_while_departing_is_written_by_the_thief() {
        let move_id = MoveId::generate();
        let mut ledger = departing_ledger(node(1), node(2), move_id);
        append(
            &mut ledger,
            node(3),
            EventKind::Stolen {
                by: node(3),
                from: node(1),
                reason: String::new(),
            },
        )
        .unwrap();
        assert_eq!(ledger.lease().holder(), node(3));
        assert_eq!(ledger.lease().epoch(), Epoch::FIRST.next().unwrap());
    }

    #[test]
    fn suspended_must_name_the_latest_published_snapshot() {
        let mut ledger = new_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::Suspended { snapshot: snap(1) },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::UnknownSnapshot { snapshot } if snapshot == snap(1)));
    }

    #[test]
    fn resumed_needs_the_owner_and_a_snapshot_once_one_exists() {
        let mut ledger = new_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::Resumed {
                node: node(2),
                snapshot: None,
                harness: HarnessKind::Codex,
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::NotOwner { owner } if owner == node(1)));
        append(
            &mut ledger,
            node(1),
            EventKind::Resumed {
                node: node(1),
                snapshot: None,
                harness: HarnessKind::Codex,
            },
        )
        .unwrap();
        assert_eq!(ledger.manifest().harness, HarnessKind::Codex);
        let mut ledger = suspended_ledger(node(1));
        let err = append(
            &mut ledger,
            node(1),
            EventKind::Resumed {
                node: node(1),
                snapshot: None,
                harness: HarnessKind::ClaudeCode,
            },
        )
        .unwrap_err();
        assert!(matches!(err, LedgerError::ResumedWithoutSnapshot));
    }

    #[test]
    fn env_spec_changed_updates_the_manifest() {
        let mut ledger = new_ledger(node(1));
        let id = EnvSpecId::of(b"spec");
        append(
            &mut ledger,
            node(1),
            EventKind::EnvSpecChanged { env_spec: id },
        )
        .unwrap();
        assert_eq!(ledger.manifest().env_spec, Some(id));
    }

    #[test]
    fn replay_rebuilds_the_same_manifest() {
        let move_id = MoveId::generate();
        let mut ledger = departing_ledger(node(1), node(2), move_id);
        append(
            &mut ledger,
            node(2),
            EventKind::MoveImported {
                move_id,
                from: node(1),
            },
        )
        .unwrap();
        let rebuilt = Ledger::replay(job(), ledger.records().iter().cloned()).unwrap();
        assert_eq!(rebuilt.manifest(), ledger.manifest());
        let bytes = ledger.manifest().encode().unwrap();
        assert_eq!(&JobManifest::decode(&bytes).unwrap(), ledger.manifest());
    }

    #[test]
    fn job_manifest_with_trailing_bytes_is_refused() {
        let ledger = new_ledger(node(1));
        let mut bytes = ledger.manifest().encode().unwrap();
        bytes.push(0);
        assert!(matches!(
            JobManifest::decode(&bytes),
            Err(JobManifestError::Codec(CodecError::TrailingBytes(1)))
        ));
    }

    #[test]
    fn commit_persists_before_it_shows_and_persists_nothing_invalid() {
        let mut store = MemoryStore::default();
        let created =
            Ledger::created(job(), "g".into(), HarnessKind::Codex, node(1), eid(), 1).unwrap();
        store.append(&created).unwrap();
        let mut ledger = Ledger::open(job(), &store).unwrap();
        let note = ledger
            .propose(node(1), EventKind::Note { text: "n".into() }, eid(), 2)
            .unwrap();
        ledger
            .commit(&mut store, note, Author::Node(node(1)))
            .unwrap();
        assert_eq!(store.records().unwrap().len(), 2);
        let bad = Record::encode(Event {
            id: eid(),
            prev: None,
            epoch: Epoch::FIRST,
            at: 3,
            kind: EventKind::Note {
                text: "unlinked".into(),
            },
        })
        .unwrap();
        assert!(
            ledger
                .commit(&mut store, bad, Author::Node(node(1)))
                .is_err()
        );
        assert_eq!(store.records().unwrap().len(), 2);
        assert_eq!(ledger.records().len(), 2);
    }

    #[test]
    fn suffix_after_returns_what_a_replica_lacks() {
        let mut ledger = new_ledger(node(1));
        let first = ledger.head();
        append(&mut ledger, node(1), EventKind::Note { text: "a".into() }).unwrap();
        append(&mut ledger, node(1), EventKind::Note { text: "b".into() }).unwrap();
        assert_eq!(ledger.suffix_after(first).unwrap().len(), 2);
        assert_eq!(ledger.suffix_after(ledger.head()).unwrap().len(), 0);
        assert!(ledger.suffix_after(RecordHash::of(b"elsewhere")).is_none());
    }

    #[test]
    fn scan_refuses_an_unknown_format_version() {
        let a = Ledger::created(job(), "g".into(), HarnessKind::Codex, node(1), eid(), 1).unwrap();
        let mut frame = a.bytes().to_vec();
        frame[HEADER_CHECK_LEN + 4] = 9;
        let hashed_len = frame.len() - CHECKSUM_LEN;
        let digest = blake3::hash(&frame[..hashed_len]);
        frame[hashed_len..].copy_from_slice(&digest.as_bytes()[..CHECKSUM_LEN]);
        assert!(matches!(
            Record::decode(&frame),
            Err(RecordError::UnknownVersion { version: 9 })
        ));
    }
}
