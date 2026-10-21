//! Harness-native session identifiers.
//!
//! Invariant: a `SessionId` is a value a harness minted (a Claude session
//! UUID, a Codex thread id, an ACP `sessionId`) and is safe to put in a file
//! name or on a command line: no slash, no whitespace, no NUL, never a
//! leading `-` or `.`. Parsed once, at the boundary, including inside
//! `Deserialize`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// The lowercase hex digits a UUID is written with.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The most characters a session id may have.
pub const MAX_SESSION_ID_LEN: usize = 128;

/// Why a session id was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionIdError {
    /// The id has no characters.
    #[error("session id is empty")]
    Empty,
    /// The id is longer than `MAX_SESSION_ID_LEN`.
    #[error("session id is {len} characters, above {MAX_SESSION_ID_LEN}")]
    TooLong {
        /// The character count found.
        len: usize,
    },
    /// A character outside `[A-Za-z0-9._:-]`.
    #[error("session id has an invalid character at index {index}")]
    BadChar {
        /// Zero-based index of the offending character.
        index: usize,
    },
    /// The id starts with `-` or `.`, which a command line or a file system reads as something else.
    #[error("session id starts with `-` or `.`")]
    BadStart,
}

/// A session identifier minted by a harness.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, SessionIdError> {
        if text.is_empty() {
            return Err(SessionIdError::Empty);
        }
        let len = text.chars().count();
        if len > MAX_SESSION_ID_LEN {
            return Err(SessionIdError::TooLong { len });
        }
        let bad = text
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')));
        if let Some((index, _)) = bad {
            return Err(SessionIdError::BadChar { index });
        }
        if text.starts_with('-') || text.starts_with('.') {
            return Err(SessionIdError::BadStart);
        }
        Ok(Self(text.to_owned()))
    }

    /// The id of the UUID with these 16 bytes, in its lowercase
    /// hyphenated form: hex digits and `-`, starting with a digit or a
    /// letter, which the rules always accept.
    #[must_use]
    pub fn from_uuid_bytes(bytes: [u8; 16]) -> Self {
        let mut text = String::with_capacity(36);
        for (i, b) in bytes.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                text.push('-');
            }
            text.push(char::from(HEX[usize::from(b >> 4)]));
            text.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
        Self(text)
    }

    /// Returns the id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({:?})", self.0)
    }
}

impl FromStr for SessionId {
    type Err = SessionIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_rules() {
        assert!(SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").is_ok());
        assert!(SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").is_ok());
        assert!(SessionId::parse("ses_01J9X:abc").is_ok());
        assert_eq!(SessionId::parse(""), Err(SessionIdError::Empty));
        assert_eq!(SessionId::parse("-rf"), Err(SessionIdError::BadStart));
        assert_eq!(SessionId::parse(".."), Err(SessionIdError::BadStart));
        assert_eq!(
            SessionId::parse("a/b"),
            Err(SessionIdError::BadChar { index: 1 })
        );
        assert_eq!(
            SessionId::parse("a b"),
            Err(SessionIdError::BadChar { index: 1 })
        );
        let long = "a".repeat(MAX_SESSION_ID_LEN + 1);
        assert_eq!(
            SessionId::parse(&long),
            Err(SessionIdError::TooLong {
                len: MAX_SESSION_ID_LEN + 1
            })
        );
        let bytes = postcard::to_stdvec("../x").unwrap();
        assert!(postcard::from_bytes::<SessionId>(&bytes).is_err());
    }

    #[test]
    fn a_uuid_is_written_lowercase_and_hyphenated() {
        let bytes = [
            0x9c, 0x97, 0x96, 0xf0, 0x26, 0x82, 0x45, 0x46, 0xa9, 0x63, 0xd3, 0x14, 0xb3, 0xad,
            0x54, 0x3f,
        ];
        assert_eq!(
            SessionId::from_uuid_bytes(bytes).as_str(),
            "9c9796f0-2682-4546-a963-d314b3ad543f"
        );
    }
}
