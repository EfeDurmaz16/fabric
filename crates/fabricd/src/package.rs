//! The work package of a planned handoff (ADR 0024), built by the
//! authority from its journal at the cut the continue asked for.
//!
//! Invariant: a package holds only what the journal recorded: the job's
//! intent and notes, the last receipts of the attempt it continues, the
//! cut's snapshot and evidence, and the digest of the job's newest event
//! as the ledger head. `build_package` bounds its size and refuses any
//! text that looks like a credential, so a package that leaves this node
//! carries no secret value and is never larger than `PACKAGE_MAX_BYTES`.
//! It names no artifact in M5.

use fabric_core::managed::{
    AttemptId, EvidenceKind, EvidenceRef, ManagedState, PackageError, PackageInput, WorkPackage,
    WorkspaceRevision, build_package,
};
use fabric_journal::{CommitError, Journal};

/// Why no package was built.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildError {
    /// The journal could not be read.
    #[error(transparent)]
    Journal(#[from] CommitError),
    /// The attempt the continue continues was never cut, so there is no
    /// workspace to hand off.
    #[error("attempt {0} has no cut to continue from")]
    NoCut(AttemptId),
    /// The package past its bounds, or holding a credential-like text.
    #[error(transparent)]
    Package(#[from] PackageError),
}

/// The package of the continue that grants a new attempt after `from`:
/// notes from the journal, `from`'s last receipts, the workspace at its
/// newest cut and the job's head, the cut as evidence.
pub(crate) fn build_for_continue(
    journal: &Journal,
    state: &ManagedState,
    from: AttemptId,
) -> Result<WorkPackage, BuildError> {
    let job = state.job.id;
    let (snapshot, cut) = journal
        .last_cut(job, from)?
        .ok_or(BuildError::NoCut(from))?;
    let head = journal.head(job)?.ok_or(BuildError::NoCut(from))?;
    let notes = journal.notes(job)?;
    let receipts = journal.receipts(job, from)?;
    let evidence = [EvidenceRef {
        digest: cut.0,
        attempt: from,
        kind: EvidenceKind::Checkpoint,
    }];
    Ok(build_package(&PackageInput {
        state,
        notes: &notes,
        receipts: &receipts,
        workspace: WorkspaceRevision {
            bundle_head: snapshot,
            ledger_head: head,
        },
        evidence: &evidence,
        artifacts: &[],
    })?)
}

#[cfg(test)]
mod tests {
    use fabric_core::managed::{
        Command, Note, PACKAGE_MAX_BYTES, ReceiptStatus, TaskClass, TaskState,
    };

    use super::*;
    use crate::launch::fixture::{self, OTHER, REMOTE};

    /// The package of a continue is built from the journal at the cut:
    /// the decision noted, the test task's interrupted receipt, the cut's
    /// snapshot and digest, the job's newest event as the ledger head;
    /// within its bound, and a note that looks like a credential never
    /// reaches the journal it is built from.
    #[tokio::test]
    async fn a_package_is_built_from_the_journal_at_the_cut() {
        let world = fixture::World::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let test = world
            .register(job, "test", TaskClass::Test, &["cargo", "test"])
            .await;
        world
            .submit(
                job,
                "note",
                Command::RecordNote {
                    note: Note::Decision {
                        what: "use the fake agent".into(),
                        reason: "the real one needs a login".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert!(
            world
                .submit(
                    job,
                    "leak",
                    Command::RecordNote {
                        note: Note::Decision {
                            what: "token".into(),
                            reason: "see https://u:p4ss@example.invalid/x".into(),
                        },
                    },
                )
                .await
                .is_err(),
            "a credential-like note was taken"
        );
        let snapshot = world.cut(job, first, REMOTE).await;
        world.continued_to(job, OTHER).await;
        let built = world
            .authority
            .with_journal(false, move |journal| {
                let state = journal.job(job)?.expect("the job");
                Ok((
                    build_for_continue(journal, &state, first).map_err(|e| e.to_string()),
                    journal.head(job)?,
                    journal.last_cut(job, first)?,
                ))
            })
            .await
            .unwrap();
        let (package, head, cut) = built;
        let package = package.unwrap();
        let (cut_snapshot, cut_digest) = cut.unwrap();
        assert_eq!(cut_snapshot, snapshot);
        assert_eq!(package.job, job);
        assert_eq!(package.workspace.bundle_head, snapshot);
        assert_eq!(Some(package.workspace.ledger_head), head);
        assert_eq!(package.decisions.len(), 1);
        assert_eq!(package.decisions[0].what, "use the fake agent");
        assert_eq!(
            package.evidence,
            vec![EvidenceRef {
                digest: cut_digest.0,
                attempt: first,
                kind: EvidenceKind::Checkpoint,
            }]
        );
        let summary = package.tasks.iter().find(|t| t.task == test).unwrap();
        assert_eq!(summary.state, TaskState::Open);
        assert_eq!(
            summary.receipt.as_ref().map(|r| r.status.clone()),
            Some(ReceiptStatus::Interrupted)
        );
        assert!(package.artifacts.is_empty());
        let bytes = package.canonical_json();
        assert!(bytes.len() <= PACKAGE_MAX_BYTES);
        assert!(
            !fabric_core::credential::looks_like_credential(&String::from_utf8(bytes).unwrap()),
            "the package carries a credential-like text"
        );
    }
}
