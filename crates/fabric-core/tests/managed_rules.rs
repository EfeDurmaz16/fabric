//! The rules of managed jobs (ADR 0019): identity, generations, the
//! replay rules that live in the domain, and replay equals projection
//! over random command and observation sequences.

#![allow(
    clippy::unwrap_used,
    reason = "the helpers here are test code; a failed unwrap is the test failing"
)]

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::*;
use fabric_core::path::AbsPath;
use proptest::prelude::*;

const NODE: NodeId = NodeId::from_bytes([9; 32]);

/// Hands out fresh ids, as the authority does.
struct Ids(u8);

impl Ids {
    fn next(&mut self) -> NewIds {
        self.0 = self.0.wrapping_add(1);
        let n = self.0;
        NewIds {
            job: JobId::from_bytes([1; 16]),
            task: TaskId::from_bytes([n; 16]),
            attempt: AttemptId::from_bytes([n; 16]),
            operation: OperationId::from_bytes([n; 16]),
            at_ms: u64::from(n),
        }
    }
}

/// A job and its whole history, kept the way the journal keeps them.
struct Kept {
    state: Option<ManagedState>,
    history: Vec<(WorkRevision, ManagedEvent)>,
    ids: Ids,
}

impl Kept {
    fn new() -> Kept {
        Kept {
            state: None,
            history: Vec::new(),
            ids: Ids(0),
        }
    }

    fn commit(&mut self, t: Transition) -> Transition {
        if !t.events.is_empty() {
            self.state = Some(apply_batch(self.state.take(), t.revision, &t.events).unwrap());
            self.history
                .extend(t.events.iter().cloned().map(|e| (t.revision, e)));
        }
        t
    }

    fn submit(&mut self, key: &str, body: Command) -> Result<Transition, DomainError> {
        let envelope = Envelope {
            key: OperationKey::parse(key).unwrap(),
            job: self.state.as_ref().map(|s| s.job.id),
            expected: None,
            body,
        };
        let ids = self.ids.next();
        accept_command(self.state.as_ref(), &envelope, &ids).map(|t| self.commit(t))
    }

    fn observe(&mut self, observation: &Observation) -> Result<Transition, DomainError> {
        apply_observation(self.state.as_ref().unwrap(), observation).map(|t| self.commit(t))
    }

    fn acknowledge(&mut self, ack: &CheckpointAck) -> Result<Transition, DomainError> {
        acknowledge(self.state.as_ref().unwrap(), ack).map(|t| self.commit(t))
    }

    fn mark(
        &mut self,
        operation: OperationId,
        mark: OperationMark,
    ) -> Result<Transition, DomainError> {
        mark_operation(self.state.as_ref().unwrap(), operation, mark).map(|t| self.commit(t))
    }

    fn state(&self) -> &ManagedState {
        self.state.as_ref().unwrap()
    }

    fn replays(&self) {
        let replayed = replay(self.state().job.id, &self.history).unwrap();
        assert_eq!(&replayed, self.state());
    }
}

fn create() -> Command {
    Command::CreateJob {
        intent: Intent {
            goal: "make the greeting louder".into(),
            harness: HarnessKind::ClaudeCode,
            acceptance: Vec::new(),
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
    }
}

fn cut(snapshot: SnapshotId) -> CheckpointCut {
    CheckpointCut {
        snapshot,
        receipts: Vec::new(),
        facts: Vec::new(),
        evidence: Vec::new(),
    }
}

fn start(task: TaskId) -> Command {
    Command::StartAttempt {
        task,
        resource: ResourceRef::Node { node: NODE },
        initial_checkpoint: None,
    }
}

fn made() -> Kept {
    let mut kept = Kept::new();
    kept.submit("create", create()).unwrap();
    kept
}

#[test]
fn a_new_job_has_its_primary_task_at_revision_one() {
    let kept = made();
    let s = kept.state();
    assert_eq!(s.job.revision, WorkRevision::FIRST);
    assert_eq!(s.job.state, JobState::Active);
    assert_eq!(s.tasks.len(), 1);
    assert_eq!(s.tasks[0].id, s.job.primary);
    assert_eq!(s.tasks[0].kind, TaskKind::Primary);
    assert_eq!(s.tasks[0].class, TaskClass::Harness);
    // M5: the create's effect is the initial placement, so it stays
    // accepted until a `Placed` observation grants or parks.
    assert_eq!(s.operations[0].status, OperationStatus::Accepted);
    assert!(s.parked());
    kept.replays();
}

#[test]
fn a_worker_loss_is_not_a_cancel_and_does_not_grant_again() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let first = kept.state().attempts[0].clone();
    assert_eq!(first.generation, Generation::FIRST);
    kept.mark(first.granted_by, OperationMark::Dispatched)
        .unwrap();
    kept.observe(&Observation::Started {
        attempt: first.id,
        generation: first.generation,
    })
    .unwrap();
    assert_eq!(
        kept.state().operation(first.granted_by).unwrap().status,
        OperationStatus::Succeeded
    );
    kept.observe(&Observation::Lost {
        attempt: first.id,
        generation: first.generation,
    })
    .unwrap();
    let s = kept.state();
    assert_eq!(s.attempts[0].status, AttemptStatus::Lost);
    assert_eq!(s.task(primary).unwrap().state, TaskState::Open);
    assert_eq!(s.job.state, JobState::Active);
    assert_eq!(
        s.attempts.len(),
        1,
        "a loss granted a new attempt by itself"
    );

    // Only an explicit command grants again, at the next generation.
    kept.submit("start-2", start(primary)).unwrap();
    let second = kept.state().attempts[1].clone();
    assert_eq!(second.generation, Generation::new(2).unwrap());
    assert_ne!(second.id, first.id);
    kept.replays();
}

#[test]
fn no_second_grant_while_an_attempt_may_run_even_after_a_timeout() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let op = kept.state().attempts[0].granted_by;
    kept.mark(op, OperationMark::Dispatched).unwrap();
    // A timeout leaves the dispatch unknown; it is not permission to replay.
    kept.observe(&Observation::Unknown { operation: op })
        .unwrap();
    assert_eq!(
        kept.state().operation(op).unwrap().status,
        OperationStatus::Unknown
    );
    assert!(matches!(
        kept.submit("start-2", start(primary)),
        Err(DomainError::AttemptStillLive { .. })
    ));
    assert!(matches!(
        kept.submit(
            "finish",
            Command::FinishTask {
                task: primary,
                outcome: TaskOutcome::Succeeded
            }
        ),
        Err(DomainError::AttemptStillLive { .. })
    ));
    // M5: a cancel with a live attempt is accepted and asks the executor
    // to stop it; the job is not cancelled until the attempt is seen ended.
    let cancel = kept
        .submit("cancel", Command::CancelJob { reason: "x".into() })
        .unwrap();
    assert!(matches!(
        cancel.events[1],
        ManagedEvent::EffectRequested {
            request: EffectRequest::CancelJob { .. },
            ..
        }
    ));
    assert_eq!(kept.state().job.state, JobState::Active);
    // Dispatching again is refused: the operation is past accepted.
    assert!(matches!(
        kept.mark(op, OperationMark::Dispatched),
        Err(DomainError::OperationState { .. })
    ));
    kept.replays();
}

#[test]
fn an_observation_at_an_older_generation_changes_nothing_but_is_recorded() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let old = kept.state().attempts[0].clone();
    kept.observe(&Observation::Lost {
        attempt: old.id,
        generation: old.generation,
    })
    .unwrap();
    kept.submit("start-2", start(primary)).unwrap();
    let before = kept.state().clone();
    let t = kept
        .observe(&Observation::Exited {
            attempt: old.id,
            generation: old.generation,
            outcome: ExitOutcome::Succeeded,
        })
        .unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::StaleObservationRefused { .. }]
    ));
    let after = kept.state();
    assert_eq!(after.attempts, before.attempts);
    assert_eq!(after.tasks, before.tasks);
    // A report that claims the current generation for the old attempt is refused outright.
    assert!(matches!(
        kept.observe(&Observation::Started {
            attempt: old.id,
            generation: Generation::new(2).unwrap(),
        }),
        Err(DomainError::StaleGeneration { .. })
    ));
    // So is one from the future.
    assert!(matches!(
        kept.observe(&Observation::Started {
            attempt: old.id,
            generation: Generation::new(3).unwrap(),
        }),
        Err(DomainError::StaleGeneration { .. })
    ));
    kept.replays();
}

#[test]
fn a_repeated_report_changes_nothing_and_an_ended_attempt_stays_ended() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let a = kept.state().attempts[0].clone();
    let started = Observation::Started {
        attempt: a.id,
        generation: a.generation,
    };
    assert!(!kept.observe(&started).unwrap().events.is_empty());
    let revision = kept.state().job.revision;
    assert!(kept.observe(&started).unwrap().events.is_empty());
    assert_eq!(kept.state().job.revision, revision);
    let snapshot = SnapshotId::of(b"manifest");
    kept.observe(&Observation::Checkpointed {
        attempt: a.id,
        generation: a.generation,
        snapshot,
        cut: cut(snapshot),
    })
    .unwrap();
    assert!(matches!(
        kept.observe(&Observation::Lost {
            attempt: a.id,
            generation: a.generation
        }),
        Err(DomainError::AttemptEnded { .. })
    ));
    kept.replays();
}

#[test]
fn the_expected_revision_is_checked_and_closed_jobs_take_no_command() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    let wrong = Envelope {
        key: OperationKey::parse("k").unwrap(),
        job: Some(kept.state().job.id),
        expected: Some(WorkRevision::new(7).unwrap()),
        body: start(primary),
    };
    let ids = kept.ids.next();
    assert_eq!(
        accept_command(Some(kept.state()), &wrong, &ids),
        Err(DomainError::RevisionMismatch {
            expected: WorkRevision::new(7).unwrap(),
            current: WorkRevision::FIRST
        })
    );
    kept.submit(
        "finish",
        Command::FinishTask {
            task: primary,
            outcome: TaskOutcome::Failed,
        },
    )
    .unwrap();
    assert_eq!(kept.state().job.state, JobState::Finished);
    assert!(matches!(
        kept.submit("start", start(primary)),
        Err(DomainError::JobClosed { .. })
    ));
    kept.replays();
}

#[test]
fn a_create_names_no_job_and_other_commands_name_theirs() {
    let kept = made();
    let mut ids = Ids(100);
    let named = Envelope {
        key: OperationKey::parse("c").unwrap(),
        job: Some(JobId::from_bytes([3; 16])),
        expected: None,
        body: create(),
    };
    assert_eq!(
        accept_command(None, &named, &ids.next()),
        Err(DomainError::BadEnvelope)
    );
    let other = Envelope {
        key: OperationKey::parse("c").unwrap(),
        job: Some(JobId::from_bytes([3; 16])),
        expected: None,
        body: Command::CancelJob { reason: "x".into() },
    };
    assert_eq!(
        accept_command(Some(kept.state()), &other, &ids.next()),
        Err(DomainError::BadEnvelope)
    );
    assert_eq!(
        accept_command(None, &other, &ids.next()),
        Err(DomainError::UnknownJob {
            job: JobId::from_bytes([3; 16])
        })
    );
}

#[test]
fn child_tasks_need_an_open_parent_and_cancel_closes_open_tasks() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit(
        "child",
        Command::RegisterTask {
            parent: primary,
            kind: TaskKind::Child {
                label: "tests".into(),
            },
            restart: RestartPolicy::Block,
            class: TaskClass::Test,
        },
    )
    .unwrap();
    assert!(matches!(
        kept.submit(
            "second-primary",
            Command::RegisterTask {
                parent: primary,
                kind: TaskKind::Primary,
                restart: RestartPolicy::Block,
                class: TaskClass::Harness,
            }
        ),
        Err(DomainError::BadParent { .. })
    ));
    kept.submit(
        "cancel",
        Command::CancelJob {
            reason: "done".into(),
        },
    )
    .unwrap();
    let s = kept.state();
    assert_eq!(s.job.state, JobState::Cancelled);
    assert!(s.tasks.iter().all(|t| t.state == TaskState::Cancelled));
    kept.replays();
}

#[test]
fn dedupe_tells_a_retry_from_a_conflict() {
    let envelope = Envelope {
        key: OperationKey::parse("k").unwrap(),
        job: None,
        expected: None,
        body: create(),
    };
    let digest = request_digest(&envelope).unwrap();
    let stored = Operation {
        id: OperationId::from_bytes([1; 16]),
        key: envelope.key.clone(),
        job: None,
        expected_revision: None,
        digest,
        status: OperationStatus::Succeeded,
    };
    assert_eq!(dedupe(None, &digest), Dedupe::New);
    assert_eq!(dedupe(Some(&stored), &digest), Dedupe::Replay);
    let other = Envelope {
        body: Command::CancelJob { reason: "x".into() },
        ..envelope.clone()
    };
    let other_digest = request_digest(&other).unwrap();
    assert_ne!(other_digest, digest);
    assert_eq!(dedupe(Some(&stored), &other_digest), Dedupe::Conflict);
    // The key is not part of the digest; the expected revision is.
    let rekeyed = Envelope {
        key: OperationKey::parse("other").unwrap(),
        ..envelope.clone()
    };
    assert_eq!(request_digest(&rekeyed).unwrap(), digest);
    let expecting = Envelope {
        expected: Some(WorkRevision::FIRST),
        ..envelope
    };
    assert_ne!(request_digest(&expecting).unwrap(), digest);
}

#[test]
fn frames_and_answers_have_one_byte_form() {
    let kept = made();
    let frame = EventFrame {
        job: kept.state().job.id,
        revision: WorkRevision::FIRST,
        operation: Some(OperationId::from_bytes([1; 16])),
        prev: None,
        at_ms: 5,
        event: kept.history[1].1.clone(),
    };
    let bytes = frame.encode().unwrap();
    assert_eq!(bytes[0], MANAGED_FORMAT);
    assert_eq!(EventFrame::decode(&bytes).unwrap(), frame);
    let mut other = bytes.clone();
    other[0] = 3;
    assert!(matches!(
        EventFrame::decode(&other),
        Err(FrameError::UnknownFormat(3))
    ));
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(EventFrame::decode(&trailing).is_err());
    assert!(matches!(EventFrame::decode(&[]), Err(FrameError::Empty)));

    let answer = StoredResult::Rejected(DomainError::RevisionMismatch {
        expected: WorkRevision::new(2).unwrap(),
        current: WorkRevision::FIRST,
    });
    let bytes = answer.encode().unwrap();
    assert_eq!(StoredResult::decode(&bytes).unwrap(), answer);
}

#[test]
fn a_broken_history_does_not_replay() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let job = kept.state().job.id;
    // A gap in revisions.
    let mut gap = kept.history.clone();
    for (r, _) in gap.iter_mut().skip(3) {
        *r = WorkRevision::new(3).unwrap();
    }
    assert!(matches!(replay(job, &gap), Err(DomainError::Broken { .. })));
    // A grant at a generation that skips one.
    let mut skipped = kept.history.clone();
    let grant = skipped
        .iter_mut()
        .rev()
        .find_map(|(_, e)| match e {
            ManagedEvent::AttemptGranted { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    *grant = Generation::new(2).unwrap();
    assert!(matches!(
        replay(job, &skipped),
        Err(DomainError::Broken { .. })
    ));
    // Another job's history.
    assert!(replay(JobId::from_bytes([2; 16]), &kept.history).is_err());
    assert!(replay(job, &[]).is_err());
}

/// One step of a random run.
#[derive(Debug, Clone)]
enum Step {
    Start,
    Dispatch,
    Unknown,
    Observe(u8, u8),
    Child,
    Finish(u8),
    Cancel,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::Start),
        2 => Just(Step::Dispatch),
        1 => Just(Step::Unknown),
        4 => (0u8..4, 0u8..6).prop_map(|(a, k)| Step::Observe(a, k)),
        1 => Just(Step::Child),
        1 => (0u8..3).prop_map(Step::Finish),
        1 => Just(Step::Cancel),
    ]
}

proptest! {
    /// Whatever commands and reports arrive, in whatever order, the state
    /// the authority keeps is the replay of its history; no task has two
    /// attempts that may run; generations are unique and grow by one; and
    /// a loss never closes a task.
    #[test]
    fn replay_equals_projection(steps in proptest::collection::vec(step(), 0..40)) {
        let mut kept = made();
        let mut n = 0u32;
        for step in steps {
            n += 1;
            let key = format!("k{n}");
            let s = kept.state().clone();
            let _ = match step {
                Step::Start => kept.submit(&key, start(s.job.primary)),
                Step::Dispatch => match s.operations.iter().find(|o| o.status == OperationStatus::Accepted) {
                    Some(op) => kept.mark(op.id, OperationMark::Dispatched),
                    None => continue,
                },
                Step::Unknown => match s.operations.iter().find(|o| o.status == OperationStatus::Dispatched) {
                    Some(op) => kept.observe(&Observation::Unknown { operation: op.id }),
                    None => continue,
                },
                Step::Observe(which, kind) => {
                    let Some(a) = s.attempts.get(usize::from(which) % s.attempts.len().max(1)) else { continue };
                    let (attempt, generation) = (a.id, a.generation);
                    let observation = match kind {
                        0 => Observation::Started { attempt, generation },
                        1 => Observation::Lost { attempt, generation },
                        2 => Observation::Exited { attempt, generation, outcome: ExitOutcome::Succeeded },
                        3 => Observation::Exited { attempt, generation, outcome: ExitOutcome::Failed { code: Some(1) } },
                        4 => Observation::Checkpointed { attempt, generation, snapshot: SnapshotId::of(&[which]), cut: cut(SnapshotId::of(&[which])) },
                        _ => Observation::Exited { attempt, generation, outcome: ExitOutcome::Cancelled },
                    };
                    let lost = matches!(observation, Observation::Lost { .. });
                    let before = kept.state().tasks.clone();
                    let out = kept.observe(&observation);
                    if lost {
                        prop_assert_eq!(&kept.state().tasks, &before);
                    }
                    out
                }
                Step::Child => kept.submit(&key, Command::RegisterTask {
                    parent: s.job.primary,
                    kind: TaskKind::Child { label: key.clone() },
                    restart: RestartPolicy::Block,
                    class: TaskClass::Build,
                }),
                Step::Finish(which) => {
                    let task = s.tasks[usize::from(which) % s.tasks.len()].id;
                    kept.submit(&key, Command::FinishTask { task, outcome: TaskOutcome::Succeeded })
                }
                Step::Cancel => kept.submit(&key, Command::CancelJob { reason: key.clone() }),
            };
            let s = kept.state();
            prop_assert!(s.attempts.iter().filter(|a| a.status.may_run()).count() <= 1);
            for (i, a) in s.attempts.iter().enumerate() {
                prop_assert_eq!(a.generation.get(), i as u64 + 1);
            }
        }
        kept.replays();
    }
}

// M6 (ADR 0028): acknowledged checkpoints, the fence, loss as
// availability, supersede and regrant, and the recovery policy.

const OTHER_NODE: NodeId = NodeId::from_bytes([7; 32]);

/// A job whose first attempt is running on `NODE`.
fn running() -> Kept {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let first = kept.state().attempts[0].clone();
    kept.mark(first.granted_by, OperationMark::Dispatched)
        .unwrap();
    kept.observe(&Observation::Started {
        attempt: first.id,
        generation: first.generation,
    })
    .unwrap();
    kept
}

fn snapshot(n: u8) -> SnapshotId {
    SnapshotId::of(&[n; 4])
}

fn cut_digest(n: u8) -> CutDigest {
    CutDigest::from_bytes([n; 32])
}

fn capture(kept: &mut Kept, n: u8) -> Result<Transition, DomainError> {
    let a = kept.state().live_attempt().unwrap().clone();
    kept.observe(&Observation::Captured {
        attempt: a.id,
        generation: a.generation,
        snapshot: snapshot(n),
        cut: cut_digest(n),
    })
}

fn ack_of(kept: &Kept, n: u8) -> CheckpointAck {
    let c = kept.state().checkpoint(snapshot(n)).unwrap();
    CheckpointAck {
        attempt: c.attempt,
        generation: c.generation,
        snapshot: c.snapshot,
        cut: c.cut,
        manifests: 1,
        chunks: 2,
        bytes: 300,
        fetched_from: NODE,
    }
}

/// The running attempt goes silent: `unknown` by `ResourceUnavailable`.
fn gone_silent(kept: &mut Kept) -> Attempt {
    let a = kept.state().live_attempt().unwrap().clone();
    kept.observe(&Observation::ResourceUnavailable {
        attempt: a.id,
        generation: a.generation,
        last_heartbeat_ms: Some(40),
        at_ms: 50,
    })
    .unwrap();
    kept.state().attempt(a.id).unwrap().clone()
}

fn token(a: &Attempt) -> GrantToken {
    GrantToken {
        job: a.job,
        attempt: a.id,
        generation: a.generation,
    }
}

#[test]
fn a_captured_checkpoint_restarts_nothing_until_acknowledged() {
    let mut kept = running();
    let t = capture(&mut kept, 1).unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::CheckpointCaptured { .. }]
    ));
    let a = kept.state().attempts[0].clone();
    assert_eq!(
        a.status,
        AttemptStatus::Running,
        "the attempt keeps running"
    );
    assert_eq!(kept.state().acknowledged(), Vec::<&CheckpointAck>::new());
    assert_eq!(
        kept.state().captured_only(),
        vec![(a.id, a.generation, snapshot(1))]
    );
    // The same capture again changes nothing; another cut for the same
    // snapshot is refused.
    assert!(capture(&mut kept, 1).unwrap().events.is_empty());
    assert!(matches!(
        kept.observe(&Observation::Captured {
            attempt: a.id,
            generation: a.generation,
            snapshot: snapshot(1),
            cut: cut_digest(9),
        }),
        Err(DomainError::UnknownCheckpoint { .. })
    ));
    // Captured but not acknowledged: no restart point.
    let a = gone_silent(&mut kept);
    assert!(matches!(
        kept.submit(
            "recover-1",
            Command::Recover {
                attempt: a.id,
                resource: ResourceRef::Node { node: OTHER_NODE },
                from: snapshot(1),
                reason: "test".into(),
            }
        ),
        Err(DomainError::NotAcknowledged { .. })
    ));
    let decision = recovery_decision(
        kept.state(),
        a.id,
        OutcomePolicy::Checkpoint,
        &[],
        Ok(ResourceRef::Node { node: OTHER_NODE }),
    );
    assert_eq!(decision, Err(RecoveryBlock::NoAcknowledgedCheckpoint));
    // Acknowledged, and it is one.
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    assert_eq!(kept.state().acknowledged().len(), 1);
    assert!(kept.state().captured_only().is_empty());
    assert!(matches!(
        recovery_decision(
            kept.state(),
            a.id,
            OutcomePolicy::Checkpoint,
            &[],
            Ok(ResourceRef::Node { node: OTHER_NODE }),
        ),
        Ok(Command::Recover { from, .. }) if from == snapshot(1)
    ));
    kept.replays();
}

#[test]
fn an_acknowledgement_names_the_reported_snapshot_and_cut() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    let good = ack_of(&kept, 1);
    let unknown = CheckpointAck {
        snapshot: snapshot(2),
        ..good.clone()
    };
    assert!(matches!(
        kept.acknowledge(&unknown),
        Err(DomainError::UnknownCheckpoint { .. })
    ));
    let other_cut = CheckpointAck {
        cut: cut_digest(2),
        ..good.clone()
    };
    assert!(matches!(
        kept.acknowledge(&other_cut),
        Err(DomainError::CutMismatch { .. })
    ));
    let other_generation = CheckpointAck {
        generation: Generation::new(2).unwrap(),
        ..good.clone()
    };
    assert!(matches!(
        kept.acknowledge(&other_generation),
        Err(DomainError::StaleGeneration { .. })
    ));
    let t = kept.acknowledge(&good).unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::CheckpointAcknowledged { .. }]
    ));
    assert!(kept.acknowledge(&good).unwrap().events.is_empty());
    // An attempt that went silent after capturing still owns its checkpoint.
    capture(&mut kept, 3).unwrap();
    gone_silent(&mut kept);
    kept.acknowledge(&ack_of(&kept, 3)).unwrap();
    assert_eq!(kept.state().acknowledged()[0].snapshot, snapshot(3));
    kept.replays();
}

#[test]
fn unavailable_is_unknown_and_still_may_run() {
    let mut kept = running();
    let a = gone_silent(&mut kept);
    assert_eq!(a.status, AttemptStatus::Unknown);
    assert_eq!(a.unavailable_since_ms, Some(50));
    assert!(a.status.may_run());
    // The M2 wait rule holds: nothing grants behind it, and a cancel only
    // asks the executor to stop it first (M5): the job is not cancelled
    // and the attempt stays `unknown`, never taken for stopped.
    let primary = kept.state().job.primary;
    assert!(matches!(
        kept.submit("start-2", start(primary)),
        Err(DomainError::AttemptStillLive { .. })
    ));
    let cancel = kept
        .submit(
            "cancel",
            Command::CancelJob {
                reason: "no".into(),
            },
        )
        .unwrap();
    assert!(
        cancel
            .events
            .iter()
            .any(|e| matches!(e, ManagedEvent::EffectRequested { .. }))
    );
    assert!(
        !cancel
            .events
            .iter()
            .any(|e| matches!(e, ManagedEvent::JobCancelled { .. }))
    );
    assert_eq!(kept.state().job.state, JobState::Active);
    assert_eq!(
        kept.state().attempt(a.id).unwrap().status,
        AttemptStatus::Unknown
    );
    // Unavailable again changes nothing; it never becomes lost by itself.
    assert!(
        kept.observe(&Observation::ResourceUnavailable {
            attempt: a.id,
            generation: a.generation,
            last_heartbeat_ms: Some(40),
            at_ms: 60,
        })
        .unwrap()
        .events
        .is_empty()
    );
    assert_eq!(
        kept.state().attempt(a.id).unwrap().status,
        AttemptStatus::Unknown
    );
    kept.replays();
}

#[test]
fn reachable_returns_only_an_attempt_unavailable_made_unknown() {
    let mut kept = running();
    let a = kept.state().attempts[0].clone();
    // Reachable while running: nothing.
    assert!(
        kept.observe(&Observation::ResourceReachable {
            attempt: a.id,
            generation: a.generation,
        })
        .unwrap()
        .events
        .is_empty()
    );
    gone_silent(&mut kept);
    let t = kept
        .observe(&Observation::ResourceReachable {
            attempt: a.id,
            generation: a.generation,
        })
        .unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [ManagedEvent::AttemptReachable { .. }]
    ));
    let back = kept.state().attempt(a.id).unwrap();
    assert_eq!(back.status, AttemptStatus::Running);
    assert_eq!(back.unavailable_since_ms, None);
    // An ended attempt is neither.
    kept.observe(&Observation::Exited {
        attempt: a.id,
        generation: a.generation,
        outcome: ExitOutcome::Succeeded,
    })
    .unwrap();
    assert!(matches!(
        kept.observe(&Observation::ResourceUnavailable {
            attempt: a.id,
            generation: a.generation,
            last_heartbeat_ms: None,
            at_ms: 70,
        }),
        Err(DomainError::AttemptEnded { .. })
    ));
    kept.replays();
}

#[test]
fn supersede_needs_an_unknown_attempt() {
    let mut kept = running();
    let a = kept.state().attempts[0].clone();
    let supersede = |attempt| Command::Supersede {
        attempt,
        reason: "operator".into(),
    };
    assert!(matches!(
        kept.submit("s-1", supersede(a.id)),
        Err(DomainError::AttemptReachable { .. })
    ));
    assert!(matches!(
        kept.submit("s-2", supersede(AttemptId::from_bytes([0xee; 16]))),
        Err(DomainError::UnknownAttempt { .. })
    ));
    gone_silent(&mut kept);
    let t = kept.submit("s-3", supersede(a.id)).unwrap();
    assert!(matches!(
        t.events.as_slice(),
        [
            ManagedEvent::CommandAccepted { .. },
            ManagedEvent::GrantSuperseded { by: None, .. }
        ]
    ));
    let s = kept.state();
    assert_eq!(
        s.attempt(a.id).unwrap().status,
        AttemptStatus::Superseded { by: None }
    );
    assert_eq!(
        s.job.generation, a.generation,
        "the generation did not move"
    );
    assert_eq!(s.tasks[0].state, TaskState::Open);
    assert!(s.live_attempt().is_none());
    // The fence refuses it from now on, and its late exit is refused too.
    assert_eq!(
        fence(Some(s), &token(&a), Some(NODE)),
        FenceVerdict::Superseded { by: None }
    );
    assert!(matches!(
        kept.observe(&Observation::Exited {
            attempt: a.id,
            generation: a.generation,
            outcome: ExitOutcome::Succeeded,
        }),
        Err(DomainError::AttemptEnded { .. })
    ));
    // A plain StartAttempt is admitted now, by the M2 rules.
    let primary = kept.state().job.primary;
    kept.submit("start-2", start(primary)).unwrap();
    assert_eq!(kept.state().job.generation.get(), 2);
    kept.replays();
}

#[test]
fn recover_grants_the_next_generation_from_an_acknowledged_checkpoint_in_one_batch() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    let a = kept.state().attempts[0].clone();
    let recover = Command::Recover {
        attempt: a.id,
        resource: ResourceRef::Node { node: OTHER_NODE },
        from: snapshot(1),
        reason: "the laptop lid".into(),
    };
    assert!(matches!(
        kept.submit("r-0", recover.clone()),
        Err(DomainError::AttemptReachable { .. })
    ));
    gone_silent(&mut kept);
    let t = kept.submit("r-1", recover).unwrap();
    assert_eq!(t.events.len(), 3);
    assert!(matches!(
        t.events.as_slice(),
        [
            ManagedEvent::CommandAccepted {
                kind: CommandKind::Recover,
                ..
            },
            ManagedEvent::GrantSuperseded { by: Some(_), .. },
            ManagedEvent::AttemptGranted {
                initial_checkpoint: Some(_),
                ..
            }
        ]
    ));
    let s = kept.state();
    let next = s.attempts[1].clone();
    assert_eq!(next.generation.get(), 2);
    assert_eq!(s.job.generation, next.generation);
    assert_eq!(next.initial_checkpoint, Some(snapshot(1)));
    assert_eq!(next.resource, ResourceRef::Node { node: OTHER_NODE });
    assert_eq!(
        s.attempt(a.id).unwrap().status,
        AttemptStatus::Superseded {
            by: Some(next.generation)
        }
    );
    assert_eq!(s.live_attempt().map(|l| l.id), Some(next.id));
    // The recover is a grant: it waits for its one dispatch.
    assert!(s.grant_may_dispatch(next.granted_by));
    assert_eq!(
        s.operation(next.granted_by).unwrap().status,
        OperationStatus::Accepted
    );
    kept.mark(next.granted_by, OperationMark::Dispatched)
        .unwrap();
    // The old attempt's late report is stale and recorded as such.
    let late = kept
        .observe(&Observation::Exited {
            attempt: a.id,
            generation: a.generation,
            outcome: ExitOutcome::Succeeded,
        })
        .unwrap();
    assert!(matches!(
        late.events.as_slice(),
        [ManagedEvent::StaleObservationRefused { .. }]
    ));
    assert_eq!(
        fence(Some(kept.state()), &token(&a), Some(NODE)),
        FenceVerdict::Superseded {
            by: Some(next.generation)
        }
    );
    kept.replays();
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "every status, generation and node is walked in one place"
)]
fn fence_only_the_live_attempt_from_its_node_is_live() {
    let mut kept = made();
    let job = kept.state().job.id;
    let ghost = GrantToken {
        job,
        attempt: AttemptId::from_bytes([0xee; 16]),
        generation: Generation::FIRST,
    };
    assert_eq!(
        fence(Some(kept.state()), &ghost, None),
        FenceVerdict::UnknownAttempt
    );
    assert_eq!(
        fence(
            Some(kept.state()),
            &GrantToken {
                job: JobId::from_bytes([0xdd; 16]),
                ..ghost
            },
            None
        ),
        FenceVerdict::UnknownJob
    );
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let a = kept.state().attempts[0].clone();
    // Granted: live from its node and from the authority's own host.
    assert_eq!(
        fence(Some(kept.state()), &token(&a), None),
        FenceVerdict::Live
    );
    assert_eq!(
        fence(Some(kept.state()), &token(&a), Some(NODE)),
        FenceVerdict::Live
    );
    assert_eq!(
        fence(Some(kept.state()), &token(&a), Some(OTHER_NODE)),
        FenceVerdict::WrongNode {
            granted: NODE,
            from: OTHER_NODE
        }
    );
    // A token at another generation for the same attempt is not its token.
    let wrong_generation = GrantToken {
        generation: Generation::new(2).unwrap(),
        ..token(&a)
    };
    assert_eq!(
        fence(Some(kept.state()), &wrong_generation, None),
        FenceVerdict::UnknownAttempt
    );
    kept.mark(a.granted_by, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Started {
        attempt: a.id,
        generation: a.generation,
    })
    .unwrap();
    assert_eq!(
        fence(Some(kept.state()), &token(&a), Some(NODE)),
        FenceVerdict::Live
    );
    // Unknown: not writable, not obsolete.
    gone_silent(&mut kept);
    let v = fence(Some(kept.state()), &token(&a), Some(NODE));
    assert_eq!(
        v,
        FenceVerdict::NotWritable {
            status: AttemptStatus::Unknown
        }
    );
    assert!(!v.is_obsolete());
    // Lost, and then a later grant: stale.
    kept.observe(&Observation::Lost {
        attempt: a.id,
        generation: a.generation,
    })
    .unwrap();
    assert_eq!(
        fence(Some(kept.state()), &token(&a), Some(NODE)),
        FenceVerdict::NotWritable {
            status: AttemptStatus::Lost
        }
    );
    kept.submit("start-2", start(primary)).unwrap();
    let v = fence(Some(kept.state()), &token(&a), Some(NODE));
    assert_eq!(
        v,
        FenceVerdict::Stale {
            current: Generation::new(2).unwrap()
        }
    );
    assert!(v.is_obsolete());
    let b = kept.state().attempts[1].clone();
    assert_eq!(
        fence(Some(kept.state()), &token(&b), Some(NODE)),
        FenceVerdict::Live
    );
    // Every ended status is not writable.
    for (n, outcome) in [
        ExitOutcome::Succeeded,
        ExitOutcome::Failed { code: Some(1) },
        ExitOutcome::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let mut k = running();
        let a = k.state().attempts[0].clone();
        k.observe(&Observation::Exited {
            attempt: a.id,
            generation: a.generation,
            outcome,
        })
        .unwrap();
        let v = fence(Some(k.state()), &token(&a), Some(NODE));
        assert!(matches!(v, FenceVerdict::NotWritable { .. }), "{n}: {v:?}");
    }
    kept.replays();
}

#[test]
fn block_is_the_default_and_format_1_decodes_to_it() {
    assert_eq!(RecoveryPolicy::default(), RecoveryPolicy::Block);
    // An M2 to M4 policy is format 1, the restart variant alone; it widens
    // with every M5 default and `recovery: Block`.
    let format_1 = [MANAGED_FORMAT_V1, 1];
    let widened = JobPolicy::from_stored(&format_1).unwrap();
    assert_eq!(widened, JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly));
    assert_eq!(widened.recovery, RecoveryPolicy::Block);
    let format_1_block = [MANAGED_FORMAT_V1, 0];
    assert_eq!(
        JobPolicy::from_stored(&format_1_block).unwrap().recovery,
        RecoveryPolicy::Block
    );
    // Format 2 keeps the recovery policy.
    let after = JobPolicy {
        recovery: RecoveryPolicy::RestartFromAcknowledged,
        ..JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
    };
    let bytes = after.to_stored().unwrap();
    assert_eq!(bytes[0], MANAGED_FORMAT);
    assert_eq!(JobPolicy::from_stored(&bytes).unwrap(), after);
    assert_ne!(
        JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
            .to_stored()
            .unwrap(),
        bytes
    );
}

#[test]
fn restart_from_acknowledged_is_refused_for_an_opaque_primary() {
    assert!(matches!(
        admit_recovery(RecoveryPolicy::RestartFromAcknowledged, OutcomePolicy::Block),
        Err(DomainError::RecoveryUnsupported { kind }) if kind == "block"
    ));
    for outcome in [OutcomePolicy::CancelAndRestart, OutcomePolicy::Checkpoint] {
        admit_recovery(RecoveryPolicy::RestartFromAcknowledged, outcome).unwrap();
        admit_recovery(RecoveryPolicy::Block, outcome).unwrap();
    }
    admit_recovery(RecoveryPolicy::Block, OutcomePolicy::Block).unwrap();
    // The primary task of a managed job is a harness task, so a create
    // with the policy is admitted and the policy is kept.
    let mut kept = Kept::new();
    let command = match create() {
        Command::CreateJob { intent, source, .. } => Command::CreateJob {
            intent,
            source,
            policy: JobPolicy {
                recovery: RecoveryPolicy::RestartFromAcknowledged,
                ..JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
            },
        },
        other => other,
    };
    kept.submit("create", command).unwrap();
    assert_eq!(
        kept.state().job.policy.recovery,
        RecoveryPolicy::RestartFromAcknowledged
    );
    kept.replays();
}

#[test]
fn recovery_decision_falls_back_to_block_with_the_failed_condition() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    let a = gone_silent(&mut kept);
    let here = Ok(ResourceRef::Node { node: OTHER_NODE });
    assert_eq!(
        recovery_decision(kept.state(), a.id, OutcomePolicy::Block, &[], here.clone()),
        Err(RecoveryBlock::OutcomeBlocks {
            kind: "block".into()
        })
    );
    let blocker = OperationId::from_bytes([0xbb; 16]);
    assert_eq!(
        recovery_decision(
            kept.state(),
            a.id,
            OutcomePolicy::Checkpoint,
            &[blocker],
            here.clone()
        ),
        Err(RecoveryBlock::ReplacementBlocked {
            blockers: vec![blocker]
        })
    );
    let exclusion = RecoveryExclusion {
        node: NODE,
        reason: "unavailable".into(),
    };
    assert_eq!(
        recovery_decision(
            kept.state(),
            a.id,
            OutcomePolicy::Checkpoint,
            &[],
            Err(vec![exclusion.clone()])
        ),
        Err(RecoveryBlock::NoEligibleNode {
            exclusions: vec![exclusion]
        })
    );
    let decided = recovery_decision(
        kept.state(),
        a.id,
        OutcomePolicy::Checkpoint,
        &[],
        here.clone(),
    )
    .unwrap();
    assert!(matches!(
        &decided,
        Command::Recover { attempt, from, resource, .. }
            if *attempt == a.id && *from == snapshot(1) && *resource == ResourceRef::Node { node: OTHER_NODE }
    ));
    // The decision is a command the rules accept as is.
    kept.submit("auto-recover", decided).unwrap();
    assert_eq!(kept.state().job.generation.get(), 2);
    // A cancel-and-restart task takes the job's newest acknowledged
    // checkpoint, which another attempt may have made; a checkpoint task
    // takes only its own.
    let b = gone_silent(&mut kept);
    assert_eq!(
        recovery_decision(
            kept.state(),
            b.id,
            OutcomePolicy::Checkpoint,
            &[],
            here.clone()
        ),
        Err(RecoveryBlock::NoAcknowledgedCheckpoint)
    );
    assert!(matches!(
        recovery_decision(kept.state(), b.id, OutcomePolicy::CancelAndRestart, &[], here),
        Ok(Command::Recover { from, .. }) if from == snapshot(1)
    ));
    kept.replays();
}

/// After a `Supersede`, a `StartAttempt` is admitted by the M2 rules, but
/// it resumes only from a checkpoint the authority acknowledged: a
/// captured-only snapshot, or one the job never saw, is refused the way
/// `Recover` refuses it. The job's first attempt may start from any
/// snapshot the operator names.
#[test]
fn a_start_after_the_first_attempt_resumes_only_from_an_acknowledged_checkpoint() {
    let mut first = made();
    let primary = first.state().job.primary;
    first
        .submit(
            "start-any",
            Command::StartAttempt {
                task: primary,
                resource: ResourceRef::Node { node: NODE },
                initial_checkpoint: Some(snapshot(9)),
            },
        )
        .unwrap();
    assert_eq!(
        first.state().attempts[0].initial_checkpoint,
        Some(snapshot(9))
    );

    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    let a = gone_silent(&mut kept);
    kept.submit(
        "supersede",
        Command::Supersede {
            attempt: a.id,
            reason: "operator".into(),
        },
    )
    .unwrap();
    assert!(kept.state().live_attempt().is_none());
    let primary = kept.state().job.primary;
    let start_from = |from: SnapshotId| Command::StartAttempt {
        task: primary,
        resource: ResourceRef::Node { node: NODE },
        initial_checkpoint: Some(from),
    };
    // Captured on the runner, never verified by the authority: refused.
    assert!(matches!(
        kept.submit("start-captured", start_from(snapshot(1))),
        Err(DomainError::NotAcknowledged { snapshot }) if snapshot == snapshot_of(1)
    ));
    // A snapshot the job never saw: refused the same way.
    assert!(matches!(
        kept.submit("start-unknown", start_from(snapshot(8))),
        Err(DomainError::NotAcknowledged { snapshot }) if snapshot == snapshot_of(8)
    ));
    assert_eq!(kept.state().attempts.len(), 1, "nothing was granted");
    // Acknowledged: the start is admitted from it.
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    kept.submit("start-acknowledged", start_from(snapshot(1)))
        .unwrap();
    let next = &kept.state().attempts[1];
    assert_eq!(next.initial_checkpoint, Some(snapshot(1)));
    assert_eq!(next.generation.get(), 2);
    kept.replays();
}

fn snapshot_of(n: u8) -> SnapshotId {
    snapshot(n)
}

/// The recovery decision judges an attempt by its own task's class, never
/// by the job's primary: an `opaque` child's outcome policy is `block` and
/// the authority never restarts it by itself, while the primary is a
/// harness task.
#[test]
fn an_attempt_is_judged_by_its_own_task_not_the_primary() {
    assert_eq!(
        TaskClass::Harness.outcome_policy(),
        OutcomePolicy::Checkpoint
    );
    assert_eq!(TaskClass::Opaque.outcome_policy(), OutcomePolicy::Block);
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    let primary_attempt = kept.state().attempts[0].clone();
    kept.observe(&Observation::Exited {
        attempt: primary_attempt.id,
        generation: primary_attempt.generation,
        outcome: ExitOutcome::Succeeded,
    })
    .unwrap();
    let primary = kept.state().job.primary;
    kept.submit(
        "register-child",
        Command::RegisterTask {
            parent: primary,
            kind: TaskKind::Child {
                label: "build".into(),
            },
            restart: RestartPolicy::ExplicitOnly,
            class: TaskClass::Opaque,
        },
    )
    .unwrap();
    let child = kept.state().tasks[1].clone();
    kept.submit("start-child", start(child.id)).unwrap();
    let b = kept.state().live_attempt().unwrap().clone();
    kept.mark(b.granted_by, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Started {
        attempt: b.id,
        generation: b.generation,
    })
    .unwrap();
    let b = gone_silent(&mut kept);
    assert_eq!(b.task, child.id);
    let primary_task = kept.state().task(primary).unwrap().clone();
    let child_task = kept.state().task(child.id).unwrap().clone();
    assert_eq!(
        OutcomePolicy::of_task(&primary_task),
        OutcomePolicy::Checkpoint
    );
    assert_eq!(OutcomePolicy::of_task(&child_task), OutcomePolicy::Block);
    // Judged by its own task, the child's attempt is blocked although the
    // job holds an acknowledged checkpoint the primary's policy would use.
    let here = Ok(ResourceRef::Node { node: OTHER_NODE });
    assert_eq!(
        recovery_decision(
            kept.state(),
            b.id,
            OutcomePolicy::of_task(&child_task),
            &[],
            here.clone()
        ),
        Err(RecoveryBlock::OutcomeBlocks {
            kind: "block".into()
        })
    );
    // The same attempt under a cancel-and-restart kind would take the
    // job's acknowledged checkpoint: the kind decides, not the job.
    assert!(matches!(
        recovery_decision(
            kept.state(),
            b.id,
            OutcomePolicy::CancelAndRestart,
            &[],
            here
        ),
        Ok(Command::Recover { from, .. }) if from == snapshot(1)
    ));
    kept.replays();
}

#[test]
fn a_recover_fails_the_old_attempt_s_checkpoint_so_the_new_one_may_take_one() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    let a = kept.state().attempts[0].clone();
    let asked = kept
        .submit("cp-1", Command::Checkpoint { attempt: a.id })
        .unwrap();
    let ManagedEvent::CommandAccepted { operation, .. } = asked.events[0] else {
        panic!("not accepted");
    };
    kept.mark(operation, OperationMark::Dispatched).unwrap();
    gone_silent(&mut kept);
    let t = kept
        .submit(
            "r-1",
            Command::Recover {
                attempt: a.id,
                resource: ResourceRef::Node { node: OTHER_NODE },
                from: snapshot(1),
                reason: "silent".into(),
            },
        )
        .unwrap();
    assert!(t.events.iter().any(|e| matches!(
        e,
        ManagedEvent::OperationMarked { operation: o, status: OperationMark::Failed } if *o == operation
    )));
    assert_eq!(
        kept.state().operation(operation).unwrap().status,
        OperationStatus::Failed
    );
    // The new attempt runs, and a checkpoint of it is not refused as in flight.
    let next = kept.state().attempts[1].clone();
    kept.mark(next.granted_by, OperationMark::Dispatched)
        .unwrap();
    kept.observe(&Observation::Started {
        attempt: next.id,
        generation: next.generation,
    })
    .unwrap();
    kept.submit("cp-2", Command::Checkpoint { attempt: next.id })
        .unwrap();
    kept.replays();
}

#[test]
fn an_open_cancel_refuses_a_recover_and_a_supersede_completes_it() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    let a = kept.state().attempts[0].clone();
    let asked = kept
        .submit(
            "cancel",
            Command::CancelJob {
                reason: "stop".into(),
            },
        )
        .unwrap();
    let ManagedEvent::CommandAccepted { operation, .. } = asked.events[0] else {
        panic!("not accepted");
    };
    kept.mark(operation, OperationMark::Dispatched).unwrap();
    gone_silent(&mut kept);
    assert_eq!(
        kept.submit(
            "r-1",
            Command::Recover {
                attempt: a.id,
                resource: ResourceRef::Node { node: OTHER_NODE },
                from: snapshot(1),
                reason: "silent".into(),
            },
        ),
        Err(DomainError::EffectInFlight { operation })
    );
    kept.submit(
        "s-1",
        Command::Supersede {
            attempt: a.id,
            reason: "silent".into(),
        },
    )
    .unwrap();
    let s = kept.state();
    assert_eq!(s.job.state, JobState::Cancelled);
    assert_eq!(
        s.operation(operation).unwrap().status,
        OperationStatus::Succeeded
    );
    kept.replays();
}

#[test]
fn an_acknowledgement_that_lands_after_a_regrant_is_stale() {
    let mut kept = running();
    capture(&mut kept, 1).unwrap();
    kept.acknowledge(&ack_of(&kept, 1)).unwrap();
    // A second capture whose fetch is still running when the attempt goes silent.
    capture(&mut kept, 2).unwrap();
    let late = ack_of(&kept, 2);
    let a = gone_silent(&mut kept);
    kept.submit(
        "r-1",
        Command::Recover {
            attempt: a.id,
            resource: ResourceRef::Node { node: OTHER_NODE },
            from: snapshot(1),
            reason: "silent".into(),
        },
    )
    .unwrap();
    assert_eq!(
        kept.acknowledge(&late),
        Err(DomainError::StaleGeneration {
            given: a.generation,
            current: kept.state().job.generation,
        })
    );
    let acked: Vec<SnapshotId> = kept
        .state()
        .acknowledged()
        .iter()
        .map(|c| c.snapshot)
        .collect();
    assert_eq!(acked, vec![snapshot(1)]);
    // The acknowledgement it already holds is still the same answer.
    assert!(
        kept.acknowledge(&ack_of(&kept, 1))
            .unwrap()
            .events
            .is_empty()
    );
    kept.replays();
}

#[test]
fn a_grant_never_launched_comes_back_granted_and_is_dispatched() {
    let mut kept = made();
    let primary = kept.state().job.primary;
    kept.submit("start-1", start(primary)).unwrap();
    let a = gone_silent(&mut kept);
    kept.observe(&Observation::ResourceReachable {
        attempt: a.id,
        generation: a.generation,
    })
    .unwrap();
    let back = kept.state().attempt(a.id).unwrap().clone();
    assert_eq!(back.status, AttemptStatus::Granted);
    assert!(kept.state().grant_may_dispatch(back.granted_by));
    kept.mark(back.granted_by, OperationMark::Dispatched)
        .unwrap();
    kept.observe(&Observation::Started {
        attempt: a.id,
        generation: a.generation,
    })
    .unwrap();
    let s = kept.state();
    assert_eq!(s.attempt(a.id).unwrap().status, AttemptStatus::Running);
    assert_eq!(
        s.operation(back.granted_by).unwrap().status,
        OperationStatus::Succeeded
    );
    kept.replays();
}
