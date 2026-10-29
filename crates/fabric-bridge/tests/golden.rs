//! Every golden fixture of bridge protocol v1 (`protocol/bridge/v1`)
//! against this crate's frame reader and parsers: each accepted message
//! decodes and writes back to the same JSON, each refused one is refused
//! with the code and reason the fixture names, on the side that reads it.
//! `fabric-client/tests/bridge_fixtures.rs` pins the files themselves.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use fabric_bridge::frames;
use fabric_bridge::wire::{Expect, Hello, Phase, Profile, ToFabric, ToWorker, Versions};
use serde_json::Value;
use tokio::io::{AsyncRead, ReadBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/bridge/v1")
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn dir(name: &str) -> Vec<(String, Value)> {
    let mut out: Vec<_> = fs::read_dir(root().join(name))
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                load(&p),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn profiles() -> Vec<Profile> {
    ["fixture", "docker"]
        .iter()
        .map(|name| {
            Profile::from_value(&load(&root().join(format!("profiles/{name}.json")))).unwrap()
        })
        .collect()
}

fn canonical_hello() -> Hello {
    let value = load(&root().join("cases/hello_valid.json"))["message"].clone();
    match ToWorker::from_value(&value).unwrap() {
        ToWorker::Hello(hello) => hello,
        other => panic!("not a hello: {other:?}"),
    }
}

fn phase(name: &str) -> Phase {
    match name {
        "handshake" => Phase::Handshake,
        "session" => Phase::Session,
        other => panic!("phase {other}"),
    }
}

/// Parses `message` on the side that reads it, as `direction` says.
fn parse(message: &Value, direction: &str, at: Phase) -> Result<Value, (String, String)> {
    let profiles = profiles();
    match direction {
        "fabric_to_worker" => {
            ToWorker::parse_as_worker(message, at, Versions::V1, &profiles, &profiles[0])
                .map(|m| m.to_value())
                .map_err(|r| (r.code.to_owned(), r.reason.to_owned()))
        }
        "worker_to_fabric" => {
            let hello = canonical_hello();
            let expect = match at {
                Phase::Handshake => Expect::Handshake(&hello),
                Phase::Session => Expect::Session(&profiles[0]),
            };
            ToFabric::parse(message, expect)
                .map(|m| m.to_value())
                .map_err(|r| (r.code.to_owned(), r.reason.to_owned()))
        }
        other => panic!("direction {other}"),
    }
}

#[test]
fn every_profile_parses_and_writes_back_the_same_document() {
    for (name, value) in dir("profiles") {
        let profile = Profile::from_value(&value).unwrap();
        assert_eq!(profile.to_value(), value, "{name}");
    }
}

#[test]
fn every_case_is_accepted_or_refused_as_it_says_and_writes_back_unchanged() {
    let cases = dir("cases");
    assert_eq!(cases.len(), 50, "the v1 case set");
    for (name, case) in cases {
        let direction = case["direction"].as_str().unwrap();
        let at = phase(case["phase"].as_str().unwrap());
        let parsed = parse(&case["message"], direction, at);
        let expect = &case["expect"];
        match expect["outcome"].as_str().unwrap() {
            "accept" => {
                let written = parsed.unwrap_or_else(|e| panic!("{name}: refused {e:?}"));
                assert_eq!(
                    written, case["message"],
                    "{name}: does not write back unchanged"
                );
            }
            "reject" => {
                let (code, reason) = parsed.err().unwrap_or_else(|| panic!("{name}: accepted"));
                assert_eq!(
                    (code.as_str(), reason.as_str()),
                    (
                        expect["code"].as_str().unwrap(),
                        expect["reason"].as_str().unwrap()
                    ),
                    "{name}"
                );
            }
            other => panic!("{name}: outcome {other}"),
        }
    }
}

/// A reader over `bytes` that counts what it hands out.
struct Counting<'a> {
    bytes: &'a [u8],
    read: usize,
}

impl AsyncRead for Counting<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let n = buf.remaining().min(self.bytes.len() - self.read);
        let start = self.read;
        buf.put_slice(&self.bytes[start..start + n]);
        self.read += n;
        Poll::Ready(Ok(()))
    }
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// The cap the side reading `direction` applies in `phase`.
fn cap(direction: &str, at: Phase) -> usize {
    match (direction, at) {
        ("worker_to_fabric", Phase::Session) => 16 << 20,
        _ => 1 << 20,
    }
}

#[tokio::test]
async fn every_frame_is_read_or_refused_as_it_says() {
    let frames_dir = dir("frames");
    assert_eq!(frames_dir.len(), 15, "the v1 frame set");
    for (name, fixture) in frames_dir {
        let direction = fixture["direction"].as_str().unwrap();
        let at = phase(fixture["phase"].as_str().unwrap());
        let bytes = unhex(fixture["hex"].as_str().unwrap());
        let mut reader = Counting {
            bytes: &bytes,
            read: 0,
        };
        let result = frames::read(&mut reader, cap(direction, at)).await;
        let expect = &fixture["expect"];
        match expect["outcome"].as_str().unwrap() {
            "accept" => {
                let value = result
                    .unwrap_or_else(|e| panic!("{name}: {e}"))
                    .unwrap_or_else(|| panic!("{name}: no frame"));
                assert_eq!(value["type"], expect["type"], "{name}");
                let written =
                    parse(&value, direction, at).unwrap_or_else(|e| panic!("{name}: {e:?}"));
                assert_eq!(written, value, "{name}: does not write back unchanged");
                assert_eq!(reader.read, bytes.len(), "{name}: read the whole frame");
            }
            "reject" => {
                let fault = result.err().unwrap_or_else(|| panic!("{name}: accepted"));
                assert_eq!(
                    fault.code(),
                    expect["code"].as_str().unwrap(),
                    "{name}: {fault}"
                );
                if !expect["read_body"].as_bool().unwrap() {
                    assert!(
                        reader.read <= 4,
                        "{name}: read {} bytes, past the length",
                        reader.read
                    );
                }
            }
            other => panic!("{name}: outcome {other}"),
        }
    }
}

/// Every message a scenario sends or receives decodes on the side that
/// reads it and writes back unchanged; a send the worker refuses (an
/// action v1 does not define, a hello with no common version) still
/// decodes by its structure.
#[test]
fn every_scenario_message_decodes_and_writes_back_unchanged() {
    let scenarios = dir("scenarios");
    assert_eq!(scenarios.len(), 18, "the v1 scenario set");
    for (name, scenario) in scenarios {
        let mut at = if scenario["steps"][0]["handshake"] == Value::Bool(false) {
            Phase::Handshake
        } else {
            Phase::Session
        };
        for step in scenario["steps"].as_array().unwrap() {
            if step.get("respawn").is_some() {
                at = Phase::Handshake;
            }
            let (direction, message) = match (step.get("send"), step.get("receive")) {
                (Some(m), _) => ("fabric_to_worker", m),
                (_, Some(m)) => ("worker_to_fabric", m),
                _ => continue,
            };
            let written = match parse(message, direction, at) {
                Ok(written) => written,
                Err((code, _)) if code == "unsupported" || code == "hello_refused" => {
                    ToWorker::from_value(message).unwrap().to_value()
                }
                Err(e) => panic!("{name}: {e:?} for {message}"),
            };
            assert_eq!(&written, message, "{name}");
            if message["type"] == "hello_ack" {
                at = Phase::Session;
            }
        }
    }
}

#[test]
fn a_negative_zero_or_a_float_is_not_an_integer() {
    let mut message = load(&root().join("cases/request_create_valid.json"))["message"].clone();
    for bad in ["-0", "1.0", "1e3", "9007199254740992"] {
        message["deadline_ms"] = serde_json::from_str(bad).unwrap();
        let refusal = ToWorker::from_value(&message).unwrap_err();
        assert_eq!(
            (refusal.code, refusal.reason),
            ("invalid", "bad_integer"),
            "{bad}"
        );
    }
}
