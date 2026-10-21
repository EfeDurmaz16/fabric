//! Ids as their text form on the client socket.
//!
//! Invariant: an id crosses the socket as the one text form its type
//! prints and parses (ADR 0018, Encoding rules), never as the raw bytes the
//! core serializes for postcard. A text that does not parse fails the whole
//! message, so nothing past the boundary holds an id that is not one.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serializer, de::Error};

/// Writes `value` as its `Display` text.
pub(crate) fn serialize<T: Display, S: Serializer>(value: &T, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(value)
}

/// Reads a value from its text with `FromStr`.
pub(crate) fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
where
    T: FromStr,
    T::Err: Display,
    D: Deserializer<'de>,
{
    let text = String::deserialize(d)?;
    text.parse()
        .map_err(|e| D::Error::custom(format!("{text:?}: {e}")))
}

/// The same for an optional value, which is absent rather than `null` when it has none.
pub(crate) mod option {
    use super::{Deserialize, Deserializer, Display, Error, FromStr, Serializer};

    /// Writes the text of `value`; the field is skipped when it is `None`.
    #[expect(
        clippy::ref_option,
        reason = "serde's `with` passes the field by reference"
    )]
    pub(crate) fn serialize<T: Display, S: Serializer>(
        value: &Option<T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => s.collect_str(value),
            None => s.serialize_none(),
        }
    }

    /// Reads a text, or `null` as `None`.
    pub(crate) fn deserialize<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(d)?
            .map(|text| {
                text.parse()
                    .map_err(|e| D::Error::custom(format!("{text:?}: {e}")))
            })
            .transpose()
    }
}
