//! The work package and continuation fidelity (ADR 0024): the limits, the
//! credential refusal, evidence by digest only, and the fidelity ladder.

#![allow(
    clippy::unwrap_used,
    reason = "the helpers here are test code; a failed unwrap is the test failing"
)]

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::package::{ArtifactRef, NextAction};
use fabric_core::managed::placement::{Capacity, HarnessInventory, PathLayout, Warmth};
use fabric_core::managed::*;
use fabric_core::path::AbsPath;
use fabric_core::snapshot::Platform;
use fabric_core::version::HarnessVersion;

fn ids(n: u8) -> NewIds {
    NewIds {
        job: JobId::from_bytes([1; 16]),
        task: TaskId::from_bytes([n; 16]),
        attempt: AttemptId::from_bytes([n; 16]),
        operation: OperationId::from_bytes([n; 16]),
        at_ms: u64::from(n),
    }
}

/// A state whose create went through the domain, so the builder is the
/// second guard; `state_planted` makes one the command would have refused.
fn state_with(goal: &str, acceptance: Vec<String>) -> ManagedState {
    let create = Envelope {
        key: OperationKey::parse("create").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: goal.into(),
                harness: HarnessKind::ClaudeCode,
                acceptance,
            },
            source: SourceBinding::Empty,
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    };
    let made = accept_command(None, &create, &ids(1)).unwrap();
    with_child(apply_batch(None, made.revision, &made.events).unwrap())
}

/// A state folded from hand-made events, as a history the command guard
/// never saw would be; the builder must refuse what the command refuses.
fn state_planted(goal: &str, acceptance: Vec<String>) -> ManagedState {
    let events = vec![
        ManagedEvent::CommandAccepted {
            operation: ids(1).operation,
            key: OperationKey::parse("create").unwrap(),
            digest: RequestDigest::from_bytes([0; 32]),
            kind: CommandKind::CreateJob,
            effect: true,
        },
        ManagedEvent::JobCreated {
            job: ids(1).job,
            intent: Intent {
                goal: goal.into(),
                harness: HarnessKind::ClaudeCode,
                acceptance,
            },
            source: SourceBinding::Empty,
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
            primary: ids(1).task,
        },
        ManagedEvent::TaskRegistered {
            task: ids(1).task,
            parent: None,
            kind: TaskKind::Primary,
            restart: RestartPolicy::ExplicitOnly,
            class: TaskClass::Harness,
        },
    ];
    with_child(apply_batch(None, WorkRevision::FIRST, &events).unwrap())
}

fn with_child(state: ManagedState) -> ManagedState {
    let child = Envelope {
        key: OperationKey::parse("child").unwrap(),
        job: Some(state.job.id),
        expected: None,
        body: Command::RegisterTask {
            parent: state.job.primary,
            kind: TaskKind::Child {
                label: "tests".into(),
            },
            restart: RestartPolicy::Block,
            class: TaskClass::Test,
        },
    };
    let t = accept_command(Some(&state), &child, &ids(2)).unwrap();
    apply_batch(Some(state), t.revision, &t.events).unwrap()
}

fn workspace() -> WorkspaceRevision {
    WorkspaceRevision {
        bundle_head: SnapshotId::of(b"bundle"),
        ledger_head: EventDigest::from_bytes([4; 32]),
    }
}

fn input<'a>(
    state: &'a ManagedState,
    notes: &'a [(WorkRevision, Note)],
    receipts: &'a [TaskReceipt],
    evidence: &'a [EvidenceRef],
    artifacts: &'a [ArtifactRef],
) -> PackageInput<'a> {
    PackageInput {
        state,
        notes,
        receipts,
        workspace: workspace(),
        evidence,
        artifacts,
    }
}

#[test]
fn a_package_over_a_limit_is_refused_naming_the_field() {
    let state = state_with("goal", Vec::new());
    let long = "x".repeat(TEXT_MAX_BYTES + 1);
    let notes = [(
        WorkRevision::FIRST,
        Note::Decision {
            what: long.clone(),
            reason: "r".into(),
        },
    )];
    assert_eq!(
        build_package(&input(&state, &notes, &[], &[], &[])).unwrap_err(),
        PackageError::TextTooLong {
            field: "decision.what",
            bytes: TEXT_MAX_BYTES + 1
        }
    );
    let state = state_planted(&long, Vec::new());
    assert!(matches!(
        build_package(&input(&state, &[], &[], &[], &[])),
        Err(PackageError::TextTooLong {
            field: "intent.goal",
            ..
        })
    ));
    // Many fields each under the text limit still overflow the package.
    let state = state_with("goal", Vec::new());
    let many: Vec<_> = (0..20)
        .map(|i| {
            (
                WorkRevision::new(i + 1).unwrap(),
                Note::Question {
                    text: "q".repeat(TEXT_MAX_BYTES),
                },
            )
        })
        .collect();
    assert!(matches!(
        build_package(&input(&state, &many, &[], &[], &[])),
        Err(PackageError::TooLarge { bytes }) if bytes > PACKAGE_MAX_BYTES
    ));
    let ok = build_package(&input(&state, &many[..3], &[], &[], &[])).unwrap();
    assert!(ok.canonical_json().len() <= PACKAGE_MAX_BYTES);
}

#[test]
fn planted_credentials_and_approvals_never_enter_a_package() {
    let token = format!("ghp_{}", "a".repeat(40));
    let state = state_with("goal", Vec::new());
    let plants = [
        Note::Decision {
            what: format!("use {token}"),
            reason: "r".into(),
        },
        Note::Decision {
            what: "w".into(),
            reason: format!("because {token}"),
        },
        Note::Question {
            text: format!("is {token} right?"),
        },
        Note::Answer {
            question: WorkRevision::FIRST,
            text: token.clone(),
        },
    ];
    for note in plants {
        let notes = [(WorkRevision::FIRST, note)];
        assert!(matches!(
            build_package(&input(&state, &notes, &[], &[], &[])),
            Err(PackageError::CredentialLike { .. })
        ));
    }
    // The create command refuses a planted goal before the journal records it.
    let planted = Envelope {
        key: OperationKey::parse("create").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: format!("fix with {token}"),
                harness: HarnessKind::ClaudeCode,
                acceptance: Vec::new(),
            },
            source: SourceBinding::Empty,
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    };
    assert!(matches!(
        accept_command(None, &planted, &ids(1)),
        Err(DomainError::CredentialLike { .. })
    ));
    // So is one in an ACP agent's arguments, which every path journals.
    let in_args = Envelope {
        key: OperationKey::parse("create-acp").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: "fix the test".into(),
                harness: HarnessKind::Acp {
                    program: fabric_core::harness::ProgramName::parse("opencode").unwrap(),
                    args: vec!["--api-key".into(), token.clone()],
                },
                acceptance: Vec::new(),
            },
            source: SourceBinding::Empty,
            policy: JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly),
        },
    };
    assert!(matches!(
        accept_command(None, &in_args, &ids(2)),
        Err(DomainError::CredentialLike { .. })
    ));
    // And the builder refuses one that reached a history anyway.
    let state = state_planted(&format!("fix with {token}"), Vec::new());
    assert_eq!(
        build_package(&input(&state, &[], &[], &[], &[])).unwrap_err(),
        PackageError::CredentialLike {
            field: "intent.goal"
        }
    );
    let state = state_planted(
        "goal",
        vec![format!("-----BEGIN RSA PRIVATE KEY----- {token}")],
    );
    assert!(matches!(
        build_package(&input(&state, &[], &[], &[], &[])),
        Err(PackageError::CredentialLike {
            field: "intent.acceptance"
        })
    ));
    let artifacts = [ArtifactRef {
        name: format!("https://user:{token}@host/x"),
        digest: [1; 32],
        bytes: 1,
    }];
    let state = state_with("goal", Vec::new());
    assert!(matches!(
        build_package(&input(&state, &[], &[], &[], &artifacts)),
        Err(PackageError::CredentialLike {
            field: "artifact.name"
        })
    ));
    // The command refuses the plant before the journal records it.
    let note = Envelope {
        key: OperationKey::parse("note").unwrap(),
        job: Some(state.job.id),
        expected: None,
        body: Command::RecordNote {
            note: Note::Question { text: token },
        },
    };
    assert!(matches!(
        accept_command(Some(&state), &note, &ids(9)),
        Err(DomainError::CredentialLike { .. })
    ));
    // A clean package holds no approval of any kind: its JSON has no such key.
    let package = build_package(&input(&state, &[], &[], &[], &[])).unwrap();
    let json = String::from_utf8(package.canonical_json()).unwrap();
    assert!(!json.contains("approval"));
    assert!(!json.contains("token"));
}

#[test]
fn evidence_is_by_digest_only() {
    let state = state_with("goal", Vec::new());
    let evidence = [
        EvidenceRef {
            digest: [7; 32],
            attempt: AttemptId::from_bytes([1; 16]),
            kind: EvidenceKind::Checkpoint,
        },
        EvidenceRef {
            digest: [8; 32],
            attempt: AttemptId::from_bytes([2; 16]),
            kind: EvidenceKind::Exit,
        },
    ];
    let receipts = [TaskReceipt {
        task: state.tasks[1].id,
        class: TaskClass::Test,
        status: ReceiptStatus::Interrupted,
        exit: None,
        interrupted: true,
        workspace: Some(workspace()),
    }];
    let package = build_package(&input(&state, &[], &receipts, &evidence, &[])).unwrap();
    assert_eq!(package.evidence, evidence.to_vec());
    let json = String::from_utf8(package.canonical_json()).unwrap();
    assert!(json.contains(&format!(r#""digest":"{}""#, "07".repeat(32))));
    // The package's size does not grow with the evidence's size: only refs travel.
    let big_evidence: Vec<_> = (0..200u8)
        .map(|i| EvidenceRef {
            digest: [i; 32],
            attempt: AttemptId::from_bytes([1; 16]),
            kind: EvidenceKind::Receipt,
        })
        .collect();
    let bigger = build_package(&input(&state, &[], &receipts, &big_evidence, &[])).unwrap();
    assert!(bigger.canonical_json().len() < 200 * 200);
    // The interrupted test is carried interrupted, never passed.
    let test = package
        .tasks
        .iter()
        .find(|t| t.class == TaskClass::Test)
        .unwrap();
    assert!(!test.receipt.as_ref().unwrap().passed());
    assert!(json.contains(r#""status":"interrupted""#));
    // The digest is stable and under its own domain.
    assert_eq!(package.digest(), package.clone().digest());
    assert_ne!(
        package.digest().0,
        *blake3::hash(&package.canonical_json()).as_bytes()
    );
    assert!(package.next_actions.contains(&NextAction::Continue));
    assert!(package.next_actions.contains(&NextAction::FinishTask {
        task: state.tasks[1].id
    }));
    assert!(!package.next_actions.contains(&NextAction::FinishTask {
        task: state.job.primary
    }));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the handoff's steps read best as one script"
)]
fn a_refusal_is_never_lowered() {
    // A continuation that achieves a fidelity the continue did not allow
    // is refused by the authority, never recorded lower.
    let state = state_with("goal", Vec::new());
    let start = Envelope {
        key: OperationKey::parse("start").unwrap(),
        job: Some(state.job.id),
        expected: None,
        body: Command::StartAttempt {
            task: state.job.primary,
            resource: ResourceRef::Node {
                node: NodeId::from_bytes([9; 32]),
            },
            initial_checkpoint: None,
        },
    };
    let t = accept_command(Some(&state), &start, &ids(3)).unwrap();
    let state = apply_batch(Some(state), t.revision, &t.events).unwrap();
    let first = state.attempts[0].id;
    let cut = CheckpointCut {
        snapshot: SnapshotId::of(b"s"),
        // One receipt per open task: the harness and its test child.
        receipts: state
            .tasks
            .iter()
            .map(|task| TaskReceipt {
                task: task.id,
                class: task.class,
                status: ReceiptStatus::Interrupted,
                exit: None,
                interrupted: true,
                workspace: None,
            })
            .collect(),
        facts: Vec::new(),
        evidence: Vec::new(),
    };
    let t = apply_observation(
        &state,
        &Observation::Checkpointed {
            attempt: first,
            generation: Generation::FIRST,
            snapshot: cut.snapshot,
            cut,
        },
    )
    .unwrap();
    let state = apply_batch(Some(state), t.revision, &t.events).unwrap();
    let only_native = AllowedFidelity {
        native_session: true,
        new_native_session_with_package: false,
        workspace_only: false,
    };
    let cont = Envelope {
        key: OperationKey::parse("continue").unwrap(),
        job: Some(state.job.id),
        expected: None,
        body: Command::Continue {
            task: state.job.primary,
            fidelity: only_native,
            pin: None,
        },
    };
    let t = accept_command(Some(&state), &cont, &ids(4)).unwrap();
    let state = apply_batch(Some(state), t.revision, &t.events).unwrap();
    let decision = PlacementDecision {
        kind: PlacementKind::PlannedHandoff,
        decided_at_ms: 1,
        candidates: Vec::new(),
        chosen: Some(NodeId::from_bytes([9; 32])),
    };
    let t = apply_observation(
        &state,
        &Observation::Placed {
            operation: ids(4).operation,
            task: state.job.primary,
            decision,
        },
    )
    .unwrap();
    let state = apply_batch(Some(state), t.revision, &t.events).unwrap();
    let second = state.attempts[1].id;
    assert_eq!(
        state.attempts[1].initial_checkpoint,
        Some(SnapshotId::of(b"s"))
    );
    let lower = Observation::Continued {
        attempt: second,
        generation: state.job.generation,
        from: first,
        achieved: Fidelity::WorkspaceOnly,
        package: None,
    };
    assert!(matches!(
        apply_observation(&state, &lower),
        Err(DomainError::FidelityRefused { allowed, .. }) if allowed == only_native
    ));
    let native = Observation::Continued {
        attempt: second,
        generation: state.job.generation,
        from: first,
        achieved: Fidelity::NativeSession,
        package: None,
    };
    let t = apply_observation(&state, &native).unwrap();
    assert!(matches!(
        t.events[0],
        ManagedEvent::ContinuationRecorded {
            requested,
            achieved: Fidelity::NativeSession,
            ..
        } if requested == only_native
    ));
    let state = apply_batch(Some(state), t.revision, &t.events).unwrap();
    assert_eq!(
        state.operation(ids(4).operation).unwrap().status,
        OperationStatus::Succeeded
    );
    // A continue that allows nothing is refused at the command.
    let none = AllowedFidelity {
        native_session: false,
        new_native_session_with_package: false,
        workspace_only: false,
    };
    assert!(matches!(
        AllowedFidelity::DEFAULT.highest(&[Fidelity::WorkspaceOnly]),
        Err(FidelityRefusal { .. })
    ));
    assert!(none.highest(&[Fidelity::NativeSession]).is_err());
}

fn advert(harnesses: Vec<HarnessInventory>) -> NodeAdvert {
    NodeAdvert {
        node: NodeId::from_bytes([1; 32]),
        platform: Platform {
            os: "macos".into(),
            arch: "arm64".into(),
        },
        harnesses,
        layout: PathLayout {
            home: AbsPath::parse("/Users/u").unwrap(),
            root: AbsPath::parse("/Users/u/w").unwrap(),
        },
        capacity: Capacity {
            load_1m_milli: 100,
            free_disk_bytes: 1 << 40,
        },
        warmth: Warmth {
            bundle_chain: false,
            workspace: false,
        },
        measured_at_ms: 0,
    }
}

#[test]
fn the_highest_allowed_and_offered_fidelity_wins() {
    let v = |t: &str| HarnessVersion::parse(t).unwrap();
    let claude = HarnessInventory {
        kind: HarnessKind::ClaudeCode,
        version: v("2.1.0"),
    };
    let same = advert(vec![claude.clone()]);
    let offered = offered_fidelity(&HarnessKind::ClaudeCode, Some(&v("2.0.0")), &same, true);
    assert_eq!(
        offered,
        vec![
            Fidelity::NativeSession,
            Fidelity::NewNativeSessionWithPackage,
            Fidelity::WorkspaceOnly
        ]
    );
    assert_eq!(
        AllowedFidelity::DEFAULT.highest(&offered),
        Ok(Fidelity::NativeSession)
    );
    // An older target than the session's newest writer offers no native session.
    let old = offered_fidelity(&HarnessKind::ClaudeCode, Some(&v("2.2.0")), &same, true);
    assert_eq!(old[0], Fidelity::NewNativeSessionWithPackage);
    // Another layout offers no native session either.
    let moved = offered_fidelity(&HarnessKind::ClaudeCode, None, &same, false);
    assert_eq!(moved[0], Fidelity::NewNativeSessionWithPackage);
    // Another harness offers a new session with the package.
    let codex = advert(vec![HarnessInventory {
        kind: HarnessKind::Codex,
        version: v("0.1.0"),
    }]);
    let other = offered_fidelity(&HarnessKind::ClaudeCode, None, &codex, true);
    assert_eq!(
        AllowedFidelity::DEFAULT.highest(&other),
        Ok(Fidelity::NewNativeSessionWithPackage)
    );
    // No harness at all offers the workspace only, which the default does not allow.
    let bare = advert(Vec::new());
    let none = offered_fidelity(&HarnessKind::ClaudeCode, None, &bare, true);
    assert_eq!(none, vec![Fidelity::WorkspaceOnly]);
    assert!(AllowedFidelity::DEFAULT.highest(&none).is_err());
    let opted_in = AllowedFidelity {
        workspace_only: true,
        ..AllowedFidelity::DEFAULT
    };
    assert_eq!(opted_in.highest(&none), Ok(Fidelity::WorkspaceOnly));
}
