//! The framing of the client socket: a `u32` little-endian length, then
//! that many bytes of one UTF-8 JSON object (ADR 0018).
//!
//! Invariant: a reader checks the length against [`MAX_FRAME`] before it
//! allocates anything, and a frame holds exactly one value with nothing
//! after it, whose every object is a JSON object with named fields, never
//! an array in field order. Both directions use the same functions, so the
//! node and every Rust client agree on the bytes.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame either side sends or reads: 16 MiB.
pub const MAX_FRAME: usize = 16 << 20;

/// Why a frame could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The socket failed.
    #[error("control socket: {0}")]
    Io(#[from] std::io::Error),
    /// A frame declares, or would need, more bytes than [`MAX_FRAME`].
    #[error("control message of {0} bytes exceeds the cap of {MAX_FRAME}")]
    TooLarge(usize),
    /// A frame declares no bytes, which holds no JSON object.
    #[error("control message is empty")]
    Empty,
    /// A frame is not the JSON value expected here.
    #[error("control message does not parse: {0}")]
    Malformed(#[from] serde_json::Error),
}

/// The bytes of `value` as one frame: its length, then its JSON.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let json = serde_json::to_vec(value)?;
    let len = u32::try_from(json.len())
        .ok()
        .filter(|len| *len as usize <= MAX_FRAME)
        .ok_or(FrameError::TooLarge(json.len()))?;
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&json);
    Ok(frame)
}

/// Writes `value` as one frame and flushes it.
pub async fn write<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    writer.write_all(&encode(value)?).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads the bytes of one frame; `None` when the peer closed the connection before a frame began.
pub async fn read_bytes<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut header = [0; 4];
    let mut filled = 0;
    while filled < header.len() {
        match reader.read(&mut header[filled..]).await? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
            n => filled += n,
        }
    }
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// Reads one frame as a `T`; `None` when the peer closed the connection before a frame began.
pub async fn read<R, T>(reader: &mut R) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: Serialize + DeserializeOwned,
{
    match read_bytes(reader).await? {
        Some(bytes) => Ok(Some(parse(&bytes)?)),
        None => Ok(None),
    }
}

/// Parses the JSON of one frame as a `T`, refusing a JSON array where `T`
/// holds an object, at any depth.
///
/// serde's derived structs and tagged enums also read an array as their
/// fields in declaration order, so `[1, "x"]` would read as a hello and
/// `["job_status", "<job>"]` as a request, tying such a client to the Rust
/// field order that ADR 0018 says nobody depends on. The TypeScript SDK
/// refuses the same bytes. The check writes the value back and requires
/// every array the frame held to still be an array there; an object field
/// the type does not know is skipped, as the parse skipped it.
pub fn parse<T: Serialize + DeserializeOwned>(json: &[u8]) -> Result<T, serde_json::Error> {
    let frame: Value = serde_json::from_slice(json)?;
    let value = T::deserialize(&frame)?;
    if positional(&frame, &serde_json::to_value(&value)?) {
        return Err(serde::de::Error::custom(
            "a JSON array stands where the message has an object with named fields",
        ));
    }
    Ok(value)
}

/// Whether `frame` holds an array where `read`, the same value as the type wrote it back, holds none.
fn positional(frame: &Value, read: &Value) -> bool {
    match (frame, read) {
        (Value::Array(frame), Value::Array(read)) => {
            frame.iter().zip(read).any(|(a, b)| positional(a, b))
        }
        (Value::Array(_), _) => true,
        (Value::Object(frame), Value::Object(read)) => frame
            .iter()
            .any(|(key, a)| read.get(key).is_some_and(|b| positional(a, b))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A length past the cap is refused from the header alone, before the
    /// reader allocates or waits for a body that may never come.
    #[tokio::test]
    async fn a_frame_over_the_cap_is_refused_from_its_header() {
        let header = u32::try_from(MAX_FRAME + 1).unwrap().to_le_bytes();
        let mut reader: &[u8] = &header;
        assert!(matches!(
            read_bytes(&mut reader).await,
            Err(FrameError::TooLarge(len)) if len == MAX_FRAME + 1
        ));
    }

    #[tokio::test]
    async fn a_connection_closed_between_frames_is_not_an_error_and_one_closed_inside_a_header_is()
    {
        let mut empty: &[u8] = &[];
        assert!(read_bytes(&mut empty).await.unwrap().is_none());
        let mut torn: &[u8] = &[5, 0];
        assert!(matches!(
            read_bytes(&mut torn).await,
            Err(FrameError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }
}
