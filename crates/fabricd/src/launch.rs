//! The authority's half of the launch path (docs/managed-jobs.md,
//! "Launch path"): what it sends the node it granted an attempt to, and
//! the instructions it sends the node that runs one.
//!
//! Invariant: a launch request is built from the journal alone, at the
//! grant: the job's intent, source and policy, the grant's checkpoint and
//! the node whose store holds it, the open build and test tasks under the
//! attempt's task with their programs, and for a continue the fidelities
//! allowed and the work package. The authority keeps no "launch sent"
//! fact; the grant is the durable fact and the node's launch record is
//! the other half, so sending again after a restart is answered
//! `AlreadyLaunched` and launches nothing twice.

use std::sync::Arc;
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::id::NodeId;
use fabric_core::managed::launch::{
    ContinuationAsk, GrantToken, InstructAnswer, Instruction, LaunchAnswer, LaunchRequest,
    TaskLaunch,
};
use fabric_core::managed::placement::PlacementDecision;
use fabric_core::managed::{
    AllowedFidelity, AttemptId, CommandKind, EffectRequest, ManagedJob, ManagedState,
    OperationStatus, ResourceRef, TaskClass, TaskId, TaskState,
};
use fabric_journal::Journal;
use fabric_net::rpc::Service;

use crate::authority::AuthorityError;
use crate::error::NodeError;
use crate::node::Node;
use crate::package::{BuildError, build_for_continue};
use crate::placement::PlacementAsk;

/// How long the authority waits for a node to answer a launch or an
/// instruction: the answer says the node took it, never what came of it,
/// so it is short however long a clone or a drain takes.
pub(crate) const LAUNCH_DEADLINE: Duration = Duration::from_secs(15);

/// A launch, built and addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchPlan {
    /// What is sent.
    pub request: LaunchRequest,
    /// The node the grant names.
    pub node: NodeId,
}

/// Why no launch was planned.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlanError {
    /// The journal could not be read; the launch is tried again later.
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    /// The grant cannot be launched as recorded: it is failed with this
    /// reason as the harness receipt's `Blocked`, so its task is free.
    #[error("{0}")]
    Blocked(String),
}

impl From<fabric_journal::CommitError> for PlanError {
    fn from(error: fabric_journal::CommitError) -> Self {
        PlanError::Authority(error.into())
    }
}

/// The fidelities both allow.
fn both(a: AllowedFidelity, b: AllowedFidelity) -> AllowedFidelity {
    AllowedFidelity {
        native_session: a.native_session && b.native_session,
        new_native_session_with_package: a.new_native_session_with_package
            && b.new_native_session_with_package,
        workspace_only: a.workspace_only && b.workspace_only,
    }
}

/// Whether `task` is `root` or below it.
fn under(state: &ManagedState, task: TaskId, root: TaskId) -> bool {
    let mut at = Some(task);
    // A parent chain is at most as long as the task list.
    for _ in 0..=state.tasks.len() {
        match at {
            Some(t) if t == root => return true,
            Some(t) => at = state.task(t).and_then(|t| t.parent),
            None => return false,
        }
    }
    false
}

/// Whether a spawn of `task` is on its way to the live attempt: its
/// `Spawn` is accepted, dispatched or of unknown fate. That instruction
/// starts it, so a launch does not name it too.
fn spawn_in_flight(state: &ManagedState, task: TaskId) -> bool {
    state.requests.iter().any(|(op, request)| {
        *request == EffectRequest::Spawn { task }
            && state.operation(*op).is_some_and(|o| {
                matches!(
                    o.status,
                    OperationStatus::Accepted
                        | OperationStatus::Dispatched
                        | OperationStatus::Unknown
                )
            })
    })
}

/// The open build and test tasks under `root` that have a program, as
/// launches, at most `MAX_TASK_LAUNCHES` (the rest wait for a spawn). A
/// task whose spawn is in flight is left to it.
pub(crate) fn task_launches(state: &ManagedState, root: TaskId) -> Vec<TaskLaunch> {
    state
        .tasks
        .iter()
        .filter(|t| {
            t.id != root
                && t.state == TaskState::Open
                && matches!(t.class, TaskClass::Build | TaskClass::Test)
                && under(state, t.id, root)
                && !spawn_in_flight(state, t.id)
        })
        .filter_map(|t| {
            t.program.clone().map(|program| TaskLaunch {
                task: t.id,
                class: t.class,
                program,
            })
        })
        .take(fabric_core::managed::launch::MAX_TASK_LAUNCHES)
        .collect()
}

/// The token of `attempt`.
pub(crate) fn token_of(state: &ManagedState, attempt: AttemptId) -> Option<GrantToken> {
    state.attempt(attempt).map(|a| GrantToken {
        job: state.job.id,
        attempt: a.id,
        generation: a.generation,
    })
}

/// Builds the launch of `attempt` from the journal: intent, source and
/// policy from the job; `initial_checkpoint` from the grant; `snapshot_at`
/// the node of the attempt that cut it, or this node for an import's
/// snapshot; `tasks` the open build and test tasks under the attempt's
/// task with a program; `continuation` and `package` for a grant a
/// continue made.
pub(crate) fn plan(
    journal: &Journal,
    state: &ManagedState,
    attempt: AttemptId,
) -> Result<LaunchPlan, PlanError> {
    let blocked = |reason: &str| PlanError::Blocked(reason.to_owned());
    let found = state
        .attempt(attempt)
        .ok_or_else(|| blocked("the attempt is not in the journal"))?;
    let ResourceRef::Node { node } = found.resource;
    let grant = state
        .operation(found.granted_by)
        .ok_or_else(|| blocked("the attempt's grant is not in the journal"))?;
    let from = (grant.kind == CommandKind::Continue)
        .then(|| {
            state
                .attempts
                .iter()
                .rev()
                .find(|a| a.task == found.task && a.id != found.id)
        })
        .flatten();
    let (continuation, package) = match (grant.kind, from) {
        (CommandKind::Continue, Some(from)) => {
            let asked = match state.request(grant.id) {
                Some(EffectRequest::Continue { fidelity, .. }) => *fidelity,
                _ => state.job.policy.allowed_fidelity,
            };
            let package = build_for_continue(journal, state, from.id).map_err(|e| match e {
                BuildError::Journal(e) => PlanError::from(e),
                e => PlanError::Blocked(format!("no work package: {e}")),
            })?;
            (
                Some(ContinuationAsk {
                    from: from.id,
                    allowed: both(state.job.policy.allowed_fidelity, asked),
                }),
                Some(package),
            )
        }
        (CommandKind::Continue, None) => {
            return Err(blocked("a continue's grant continues no attempt"));
        }
        _ => (None, None),
    };
    let snapshot_at = match (found.initial_checkpoint, from) {
        (None, _) => None,
        (Some(_), Some(from)) => {
            let ResourceRef::Node { node } = from.resource;
            Some(node)
        }
        (Some(_), None) => Some(journal.authority()?),
    };
    let request = LaunchRequest {
        token: GrantToken {
            job: state.job.id,
            attempt: found.id,
            generation: found.generation,
        },
        task: found.task,
        intent: state.job.intent.clone(),
        source: state.job.source.clone(),
        policy: state.job.policy.clone(),
        initial_checkpoint: found.initial_checkpoint,
        snapshot_at,
        tasks: task_launches(state, found.task),
        continuation,
        package,
    };
    request
        .check()
        .map_err(|e| PlanError::Blocked(e.to_string()))?;
    Ok(LaunchPlan { request, node })
}

/// What the dispatch reaches nodes through: the launch and instruction
/// of the launch path and the placement probe. The node's own link is
/// [`NodeLink`]; the tests give one that records what it was asked.
pub(crate) trait Link: Send + Sync {
    /// Sends `request` to `target`.
    fn launch(
        &self,
        target: NodeId,
        request: LaunchRequest,
    ) -> BoxFuture<'_, Result<LaunchAnswer, NodeError>>;
    /// Sends `instruction` for `token` to `target`.
    fn instruct(
        &self,
        target: NodeId,
        token: GrantToken,
        instruction: Instruction,
    ) -> BoxFuture<'_, Result<InstructAnswer, NodeError>>;
    /// One probe round and the decision for `job` (ADR 0026).
    fn place<'a>(
        &'a self,
        job: &'a ManagedJob,
        ask: &'a PlacementAsk,
    ) -> BoxFuture<'a, PlacementDecision>;
}

/// The node's link: a peer through its kept session, this node through
/// its own `Service`, under its own id as the peer, so the runner takes
/// a launch from its own authority by the same path as from another.
#[derive(Debug, Clone)]
pub(crate) struct NodeLink(pub Arc<Node>);

fn local_fault(node: NodeId, fault: &fabric_net::wire::Fault) -> NodeError {
    NodeError::Peer {
        peer: node,
        reason: fault.to_string(),
    }
}

impl Link for NodeLink {
    fn launch(
        &self,
        target: NodeId,
        request: LaunchRequest,
    ) -> BoxFuture<'_, Result<LaunchAnswer, NodeError>> {
        Box::pin(async move {
            let node = &self.0;
            if target == node.id {
                return Service::launch(node.as_ref(), node.id, request)
                    .await
                    .map_err(|f| local_fault(node.id, &f));
            }
            node.call(target, |session| {
                let request = request.clone();
                async move { session.launch(request, LAUNCH_DEADLINE).await }
            })
            .await
        })
    }

    fn instruct(
        &self,
        target: NodeId,
        token: GrantToken,
        instruction: Instruction,
    ) -> BoxFuture<'_, Result<InstructAnswer, NodeError>> {
        Box::pin(async move {
            let node = &self.0;
            if target == node.id {
                return Service::instruct(node.as_ref(), node.id, token, instruction)
                    .await
                    .map_err(|f| local_fault(node.id, &f));
            }
            node.call(target, |session| {
                let instruction = instruction.clone();
                async move { session.instruct(token, instruction, LAUNCH_DEADLINE).await }
            })
            .await
        })
    }

    fn place<'a>(
        &'a self,
        job: &'a ManagedJob,
        ask: &'a PlacementAsk,
    ) -> BoxFuture<'a, PlacementDecision> {
        Box::pin(self.0.place_job(job, ask))
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{REMOTE, World};
    use super::*;

    /// A launch names the open build and test tasks with a program, and
    /// leaves to its spawn a task registered while the attempt was live.
    #[tokio::test]
    async fn a_launch_names_the_open_programs_but_not_one_a_spawn_starts() {
        let world = World::new();
        let job = world.created("create").await;
        let idle = world
            .register(job, "idle", TaskClass::Build, &["cargo", "build"])
            .await;
        world.granted(job, REMOTE).await;
        let live = world
            .register(job, "live", TaskClass::Test, &["cargo", "test"])
            .await;
        let state = world.state(job).await;
        let names: Vec<TaskId> = task_launches(&state, state.job.primary)
            .into_iter()
            .map(|t| t.task)
            .collect();
        assert_eq!(names, vec![idle]);
        assert!(spawn_in_flight(&state, live));
    }
}

/// What the dispatch, report and package tests share: an authority on a
/// state directory of its own, the commands and reports that put a job
/// where a test needs it, and a link that records what it was asked.
#[cfg(test)]
pub(crate) mod fixture {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use fabric_core::harness::{HarnessKind, ProgramName};
    use fabric_core::id::{JobId, SnapshotId};
    use fabric_core::managed::launch::LaunchRefusal;
    use fabric_core::managed::placement::PlacementKind;
    use fabric_core::managed::{
        AttemptStatus, CheckpointCut, Command, Envelope, Intent, JobPolicy, Observation,
        OperationId, OperationKey, Program, ReceiptStatus, RestartPolicy, SourceBinding,
        StoredResult, TaskKind, TaskReceipt,
    };
    use fabric_journal::Answer;

    use super::*;
    use crate::authority::Authority;
    use crate::paths::StateDir;

    /// The authority's own node.
    pub(crate) const OWN: NodeId = NodeId::from_bytes([5; 32]);
    /// A peer a grant names.
    pub(crate) const REMOTE: NodeId = NodeId::from_bytes([6; 32]);
    /// Another peer, where a continue lands.
    pub(crate) const OTHER: NodeId = NodeId::from_bytes([7; 32]);

    /// An authority in a directory of its own.
    pub(crate) struct World {
        _dir: tempfile::TempDir,
        pub(crate) authority: Authority,
    }

    fn key(text: &str) -> OperationKey {
        OperationKey::parse(text).unwrap()
    }

    impl World {
        pub(crate) fn new() -> World {
            let dir = tempfile::tempdir().unwrap();
            let state =
                StateDir::resolve(Some(dir.path().as_os_str()), std::path::Path::new("/")).unwrap();
            let authority = Authority::start(&state, OWN).unwrap();
            World {
                _dir: dir,
                authority,
            }
        }

        pub(crate) async fn submit(
            &self,
            job: JobId,
            key_text: &str,
            body: Command,
        ) -> Result<OperationId, String> {
            let answer: Answer = self
                .authority
                .submit(Envelope {
                    key: key(key_text),
                    job: Some(job),
                    expected: None,
                    body,
                })
                .await
                .map_err(|e| e.to_string())?;
            match answer.result {
                StoredResult::Accepted(a) => Ok(a.operation),
                StoredResult::Rejected(e) => Err(e.to_string()),
            }
        }

        /// A job from an empty source, its create waiting for its placement.
        pub(crate) async fn created(&self, key_text: &str) -> JobId {
            let answer = self
                .authority
                .submit(Envelope {
                    key: key(key_text),
                    job: None,
                    expected: None,
                    body: Command::CreateJob {
                        intent: Intent {
                            goal: "make the tests pass".into(),
                            harness: HarnessKind::Acp {
                                program: ProgramName::parse("sh").unwrap(),
                                args: Vec::new(),
                            },
                            acceptance: Vec::new(),
                        },
                        source: SourceBinding::Empty,
                        policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
                    },
                })
                .await
                .unwrap();
            match answer.result {
                StoredResult::Accepted(a) => a.job,
                StoredResult::Rejected(e) => panic!("rejected: {e}"),
            }
        }

        pub(crate) async fn state(&self, job: JobId) -> ManagedState {
            self.authority.job(job).await.unwrap().unwrap()
        }

        pub(crate) fn decision(kind: PlacementKind, node: NodeId) -> PlacementDecision {
            PlacementDecision {
                kind,
                decided_at_ms: 1,
                candidates: Vec::new(),
                chosen: Some(node),
            }
        }

        /// The create placed on `node`: the grant it makes.
        pub(crate) async fn granted(&self, job: JobId, node: NodeId) -> AttemptId {
            let state = self.state(job).await;
            let create = state.operations[0].id;
            self.authority
                .observe_authority(
                    job,
                    Observation::Placed {
                        operation: create,
                        task: state.job.primary,
                        decision: Self::decision(PlacementKind::Initial, node),
                    },
                )
                .await
                .unwrap();
            self.state(job).await.attempts.last().unwrap().id
        }

        pub(crate) async fn started(&self, job: JobId, attempt: AttemptId) {
            let state = self.state(job).await;
            let generation = state.attempt(attempt).unwrap().generation;
            self.authority
                .observe_authority(
                    job,
                    Observation::Started {
                        attempt,
                        generation,
                    },
                )
                .await
                .unwrap();
        }

        /// A child of the primary task with a program.
        pub(crate) async fn register(
            &self,
            job: JobId,
            key_text: &str,
            class: TaskClass,
            argv: &[&str],
        ) -> TaskId {
            let primary = self.state(job).await.job.primary;
            self.submit(
                job,
                key_text,
                Command::RegisterTask {
                    parent: primary,
                    kind: TaskKind::Child {
                        label: key_text.into(),
                    },
                    restart: RestartPolicy::Block,
                    class,
                    program: Some(Program {
                        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                        cwd: None,
                        env_allowlist: Vec::new(),
                    }),
                },
            )
            .await
            .unwrap();
            self.state(job).await.tasks.last().unwrap().id
        }

        /// The cut of `attempt`, every open task interrupted, as `from` reports it.
        pub(crate) async fn cut(&self, job: JobId, attempt: AttemptId, from: NodeId) -> SnapshotId {
            let _ = from;
            let state = self.state(job).await;
            let generation = state.attempt(attempt).unwrap().generation;
            let snapshot = SnapshotId::of(b"the cut");
            let cut = CheckpointCut {
                snapshot,
                receipts: state
                    .tasks
                    .iter()
                    .filter(|t| t.state == TaskState::Open)
                    .map(|t| TaskReceipt {
                        task: t.id,
                        class: t.class,
                        status: if t.class == TaskClass::Harness {
                            ReceiptStatus::Checkpointed { snapshot }
                        } else {
                            ReceiptStatus::Interrupted
                        },
                        exit: None,
                        interrupted: true,
                        workspace: None,
                    })
                    .collect(),
                facts: Vec::new(),
                evidence: Vec::new(),
            };
            self.authority
                .observe_authority(
                    job,
                    Observation::Checkpointed {
                        attempt,
                        generation,
                        snapshot,
                        cut,
                    },
                )
                .await
                .unwrap();
            assert!(matches!(
                self.state(job).await.attempt(attempt).unwrap().status,
                AttemptStatus::Checkpointed { .. } | AttemptStatus::Running
            ));
            snapshot
        }

        /// A continue of the primary task, placed on `node` as a planned
        /// handoff; the new attempt.
        pub(crate) async fn continued_to(&self, job: JobId, node: NodeId) -> AttemptId {
            let primary = self.state(job).await.job.primary;
            let cont = self
                .submit(
                    job,
                    "continue",
                    Command::Continue {
                        task: primary,
                        fidelity: AllowedFidelity::DEFAULT,
                        pin: None,
                    },
                )
                .await
                .unwrap();
            self.authority
                .observe_authority(
                    job,
                    Observation::Placed {
                        operation: cont,
                        task: primary,
                        decision: Self::decision(PlacementKind::PlannedHandoff, node),
                    },
                )
                .await
                .unwrap();
            self.state(job).await.attempts.last().unwrap().id
        }
    }

    /// What a test link was asked, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Asked {
        Launch(NodeId, Box<LaunchRequest>),
        Instruct(NodeId, GrantToken, Instruction),
        Place(PlacementKind),
    }

    /// A link that records what it is asked and answers as a runner
    /// would: a second launch of an attempt is `AlreadyLaunched`.
    pub(crate) struct FakeLink {
        pub(crate) asked: Mutex<Vec<Asked>>,
        launched: Mutex<HashSet<AttemptId>>,
        /// The answer to a first launch.
        pub(crate) launch_answer: Mutex<LaunchAnswer>,
        /// The answer to an instruction.
        pub(crate) instruct_answer: Mutex<InstructAnswer>,
        /// How many instructions fail on the link before one is answered.
        pub(crate) instruct_failures: Mutex<u32>,
        /// Where a placement lands.
        pub(crate) place_on: Mutex<Option<NodeId>>,
        /// The pin of the job each placement was asked with, in order.
        pub(crate) placed_pins: Mutex<Vec<Option<NodeId>>>,
    }

    impl FakeLink {
        pub(crate) fn new() -> FakeLink {
            FakeLink {
                asked: Mutex::new(Vec::new()),
                launched: Mutex::new(HashSet::new()),
                launch_answer: Mutex::new(LaunchAnswer::Accepted),
                instruct_answer: Mutex::new(InstructAnswer::Accepted),
                instruct_failures: Mutex::new(0),
                place_on: Mutex::new(None),
                placed_pins: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn asked(&self) -> Vec<Asked> {
            self.asked.lock().unwrap().clone()
        }

        pub(crate) fn launches(&self) -> Vec<(NodeId, LaunchRequest)> {
            self.asked()
                .into_iter()
                .filter_map(|a| match a {
                    Asked::Launch(node, request) => Some((node, *request)),
                    _ => None,
                })
                .collect()
        }

        pub(crate) fn instructions(&self) -> Vec<(NodeId, GrantToken, Instruction)> {
            self.asked()
                .into_iter()
                .filter_map(|a| match a {
                    Asked::Instruct(node, token, instruction) => Some((node, token, instruction)),
                    _ => None,
                })
                .collect()
        }

        /// How many attempts were started, whatever was sent.
        pub(crate) fn started(&self) -> usize {
            self.launched.lock().unwrap().len()
        }
    }

    impl Link for FakeLink {
        fn launch(
            &self,
            target: NodeId,
            request: LaunchRequest,
        ) -> BoxFuture<'_, Result<LaunchAnswer, NodeError>> {
            let attempt = request.token.attempt;
            self.asked
                .lock()
                .unwrap()
                .push(Asked::Launch(target, Box::new(request)));
            let answer = if self.launched.lock().unwrap().contains(&attempt) {
                LaunchAnswer::Refused {
                    reason: LaunchRefusal::AlreadyLaunched,
                }
            } else {
                let answer = self.launch_answer.lock().unwrap().clone();
                if answer == LaunchAnswer::Accepted {
                    self.launched.lock().unwrap().insert(attempt);
                }
                answer
            };
            Box::pin(async move { Ok(answer) })
        }

        fn instruct(
            &self,
            target: NodeId,
            token: GrantToken,
            instruction: Instruction,
        ) -> BoxFuture<'_, Result<InstructAnswer, NodeError>> {
            self.asked
                .lock()
                .unwrap()
                .push(Asked::Instruct(target, token, instruction));
            let mut failures = self.instruct_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Box::pin(async move {
                    Err(NodeError::Io {
                        path: std::path::PathBuf::from("link"),
                        source: std::io::Error::other("the link is down"),
                    })
                });
            }
            drop(failures);
            let answer = self.instruct_answer.lock().unwrap().clone();
            Box::pin(async move { Ok(answer) })
        }

        fn place<'a>(
            &'a self,
            job: &'a ManagedJob,
            ask: &'a PlacementAsk,
        ) -> BoxFuture<'a, PlacementDecision> {
            self.placed_pins
                .lock()
                .unwrap()
                .push(job.policy.placement.pin);
            self.asked.lock().unwrap().push(Asked::Place(ask.kind));
            let chosen = *self.place_on.lock().unwrap();
            let kind = ask.kind;
            Box::pin(async move {
                PlacementDecision {
                    kind,
                    decided_at_ms: 2,
                    candidates: Vec::new(),
                    chosen,
                }
            })
        }
    }
}
