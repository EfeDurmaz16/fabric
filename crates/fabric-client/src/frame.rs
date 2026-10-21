//! The framing of the client socket: a `u32` little-endian length, then
//! that many bytes of one UTF-8 JSON object (ADR 0018).
//!
//! Invariant: a reader checks the length against [`MAX_FRAME`] before it
//! allocates anything, and a frame holds exactly one value with nothing
//! after it, whose every object is a JSON object with named fields, never
//! an array in field order, whose every variant is named, never numbered,
//! and whose every integer a JavaScript number holds exactly. Both
//! directions use the same functions, so the node and every Rust client
//! agree on the bytes, and the TypeScript SDK refuses what they refuse.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame either side sends or reads: 16 MiB.
pub const MAX_FRAME: usize = 16 << 20;

/// The largest integer a frame carries, `2^53 - 1`, and the negation of the
/// smallest: the range a JavaScript number holds exactly (ADR 0018,
/// Encoding rules).
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

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

/// Parses the JSON of one frame as a `T`, refusing, at any depth, a JSON
/// array where `T` holds an object, a number where it holds a name, and an
/// integer beyond [`MAX_SAFE_INTEGER`] either way.
///
/// serde's derived structs and tagged enums also read an array as their
/// fields in declaration order, so `[1, "x"]` would read as a hello and
/// `["job_status", "<job>"]` as a request, and an enum nested in a message
/// reads an integer tag as its variant's index, so `{"kind": 0}` would
/// read as the first harness. Either ties such a client to the Rust
/// declaration order that ADR 0018 says nobody depends on. An integer past
/// `2^53 - 1` fits a Rust `u64` but not a JavaScript number. The TypeScript
/// SDK refuses all three. The check writes the value back and requires
/// every array the frame held to still be an array there, every number to
/// still be a number, and every number to be in range; an object field the
/// type does not know is skipped, as the parse skipped it.
pub fn parse<T: Serialize + DeserializeOwned>(json: &[u8]) -> Result<T, serde_json::Error> {
    let frame: Value = serde_json::from_slice(json)?;
    let value = T::deserialize(&frame)?;
    if let Some(problem) = unlike(&frame, &serde_json::to_value(&value)?) {
        return Err(serde::de::Error::custom(problem));
    }
    Ok(value)
}

/// What in `frame` the type does not read as written: `read` is the same
/// value as the type wrote it back.
fn unlike(frame: &Value, read: &Value) -> Option<&'static str> {
    match (frame, read) {
        (Value::Array(frame), Value::Array(read)) => {
            frame.iter().zip(read).find_map(|(a, b)| unlike(a, b))
        }
        (Value::Array(_), _) => {
            Some("a JSON array stands where the message has an object with named fields")
        }
        (Value::Object(frame), Value::Object(read)) => frame
            .iter()
            .find_map(|(key, a)| read.get(key).and_then(|b| unlike(a, b))),
        (Value::Number(number), Value::Number(_)) => (!safe(number)).then_some(
            "an integer stands beyond 2^53 - 1 or below its negation, which a JavaScript number does not hold exactly",
        ),
        (Value::Number(_), _) => Some(
            "a number stands where the message has a name, such as a variant written as its index",
        ),
        _ => None,
    }
}

/// Whether `number` is an integer a JavaScript number holds exactly, or not an integer at all.
fn safe(number: &serde_json::Number) -> bool {
    if let Some(n) = number.as_u64() {
        n <= MAX_SAFE_INTEGER
    } else if let Some(n) = number.as_i64() {
        n.unsigned_abs() <= MAX_SAFE_INTEGER
    } else {
        true
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
