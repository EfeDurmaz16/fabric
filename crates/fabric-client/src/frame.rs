//! The framing of the client socket: a `u32` little-endian length, then
//! that many bytes of one UTF-8 JSON object (ADR 0018).
//!
//! Invariant: a reader checks the length against [`MAX_FRAME`] before it
//! allocates anything, and a frame holds exactly one value with nothing
//! after it, whose every object is a JSON object with named fields, never
//! an array in field order, whose every variant is named as a string,
//! never numbered or written as a map, and whose every integer a
//! JavaScript number holds exactly. Both directions use the same
//! functions, so the node and every Rust client agree on the bytes, and
//! the TypeScript SDK refuses what they refuse.

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
///
/// An integer beyond [`MAX_SAFE_INTEGER`], or below its negation, is
/// written as that bound, so a writer never sends a frame [`parse`] or the
/// TypeScript SDK refuses. Only a value a peer reported can be that large,
/// such as a ledger record's time from a broken or forged clock, which a
/// node relays in `job_log` and a steal's warning, or a peer's clock in
/// `node_ping`; refusing the frame instead would fail every later read of
/// that job (review finding on PR #13).
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let mut json = serde_json::to_vec(value)?;
    if has_long_integer(&json) {
        let mut tree = serde_json::to_value(value)?;
        bound(&mut tree);
        json = serde_json::to_vec(&tree)?;
    }
    let len = u32::try_from(json.len())
        .ok()
        .filter(|len| *len as usize <= MAX_FRAME)
        .ok_or(FrameError::TooLarge(json.len()))?;
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&json);
    Ok(frame)
}

/// Whether `json` holds 16 digits in a row outside a string, the least
/// an integer beyond [`MAX_SAFE_INTEGER`], itself 16 digits long, needs.
/// A cheap pass, so the common frame is written as serde wrote it.
fn has_long_integer(json: &[u8]) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut digits = 0;
    for &byte in json {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else if byte.is_ascii_digit() {
            digits += 1;
            if digits >= 16 {
                return true;
            }
        } else {
            digits = 0;
            in_string = byte == b'"';
        }
    }
    false
}

/// Writes every integer in `value` beyond [`MAX_SAFE_INTEGER`], or below
/// its negation, as that bound.
fn bound(value: &mut Value) {
    let max = i64::try_from(MAX_SAFE_INTEGER).unwrap_or(i64::MAX);
    match value {
        Value::Number(number) => {
            if number.as_u64().is_some_and(|n| n > MAX_SAFE_INTEGER) {
                *number = MAX_SAFE_INTEGER.into();
            } else if number.as_i64().is_some_and(|n| n < -max) {
                *number = (-max).into();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(bound),
        Value::Object(fields) => fields.values_mut().for_each(bound),
        _ => {}
    }
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
/// array where `T` holds an object, a number or an object where it holds a
/// name, any other value in a form `T` does not write, and an integer
/// beyond [`MAX_SAFE_INTEGER`] either way.
///
/// serde's derived structs and tagged enums also read an array as their
/// fields in declaration order, so `[1, "x"]` would read as a hello and
/// `["job_status", "<job>"]` as a request, an enum nested in a message
/// reads an integer tag as its variant's index, so `{"kind": 0}` would
/// read as the first harness, and a variant without data also reads from a
/// map of its name to null, so `{"approve": null}` would read as the
/// decision `"approve"`. Each is a second encoding of the same message,
/// and the first two tie such a client to the Rust declaration order that
/// ADR 0018 says nobody depends on. An integer past `2^53 - 1` fits a Rust
/// `u64` but not a JavaScript number. The TypeScript SDK refuses all of
/// them. The check writes the value back and requires every value the
/// frame held to have the JSON type it has there, null aside, which an
/// optional field may be written as, and every number to be in range; an
/// object field the type does not know is skipped, as the parse skipped it.
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
        (Value::Object(_), Value::String(_)) => Some(
            "an object stands where the message has a name, such as a variant written as a map from its name to null",
        ),
        // An optional field written as null reads as left out.
        (Value::Null, _) => None,
        (frame, read) => (std::mem::discriminant(frame) != std::mem::discriminant(read))
            .then_some("a value stands in a form other than the one the message writes there"),
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

    /// A record's time and a peer's clock come from another node, so a
    /// node can hold one past `2^53 - 1`. It writes the bound instead, and
    /// the frame still reads, here and in the TypeScript SDK; small
    /// numbers, and digits inside a string, are written as they are
    /// (review finding on PR #13).
    #[tokio::test]
    async fn a_writer_bounds_an_integer_no_reader_takes() {
        use crate::api::{LedgerEvent, LogRecord, Pong, Reply, Wire};

        let max = i64::try_from(MAX_SAFE_INTEGER).unwrap();
        for (at, written) in [
            (u64::MAX, MAX_SAFE_INTEGER),
            (MAX_SAFE_INTEGER + 1, MAX_SAFE_INTEGER),
            (MAX_SAFE_INTEGER, MAX_SAFE_INTEGER),
            (1_760_000_000_000, 1_760_000_000_000),
        ] {
            let log = Reply::JobLog(crate::api::JobLog {
                left_out: 0,
                records: vec![LogRecord {
                    id: "01JC0000000000000000000000".parse().unwrap(),
                    epoch: fabric_core::id::Epoch::FIRST,
                    at,
                    event: LedgerEvent::Note {
                        text: "12345678901234567890".to_owned(),
                    },
                }],
            });
            let mut reader: &[u8] = &encode(&log).unwrap();
            let Some(Reply::JobLog(log)) = read(&mut reader).await.unwrap() else {
                panic!("not a job log");
            };
            assert_eq!(log.records[0].at, written);
            assert_eq!(
                log.records[0].event,
                LedgerEvent::Note {
                    text: "12345678901234567890".to_owned()
                }
            );
        }

        for (skew, written) in [(i64::MIN, -max), (i64::MAX, max), (-340, -340)] {
            let pong = Reply::NodePing(Pong {
                node: "fab1aibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaiba"
                    .parse()
                    .unwrap(),
                rtt_ms: 12,
                wire: Wire::Quic,
                clock_skew_ms: skew,
            });
            let mut reader: &[u8] = &encode(&pong).unwrap();
            let Some(Reply::NodePing(pong)) = read(&mut reader).await.unwrap() else {
                panic!("not a pong");
            };
            assert_eq!(pong.clock_skew_ms, written);
        }
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
