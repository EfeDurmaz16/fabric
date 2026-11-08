//! The facade protocol v1 fixtures in `protocol/facade/v1` (ADR 0027,
//! docs/facade-protocol.md) are the contract the Rust client and the
//! TypeScript SDK test against. This test checks that every fixture
//! matches `manifest.sha256`, that no file is missing from or extra to it,
//! that the manifest's own digest is the one pinned below, which
//! `sdk/typescript/test/facade_fixtures.test.ts` pins too, and that every
//! case has the shape docs/facade-protocol.md gives a case: it names
//! itself, says which way it travels and what to expect, and the set of
//! accepted message types is exactly the catalogue, so a command cannot
//! lose its fixture without this test saying so.
//!
//! The parser checks run against each case too: a message a client sends
//! is read as the authority reads it (`ClientMessage::from_json`), so a
//! case the authority refuses before it asks its journal is refused here
//! with the same code and reason, and an accepted one writes back to the
//! same JSON; an answer is read as a client reads it
//! (`FacadeAnswer::from_json`). The cases the authority refuses after the
//! parse, by token, scope, key or domain, are the node's tests
//! (`fabricd::facade::tests`).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use fabric_client::facade::{ClientMessage, FacadeAnswer, RefusalCode};
use serde_json::Value;

mod support;
use support::sha256;

/// The sha256 of `protocol/facade/v1/manifest.sha256`. Change it only with
/// the same change in `sdk/typescript/test/facade_fixtures.test.ts`
/// (docs/facade-protocol.md, Fixtures).
const MANIFEST_SHA256: &str = "edc586f8a82573e41dc647af045021740cb5568141b582abc482702ad2faffa7";

/// The messages a client sends; every one has a case the authority accepts.
const CLIENT_TYPES: [&str; 6] = [
    "submit",
    "inspect",
    "watch",
    "cancel",
    "checkpoint",
    "continue",
];
/// The messages the authority answers with; every one has a case a client accepts.
const ANSWER_TYPES: [&str; 4] = ["accepted", "inspection", "events", "refused"];
/// The `code` values of `refused` (docs/facade-protocol.md, `refused`).
const REFUSAL_CODES: [&str; 9] = [
    "unsupported_version",
    "bad_request",
    "unauthorized",
    "scope",
    "unknown_job",
    "conflict",
    "revision_mismatch",
    "refused",
    "unavailable",
];
/// Fields that carry a counter and must be a canonical decimal string or null.
const COUNTER_FIELDS: [&str; 7] = [
    "revision",
    "expected_revision",
    "current_revision",
    "generation",
    "seq",
    "next",
    "after",
];

type Fallible<T> = Result<T, Box<dyn Error>>;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/facade/v1")
}

fn files(dir: &Path, base: &Path, out: &mut Vec<String>) -> Fallible<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files(&path, base, out)?;
        } else {
            let relative = path.strip_prefix(base)?;
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn manifest() -> Fallible<BTreeMap<String, String>> {
    let text = fs::read_to_string(root().join("manifest.sha256"))?;
    let mut entries = BTreeMap::new();
    let mut previous = String::new();
    for line in text.lines() {
        let (digest, path) = line
            .split_once("  ")
            .ok_or_else(|| format!("manifest line without two spaces: {line}"))?;
        assert_eq!(digest.len(), 64, "{path}: digest length");
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "{path}: digest is lowercase hex"
        );
        assert!(previous.as_str() < path, "manifest not sorted at {path}");
        path.clone_into(&mut previous);
        entries.insert(path.to_owned(), digest.to_owned());
    }
    Ok(entries)
}

fn cases() -> Fallible<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root().join("cases"))? {
        let path = entry?.path();
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("case file name")?
            .to_owned();
        let case: Value = serde_json::from_slice(&fs::read(&path)?)?;
        out.push((stem, case));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn is_canonical_decimal(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
        && text.len() <= 20
}

/// Every counter field anywhere in `value` is a canonical decimal string or null.
fn counters_are_strings(name: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if COUNTER_FIELDS.contains(&key.as_str()) {
                    let ok = match inner {
                        Value::Null => true,
                        Value::String(text) => is_canonical_decimal(text),
                        _ => false,
                    };
                    assert!(
                        ok,
                        "{name}: {key} = {inner}, not a canonical decimal string or null"
                    );
                }
                counters_are_strings(name, inner);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| counters_are_strings(name, item)),
        _ => {}
    }
}

#[test]
fn the_manifest_is_the_pinned_one() -> Fallible<()> {
    let bytes = fs::read(root().join("manifest.sha256"))?;
    assert_eq!(
        sha256(&bytes),
        MANIFEST_SHA256,
        "protocol/facade/v1/manifest.sha256 changed; update the pin here and in sdk/typescript/test/facade_fixtures.test.ts together"
    );
    Ok(())
}

#[test]
fn every_fixture_matches_the_manifest_and_none_is_missing_or_extra() -> Fallible<()> {
    let listed = manifest()?;
    let mut found = Vec::new();
    files(&root(), &root(), &mut found)?;
    found.retain(|path| path != "manifest.sha256");
    found.sort();
    let listed_paths: Vec<&String> = listed.keys().collect();
    let found_paths: Vec<&String> = found.iter().collect();
    assert_eq!(found_paths, listed_paths, "files and manifest differ");
    for (path, digest) in &listed {
        let bytes = fs::read(root().join(path))?;
        assert_eq!(
            &sha256(&bytes),
            digest,
            "{path} does not match the manifest"
        );
        assert!(
            !bytes.contains(&b'\r'),
            "{path} has a carriage return; fixtures are LF only"
        );
        assert!(
            path.starts_with("cases/")
                && Path::new(path).extension().is_some_and(|ext| ext == "json"),
            "{path}: v1 holds cases only"
        );
    }
    Ok(())
}

#[test]
fn every_case_names_itself_and_says_what_to_expect() -> Fallible<()> {
    let mut accepted_client = BTreeSet::new();
    let mut accepted_answers = BTreeSet::new();
    let mut seen = 0;
    for (stem, case) in cases()? {
        let name = case["case"].as_str().ok_or("case")?;
        assert_eq!(name, stem, "{stem}: a case is named after its file");
        let direction = case["direction"].as_str().ok_or("direction")?;
        let message = case["message"].as_object().ok_or("message")?;
        let kind = message["type"].as_str().ok_or("type")?;
        let expect = case["expect"].as_object().ok_or("expect")?;
        let outcome = expect["outcome"].as_str().ok_or("outcome")?;
        match direction {
            "client_to_authority" => {
                let given = case["given"].as_object().ok_or("given")?;
                assert!(given["now_ms"].is_u64(), "{name}: given.now_ms");
                for token in given["tokens"].as_array().ok_or("tokens")? {
                    let bearer = token["bearer"].as_str().ok_or("bearer")?;
                    assert_eq!(bearer.len(), 64, "{name}: a bearer is 64 hex characters");
                    assert!(token["token_id"].is_string(), "{name}: token_id");
                }
                assert!(given["jobs"].is_array(), "{name}: given.jobs");
                assert!(given["operations"].is_array(), "{name}: given.operations");
                if outcome == "accept" {
                    assert!(CLIENT_TYPES.contains(&kind), "{name}: accepted type {kind}");
                    assert_eq!(message["facade"], 1, "{name}: facade version");
                    assert!(message["token"].is_string(), "{name}: token");
                    accepted_client.insert(kind.to_owned());
                    let answer = expect["answer"].as_object().ok_or("answer")?;
                    let answer_type = answer["type"].as_str().ok_or("answer type")?;
                    assert!(
                        ANSWER_TYPES.contains(&answer_type),
                        "{name}: answer type {answer_type}"
                    );
                    assert_ne!(
                        answer_type, "refused",
                        "{name}: an accepted case is not refused"
                    );
                }
            }
            "authority_to_client" => {
                assert!(
                    case.get("given").is_none(),
                    "{name}: an answer has no given"
                );
                if outcome == "accept" {
                    assert!(ANSWER_TYPES.contains(&kind), "{name}: accepted type {kind}");
                    assert_eq!(message["facade"], 1, "{name}: facade version");
                    accepted_answers.insert(kind.to_owned());
                    if kind == "refused" {
                        let code = message["code"].as_str().ok_or("code")?;
                        assert!(REFUSAL_CODES.contains(&code), "{name}: code {code}");
                        assert!(message["reason"].is_string(), "{name}: reason");
                        assert_eq!(
                            code == "revision_mismatch",
                            message.contains_key("current_revision"),
                            "{name}: current_revision exactly for revision_mismatch"
                        );
                    }
                }
            }
            other => panic!("{name}: unknown direction {other}"),
        }
        match outcome {
            "accept" => counters_are_strings(name, &case["message"]),
            "reject" => {
                let code = expect["code"].as_str().ok_or("code")?;
                assert!(expect["reason"].is_string(), "{name}: reason");
                if direction == "client_to_authority" {
                    assert!(REFUSAL_CODES.contains(&code), "{name}: refusal code {code}");
                    assert_ne!(
                        code, "unavailable",
                        "{name}: a fixture never refuses as unavailable"
                    );
                } else {
                    assert_eq!(
                        code, "protocol_violation",
                        "{name}: a client refuses a frame as a protocol violation"
                    );
                }
            }
            other => panic!("{name}: unknown outcome {other}"),
        }
        seen += 1;
    }
    let want_client: BTreeSet<String> = CLIENT_TYPES.iter().map(|s| (*s).to_owned()).collect();
    let want_answers: BTreeSet<String> = ANSWER_TYPES.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        accepted_client, want_client,
        "every command has an accepted case"
    );
    assert_eq!(
        accepted_answers, want_answers,
        "every answer has an accepted case"
    );
    assert!(seen >= 40, "expected the facade cases, found {seen}");
    Ok(())
}

#[test]
fn the_named_failure_modes_have_their_cases() -> Fallible<()> {
    let names: BTreeSet<String> = cases()?.into_iter().map(|(stem, _)| stem).collect();
    for required in [
        "submit_duplicate_key",
        "submit_conflict",
        "submit_revoked_token",
        "submit_expired_token",
        "inspect_scope_refused",
        "cancel_scope_refused",
        "submit_pin_outside_resource_scope",
        "watch_lagged",
        "watch_revoked_token",
        "cancel_revision_mismatch",
        "submit_repository_source_rejected",
    ] {
        assert!(names.contains(required), "missing case {required}");
    }
    Ok(())
}

/// Which refusals the parse alone decides: the schema and the version.
/// Every other code needs the authority's journal.
fn parse_decides(code: &str) -> bool {
    matches!(code, "bad_request" | "unsupported_version")
}

#[test]
fn every_client_message_reads_as_the_authority_reads_it() -> Fallible<()> {
    let mut checked = 0;
    for (name, case) in cases()? {
        if case["direction"] != "client_to_authority" {
            continue;
        }
        let expect = &case["expect"];
        let parsed = ClientMessage::from_json(&case["message"]);
        match expect["outcome"].as_str() {
            Some("accept") => {
                let message = parsed.map_err(|e| format!("{name}: refused as {e}"))?;
                assert_eq!(message.to_json(), case["message"], "{name}: writes back");
                assert_eq!(message.kind(), case["message"]["type"], "{name}: type");
            }
            Some("reject") => {
                let code = expect["code"].as_str().ok_or("code")?;
                let reason = expect["reason"].as_str().ok_or("reason")?;
                if parse_decides(code) {
                    let refusal = parsed.err().ok_or_else(|| format!("{name}: accepted"))?;
                    assert_eq!(
                        refusal.code.name(),
                        code,
                        "{name}: code ({})",
                        refusal.message
                    );
                    assert_eq!(
                        refusal.reason, reason,
                        "{name}: reason ({})",
                        refusal.message
                    );
                    assert_eq!(refusal.to_json()["code"], code, "{name}: refused answer");
                } else {
                    assert!(
                        parsed.is_ok(),
                        "{name}: the parse refused what the authority decides: {parsed:?}"
                    );
                }
            }
            other => panic!("{name}: outcome {other:?}"),
        }
        checked += 1;
    }
    assert!(checked >= 30, "client cases checked: {checked}");
    Ok(())
}

#[test]
fn every_answer_reads_as_a_client_reads_it() -> Fallible<()> {
    let mut checked = 0;
    for (name, case) in cases()? {
        if case["direction"] != "authority_to_client" {
            continue;
        }
        let expect = &case["expect"];
        let parsed = FacadeAnswer::from_json(&case["message"]);
        match expect["outcome"].as_str() {
            Some("accept") => {
                let answer = parsed.map_err(|e| format!("{name}: refused as {e}"))?;
                assert_eq!(answer.to_json(), case["message"], "{name}: writes back");
                if let FacadeAnswer::Refused(refusal) = &answer {
                    assert!(
                        RefusalCode::ALL.contains(&refusal.code),
                        "{name}: code {}",
                        refusal.code
                    );
                }
            }
            Some("reject") => {
                let violation = parsed.err().ok_or_else(|| format!("{name}: accepted"))?;
                assert_eq!(expect["code"], "protocol_violation", "{name}");
                assert_eq!(violation.reason, expect["reason"], "{name}: {violation}");
            }
            other => panic!("{name}: outcome {other:?}"),
        }
        checked += 1;
    }
    assert!(checked >= 10, "answer cases checked: {checked}");
    Ok(())
}
