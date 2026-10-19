//! The one place the crate talks to postcard.
//!
//! Invariant: every value this crate persists or sends has exactly one byte
//! form. `decode` refuses bytes that do not end where the value ends, so a
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

/// Decodes exactly one value that occupies all of `bytes`.
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let (value, rest) = postcard::take_from_bytes::<T>(bytes)?;
    if !rest.is_empty() {
        return Err(CodecError::TrailingBytes(rest.len()));
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
}
