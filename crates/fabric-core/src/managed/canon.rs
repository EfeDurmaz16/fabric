//! The canonical JSON of the M5 documents that are digested: the
//! checkpoint cut and the work package.
//!
//! Invariant: one value has one byte form. Object keys are sorted and
//! unique, there is no whitespace, every number travels as a decimal
//! string (the counters rule of the client API), bytes travel as
//! lowercase hex, and strings escape only what JSON requires, with
//! control characters as `\u00XX`. No serde and no JSON crate: the
//! documents are built field by field by the types that own them, so a
//! field order or a type change is a change made on purpose.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A canonical JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object; the map keeps the keys sorted.
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// A number, as the decimal string every counter travels as.
    pub(crate) fn num(n: impl std::fmt::Display) -> Json {
        Json::Str(n.to_string())
    }

    /// Bytes as lowercase hex.
    pub(crate) fn hex(bytes: &[u8]) -> Json {
        let mut text = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(text, "{b:02x}");
        }
        Json::Str(text)
    }

    /// A string.
    pub(crate) fn str(text: impl Into<String>) -> Json {
        Json::Str(text.into())
    }

    /// An object from its fields; a key given twice is a programming
    /// error and keeps the last value.
    pub(crate) fn obj<const N: usize>(fields: [(&str, Json); N]) -> Json {
        Json::Obj(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    /// An optional value: `null` when absent.
    pub(crate) fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> Json) -> Json {
        value.map_or(Json::Null, f)
    }

    /// An array from an iterator.
    pub(crate) fn arr<T>(items: impl IntoIterator<Item = T>, f: impl Fn(T) -> Json) -> Json {
        Json::Arr(items.into_iter().map(f).collect())
    }

    /// The canonical bytes.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write(&mut out);
        out.into_bytes()
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Str(s) => write_str(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sort_numbers_are_strings_and_control_characters_escape() {
        let value = Json::obj([
            ("b", Json::num(7_u64)),
            ("a", Json::arr([true, false], Json::Bool)),
            ("c", Json::str("x\n\"\u{1}")),
            ("d", Json::opt(None::<u8>, Json::num)),
            ("e", Json::hex(&[0, 255])),
        ]);
        assert_eq!(
            String::from_utf8(value.to_bytes()).unwrap(),
            r#"{"a":[true,false],"b":"7","c":"x\n\"\u0001","d":null,"e":"00ff"}"#
        );
    }
}
