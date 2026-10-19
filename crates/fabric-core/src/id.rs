//! Identifiers.
//!
//! Invariant: an id is a value, never a path, a hostname or a vendor session
//! id. Every id has one text form that parses back to the same bytes, and
//! every id serializes as its raw bytes so a ledger record stays small.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Why a text id did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseIdError {
    /// The text is not a 26-character Crockford base32 ULID.
    #[error("not a ULID")]
    BadUlid,
    /// The text does not start with the prefix the id kind uses.
    #[error("missing the {0} prefix")]
    BadPrefix(&'static str),
    /// The text decodes to the wrong number of bytes.
    #[error("expected {expected} bytes, got {actual}")]
    BadLength {
        /// The byte length the id kind has.
        expected: usize,
        /// The byte length the text decoded to.
        actual: usize,
    },
    /// A character outside the id's alphabet.
    #[error("invalid character at index {index}")]
    BadChar {
        /// Zero-based index of the offending character in the text.
        index: usize,
    },
}

macro_rules! ulid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name([u8; 16]);

        impl $name {
            /// Makes a fresh id from the clock and the process random source.
            #[must_use]
            pub fn generate() -> Self {
                Self(Ulid::generate().to_bytes())
            }

            /// Wraps the 16 raw bytes of a ULID.
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            /// Returns the 16 raw bytes.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0
            }

            /// Returns the millisecond timestamp embedded in the id.
            #[must_use]
            pub const fn timestamp_ms(self) -> u64 {
                Ulid::from_bytes(self.0).timestamp_ms()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&Ulid::from_bytes(self.0), f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(s)
                    .map(|u| Self(u.to_bytes()))
                    .map_err(|_| ParseIdError::BadUlid)
            }
        }
    };
}

ulid_id!(
    JobId,
    "The identity of a job; a ULID made on the node that created it."
);
ulid_id!(
    EventId,
    "The identity of one ledger record; a ULID made by its writer."
);
ulid_id!(
    MoveId,
    "The identity of one move; a ULID made by the owner that started it."
);

macro_rules! hash_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Hashes `bytes` with blake3 and wraps the digest.
            #[must_use]
            pub fn of(bytes: &[u8]) -> Self {
                Self(*blake3::hash(bytes).as_bytes())
            }

            /// Wraps a 32-byte digest.
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// Returns the 32 raw bytes.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 32] {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_hex(f, &self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_hex32(s).map(Self)
            }
        }
    };
}

hash_id!(
    SnapshotId,
    "The identity of a snapshot: blake3 of its manifest bytes."
);
hash_id!(
    ChunkId,
    "The identity of a stored object: blake3 of its uncompressed bytes."
);
hash_id!(
    EnvSpecId,
    "The identity of an environment spec: blake3 of its postcard bytes."
);
hash_id!(
    RecordHash,
    "The blake3 hash of one ledger record's bytes; what the next record links to."
);

/// The identity of a node: its ed25519 public key.
///
/// The text form is `fab1` followed by the 32 key bytes in lowercase base32
/// without padding, so an id can be read aloud from one screen and typed on
/// another.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId([u8; 32]);

impl NodeId {
    /// The text prefix every node id starts with.
    pub const PREFIX: &'static str = "fab1";

    /// Wraps a 32-byte ed25519 public key.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the 32 raw key bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::PREFIX)?;
        f.write_str(&base32::encode(&self.0))
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({self})")
    }
}

impl FromStr for NodeId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s
            .strip_prefix(Self::PREFIX)
            .ok_or(ParseIdError::BadPrefix(Self::PREFIX))?;
        let bytes = base32::decode(body)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|v: Vec<u8>| ParseIdError::BadLength {
                expected: 32,
                actual: v.len(),
            })?;
        Ok(Self(bytes))
    }
}

/// The lease epoch of a job.
///
/// It starts at one when the job is created and grows by exactly one on each
/// `MoveImported` and `Stolen` record. It never repeats and never goes back.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Epoch(u64);

impl Epoch {
    /// The epoch of the `Created` record.
    pub const FIRST: Epoch = Epoch(1);

    /// Wraps a number read from outside; `None` for zero, which no job ever has.
    #[must_use]
    pub const fn new(n: u64) -> Option<Epoch> {
        if n == 0 { None } else { Some(Epoch(n)) }
    }

    /// Returns the epoch after this one, or `None` if the counter is exhausted.
    #[must_use]
    pub const fn next(self) -> Option<Epoch> {
        match self.0.checked_add(1) {
            Some(n) => Some(Epoch(n)),
            None => None,
        }
    }

    /// Returns the epoch as a number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Debug for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Epoch({})", self.0)
    }
}

fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for b in bytes {
        write!(f, "{b:02x}")?;
    }
    Ok(())
}

fn parse_hex32(s: &str) -> Result<[u8; 32], ParseIdError> {
    if s.len() != 64 {
        return Err(ParseIdError::BadLength {
            expected: 32,
            actual: s.len() / 2,
        });
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_val(pair[0]).ok_or(ParseIdError::BadChar { index: 2 * i })?;
        let lo = hex_val(pair[1]).ok_or(ParseIdError::BadChar { index: 2 * i + 1 })?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// RFC 4648 base32, lowercase, without padding.
mod base32 {
    use super::ParseIdError;

    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

    pub(super) fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for &b in bytes {
            buffer = (buffer << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let index = ((buffer >> bits) & 0x1f) as usize;
                out.push(char::from(ALPHABET[index]));
            }
        }
        if bits > 0 {
            let index = ((buffer << (5 - bits)) & 0x1f) as usize;
            out.push(char::from(ALPHABET[index]));
        }
        out
    }

    pub(super) fn decode(text: &str) -> Result<Vec<u8>, ParseIdError> {
        let mut out = Vec::with_capacity(text.len() * 5 / 8);
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for (index, c) in text.bytes().enumerate() {
            let value = match c {
                b'a'..=b'z' => c - b'a',
                b'2'..=b'7' => c - b'2' + 26,
                _ => return Err(ParseIdError::BadChar { index }),
            };
            buffer = (buffer << 5) | u32::from(value);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn node_id_text_form_has_prefix_and_52_characters() {
        let id = NodeId::from_bytes([0xab; 32]);
        let text = id.to_string();
        assert!(text.starts_with("fab1"));
        assert_eq!(text.len(), 4 + 52);
        assert_eq!(text.parse::<NodeId>(), Ok(id));
    }

    #[test]
    fn node_id_rejects_wrong_prefix_and_wrong_length() {
        assert_eq!(
            "xyz1aaaa".parse::<NodeId>(),
            Err(ParseIdError::BadPrefix("fab1"))
        );
        assert_eq!(
            "fab1aaaa".parse::<NodeId>(),
            Err(ParseIdError::BadLength {
                expected: 32,
                actual: 2
            })
        );
        assert_eq!(
            "fab1AAAA".parse::<NodeId>(),
            Err(ParseIdError::BadChar { index: 0 })
        );
    }

    #[test]
    fn hash_id_text_form_is_lowercase_hex() {
        let id = SnapshotId::of(b"hello");
        let text = id.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<SnapshotId>(), Ok(id));
        assert_eq!(
            "zz".parse::<SnapshotId>(),
            Err(ParseIdError::BadLength {
                expected: 32,
                actual: 1
            })
        );
    }

    #[test]
    fn ulid_id_rejects_garbage() {
        assert_eq!("not-a-ulid".parse::<JobId>(), Err(ParseIdError::BadUlid));
        let id = JobId::generate();
        assert_eq!(id.to_string().parse::<JobId>(), Ok(id));
    }

    #[test]
    fn epoch_zero_does_not_exist_and_next_stops_at_the_counter_limit() {
        assert_eq!(Epoch::new(0), None);
        assert_eq!(Epoch::new(1), Some(Epoch::FIRST));
        assert_eq!(Epoch::FIRST.next(), Some(Epoch(2)));
        assert_eq!(Epoch(u64::MAX).next(), None);
    }

    #[test]
    fn ids_serialize_as_raw_bytes() {
        let job = JobId::from_bytes([7; 16]);
        assert_eq!(postcard::to_stdvec(&job).unwrap().len(), 16);
        let node = NodeId::from_bytes([9; 32]);
        assert_eq!(postcard::to_stdvec(&node).unwrap().len(), 32);
    }

    proptest! {
        #[test]
        fn node_id_round_trips(bytes in any::<[u8; 32]>()) {
            let id = NodeId::from_bytes(bytes);
            prop_assert_eq!(id.to_string().parse::<NodeId>(), Ok(id));
        }

        #[test]
        fn base32_round_trips_any_length(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let text = base32::encode(&bytes);
            prop_assert_eq!(base32::decode(&text), Ok(bytes));
        }

        #[test]
        fn ulid_id_round_trips(bytes in any::<[u8; 16]>()) {
            let id = MoveId::from_bytes(bytes);
            prop_assert_eq!(id.to_string().parse::<MoveId>(), Ok(id));
        }
    }
}
