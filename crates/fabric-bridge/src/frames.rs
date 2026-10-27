//! Bridge frames: Fabric's client framing (ADR 0018), a `u32`
//! little-endian length and one UTF-8 JSON object, read under the cap the
//! protocol gives each direction and phase (docs/bridge-protocol.md,
//! Framing).
//!
//! Invariant: the declared length is checked against the cap before the
//! body is allocated or read, and a body is one JSON object with nothing
//! after it, not even whitespace. Each refusal names the protocol's code:
//! `empty`, `truncated`, `malformed` or `oversize`.

use std::io;

use fabric_client::frame::{self, FrameError};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// Why a frame was refused.
#[derive(Debug, thiserror::Error)]
pub enum FrameFault {
    /// Length 0.
    #[error("empty frame")]
    Empty,
    /// The stream ended inside the length or the body.
    #[error("truncated frame")]
    Truncated,
    /// The body is not UTF-8, not one JSON object, or has bytes after it.
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// The declared length is above the cap.
    #[error("frame of {len} bytes exceeds the cap of {cap}")]
    Oversize {
        /// The declared length.
        len: usize,
        /// The cap.
        cap: usize,
    },
    /// The stream failed.
    #[error("stream: {0}")]
    Io(io::Error),
}

impl FrameFault {
    /// The protocol's code for it; `io` for a failed stream.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            FrameFault::Empty => "empty",
            FrameFault::Truncated => "truncated",
            FrameFault::Malformed(_) => "malformed",
            FrameFault::Oversize { .. } => "oversize",
            FrameFault::Io(_) => "io",
        }
    }
}

/// Reads one frame of at most `cap` bytes as a JSON object; `None` when
/// the stream ended cleanly between frames.
pub async fn read<R: AsyncRead + Unpin>(
    reader: &mut R,
    cap: usize,
) -> Result<Option<Value>, FrameFault> {
    match frame::read_bytes_within(reader, cap).await {
        Ok(Some(body)) => decode(&body).map(Some),
        Ok(None) => Ok(None),
        Err(FrameError::Empty) => Err(FrameFault::Empty),
        Err(FrameError::OverCap { len, cap }) => Err(FrameFault::Oversize { len, cap }),
        Err(FrameError::TooLarge(len)) => Err(FrameFault::Oversize { len, cap }),
        Err(FrameError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
            Err(FrameFault::Truncated)
        }
        Err(FrameError::Io(e)) => Err(FrameFault::Io(e)),
        Err(FrameError::Malformed(e)) => Err(FrameFault::Malformed(e.to_string())),
    }
}

/// One JSON object from a frame body, with nothing after it.
pub fn decode(body: &[u8]) -> Result<Value, FrameFault> {
    let text = std::str::from_utf8(body).map_err(|e| FrameFault::Malformed(e.to_string()))?;
    let mut values = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    let value = match values.next() {
        Some(Ok(value)) => value,
        Some(Err(e)) => return Err(FrameFault::Malformed(e.to_string())),
        None => return Err(FrameFault::Malformed("no JSON value".into())),
    };
    if values.byte_offset() != text.len() {
        return Err(FrameFault::Malformed("bytes after the JSON object".into()));
    }
    if !value.is_object() {
        return Err(FrameFault::Malformed("not a JSON object".into()));
    }
    Ok(value)
}

/// The bytes of `value` as one frame, refused when the body is over `cap`.
pub fn encode(value: &Value, cap: usize) -> Result<Vec<u8>, FrameFault> {
    let bytes = frame::encode(value).map_err(|e| match e {
        FrameError::TooLarge(len) => FrameFault::Oversize {
            len,
            cap: cap.min(frame::MAX_FRAME),
        },
        other => FrameFault::Malformed(other.to_string()),
    })?;
    let len = bytes.len() - 4;
    if len > cap {
        return Err(FrameFault::Oversize { len, cap });
    }
    Ok(bytes)
}

/// Writes one frame and flushes it.
pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> io::Result<()> {
    writer.write_all(frame).await?;
    writer.flush().await
}
