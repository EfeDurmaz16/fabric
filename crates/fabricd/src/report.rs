//! What a node that runs an attempt sends its authority, accepted into
//! the journal (docs/managed-jobs.md, "Launch path"): one report per
//! observation, and a heartbeat that names every grant the node runs.
//!
//! Invariant: a report is applied only while its token is the job's
//! live grant on the node that sends it. `Report::check` refuses a report
//! whose observation names another attempt or generation than its token
//! before the journal is read; a report from another node than the
//! grant's is `WrongNode` and writes nothing; a report at an older
//! generation is recorded as `StaleObservationRefused` by the domain
//! (replay rule 7) and answered `StaleGeneration`, never applied. A
//! heartbeat writes nothing.

use std::collections::BTreeMap;

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::launch::{
    GrantVerdict, Heartbeat, HeartbeatAnswer, MAX_HEARTBEAT_ATTEMPTS, Report, ReportAnswer,
    ReportRefusal,
};
use fabric_core::managed::{DomainError, ManagedEvent, ManagedState, ResourceRef};
use fabric_journal::CommitError;

use crate::authority::{Authority, AuthorityError};
use crate::managed::refusal_code;
use crate::node::Node;

fn refused(reason: ReportRefusal) -> ReportAnswer {
    ReportAnswer::Refused { reason }
}

fn node_of(state: &ManagedState, attempt: fabric_core::managed::AttemptId) -> Option<NodeId> {
    state.attempt(attempt).map(|a| match a.resource {
        ResourceRef::Node { node } => node,
    })
}

/// Accepts `report` from `from` (`None`: this node, `own`) into the
/// authority's journal.
pub(crate) async fn accept(
    authority: &Authority,
    own: NodeId,
    from: Option<NodeId>,
    report: Report,
) -> ReportAnswer {
    if let Err(reason) = report.check() {
        return refused(reason);
    }
    let sender = from.unwrap_or(own);
    let job = report.token.job;
    let state = match authority.job(job).await {
        Ok(Some(state)) => state,
        Ok(None) => return refused(ReportRefusal::UnknownJob),
        Err(AuthorityError::Commit(CommitError::Domain(DomainError::UnknownJob { .. }))) => {
            return refused(ReportRefusal::UnknownJob);
        }
        Err(_) => return refused(ReportRefusal::Unavailable),
    };
    match node_of(&state, report.token.attempt) {
        None => return refused(ReportRefusal::UnknownAttempt),
        Some(node) if node != sender => return refused(ReportRefusal::WrongNode),
        Some(_) => {}
    }
    match authority.observe(job, report.observation).await {
        Ok(transition) => {
            let refused_at = transition.events.iter().find_map(|e| match e {
                ManagedEvent::StaleObservationRefused { current, .. } => Some(*current),
                _ => None,
            });
            match refused_at {
                Some(current) => refused(ReportRefusal::StaleGeneration { current }),
                None => ReportAnswer::Recorded {
                    revision: transition.revision,
                },
            }
        }
        Err(AuthorityError::Commit(CommitError::Domain(DomainError::StaleGeneration {
            current,
            ..
        }))) => refused(ReportRefusal::StaleGeneration { current }),
        Err(AuthorityError::Commit(CommitError::Domain(e))) => refused(ReportRefusal::Domain {
            reason: refusal_code(&e).to_owned(),
        }),
        Err(_) => refused(ReportRefusal::Unavailable),
    }
}

/// One verdict per token of `beat`, in its order, at most
/// `MAX_HEARTBEAT_ATTEMPTS`: `Live` for the job's live grant on the
/// sender, `Stale` when the job moved to a later generation, `Unknown`
/// for a job or attempt the journal does not hold as the sender's live
/// grant. A token whose job cannot be read (the journal is closed,
/// refused or unreadable) gets no verdict, which says nothing of it: a
/// journal that failed to open is not a cancel. Writes nothing.
pub(crate) async fn heartbeat(
    authority: &Authority,
    own: NodeId,
    from: Option<NodeId>,
    beat: &Heartbeat,
) -> HeartbeatAnswer {
    let sender = from.unwrap_or(own);
    let mut jobs: BTreeMap<JobId, Result<Option<ManagedState>, ()>> = BTreeMap::new();
    let mut verdicts = Vec::new();
    for token in beat.attempts.iter().take(MAX_HEARTBEAT_ATTEMPTS) {
        if let std::collections::btree_map::Entry::Vacant(slot) = jobs.entry(token.job) {
            slot.insert(match authority.job(token.job).await {
                Ok(state) => Ok(state),
                Err(AuthorityError::Commit(CommitError::Domain(DomainError::UnknownJob {
                    ..
                }))) => Ok(None),
                Err(_) => Err(()),
            });
        }
        let Some(Ok(state)) = jobs.get(&token.job) else {
            continue;
        };
        let verdict = match state.as_ref() {
            None => GrantVerdict::Unknown,
            Some(state) if state.job.generation > token.generation => GrantVerdict::Stale {
                current: state.job.generation,
            },
            Some(state) => match state.attempt(token.attempt) {
                Some(attempt)
                    if attempt.generation == token.generation
                        && attempt.status.may_run()
                        && node_of(state, attempt.id) == Some(sender) =>
                {
                    GrantVerdict::Live
                }
                _ => GrantVerdict::Unknown,
            },
        };
        verdicts.push((token.attempt, verdict));
    }
    HeartbeatAnswer { verdicts }
}

impl Node {
    /// `Report::check`, then the grant's node against `from` (`None` is
    /// this node; another node is `WrongNode`), then `Authority::observe`;
    /// a report at an older generation answers `StaleGeneration` (the
    /// domain recorded it), another domain refusal answers `Domain` by
    /// its snake-case name, a closed authority `Unavailable`.
    #[allow(
        dead_code,
        reason = "the node's service takes reports through it once the runner lands (owner B)"
    )]
    pub(crate) async fn accept_report(&self, from: Option<NodeId>, report: Report) -> ReportAnswer {
        accept(&self.authority, self.id, from, report).await
    }

    /// One verdict per token: `Live` when the token is the job's live
    /// grant on `from`, `Stale { current }` when the job's generation is
    /// newer, `Unknown` otherwise. Writes nothing.
    #[allow(
        dead_code,
        reason = "the node's service takes heartbeats through it once the runner lands (owner B)"
    )]
    pub(crate) async fn accept_heartbeat(
        &self,
        from: Option<NodeId>,
        beat: &Heartbeat,
    ) -> HeartbeatAnswer {
        heartbeat(&self.authority, self.id, from, beat).await
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::managed::launch::GrantToken;
    use fabric_core::managed::{AttemptStatus, Generation, Observation, OperationStatus};

    use super::*;
    use crate::launch::fixture::{OTHER, OWN, REMOTE, World};

    fn token(state: &ManagedState, attempt: fabric_core::managed::AttemptId) -> GrantToken {
        crate::launch::token_of(state, attempt).unwrap()
    }

    /// A report at an older generation is refused `StaleGeneration` and
    /// recorded as `StaleObservationRefused`; the attempt it names stays
    /// as it was.
    #[tokio::test]
    async fn a_report_at_an_older_generation_is_refused_and_recorded() {
        let world = World::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        world.cut(job, first, REMOTE).await;
        let second = world.continued_to(job, OTHER).await;
        let state = world.state(job).await;
        let old = token(&state, first);
        let answer = accept(
            &world.authority,
            OWN,
            Some(REMOTE),
            Report {
                token: old,
                observation: Observation::Exited {
                    attempt: first,
                    generation: old.generation,
                    outcome: fabric_core::managed::ExitOutcome::Succeeded,
                },
            },
        )
        .await;
        let current = state.attempt(second).unwrap().generation;
        assert_eq!(
            answer,
            ReportAnswer::Refused {
                reason: ReportRefusal::StaleGeneration { current }
            }
        );
        let events = world
            .authority
            .with_journal(false, move |j| Ok(j.events(job, None, 1_000)?))
            .await
            .unwrap();
        assert!(events.iter().any(|e| matches!(
            e.frame.event,
            ManagedEvent::StaleObservationRefused { attempt, .. } if attempt == first
        )));
        let after = world.state(job).await;
        assert!(matches!(
            after.attempt(first).unwrap().status,
            AttemptStatus::Checkpointed { .. }
        ));
    }

    /// Only the node a grant names reports on it; this node's own runner
    /// reports as `None`.
    #[tokio::test]
    async fn a_report_from_another_node_than_the_grant_is_refused() {
        let world = World::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        let state = world.state(job).await;
        let started = Report {
            token: token(&state, first),
            observation: Observation::Started {
                attempt: first,
                generation: Generation::FIRST,
            },
        };
        for from in [Some(OTHER), None] {
            assert_eq!(
                accept(&world.authority, OWN, from, started.clone()).await,
                ReportAnswer::Refused {
                    reason: ReportRefusal::WrongNode
                }
            );
        }
        assert_eq!(
            world.state(job).await.attempt(first).unwrap().status,
            AttemptStatus::Granted
        );
        // From the node the grant names, Started settles the grant.
        assert!(matches!(
            accept(&world.authority, OWN, Some(REMOTE), started).await,
            ReportAnswer::Recorded { .. }
        ));
        let state = world.state(job).await;
        assert_eq!(state.attempt(first).unwrap().status, AttemptStatus::Running);
        assert_eq!(state.operations[0].status, OperationStatus::Succeeded);
        // A grant on this node takes its own runner's report.
        let local = World::new();
        let job = local.created("create").await;
        let attempt = local.granted(job, OWN).await;
        let state = local.state(job).await;
        assert!(matches!(
            accept(
                &local.authority,
                OWN,
                None,
                Report {
                    token: token(&state, attempt),
                    observation: Observation::Started {
                        attempt,
                        generation: Generation::FIRST,
                    },
                },
            )
            .await,
            ReportAnswer::Recorded { .. }
        ));
    }

    /// A report whose observation names another attempt or generation
    /// than its token, or one a node never sends, is refused before the
    /// journal is read: nothing is written.
    #[tokio::test]
    async fn a_report_whose_token_and_observation_differ_is_refused_before_the_journal() {
        let world = World::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        let state = world.state(job).await;
        let before = state.job.revision;
        let token = token(&state, first);
        let other = fabric_core::managed::AttemptId::from_bytes([9; 16]);
        let cases = [
            (
                Observation::Started {
                    attempt: other,
                    generation: token.generation,
                },
                ReportRefusal::TokenMismatch,
            ),
            (
                Observation::Started {
                    attempt: first,
                    generation: token.generation.next().unwrap(),
                },
                ReportRefusal::TokenMismatch,
            ),
            (
                Observation::Lost {
                    attempt: first,
                    generation: token.generation,
                },
                ReportRefusal::NotReportable,
            ),
        ];
        for (observation, reason) in cases {
            assert_eq!(
                accept(
                    &world.authority,
                    OWN,
                    Some(REMOTE),
                    Report { token, observation }
                )
                .await,
                ReportAnswer::Refused { reason }
            );
        }
        // A job this authority does not have is unknown, and a domain
        // refusal is named.
        let mut unknown = token;
        unknown.job = JobId::from_bytes([42; 16]);
        assert_eq!(
            accept(
                &world.authority,
                OWN,
                Some(REMOTE),
                Report {
                    token: unknown,
                    observation: Observation::Started {
                        attempt: first,
                        generation: token.generation,
                    },
                }
            )
            .await,
            ReportAnswer::Refused {
                reason: ReportRefusal::UnknownJob
            }
        );
        assert_eq!(world.state(job).await.job.revision, before);
    }

    /// A heartbeat is answered per token: the live grant on its node is
    /// `Live`, a grant the job moved past is `Stale`, a grant on another
    /// node or of a job this authority does not hold is `Unknown`.
    #[tokio::test]
    async fn a_heartbeat_names_each_grant_live_stale_or_unknown() {
        let world = World::new();
        let job = world.created("create").await;
        let first = world.granted(job, REMOTE).await;
        world.started(job, first).await;
        let state = world.state(job).await;
        let live = token(&state, first);
        let stranger = GrantToken {
            job: JobId::from_bytes([42; 16]),
            ..live
        };
        let beat = Heartbeat {
            attempts: vec![live, stranger],
        };
        let answer = heartbeat(&world.authority, OWN, Some(REMOTE), &beat).await;
        assert_eq!(
            answer.verdicts,
            vec![(first, GrantVerdict::Live), (first, GrantVerdict::Unknown)]
        );
        let elsewhere = heartbeat(&world.authority, OWN, Some(OTHER), &beat).await;
        assert_eq!(elsewhere.verdicts[0], (first, GrantVerdict::Unknown));
        world.cut(job, first, REMOTE).await;
        let second = world.continued_to(job, OTHER).await;
        let current = world.state(job).await.attempt(second).unwrap().generation;
        let answer = heartbeat(&world.authority, OWN, Some(REMOTE), &beat).await;
        assert_eq!(answer.verdicts[0], (first, GrantVerdict::Stale { current }));
        let revision = world.state(job).await.job.revision;
        let _ = heartbeat(&world.authority, OWN, Some(OTHER), &beat).await;
        assert_eq!(
            world.state(job).await.job.revision,
            revision,
            "a heartbeat wrote"
        );
    }

    /// An authority whose journal it cannot open answers a heartbeat with
    /// no verdict for the tokens it cannot read, never `Unknown`: a node
    /// obeys `Unknown` by stopping the attempt, and a journal that failed
    /// to open is not a cancel.
    #[tokio::test]
    async fn a_heartbeat_to_an_authority_that_cannot_read_its_journal_names_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::paths::StateDir::resolve(
            Some(dir.path().as_os_str()),
            std::path::Path::new("/"),
        )
        .unwrap();
        let managed = crate::authority::managed_dir(&state);
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join(fabric_journal::FILE_NAME), b"not a journal").unwrap();
        let authority = Authority::start(&state, OWN).unwrap();
        let token = GrantToken {
            job: JobId::from_bytes([7; 16]),
            attempt: fabric_core::managed::AttemptId::from_bytes([8; 16]),
            generation: fabric_core::managed::Generation::FIRST,
        };
        assert!(
            authority.job(token.job).await.is_err(),
            "the journal is refused"
        );
        let answer = heartbeat(
            &authority,
            OWN,
            Some(REMOTE),
            &Heartbeat {
                attempts: vec![token],
            },
        )
        .await;
        assert_eq!(answer.verdicts, Vec::new());
    }
}
