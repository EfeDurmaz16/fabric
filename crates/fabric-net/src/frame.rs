//! Length-prefixed frames (ADR 0005).
//!
//! Invariant: a reader allocates for a frame only after its declared length
//! passed the cap the reader chose, so a peer cannot make this node hold
//! more than one frame per stream in memory. A frame is
//! `u32 length | u8 version | payload`, little endian, with `length`
//! counting the version byte and the payload.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The frame format version this crate writes.
pub const FRAME_VERSION: u8 = 1;

/// The most bytes a control frame may carry: ledger records, move messages, chunk id lists.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;

/// The most bytes one chunk may hold (ADR 0004: whole-file objects up to 8 MiB).
pub const MAX_CHUNK_BYTES: usize = 8 << 20;

/// The most bytes a bulk frame may carry: one chunk plus its envelope.
pub const MAX_BULK_FRAME: usize = MAX_CHUNK_BYTES + 1024;

const LENGTH_LEN: usize = 4;

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The frame declares more bytes than the reader or writer allows.
    #[error("frame of {len} bytes exceeds the cap of {max}")]
    TooLarge {
        /// The declared length.
        len: usize,
        /// The cap in force.
        max: usize,
    },
    /// The frame declares fewer bytes than its one-byte header.
    #[error("frame length {0} is below the one-byte header")]
    TooShort(u32),
    /// The frame's version byte is not one this crate reads.
    #[error("frame version {0} is not supported")]
    BadVersion(u8),
    /// The stream ended cleanly before a frame started.
    #[error("the stream ended")]
    Eof,
    /// The stream ended inside a frame.
    #[error("the stream ended inside a frame")]
    Truncated,
    /// The stream failed.
    #[error("io: {0}")]
    Io(#[source] io::Error),
}

/// Writes one frame, refusing a payload that would exceed `max`.
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    payload: &[u8],
    max: usize,
) -> Result<(), FrameError> {
    let len = payload.len() + 1;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let length = u32::try_from(len).map_err(|_| FrameError::TooLarge { len, max })?;
    let mut frame = Vec::with_capacity(LENGTH_LEN + len);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.push(FRAME_VERSION);
    frame.extend_from_slice(payload);
    writer.write_all(&frame).await.map_err(FrameError::Io)?;
    writer.flush().await.map_err(FrameError::Io)
}

/// Reads one frame's payload, refusing a declared length above `max` before allocating.
pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    max: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; LENGTH_LEN];
    let mut filled = 0;
    while filled < LENGTH_LEN {
        let n = reader
            .read(&mut header[filled..])
            .await
            .map_err(FrameError::Io)?;
        if n == 0 {
            return Err(if filled == 0 {
                FrameError::Eof
            } else {
                FrameError::Truncated
            });
        }
        filled += n;
    }
    let length = u32::from_le_bytes(header);
    let len = usize::try_from(length).map_err(|_| FrameError::TooLarge {
        len: usize::MAX,
        max,
    })?;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    if len == 0 {
        return Err(FrameError::TooShort(length));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.map_err(|err| {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            FrameError::Truncated
        } else {
            FrameError::Io(err)
        }
    })?;
    let version = body[0];
    if version != FRAME_VERSION {
        return Err(FrameError::BadVersion(version));
    }
    body.drain(..1);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[tokio::test]
    async fn a_declared_length_above_the_cap_is_refused_before_the_body_is_read() {
        let mut wire = Vec::new();
        let over = u32::try_from(MAX_CONTROL_FRAME + 1).unwrap();
        wire.extend_from_slice(&over.to_le_bytes());
        wire.push(FRAME_VERSION);
        let mut cursor = wire.as_slice();
        assert!(matches!(
            read_frame(&mut cursor, MAX_CONTROL_FRAME).await,
            Err(FrameError::TooLarge {
                max: MAX_CONTROL_FRAME,
                ..
            })
        ));
        let mut sink = Vec::new();
        assert!(matches!(
            write_frame(&mut sink, &vec![0u8; MAX_CONTROL_FRAME], MAX_CONTROL_FRAME).await,
            Err(FrameError::TooLarge { .. })
        ));
        assert!(sink.is_empty());
    }

    #[tokio::test]
    async fn an_ended_stream_is_eof_before_a_frame_and_truncated_inside_one() {
        let mut empty: &[u8] = &[];
        assert!(matches!(
            read_frame(&mut empty, 16).await,
            Err(FrameError::Eof)
        ));
        let mut partial: &[u8] = &[5, 0, 0, 0, FRAME_VERSION, 1];
        assert!(matches!(
            read_frame(&mut partial, 16).await,
            Err(FrameError::Truncated)
        ));
        let mut zero: &[u8] = &[0, 0, 0, 0];
        assert!(matches!(
            read_frame(&mut zero, 16).await,
            Err(FrameError::TooShort(0))
        ));
        let mut other: &[u8] = &[1, 0, 0, 0, 9];
        assert!(matches!(
            read_frame(&mut other, 16).await,
            Err(FrameError::BadVersion(9))
        ));
    }

    proptest! {
        #[test]
        fn frames_round_trip(payload in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            rt.block_on(async {
                let mut wire = Vec::new();
                write_frame(&mut wire, &payload, MAX_CONTROL_FRAME).await.unwrap();
                prop_assert_eq!(wire.len(), LENGTH_LEN + 1 + payload.len());
                let mut cursor = wire.as_slice();
                let back = read_frame(&mut cursor, MAX_CONTROL_FRAME).await.unwrap();
                prop_assert_eq!(back, payload);
                prop_assert!(cursor.is_empty());
                Ok(())
            })?;
        }
    }
}
