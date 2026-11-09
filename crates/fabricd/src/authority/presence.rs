//! The last heartbeat per live attempt on a remote node, in memory (ADR
//! 0028). A heartbeat is never a journal write; only the transitions it
//! causes (`AttemptUnavailable`, `AttemptReachable`) are history.
//!
//! Invariant: an attempt is tracked from its grant to a remote node, or
//! from the authority's start for a grant it finds live, until it ends or
//! loses its grant. Silence is measured on the authority's monotonic
//! clock (`clock::mono_ms`), never on the wall clock, so a clock step or
//! a host sleep names nothing overdue; the wall clock is carried only for
//! the `last_heartbeat_ms` a record holds. `overdue` names a tracked
//! attempt once per silence: after `mark_unavailable` it is named again
//! only after `returned`. `unavailable` names an attempt only once it has
//! been unavailable for `UNAVAILABLE_AFTER_MS`, the dwell the recovery
//! policy waits for. An attempt on the authority's own node is never
//! tracked: it never heartbeats and replay rule 4 covers it.

use std::collections::HashMap;

use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::{
    AttemptId, Generation, GrantToken, RecoveryBlock, UNAVAILABLE_AFTER_MS,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tracked {
    job: JobId,
    generation: Generation,
    node: NodeId,
    /// When tracking began, on the monotonic clock: the grant, or the
    /// authority's start.
    since: u64,
    /// The last heartbeat or covered write: `(monotonic, wall)`.
    last_seen: Option<(u64, u64)>,
    /// When `ResourceUnavailable` was observed for the current silence, on
    /// the monotonic clock; `None` while the attempt is reachable.
    unavailable_since: Option<u64>,
    /// The last `RecoveryBlocked` reason recorded for this silence, so the
    /// same reason is not recorded every tick.
    blocked: Option<RecoveryBlock>,
}

impl Tracked {
    fn heard(&self) -> u64 {
        self.last_seen
            .map_or(self.since, |(mono, _)| mono.max(self.since))
    }
}

/// One attempt the authority has made `unknown` and may restart (ADR 0028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Overdue {
    /// The job.
    pub job: JobId,
    /// The attempt.
    pub attempt: AttemptId,
    /// Its generation.
    pub generation: Generation,
    /// The last heartbeat the authority holds, on the wall clock; `None`
    /// for one never heard from.
    pub last_heartbeat_ms: Option<u64>,
}

/// What the authority knows about the attempts it granted to other nodes.
#[derive(Debug, Default)]
pub(crate) struct Presence {
    tracked: HashMap<AttemptId, Tracked>,
}

impl Presence {
    /// Starts tracking `attempt`, granted to `node`, reachable, from `now`.
    /// An attempt already tracked keeps what it has.
    pub(crate) fn track(
        &mut self,
        job: JobId,
        attempt: AttemptId,
        generation: Generation,
        node: NodeId,
        now: u64,
    ) {
        self.insert(job, attempt, generation, node, now, None);
    }

    /// Starts tracking `attempt`, granted to `node`, as unavailable since
    /// `now`: the journal says it is `unknown` for want of heartbeats, as
    /// the authority finds at a restart. Its dwell counts from `now`.
    pub(crate) fn track_unavailable(
        &mut self,
        job: JobId,
        attempt: AttemptId,
        generation: Generation,
        node: NodeId,
        now: u64,
    ) {
        self.insert(job, attempt, generation, node, now, Some(now));
    }

    fn insert(
        &mut self,
        job: JobId,
        attempt: AttemptId,
        generation: Generation,
        node: NodeId,
        now: u64,
        unavailable_since: Option<u64>,
    ) {
        self.tracked.entry(attempt).or_insert(Tracked {
            job,
            generation,
            node,
            since: now,
            last_seen: None,
            unavailable_since,
            blocked: None,
        });
    }

    /// Stops tracking `attempt`: it ended or lost its grant.
    pub(crate) fn forget(&mut self, attempt: AttemptId) {
        self.tracked.remove(&attempt);
    }

    /// A heartbeat or a covered write from `from` for `token`: refreshes
    /// the presence of a tracked, reachable attempt and returns `true`.
    /// `false` for an attempt not tracked for `from` at this generation,
    /// and for one marked unavailable, which the journal brings back
    /// (`returned`) so the two never disagree. Writes nothing.
    pub(crate) fn seen(
        &mut self,
        from: NodeId,
        token: &GrantToken,
        now: u64,
        wall_ms: u64,
    ) -> bool {
        let Some(tracked) = self.tracked.get_mut(&token.attempt) else {
            return false;
        };
        if tracked.node != from
            || tracked.generation != token.generation
            || tracked.unavailable_since.is_some()
        {
            return false;
        }
        tracked.last_seen = Some((now, wall_ms));
        true
    }

    /// The journal observed `ResourceReachable` for `attempt`: its silence
    /// ends, heard at `now`.
    pub(crate) fn returned(&mut self, attempt: AttemptId, now: u64, wall_ms: u64) {
        if let Some(t) = self.tracked.get_mut(&attempt) {
            t.unavailable_since = None;
            t.blocked = None;
            t.last_seen = Some((now, wall_ms));
        }
    }

    /// The tracked attempts not heard from for `UNAVAILABLE_AFTER_MS`,
    /// counted from the last heartbeat or from when tracking began, and
    /// not yet marked unavailable. Run every `HEARTBEAT_INTERVAL_MS` on the
    /// authority's monotonic clock; the caller observes each
    /// `ResourceUnavailable` and then calls `mark_unavailable`.
    pub(crate) fn overdue(&self, now: u64) -> Vec<Overdue> {
        let mut out: Vec<Overdue> = self
            .tracked
            .iter()
            .filter(|(_, t)| t.unavailable_since.is_none())
            .filter(|(_, t)| now.saturating_sub(t.heard()) >= UNAVAILABLE_AFTER_MS)
            .map(|(attempt, t)| Overdue {
                job: t.job,
                attempt: *attempt,
                generation: t.generation,
                last_heartbeat_ms: t.last_seen.map(|(_, wall)| wall),
            })
            .collect();
        out.sort_by_key(|o| (o.job, o.attempt));
        out
    }

    /// Records that `ResourceUnavailable` was observed for `attempt` at `now`.
    pub(crate) fn mark_unavailable(&mut self, attempt: AttemptId, now: u64) {
        if let Some(t) = self.tracked.get_mut(&attempt) {
            t.unavailable_since = Some(now);
        }
    }

    /// The attempts that have been unavailable for `UNAVAILABLE_AFTER_MS`
    /// and are not back, for the recovery decision to run: a silence of
    /// `UNAVAILABLE_AFTER_MS` made each `unknown`, and as long again has
    /// passed without a heartbeat.
    pub(crate) fn unavailable(&self, now: u64) -> Vec<Overdue> {
        let mut out: Vec<Overdue> = self
            .tracked
            .iter()
            .filter(|(_, t)| {
                t.unavailable_since
                    .is_some_and(|since| now.saturating_sub(since) >= UNAVAILABLE_AFTER_MS)
            })
            .map(|(attempt, t)| Overdue {
                job: t.job,
                attempt: *attempt,
                generation: t.generation,
                last_heartbeat_ms: t.last_seen.map(|(_, wall)| wall),
            })
            .collect();
        out.sort_by_key(|o| (o.job, o.attempt));
        out
    }

    /// Whether `reason` is new for this silence of `attempt`; records it.
    /// The same reason twice in a row is not recorded twice.
    pub(crate) fn block_changed(&mut self, attempt: AttemptId, reason: &RecoveryBlock) -> bool {
        let Some(t) = self.tracked.get_mut(&attempt) else {
            return true;
        };
        if t.blocked.as_ref() == Some(reason) {
            return false;
        }
        t.blocked = Some(reason.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: JobId = JobId::from_bytes([1; 16]);
    const A: AttemptId = AttemptId::from_bytes([2; 16]);
    const RUNNER: NodeId = NodeId::from_bytes([3; 32]);
    const OTHER: NodeId = NodeId::from_bytes([4; 32]);
    /// A wall-clock reading far from the monotonic one, so a mix-up shows.
    const WALL: u64 = 1_700_000_000_000;

    fn token(attempt: AttemptId) -> GrantToken {
        GrantToken {
            job: JOB,
            attempt,
            generation: Generation::FIRST,
        }
    }

    #[test]
    fn silence_is_counted_on_the_monotonic_clock_from_the_grant_or_the_last_heartbeat() {
        let mut p = Presence::default();
        p.track(JOB, A, Generation::FIRST, RUNNER, 1_000);
        assert!(p.overdue(1_000 + UNAVAILABLE_AFTER_MS - 1).is_empty());
        let overdue = p.overdue(1_000 + UNAVAILABLE_AFTER_MS);
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].last_heartbeat_ms, None);
        // A heartbeat resets the clock; one from another node does not.
        assert!(!p.seen(OTHER, &token(A), 5_000, WALL));
        assert_eq!(p.overdue(11_000).len(), 1);
        assert!(p.seen(RUNNER, &token(A), 5_000, WALL + 5_000));
        assert!(p.overdue(14_999).is_empty());
        // The record carries the wall clock; the decision used the other.
        let overdue = p.overdue(15_000);
        assert_eq!(overdue[0].last_heartbeat_ms, Some(WALL + 5_000));
    }

    #[test]
    fn an_unavailable_attempt_is_named_once_and_dwells_before_recovery() {
        let mut p = Presence::default();
        p.track(JOB, A, Generation::FIRST, RUNNER, 0);
        assert_eq!(p.overdue(UNAVAILABLE_AFTER_MS).len(), 1);
        p.mark_unavailable(A, UNAVAILABLE_AFTER_MS);
        assert!(p.overdue(UNAVAILABLE_AFTER_MS * 3).is_empty());
        // Unknown, but not yet for the threshold: the policy waits.
        assert!(p.unavailable(UNAVAILABLE_AFTER_MS).is_empty());
        assert!(p.unavailable(2 * UNAVAILABLE_AFTER_MS - 1).is_empty());
        assert_eq!(p.unavailable(2 * UNAVAILABLE_AFTER_MS).len(), 1);
        // The reason of a blocked restart is recorded once per silence.
        let reason = RecoveryBlock::NoAcknowledgedCheckpoint;
        assert!(p.block_changed(A, &reason));
        assert!(!p.block_changed(A, &reason));
        assert!(p.block_changed(
            A,
            &RecoveryBlock::OutcomeBlocks {
                kind: "block".into()
            }
        ));
        // A heartbeat while unavailable refreshes nothing by itself: the
        // journal brings the attempt back, then silence counts again.
        assert!(!p.seen(RUNNER, &token(A), 50_000, WALL));
        assert_eq!(p.unavailable(50_000).len(), 1);
        p.returned(A, 50_000, WALL + 50_000);
        assert!(p.unavailable(u64::MAX).is_empty());
        assert!(p.seen(RUNNER, &token(A), 50_001, WALL + 50_001));
        assert!(p.overdue(50_001 + UNAVAILABLE_AFTER_MS - 1).is_empty());
        assert_eq!(p.overdue(50_001 + UNAVAILABLE_AFTER_MS).len(), 1);
        assert!(p.block_changed(A, &reason), "a new silence records again");
        p.forget(A);
        assert!(p.overdue(u64::MAX).is_empty());
        assert!(p.unavailable(u64::MAX).is_empty());
    }

    #[test]
    fn an_attempt_found_unknown_at_a_restart_dwells_from_the_restart() {
        let mut p = Presence::default();
        p.track_unavailable(JOB, A, Generation::FIRST, RUNNER, 7_000);
        assert!(p.overdue(u64::MAX).is_empty(), "named once per silence");
        assert!(p.unavailable(7_000 + UNAVAILABLE_AFTER_MS - 1).is_empty());
        assert_eq!(p.unavailable(7_000 + UNAVAILABLE_AFTER_MS).len(), 1);
        p.returned(A, 8_000, WALL);
        assert!(p.unavailable(u64::MAX).is_empty());
    }
}
