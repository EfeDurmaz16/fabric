//! `fabric job export --psf`, `fabric psf inspect` and `fabric job import
//! --psf` on a real node (ADR 0029): the container names the revision and
//! carries no workspace byte, so a secret planted in the workspace never
//! enters it, and the node it came from reads it back as its own job.

use std::fs;
use std::path::Path;

use crate::world::{Fallible, Wire, text};

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
    let written = p.a.ok(&["job", "export", &job, "--psf", dir_text])?;
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

    let inspected = p.a.ok(&["psf", "inspect", dir_text])?;
    assert!(
        inspected.contains("digest ") && inspected.contains("offered "),
        "{inspected}"
    );

    let imported = p.a.ok(&["job", "import", "--psf", dir_text])?;
    assert!(
        imported.contains(&format!("matches job {job}")),
        "{imported}"
    );
    assert!(
        imported.contains(&format!("fabric job resume {job}")),
        "{imported}"
    );

    fs::write(dir.join("stray.txt"), "not named by the manifest")?;
    let refused = p.a.run(&["job", "import", "--psf", dir_text])?;
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("blob_unlisted stray.txt"),
        "{}",
        text(&refused)
    );

    let again = p.a.run(&["job", "export", &job, "--psf", dir_text])?;
    assert!(
        !again.status.success(),
        "an export over an existing directory: {}",
        text(&again)
    );
    Ok(())
}
