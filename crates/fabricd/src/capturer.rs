//! The real [`Capturer`] of the executor (ADR 0023, the launch path): the
//! harness task's checkpoint through its adapter's suspend path into the
//! store, the cut's capture of the attempt's root, and the node's facts.
//!
//! Invariant: what a cut captures leaves this node only as the job's
//! policy lets it: the store's ignore policy with the job's export
//! allowlist, nothing wider, so a path a secret pattern matches stays
//! here unless the job named it. A harness state goes into the snapshot
//! only as its adapter captured it, redacted, the way `job suspend`
//! captures it; with no session to capture the snapshot holds the
//! workspace alone, and no native-session continuation is offered from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fabric_core::BoxFuture;
use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::{AttemptId, TaskId};
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::session::SessionId;
use fabric_store::capture::{CaptureRequest, UnmeasuredKeys, capture};
use fabric_store::policy::IgnorePolicy;
use fabric_store::store::Store;

use crate::executor::Capturer;
use crate::node::lock;
use crate::transfer::with_store;

/// What the capturer knows of one attempt this node runs.
#[derive(Debug, Clone)]
pub(crate) struct Attempt {
    /// The job.
    pub job: JobId,
    /// The root its tasks run in.
    pub root: AbsPath,
    /// The harness of its primary task.
    pub harness: HarnessKind,
    /// The paths that may leave although a secret pattern matches them.
    pub export: Vec<RelPath>,
    /// The harness's session, once its adapter named it.
    pub session: Option<SessionId>,
}

/// The suspend path of a harness: its adapter's capture of a session's
/// state into a directory. The node wires the adapters; the tests a fake.
pub(crate) trait Suspend: Send + Sync {
    /// Captures `session`'s state for `attempt` into `out`, redacted.
    fn suspend<'a>(
        &'a self,
        attempt: &'a Attempt,
        session: &'a SessionId,
        out: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// The facts of this node a cut records, measured when it is taken.
pub(crate) trait Facts: Send + Sync {
    /// The facts, sorted by name.
    fn measure(&self) -> BoxFuture<'_, Vec<(String, String)>>;
}

/// The capturer the node wires into its executor.
pub(crate) struct NodeCapturer {
    store: Store,
    node: NodeId,
    /// Where a harness state is captured before it goes into the store.
    states: PathBuf,
    suspend: Mutex<Arc<dyn Suspend>>,
    facts: Mutex<Arc<dyn Facts>>,
    attempts: Mutex<BTreeMap<AttemptId, Attempt>>,
    /// The facts the last capture measured, which the cut then records.
    measured: Mutex<Vec<(String, String)>>,
}

impl std::fmt::Debug for NodeCapturer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeCapturer({})", self.states.display())
    }
}

impl NodeCapturer {
    /// A capturer over `store` for `node`; harness states are staged under `states`.
    pub(crate) fn new(
        store: Store,
        node: NodeId,
        states: PathBuf,
        suspend: Arc<dyn Suspend>,
        facts: Arc<dyn Facts>,
    ) -> Self {
        NodeCapturer {
            store,
            node,
            states,
            suspend: Mutex::new(suspend),
            facts: Mutex::new(facts),
            attempts: Mutex::new(BTreeMap::new()),
            measured: Mutex::new(Vec::new()),
        }
    }

    /// Binds the suspend path and the facts once the node exists.
    pub(crate) fn bind(&self, suspend: Arc<dyn Suspend>, facts: Arc<dyn Facts>) {
        *lock(&self.suspend) = suspend;
        *lock(&self.facts) = facts;
    }

    /// Names the attempt's root and policy, before anything runs in it;
    /// `None` forgets it once it ended.
    pub(crate) fn set(&self, attempt: AttemptId, what: Option<Attempt>) {
        let mut attempts = lock(&self.attempts);
        match what {
            Some(what) => {
                attempts.insert(attempt, what);
            }
            None => {
                attempts.remove(&attempt);
            }
        }
    }

    /// Records the harness's session once its adapter named it.
    pub(crate) fn set_session(&self, attempt: AttemptId, session: SessionId) {
        if let Some(what) = lock(&self.attempts).get_mut(&attempt) {
            what.session = Some(session);
        }
    }

    fn attempt(&self, attempt: AttemptId) -> Result<Attempt, String> {
        lock(&self.attempts)
            .get(&attempt)
            .cloned()
            .ok_or_else(|| format!("attempt {attempt} has no root on this node"))
    }

    /// Captures the attempt's root, and `state` as its harness state when
    /// given, into a published snapshot under the job's export allowlist.
    async fn snapshot(&self, what: &Attempt, state: Option<PathBuf>) -> Result<SnapshotId, String> {
        let facts = Arc::clone(&lock(&self.facts));
        *lock(&self.measured) = facts.measure().await;
        let root = PathBuf::from(what.root.as_str());
        let job = what.job;
        let node = self.node;
        let export = what.export.clone();
        with_store(&self.store, move |store| {
            let mut policy = IgnorePolicy::default();
            policy
                .load_fabricignore(&root)
                .map_err(|e| crate::error::NodeError::Task(e.to_string()))?;
            for path in export {
                policy.allow_export(path);
            }
            let captured = capture(
                store,
                &CaptureRequest {
                    root: &root,
                    job_id: job,
                    created_by: node,
                    created_at: crate::clock::now_ms(),
                    parent: None,
                    policy: &policy,
                    carry_keys: &UnmeasuredKeys,
                    harness_state: state.as_deref(),
                    env_spec: None,
                    allow_large_untracked: false,
                },
            )
            .map_err(|e| crate::error::NodeError::Task(e.to_string()))?;
            Ok(store.publish(&captured.manifest)?)
        })
        .await
        .map_err(|e| e.to_string())
    }
}

impl Capturer for NodeCapturer {
    fn checkpoint_harness(
        &self,
        attempt: AttemptId,
        _task: TaskId,
    ) -> BoxFuture<'_, Result<SnapshotId, String>> {
        Box::pin(async move {
            let what = self.attempt(attempt)?;
            let Some(session) = what.session.clone() else {
                // No session to carry: the workspace is what the turn left.
                return self.snapshot(&what, None).await;
            };
            let out = self.states.join(attempt.to_string());
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let suspend = Arc::clone(&lock(&self.suspend));
            suspend.suspend(&what, &session, &out).await?;
            self.snapshot(&what, Some(out)).await
        })
    }

    fn capture(&self, attempt: AttemptId) -> BoxFuture<'_, Result<SnapshotId, String>> {
        Box::pin(async move {
            let what = self.attempt(attempt)?;
            self.snapshot(&what, None).await
        })
    }

    fn facts(&self) -> Vec<(String, String)> {
        lock(&self.measured).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSuspend {
        asked: Mutex<Vec<SessionId>>,
    }

    impl Suspend for FakeSuspend {
        fn suspend<'a>(
            &'a self,
            _attempt: &'a Attempt,
            session: &'a SessionId,
            out: &'a Path,
        ) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                lock(&self.asked).push(session.clone());
                std::fs::write(out.join("transcript.jsonl"), b"{\"turn\":1}\n")
                    .map_err(|e| e.to_string())
            })
        }
    }

    struct FixedFacts;

    impl Facts for FixedFacts {
        fn measure(&self) -> BoxFuture<'_, Vec<(String, String)>> {
            Box::pin(async { vec![("os".to_owned(), "test".to_owned())] })
        }
    }

    struct Bench {
        _dir: tempfile::TempDir,
        root: PathBuf,
        store: Store,
        capturer: NodeCapturer,
        suspend: Arc<FakeSuspend>,
    }

    fn bench(export: Vec<RelPath>, session: Option<SessionId>) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(root.join(".env"), b"TOKEN=not-for-export\n").unwrap();
        let store = Store::create(
            &dir.path().join("store"),
            fabric_store::objects::ChunkParams::DEFAULT,
        )
        .unwrap();
        let suspend = Arc::new(FakeSuspend {
            asked: Mutex::new(Vec::new()),
        });
        let capturer = NodeCapturer::new(
            store.handle(),
            NodeId::from_bytes([3; 32]),
            dir.path().join("states"),
            suspend.clone(),
            Arc::new(FixedFacts),
        );
        capturer.set(
            AttemptId::from_bytes([7; 16]),
            Some(Attempt {
                job: JobId::from_bytes([1; 16]),
                root: AbsPath::parse(root.to_str().unwrap()).unwrap(),
                harness: HarnessKind::Codex,
                export,
                session,
            }),
        );
        Bench {
            _dir: dir,
            root,
            store,
            capturer,
            suspend,
        }
    }

    fn paths(store: &Store, snapshot: SnapshotId) -> Vec<String> {
        store
            .manifest(snapshot)
            .unwrap()
            .files
            .iter()
            .map(|entry| entry.rel_path.as_str().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_harness_checkpoint_goes_through_the_suspend_path() {
        let session = SessionId::parse("019f233e-bb33-7342-85fa-f2d33b14368a").unwrap();
        let b = bench(Vec::new(), Some(session.clone()));
        let attempt = AttemptId::from_bytes([7; 16]);
        let snapshot = b
            .capturer
            .checkpoint_harness(attempt, TaskId::from_bytes([9; 16]))
            .await
            .unwrap();
        assert_eq!(
            *lock(&b.suspend.asked),
            vec![session],
            "the adapter's capture ran"
        );
        let manifest = b.store.manifest(snapshot).unwrap();
        assert!(
            !manifest.harness_state.entries.is_empty(),
            "the snapshot holds the harness state the suspend path captured"
        );
        assert!(paths(&b.store, snapshot).contains(&"src/main.rs".to_owned()));
        assert_eq!(
            b.capturer.facts(),
            vec![("os".to_owned(), "test".to_owned())]
        );
        assert!(b.root.join("src/main.rs").exists(), "the root is untouched");

        // No session named yet: the workspace alone, and no suspend asked.
        let b = bench(Vec::new(), None);
        let snapshot = b
            .capturer
            .checkpoint_harness(attempt, TaskId::from_bytes([9; 16]))
            .await
            .unwrap();
        assert!(lock(&b.suspend.asked).is_empty());
        assert!(
            b.store
                .manifest(snapshot)
                .unwrap()
                .harness_state
                .entries
                .is_empty()
        );
        // An attempt this capturer was never told of has nothing to capture.
        assert!(
            b.capturer
                .capture(AttemptId::from_bytes([8; 16]))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_capture_honours_the_export_allowlist() {
        let attempt = AttemptId::from_bytes([7; 16]);
        let b = bench(Vec::new(), None);
        let snapshot = b.capturer.capture(attempt).await.unwrap();
        let held = paths(&b.store, snapshot);
        assert!(held.contains(&"src/main.rs".to_owned()), "{held:?}");
        assert!(
            !held.contains(&".env".to_owned()),
            "a secret stays here: {held:?}"
        );

        let b = bench(vec![RelPath::parse(".env").unwrap()], None);
        let snapshot = b.capturer.capture(attempt).await.unwrap();
        let held = paths(&b.store, snapshot);
        assert!(
            held.contains(&".env".to_owned()),
            "the job allowed it: {held:?}"
        );
    }
}
