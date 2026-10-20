//! Property tests for the JSON text transform (ADR 0006: for arbitrary
//! roots, a rewritten transcript leaves every signed block byte-identical;
//! a signed block that holds a secret is dropped whole and nothing else
//! changes) and for the path map's boundary rule.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed unwrap inside a test is the test failing"
)]
use fabric_harness::jsontext::transform;
use fabric_harness::rewrite::{PathMap, Redactor};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const OLD_ROOT: &str = "/Users/alpha/fabric/jobs/j1/work";
const NEW_ROOT: &str = "/Users/beta/elsewhere/work";
const OLD_HOME: &str = "/Users/alpha";
const NEW_HOME: &str = "/Users/beta";
const SECRET: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";

fn map() -> PathMap {
    PathMap::new([
        (OLD_ROOT.to_owned(), NEW_ROOT.to_owned()),
        (OLD_HOME.to_owned(), NEW_HOME.to_owned()),
    ])
}

fn text_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(OLD_ROOT.to_owned()),
        Just(format!("{OLD_ROOT}/src/lib.rs")),
        Just(format!("cd {OLD_ROOT} && cargo test")),
        Just(format!("{OLD_HOME}/.claude/projects/x/s.jsonl")),
        Just(format!("{OLD_ROOT}-old/keep")),
        Just(format!("{OLD_HOME}2/keep")),
        Just("plain text with \"quotes\" and \\ and \n newlines".to_owned()),
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
    fn signed_blocks_are_byte_identical_and_the_rest_is_mapped(value in value_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, &map(), None).unwrap();
        let after = outcome.text.clone().unwrap_or_else(|| text.clone());
        let parsed: Value = serde_json::from_str(&after).expect("output is JSON");

        let mut spans = Vec::new();
        signed_spans(&value, &mut spans);
        for span in &spans {
            prop_assert!(after.contains(span.as_str()), "signed block changed: {span}");
        }

        let mut before_strings = Vec::new();
        strings_outside_signed(&value, &mut before_strings);
        let mut after_strings = Vec::new();
        strings_outside_signed(&parsed, &mut after_strings);
        prop_assert_eq!(before_strings.len(), after_strings.len());
        for (before, after) in before_strings.iter().zip(&after_strings) {
            let expected = map().apply(before).unwrap_or_else(|| before.clone());
            prop_assert_eq!(after, &expected);
        }

        let again = transform(&after, &map(), None).unwrap();
        prop_assert!(again.text.is_none(), "transform is idempotent");
    }

    #[test]
    fn redaction_drops_only_the_signed_blocks_that_hold_a_secret(value in value_with_secrets_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, &PathMap::default(), Some(Redactor::standard())).unwrap();
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
    }

    #[test]
    fn an_empty_map_never_changes_valid_json(value in value_strategy()) {
        let text = serde_json::to_string(&value).unwrap();
        let outcome = transform(&text, &PathMap::default(), None).unwrap();
        prop_assert!(outcome.text.is_none());
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        let outcome = transform(&pretty, &PathMap::default(), None).unwrap();
        prop_assert!(outcome.text.is_none());
    }

    #[test]
    fn path_map_is_idempotent_and_respects_boundaries(text in text_strategy(), suffix in "[a-zA-Z0-9._-]{1,4}") {
        let m = map();
        let once = m.apply(&text).unwrap_or_else(|| text.clone());
        prop_assert!(m.apply(&once).is_none());
        // Glued to the root, the root rule must not fire; the home rule still
        // does, because the home is followed by a `/`.
        let glued = format!("{OLD_ROOT}{suffix}");
        prop_assert_eq!(
            m.apply(&glued),
            Some(format!("{NEW_HOME}/fabric/jobs/j1/work{suffix}"))
        );
    }
}
