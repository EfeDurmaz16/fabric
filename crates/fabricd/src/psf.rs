//! `job_export_psf`: a job's latest snapshot as a Portable Session Format
//! v1 container (ADR 0029, docs/portable-session-format.md).
//!
//! The container names the revision, it does not hold it: the manifest
//! carries the bundle head and the ledger head, the cache rows and the
//! paths left behind, and the one blob is the captured harness state,
//! which the capture already redacted and filtered by the denylist (ADR
//! 0006). No workspace byte is read for the export, so a secret in the
//! workspace cannot reach it; a token shape in a text the manifest does
//! carry, a goal or a remote URL, is refused by the format's own check
//! before anything is written.

use std::path::PathBuf;

use fabric_client::api::PsfExported;
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, RecordHash, SnapshotId};
use fabric_core::ledger::{EventKind, Ledger};
use fabric_core::snapshot::{
    CacheClass, CacheEntry, CarryKey, CarryState, GitHead, SnapshotManifest,
};
use fabric_harness::manifest::{MANIFEST_FILE, StateManifest};
use fabric_psf::{
    Attachment, Bound, Cache, Fidelity, HARNESS_STATE_FORMAT, Harness, Job, Manifest, Parts,
    Produced, Source, Workspace, harness_state, hex, sha256, write_container,
};

use crate::error::NodeError;
use crate::local::Liveness;
use crate::node::Node;
use crate::transfer::with_store;

/// The writer this node names in `produced.writer`.
const WRITER: &str = concat!("fabric/", env!("CARGO_PKG_VERSION"));

impl Node {
    /// Writes the latest snapshot of job `id` as a container at `dir`.
    pub(crate) async fn job_export_psf(
        &self,
        id: JobId,
        dir: PathBuf,
    ) -> Result<PsfExported, NodeError> {
        let job = self.job(id).await?;
        let job = job.lock().await;
        // A running harness is ahead of the latest snapshot and may be
        // rewriting the state the attachment would carry: the export
        // names a revision at rest, as the orphan export does.
        if let Some(record) = job.harness_record()?
            && record.liveness().await != Liveness::Gone
        {
            return Err(NodeError::AlreadyRunningHarness {
                job: id,
                pid: record.pid,
            });
        }
        let latest = job.latest().ok_or(NodeError::NoSnapshot(id))?;
        let snapshot = with_store(&self.store, move |store| Ok(store.manifest(latest)?)).await?;
        let chain = self.bundle_chain(latest).await?;
        let ledger = job.ledger();
        let (ledger_head, epoch) = revision_record(ledger, latest);
        let created_by = ledger
            .records()
            .iter()
            .find_map(|r| match &r.event().kind {
                EventKind::Created { created_by, .. } => Some(*created_by),
                _ => None,
            })
            .unwrap_or(snapshot.created_by);
        let state_dir = job.dir().harness_state();
        let state = if state_dir.join(MANIFEST_FILE).is_file() {
            Some((StateManifest::read(&state_dir)?, state_dir))
        } else {
            None
        };
        let harness = harness_of(job.harness());
        let (attachments, blobs) = attachment(&harness, state.as_ref()).await?;
        let harness = Harness {
            version: state
                .as_ref()
                .and_then(|(m, _)| m.harness_version().map(str::to_owned)),
            ..harness
        };
        let parts = Parts {
            job: Job {
                id: id.to_string(),
                goal: ledger.manifest().goal.clone(),
                created_by: created_by.to_string(),
            },
            produced: Produced {
                at_ms: crate::clock::now_ms(),
                by: self.id.to_string(),
                writer: WRITER.to_owned(),
            },
            workspace: Workspace {
                root: snapshot.root.to_string(),
                bundle_head: latest.to_string(),
                ledger_head: ledger_head.to_string(),
                epoch,
                os: snapshot.platform.os.clone(),
                arch: snapshot.platform.arch.clone(),
            },
            source: source(&snapshot, &chain),
            fidelity: if attachments.is_empty() {
                Fidelity::WorkspaceOnly
            } else {
                Fidelity::NativeSession
            },
            harness,
            reason: None,
            attachments,
            caches: caches(&snapshot.caches),
            evidence: Vec::new(),
            package: None,
            left_behind: snapshot
                .left_behind
                .iter()
                .map(|l| l.path.to_string())
                .collect(),
            allowlist: Vec::new(),
        };
        let manifest = Manifest::new(&parts).map_err(|r| {
            NodeError::Psf(format!(
                "the job's state does not make a valid document: {r}"
            ))
        })?;
        let written = tokio::task::spawn_blocking({
            let dir = dir.clone();
            let manifest = manifest.clone();
            move || write_container(&dir, &manifest, &blobs)
        })
        .await
        .map_err(|e| NodeError::Task(e.to_string()))?
        .map_err(|e| NodeError::Psf(format!("{}: {e}", dir.display())))?;
        Ok(PsfExported {
            dir,
            digest: hex(&written.manifest.digest()),
            fidelity: written.manifest.parts().fidelity.as_str().to_owned(),
            snapshot: latest,
            attachments: written.present,
            left_behind: parts.left_behind,
        })
    }
}

/// The captured harness state as the one attachment, when it is the job's
/// harness's: packed, named and bound to the root and home it was captured under.
async fn attachment(
    harness: &Harness,
    state: Option<&(StateManifest, PathBuf)>,
) -> Result<(Vec<Attachment>, Vec<Vec<u8>>), NodeError> {
    match state {
        Some((manifest, dir)) if harness_of(manifest.kind()).kind == harness.kind => {
            let blob = tokio::task::spawn_blocking({
                let dir = dir.clone();
                move || harness_state::pack(&dir)
            })
            .await
            .map_err(|e| NodeError::Task(e.to_string()))?
            .map_err(|e| NodeError::Psf(e.to_string()))?;
            let attachment = Attachment {
                name: format!("{}-state", harness.kind.replace('_', "-")),
                harness: harness.kind.clone(),
                format: HARNESS_STATE_FORMAT.to_owned(),
                fidelity: Fidelity::NativeSession,
                session: Some(manifest.session().to_string()),
                bound: Some(Bound {
                    root: manifest.root().to_string(),
                    home: manifest.home().to_string(),
                }),
                digest: hex(&sha256(&blob)),
                bytes: blob.len() as u64,
            };
            Ok((vec![attachment], vec![blob]))
        }
        _ => Ok((Vec::new(), Vec::new())),
    }
}

/// The harness in the format's spelling: the client API's kind names.
fn harness_of(kind: &HarnessKind) -> Harness {
    match kind {
        HarnessKind::ClaudeCode => Harness {
            kind: "claude_code".to_owned(),
            program: None,
            args: Vec::new(),
            version: None,
        },
        HarnessKind::Codex => Harness {
            kind: "codex".to_owned(),
            program: None,
            args: Vec::new(),
            version: None,
        },
        HarnessKind::Acp { program, args } => Harness {
            kind: "acp".to_owned(),
            program: Some(program.as_str().to_owned()),
            args: args.clone(),
            version: None,
        },
    }
}

/// Where the workspace came from: the remote `origin` and the commit
/// `HEAD` named when the snapshot was taken, or the first snapshot of
/// the bundle chain when the repository has no remote another node can
/// fetch.
fn source(snapshot: &SnapshotManifest, chain: &[SnapshotId]) -> Source {
    let from_git = snapshot.git.as_ref().and_then(|git| {
        let uri = git
            .config
            .iter()
            .find(|(key, _)| key.to_string() == "remote.origin.url")
            .map(|(_, value)| value.as_str().to_owned())
            .filter(|uri| fetchable_elsewhere(uri))?;
        let commit = match &git.head {
            GitHead::Detached(oid) => oid.as_str().to_owned(),
            GitHead::Branch(branch) => {
                let name = format!("refs/heads/{}", branch.as_str());
                git.refs
                    .iter()
                    .find(|(r, _)| r.as_str() == name)
                    .map(|(_, oid)| oid.as_str().to_owned())?
            }
        };
        Some(Source::Git { uri, commit })
    });
    from_git.unwrap_or_else(|| match chain.last() {
        Some(first) => Source::Snapshot(first.to_string()),
        None => Source::Empty,
    })
}

/// The cache rows of section 5.4, sorted by path.
fn caches(entries: &[CacheEntry]) -> Vec<Cache> {
    let mut rows: Vec<Cache> = entries
        .iter()
        .map(|entry| Cache {
            path: entry.path.to_string(),
            class: match entry.class {
                CacheClass::Deps => "deps",
                CacheClass::Build => "build",
                CacheClass::ToolCache => "tool_cache",
            }
            .to_owned(),
            carry: match entry.carry {
                CarryState::Carried => "carried",
                CarryState::Partial => "partial",
                CarryState::NotCarried => "not_carried",
            }
            .to_owned(),
            key: Some(carry_key(&entry.key)),
            digest: None,
        })
        .collect();
    rows.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    rows
}

/// A carry key as its exact strings, `kind` first.
fn carry_key(key: &CarryKey) -> Vec<(String, String)> {
    let pairs: Vec<(&str, String)> = match key {
        CarryKey::Node {
            node_major,
            package_manager,
            pm_major,
            pnpm_store_path,
        } => {
            let mut pairs = vec![
                ("kind", "node".to_owned()),
                ("node_major", node_major.clone()),
                ("package_manager", package_manager.clone()),
                ("pm_major", pm_major.clone()),
            ];
            if let Some(path) = pnpm_store_path {
                pairs.push(("pnpm_store_path", path.clone()));
            }
            pairs
        }
        CarryKey::Rust {
            rustc_vv,
            cargo_home,
        } => vec![
            ("kind", "rust".to_owned()),
            ("rustc_vv", rustc_vv.clone()),
            ("cargo_home", cargo_home.to_string()),
        ],
        CarryKey::Python {
            python_version,
            python_home,
        } => vec![
            ("kind", "python".to_owned()),
            ("python_version", python_version.clone()),
            ("python_home", python_home.to_string()),
        ],
        CarryKey::Other { tool, version } => vec![
            ("kind", "other".to_owned()),
            ("tool", tool.clone()),
            ("version", version.clone()),
        ],
        CarryKey::Unmeasured => vec![("kind", "unmeasured".to_owned())],
    };
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// The revision `latest` is (section 5.2): the hash of the record at
/// which it became the job's state, the last `SnapshotPublished` or
/// `Suspended` that names it, and the epoch that record was written in.
/// Records after it, a `Resumed` or a note, do not change the revision.
fn revision_record(ledger: &Ledger, latest: SnapshotId) -> (RecordHash, u64) {
    ledger
        .records()
        .iter()
        .rev()
        .find(|r| match &r.event().kind {
            EventKind::SnapshotPublished { snapshot, .. } | EventKind::Suspended { snapshot } => {
                *snapshot == latest
            }
            _ => false,
        })
        .map_or_else(
            // The latest snapshot is set by its SnapshotPublished, so a
            // ledger always holds one; the head is the safe answer if not.
            || (ledger.head(), ledger.lease().epoch().get()),
            |r| (r.hash(), r.event().epoch.get()),
        )
}

/// True when `uri` names a repository another node can fetch: a URL of a
/// network scheme, or git's `[user@]host:path`, on a host that is not
/// this one. A local path in any spelling, a `file:` URL, a relative or
/// `~` path, or `localhost` names nothing elsewhere (section 5.2).
fn fetchable_elsewhere(uri: &str) -> bool {
    // `<transport>::<address>` runs a remote helper, never a fetch elsewhere.
    if uri.contains("::") || uri.chars().any(char::is_whitespace) {
        return false;
    }
    let host = if let Some((scheme, rest)) = uri.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !["https", "http", "ssh", "git", "git+ssh", "ssh+git"].contains(&scheme.as_str()) {
            return false;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        host.split(':').next().unwrap_or_default()
    } else {
        // git's scp form: a colon with no slash before it. One letter
        // before the colon is a drive, not a host.
        let Some((before, _)) = uri.split_once(':') else {
            return false;
        };
        let host = before.rsplit_once('@').map_or(before, |(_, host)| host);
        if before.contains('/') || host.len() < 2 {
            return false;
        }
        host
    };
    let host = host.to_ascii_lowercase();
    !(host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.starts_with("127.")
        || host == "0.0.0.0"
        || host.starts_with('['))
}

#[cfg(test)]
mod tests {
    use fabric_core::harness::HarnessKind;
    use fabric_core::id::{EventId, JobId, NodeId, SnapshotId};
    use fabric_core::ledger::{EventKind, Ledger};

    use super::{fetchable_elsewhere, revision_record};
    use crate::job::Job;
    use crate::paths::StateDir;

    /// Two exports of one revision name the same ledger head, whatever
    /// was written after it: a resume, a note, a later suspend of the
    /// same snapshot moves it to that suspend.
    #[test]
    fn the_ledger_head_is_the_record_that_made_the_revision() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDir::resolve(Some(dir.path().as_os_str()), dir.path()).unwrap();
        let (id, me) = (JobId::from_bytes([1; 16]), NodeId::from_bytes([3; 32]));
        let created = Ledger::created(
            id,
            "goal".into(),
            HarnessKind::Codex,
            me,
            EventId::from_bytes([4; 16]),
            1,
        )
        .unwrap();
        let mut job = Job::create(id, state.job(id), &created).unwrap();
        let s1 = SnapshotId::from_bytes([7; 32]);
        job.append(
            me,
            EventKind::SnapshotPublished {
                snapshot: s1,
                parent: None,
            },
        )
        .unwrap();
        let suspended = job.append(me, EventKind::Suspended { snapshot: s1 }).unwrap();
        let want = (suspended.hash(), suspended.event().epoch.get());
        assert_eq!(revision_record(job.ledger(), s1), want);
        job.append(
            me,
            EventKind::Resumed {
                node: me,
                snapshot: Some(s1),
                harness: HarnessKind::Codex,
            },
        )
        .unwrap();
        job.append(me, EventKind::Note { text: "n".into() }).unwrap();
        assert_eq!(revision_record(job.ledger(), s1), want);
        let again = job.append(me, EventKind::Suspended { snapshot: s1 }).unwrap();
        assert_eq!(
            revision_record(job.ledger(), s1),
            (again.hash(), again.event().epoch.get())
        );
    }

    #[test]
    fn only_a_network_remote_on_another_host_is_a_source() {
        for uri in [
            "https://github.com/example/repo.git",
            "http://git.example.com/repo",
            "ssh://git@github.com/example/repo.git",
            "ssh://git@github.com:22/example/repo.git",
            "git://git.example.com/repo.git",
            "git+ssh://git@example.com/repo",
            "git@github.com:example/repo.git",
            "github.com:example/repo.git",
        ] {
            assert!(fetchable_elsewhere(uri), "{uri}");
        }
        for uri in [
            "/Users/me/repos/x",
            "file:///Users/me/repos/x",
            "file://localhost/Users/me/repos/x",
            "FILE:///x",
            "../sibling",
            "./x",
            "x",
            "~/repos/x",
            "~me/repos/x",
            "C:/repos/x",
            "./a:b",
            "a/b:c",
            "ssh://localhost/repo",
            "https://127.0.0.1/repo",
            "ssh://[::1]/repo",
            "git@localhost:repo",
            "https:///repo",
            "ext::sh -c touch% /tmp/x",
            ":repo",
            "",
        ] {
            assert!(!fetchable_elsewhere(uri), "{uri}");
        }
    }
}
