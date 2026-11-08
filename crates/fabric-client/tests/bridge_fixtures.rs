//! The bridge protocol v1 fixtures in `protocol/bridge/v1` (ADR 0021,
//! docs/bridge-protocol.md) are the contract both Fabric and the Capsule
//! worker test against. Capsule keeps a byte-identical copy. This test
//! checks that every fixture matches `manifest.sha256`, that no file is
//! missing from or extra to it, and that the manifest's own digest is the
//! one pinned below, which Capsule's `scripts/bridge-fixtures.test.mjs`
//! pins too. It also checks the framed fixtures against the framing caps
//! of `fabric_client::frame`, so the bridge and the client API cannot
//! drift apart on the bytes of a frame. The sha256 and the manifest walk
//! are shared with `psf_fixtures.rs` in `common`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Fallible, sha256};
use fabric_client::frame::{MAX_FRAME, MAX_REQUEST};
use serde_json::Value;

/// The sha256 of `protocol/bridge/v1/manifest.sha256`. Change it only with
/// the same change in Capsule's copy (docs/bridge-protocol.md, Fixtures).
const MANIFEST_SHA256: &str = "40f6490fd8c54fec1f138946386f8b7793f4b8b4b88bb4eb330f828cb5900601";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/bridge/v1")
}

#[test]
fn sha256_matches_the_fips_known_answers() {
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn the_manifest_is_the_pinned_one() -> Fallible<()> {
    let bytes = fs::read(root().join("manifest.sha256"))?;
    assert_eq!(
        sha256(&bytes),
        MANIFEST_SHA256,
        "protocol/bridge/v1/manifest.sha256 changed; update the pin here and in Capsule together"
    );
    Ok(())
}

#[test]
fn every_fixture_matches_the_manifest_and_none_is_missing_or_extra() -> Fallible<()> {
    common::check_tree_against_manifest(&root())
}

fn unhex(text: &str) -> Fallible<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", text.len()).into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&text[i..i + 2], 16)?))
        .collect()
}

/// The cap a reader on the receiving side of `direction` applies in `phase`.
fn cap(direction: &str, phase: &str) -> usize {
    match (direction, phase) {
        ("worker_to_fabric", "session") => MAX_FRAME,
        _ => MAX_REQUEST,
    }
}

#[test]
fn framed_fixtures_agree_with_the_framing_caps_and_the_message_cases() -> Fallible<()> {
    assert_eq!(
        MAX_REQUEST,
        1 << 20,
        "the bridge inherits the 1 MiB request cap"
    );
    assert_eq!(
        MAX_FRAME,
        16 << 20,
        "the bridge inherits the 16 MiB frame cap"
    );
    let mut seen = 0;
    for entry in fs::read_dir(root().join("frames"))? {
        let path = entry?.path();
        let fixture: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let name = fixture["case"].as_str().ok_or("case")?;
        let direction = fixture["direction"].as_str().ok_or("direction")?;
        let phase = fixture["phase"].as_str().ok_or("phase")?;
        let bytes = unhex(fixture["hex"].as_str().ok_or("hex")?)?;
        let expect = &fixture["expect"];
        let limit = cap(direction, phase);
        match expect["outcome"].as_str() {
            Some("accept") => {
                let declared =
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                assert_eq!(declared, bytes.len() - 4, "{name}: length prefix");
                assert!(declared > 0 && declared <= limit, "{name}: within the cap");
                let body: Value = serde_json::from_slice(&bytes[4..])?;
                assert_eq!(body["type"], expect["type"], "{name}: message type");
                let case: Value =
                    serde_json::from_slice(&fs::read(root().join(format!("cases/{name}.json")))?)?;
                assert_eq!(body, case["message"], "{name}: frame and case differ");
            }
            Some("reject") => {
                let code = expect["code"].as_str().ok_or("code")?;
                let read_body = expect["read_body"].as_bool().ok_or("read_body")?;
                if bytes.len() < 4 {
                    assert_eq!(code, "truncated", "{name}");
                    assert!(!read_body, "{name}");
                } else {
                    let declared =
                        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                    match code {
                        "oversize" => assert!(declared > limit && !read_body, "{name}"),
                        "empty" => assert!(declared == 0 && !read_body, "{name}"),
                        "truncated" => assert!(declared > bytes.len() - 4 && read_body, "{name}"),
                        "malformed" => {
                            assert_eq!(declared, bytes.len() - 4, "{name}");
                            assert!(read_body, "{name}");
                        }
                        other => panic!("{name}: unknown reject code {other}"),
                    }
                }
            }
            other => panic!("{name}: unknown outcome {other:?}"),
        }
        seen += 1;
    }
    assert!(seen >= 10, "expected the framed fixtures, found {seen}");
    Ok(())
}
