//! The identities and counters of managed jobs (ADR 0019).
//!
//! Invariant: like every id in `crate::id`, each of these has one text form
//! that parses back to the same value and serializes as its raw bytes or
//! its number. A counter is never zero, on the wire included, and grows by
//! one with `next`, which says `None` instead of wrapping.

use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

use crate::id::ParseIdError;

macro_rules! managed_ulid {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name([u8; 16]);

        impl $name {
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
                let ulid = Ulid::from_string(s).map_err(|_| ParseIdError::BadUlid)?;
                if ulid.to_string() != s {
                    return Err(ParseIdError::BadUlid);
                }
                Ok(Self(ulid.to_bytes()))
            }
        }
    };
}

managed_ulid!(
    TaskId,
    "One logical piece of a managed job's work; a ULID the authority makes."
);
managed_ulid!(
    AttemptId,
    "One execution of one task on one resource under one generation; a ULID the authority makes when it grants."
);
managed_ulid!(
    OperationId,
    "One accepted command; a ULID the authority makes when it commits the command."
);

macro_rules! counter {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU64);

        impl $name {
            /// The first value: one.
            pub const FIRST: $name = $name(NonZeroU64::MIN);

            /// Wraps a number read from outside; `None` for zero.
            #[must_use]
            pub const fn new(n: u64) -> Option<$name> {
                match NonZeroU64::new(n) {
                    Some(n) => Some($name(n)),
                    None => None,
                }
            }

            /// The value after this one, or `None` when the counter is exhausted.
            #[must_use]
            pub const fn next(self) -> Option<$name> {
                match self.0.checked_add(1) {
                    Some(n) => Some($name(n)),
                    None => None,
                }
            }

            /// The value as a number.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.get().serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let n = u64::deserialize(deserializer)?;
                $name::new(n).ok_or_else(|| {
                    serde::de::Error::custom(concat!(stringify!($name), " zero does not exist"))
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseCounterError;

            /// Reads the canonical decimal form: digits only, no sign, no leading zero.
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(ParseCounterError);
                }
                s.parse::<u64>()
                    .ok()
                    .and_then($name::new)
                    .ok_or(ParseCounterError)
            }
        }
    };
}

counter!(
    WorkRevision,
    "Accepted changes to one managed job; creation is revision one."
);
counter!(
    Generation,
    "The fencing generation of a managed job's canonical execution domain."
);
counter!(
    EventSeq,
    "The order of an event among all the events of one journal."
);

/// A counter's text was not a canonical decimal number of at least one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a positive decimal number in its canonical form")]
pub struct ParseCounterError;

/// The most bytes an operation key may have.
pub const MAX_OPERATION_KEY: usize = 128;

/// The caller's name for one command: the key the authority deduplicates on.
///
/// One to [`MAX_OPERATION_KEY`] bytes of `A-Z`, `a-z`, `0-9`, `.`, `_`, `:`
/// and `-`. A ULID is a good key.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OperationKey(String);

/// An operation key broke the rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationKeyError {
    /// The key is empty or longer than [`MAX_OPERATION_KEY`] bytes.
    #[error("an operation key has 1 to {MAX_OPERATION_KEY} bytes, this one has {0}")]
    Length(usize),
    /// A byte outside the key alphabet.
    #[error(
        "an operation key holds only A-Z, a-z, 0-9, '.', '_', ':' and '-'; byte {index} is not one"
    )]
    BadChar {
        /// Zero-based index of the byte.
        index: usize,
    },
}

impl OperationKey {
    /// Checks `key` against the rules and wraps it.
    pub fn parse(key: &str) -> Result<OperationKey, OperationKeyError> {
        if key.is_empty() || key.len() > MAX_OPERATION_KEY {
            return Err(OperationKeyError::Length(key.len()));
        }
        if let Some(index) = key
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')))
        {
            return Err(OperationKeyError::BadChar { index });
        }
        Ok(OperationKey(key.to_owned()))
    }

    /// The key's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OperationKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        OperationKey::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl FromStr for OperationKey {
    type Err = OperationKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OperationKey::parse(s)
    }
}

impl fmt::Display for OperationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for OperationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OperationKey({})", self.0)
    }
}

/// The blake3 digest of a command's canonical bytes, under the
/// `fabric.managed.command.v1` domain; what tells one command from another
/// under one operation key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RequestDigest([u8; 32]);

/// The blake3 digest of one stored event frame; what the job's next event links to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventDigest([u8; 32]);

macro_rules! digest_common {
    ($name:ident) => {
        impl $name {
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
                for b in &self.0 {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }
    };
}

digest_common!(RequestDigest);
digest_common!(EventDigest);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_keys_take_the_alphabet_and_the_length_and_nothing_else() {
        assert!(OperationKey::parse("01JABCDEF.retry_2:x-y").is_ok());
        assert_eq!(OperationKey::parse(""), Err(OperationKeyError::Length(0)));
        assert_eq!(
            OperationKey::parse(&"k".repeat(129)),
            Err(OperationKeyError::Length(129))
        );
        assert!(OperationKey::parse(&"k".repeat(128)).is_ok());
        for bad in ["a b", "a/b", "é", "a\u{1b}b", "a\nb"] {
            assert!(OperationKey::parse(bad).is_err(), "{bad:?}");
        }
        let bytes = postcard::to_stdvec("a b").unwrap();
        assert!(postcard::from_bytes::<OperationKey>(&bytes).is_err());
    }

    #[test]
    fn counters_refuse_zero_and_noncanonical_text() {
        assert_eq!(WorkRevision::new(0), None);
        assert_eq!(Generation::FIRST.next(), Generation::new(2));
        assert_eq!(EventSeq::new(u64::MAX).unwrap().next(), None);
        assert_eq!(
            "12".parse::<WorkRevision>(),
            Ok(WorkRevision::new(12).unwrap())
        );
        for bad in ["", "0", "012", "+1", "-1", "1.0", "18446744073709551616"] {
            assert!(bad.parse::<Generation>().is_err(), "{bad:?}");
        }
        let zero = postcard::to_stdvec(&0u64).unwrap();
        assert!(postcard::from_bytes::<Generation>(&zero).is_err());
    }

    #[test]
    fn managed_ulids_have_one_text_form() {
        let id = TaskId::from_bytes([0x01, 0x92, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9]);
        assert_eq!(id.to_string().parse::<TaskId>(), Ok(id));
        assert!(id.to_string().to_lowercase().parse::<TaskId>().is_err());
    }
}
