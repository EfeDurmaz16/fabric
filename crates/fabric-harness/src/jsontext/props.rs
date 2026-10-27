//! Property tests for the JSON redaction (ADR 0006): a signed block that
//! holds no secret stays byte-identical, one that holds a secret is dropped
//! whole however many of its neighbours are dropped too and whatever
//! signed blocks it holds itself, no secret survives outside the kept
//! blocks, the output is JSON, and redacting the output again changes
//! nothing. The secrets include shapes that JSON escapes (a quoted value,
//! an env line after a newline), so a check on raw bytes cannot pass.

use std::fmt::Write as _;

use proptest::prelude::*;
use serde_json::{Map, Value, json};

use super::transform;
use crate::redact::Redactor;

/// Secret shapes: a token, a quoted assignment and an env line after a
/// newline, the last two hidden by JSON escapes in the raw text.
const SECRETS: &[&str] = &[
    "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab",
    "DB_PASSWORD=\"hunter22xyz\"",
    "NODE_ENV=dev\nAPI_KEY=abcd1234efgh",
];

fn secret_strategy() -> impl Strategy<Value = &'static str> {
    prop::sample::select(SECRETS)
}

fn redactor() -> &'static Redactor {
    Redactor::standard().unwrap()
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("/Users/alpha/fabric/jobs/j1/work/src/lib.rs".to_owned()),
        Just("cd /Users/alpha/w && cargo test".to_owned()),
        Just("plain text with \"quotes\" and \\ and \n newlines".to_owned()),
        Just("caf\u{e9} \u{1F600}".to_owned()),
        "[a-zA-Z0-9 /._-]{0,24}",
        "\\PC{0,8}",
    ]
}

fn signed_block() -> impl Strategy<Value = Value> {
    (text_strategy(), "[A-Za-z0-9+/=]{4,16}").prop_map(|(thinking, signature)| {
        json!({ "type": "thinking", "thinking": thinking, "signature": signature })
    })
}

/// A signed block that holds a secret one time in four.
fn signed_block_maybe_secret() -> impl Strategy<Value = Value> {
    (
        text_strategy(),
        "[A-Za-z0-9+/=]{4,16}",
        0..4u8,
        secret_strategy(),
    )
        .prop_map(|(thinking, signature, roll, secret)| {
            let thinking = if roll == 0 {
                format!("{thinking} token {secret}")
            } else {
                thinking
            };
            json!({ "type": "thinking", "thinking": thinking, "signature": signature })
        })
}

/// A signed block that holds an array of signed blocks, each of which may
/// hold a secret.
fn nested_signed_block() -> impl Strategy<Value = Value> {
    (
        signed_block_maybe_secret(),
        prop::collection::vec(signed_block_maybe_secret(), 0..3),
    )
        .prop_map(|(mut outer, inner)| {
            if let Value::Object(map) = &mut outer {
                map.insert("items".to_owned(), Value::Array(inner));
            }
            outer
        })
}

fn value_with_secrets_strategy() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        text_strategy().prop_map(Value::String),
        secret_strategy().prop_map(|secret| Value::String(format!("key {secret} here"))),
        any::<i64>().prop_map(|n| json!(n)),
        Just(Value::Null),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(
                prop_oneof![
                    inner.clone(),
                    signed_block_maybe_secret(),
                    nested_signed_block()
                ],
                0..5
            )
            .prop_map(Value::Array),
            prop::collection::vec((text_strategy(), inner), 0..5).prop_map(|pairs| {
                let mut object = Map::new();
                for (key, value) in pairs {
                    if key != "signature" {
                        object.insert(key, value);
                    }
                }
                Value::Object(object)
            }),
        ]
    })
}

fn value_strategy() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        text_strategy().prop_map(Value::String),
        any::<i64>().prop_map(|n| json!(n)),
        any::<bool>().prop_map(Value::Bool),
        Just(Value::Null),
        (-1.0e6f64..1.0e6).prop_map(|f| json!(f)),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(prop_oneof![inner.clone(), signed_block()], 0..5)
                .prop_map(Value::Array),
            prop::collection::vec((text_strategy(), inner), 0..5).prop_map(|pairs| {
                let mut object = Map::new();
                for (key, value) in pairs {
                    if key != "signature" {
                        object.insert(key, value);
                    }
                }
                Value::Object(object)
            }),
        ]
    })
}

/// The rule `jsontext` documents: a thinking block with a signature, or a
/// redacted thinking block.
fn is_signed(map: &Map<String, Value>) -> bool {
    match map.get("type").and_then(Value::as_str) {
        Some("thinking") => map.contains_key("signature"),
        Some("redacted_thinking") => true,
        _ => false,
    }
}

fn signed_spans(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            if is_signed(map) {
                out.push(value.clone());
            } else {
                map.values().for_each(|v| signed_spans(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| signed_spans(v, out)),
        _ => {}
    }
}

/// Every string in `value`, keys included.
fn all_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                out.push(key.clone());
                all_strings(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| all_strings(v, out)),
        Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

fn holds_secret(value: &Value) -> bool {
    let mut strings = Vec::new();
    all_strings(value, &mut strings);
    strings.iter().any(|s| redactor().matches(s))
}

fn strings_outside_signed(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if !is_signed(map) {
                map.values().for_each(|v| strings_outside_signed(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| strings_outside_signed(v, out)),
        Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

/// Writes `value` as JSON with the characters of its keys spelled as
/// `\uXXXX` escapes wherever `escape` yields true, cycling through it. Every
/// reader decodes the result to the same value, so the redaction must
/// treat it as the canonical text.
fn encode_with_escaped_keys(value: &Value, escape: &[bool]) -> String {
    fn write(value: &Value, escape: &[bool], next: &mut usize, out: &mut String) {
        match value {
            Value::Object(map) => {
                out.push('{');
                for (i, (key, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    for c in key.chars() {
                        let flip = !escape.is_empty() && escape[*next % escape.len()];
                        *next += 1;
                        if flip && c.is_ascii() {
                            write!(out, "\\u{:04x}", u32::from(c)).unwrap();
                        } else {
                            let quoted = serde_json::to_string(&c.to_string()).unwrap();
                            out.push_str(&quoted[1..quoted.len() - 1]);
                        }
                    }
                    out.push_str("\":");
                    write(v, escape, next, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(v, escape, next, out);
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).unwrap()),
        }
    }
    let mut out = String::new();
    write(value, escape, &mut 0, &mut out);
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn redaction_drops_only_the_signed_blocks_that_hold_a_secret(value in value_with_secrets_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, redactor()).unwrap();
        let after = outcome.text.clone().unwrap_or_else(|| text.clone());
        let parsed: Value = serde_json::from_str(&after).expect("output is JSON");
        prop_assert!(!holds_secret(&parsed), "a secret survived: {after}");

        let mut spans = Vec::new();
        signed_spans(&value, &mut spans);
        let (with_secret, clean): (Vec<&Value>, Vec<&Value>) =
            spans.iter().partition(|span| holds_secret(span));
        prop_assert_eq!(outcome.dropped_signed_blocks as usize, with_secret.len());
        for span in clean {
            let raw = serde_json::to_string(span).unwrap();
            prop_assert!(after.contains(raw.as_str()), "signed block changed: {raw}");
        }

        let mut before_strings = Vec::new();
        strings_outside_signed(&value, &mut before_strings);
        let mut after_strings = Vec::new();
        strings_outside_signed(&parsed, &mut after_strings);
        prop_assert_eq!(before_strings.len(), after_strings.len());
        for (before, after) in before_strings.iter().zip(&after_strings) {
            let mut counts = crate::redact::RedactionCounts::new();
            let expected = redactor()
                .apply(before, &mut counts)
                .unwrap_or_else(|| before.clone());
            prop_assert_eq!(after, &expected);
        }

        let again = transform(&after, redactor()).unwrap();
        prop_assert!(again.text.is_none(), "redaction is idempotent");
    }

    #[test]
    fn json_without_a_secret_is_never_changed(value in value_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, redactor()).unwrap();
        prop_assert!(outcome.text.is_none());
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        let outcome = transform(&pretty, redactor()).unwrap();
        prop_assert!(outcome.text.is_none());
    }

    /// H1: a key spelled with escapes names the same field, so the
    /// redaction drops the same signed blocks and leaves the same value as
    /// it does for the canonical spelling.
    #[test]
    fn escaped_key_spellings_are_redacted_as_the_canonical_ones(
        value in value_with_secrets_strategy(),
        escape in prop::collection::vec(any::<bool>(), 1..8),
    ) {
        let canonical = serde_json::to_string(&value).unwrap();
        let escaped = encode_with_escaped_keys(&value, &escape);
        let reread: Value = serde_json::from_str(&escaped).expect("the spelling is JSON");
        prop_assert_eq!(&reread, &value);

        let plain = transform(&canonical, redactor()).unwrap();
        let spelled = transform(&escaped, redactor()).unwrap();
        prop_assert_eq!(plain.dropped_signed_blocks, spelled.dropped_signed_blocks);
        let plain_after: Value =
            serde_json::from_str(plain.text.as_deref().unwrap_or(&canonical)).unwrap();
        let spelled_after: Value =
            serde_json::from_str(spelled.text.as_deref().unwrap_or(&escaped)).unwrap();
        prop_assert_eq!(plain_after, spelled_after);
    }
}
