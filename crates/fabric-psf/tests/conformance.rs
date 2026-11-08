//! The independence check: the Python reader (`scripts/psf_check.py`)
//! prints a verdict line per fixture, and every line is the one this
//! crate prints. The TypeScript reader's test (`sdk/typescript/test/psf.test.ts`)
//! diffs its own lines against both.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use fabric_psf::fixture_verdicts;
use serde_json::Value;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn python(mode: &str, arg: &Path) -> Vec<String> {
    let output = Command::new("python3")
        .arg("-I")
        .arg(repo().join("scripts/psf_check.py"))
        .arg(mode)
        .arg(arg)
        .output()
        .expect("python3 runs; the independence check needs it");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The accepted fixture inputs.
fn seeds() -> Vec<String> {
    let dir = repo().join("protocol/psf/v1/cases");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap()).unwrap())
        .filter(|case| case["expect"]["outcome"] == "accept")
        .map(|case| case["input"].as_str().unwrap().to_owned())
        .collect()
}

/// Every path into `value`, as the keys and indices that reach it.
fn paths(value: &Value, at: &[Value], out: &mut Vec<Vec<Value>>) {
    out.push(at.to_vec());
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                paths(v, &[at, &[Value::String(k.clone())]].concat(), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                paths(v, &[at, &[Value::from(i)]].concat(), out);
            }
        }
        _ => {}
    }
}

/// Removes the value at `at`, or puts `replace` there.
fn edit(value: &mut Value, at: &[Value], replace: Option<&Value>) {
    let index = |step: &Value| usize::try_from(step.as_u64().unwrap()).unwrap();
    let Some((last, parents)) = at.split_last() else {
        return;
    };
    let mut node = value;
    for step in parents {
        node = match step {
            Value::String(k) => node.get_mut(k.as_str()).unwrap(),
            _ => node.get_mut(index(step)).unwrap(),
        };
    }
    match (last, node, replace) {
        (Value::String(k), Value::Object(map), None) => {
            map.remove(k.as_str());
        }
        (Value::String(k), Value::Object(map), Some(r)) => {
            map.insert(k.clone(), r.clone());
        }
        (i, Value::Array(items), None) => {
            items.remove(index(i));
        }
        (i, Value::Array(items), Some(r)) => items[index(i)] = r.clone(),
        _ => {}
    }
}

/// What every value is replaced with in turn.
fn replacements() -> Vec<Value> {
    let mut out: Vec<Value> = [
        "",
        "x",
        "0",
        "01",
        "/a/../b",
        "a/.GIT/b",
        "/Users/other",
        "native_session",
        "new_native_session_with_package",
        "unsupported",
        "codex",
        "Cafe\u{301}",
    ]
    .into_iter()
    .map(Value::from)
    .collect();
    out.extend([
        Value::Null,
        Value::Array(vec![]),
        serde_json::json!({}),
        Value::Bool(true),
        Value::from(7),
        Value::from(format!("ghp_{}", "a1".repeat(18))),
    ]);
    out
}

/// Every accepted fixture input, then each one changed at every value it
/// holds (removed, or replaced by each of [`replacements`]), a few
/// whole-document edits, nesting around `serde_json`'s depth limit, and
/// bytes that are not a document.
fn mutations() -> Vec<String> {
    let mut out = Vec::new();
    for seed in seeds() {
        out.push(seed.clone());
        let value: Value = serde_json::from_str(&seed).unwrap();
        let mut all = Vec::new();
        paths(&value, &[], &mut all);
        for at in all.iter().filter(|at| !at.is_empty()) {
            let mut removed = value.clone();
            edit(&mut removed, at, None);
            out.push(serde_json::to_string(&removed).unwrap());
            for r in &replacements() {
                let mut changed = value.clone();
                edit(&mut changed, at, Some(r));
                out.push(serde_json::to_string(&changed).unwrap());
            }
        }
        let long = "g".repeat(16 * 1024 + 1);
        out.push(seed.replacen(
            "\"writer\":\"",
            &format!("\"writer\":\"{}", &long[..120]),
            1,
        ));
        out.push(seed.replacen("\"goal\":\"", &format!("\"goal\":\"{long}"), 1));
        out.push(format!("{seed}\n"));
        out.push(format!(" {seed}"));
        out.push(seed.replacen("\"psf\":\"1\"", "\"psf\":\"1\",\"psf\":\"1\"", 1));
        out.push(seed.replacen('{', "{\"zz_new\":\"kept\",", 1));
        out.push(seed.replacen('{', "{\"Bad\":\"x\",", 1));
        out.push(seed.replacen("\"requires\":[]", "\"requires\":[\"signature\"]", 1));
    }
    for depth in 126..=130 {
        out.push(format!(
            "{{\"a\":{}{}}}",
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        ));
    }
    out.extend(
        [
            "",
            "[]",
            "null",
            "{}",
            "{\"psf\":\"2\"}",
            "{\"psf\":1}",
            "\u{feff}{}",
            "{\"a\":\"\\ud800\"}",
            "{\"a\":NaN}",
        ]
        .map(str::to_owned),
    );
    out
}

#[test]
fn the_python_reader_gives_every_fixture_the_same_verdict() {
    let fixtures = repo().join("protocol/psf/v1");
    let python = python("--verdicts", &fixtures);
    let rust = fixture_verdicts(&fixtures).unwrap();
    assert_eq!(rust, python, "the Rust and Python readers disagree");
}

#[test]
fn the_python_reader_agrees_on_every_generated_document() {
    use std::fmt::Write;
    let inputs = mutations();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lines");
    let text = inputs.iter().fold(String::new(), |mut text, i| {
        let _ = writeln!(text, "{}", serde_json::to_string(i).unwrap());
        text
    });
    std::fs::write(&file, text).unwrap();
    let python = python("--lines", &file);
    assert_eq!(python.len(), inputs.len());
    let mut codes = std::collections::BTreeSet::new();
    for (i, (input, p)) in inputs.iter().zip(&python).enumerate() {
        let r = fabric_psf::case_verdict(&format!("lines/{i}"), input.as_bytes()).to_string();
        assert_eq!(&r, p, "the readers disagree on {input:?}");
        let v: Value = serde_json::from_str(&r).unwrap();
        codes.insert(v["code"].as_str().unwrap_or("accept").to_owned());
    }
    assert!(
        codes.len() >= 12,
        "the generated documents reach too few codes: {codes:?}"
    );
}
