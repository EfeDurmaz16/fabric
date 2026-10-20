//! The message vocabulary (ADR 0005), defined once.
//!
//! Invariant: every per-job message carries the fencing token, job id and
//! epoch, in one place, the envelope, so a receiver runs the ADR 0003 check
//! before it reads the body. Nothing of unbounded size is a control
//! message: ledger records are capped by the ledger, manifests travel as
//! chunks, and one chunk batch names at most `MAX_CHUNK_BATCH` ids. Every
//! bound is enforced when bytes are parsed, not after.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::id::{ChunkId, Epoch, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::lease::FencingToken;
use fabric_core::moves::MoveOffer;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

/// The protocol version both sides must speak; a mismatch ends the session at `Hello`.
pub const PROTOCOL_VERSION: u16 = 1;

/// The most chunk ids one `WantChunks` request may name.
pub const MAX_CHUNK_BATCH: usize = 256;

/// The longest fabric version string in a `Hello`.
pub const MAX_VERSION_LEN: usize = 64;

/// The longest text a `Fault` may carry.
pub const MAX_FAULT_TEXT: usize = 256;

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

impl Hello {
    /// Builds a `Hello` for this protocol version with the clock read now.
    #[must_use]
    pub fn now(node: NodeId, fabric_version: VersionText) -> Self {
        let clock_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        Self {
            node,
            protocol_version: PROTOCOL_VERSION,
            fabric_version,
            clock_ms,
        }
    }
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
    /// Asks for the records after `after`; answered with `Records`.
    Pull {
        /// The receiver's head, or the record it last saw.
        after: RecordHash,
    },
    /// Replicates records, each as its exact ledger bytes; answered with `Accepted`, `Rejected` or an epoch answer.
    Append {
        /// The records in chain order.
        records: Vec<Vec<u8>>,
    },
    /// Asks the owner to start a move toward the sender; answered with `MoveStarted` or `Rejected`.
    MoveRequest,
    /// The owner's offer after `MoveStarted` is durable; answered with `MoveDecision`, `Rejected`, `SnapshotIncomplete` or an epoch answer.
    MoveOffer(MoveOffer),
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

/// One response: the request number it answers and the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The request number.
    pub id: RequestId,
    /// The answer.
    pub body: ResponseBody,
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
    ManifestChunks {
        /// The manifest's length; refused above the manifest cap before any chunk is fetched.
        len: u64,
        /// The chunks that make the manifest, in order.
        ids: Vec<ChunkId>,
    },
    /// One chunk; the receiver trusts it only after hashing the bytes.
    Chunk {
        /// The chunk.
        id: ChunkId,
        /// Its uncompressed bytes.
        bytes: Vec<u8>,
    },
    /// Every requested chunk was sent.
    ChunksDone,
    /// The request was understood but could not be served.
    Fault(Fault),
}

/// The answer to a per-job message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobResponse {
    /// The records after the asked head, in chain order.
    Records(Vec<Vec<u8>>),
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
        record: Vec<u8>,
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
    use fabric_core::id::JobId;
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

    fn records() -> impl Strategy<Value = Vec<Vec<u8>>> {
        vec(vec(any::<u8>(), 0..64), 0..4)
    }

    fn job_request() -> impl Strategy<Value = JobRequest> {
        prop_oneof![
            bytes32(RecordHash::from_bytes).prop_map(|after| JobRequest::Pull { after }),
            records().prop_map(|records| JobRequest::Append { records }),
            Just(JobRequest::MoveRequest),
            (
                bytes16(JobId::from_bytes),
                bytes16(MoveId::from_bytes),
                bytes32(RecordHash::from_bytes),
                bytes32(SnapshotId::from_bytes),
            )
                .prop_map(|(job_id, move_id, head, snapshot)| JobRequest::MoveOffer(
                    MoveOffer {
                        job_id,
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
            records().prop_map(JobResponse::Records),
            bytes32(RecordHash::from_bytes).prop_map(|head| JobResponse::Accepted { head }),
            bytes32(RecordHash::from_bytes).prop_map(|head| JobResponse::Rejected { head }),
            (epoch(), epoch())
                .prop_map(|(yours, current)| JobResponse::StaleEpoch { yours, current }),
            (epoch(), epoch())
                .prop_map(|(yours, current)| JobResponse::AheadOfUs { yours, current }),
            bytes16(MoveId::from_bytes).prop_map(|move_id| JobResponse::MoveStarted { move_id }),
            vec(any::<u8>(), 0..64).prop_map(|record| JobResponse::MoveDecision { record }),
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
            (any::<u64>(), vec(bytes32(ChunkId::from_bytes), 0..8))
                .prop_map(|(len, ids)| ResponseBody::ManifestChunks { len, ids }),
            (bytes32(ChunkId::from_bytes), vec(any::<u8>(), 0..64))
                .prop_map(|(id, bytes)| ResponseBody::Chunk { id, bytes }),
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
