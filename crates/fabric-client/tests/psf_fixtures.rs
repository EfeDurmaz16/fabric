//! The Portable Session Format v1 fixtures in `protocol/psf/v1` (ADR 0029,
//! docs/portable-session-format.md) are the contract every PSF reader
//! tests against. This test pins them until the `fabric-psf` crate takes
//! it over: every file matches `manifest.sha256`, none is missing or extra,
//! the manifest's own digest is the one below, and every case has the
//! shape section 12 of the format document names. For an accepted case it
//! also checks that the input is canonical under section 4, that its sorted
//! lists are sorted, and that `expect.digest` is the sha256 of the input,
//! so the fixtures cannot disagree with the rules they illustrate. It is
//! not a reader: it does not check the refusal a refused case names, only
//! that the code is one of the closed set and that the set is covered.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use common::{Fallible, sha256};
use serde_json::{Map, Value};

/// The sha256 of `protocol/psf/v1/manifest.sha256`. Change it only with
/// the fixtures, under the rules of docs/portable-session-format.md,
/// Compatibility.
const MANIFEST_SHA256: &str = "7ebb8217a9eac3b82f3ea7648c20b85033bf95392a35dab468f93e3b529de975";

/// The manifest cap of section 7.
const MAX_MANIFEST: usize = 1 << 20;

/// The manifest-level refusal codes of section 10, closed in v1.
const MANIFEST_CODES: [&str; 14] = [
    "too_large",
    "not_json",
    "not_canonical",
    "unsupported_version",
    "missing_field",
    "unknown_requirement",
    "bad_id",
    "bad_path",
    "bad_enum",
    "duplicate",
    "secret_policy",
    "credential_like",
    "fidelity_mismatch",
    "package_mismatch",
];

/// The container-level refusal codes of section 10, closed in v1.
const CONTAINER_CODES: [&str; 3] = ["manifest_missing", "blob_mismatch", "blob_unlisted"];

/// The fidelity ladder of ADR 0024, as section 6 names it.
const FIDELITIES: [&str; 4] = [
    "native_session",
    "new_native_session_with_package",
    "workspace_only",
    "unsupported",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/psf/v1")
}

fn str_field<'a>(object: &'a Map<String, Value>, key: &str) -> Fallible<&'a str> {
    object[key]
        .as_str()
        .ok_or_else(|| format!("{key} is not a string").into())
}

fn set_of(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// The canonical bytes of `value` under section 4 of the format document,
/// or why it has none.
fn canonical(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => return Err(format!("JSON number {n}; integers are decimal strings")),
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if !key
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    return Err(format!("key {key} is not [a-z0-9_]"));
                }
                write_str(key, out);
                out.push(':');
                canonical(&fields[*key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_str(s: &str, out: &mut String) {
    use std::fmt::Write as _;
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

/// The list `key` of `document` is strictly sorted by its entries' `by`.
fn sorted_by(document: &Map<String, Value>, key: &str, by: &str, stem: &str) -> Fallible<()> {
    let items = document[key].as_array().ok_or(format!("{stem}: {key}"))?;
    let names: Vec<&str> = items
        .iter()
        .map(|item| item[by].as_str().ok_or(format!("{stem}: {key}[].{by}")))
        .collect::<Result<_, _>>()?;
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "{stem}: {key} is sorted by {by} with no duplicates"
    );
    Ok(())
}

/// What was covered by the cases or the containers.
#[derive(Default)]
struct Coverage {
    codes: BTreeSet<String>,
    fidelities: BTreeSet<String>,
    with_package: usize,
    detached: usize,
}

fn check_accepted_case(
    stem: &str,
    input: &str,
    expect: &Map<String, Value>,
    coverage: &mut Coverage,
) -> Fallible<()> {
    assert!(
        stem.starts_with("valid_"),
        "{stem}: accepted cases are valid_*"
    );
    assert!(
        input.len() <= MAX_MANIFEST,
        "{stem}: within the manifest cap"
    );
    assert_eq!(
        str_field(expect, "digest")?,
        sha256(input.as_bytes()),
        "{stem}: expect.digest is the sha256 of the input"
    );
    assert_eq!(str_field(expect, "psf")?, "1", "{stem}");
    let document: Value = serde_json::from_str(input)?;
    let mut again = String::new();
    canonical(&document, &mut again).map_err(|why| format!("{stem}: {why}"))?;
    assert_eq!(again, input, "{stem}: the input is its own canonical form");
    let document = document.as_object().ok_or("document")?;
    assert_eq!(str_field(document, "psf")?, "1", "{stem}");
    sorted_by(document, "attachments", "name", stem)?;
    sorted_by(document, "caches", "path", stem)?;
    let level = document["fidelity"]["level"]
        .as_str()
        .ok_or("fidelity.level")?;
    let fidelity = str_field(expect, "fidelity")?;
    assert_eq!(
        level, fidelity,
        "{stem}: expect.fidelity is the offered level"
    );
    assert!(
        FIDELITIES.contains(&fidelity),
        "{stem}: a fidelity of the ladder"
    );
    coverage.fidelities.insert(fidelity.to_owned());
    let package = expect["package"].as_bool().ok_or("expect.package")?;
    assert_eq!(
        !document["package"].is_null(),
        package,
        "{stem}: expect.package says whether a package is present"
    );
    if package {
        coverage.with_package += 1;
        assert_eq!(document["package"]["version"], "1", "{stem}");
        assert_eq!(document["package"]["job"], document["job"]["id"], "{stem}");
    }
    assert_eq!(
        document["secrets"]["policy"], "never_inside",
        "{stem}: an accepted document names the one policy"
    );
    Ok(())
}

fn check_refused_case(
    stem: &str,
    expect: &Map<String, Value>,
    coverage: &mut Coverage,
) -> Fallible<()> {
    assert!(
        stem.starts_with("refuse_"),
        "{stem}: refused cases are refuse_*"
    );
    let code = str_field(expect, "code")?;
    assert!(
        MANIFEST_CODES.contains(&code),
        "{stem}: {code} is not a v1 code"
    );
    assert!(
        expect["field"].is_string(),
        "{stem}: a field, possibly empty"
    );
    coverage.codes.insert(code.to_owned());
    Ok(())
}

#[test]
fn the_manifest_is_the_pinned_one() -> Fallible<()> {
    let bytes = fs::read(root().join("manifest.sha256"))?;
    assert_eq!(
        sha256(&bytes),
        MANIFEST_SHA256,
        "protocol/psf/v1/manifest.sha256 changed; update the pin here under the Compatibility rules of docs/portable-session-format.md"
    );
    Ok(())
}

#[test]
fn every_fixture_matches_the_manifest_and_none_is_missing_or_extra() -> Fallible<()> {
    common::check_tree_against_manifest(&root())
}

#[test]
fn every_case_has_the_shape_of_section_12_and_the_closed_sets_are_covered() -> Fallible<()> {
    let mut coverage = Coverage::default();
    let mut seen = 0;
    for entry in fs::read_dir(root().join("cases"))? {
        let path = entry?.path();
        let stem = path
            .file_stem()
            .ok_or("stem")?
            .to_string_lossy()
            .into_owned();
        let case: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let case = case.as_object().ok_or("case is not an object")?;
        assert_eq!(
            str_field(case, "case")?,
            stem,
            "{stem}: case names its file"
        );
        assert!(!str_field(case, "summary")?.is_empty(), "{stem}: a summary");
        let input = str_field(case, "input")?;
        let expect = case["expect"].as_object().ok_or("expect")?;
        match str_field(expect, "outcome")? {
            "accept" => check_accepted_case(&stem, input, expect, &mut coverage)?,
            "refuse" => check_refused_case(&stem, expect, &mut coverage)?,
            other => panic!("{stem}: unknown outcome {other}"),
        }
        seen += 1;
    }
    assert!(seen >= 20, "expected the cases, found {seen}");
    assert_eq!(
        coverage.codes,
        set_of(&MANIFEST_CODES),
        "every manifest-level refusal has a case"
    );
    assert_eq!(
        coverage.fidelities,
        set_of(&FIDELITIES),
        "every fidelity has a case"
    );
    assert!(
        coverage.with_package >= 2,
        "two accepted cases carry a work package"
    );
    Ok(())
}

/// The blobs of a container directory: file name and bytes.
fn blobs_of(dir: &Path) -> Fallible<Vec<(String, Vec<u8>)>> {
    let mut blobs = Vec::new();
    if dir.join("blobs").is_dir() {
        for blob in fs::read_dir(dir.join("blobs"))? {
            let blob = blob?.path();
            blobs.push((
                blob.file_name()
                    .ok_or("blob")?
                    .to_string_lossy()
                    .into_owned(),
                fs::read(&blob)?,
            ));
        }
    }
    Ok(blobs)
}

fn attachments_of(dir: &Path) -> Fallible<Vec<Value>> {
    let document: Value = serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
    Ok(document["attachments"]
        .as_array()
        .ok_or("attachments")?
        .clone())
}

fn check_accepted_container(
    name: &str,
    dir: &Path,
    expect: &Map<String, Value>,
    coverage: &mut Coverage,
) -> Fallible<()> {
    let bytes = fs::read(dir.join("manifest.json"))?;
    assert_eq!(
        str_field(expect, "digest")?,
        sha256(&bytes),
        "{name}: digest"
    );
    let attachments = attachments_of(dir)?;
    let blobs = blobs_of(dir)?;
    let mut detached = Vec::new();
    for attachment in &attachments {
        let digest = attachment["digest"].as_str().ok_or("digest")?;
        let length: usize = attachment["bytes"].as_str().ok_or("bytes")?.parse()?;
        match blobs.iter().find(|(file, _)| file == digest) {
            Some((_, data)) => {
                assert_eq!(sha256(data), digest, "{name}: blob matches its name");
                assert_eq!(data.len(), length, "{name}: blob length as declared");
            }
            None => detached.push(Value::String(
                attachment["name"].as_str().ok_or("name")?.to_owned(),
            )),
        }
    }
    for (file, _) in &blobs {
        assert!(
            attachments.iter().any(|a| a["digest"] == *file),
            "{name}: {file} is named by the manifest"
        );
    }
    assert_eq!(
        expect["detached"],
        Value::Array(detached.clone()),
        "{name}: detached"
    );
    coverage.detached += detached.len();
    Ok(())
}

fn check_refused_container(
    name: &str,
    dir: &Path,
    expect: &Map<String, Value>,
    coverage: &mut Coverage,
) -> Fallible<()> {
    let code = str_field(expect, "code")?;
    assert!(
        CONTAINER_CODES.contains(&code),
        "{name}: {code} is not a v1 code"
    );
    let path = str_field(expect, "path")?;
    let blobs = blobs_of(dir)?;
    match code {
        "manifest_missing" => {
            assert!(
                !dir.join("manifest.json").exists(),
                "{name}: no manifest.json"
            );
            assert_eq!(path, "manifest.json", "{name}");
        }
        "blob_mismatch" => {
            let file = path.strip_prefix("blobs/").ok_or("blobs/ path")?;
            let (_, data) = blobs.iter().find(|(f, _)| f == file).ok_or("the blob")?;
            assert_ne!(
                sha256(data),
                file,
                "{name}: the blob does not hash to its name"
            );
        }
        "blob_unlisted" => {
            let file = path.strip_prefix("blobs/").ok_or("blobs/ path")?;
            assert!(
                blobs.iter().any(|(f, _)| f == file),
                "{name}: the blob exists"
            );
            let named = attachments_of(dir)?.iter().any(|a| a["digest"] == file);
            assert!(!named, "{name}: the manifest does not name it");
        }
        other => return Err(format!("{name}: unknown container code {other}").into()),
    }
    coverage.codes.insert(code.to_owned());
    Ok(())
}

#[test]
fn every_container_is_as_its_expectation_says() -> Fallible<()> {
    let mut coverage = Coverage::default();
    for entry in fs::read_dir(root().join("containers"))? {
        let dir = entry?.path();
        let name = dir
            .file_name()
            .ok_or("name")?
            .to_string_lossy()
            .into_owned();
        let expectation: Value = serde_json::from_slice(&fs::read(dir.join("expect.json"))?)?;
        let expectation = expectation.as_object().ok_or("expect.json")?;
        assert_eq!(str_field(expectation, "container")?, name, "{name}");
        let expect = expectation["expect"].as_object().ok_or("expect")?;
        match str_field(expect, "outcome")? {
            "accept" => check_accepted_container(&name, &dir, expect, &mut coverage)?,
            "refuse" => check_refused_container(&name, &dir, expect, &mut coverage)?,
            other => panic!("{name}: unknown outcome {other}"),
        }
    }
    assert_eq!(
        coverage.codes,
        set_of(&CONTAINER_CODES),
        "every container-level refusal has a container"
    );
    assert!(coverage.detached >= 1, "a container with a detached blob");
    Ok(())
}
