//! The one place the crate talks to postcard.
//!
//! Invariant: every value this crate persists or sends has exactly one byte
//! form. `decode` refuses bytes that do not end where the value ends and
//! bytes that are not what `encode` writes for the value they decode to, so a
//! hash of the bytes is a hash of the value and nothing else, and no public
//! error type names the codec crate.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Why bytes did not decode as a value, or a value did not encode.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The bytes are not an encoding of the type, or the value cannot be encoded.
    #[error("malformed encoding: {0}")]
    Malformed(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
    /// Bytes follow the encoded value; a canonical encoding has none.
    #[error("{0} bytes follow the encoded value")]
    TrailingBytes(usize),
    /// The bytes decode to a value whose canonical encoding is different; an overlong varint, for example.
    #[error("the bytes are not the canonical encoding of the value they decode to")]
    NonCanonical,
}

impl From<postcard::Error> for CodecError {
    fn from(err: postcard::Error) -> Self {
        CodecError::Malformed(Box::new(err))
    }
}

/// Returns the postcard bytes of `value`.
pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    Ok(postcard::to_stdvec(value)?)
}

/// Decodes exactly one value that occupies all of `bytes` in its canonical form.
pub(crate) fn decode<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let value = decode_exact(bytes)?;
    if encode(&value)? != bytes {
        return Err(CodecError::NonCanonical);
    }
    Ok(value)
}

/// Decodes one value that occupies all of `bytes`, without the canonical check.
fn decode_exact<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let (value, rest) = postcard::take_from_bytes::<T>(bytes)?;
    if !rest.is_empty() {
        return Err(CodecError::TrailingBytes(rest.len()));
    }
    Ok(value)
}

/// Why versioned bytes were refused.
#[derive(Debug, thiserror::Error)]
pub(crate) enum VersionedError {
    /// The leading version is not the one the caller reads.
    #[error("format version {0} is not supported")]
    UnknownVersion(u16),
    /// The version matches but the value does not decode.
    #[error(transparent)]
    Codec(#[from] CodecError),
}

/// Returns `version` as a varint followed by the postcard bytes of `value`.
pub(crate) fn encode_versioned<T: Serialize>(
    version: u16,
    value: &T,
) -> Result<Vec<u8>, CodecError> {
    encode(&(version, value))
}

/// Decodes bytes written by `encode_versioned`, refusing any version but `expected`.
///
/// The version is read first so that bytes of another version are
/// `UnknownVersion` whatever follows them; then the whole of `bytes`, the
/// version varint included, must be the canonical encoding.
pub(crate) fn decode_versioned<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    expected: u16,
) -> Result<T, VersionedError> {
    let (version, rest) = postcard::take_from_bytes::<u16>(bytes).map_err(CodecError::from)?;
    if version != expected {
        return Err(VersionedError::UnknownVersion(version));
    }
    let value: T = decode_exact(rest)?;
    if encode_versioned(expected, &value)? != bytes {
        return Err(VersionedError::Codec(CodecError::NonCanonical));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = encode(&7u32).unwrap();
        assert_eq!(decode::<u32>(&bytes).unwrap(), 7);
        bytes.push(0);
        assert!(matches!(
            decode::<u32>(&bytes),
            Err(CodecError::TrailingBytes(1))
        ));
    }

    /// postcard reads `[0x87, 0x00]` as 7, the same value as `[0x07]`; only
    /// the short form is the value's byte form, in the value and in the
    /// version prefix alike.
    #[test]
    fn an_overlong_varint_is_refused() {
        assert_eq!(decode::<u32>(&[0x07]).unwrap(), 7);
        assert!(matches!(
            decode::<u32>(&[0x87, 0x00]),
            Err(CodecError::NonCanonical)
        ));
        assert!(matches!(
            decode_versioned::<u32>(&[0x83, 0x00, 0x07], 3),
            Err(VersionedError::Codec(CodecError::NonCanonical))
        ));
        assert!(matches!(
            decode_versioned::<u32>(&[0x03, 0x87, 0x00], 3),
            Err(VersionedError::Codec(CodecError::NonCanonical))
        ));
    }

    #[test]
    fn versioned_bytes_carry_the_version_first() {
        let bytes = encode_versioned(3, &"x").unwrap();
        assert_eq!(bytes[0], 3);
        assert_eq!(decode_versioned::<String>(&bytes, 3).unwrap(), "x");
        assert!(matches!(
            decode_versioned::<String>(&bytes, 4),
            Err(VersionedError::UnknownVersion(3))
        ));
    }
}
