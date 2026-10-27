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
        },
        source: SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        policy: JobPolicy {
            restart: RestartPolicy::ExplicitOnly,
        },
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
    assert_eq!(s.operations[0].status, OperationStatus::Succeeded);
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
    assert!(matches!(
        kept.submit("cancel", Command::CancelJob { reason: "x".into() }),
        Err(DomainError::AttemptStillLive { .. })
    ));
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
    other[0] = 2;
    assert!(matches!(
        EventFrame::decode(&other),
        Err(FrameError::UnknownFormat(2))
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
    if let Some((_, ManagedEvent::AttemptGranted { generation, .. })) = skipped.last_mut() {
        *generation = Generation::new(2).unwrap();
    }
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
                        4 => Observation::Checkpointed { attempt, generation, snapshot: SnapshotId::of(&[which]) },
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
