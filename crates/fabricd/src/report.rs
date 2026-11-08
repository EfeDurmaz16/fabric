//! What a node that runs an attempt sends its authority, accepted into
//! the journal (docs/managed-jobs.md, "Launch path"): one report per
//! observation, and a heartbeat that names every grant the node runs.
//!
//! Invariant: both go through the authority's fence and presence (ADR
//! 0028), the one place that judges a grant. `Report::check` refuses a
//! report whose observation names another attempt or generation than its
//! token before the journal is read; then `Authority::report` runs the
//! fence in the write's own transaction: a report from another node than
//! the grant's is `WrongNode`, a report at an older generation is
//! recorded as `StaleObservationRefused` and answered `StaleGeneration`,
//! a superseded or ended grant is refused and recorded, and nothing of
//! it is applied. A report or a heartbeat from the node an `unknown`
//! attempt was granted to is presence: the attempt is observed reachable
//! before the fence runs. A heartbeat is answered with the fence's
//! verdict per grant and writes nothing else.

use fabric_core::id::NodeId;
use fabric_core::managed::DomainError;
use fabric_core::managed::launch::{
    Heartbeat, HeartbeatAnswer, MAX_HEARTBEAT_ATTEMPTS, Report, ReportAnswer, ReportRefusal,
};
use fabric_journal::CommitError;

use crate::authority::{Authority, AuthorityError};
use crate::managed::refusal_code;
use crate::node::Node;

fn refused(reason: ReportRefusal) -> ReportAnswer {
    ReportAnswer::Refused { reason }
}

/// Accepts `report` from `from` (`None`: this node) into the authority's
/// journal, behind the fence.
pub(crate) async fn accept(
    authority: &Authority,
    from: Option<NodeId>,
    report: Report,
) -> ReportAnswer {
    if let Err(reason) = report.check() {
        return refused(reason);
    }
    match authority
        .report(report.token, from, report.observation)
        .await
    {
        Ok(Ok(transition)) => ReportAnswer::Recorded {
            revision: transition.revision,
        },
        Ok(Err(verdict)) => refused(ReportRefusal::fenced(&verdict).unwrap_or(
            ReportRefusal::Domain {
                reason: "fence_answered_live_without_a_write".to_owned(),
            },
        )),
        Err(AuthorityError::Commit(CommitError::Domain(DomainError::UnknownJob { .. }))) => {
            refused(ReportRefusal::UnknownJob)
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

/// The fence's verdict on each token of `beat` from `from` (`None`: this
/// node, `own`), in its order, at most `MAX_HEARTBEAT_ATTEMPTS`, through
/// `Authority::heartbeat`: presence first, then the fence. An authority
/// that cannot read its journal (closed, refused or unreadable) answers
/// no verdict, which says nothing of any grant: a journal that failed to
/// open is not a cancel.
pub(crate) async fn heartbeat(
    authority: &Authority,
    own: NodeId,
    from: Option<NodeId>,
    beat: &Heartbeat,
) -> HeartbeatAnswer {
    let sender = from.unwrap_or(own);
    let beat = Heartbeat {
        attempts: beat
            .attempts
            .iter()
            .take(MAX_HEARTBEAT_ATTEMPTS)
            .copied()
            .collect(),
    };
    match authority.heartbeat(sender, beat).await {
        Ok(answer) => answer,
        Err(e) => {
            tracing::warn!(error = %e, "a heartbeat was not judged: the journal cannot be read");
            HeartbeatAnswer {
                verdicts: Vec::new(),
            }
        }
    }
}

impl Node {
    /// `accept` for a report from `from` (`None` is this node).
    pub(crate) async fn accept_report(&self, from: Option<NodeId>, report: Report) -> ReportAnswer {
        accept(&self.authority, from, report).await
    }

    /// `heartbeat` for a heartbeat from `from` (`None` is this node).
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
    use fabric_core::id::JobId;
    use fabric_core::managed::launch::GrantToken;
    use fabric_core::managed::{
        AttemptStatus, FenceVerdict, Generation, ManagedEvent, ManagedState, Observation,
        OperationStatus,
    };

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
                accept(&world.authority, from, started.clone()).await,
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
            accept(&world.authority, Some(REMOTE), started).await,
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

    /// A heartbeat is answered per token with the fence's verdict: the
    /// live grant on its node is `Live`, a grant the job moved past is
    /// `Stale`, a grant seen from another node is `WrongNode`, and a job
    /// this authority does not hold is `UnknownJob`.
    #[tokio::test]
    async fn a_heartbeat_names_each_grant_by_the_fence() {
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
            vec![
                (first, FenceVerdict::Live),
                (first, FenceVerdict::UnknownJob)
            ]
        );
        let elsewhere = heartbeat(&world.authority, OWN, Some(OTHER), &beat).await;
        assert_eq!(
            elsewhere.verdicts[0],
            (
                first,
                FenceVerdict::WrongNode {
                    granted: REMOTE,
                    from: OTHER
                }
            )
        );
        world.cut(job, first, REMOTE).await;
        let second = world.continued_to(job, OTHER).await;
        let current = world.state(job).await.attempt(second).unwrap().generation;
        let answer = heartbeat(&world.authority, OWN, Some(REMOTE), &beat).await;
        assert_eq!(answer.verdicts[0], (first, FenceVerdict::Stale { current }));
        let revision = world.state(job).await.job.revision;
        let _ = heartbeat(&world.authority, OWN, Some(OTHER), &beat).await;
        assert_eq!(
            world.state(job).await.job.revision,
            revision,
            "a heartbeat wrote"
        );
    }

    /// An authority whose journal it cannot open answers a heartbeat with
    /// no verdict for the tokens it cannot read, never `UnknownJob`: a
    /// node obeys `UnknownJob` by stopping the attempt, and a journal that
    /// failed to open is not a cancel.
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
