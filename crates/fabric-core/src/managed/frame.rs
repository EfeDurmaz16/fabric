//! The byte forms of managed values: request digests, stored event frames
//! and stored answers.
//!
//! Invariant: every form is a format byte followed by the canonical
//! postcard bytes of ADR 0002 (`crate::codec`): decoding refuses another
//! format byte, trailing bytes and a non-canonical encoding, so a digest
//! of the bytes is a digest of the value. A frame names the digest of the
//! job's previous frame, so a job's history is a hash chain.

use serde::{Deserialize, Serialize};

use super::ids::{EventDigest, OperationId, RequestDigest, WorkRevision};
use super::model::{Command, Envelope, ManagedEvent, StoredResult};
use crate::codec::{self, CodecError};
use crate::id::JobId;

/// The format byte of every managed frame and stored answer this crate writes.
pub const MANAGED_FORMAT: u8 = 1;

/// The domain separator of a request digest.
const COMMAND_DOMAIN: &[u8] = b"fabric.managed.command.v1\0";

/// Why managed bytes did not decode.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// No bytes at all.
    #[error("an empty frame")]
    Empty,
    /// A format byte this crate does not read.
    #[error("managed format {0} is not supported")]
    UnknownFormat(u8),
    /// The bytes after the format byte are not a canonical value.
    #[error(transparent)]
    Codec(#[from] CodecError),
}

/// The digest of a command: blake3 over the domain separator and the
/// canonical bytes of the job, the expected revision and the command. The
/// key is not part of it: it is what the digest is compared under.
pub fn request_digest(envelope: &Envelope<Command>) -> Result<RequestDigest, CodecError> {
    let body = codec::encode(&(envelope.job, envelope.expected, &envelope.body))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(COMMAND_DOMAIN);
    hasher.update(&body);
    Ok(RequestDigest::from_bytes(*hasher.finalize().as_bytes()))
}

/// One stored event: the event with where it sits in its job's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFrame {
    /// The job.
    pub job: JobId,
    /// The revision of the batch it belongs to.
    pub revision: WorkRevision,
    /// The operation whose command made it; `None` for an observation's events.
    pub operation: Option<OperationId>,
    /// The digest of the job's previous frame; `None` for its first.
    pub prev: Option<EventDigest>,
    /// Unix milliseconds when it was committed; informational.
    pub at_ms: u64,
    /// The event.
    pub event: ManagedEvent,
}

fn encode_framed<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut bytes = vec![MANAGED_FORMAT];
    bytes.extend(codec::encode(value)?);
    Ok(bytes)
}

fn decode_framed<T: Serialize + serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, FrameError> {
    let (&format, rest) = bytes.split_first().ok_or(FrameError::Empty)?;
    if format != MANAGED_FORMAT {
        return Err(FrameError::UnknownFormat(format));
    }
    Ok(codec::decode(rest)?)
}

impl EventFrame {
    /// The frame's stored bytes.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_framed(self)
    }

    /// Reads stored bytes back, refusing any but the canonical form.
    pub fn decode(bytes: &[u8]) -> Result<EventFrame, FrameError> {
        decode_framed(bytes)
    }

    /// The digest of a frame's stored bytes.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> EventDigest {
        EventDigest::from_bytes(*blake3::hash(bytes).as_bytes())
    }
}

impl StoredResult {
    /// The answer's stored bytes: what a retry under its key gets back.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_framed(self)
    }

    /// Reads stored bytes back, refusing any but the canonical form.
    pub fn decode(bytes: &[u8]) -> Result<StoredResult, FrameError> {
        decode_framed(bytes)
    }
}
