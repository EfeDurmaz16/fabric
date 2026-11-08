//! `fabric job export --psf`, `fabric psf inspect` and `fabric job import
//! --psf` on a real node (ADR 0029): the container names the revision and
//! carries no workspace byte, so a secret planted in the workspace never
//! enters it, and the node it came from reads it back as its own job.
//! Each keeps the CLI contract's streams (docs/cli-contract.md): the
//! export prints the document digest alone, a read prints its data, the
//! sentences and hints go to standard error, and `--json` is one document.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::GIVE_UP;
use crate::world::{Fallible, Node, Wire, finish, text};

/// `fabric args...` on `node`, which must succeed: its standard output and standard error.
fn streams(node: &Node, args: &[&str]) -> Fallible<(String, String)> {
    let output = node.run(args)?;
    if !output.status.success() {
        return Err(format!("`fabric {}` failed:\n{}", args.join(" "), text(&output)).into());
    }
    Ok((
        String::from_utf8(output.stdout)?,
        String::from_utf8(output.stderr)?,
    ))
}

/// `fabric args... --json` on `node`: standard error empty, one document on one line.
fn document(node: &Node, args: &[&str]) -> Fallible<Value> {
    let mut args = args.to_vec();
    args.push("--json");
    let (out, err) = streams(node, &args)?;
    assert_eq!(err, "", "`fabric {}`", args.join(" "));
    assert_eq!(out.lines().count(), 1, "`fabric {}`: {out}", args.join(" "));
    Ok(serde_json::from_str(&out)?)
}

/// Every file under `dir`, with its bytes.
fn files(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Fallible {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files(&entry.path(), out)?;
        } else {
            out.push((entry.path().display().to_string(), fs::read(entry.path())?));
        }
    }
    Ok(())
}

#[test]
fn e2e_a_psf_export_never_carries_a_workspace_secret_and_imports_back() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    let job = crate::new_job(&p.env, &p.a, crate::AGENT)?;
    p.a.ok(&["job", "run", &job, "--prompt", "one"])?;
    let token = format!("ghp_{}", "Z9".repeat(18));
    let work = p.env.work(&job);
    fs::write(work.join(".env"), format!("GITHUB_TOKEN={token}\n"))?;
    fs::write(work.join("notes.txt"), "work that stays in the revision\n")?;
    p.a.ok(&["job", "suspend", &job])?;

    let dir = p.env.root.join("job.psf");
    let dir_text = dir.to_str().ok_or("the test directory is not UTF-8")?;
    let (digest, written) = streams(&p.a, &["job", "export", &job, "--psf", dir_text])?;
    let digest = digest
        .strip_suffix('\n')
        .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| format!("not a bare digest on stdout: {digest:?}"))?
        .to_owned();
    assert!(written.contains("as a portable session"), "{written}");
    assert!(written.contains("left behind on this node"), "{written}");
    assert!(written.contains(".env"), "{written}");
    let mut all = Vec::new();
    files(&dir, &mut all)?;
    assert!(!all.is_empty());
    for (path, bytes) in &all {
        let haystack = String::from_utf8_lossy(bytes);
        assert!(!haystack.contains(&token), "{path} holds the planted token");
        assert!(
            !haystack.contains("work that stays"),
            "{path} holds workspace bytes"
        );
    }
    let manifest = fs::read_to_string(dir.join("manifest.json"))?;
    assert!(
        manifest.contains("\"left_behind\":[\".env\"]"),
        "{manifest}"
    );

    let (inspected, err) = streams(&p.a, &["psf", "inspect", dir_text])?;
    assert_eq!(err, "");
    assert!(
        inspected.contains(&format!("digest {digest}\n")) && inspected.contains("offered "),
        "{inspected}"
    );
    let doc = document(&p.a, &["psf", "inspect", dir_text])?;
    assert_eq!(doc["container"]["digest"], digest.as_str(), "{doc}");
    assert!(doc["container"]["offered"].is_string(), "{doc}");

    let (imported, hint) = streams(&p.a, &["job", "import", "--psf", dir_text])?;
    assert!(
        imported.contains(&format!("matches job {job}")),
        "{imported}"
    );
    assert!(
        hint.contains(&format!("fabric job resume {job}")),
        "the hint is on standard error: {hint}"
    );
    let doc = document(&p.a, &["job", "import", "--psf", dir_text])?;
    assert_eq!(doc["container"]["job"], job.as_str(), "{doc}");
    assert_eq!(doc["container"]["digest"], digest.as_str(), "{doc}");
    assert!(doc["container"]["achieved"].is_string(), "{doc}");
    assert!(
        doc.get("outcome").is_none(),
        "an import changes nothing: {doc}"
    );

    let json_dir = p.env.root.join("json.psf");
    let json_text = json_dir.to_str().ok_or("the test directory is not UTF-8")?;
    let doc = document(&p.a, &["job", "export", &job, "--psf", json_text])?;
    assert_eq!(doc["outcome"], "exported", "{doc}");
    assert_eq!(doc["container"]["job"], job.as_str(), "{doc}");
    // Another export is another document: written at another time, another digest.
    assert!(doc["container"]["digest"].is_string(), "{doc}");

    fs::write(dir.join("stray.txt"), "not named by the manifest")?;
    let refused = p.a.run(&["job", "import", "--psf", dir_text])?;
    assert!(!refused.status.success(), "{}", text(&refused));
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("blob_unlisted stray.txt"),
        "{}",
        text(&refused)
    );
    let refused = p.a.run(&["job", "import", "--psf", dir_text, "--json"])?;
    assert!(refused.stdout.is_empty(), "{}", text(&refused));
    let doc: Value = serde_json::from_slice(&refused.stderr)?;
    assert_eq!(doc["reason"], "invalid_container", "{doc}");

    let again = p.a.run(&["job", "export", &job, "--psf", dir_text])?;
    assert!(
        !again.status.success(),
        "an export over an existing directory: {}",
        text(&again)
    );

    // A running harness is past the latest snapshot: no export names it.
    let (running, _pid) = p.a.run_lingering(&job)?;
    let live = p.env.root.join("live.psf");
    let live_text = live.to_str().ok_or("the test directory is not UTF-8")?;
    let refused = p.a.run(&["job", "export", &job, "--psf", live_text])?;
    p.a.ok(&["job", "suspend", &job])?;
    let _ = finish(running, GIVE_UP)?;
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("already runs as pid"),
        "{}",
        text(&refused)
    );
    assert!(!live.exists(), "a refused export wrote {}", live.display());
    Ok(())
}
