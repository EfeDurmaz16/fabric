//! The provider operation contract: identity, fencing, the reconciliation
//! state machine, the replacement barrier, cleanup and abandonment
//! (docs/managed-jobs.md, ADR 0021 and ADR 0022).

use fabric_core::managed::{Command, Observation, OperationMark, StoredResult};

use super::*;
use crate::tests::{Fixture, create, envelope, start};

const JOB: JobId = JobId::from_bytes([0x01; 16]);
const TASK: TaskId = TaskId::from_bytes([1; 16]);
const ATTEMPT: AttemptId = AttemptId::from_bytes([2; 16]);

/// A job whose primary task has a granted attempt at generation one.
fn granted() -> Fixture {
    let mut f = Fixture::new();
    f.submit("create", None, create());
    f.submit("start", Some(JOB), start(TASK));
    f
}

fn op(n: u8) -> OperationId {
    OperationId::from_bytes([0x80 + n; 16])
}

/// A locator the bridge would store, with a key of its own.
fn loc(n: u8) -> LocatorRecord {
    LocatorRecord {
        text: format!(
            r#"{{"kind":"sandbox","native_id":"fx-{n}","provider":"fixture","scope":{{}}}}"#
        ),
        key: ResourceKey::from_bytes([n; 32]),
    }
}

fn dispatch(n: u8) -> ProviderDispatch {
    ProviderDispatch {
        operation: op(n),
        key: OperationKey::parse(&format!("p-{n}")).unwrap(),
        digest: RequestDigest::from_bytes([n; 32]),
        job: JOB,
        task: TASK,
        attempt: ATTEMPT,
        generation: Generation::FIRST,
        profile: "fixture".into(),
        action: "sandbox.create".into(),
        effect: EffectClass::Create,
        purpose: Purpose::Effect,
        target: None,
    }
}

fn exec(n: u8, target: &LocatorRecord) -> ProviderDispatch {
    ProviderDispatch {
        action: "sandbox.exec".into(),
        effect: EffectClass::Mutate,
        target: Some(target.clone()),
        ..dispatch(n)
    }
}

fn destroy(n: u8, target: &LocatorRecord) -> ProviderDispatch {
    ProviderDispatch {
        action: "sandbox.destroy".into(),
        effect: EffectClass::Destroy,
        target: Some(target.clone()),
        ..dispatch(n)
    }
}

/// A cleanup of create `creating`, whose locator is `target`.
fn cleanup(n: u8, creating: u8, target: &LocatorRecord) -> ProviderDispatch {
    ProviderDispatch {
        purpose: Purpose::Cleanup {
            cleans: op(creating),
        },
        ..destroy(n, target)
    }
}

fn none() -> ProviderAnswer {
    ProviderAnswer::default()
}

fn status(f: &Fixture, n: u8) -> ProviderStatus {
    f.journal.provider_operation(op(n)).unwrap().unwrap().status
}

fn begin(f: &mut Fixture, d: &ProviderDispatch) {
    f.journal.provider_begin(d, None, 1).unwrap();
}

fn to(f: &mut Fixture, n: u8, s: ProviderStatus, locator: Option<&LocatorRecord>) {
    f.journal
        .provider_record(op(n), s, &none(), locator, 2)
        .unwrap();
}

/// A create of the job that succeeded with locator `loc(n)`.
fn created(f: &mut Fixture, n: u8) {
    begin(f, &dispatch(n));
    to(f, n, ProviderStatus::Succeeded, Some(&loc(n)));
}

/// The job's attempt is lost, so a replacement may be asked for.
fn lose(f: &mut Fixture) {
    f.journal
        .mark(
            JOB,
            OperationId::from_bytes([2; 16]),
            OperationMark::Dispatched,
            3,
        )
        .unwrap();
    f.journal
        .observe(
            JOB,
            &Observation::Lost {
                attempt: ATTEMPT,
                generation: Generation::FIRST,
            },
            4,
        )
        .unwrap();
}

/// Asks for a replacement attempt under `key`.
fn replace(f: &mut Fixture, key: &str) -> StoredResult {
    f.submit(key, Some(JOB), start(TASK)).result
}

fn blocked_by(result: &StoredResult) -> Option<OperationId> {
    match result {
        StoredResult::Rejected(DomainError::ReplacementBlocked { operation }) => Some(*operation),
        _ => None,
    }
}

fn abandon(n: u8, expected: ProviderStatus, acknowledged: Acknowledged) -> AbandonApproval {
    AbandonApproval {
        operation: op(n),
        expected,
        acknowledged,
        reason: "the owner removed it by hand".into(),
    }
}

fn refusal(result: Result<(), CommitError>) -> ProviderError {
    match result {
        Err(CommitError::Provider(e)) => e,
        other => panic!("not a provider refusal: {other:?}"),
    }
}

/// A key or id the journal holds, a command's included, is never
/// recorded again, so nothing the journal holds is sent twice.
#[test]
fn a_key_or_an_id_is_dispatched_once() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    let again = f.journal.provider_begin(&dispatch(1), None, 2);
    assert!(matches!(refusal(again), ProviderError::KeyUsed { .. }));
    let mut command_key = dispatch(2);
    command_key.key = OperationKey::parse("start").unwrap();
    assert!(matches!(
        refusal(f.journal.provider_begin(&command_key, None, 3)),
        ProviderError::KeyUsed { .. }
    ));
    let mut same_id = dispatch(3);
    same_id.operation = op(1);
    assert!(matches!(
        refusal(f.journal.provider_begin(&same_id, None, 4)),
        ProviderError::OperationUsed { .. }
    ));
    let mut command_id = dispatch(4);
    command_id.operation = OperationId::from_bytes([2; 16]);
    assert!(matches!(
        refusal(f.journal.provider_begin(&command_id, None, 5)),
        ProviderError::OperationUsed { .. }
    ));
}

/// Keys are one space: a command cannot take a provider operation's key.
#[test]
fn a_command_cannot_take_a_provider_key() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    let envelope = envelope(
        "p-1",
        Some(JOB),
        Command::CancelJob {
            reason: "late".into(),
        },
    );
    let ids = f.ids();
    assert!(matches!(
        f.journal.submit(&envelope, ids),
        Err(CommitError::Domain(DomainError::OperationConflict { .. }))
    ));
}

/// Fabric fences: only the job's current, live attempt at its own
/// generation can ask for an effect.
#[test]
fn only_the_current_live_attempt_dispatches() {
    let mut f = granted();
    let mut stale = dispatch(1);
    stale.generation = Generation::new(2).unwrap();
    assert!(matches!(
        f.journal.provider_begin(&stale, None, 1),
        Err(CommitError::Domain(DomainError::StaleGeneration { .. }))
    ));
    let mut other_task = dispatch(2);
    other_task.task = TaskId::from_bytes([9; 16]);
    assert!(matches!(
        f.journal.provider_begin(&other_task, None, 2),
        Err(CommitError::Domain(DomainError::UnknownTask { .. }))
    ));
    lose(&mut f);
    assert!(matches!(
        f.journal.provider_begin(&dispatch(3), None, 5),
        Err(CommitError::Domain(DomainError::AttemptEnded { .. }))
    ));
    assert!(f.journal.provider_operation(op(3)).unwrap().is_none());
}

/// A destroy whose worker never answered, then a restart: the operation
/// is unknown and still names the resource it acted on, so it can be
/// observed by its original target. No answer changes the dispatch.
#[test]
fn the_target_is_kept_from_dispatch_through_recovery() {
    let mut f = granted();
    let d = destroy(1, &loc(1));
    begin(&mut f, &d);
    let recovery = f
        .journal
        .recover(2, |_| crate::Liveness::CannotTell)
        .unwrap();
    assert_eq!(recovery.provider_unknown, vec![(JOB, op(1))]);
    to(&mut f, 1, ProviderStatus::Succeeded, Some(&loc(2)));
    let row = f.journal.provider_operation(op(1)).unwrap().unwrap();
    assert_eq!(
        row.dispatch, d,
        "the dispatch, its target included, is kept"
    );
    assert_eq!(row.locator, Some(loc(2)));
}

/// Every pair of statuses against the transition table: a record moves
/// only along the machine, `abandoned` is reached only by the owner from
/// `unknown`, `attached` or `observed`, and a terminal status never moves.
#[test]
fn statuses_move_only_along_the_machine() {
    use ProviderStatus::{
        Abandoned, Attached, Cancelled, Dispatched, Failed, Observed, Rejected, Succeeded, Unknown,
    };
    let path = |s: ProviderStatus| -> &'static [ProviderStatus] {
        match s {
            Dispatched => &[],
            Unknown => &[Unknown],
            Attached => &[Unknown, Attached],
            Observed => &[Unknown, Observed],
            Succeeded => &[Succeeded],
            Failed => &[Failed],
            Cancelled => &[Cancelled],
            Rejected => &[Rejected],
            Abandoned => &[Unknown, Abandoned],
        }
    };
    let allowed = |from: ProviderStatus, to: ProviderStatus| -> bool {
        matches!(
            (from, to),
            (
                Dispatched,
                Dispatched | Unknown | Succeeded | Failed | Cancelled | Rejected
            ) | (
                Unknown,
                Unknown | Attached | Observed | Succeeded | Failed | Cancelled
            ) | (
                Attached,
                Attached | Observed | Succeeded | Failed | Cancelled
            ) | (Observed, Observed | Succeeded | Failed | Cancelled)
        )
    };
    let mut f = granted();
    let mut n = 0u8;
    for from in ProviderStatus::ALL {
        for target in ProviderStatus::ALL {
            n += 1;
            // An exec, so the create guard plays no part, with a target,
            // so attached and observed name a resource.
            begin(&mut f, &exec(n, &loc(1)));
            for step in path(from) {
                if *step == Abandoned {
                    let ack = Acknowledged::Resource(loc(1).key);
                    f.journal
                        .provider_abandon(&abandon(n, Unknown, ack), 3)
                        .unwrap();
                } else {
                    to(&mut f, n, *step, None);
                }
            }
            assert_eq!(status(&f, n), from);
            let recorded = f.journal.provider_record(op(n), target, &none(), None, 4);
            assert_eq!(
                recorded.is_ok(),
                allowed(from, target),
                "{} to {}: {recorded:?}",
                from.name(),
                target.name()
            );
            let now = status(&f, n);
            let abandoned = f
                .journal
                .provider_abandon(&abandon(n, now, Acknowledged::Resource(loc(1).key)), 5);
            assert_eq!(
                abandoned.is_ok(),
                matches!(now, Unknown | Attached | Observed),
                "abandon from {}: {abandoned:?}",
                now.name()
            );
        }
    }
    // A create with no locator and no target cannot be attached or observed.
    let mut g = granted();
    begin(&mut g, &dispatch(1));
    to(&mut g, 1, Unknown, None);
    for s in [Attached, Observed] {
        assert!(matches!(
            refusal(g.journal.provider_record(op(1), s, &none(), None, 6)),
            ProviderError::Transition { .. }
        ));
    }
    to(&mut g, 1, Attached, Some(&loc(1)));
    assert_eq!(
        g.journal
            .provider_operation(op(1))
            .unwrap()
            .unwrap()
            .locator,
        Some(loc(1))
    );
}

/// A create lost with its answer leaves the job's next create waiting
/// until the lost one is attached, observed, settled or abandoned: never
/// a second resource beside one nobody can name.
#[test]
fn a_create_waits_while_a_create_of_the_job_is_unknown() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    // Two creates in flight at once are not a duplicate.
    begin(&mut f, &dispatch(2));
    to(&mut f, 1, ProviderStatus::Unknown, None);
    for (n, refused) in [(3, None), (4, Some(answer("denied", "policy_denied")))] {
        assert_eq!(
            refusal(f.journal.provider_begin(&dispatch(n), refused.as_ref(), 3)),
            ProviderError::CreateUnresolved { operation: op(1) }
        );
        assert!(f.journal.provider_operation(op(n)).unwrap().is_none());
    }
    // Not an exec or a destroy.
    begin(&mut f, &exec(5, &loc(2)));
    begin(&mut f, &destroy(6, &loc(2)));
    // Attached, it no longer holds creates back.
    to(&mut f, 1, ProviderStatus::Attached, Some(&loc(1)));
    begin(&mut f, &dispatch(7));
    to(&mut f, 7, ProviderStatus::Unknown, None);
    assert!(matches!(
        refusal(f.journal.provider_begin(&dispatch(8), None, 4)),
        ProviderError::CreateUnresolved { .. }
    ));
    // Nor once the owner abandoned it.
    f.journal
        .provider_abandon(
            &abandon(7, ProviderStatus::Unknown, Acknowledged::Nothing),
            5,
        )
        .unwrap();
    begin(&mut f, &dispatch(8));
}

fn answer(code: &str, reason: &str) -> ProviderAnswer {
    ProviderAnswer {
        code: Some(code.into()),
        reason: Some(reason.into()),
        effect: Some("none".into()),
    }
}

/// An authority restart makes a dispatched create unknown; it is never
/// resent (its key and id stay used), another create waits, and an
/// attach that finds its resource moves it on with the locator.
#[test]
fn recovery_makes_a_dispatched_create_unknown_and_it_is_attached_not_resent() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    f.reopen();
    let recovery = f
        .journal
        .recover(2, |_| crate::Liveness::CannotTell)
        .unwrap();
    assert_eq!(recovery.provider_unknown, vec![(JOB, op(1))]);
    assert_eq!(status(&f, 1), ProviderStatus::Unknown);
    assert!(matches!(
        refusal(f.journal.provider_begin(&dispatch(1), None, 3)),
        ProviderError::KeyUsed { .. }
    ));
    assert!(matches!(
        refusal(f.journal.provider_begin(&dispatch(2), None, 3)),
        ProviderError::CreateUnresolved { .. }
    ));
    // The attach found it still starting: attached, with its locator.
    let pending = ProviderAnswer {
        code: Some("pending".into()),
        reason: Some("observation_deadline".into()),
        effect: Some("pending".into()),
    };
    f.journal
        .provider_record(op(1), ProviderStatus::Attached, &pending, Some(&loc(1)), 4)
        .unwrap();
    let row = f.journal.provider_operation(op(1)).unwrap().unwrap();
    assert_eq!(
        (row.status, row.locator),
        (ProviderStatus::Attached, Some(loc(1)))
    );
    // A second recovery leaves it so; an observe settles it.
    assert!(
        f.journal
            .recover(5, |_| crate::Liveness::CannotTell)
            .unwrap()
            .provider_unknown
            .is_empty()
    );
    to(&mut f, 1, ProviderStatus::Succeeded, None);
    assert_eq!(
        f.journal
            .provider_operation(op(1))
            .unwrap()
            .unwrap()
            .locator,
        Some(loc(1))
    );
}

/// An absent attach keeps the create unknown and stores the scope it
/// searched; it is not a status any read could settle.
#[test]
fn an_absent_attach_keeps_unknown_and_stores_what_it_searched() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    to(&mut f, 1, ProviderStatus::Unknown, None);
    let absent = ProviderAnswer {
        code: Some("unknown".into()),
        reason: Some("absent".into()),
        effect: Some("unknown".into()),
    };
    let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{}}"#;
    f.journal
        .provider_searched(op(1), &absent, searched, 2)
        .unwrap();
    let row = f.journal.provider_operation(op(1)).unwrap().unwrap();
    assert_eq!(row.status, ProviderStatus::Unknown);
    assert_eq!(row.answer, absent);
    assert_eq!(row.searched.as_deref(), Some(searched));
    // Only an unknown operation was searched for.
    begin(&mut f, &exec(2, &loc(1)));
    assert!(matches!(
        refusal(f.journal.provider_searched(op(2), &absent, searched, 3)),
        ProviderError::Transition { .. }
    ));
}

/// Restart recovery moves only `dispatched`: attached, observed and
/// abandoned hold no request in flight.
#[test]
fn recovery_leaves_attached_observed_and_abandoned_as_they_are() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    to(&mut f, 1, ProviderStatus::Unknown, None);
    to(&mut f, 1, ProviderStatus::Attached, Some(&loc(1)));
    begin(&mut f, &destroy(2, &loc(2)));
    to(&mut f, 2, ProviderStatus::Unknown, None);
    to(&mut f, 2, ProviderStatus::Observed, None);
    begin(&mut f, &exec(3, &loc(3)));
    to(&mut f, 3, ProviderStatus::Unknown, None);
    f.journal
        .provider_abandon(
            &abandon(
                3,
                ProviderStatus::Unknown,
                Acknowledged::Resource(loc(3).key),
            ),
            3,
        )
        .unwrap();
    begin(&mut f, &exec(4, &loc(3)));
    f.reopen();
    let recovery = f
        .journal
        .recover(4, |_| crate::Liveness::CannotTell)
        .unwrap();
    assert_eq!(recovery.provider_unknown, vec![(JOB, op(4))]);
    assert_eq!(status(&f, 1), ProviderStatus::Attached);
    assert_eq!(status(&f, 2), ProviderStatus::Observed);
    assert_eq!(status(&f, 3), ProviderStatus::Abandoned);
    assert_eq!(
        f.journal
            .provider_operation(op(3))
            .unwrap()
            .unwrap()
            .abandon_reason
            .as_deref(),
        Some("the owner removed it by hand")
    );
}

/// The owner's abandonment applies only to the row as the owner saw it:
/// its status and what it names.
#[test]
fn an_abandonment_needs_the_state_the_owner_saw() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    let resource = |n| Acknowledged::Resource(loc(n).key);
    // Not while dispatched.
    assert_eq!(
        refusal(f.journal.provider_abandon(
            &abandon(1, ProviderStatus::Dispatched, Acknowledged::Nothing),
            1
        )),
        ProviderError::Abandon {
            operation: op(1),
            refusal: AbandonRefusal::NotAbandonable {
                status: ProviderStatus::Dispatched
            }
        }
    );
    to(&mut f, 1, ProviderStatus::Unknown, None);
    // The owner saw it attached, but it is unknown.
    let stale = f
        .journal
        .provider_abandon(&abandon(1, ProviderStatus::Attached, resource(1)), 2);
    assert!(matches!(
        refusal(stale),
        ProviderError::Abandon {
            refusal: AbandonRefusal::Stale { .. },
            ..
        }
    ));
    // It names nothing, so a resource is not what the owner accepts.
    let wrong = f
        .journal
        .provider_abandon(&abandon(1, ProviderStatus::Unknown, resource(1)), 3);
    assert!(matches!(
        refusal(wrong),
        ProviderError::Abandon {
            refusal: AbandonRefusal::Acknowledgement,
            ..
        }
    ));
    // After an absent attach it names what was searched.
    let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{}}"#;
    f.journal
        .provider_searched(op(1), &none(), searched, 4)
        .unwrap();
    let nothing = f.journal.provider_abandon(
        &abandon(1, ProviderStatus::Unknown, Acknowledged::Nothing),
        5,
    );
    assert!(matches!(
        refusal(nothing),
        ProviderError::Abandon {
            refusal: AbandonRefusal::Acknowledgement,
            ..
        }
    ));
    for reason in ["", "a\nb", &"x".repeat(MAX_ABANDON_REASON + 1)] {
        let mut a = abandon(
            1,
            ProviderStatus::Unknown,
            Acknowledged::Searched(searched.into()),
        );
        a.reason = reason.to_owned();
        assert!(matches!(
            refusal(f.journal.provider_abandon(&a, 6)),
            ProviderError::Abandon {
                refusal: AbandonRefusal::Reason,
                ..
            }
        ));
    }
    assert_eq!(
        status(&f, 1),
        ProviderStatus::Unknown,
        "nothing was written"
    );
    f.journal
        .provider_abandon(
            &abandon(
                1,
                ProviderStatus::Unknown,
                Acknowledged::Searched(searched.into()),
            ),
            7,
        )
        .unwrap();
    assert_eq!(status(&f, 1), ProviderStatus::Abandoned);
    // Abandoned is terminal: no read and no second abandonment moves it.
    assert!(
        f.journal
            .provider_record(op(1), ProviderStatus::Succeeded, &none(), None, 8)
            .is_err()
    );
    assert!(matches!(
        refusal(f.journal.provider_abandon(
            &abandon(1, ProviderStatus::Abandoned, Acknowledged::Nothing),
            9
        )),
        ProviderError::Abandon {
            refusal: AbandonRefusal::NotAbandonable { .. },
            ..
        }
    ));
}

/// A stop that failed or that nobody saw finish keeps the replacement
/// waiting, under the refusal stored with its key, until a destroy of the
/// same resource succeeds.
#[test]
fn a_failed_or_unknown_destroy_blocks_a_replacement_until_a_destroy_succeeds() {
    for path in [
        &[ProviderStatus::Failed][..],
        &[ProviderStatus::Cancelled],
        &[ProviderStatus::Unknown],
        &[ProviderStatus::Unknown, ProviderStatus::Observed],
    ] {
        let mut f = granted();
        created(&mut f, 1);
        begin(&mut f, &destroy(2, &loc(1)));
        for step in path {
            to(&mut f, 2, *step, None);
        }
        let stop = status(&f, 2);
        // A destroy of another resource that succeeded does not count.
        begin(&mut f, &destroy(3, &loc(9)));
        to(&mut f, 3, ProviderStatus::Succeeded, None);
        lose(&mut f);
        let blockers = f.journal.replacement_blockers(JOB).unwrap();
        assert_eq!(
            blockers,
            vec![Blocker::StopNotConfirmed {
                operation: op(2),
                status: status(&f, 2)
            }],
            "{}",
            stop.name()
        );
        let refused = replace(&mut f, "replace-1");
        assert_eq!(blocked_by(&refused), Some(op(2)));
        // A stop nobody saw finish is observed first: a fresh destroy
        // waits while it may still run.
        if !stop.is_terminal() {
            assert!(matches!(
                refusal(f.journal.provider_begin(&cleanup(4, 1, &loc(1)), None, 5)),
                ProviderError::Cleanup {
                    refusal: CleanupRefusal::InFlight { .. },
                    ..
                }
            ));
            // Abandoned, it no longer holds the cleanup back, and it
            // still blocks the replacement.
            f.journal
                .provider_abandon(&abandon(2, stop, Acknowledged::Resource(loc(1).key)), 6)
                .unwrap();
            assert_eq!(blocked_by(&replace(&mut f, "replace-0")), Some(op(2)));
        }
        // A fresh destroy of that resource, a cleanup now the attempt
        // ended, that succeeds, unblocks it.
        begin(&mut f, &cleanup(4, 1, &loc(1)));
        to(&mut f, 4, ProviderStatus::Succeeded, None);
        assert!(f.journal.replacement_blockers(JOB).unwrap().is_empty());
        // The refusal stays under its key; a new key is granted.
        assert_eq!(blocked_by(&replace(&mut f, "replace-1")), Some(op(2)));
        assert_eq!(blocked_by(&replace(&mut f, "replace-2")), None);
        assert_eq!(f.journal.job(JOB).unwrap().unwrap().job.generation.get(), 2);
    }
}

/// An exec that may still run blocks a replacement until its sandbox is
/// destroyed; a settled exec does not.
#[test]
fn an_unsettled_exec_blocks_a_replacement_until_its_sandbox_is_destroyed() {
    let mut f = granted();
    created(&mut f, 1);
    begin(&mut f, &exec(2, &loc(1)));
    begin(&mut f, &exec(3, &loc(1)));
    to(&mut f, 3, ProviderStatus::Succeeded, None);
    to(&mut f, 2, ProviderStatus::Unknown, None);
    lose(&mut f);
    assert_eq!(
        f.journal.replacement_blockers(JOB).unwrap(),
        vec![Blocker::ExecMayRun {
            operation: op(2),
            status: ProviderStatus::Unknown
        }]
    );
    assert_eq!(blocked_by(&replace(&mut f, "replace-1")), Some(op(2)));
    begin(&mut f, &cleanup(4, 1, &loc(1)));
    to(&mut f, 4, ProviderStatus::Succeeded, None);
    assert_eq!(blocked_by(&replace(&mut f, "replace-2")), None);
}

/// Abandoning a stop or an exec gives up on knowing, it does not make
/// the resource safe: the replacement still waits.
#[test]
fn abandoning_a_destroy_or_an_exec_never_unblocks_a_replacement() {
    let mut f = granted();
    created(&mut f, 1);
    begin(&mut f, &exec(2, &loc(1)));
    to(&mut f, 2, ProviderStatus::Unknown, None);
    begin(&mut f, &destroy(3, &loc(1)));
    to(&mut f, 3, ProviderStatus::Unknown, None);
    for n in [2, 3] {
        f.journal
            .provider_abandon(
                &abandon(
                    n,
                    ProviderStatus::Unknown,
                    Acknowledged::Resource(loc(1).key),
                ),
                5,
            )
            .unwrap();
    }
    lose(&mut f);
    assert_eq!(
        f.journal.replacement_blockers(JOB).unwrap(),
        vec![
            Blocker::ExecMayRun {
                operation: op(2),
                status: ProviderStatus::Abandoned
            },
            Blocker::StopNotConfirmed {
                operation: op(3),
                status: ProviderStatus::Abandoned
            },
        ]
    );
    assert_eq!(blocked_by(&replace(&mut f, "replace-1")), Some(op(2)));
}

/// A create that names its resource, or that the owner abandoned, does
/// not block a replacement; one that is dispatched or unknown does.
#[test]
fn an_attached_or_abandoned_create_does_not_block() {
    let mut f = granted();
    begin(&mut f, &dispatch(1));
    to(&mut f, 1, ProviderStatus::Unknown, None);
    lose(&mut f);
    assert_eq!(
        f.journal.replacement_blockers(JOB).unwrap(),
        vec![Blocker::CreateUnresolved {
            operation: op(1),
            status: ProviderStatus::Unknown
        }]
    );
    assert_eq!(blocked_by(&replace(&mut f, "replace-1")), Some(op(1)));
    to(&mut f, 1, ProviderStatus::Attached, Some(&loc(1)));
    assert!(f.journal.replacement_blockers(JOB).unwrap().is_empty());

    let mut g = granted();
    begin(&mut g, &dispatch(1));
    to(&mut g, 1, ProviderStatus::Unknown, None);
    g.journal
        .provider_abandon(
            &abandon(1, ProviderStatus::Unknown, Acknowledged::Nothing),
            3,
        )
        .unwrap();
    lose(&mut g);
    assert_eq!(blocked_by(&replace(&mut g, "replace-1")), None);
}

/// A cleanup is not an effect of the attempt: it is accepted after the
/// job was cancelled, and its destroy that succeeds removes the residual.
#[test]
fn a_cleanup_runs_after_the_job_is_cancelled() {
    let mut f = granted();
    created(&mut f, 1);
    lose(&mut f);
    f.submit(
        "cancel",
        Some(JOB),
        Command::CancelJob {
            reason: "done with it".into(),
        },
    );
    assert_ne!(
        f.journal.job(JOB).unwrap().unwrap().job.state,
        JobState::Active
    );
    // An effect is fenced; a cleanup of the create is not.
    assert!(matches!(
        f.journal.provider_begin(&destroy(2, &loc(1)), None, 5),
        Err(CommitError::Domain(DomainError::JobClosed { .. }))
    ));
    begin(&mut f, &cleanup(3, 1, &loc(1)));
    let row = f.journal.provider_operation(op(3)).unwrap().unwrap();
    assert_eq!(row.dispatch.purpose, Purpose::Cleanup { cleans: op(1) });
    assert_eq!(
        f.journal.residuals(JOB).unwrap().len(),
        1,
        "not yet destroyed"
    );
    to(&mut f, 3, ProviderStatus::Succeeded, None);
    assert!(f.journal.residuals(JOB).unwrap().is_empty());
}

/// A cleanup names a create of this journal with a resource, in that
/// create's own scope, that no destroy removed or is removing; anything
/// else is refused before anything is written.
#[test]
fn a_cleanup_needs_a_named_resource_of_this_job() {
    let cleans = |f: &mut Fixture, d: &ProviderDispatch| -> CleanupRefusal {
        match refusal(f.journal.provider_begin(d, None, 9)) {
            ProviderError::Cleanup { refusal, .. } => refusal,
            other => panic!("{other:?}"),
        }
    };
    let mut f = granted();
    // No such operation, and an operation that is not a create.
    assert_eq!(
        cleans(&mut f, &cleanup(9, 1, &loc(1))),
        CleanupRefusal::NotACreate
    );
    begin(&mut f, &exec(1, &loc(1)));
    assert_eq!(
        cleans(&mut f, &cleanup(9, 1, &loc(1))),
        CleanupRefusal::NotACreate
    );
    // A create with no locator, then one still dispatched.
    begin(&mut f, &dispatch(2));
    assert_eq!(
        cleans(&mut f, &cleanup(9, 2, &loc(2))),
        CleanupRefusal::Unlocated
    );
    f.journal.provider_locate(op(2), &loc(2), 3).unwrap();
    assert_eq!(
        cleans(&mut f, &cleanup(9, 2, &loc(2))),
        CleanupRefusal::StillDispatched
    );
    to(&mut f, 2, ProviderStatus::Succeeded, None);
    // Another resource, another scope, or not a destroy.
    assert_eq!(
        cleans(&mut f, &cleanup(9, 2, &loc(3))),
        CleanupRefusal::NotACreate
    );
    let mut other_attempt = cleanup(9, 2, &loc(2));
    other_attempt.attempt = AttemptId::from_bytes([7; 16]);
    assert_eq!(cleans(&mut f, &other_attempt), CleanupRefusal::NotACreate);
    let mut not_destroy = cleanup(9, 2, &loc(2));
    not_destroy.effect = EffectClass::Mutate;
    assert_eq!(cleans(&mut f, &not_destroy), CleanupRefusal::NotACreate);
    // A destroy of it in flight, then one that succeeded.
    begin(&mut f, &destroy(4, &loc(2)));
    assert_eq!(
        cleans(&mut f, &cleanup(9, 2, &loc(2))),
        CleanupRefusal::InFlight { operation: op(4) }
    );
    to(&mut f, 4, ProviderStatus::Succeeded, None);
    assert_eq!(
        cleans(&mut f, &cleanup(9, 2, &loc(2))),
        CleanupRefusal::AlreadyDestroyed
    );
    assert!(f.journal.provider_operation(op(9)).unwrap().is_none());
}

/// Residuals list what the job's creates may have left that no destroy
/// removed: with a locator when one is known, else with what an attach
/// searched.
#[test]
fn residuals_list_what_no_cleanup_removed() {
    let mut f = granted();
    created(&mut f, 1);
    created(&mut f, 2);
    // A create that failed after making something, and one that failed clean.
    begin(&mut f, &dispatch(3));
    to(&mut f, 3, ProviderStatus::Failed, Some(&loc(3)));
    begin(&mut f, &dispatch(4));
    to(&mut f, 4, ProviderStatus::Failed, None);
    // A create refused before any effect.
    f.journal
        .provider_begin(&dispatch(5), Some(&answer("denied", "policy_denied")), 1)
        .unwrap();
    // A lost create an attach did not find.
    begin(&mut f, &dispatch(6));
    to(&mut f, 6, ProviderStatus::Unknown, None);
    let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{}}"#;
    f.journal
        .provider_searched(op(6), &none(), searched, 3)
        .unwrap();
    // The second create's sandbox was destroyed.
    begin(&mut f, &destroy(7, &loc(2)));
    to(&mut f, 7, ProviderStatus::Succeeded, None);
    let residuals = f.journal.residuals(JOB).unwrap();
    assert_eq!(
        residuals,
        vec![
            Residual {
                creating: op(1),
                status: ProviderStatus::Succeeded,
                locator: Some(loc(1)),
                searched: None
            },
            Residual {
                creating: op(3),
                status: ProviderStatus::Failed,
                locator: Some(loc(3)),
                searched: None
            },
            Residual {
                creating: op(6),
                status: ProviderStatus::Unknown,
                locator: None,
                searched: Some(searched.into())
            },
        ]
    );
}

/// A destroy the provider answers `not_found` says the provider did not
/// find the resource, not where it went: it stays `unknown` with that
/// answer visible on its row, keeps the replacement barrier and holds a
/// fresh cleanup back. An observe that finds the resource absent in the
/// scope it searched is the evidence the barrier asks for: the destroy
/// settles `succeeded` with the scope stored, the residual is gone and a
/// replacement is granted.
#[test]
fn a_not_found_destroy_stays_unknown_until_an_observe_finds_its_resource_absent() {
    let mut f = granted();
    created(&mut f, 1);
    begin(&mut f, &destroy(2, &loc(1)));
    let not_found = answer("provider_failed", "not_found");
    f.journal
        .provider_record(op(2), ProviderStatus::Unknown, &not_found, None, 2)
        .unwrap();
    let row = f.journal.provider_operation(op(2)).unwrap().unwrap();
    assert_eq!(row.status, ProviderStatus::Unknown);
    assert_eq!(row.answer, not_found, "the reason stays visible");
    lose(&mut f);
    assert_eq!(
        f.journal.replacement_blockers(JOB).unwrap(),
        vec![Blocker::StopNotConfirmed {
            operation: op(2),
            status: ProviderStatus::Unknown
        }]
    );
    assert_eq!(blocked_by(&replace(&mut f, "replace-1")), Some(op(2)));
    assert_eq!(f.journal.residuals(JOB).unwrap().len(), 1);
    assert!(matches!(
        refusal(f.journal.provider_begin(&cleanup(3, 1, &loc(1)), None, 3)),
        ProviderError::Cleanup {
            refusal: CleanupRefusal::InFlight { .. },
            ..
        }
    ));

    let absent = ProviderAnswer {
        code: Some("unknown".into()),
        reason: Some("absent".into()),
        effect: Some("unknown".into()),
    };
    let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{}}"#;
    f.journal
        .provider_absent(op(2), &absent, searched, 4)
        .unwrap();
    let row = f.journal.provider_operation(op(2)).unwrap().unwrap();
    assert_eq!(row.status, ProviderStatus::Succeeded);
    assert_eq!(row.answer, absent);
    assert_eq!(row.searched.as_deref(), Some(searched));
    assert!(f.journal.replacement_blockers(JOB).unwrap().is_empty());
    assert_eq!(f.journal.residuals(JOB).unwrap(), vec![]);
    assert_eq!(blocked_by(&replace(&mut f, "replace-2")), None);
    // Settled, it never moves again.
    assert!(matches!(
        refusal(f.journal.provider_absent(op(2), &absent, searched, 5)),
        ProviderError::Transition { .. }
    ));
}

/// Absence settles a destroy only. A create or an exec whose resource an
/// observe did not find stays where it is, as after an absent attach, and
/// a destroy settled `failed` is not rewritten by a later look.
#[test]
fn an_absent_observe_settles_nothing_but_a_destroy() {
    let mut f = granted();
    created(&mut f, 1);
    begin(&mut f, &exec(2, &loc(1)));
    to(&mut f, 2, ProviderStatus::Unknown, None);
    begin(&mut f, &dispatch(3));
    to(&mut f, 3, ProviderStatus::Unknown, None);
    begin(&mut f, &destroy(4, &loc(1)));
    to(&mut f, 4, ProviderStatus::Failed, None);
    let absent = ProviderAnswer {
        code: Some("unknown".into()),
        reason: Some("absent".into()),
        effect: Some("unknown".into()),
    };
    let searched = r#"{"kind":"sandbox","provider":"fixture","scope":{}}"#;
    for (n, was) in [
        (2, ProviderStatus::Unknown),
        (3, ProviderStatus::Unknown),
        (4, ProviderStatus::Failed),
    ] {
        assert!(
            matches!(
                refusal(f.journal.provider_absent(op(n), &absent, searched, 5)),
                ProviderError::Transition { .. }
            ),
            "{n}"
        );
        let row = f.journal.provider_operation(op(n)).unwrap().unwrap();
        assert_eq!(row.status, was, "{n}");
        assert_eq!(row.searched, None, "{n}");
    }
    assert!(matches!(
        refusal(f.journal.provider_absent(op(9), &absent, searched, 5)),
        ProviderError::UnknownOperation { .. }
    ));
}
