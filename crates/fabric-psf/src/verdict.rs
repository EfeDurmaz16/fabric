//! The verdict lines every reader prints for the shared fixtures, so the
//! three readers can be diffed line by line.
//!
//! One JSON object per line, keys sorted, no whitespace: for a case
//! `{"achieved","digest","fidelity","name","outcome":"accept","package"}`
//! or `{"code","field","name","outcome":"refuse"}`; for a container
//! `{"achieved","detached","digest","name","outcome":"accept"}` or
//! `{"code","field","name","outcome":"refuse","path"}`. `achieved` is the
//! level reached on [`fixture_target`].

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::achieve::{Target, achievable};
use crate::container::read_container;
use crate::manifest::read_manifest;
use crate::sha256::hex;

/// The target the verdicts compute `achieved` for: the root and home the
/// fixtures bind to, with Claude Code and Codex at the fixtures' versions.
pub fn fixture_target() -> Target {
    Target {
        root: "/Users/me/fabric/jobs/01JABCDEFGHJKMNPQRSTVWXYZ0/work".to_owned(),
        home: "/Users/me".to_owned(),
        harnesses: vec![
            ("claude_code".to_owned(), Some("2.1.294".to_owned())),
            ("codex".to_owned(), Some("0.163.0".to_owned())),
        ],
    }
}

/// The verdict on one manifest's bytes.
pub fn case_verdict(name: &str, bytes: &[u8]) -> Value {
    match read_manifest(bytes) {
        Ok(m) => json!({
            "achieved": m.achievable(&fixture_target()).achieved.as_str(),
            "digest": hex(&m.digest()),
            "fidelity": m.parts().fidelity.as_str(),
            "name": name,
            "outcome": "accept",
            "package": m.parts().package.is_some(),
        }),
        Err(r) => {
            json!({"code": r.code.as_str(), "field": r.field, "name": name, "outcome": "refuse"})
        }
    }
}

/// The verdict on one container directory.
pub fn container_verdict(name: &str, dir: &Path) -> Value {
    match read_container(dir) {
        Ok(c) => json!({
            "achieved": achievable(&c.manifest, &fixture_target(), &c.detached).achieved.as_str(),
            "detached": c.detached,
            "digest": hex(&c.manifest.digest()),
            "name": name,
            "outcome": "accept",
        }),
        Err(r) => json!({
            "code": r.code.as_str(),
            "field": r.field,
            "name": name,
            "outcome": "refuse",
            "path": r.path,
        }),
    }
}

/// Why the fixtures could not be read.
#[derive(Debug)]
pub struct FixtureError(pub String);

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FixtureError {}

/// Every verdict line for the fixture set at `dir` (`protocol/psf/v1`):
/// the cases by file name, then the containers by directory name. Each
/// container is read from a copy without its `expect.json`, which is not
/// part of it.
pub fn fixture_verdicts(dir: &Path) -> Result<Vec<String>, FixtureError> {
    let fail = |what: &str, e: &dyn std::fmt::Display| FixtureError(format!("{what}: {e}"));
    let sorted = |path: &Path| -> Result<Vec<String>, FixtureError> {
        let mut names: Vec<String> = fs::read_dir(path)
            .map_err(|e| fail(&path.display().to_string(), &e))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort_unstable();
        Ok(names)
    };
    let mut lines = Vec::new();
    for file in sorted(&dir.join("cases"))? {
        let Some(stem) = file.strip_suffix(".json") else {
            continue;
        };
        let text =
            fs::read_to_string(dir.join("cases").join(&file)).map_err(|e| fail(&file, &e))?;
        let case: Value = serde_json::from_str(&text).map_err(|e| fail(&file, &e))?;
        let input = case
            .get("input")
            .and_then(Value::as_str)
            .ok_or_else(|| FixtureError(format!("{file}: no input")))?;
        lines.push(case_verdict(&format!("cases/{stem}"), input.as_bytes()).to_string());
    }
    for name in sorted(&dir.join("containers"))? {
        let scratch =
            std::env::temp_dir().join(format!("fabric-psf-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        copy_without_expect(&dir.join("containers").join(&name), &scratch)
            .map_err(|e| fail(&name, &e))?;
        lines.push(container_verdict(&format!("containers/{name}"), &scratch).to_string());
        let _ = fs::remove_dir_all(&scratch);
    }
    Ok(lines)
}

fn copy_without_expect(from: &Path, to: &Path) -> std::io::Result<()> {
    copy_tree(from, to, true)
}

fn copy_tree(from: &Path, to: &Path, top: bool) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target, false)?;
        } else if !(top && entry.file_name() == "expect.json") {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
