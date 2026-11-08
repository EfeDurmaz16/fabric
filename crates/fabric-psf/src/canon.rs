//! Canonical JSON (section 4): the one byte form of a value.
//!
//! Keys sort by their bytes, nothing sits between tokens, there are no
//! JSON numbers, and strings use a fixed escape set. The order is computed
//! here, not taken from the map: another crate in the build can switch
//! `serde_json` to insertion order.

use serde_json::{Map, Value};

/// Why a value has no canonical form, with the dotted path of the culprit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoForm {
    /// A JSON number, where v1 has only decimal strings.
    Number(String),
    /// An object key outside `[a-z0-9_]+`.
    Key(String),
}

/// The keys of `map` in byte order.
pub(crate) fn sorted_keys(map: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    keys
}

/// The path of key `key` under `path`.
pub(crate) fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// The path of element `index` under `path`.
pub(crate) fn element(path: &str, index: usize) -> String {
    format!("{path}[{index}]")
}

/// True for a key of rule 3.
pub(crate) fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The canonical text of `value`, or the first place in key order that has none.
pub(crate) fn write(value: &Value) -> Result<String, NoForm> {
    let mut out = String::new();
    write_into(&mut out, value, "")?;
    Ok(out)
}

fn write_into(out: &mut String, value: &Value, path: &str) -> Result<(), NoForm> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(_) => return Err(NoForm::Number(path.to_owned())),
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_into(out, item, &element(path, index))?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (index, key) in sorted_keys(map).into_iter().enumerate() {
                let at = child(path, key);
                if !is_key(key) {
                    return Err(NoForm::Key(at));
                }
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                if let Some(item) = map.get(key) {
                    write_into(out, item, &at)?;
                }
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Rule 6: `"`, `\`, newline, carriage return and tab by their short
/// escapes, every other control character as `\u00xx`, nothing else.
pub(crate) fn write_string(out: &mut String, text: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let n = u32::from(c) as usize;
                out.push_str("\\u00");
                out.push(char::from(HEX[n >> 4]));
                out.push(char::from(HEX[n & 0xf]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_by_bytes_and_controls_escape_in_lowercase_hex() {
        let value = json!({"b": "\u{8}\u{c}\u{1f}/é", "a": [true, null, "\"\\\n\r\t"]});
        assert_eq!(
            write(&value),
            Ok(r#"{"a":[true,null,"\"\\\n\r\t"],"b":"\u0008\u000c\u001f/é"}"#.to_owned())
        );
    }

    #[test]
    fn a_number_or_a_bad_key_has_no_form() {
        assert_eq!(
            write(&json!({"a": {"b": [1]}})),
            Err(NoForm::Number("a.b[0]".into()))
        );
        assert_eq!(
            write(&json!({"a": {"B": "x"}})),
            Err(NoForm::Key("a.B".into()))
        );
    }
}
