//! The authority's dispatch of a submit's initial placement (ADR 0026):
//! every accepted `CreateJob` whose source is not a repository on this
//! node is placed among the registered nodes, and the decision is
//! recorded with the grant it makes.
//!
//! Invariant: the list of what to place is read anew from the journal
//! after every commit (`Authority::pending`), so a create accepted before
//! a crash is placed after the restart, and a create whose `Placed` was
//! committed is never placed again: the journal marks it in the same
//! batch as the decision. One loop places one create at a time, so two
//! commits close together never place the same create twice. A create
//! with a repository source is the M2 form and is settled by its explicit
//! `StartAttempt`, as before M5. The grant names the chosen node; this
//! module launches nothing.

use std::collections::HashSet;
use std::sync::Arc;

use fabric_core::id::JobId;
use fabric_core::managed::placement::PlacementKind;
use fabric_core::managed::{CommandKind, Observation, OperationId, SourceBinding};

use crate::authority::{AuthorityError, PendingEffect};
use crate::node::Node;
use crate::placement::PlacementAsk;

/// Places every accepted create, now and after every later commit, until
/// the authority's thread ends. It waits without a thread of its own
/// while the authority has none, so a node with no managed job starts no
/// authority thread for it.
pub(crate) async fn run(node: Arc<Node>) {
    let Some(mut commits) = node.authority.started().await else {
        return;
    };
    // A create whose placement failed is not tried again in this process:
    // the failure is logged once, and a restart tries it again.
    let mut failed: HashSet<(JobId, OperationId)> = HashSet::new();
    loop {
        commits.borrow_and_update();
        match node.authority.pending().await {
            Ok(effects) => {
                for effect in effects {
                    if effect.kind != CommandKind::CreateJob
                        || failed.contains(&(effect.job, effect.operation))
                    {
                        continue;
                    }
                    if let Err(e) = place_create(&node, &effect).await {
                        tracing::warn!(job = %effect.job, operation = %effect.operation, error = %e, "the initial placement was not recorded");
                        failed.insert((effect.job, effect.operation));
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "the pending effects could not be read"),
        }
        if commits.changed().await.is_err() {
            return;
        }
    }
}

/// Places `effect`'s create once and records the decision.
async fn place_create(node: &Arc<Node>, effect: &PendingEffect) -> Result<(), AuthorityError> {
    let Some(state) = node.authority.job(effect.job).await? else {
        return Ok(());
    };
    let snapshot = match &state.job.source {
        SourceBinding::Repository { .. } => return Ok(()),
        SourceBinding::Snapshot { snapshot } => Some(*snapshot),
        SourceBinding::Git { .. } | SourceBinding::Empty => None,
    };
    let task = state.job.primary;
    let ask = PlacementAsk {
        task,
        kind: PlacementKind::Initial,
        required_layout: None,
        current: None,
        snapshot,
    };
    crate::faults::hit(crate::faults::Point::BeforePlacement).await;
    let decision = node.place_job(&state.job, &ask).await;
    tracing::info!(job = %effect.job, chosen = ?decision.chosen.map(|n| n.to_string()), "placed");
    node.authority
        .observe(
            effect.job,
            Observation::Placed {
                operation: effect.operation,
                task,
                decision,
            },
        )
        .await?;
    Ok(())
}
