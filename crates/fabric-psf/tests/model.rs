//! A writer's parts go through the reader's rules, and what a target
//! achieves falls for the reasons section 6 names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a failed unwrap or a panic inside a test is the test failing"
)]

use fabric_psf::{
    Attachment, Bound, Code, Fidelity, Harness, Job, Manifest, Parts, Produced, Source, Target,
    Workspace, achievable, hex, read_container, read_manifest, sha256, write_container,
    write_manifest,
};

const ROOT: &str = "/Users/me/fabric/jobs/01JABCDEFGHJKMNPQRSTVWXYZ0/work";
const NODE: &str = "fab1aeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaq";

fn parts(blob: &[u8]) -> Parts {
    Parts {
        job: Job {
            id: "01JABCDEFGHJKMNPQRSTVWXYZ0".into(),
            goal: "fix the flaky test".into(),
            created_by: NODE.into(),
        },
        produced: Produced {
            at_ms: 1,
            by: NODE.into(),
            writer: "fabric/0.1.0".into(),
        },
        workspace: Workspace {
            root: ROOT.into(),
            bundle_head: "ab".repeat(32),
            ledger_head: "cd".repeat(32),
            epoch: 2,
            os: "macos".into(),
            arch: "arm64".into(),
        },
        source: Source::Empty,
        harness: Harness {
            kind: "claude_code".into(),
            program: None,
            args: vec![],
            version: Some("2.1.294".into()),
        },
        fidelity: Fidelity::NativeSession,
        reason: None,
        attachments: vec![Attachment {
            name: "claude-state".into(),
            harness: "claude_code".into(),
            format: fabric_psf::HARNESS_STATE_FORMAT.into(),
            fidelity: Fidelity::NativeSession,
            session: Some("9c9796f0-2682-4546-a963-d314b3ad543f".into()),
            bound: Some(Bound {
                root: ROOT.into(),
                home: "/Users/me".into(),
            }),
            digest: hex(&sha256(blob)),
            bytes: blob.len() as u64,
        }],
        caches: vec![],
        evidence: vec![],
        package: None,
        left_behind: vec![".env".into()],
        allowlist: vec![],
    }
}

fn target(home: &str, version: &str) -> Target {
    Target {
        root: ROOT.into(),
        home: home.into(),
        harnesses: vec![("claude_code".into(), Some(version.into()))],
    }
}

#[test]
fn written_parts_read_back_as_the_same_parts_and_bytes() {
    let m = Manifest::new(&parts(b"state")).unwrap();
    let again = read_manifest(&write_manifest(&m)).unwrap();
    assert_eq!(again.parts(), m.parts());
    assert_eq!(again.digest(), sha256(m.bytes()));
}

#[test]
fn a_writer_cannot_write_what_a_reader_refuses() {
    let mut p = parts(b"state");
    p.fidelity = Fidelity::WorkspaceOnly;
    assert_eq!(Manifest::new(&p).unwrap_err().code, Code::FidelityMismatch);
    let mut p = parts(b"state");
    p.job.goal = format!("use ghp_{}", "a1".repeat(18));
    assert_eq!(Manifest::new(&p).unwrap_err().code, Code::CredentialLike);
    let mut p = parts(b"state");
    p.left_behind = vec!["../outside".into()];
    assert_eq!(Manifest::new(&p).unwrap_err().code, Code::BadPath);
}

#[test]
fn the_achieved_level_falls_for_home_version_and_detachment() {
    let m = Manifest::new(&parts(b"state")).unwrap();
    assert_eq!(
        m.achievable(&target("/Users/me", "2.1.294")).achieved,
        Fidelity::NativeSession
    );
    assert_eq!(
        m.achievable(&target("/Users/me", "2.2.0")).achieved,
        Fidelity::NativeSession
    );
    let other_home = m.achievable(&target("/home/me", "2.1.294"));
    assert_eq!(other_home.achieved, Fidelity::WorkspaceOnly);
    assert!(
        other_home.why[0].contains("/home/me"),
        "{:?}",
        other_home.why
    );
    assert_eq!(
        m.achievable(&target("/Users/me", "2.1.9")).achieved,
        Fidelity::WorkspaceOnly
    );
    let detached = achievable(
        &m,
        &target("/Users/me", "2.1.294"),
        &["claude-state".into()],
    );
    assert_eq!(detached.achieved, Fidelity::WorkspaceOnly);
}

#[test]
fn a_written_container_reads_back_and_a_stray_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let blob = b"state".to_vec();
    let m = Manifest::new(&parts(&blob)).unwrap();
    let c = write_container(&dir.path().join("job.psf"), &m, &[blob]).unwrap();
    assert_eq!(c.present, vec!["claude-state".to_owned()]);
    assert!(c.detached.is_empty());
    std::fs::write(dir.path().join("job.psf/notes.txt"), b"x").unwrap();
    let refused = read_container(&dir.path().join("job.psf")).unwrap_err();
    assert_eq!(
        (refused.code.as_str(), refused.path.as_str()),
        ("blob_unlisted", "notes.txt")
    );
}

/// Section 11: a v2 document is refused `unsupported_version` whatever
/// canonical rule it changed, so the version is read before the form.
#[test]
fn a_later_version_is_refused_as_such_before_its_form_is_checked() {
    for doc in [
        r#"{"psf":"2","n":2}"#,
        r#"{"z":"x","psf":"2"}"#,
        r#"{ "psf": "2" }"#,
        r#"{"psf":"2","Key":"x"}"#,
        r#"{"psf":2}"#,
    ] {
        let refused = read_manifest(doc.as_bytes()).unwrap_err();
        assert_eq!(
            (refused.code, refused.field.as_str()),
            (Code::UnsupportedVersion, "psf"),
            "{doc}"
        );
    }
}
