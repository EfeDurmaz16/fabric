//! The M5 rules of managed jobs (ADRs 0023 to 0026 in the domain): the
//! initial placement, the planned handoff, the checkpoint that leaves the
//! attempt running, the cancel that stops a live attempt first, the parent
//! that waits for its children, the notes and the policy bounds. Every
//! path ends with replay equals projection.

#![allow(
    clippy::unwrap_used,
    reason = "the helpers here are test code; a failed unwrap is the test failing"
)]

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::*;
use fabric_core::path::AbsPath;

const NODE: NodeId = NodeId::from_bytes([9; 32]);

struct Kept {
    state: Option<ManagedState>,
    history: Vec<(WorkRevision, ManagedEvent)>,
    next: u8,
}

impl Kept {
    fn new() -> Kept {
        Kept {
            state: None,
            history: Vec::new(),
            next: 0,
        }
    }

    fn ids(&mut self) -> NewIds {
        self.next += 1;
        let n = self.next;
        NewIds {
            job: JobId::from_bytes([1; 16]),
            task: TaskId::from_bytes([n; 16]),
            attempt: AttemptId::from_bytes([n; 16]),
            operation: OperationId::from_bytes([n; 16]),
            at_ms: u64::from(n),
        }
    }

    fn commit(&mut self, t: Transition) -> Transition {
        if !t.events.is_empty() {
            self.state = Some(apply_batch(self.state.take(), t.revision, &t.events).unwrap());
            self.history
                .extend(t.events.iter().cloned().map(|e| (t.revision, e)));
        }
        self.replays();
        t
    }

    fn submit(
        &mut self,
        key: &str,
        body: Command,
    ) -> Result<(OperationId, Transition), DomainError> {
        let envelope = Envelope {
            key: OperationKey::parse(key).unwrap(),
            job: self.state.as_ref().map(|s| s.job.id),
            expected: None,
            body,
        };
        let ids = self.ids();
        accept_command(self.state.as_ref(), &envelope, &ids)
            .map(|t| (ids.operation, self.commit(t)))
    }

    fn observe(&mut self, observation: &Observation) -> Result<Transition, DomainError> {
        apply_observation(self.state.as_ref().unwrap(), observation).map(|t| self.commit(t))
    }

    fn mark(&mut self, op: OperationId, mark: OperationMark) -> Result<Transition, DomainError> {
        mark_operation(self.state.as_ref().unwrap(), op, mark).map(|t| self.commit(t))
    }

    fn state(&self) -> &ManagedState {
        self.state.as_ref().unwrap()
    }

    fn replays(&self) {
        let job = self.state().job.id;
        assert_eq!(&replay(job, &self.history).unwrap(), self.state());
    }

    fn status(&self, op: OperationId) -> OperationStatus {
        self.state().operation(op).unwrap().status.clone()
    }
}

fn create_with(source: SourceBinding, policy: JobPolicy) -> Command {
    Command::CreateJob {
        intent: Intent {
            goal: "make the greeting louder".into(),
            harness: HarnessKind::ClaudeCode,
            acceptance: vec!["the tests pass".into()],
        },
        source,
        policy,
    }
}

fn create() -> Command {
    create_with(
        SourceBinding::Repository {
            path: AbsPath::parse("/src/repo").unwrap(),
        },
        JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
    )
}

fn decision(kind: PlacementKind, chosen: Option<NodeId>) -> PlacementDecision {
    PlacementDecision {
        kind,
        decided_at_ms: 1,
        candidates: Vec::new(),
        chosen,
    }
}

fn cut(snapshot: SnapshotId, receipts: Vec<TaskReceipt>) -> CheckpointCut {
    CheckpointCut {
        snapshot,
        receipts,
        facts: vec![("os".into(), "macos".into())],
        evidence: Vec::new(),
    }
}

/// A job created and placed on `NODE`, its first attempt running.
fn running() -> (Kept, OperationId) {
    let mut kept = Kept::new();
    let (create_op, _) = kept.submit("create", create()).unwrap();
    let primary = kept.state().job.primary;
    kept.observe(&Observation::Placed {
        operation: create_op,
        task: primary,
        decision: decision(PlacementKind::Initial, Some(NODE)),
    })
    .unwrap();
    let a = kept.state().attempts[0].clone();
    kept.observe(&Observation::Started {
        attempt: a.id,
        generation: a.generation,
    })
    .unwrap();
    (kept, create_op)
}

#[test]
fn a_create_is_placed_or_parked_by_its_initial_placement() {
    let mut kept = Kept::new();
    let (create_op, _) = kept.submit("create", create()).unwrap();
    assert_eq!(kept.status(create_op), OperationStatus::Accepted);
    assert!(kept.state().parked());
    let primary = kept.state().job.primary;
    // Parked: the decision is recorded, nothing is granted, the create succeeded.
    let parked = kept
        .observe(&Observation::Placed {
            operation: create_op,
            task: primary,
            decision: decision(PlacementKind::Initial, None),
        })
        .unwrap();
    assert!(matches!(
        parked.events[0],
        ManagedEvent::PlacementDecided { .. }
    ));
    assert!(kept.state().attempts.is_empty());
    assert_eq!(kept.status(create_op), OperationStatus::Succeeded);
    assert!(kept.state().parked());
    // A second placement for the same create is refused.
    assert!(matches!(
        kept.observe(&Observation::Placed {
            operation: create_op,
            task: primary,
            decision: decision(PlacementKind::Initial, Some(NODE)),
        }),
        Err(DomainError::OperationState { .. })
    ));

    // Chosen: the first attempt is granted at generation one, from the
    // snapshot source when there is one, and the create is dispatched.
    let mut kept = Kept::new();
    let snapshot = SnapshotId::of(b"import");
    let (create_op, _) = kept
        .submit(
            "create",
            create_with(
                SourceBinding::Snapshot { snapshot },
                JobPolicy::m2_defaults(RestartPolicy::Block),
            ),
        )
        .unwrap();
    let primary = kept.state().job.primary;
    kept.observe(&Observation::Placed {
        operation: create_op,
        task: primary,
        decision: decision(PlacementKind::Initial, Some(NODE)),
    })
    .unwrap();
    let a = &kept.state().attempts[0];
    assert_eq!(a.generation, Generation::FIRST);
    assert_eq!(a.granted_by, create_op);
    assert_eq!(a.initial_checkpoint, Some(snapshot));
    assert_eq!(a.resource, ResourceRef::Node { node: NODE });
    assert_eq!(kept.status(create_op), OperationStatus::Dispatched);
    assert!(!kept.state().parked());
    let (id, generation) = (a.id, a.generation);
    kept.observe(&Observation::Started {
        attempt: id,
        generation,
    })
    .unwrap();
    assert_eq!(kept.status(create_op), OperationStatus::Succeeded);
}

#[test]
fn a_bad_policy_an_oversized_acceptance_and_an_unsupported_source_are_refused() {
    let mut kept = Kept::new();
    let mut policy = JobPolicy::m2_defaults(RestartPolicy::Block);
    policy.drain_budget_ms = 10;
    assert!(matches!(
        kept.submit("c1", create_with(SourceBinding::Empty, policy)),
        Err(DomainError::BadPolicy { field }) if field == "drain_budget_ms"
    ));
    let mut policy = JobPolicy::m2_defaults(RestartPolicy::Block);
    policy.allowed_fidelity = AllowedFidelity {
        native_session: false,
        new_native_session_with_package: false,
        workspace_only: false,
    };
    assert!(matches!(
        kept.submit("c2", create_with(SourceBinding::Empty, policy)),
        Err(DomainError::BadPolicy { field }) if field == "allowed_fidelity"
    ));
    let mut policy = JobPolicy::m2_defaults(RestartPolicy::Block);
    let name = SecretName::parse("TOKEN").unwrap();
    policy.secrets = vec![
        SecretRef {
            name: name.clone(),
            scope: SecretScope::node_any(),
        },
        SecretRef {
            name,
            scope: SecretScope::node_any(),
        },
    ];
    assert!(matches!(
        kept.submit("c3", create_with(SourceBinding::Empty, policy)),
        Err(DomainError::BadPolicy { field }) if field == "secrets"
    ));
    let long = Command::CreateJob {
        intent: Intent {
            goal: "g".into(),
            harness: HarnessKind::Codex,
            acceptance: vec!["a".repeat(TEXT_MAX_BYTES + 1)],
        },
        source: SourceBinding::Empty,
        policy: JobPolicy::m2_defaults(RestartPolicy::Block),
    };
    assert!(matches!(
        kept.submit("c4", long),
        Err(DomainError::BadPolicy { field }) if field == "acceptance"
    ));
    // A git source needs a full commit id, checked at the type.
    assert!(CommitId::parse("main").is_err());
    let git = create_with(
        SourceBinding::Git {
            uri: GitUri::parse("https://github.com/o/r.git").unwrap(),
            commit: CommitId::parse(&"a".repeat(40)).unwrap(),
        },
        JobPolicy::m2_defaults(RestartPolicy::Block),
    );
    kept.submit("c5", git).unwrap();
    assert!(matches!(kept.state().job.source, SourceBinding::Git { .. }));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the handoff's steps read best as one script"
)]
fn a_continue_cuts_places_grants_and_records_the_fidelity_achieved() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    // A registered test task that the cut interrupts.
    let (_, t) = kept
        .submit(
            "child",
            Command::RegisterTask {
                parent: primary,
                kind: TaskKind::Child {
                    label: "tests".into(),
                },
                restart: RestartPolicy::Block,
                class: TaskClass::Test,
                program: Some(fabric_core::managed::Program {
                    argv: vec!["cargo".into(), "test".into()],
                    cwd: None,
                    env_allowlist: Vec::new(),
                }),
            },
        )
        .unwrap();
    let ManagedEvent::TaskRegistered { task: test, .. } = t.events[1] else {
        panic!("no task")
    };
    assert_eq!(kept.state().task(test).unwrap().class, TaskClass::Test);
    // The parent cannot finish while its child is open.
    assert!(matches!(
        kept.submit(
            "finish-early",
            Command::FinishTask {
                task: primary,
                outcome: TaskOutcome::Succeeded
            }
        ),
        Err(DomainError::AttemptStillLive { .. })
    ));
    let (cont, _) = kept
        .submit(
            "continue",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: None,
            },
        )
        .unwrap();
    assert_eq!(kept.status(cont), OperationStatus::Accepted);
    assert!(matches!(
        kept.state().request(cont),
        Some(EffectRequest::Continue { .. })
    ));
    // A second continue or a checkpoint waits while this one is in flight.
    kept.mark(cont, OperationMark::Dispatched).unwrap();
    assert!(matches!(
        kept.submit(
            "continue-2",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: None
            }
        ),
        Err(DomainError::EffectInFlight { operation }) if operation == cont
    ));
    assert!(matches!(
        kept.submit("checkpoint", Command::Checkpoint { attempt: first.id }),
        Err(DomainError::EffectInFlight { .. })
    ));
    // The cut: the attempt ends checkpointed, the receipts say the test was interrupted.
    let snapshot = SnapshotId::of(b"cut");
    let receipts = vec![
        harness_receipt(primary, snapshot),
        TaskReceipt {
            task: test,
            class: TaskClass::Test,
            status: ReceiptStatus::Interrupted,
            exit: None,
            interrupted: true,
            workspace: None,
        },
    ];
    let the_cut = cut(snapshot, receipts.clone());
    let digest = the_cut.digest();
    let t = kept
        .observe(&Observation::Checkpointed {
            attempt: first.id,
            generation: first.generation,
            snapshot,
            cut: the_cut,
        })
        .unwrap();
    assert!(matches!(
        &t.events[..],
        [
            ManagedEvent::AttemptObserved {
                status: AttemptStatus::Checkpointed { .. },
                ..
            },
            ManagedEvent::CheckpointCutRecorded { cut, receipts: r, .. },
        ] if *cut == digest && *r == receipts
    ));
    assert_eq!(kept.status(cont), OperationStatus::Dispatched);
    // Placed as a planned handoff: the next generation starts from the cut.
    kept.observe(&Observation::Placed {
        operation: cont,
        task: primary,
        decision: decision(PlacementKind::PlannedHandoff, Some(NODE)),
    })
    .unwrap();
    let second = kept.state().attempts[1].clone();
    assert_eq!(second.generation, Generation::new(2).unwrap());
    assert_eq!(second.initial_checkpoint, Some(snapshot));
    assert_eq!(second.granted_by, cont);
    // Started does not settle a continue; Continued does.
    kept.observe(&Observation::Started {
        attempt: second.id,
        generation: second.generation,
    })
    .unwrap();
    assert_eq!(kept.status(cont), OperationStatus::Dispatched);
    let t = kept
        .observe(&Observation::Continued {
            attempt: second.id,
            generation: second.generation,
            from: first.id,
            achieved: Fidelity::NewNativeSessionWithPackage,
            package: Some(PackageDigest([3; 32])),
        })
        .unwrap();
    assert!(matches!(
        t.events[0],
        ManagedEvent::ContinuationRecorded {
            achieved: Fidelity::NewNativeSessionWithPackage,
            requested,
            package: Some(package),
            ..
        } if requested == AllowedFidelity::DEFAULT && package == PackageDigest([3; 32])
    ));
    assert_eq!(kept.status(cont), OperationStatus::Succeeded);
    // The test task is still open: an interrupted test was not finished.
    assert_eq!(kept.state().task(test).unwrap().state, TaskState::Open);
}

#[test]
fn a_continue_with_no_live_attempt_places_from_the_last_checkpoint() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let snapshot = SnapshotId::of(b"s1");
    kept.observe(&Observation::Checkpointed {
        attempt: first.id,
        generation: first.generation,
        snapshot,
        cut: cut(snapshot, vec![harness_receipt(primary, snapshot)]),
    })
    .unwrap();
    let (cont, _) = kept
        .submit(
            "continue",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: Some(NODE),
            },
        )
        .unwrap();
    // No cut this time: straight to the placement, which parks it.
    kept.observe(&Observation::Placed {
        operation: cont,
        task: primary,
        decision: decision(PlacementKind::PlannedHandoff, None),
    })
    .unwrap();
    assert_eq!(kept.status(cont), OperationStatus::Succeeded);
    assert!(kept.state().parked());
    // A later continue finds a node.
    let (again, _) = kept
        .submit(
            "continue-2",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: None,
            },
        )
        .unwrap();
    kept.observe(&Observation::Placed {
        operation: again,
        task: primary,
        decision: decision(PlacementKind::PlannedHandoff, Some(NODE)),
    })
    .unwrap();
    assert_eq!(kept.state().attempts[1].initial_checkpoint, Some(snapshot));
    // A continue that allows nothing is refused.
    assert!(matches!(
        kept.submit(
            "continue-3",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity {
                    native_session: false,
                    new_native_session_with_package: false,
                    workspace_only: false
                },
                pin: None
            }
        ),
        Err(DomainError::FidelityRefused { .. })
    ));
}

#[test]
fn a_checkpoint_leaves_the_attempt_running_and_a_blocked_one_fails_the_command() {
    let (mut kept, _) = running();
    let first = kept.state().attempts[0].clone();
    assert!(matches!(
        kept.submit(
            "cp-unknown",
            Command::Checkpoint {
                attempt: AttemptId::from_bytes([77; 16])
            }
        ),
        Err(DomainError::UnknownAttempt { .. })
    ));
    let (cp, _) = kept
        .submit("cp", Command::Checkpoint { attempt: first.id })
        .unwrap();
    kept.mark(cp, OperationMark::Dispatched).unwrap();
    let snapshot = SnapshotId::of(b"cp");
    let t = kept
        .observe(&Observation::Checkpointed {
            attempt: first.id,
            generation: first.generation,
            snapshot,
            cut: cut(snapshot, vec![harness_receipt(first.task, snapshot)]),
        })
        .unwrap();
    assert!(matches!(
        &t.events[..],
        [
            ManagedEvent::CheckpointCutRecorded { .. },
            ManagedEvent::OperationMarked {
                status: OperationMark::Succeeded,
                ..
            }
        ]
    ));
    assert_eq!(kept.state().attempts[0].status, AttemptStatus::Running);
    assert_eq!(kept.status(cp), OperationStatus::Succeeded);
    // A second checkpoint converges: the same observation shape, a new cut.
    let (cp2, _) = kept
        .submit("cp2", Command::Checkpoint { attempt: first.id })
        .unwrap();
    kept.mark(cp2, OperationMark::Dispatched).unwrap();
    let t = kept
        .observe(&Observation::HandoffBlocked {
            attempt: first.id,
            generation: first.generation,
            blockers: vec![HandoffBlocker::EscapedChild { pid: 4, group: 3 }],
        })
        .unwrap();
    assert!(matches!(
        &t.events[..],
        [
            ManagedEvent::HandoffBlocked { .. },
            ManagedEvent::OperationMarked {
                status: OperationMark::Failed,
                ..
            }
        ]
    ));
    assert_eq!(kept.status(cp2), OperationStatus::Failed);
    assert_eq!(kept.state().attempts[0].status, AttemptStatus::Running);
    assert_eq!(kept.state().job.state, JobState::Active);
    // A checkpoint of an ended attempt is not live.
    kept.observe(&Observation::Exited {
        attempt: first.id,
        generation: first.generation,
        outcome: ExitOutcome::Succeeded,
    })
    .unwrap();
    assert!(matches!(
        kept.submit("cp3", Command::Checkpoint { attempt: first.id }),
        Err(DomainError::NotLive { .. })
    ));
}

#[test]
fn a_cancel_with_a_live_attempt_stops_it_first_and_the_receipts_say_interrupted() {
    let (mut kept, _) = running();
    let first = kept.state().attempts[0].clone();
    let primary = kept.state().job.primary;
    let (cancel, _) = kept
        .submit(
            "cancel",
            Command::CancelJob {
                reason: "enough".into(),
            },
        )
        .unwrap();
    assert_eq!(kept.status(cancel), OperationStatus::Accepted);
    assert_eq!(kept.state().job.state, JobState::Active);
    kept.mark(cancel, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Receipts {
        attempt: first.id,
        generation: first.generation,
        receipts: vec![TaskReceipt {
            task: primary,
            class: TaskClass::Harness,
            status: ReceiptStatus::Interrupted,
            exit: None,
            interrupted: true,
            workspace: None,
        }],
    })
    .unwrap();
    let t = kept
        .observe(&Observation::Exited {
            attempt: first.id,
            generation: first.generation,
            outcome: ExitOutcome::Cancelled,
        })
        .unwrap();
    assert!(matches!(
        &t.events[..],
        [
            ManagedEvent::AttemptObserved {
                status: AttemptStatus::Cancelled,
                ..
            },
            ManagedEvent::JobCancelled { reason },
            ManagedEvent::OperationMarked {
                status: OperationMark::Succeeded,
                ..
            }
        ] if reason == "enough"
    ));
    assert_eq!(kept.state().job.state, JobState::Cancelled);
    assert_eq!(kept.status(cancel), OperationStatus::Succeeded);

    // A cancel of one task stops its attempt and finishes the task only.
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let (cancel_task, _) = kept
        .submit(
            "cancel-task",
            Command::CancelTask {
                task: primary,
                reason: "redo".into(),
            },
        )
        .unwrap();
    kept.mark(cancel_task, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Exited {
        attempt: first.id,
        generation: first.generation,
        outcome: ExitOutcome::Cancelled,
    })
    .unwrap();
    assert_eq!(
        kept.state().task(primary).unwrap().state,
        TaskState::Cancelled
    );
    assert_eq!(kept.status(cancel_task), OperationStatus::Succeeded);
}

#[test]
fn a_parent_waits_for_its_children_and_a_cancelled_task_without_an_attempt_ends_at_commit() {
    let mut kept = Kept::new();
    kept.submit("create", create()).unwrap();
    let primary = kept.state().job.primary;
    let (_, t) = kept
        .submit(
            "child",
            Command::RegisterTask {
                parent: primary,
                kind: TaskKind::Child {
                    label: "build".into(),
                },
                restart: RestartPolicy::Block,
                class: TaskClass::Build,
                program: Some(fabric_core::managed::Program {
                    argv: vec!["cargo".into(), "build".into()],
                    cwd: None,
                    env_allowlist: Vec::new(),
                }),
            },
        )
        .unwrap();
    let ManagedEvent::TaskRegistered { task: child, .. } = t.events[1] else {
        panic!("no task")
    };
    assert!(matches!(
        kept.submit(
            "finish-parent",
            Command::FinishTask {
                task: primary,
                outcome: TaskOutcome::Succeeded
            }
        ),
        Err(DomainError::BadParent { task }) if task == child
    ));
    assert!(matches!(
        kept.submit(
            "cancel-parent",
            Command::CancelTask {
                task: primary,
                reason: "r".into()
            }
        ),
        Err(DomainError::BadParent { task }) if task == child
    ));
    let (cancel_child, t) = kept
        .submit(
            "cancel-child",
            Command::CancelTask {
                task: child,
                reason: "r".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        t.events[1],
        ManagedEvent::TaskFinished {
            outcome: TaskOutcome::Cancelled,
            ..
        }
    ));
    assert_eq!(kept.status(cancel_child), OperationStatus::Succeeded);
    kept.submit(
        "finish-parent-2",
        Command::FinishTask {
            task: primary,
            outcome: TaskOutcome::Succeeded,
        },
    )
    .unwrap();
    assert_eq!(kept.state().job.state, JobState::Finished);
}

#[test]
fn notes_are_recorded_at_commit_and_bounded() {
    let mut kept = Kept::new();
    kept.submit("create", create()).unwrap();
    let (op, t) = kept
        .submit(
            "note-1",
            Command::RecordNote {
                note: Note::Decision {
                    what: "use sqlite".into(),
                    reason: "one writer".into(),
                },
            },
        )
        .unwrap();
    assert!(matches!(t.events[1], ManagedEvent::NoteRecorded { .. }));
    assert_eq!(kept.status(op), OperationStatus::Succeeded);
    assert!(matches!(
        kept.submit(
            "note-2",
            Command::RecordNote {
                note: Note::Decision {
                    what: "w".into(),
                    reason: "  ".into()
                }
            }
        ),
        Err(DomainError::BadPolicy { field }) if field == "note.reason"
    ));
    assert!(matches!(
        kept.submit(
            "note-3",
            Command::RecordNote {
                note: Note::Question {
                    text: "q".repeat(TEXT_MAX_BYTES + 1)
                }
            }
        ),
        Err(DomainError::NoteTooLong { bytes }) if bytes == TEXT_MAX_BYTES + 1
    ));
    kept.submit(
        "note-4",
        Command::RecordNote {
            note: Note::Answer {
                question: WorkRevision::new(2).unwrap(),
                text: "yes".into(),
            },
        },
    )
    .unwrap();
}

/// The receipt of a harness task the suspend path checkpointed.
fn harness_receipt(task: TaskId, snapshot: SnapshotId) -> TaskReceipt {
    TaskReceipt {
        task,
        class: TaskClass::Harness,
        status: ReceiptStatus::Checkpointed { snapshot },
        exit: None,
        interrupted: true,
        workspace: None,
    }
}

/// A continue after the attempt was lost starts from the newest cut of
/// the task, a checkpoint command's included, not from the last attempt
/// that ended checkpointed.
#[test]
fn a_continue_starts_from_the_newest_cut_a_checkpoint_command_made() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let s2 = SnapshotId::of(b"s2");
    let (cp, _) = kept
        .submit("cp", Command::Checkpoint { attempt: first.id })
        .unwrap();
    kept.mark(cp, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Checkpointed {
        attempt: first.id,
        generation: first.generation,
        snapshot: s2,
        cut: cut(s2, vec![harness_receipt(primary, s2)]),
    })
    .unwrap();
    kept.observe(&Observation::Lost {
        attempt: first.id,
        generation: first.generation,
    })
    .unwrap();
    let (cont, _) = kept
        .submit(
            "continue",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: None,
            },
        )
        .unwrap();
    kept.observe(&Observation::Placed {
        operation: cont,
        task: primary,
        decision: decision(PlacementKind::Failover, Some(NODE)),
    })
    .unwrap();
    assert_eq!(kept.state().attempts[1].initial_checkpoint, Some(s2));
}

/// `new_native_session_with_package` is the fidelity of a handed package:
/// a continuation that reports it with no package is refused.
#[test]
fn a_package_fidelity_needs_a_package() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let (cont, _) = kept
        .submit(
            "continue",
            Command::Continue {
                task: primary,
                fidelity: AllowedFidelity::DEFAULT,
                pin: None,
            },
        )
        .unwrap();
    kept.mark(cont, OperationMark::Dispatched).unwrap();
    let snapshot = SnapshotId::of(b"cut");
    kept.observe(&Observation::Checkpointed {
        attempt: first.id,
        generation: first.generation,
        snapshot,
        cut: cut(snapshot, vec![harness_receipt(primary, snapshot)]),
    })
    .unwrap();
    kept.observe(&Observation::Placed {
        operation: cont,
        task: primary,
        decision: decision(PlacementKind::PlannedHandoff, Some(NODE)),
    })
    .unwrap();
    let second = kept.state().attempts[1].clone();
    assert!(matches!(
        kept.observe(&Observation::Continued {
            attempt: second.id,
            generation: second.generation,
            from: first.id,
            achieved: Fidelity::NewNativeSessionWithPackage,
            package: None,
        }),
        Err(DomainError::MissingPackage { .. })
    ));
    assert_eq!(kept.status(cont), OperationStatus::Dispatched);
}

/// A checkpoint command belongs to the attempt it names: when that
/// attempt ends first, the command fails, and a later attempt's cut is
/// that attempt's own end, never the command's success.
#[test]
fn a_checkpoint_of_an_attempt_that_ended_fails_and_claims_no_later_cut() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let (cp, _) = kept
        .submit("cp", Command::Checkpoint { attempt: first.id })
        .unwrap();
    kept.mark(cp, OperationMark::Dispatched).unwrap();
    kept.observe(&Observation::Exited {
        attempt: first.id,
        generation: first.generation,
        outcome: ExitOutcome::Failed { code: Some(1) },
    })
    .unwrap();
    assert_eq!(kept.status(cp), OperationStatus::Failed);
    let (_, _) = kept
        .submit(
            "start-b",
            Command::StartAttempt {
                task: primary,
                resource: ResourceRef::Node { node: NODE },
                initial_checkpoint: None,
            },
        )
        .unwrap();
    let second = kept.state().attempts[1].clone();
    let snapshot = SnapshotId::of(b"b");
    kept.observe(&Observation::Checkpointed {
        attempt: second.id,
        generation: second.generation,
        snapshot,
        cut: cut(snapshot, vec![harness_receipt(primary, snapshot)]),
    })
    .unwrap();
    assert_eq!(
        kept.state().attempts[1].status,
        AttemptStatus::Checkpointed { snapshot }
    );
}

/// A cut carries one receipt per open task of the attempt: one that
/// leaves a task out, or an empty one, is refused.
#[test]
fn a_cut_that_misses_an_open_task_is_refused() {
    let (mut kept, _) = running();
    let primary = kept.state().job.primary;
    let first = kept.state().attempts[0].clone();
    let (_, t) = kept
        .submit(
            "child",
            Command::RegisterTask {
                parent: primary,
                kind: TaskKind::Child {
                    label: "tests".into(),
                },
                restart: RestartPolicy::Block,
                class: TaskClass::Test,
                program: Some(fabric_core::managed::Program {
                    argv: vec!["cargo".into(), "test".into()],
                    cwd: None,
                    env_allowlist: Vec::new(),
                }),
            },
        )
        .unwrap();
    let ManagedEvent::TaskRegistered { task: test, .. } = t.events[1] else {
        panic!("no task")
    };
    let snapshot = SnapshotId::of(b"cut");
    for (receipts, missing) in [
        (Vec::new(), primary),
        (vec![harness_receipt(primary, snapshot)], test),
    ] {
        assert_eq!(
            kept.observe(&Observation::Checkpointed {
                attempt: first.id,
                generation: first.generation,
                snapshot,
                cut: cut(snapshot, receipts),
            }),
            Err(DomainError::MissingReceipt { task: missing })
        );
    }
}

/// A checkpoint or continue accepted and not yet dispatched is in flight
/// too: a second one is refused instead of piling up.
#[test]
fn an_accepted_checkpoint_refuses_a_second_one() {
    let (mut kept, _) = running();
    let first = kept.state().attempts[0].clone();
    let (cp, _) = kept
        .submit("cp", Command::Checkpoint { attempt: first.id })
        .unwrap();
    assert!(matches!(
        kept.submit("cp-2", Command::Checkpoint { attempt: first.id }),
        Err(DomainError::EffectInFlight { operation }) if operation == cp
    ));
}
