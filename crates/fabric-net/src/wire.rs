//! The message vocabulary (ADR 0005), defined once.
//!
//! Invariant: every per-job message carries the fencing token, job id and
//! epoch, in one place, the envelope, so a receiver runs the ADR 0003 check
//! before it reads the body. Nothing of unbounded size is a control
//! message: a record is at most `MAX_RECORD_LEN` bytes and records travel
//! in pages of at most `MAX_RECORD_PAGE` bytes, manifests travel as
//! chunks, and one chunk batch names at most `MAX_CHUNK_BATCH` ids. Every
//! bound is enforced when bytes are parsed, not after, so the largest
//! message fits the control frame of the `frame` module.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::id::{ChunkId, Epoch, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::FencingToken;
use fabric_core::ledger::MAX_RECORD_LEN;
use fabric_core::moves::MoveOffer;
use fabric_core::snapshot::MAX_MANIFEST_LEN;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::frame::MAX_CHUNK_BYTES;

/// The protocol version both sides must speak; a mismatch ends the session at `Hello`.
pub const PROTOCOL_VERSION: u16 = 1;

/// The most chunk ids one `WantChunks` request may name.
pub const MAX_CHUNK_BATCH: usize = 256;

/// The longest fabric version string in a `Hello`.
pub const MAX_VERSION_LEN: usize = 64;

/// The longest text a `Fault` may carry.
pub const MAX_FAULT_TEXT: usize = 256;

/// The most bytes the length prefix of one record takes on the wire: a
/// varint of a length up to `MAX_RECORD_LEN`.
const RECORD_PREFIX: usize = 3;

/// The most a page of records may cost, counting each record as its length
/// plus `RECORD_PREFIX`: one record of the largest size, or many small ones.
pub const MAX_RECORD_PAGE: usize = MAX_RECORD_LEN + RECORD_PREFIX;

/// Why bytes were not a message, or a value was not valid on the wire.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// The bytes are not an encoding of the type, or the value cannot be encoded.
    #[error("malformed message: {0}")]
    Malformed(#[source] postcard::Error),
    /// Bytes follow the encoded value.
    #[error("{0} bytes follow the message")]
    Trailing(usize),
    /// A chunk batch is empty or names more than `MAX_CHUNK_BATCH` ids.
    #[error("a chunk batch names 1 to {MAX_CHUNK_BATCH} ids, not {0}")]
    BatchSize(usize),
    /// A chunk batch names one id twice.
    #[error("a chunk batch names {0} twice")]
    DuplicateChunk(ChunkId),
    /// A text field exceeds its cap.
    #[error("text of {len} bytes exceeds the cap of {max}")]
    TextTooLong {
        /// The length found.
        len: usize,
        /// The cap in force.
        max: usize,
    },
    /// A record is empty or longer than `MAX_RECORD_LEN`.
    #[error("a record is 1 to {MAX_RECORD_LEN} bytes, not {0}")]
    RecordSize(usize),
    /// The records do not fit one page of `MAX_RECORD_PAGE`.
    #[error("the records do not fit a page of {MAX_RECORD_PAGE} bytes")]
    PageFull,
    /// A manifest is empty or longer than a reader decodes.
    #[error("a manifest is 1 to {MAX_MANIFEST_LEN} bytes, not {0}")]
    ManifestSize(u64),
    /// A manifest's chunk count cannot make its length.
    #[error("a manifest of {len} bytes cannot be {count} chunks")]
    ManifestChunkCount {
        /// The manifest's length.
        len: u64,
        /// The chunk count given.
        count: usize,
    },
}

/// Encodes a message as postcard bytes.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    postcard::to_stdvec(value).map_err(WireError::Malformed)
}

/// Decodes exactly one message that occupies all of `bytes`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    let (value, rest) = postcard::take_from_bytes::<T>(bytes).map_err(WireError::Malformed)?;
    if !rest.is_empty() {
        return Err(WireError::Trailing(rest.len()));
    }
    Ok(value)
}

/// A short text with a byte cap enforced on parse and on decode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BoundedText<const MAX: usize>(String);

impl<const MAX: usize> BoundedText<MAX> {
    /// Wraps `text` if it is at most `MAX` bytes.
    pub fn new(text: impl Into<String>) -> Result<Self, WireError> {
        let text = text.into();
        if text.len() > MAX {
            return Err(WireError::TextTooLong {
                len: text.len(),
                max: MAX,
            });
        }
        Ok(Self(text))
    }

    /// Wraps `text`, cutting it at a character boundary to fit `MAX` bytes.
    #[must_use]
    pub fn truncated(text: &str) -> Self {
        let mut end = text.len().min(MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self(text[..end].to_owned())
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedText<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

impl<const MAX: usize> fmt::Debug for BoundedText<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<const MAX: usize> fmt::Display for BoundedText<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The version string of the software behind a `Hello`.
pub type VersionText = BoundedText<MAX_VERSION_LEN>;

/// The text a `Fault` carries.
pub type FaultText = BoundedText<MAX_FAULT_TEXT>;

/// The first message in both directions on a new connection; it carries no job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The sender's node id; the receiver checks it against the handshake.
    pub node: NodeId,
    /// The protocol version the sender speaks.
    pub protocol_version: u16,
    /// The sender's software version, for logs and `fabric doctor`.
    pub fabric_version: VersionText,
    /// The sender's wall clock in milliseconds since the Unix epoch, to report skew.
    pub clock_ms: u64,
}

/// This node's wall clock in milliseconds since the Unix epoch; zero before 1970.
pub(crate) fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The number of a request within one session; the response repeats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One request: its number, the time the caller still allows, and the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The request number.
    pub id: RequestId,
    /// Milliseconds the caller will wait; the callee stops work past it.
    pub budget_ms: u32,
    /// What is asked.
    pub body: RequestBody,
}

/// A list of 1 to `MAX_CHUNK_BATCH` distinct chunk ids.
///
/// Each id is asked once, so the receiver can treat a second copy of a
/// chunk as a protocol error and the missing set stays exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ChunkBatch(Vec<ChunkId>);

impl ChunkBatch {
    /// Wraps `ids` if the count is within bounds and no id repeats.
    pub fn new(ids: Vec<ChunkId>) -> Result<Self, WireError> {
        if ids.is_empty() || ids.len() > MAX_CHUNK_BATCH {
            return Err(WireError::BatchSize(ids.len()));
        }
        let mut seen = std::collections::BTreeSet::new();
        if let Some(twice) = ids.iter().find(|id| !seen.insert(**id)) {
            return Err(WireError::DuplicateChunk(*twice));
        }
        Ok(Self(ids))
    }

    /// Returns the ids.
    #[must_use]
    pub fn ids(&self) -> &[ChunkId] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ChunkBatch {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ids = Vec::<ChunkId>::deserialize(deserializer)?;
        Self::new(ids).map_err(serde::de::Error::custom)
    }
}

/// The exact bytes of one ledger record: 1 to `MAX_RECORD_LEN` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RecordBytes(Vec<u8>);

impl RecordBytes {
    /// Wraps `bytes` if their length is within the ledger's bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_RECORD_LEN {
            return Err(WireError::RecordSize(bytes.len()));
        }
        Ok(Self(bytes))
    }

    /// Returns the bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the bytes, consuming the wrapper.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// What the record counts against a page.
    fn cost(&self) -> usize {
        self.0.len() + RECORD_PREFIX
    }
}

impl<'de> Deserialize<'de> for RecordBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::<u8>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Records in chain order that fit one control frame: their cost is at most `MAX_RECORD_PAGE`.
///
/// A sender fills a page with `push` until it refuses, sends it, and goes
/// on with a new page, so a long ledger crosses the wire in pieces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordPage {
    records: Vec<RecordBytes>,
    cost: usize,
}

impl RecordPage {
    /// Adds `record` at the end, or hands it back when the page has no room for it.
    pub fn push(&mut self, record: RecordBytes) -> Result<(), RecordBytes> {
        let cost = self.cost + record.cost();
        if cost > MAX_RECORD_PAGE {
            return Err(record);
        }
        self.records.push(record);
        self.cost = cost;
        Ok(())
    }

    /// Returns the records in chain order.
    #[must_use]
    pub fn records(&self) -> &[RecordBytes] {
        &self.records
    }

    /// Returns the records in chain order, consuming the page.
    #[must_use]
    pub fn into_records(self) -> Vec<RecordBytes> {
        self.records
    }
}

impl Serialize for RecordPage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.records.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RecordPage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut page = Self::default();
        for record in Vec::<RecordBytes>::deserialize(deserializer)? {
            page.push(record)
                .map_err(|_| serde::de::Error::custom(WireError::PageFull))?;
        }
        Ok(page)
    }
}

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestBody {
    /// The handshake; valid only as the first request of a session.
    Hello(Hello),
    /// Is the peer there? Answered with `Pong`.
    Ping,
    /// A per-job message, fenced by `token`.
    Job {
        /// The job and the epoch the sender believes is current.
        token: FencingToken,
        /// The message.
        body: JobRequest,
    },
    /// Asks for the chunk list of a snapshot's manifest.
    WantSnapshot {
        /// The snapshot.
        snapshot: SnapshotId,
    },
    /// Asks for the bytes of these chunks; answered with a run of `Chunk` then `ChunksDone`.
    WantChunks(ChunkBatch),
}

/// A per-job message (ADR 0003 and ADR 0005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobRequest {
    /// Asks for the records after `after`; answered with one page of `Records`.
    Pull {
        /// The receiver's head, or the record it last saw.
        after: RecordHash,
    },
    /// Replicates one page of records, each as its exact ledger bytes; answered with `Accepted`, `Rejected` or an epoch answer.
    Append {
        /// The records in chain order.
        records: RecordPage,
    },
    /// Asks the owner to start a move toward the sender; answered with `MoveStarted` or `Rejected`.
    MoveRequest,
    /// The owner's offer after `MoveStarted` is durable; answered with `MoveDecision`, `Rejected`, `SnapshotIncomplete` or an epoch answer.
    MoveOffer(OfferBody),
    /// Asks the target for its decision again; answered with `MoveDecision` or `MoveUndecided`.
    MoveStatus {
        /// The move.
        move_id: MoveId,
    },
    /// Asks the target to decide against the move; answered with `MoveDecision`.
    MoveAbort {
        /// The move.
        move_id: MoveId,
    },
}

/// A `MoveOffer` without its job id, which the request's fencing token carries.
///
/// The job id has one home on the wire, so a message cannot name one job in
/// its token and another in its offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferBody {
    /// The move.
    pub move_id: MoveId,
    /// The owner's head, which is the `MoveStarted` record.
    pub head: RecordHash,
    /// The snapshot the target must hold completely before it imports.
    pub snapshot: SnapshotId,
}

impl OfferBody {
    /// The full offer for `job_id`, the job the request's token names.
    #[must_use]
    pub fn for_job(self, job_id: JobId) -> MoveOffer {
        MoveOffer {
            job_id,
            move_id: self.move_id,
            head: self.head,
            snapshot: self.snapshot,
        }
    }
}

impl From<MoveOffer> for OfferBody {
    fn from(offer: MoveOffer) -> Self {
        Self {
            move_id: offer.move_id,
            head: offer.head,
            snapshot: offer.snapshot,
        }
    }
}

/// One response: the request number it answers and the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The request number.
    pub id: RequestId,
    /// The answer.
    pub body: ResponseBody,
}

/// Where a snapshot's manifest is: its byte length and the chunks that make it, in order.
///
/// The length is 1 to `MAX_MANIFEST_LEN` bytes, checked on decode before
/// any chunk is fetched, so a peer cannot make this node assemble more than
/// one manifest cap. The chunk count fits the length: each chunk holds at
/// least one byte and at most `MAX_CHUNK_BYTES`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestChunks {
    len: u64,
    ids: Vec<ChunkId>,
}

impl ManifestChunks {
    /// Wraps a manifest's length and chunk list if they are within bounds.
    pub fn new(len: u64, ids: Vec<ChunkId>) -> Result<Self, WireError> {
        let max = u64::try_from(MAX_MANIFEST_LEN).unwrap_or(u64::MAX);
        if len == 0 || len > max {
            return Err(WireError::ManifestSize(len));
        }
        let fewest = len.div_ceil(u64::try_from(MAX_CHUNK_BYTES).unwrap_or(u64::MAX));
        let count = u64::try_from(ids.len()).unwrap_or(u64::MAX);
        if count < fewest || count > len {
            return Err(WireError::ManifestChunkCount {
                len,
                count: ids.len(),
            });
        }
        Ok(Self { len, ids })
    }

    /// The manifest's length in bytes.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        self.len
    }

    /// The chunks that make the manifest, in order.
    #[must_use]
    pub fn ids(&self) -> &[ChunkId] {
        &self.ids
    }
}

impl<'de> Deserialize<'de> for ManifestChunks {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Unchecked {
            len: u64,
            ids: Vec<ChunkId>,
        }
        let unchecked = Unchecked::deserialize(deserializer)?;
        Self::new(unchecked.len, unchecked.ids).map_err(serde::de::Error::custom)
    }
}

/// What a response carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseBody {
    /// The acceptor's half of the handshake.
    Hello(Hello),
    /// The answer to `Ping`.
    Pong,
    /// The answer to a per-job message.
    Job(JobResponse),
    /// The manifest's byte length and ordered chunk list.
    ManifestChunks(ManifestChunks),
    /// One chunk as the store keeps it; the receiver trusts it only after it decompresses and hashes to `id`.
    Chunk {
        /// The chunk.
        id: ChunkId,
        /// One zstd frame of its bytes (ADR 0004), sent as stored.
        zstd: Vec<u8>,
    },
    /// Every requested chunk was sent.
    ChunksDone,
    /// The request was understood but could not be served.
    Fault(Fault),
}

/// The answer to a per-job message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobResponse {
    /// The first page of records after the asked head, in chain order.
    Records {
        /// The records.
        records: RecordPage,
        /// More records follow the page; the sender pulls again after its last record.
        more: bool,
    },
    /// The records linked; `head` is now the receiver's head.
    Accepted {
        /// The receiver's head.
        head: RecordHash,
    },
    /// The message does not link to the receiver's head; the sender pulls after `head` or pushes the suffix.
    Rejected {
        /// The receiver's head.
        head: RecordHash,
    },
    /// The sender's epoch is older than the receiver's; the sender fetches first.
    StaleEpoch {
        /// The sender's epoch.
        yours: Epoch,
        /// The receiver's epoch.
        current: Epoch,
    },
    /// The sender's epoch is newer than the receiver's; the receiver must fetch first.
    AheadOfUs {
        /// The sender's epoch.
        yours: Epoch,
        /// The receiver's epoch.
        current: Epoch,
    },
    /// The owner started a move toward the sender.
    MoveStarted {
        /// The move.
        move_id: MoveId,
    },
    /// The target's decision, byte for byte: a `MoveImported` or `MoveAborted` record.
    MoveDecision {
        /// The record bytes.
        record: RecordBytes,
    },
    /// The target has not decided.
    MoveUndecided {
        /// The move.
        move_id: MoveId,
    },
    /// The offer links but the snapshot is not whole on the target; the sender pushes it and offers again.
    SnapshotIncomplete {
        /// The snapshot to push.
        snapshot: SnapshotId,
    },
}

/// Why a request could not be served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fault {
    /// The receiver holds no such job.
    UnknownJob,
    /// The receiver holds no such snapshot.
    UnknownSnapshot,
    /// The receiver holds no such chunk; the run of chunks stops here.
    UnknownChunk(ChunkId),
    /// The caller's budget ran out before the work finished.
    Deadline,
    /// The request is not valid at this point of the session.
    BadRequest(FaultText),
    /// The receiver failed for a reason the caller cannot act on.
    Internal(FaultText),
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::UnknownJob => f.write_str("unknown job"),
            Fault::UnknownSnapshot => f.write_str("unknown snapshot"),
            Fault::UnknownChunk(id) => write!(f, "unknown chunk {id}"),
            Fault::Deadline => f.write_str("the budget ran out"),
            Fault::BadRequest(text) => write!(f, "bad request: {text}"),
            Fault::Internal(text) => write!(f, "peer failure: {text}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    fn bytes32<T: Copy + fmt::Debug>(wrap: fn([u8; 32]) -> T) -> impl Strategy<Value = T> {
        any::<[u8; 32]>().prop_map(wrap)
    }

    fn bytes16<T: Copy + fmt::Debug>(wrap: fn([u8; 16]) -> T) -> impl Strategy<Value = T> {
        any::<[u8; 16]>().prop_map(wrap)
    }

    fn epoch() -> impl Strategy<Value = Epoch> {
        (1..=u64::MAX).prop_filter_map("epoch zero does not exist", Epoch::new)
    }

    fn text<const MAX: usize>() -> impl Strategy<Value = BoundedText<MAX>> {
        "[ -~]{0,256}".prop_map(|s| BoundedText::truncated(&s))
    }

    fn hello() -> impl Strategy<Value = Hello> {
        (
            bytes32(NodeId::from_bytes),
            any::<u16>(),
            text::<MAX_VERSION_LEN>(),
            any::<u64>(),
        )
            .prop_map(|(node, protocol_version, fabric_version, clock_ms)| Hello {
                node,
                protocol_version,
                fabric_version,
                clock_ms,
            })
    }

    fn record() -> impl Strategy<Value = RecordBytes> {
        vec(any::<u8>(), 1..64).prop_map(|bytes| RecordBytes::new(bytes).unwrap())
    }

    fn records() -> impl Strategy<Value = RecordPage> {
        vec(record(), 0..4).prop_map(|records| {
            let mut page = RecordPage::default();
            for record in records {
                page.push(record).unwrap();
            }
            page
        })
    }

    fn job_request() -> impl Strategy<Value = JobRequest> {
        prop_oneof![
            bytes32(RecordHash::from_bytes).prop_map(|after| JobRequest::Pull { after }),
            records().prop_map(|records| JobRequest::Append { records }),
            Just(JobRequest::MoveRequest),
            (
                bytes16(MoveId::from_bytes),
                bytes32(RecordHash::from_bytes),
                bytes32(SnapshotId::from_bytes),
            )
                .prop_map(|(move_id, head, snapshot)| JobRequest::MoveOffer(
                    OfferBody {
                        move_id,
                        head,
                        snapshot,
                    }
                )),
            bytes16(MoveId::from_bytes).prop_map(|move_id| JobRequest::MoveStatus { move_id }),
            bytes16(MoveId::from_bytes).prop_map(|move_id| JobRequest::MoveAbort { move_id }),
        ]
    }

    fn request_body() -> impl Strategy<Value = RequestBody> {
        prop_oneof![
            hello().prop_map(RequestBody::Hello),
            Just(RequestBody::Ping),
            (bytes16(JobId::from_bytes), epoch(), job_request()).prop_map(
                |(job_id, epoch, body)| {
                    RequestBody::Job {
                        token: FencingToken { job_id, epoch },
                        body,
                    }
                }
            ),
            bytes32(SnapshotId::from_bytes)
                .prop_map(|snapshot| RequestBody::WantSnapshot { snapshot }),
            proptest::collection::btree_set(bytes32(ChunkId::from_bytes), 1..=MAX_CHUNK_BATCH)
                .prop_filter_map("batch", |ids| ChunkBatch::new(ids.into_iter().collect())
                    .ok())
                .prop_map(RequestBody::WantChunks),
        ]
    }

    fn job_response() -> impl Strategy<Value = JobResponse> {
        prop_oneof![
            (records(), any::<bool>())
                .prop_map(|(records, more)| JobResponse::Records { records, more }),
            bytes32(RecordHash::from_bytes).prop_map(|head| JobResponse::Accepted { head }),
            bytes32(RecordHash::from_bytes).prop_map(|head| JobResponse::Rejected { head }),
            (epoch(), epoch())
                .prop_map(|(yours, current)| JobResponse::StaleEpoch { yours, current }),
            (epoch(), epoch())
                .prop_map(|(yours, current)| JobResponse::AheadOfUs { yours, current }),
            bytes16(MoveId::from_bytes).prop_map(|move_id| JobResponse::MoveStarted { move_id }),
            record().prop_map(|record| JobResponse::MoveDecision { record }),
            bytes16(MoveId::from_bytes).prop_map(|move_id| JobResponse::MoveUndecided { move_id }),
            bytes32(SnapshotId::from_bytes)
                .prop_map(|snapshot| JobResponse::SnapshotIncomplete { snapshot }),
        ]
    }

    fn fault() -> impl Strategy<Value = Fault> {
        prop_oneof![
            Just(Fault::UnknownJob),
            Just(Fault::UnknownSnapshot),
            bytes32(ChunkId::from_bytes).prop_map(Fault::UnknownChunk),
            Just(Fault::Deadline),
            text::<MAX_FAULT_TEXT>().prop_map(Fault::BadRequest),
            text::<MAX_FAULT_TEXT>().prop_map(Fault::Internal),
        ]
    }

    fn response_body() -> impl Strategy<Value = ResponseBody> {
        prop_oneof![
            hello().prop_map(ResponseBody::Hello),
            Just(ResponseBody::Pong),
            job_response().prop_map(ResponseBody::Job),
            vec(bytes32(ChunkId::from_bytes), 1..8)
                .prop_flat_map(|ids| {
                    let count = u64::try_from(ids.len()).unwrap();
                    (count..=count * 1024, Just(ids))
                })
                .prop_map(|(len, ids)| ResponseBody::ManifestChunks(
                    ManifestChunks::new(len, ids).unwrap()
                )),
            (bytes32(ChunkId::from_bytes), vec(any::<u8>(), 0..64))
                .prop_map(|(id, zstd)| ResponseBody::Chunk { id, zstd }),
            Just(ResponseBody::ChunksDone),
            fault().prop_map(ResponseBody::Fault),
        ]
    }

    proptest! {
        #[test]
        fn requests_round_trip(id in any::<u64>(), budget_ms in any::<u32>(), body in request_body()) {
            let request = Request { id: RequestId(id), budget_ms, body };
            let bytes = encode(&request).unwrap();
            prop_assert_eq!(decode::<Request>(&bytes).unwrap(), request);
        }

        #[test]
        fn responses_round_trip(id in any::<u64>(), body in response_body()) {
            let response = Response { id: RequestId(id), body };
            let bytes = encode(&response).unwrap();
            prop_assert_eq!(decode::<Response>(&bytes).unwrap(), response);
        }

        #[test]
        fn random_bytes_never_panic(bytes in vec(any::<u8>(), 0..512)) {
            let _ = decode::<Request>(&bytes);
            let _ = decode::<Response>(&bytes);
        }
    }

    #[test]
    fn a_chunk_batch_naming_an_id_twice_does_not_decode() {
        let (x, y) = (ChunkId::of(b"x"), ChunkId::of(b"y"));
        assert!(matches!(
            ChunkBatch::new(vec![x, y, x]),
            Err(WireError::DuplicateChunk(id)) if id == x
        ));
        assert!(decode::<ChunkBatch>(&encode(&vec![x, x]).unwrap()).is_err());
    }

    #[test]
    fn a_chunk_batch_beyond_the_cap_does_not_decode() {
        let ids: Vec<ChunkId> = (0..=MAX_CHUNK_BATCH)
            .map(|n| ChunkId::of(&n.to_le_bytes()))
            .collect();
        assert!(matches!(
            ChunkBatch::new(ids.clone()),
            Err(WireError::BatchSize(257))
        ));
        let bytes = encode(&ids).unwrap();
        assert!(decode::<ChunkBatch>(&bytes).is_err());
        assert!(matches!(
            ChunkBatch::new(Vec::new()),
            Err(WireError::BatchSize(0))
        ));
    }

    #[test]
    fn bounded_text_is_capped_on_decode_and_truncated_on_a_boundary() {
        let long = "e".repeat(MAX_FAULT_TEXT + 1);
        assert!(FaultText::new(long.clone()).is_err());
        assert!(decode::<FaultText>(&encode(&long).unwrap()).is_err());
        let multibyte = "\u{20ac}".repeat(MAX_FAULT_TEXT);
        let cut = FaultText::truncated(&multibyte);
        assert_eq!(cut.as_str().len(), MAX_FAULT_TEXT - 1);
        assert_eq!(cut.as_str().chars().count(), (MAX_FAULT_TEXT - 1) / 3);
    }

    fn page_of(records: Vec<RecordBytes>) -> RecordPage {
        let mut page = RecordPage::default();
        for record in records {
            page.push(record).unwrap();
        }
        page
    }

    /// The largest page, as one record of the ledger's largest size or as
    /// many small records, fits the control frame in an `Append` request and
    /// in a `Records` answer.
    #[test]
    fn a_full_page_of_records_fits_the_control_frame() {
        use crate::frame::MAX_CONTROL_FRAME;

        let largest = RecordBytes::new(vec![7; MAX_RECORD_LEN]).unwrap();
        assert_eq!(
            encode(&largest).unwrap().len(),
            largest.cost(),
            "the length prefix of the largest record is RECORD_PREFIX bytes"
        );
        let small = (0..MAX_RECORD_PAGE / (1 + RECORD_PREFIX))
            .map(|_| RecordBytes::new(vec![7]).unwrap())
            .collect();
        for page in [page_of(vec![largest]), page_of(small)] {
            let token = FencingToken {
                job_id: JobId::from_bytes([u8::MAX; 16]),
                epoch: Epoch::new(u64::MAX).unwrap(),
            };
            let append = Request {
                id: RequestId(u64::MAX),
                budget_ms: u32::MAX,
                body: RequestBody::Job {
                    token,
                    body: JobRequest::Append {
                        records: page.clone(),
                    },
                },
            };
            let answer = Response {
                id: RequestId(u64::MAX),
                body: ResponseBody::Job(JobResponse::Records {
                    records: page,
                    more: true,
                }),
            };
            for bytes in [encode(&append).unwrap(), encode(&answer).unwrap()] {
                // The frame counts its version byte too.
                assert!(bytes.len() < MAX_CONTROL_FRAME, "{} bytes", bytes.len());
            }
        }
    }

    /// A record above the ledger's bound, or a page above its budget, does
    /// not decode, and a full page refuses one more record.
    #[test]
    fn records_and_pages_are_bounded_on_decode() {
        assert!(matches!(
            RecordBytes::new(vec![0; MAX_RECORD_LEN + 1]),
            Err(WireError::RecordSize(_))
        ));
        assert!(decode::<RecordBytes>(&encode(&vec![0u8; MAX_RECORD_LEN + 1]).unwrap()).is_err());
        assert!(decode::<RecordBytes>(&encode(&Vec::<u8>::new()).unwrap()).is_err());

        let mut page = page_of(vec![RecordBytes::new(vec![1; MAX_RECORD_LEN]).unwrap()]);
        let one_more = RecordBytes::new(vec![2]).unwrap();
        assert_eq!(page.push(one_more.clone()), Err(one_more));
        let over: Vec<Vec<u8>> = vec![vec![1; MAX_RECORD_LEN], vec![2]];
        assert!(decode::<RecordPage>(&encode(&over).unwrap()).is_err());
    }

    /// A manifest above the cap a reader decodes, or a chunk list that
    /// cannot make the stated length, is refused before any chunk is fetched.
    #[test]
    fn a_manifest_outside_its_bounds_does_not_decode() {
        let max = u64::try_from(MAX_MANIFEST_LEN).unwrap();
        let per_chunk = u64::try_from(MAX_CHUNK_BYTES).unwrap();
        let ids =
            |n: u64| -> Vec<ChunkId> { (0..n).map(|i| ChunkId::of(&i.to_le_bytes())).collect() };
        let whole = max / per_chunk;
        assert!(ManifestChunks::new(max, ids(whole)).is_ok());
        let refused = [
            (max + 1, ids(whole + 1), "longer than the cap"),
            (0, ids(1), "empty"),
            (per_chunk + 1, ids(1), "a chunk above the chunk cap"),
            (2, ids(3), "a chunk of no bytes"),
        ];
        for (len, ids, why) in refused {
            #[derive(Serialize)]
            struct Unchecked {
                len: u64,
                ids: Vec<ChunkId>,
            }
            let bytes = encode(&Unchecked { len, ids }).unwrap();
            assert!(decode::<ManifestChunks>(&bytes).is_err(), "{why}");
        }
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = encode(&RequestBody::Ping).unwrap();
        assert_eq!(decode::<RequestBody>(&bytes).unwrap(), RequestBody::Ping);
        bytes.push(0);
        assert!(matches!(
            decode::<RequestBody>(&bytes),
            Err(WireError::Trailing(1))
        ));
    }
}
