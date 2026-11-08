//! The reader against every case and container of `protocol/psf/v1`: each
//! verdict is the one its `expect` names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::fs;
use std::path::{Path, PathBuf};

use fabric_psf::fixture_verdicts;
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/psf/v1")
}

/// What each fixture's `expect` says, as the verdict fields it pins.
fn expected() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut cases: Vec<_> = fs::read_dir(root().join("cases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    cases.sort();
    for path in cases {
        let case: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let e = &case["expect"];
        let want = if e["outcome"] == "accept" {
            json!({"outcome": "accept", "digest": e["digest"], "fidelity": e["fidelity"], "package": e["package"]})
        } else {
            json!({"outcome": "refuse", "code": e["code"], "field": e["field"]})
        };
        out.push((format!("cases/{stem}"), want));
    }
    let mut containers: Vec<_> = fs::read_dir(root().join("containers"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    containers.sort();
    for path in containers {
        let expect: Value =
            serde_json::from_str(&fs::read_to_string(path.join("expect.json")).unwrap()).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let e = &expect["expect"];
        let want = if e["outcome"] == "accept" {
            json!({"outcome": "accept", "digest": e["digest"], "detached": e["detached"]})
        } else {
            json!({"outcome": "refuse", "code": e["code"], "path": e["path"]})
        };
        out.push((format!("containers/{name}"), want));
    }
    out
}

#[test]
fn every_fixture_reads_as_its_expect_says() {
    let lines = fixture_verdicts(&root()).unwrap();
    let expected = expected();
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, (name, want)) in lines.iter().zip(&expected) {
        let got: Value = serde_json::from_str(line).unwrap();
        assert_eq!(got["name"], json!(name), "{line}");
        for (key, value) in want.as_object().unwrap() {
            assert_eq!(&got[key], value, "{name}: {key} in {line}");
        }
    }
}
