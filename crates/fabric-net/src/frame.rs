//! Length-prefixed frames (ADR 0005).
//!
//! Invariant: a reader accepts a frame only if its declared length passed
//! the cap the reader chose and its version byte is one this crate reads,
//! both checked before any of the payload is read, and grows the frame's
//! buffer only as its bytes arrive, never past the declared length. A peer
//! cannot make this node hold more than one frame per stream, and a header
//! alone, four bytes that declare 8 MiB, costs this node no more than
//! `INITIAL_CAPACITY`. The one exception is a reader that already counts
//! the whole payload against a cap of its own (`Room::Held`), as a served
//! request does against its peer's request bytes (`rpc`): it takes the
//! whole buffer at once. A frame is `u32 length | u8 version | payload`,
//! little endian, with `length` counting the version byte and the payload.

use std::io;

use fabric_core::snapshot::MAX_CHUNK_LEN;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The version of the frame layout after the length prefix, which this crate writes.
///
/// It cannot version the length prefix: a reader parses the length before
/// it sees this byte, so a new length format needs a new ALPN.
pub const FRAME_VERSION: u8 = 1;

/// Room in a frame for everything around its one long field, a page of
/// records, a manifest's chunk ids or a stored chunk: the version byte, the
/// request or response envelope and the field's length.
pub(crate) const ENVELOPE_ALLOWANCE: usize = 1024;

/// The most bytes a control frame may carry: a page of ledger records, move messages, chunk id lists.
///
/// It is derived from the record page so that the largest record, which
/// is as long as the ledger allows, always fits with its envelope.
pub const MAX_CONTROL_FRAME: usize = crate::wire::MAX_RECORD_PAGE + ENVELOPE_ALLOWANCE;

/// The most bytes one stored chunk may take: zstd's worst case for `MAX_CHUNK_LEN`.
pub const MAX_STORED_CHUNK: usize = MAX_CHUNK_LEN + (MAX_CHUNK_LEN >> 8);

/// The most bytes a bulk frame may carry: one stored chunk plus its envelope.
pub const MAX_BULK_FRAME: usize = MAX_STORED_CHUNK + ENVELOPE_ALLOWANCE;

const LENGTH_LEN: usize = 4;

/// The buffer a reader starts a frame with; it grows as the frame's bytes arrive.
const INITIAL_CAPACITY: usize = 64 << 10;

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
///
/// The header and the payload are written one after the other, so a chunk's
/// payload is never copied into a second buffer while it waits on flow control.
pub(crate) async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    payload: &[u8],
    max: usize,
) -> Result<(), FrameError> {
    let len = payload.len() + 1;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let length = u32::try_from(len).map_err(|_| FrameError::TooLarge { len, max })?;
    let mut header = [0u8; LENGTH_LEN + 1];
    header[..LENGTH_LEN].copy_from_slice(&length.to_le_bytes());
    header[LENGTH_LEN] = FRAME_VERSION;
    writer.write_all(&header).await.map_err(FrameError::Io)?;
    writer.write_all(payload).await.map_err(FrameError::Io)?;
    writer.flush().await.map_err(FrameError::Io)
}

/// Reads one frame's payload, refusing a declared length above `max` before allocating.
pub(crate) async fn read_frame<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    max: usize,
) -> Result<Vec<u8>, FrameError> {
    let len = read_header(reader, max).await?;
    read_payload(reader, len, Room::Grow).await
}

/// Reads one frame's length and version byte, refusing a declared length
/// above `max` or a version this crate does not read before any of the
/// payload is read, and returns the payload's length.
pub(crate) async fn read_header<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    max: usize,
) -> Result<usize, FrameError> {
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
    let mut version = [0u8; 1];
    if reader.read(&mut version).await.map_err(FrameError::Io)? == 0 {
        return Err(FrameError::Truncated);
    }
    if version[0] != FRAME_VERSION {
        return Err(FrameError::BadVersion(version[0]));
    }
    Ok(len - 1)
}

/// How a payload's buffer is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Room {
    /// At its whole length at once: the caller already counts that length
    /// against a cap of its own, as a served request does (`rpc`).
    Held,
    /// As its bytes arrive, from `INITIAL_CAPACITY` and doubling, never
    /// past the declared length, so a header alone costs no more than
    /// `INITIAL_CAPACITY`.
    Grow,
}

/// Reads a payload of `len` bytes, the length `read_header` returned,
/// into a buffer that never holds more room than the payload needs.
pub(crate) async fn read_payload<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
    len: usize,
    room: Room,
) -> Result<Vec<u8>, FrameError> {
    let mut body = Vec::with_capacity(match room {
        Room::Held => len,
        Room::Grow => len.min(INITIAL_CAPACITY),
    });
    let declared = u64::try_from(len).map_err(|_| FrameError::TooLarge {
        len,
        max: usize::MAX,
    })?;
    let mut payload = reader.take(declared);
    while body.len() < len {
        if body.len() == body.capacity() {
            body.reserve_exact(body.capacity().clamp(1, len - body.len()));
        }
        if payload.read_buf(&mut body).await.map_err(FrameError::Io)? == 0 {
            return Err(FrameError::Truncated);
        }
    }
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

    /// A frame of a version this crate does not read is refused as soon as
    /// its version byte arrives, before any of its payload.
    #[tokio::test]
    async fn a_frame_of_another_version_is_refused_before_its_payload_arrives() {
        let (mut peer, mut here) = tokio::io::duplex(64);
        peer.write_all(&[200, 0, 0, 0, 9]).await.unwrap();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_frame(&mut here, 1024),
        )
        .await
        .expect("the reader waited for a payload it refuses");
        assert!(matches!(read, Err(FrameError::BadVersion(9))));
    }

    /// A payload's buffer never has more room than the payload: grown as
    /// the bytes arrive, it stops at the declared length instead of
    /// doubling past it, and room the caller holds is taken at once. The
    /// payload is read after its version byte, so it never moves in its
    /// buffer.
    #[tokio::test]
    async fn a_payload_buffer_is_never_larger_than_the_payload() {
        for len in [
            0,
            1,
            31,
            32,
            33,
            INITIAL_CAPACITY - 1,
            INITIAL_CAPACITY,
            INITIAL_CAPACITY + 1,
            (1 << 20) + 70,
        ] {
            let payload: Vec<u8> = (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect();
            let mut wire = Vec::new();
            write_frame(&mut wire, &payload, MAX_CONTROL_FRAME)
                .await
                .unwrap();
            let body = read_frame(&mut wire.as_slice(), MAX_CONTROL_FRAME)
                .await
                .unwrap();
            assert_eq!(body, payload);
            assert_eq!(body.capacity(), len, "grown");
            let mut cursor = wire.as_slice();
            let declared = read_header(&mut cursor, MAX_CONTROL_FRAME).await.unwrap();
            assert_eq!(declared, len);
            let held = read_payload(&mut cursor, declared, Room::Held)
                .await
                .unwrap();
            assert_eq!(held, payload);
            assert_eq!(held.capacity(), len, "held");
        }
    }

    /// A chunk that does not compress still fits a bulk frame as zstd stores it.
    #[test]
    fn the_stored_chunk_bound_is_zstds_worst_case() {
        assert_eq!(
            zstd::zstd_safe::compress_bound(MAX_CHUNK_LEN),
            MAX_STORED_CHUNK
        );
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
