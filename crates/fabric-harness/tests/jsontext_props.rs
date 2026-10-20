//! Property tests for the JSON redaction (ADR 0006): a signed block that
//! holds no secret stays byte-identical, one that holds a secret is dropped
//! whole however many of its neighbours are dropped too, no secret
//! survives outside the kept blocks, the output is JSON, and redacting the
//! output again changes nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use fabric_harness::jsontext::transform;
use fabric_harness::redact::Redactor;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const SECRET: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";

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
    (text_strategy(), "[A-Za-z0-9+/=]{4,16}", 0..4u8).prop_map(|(thinking, signature, roll)| {
        let thinking = if roll == 0 {
            format!("{thinking} token {SECRET}")
        } else {
            thinking
        };
        json!({ "type": "thinking", "thinking": thinking, "signature": signature })
    })
}

fn value_with_secrets_strategy() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        text_strategy().prop_map(Value::String),
        Just(Value::String(format!("key {SECRET} here"))),
        any::<i64>().prop_map(|n| json!(n)),
        Just(Value::Null),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(
                prop_oneof![inner.clone(), signed_block_maybe_secret()],
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

fn signed_spans(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if map.contains_key("signature") {
                out.push(serde_json::to_string(value).unwrap());
            } else {
                map.values().for_each(|v| signed_spans(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| signed_spans(v, out)),
        _ => {}
    }
}

fn strings_outside_signed(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if !map.contains_key("signature") {
                map.values().for_each(|v| strings_outside_signed(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| strings_outside_signed(v, out)),
        Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn redaction_drops_only_the_signed_blocks_that_hold_a_secret(value in value_with_secrets_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, Redactor::standard().unwrap()).unwrap();
        let after = outcome.text.clone().unwrap_or_else(|| text.clone());
        let parsed: Value = serde_json::from_str(&after).expect("output is JSON");
        prop_assert!(!after.contains(SECRET), "a secret survived: {after}");

        let mut spans = Vec::new();
        signed_spans(&value, &mut spans);
        let (with_secret, clean): (Vec<&String>, Vec<&String>) =
            spans.iter().partition(|span| span.contains(SECRET));
        prop_assert_eq!(outcome.dropped_signed_blocks as usize, with_secret.len());
        for span in clean {
            prop_assert!(after.contains(span.as_str()), "signed block changed: {span}");
        }

        let mut before_strings = Vec::new();
        strings_outside_signed(&value, &mut before_strings);
        let mut after_strings = Vec::new();
        strings_outside_signed(&parsed, &mut after_strings);
        prop_assert_eq!(before_strings.len(), after_strings.len());
        for (before, after) in before_strings.iter().zip(&after_strings) {
            let expected = before.replace(SECRET, "[REDACTED:github-token]");
            prop_assert_eq!(after, &expected);
        }

        let again = transform(&after, Redactor::standard().unwrap()).unwrap();
        prop_assert!(again.text.is_none(), "redaction is idempotent");
    }

    #[test]
    fn json_without_a_secret_is_never_changed(value in value_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, Redactor::standard().unwrap()).unwrap();
        prop_assert!(outcome.text.is_none());
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        let outcome = transform(&pretty, Redactor::standard().unwrap()).unwrap();
        prop_assert!(outcome.text.is_none());
    }
}
