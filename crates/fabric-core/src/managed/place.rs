//! The placement function (ADR 0026): hard filters, then a deterministic
//! rank of what is left.
//!
//! Invariant: `place` is a pure function of its input. The same probes in
//! any order give the same decision: every node is judged on its own
//! facts alone, the eligible set is ordered by a total key whose last part
//! is the node's identity, and the excluded set keeps the order it was
//! probed in. No warmth, load or pin ever admits a node a filter excluded;
//! a rank reason is only ever given to a node that passed every filter.

use super::placement::{
    Candidate, CurrentPlacement, Exclusion, MAX_CANDIDATES, NodeAdvert, PROBE_BOUND_MS,
    PlacementDecision, PlacementInput, Probe, RankReason, Verdict,
};
use crate::harness::HarnessKind;

/// Decides a placement from fresh probes: every exclusion of every node is
/// listed; the eligible nodes are ranked by the current healthy placement,
/// then the bundle chain, then the workspace, then the lowest 1-minute
/// load, then the node identity; the first wins. With no eligible node
/// `chosen` is `None` and the job is parked.
#[must_use]
pub fn place(input: &PlacementInput<'_>) -> PlacementDecision {
    let mut eligible: Vec<(Key, Candidate)> = Vec::new();
    let mut excluded: Vec<Candidate> = Vec::new();
    let mut seen: Vec<crate::id::NodeId> = Vec::new();
    for probe in input.candidates.iter().take(MAX_CANDIDATES) {
        if seen.contains(&probe.node) {
            continue;
        }
        seen.push(probe.node);
        let reasons = exclusions(input, probe);
        if reasons.is_empty() {
            // `exclusions` is empty only when the advert answered.
            if let Some(advert) = &probe.advert {
                let key = Key::of(input.current, probe.node, advert);
                eligible.push((
                    key,
                    Candidate {
                        node: probe.node,
                        verdict: Verdict::Eligible {
                            rank: key.reasons(),
                        },
                    },
                ));
            }
        } else {
            excluded.push(Candidate {
                node: probe.node,
                verdict: Verdict::Excluded { reasons },
            });
        }
    }
    eligible.sort_by_key(|(key, _)| *key);
    let chosen = eligible.first().map(|(_, c)| c.node);
    let mut candidates: Vec<Candidate> = eligible.into_iter().map(|(_, c)| c).collect();
    candidates.extend(excluded);
    PlacementDecision {
        kind: input.kind,
        decided_at_ms: input.now_ms,
        candidates,
        chosen,
    }
}

/// Every reason `probe`'s node is not eligible; empty when it is.
fn exclusions(input: &PlacementInput<'_>, probe: &Probe) -> Vec<Exclusion> {
    let mut out = Vec::new();
    let Some(advert) = &probe.advert else {
        out.push(Exclusion::NoAnswer {
            within_ms: PROBE_BOUND_MS,
        });
        return out;
    };
    let age_ms = input.now_ms.saturating_sub(advert.measured_at_ms);
    if age_ms > u64::from(PROBE_BOUND_MS) {
        out.push(Exclusion::Stale { age_ms });
    }
    let policy = &input.job.policy.placement;
    if let Some(need) = &policy.platform
        && *need != advert.platform
    {
        out.push(Exclusion::Platform {
            need: need.clone(),
            have: advert.platform.clone(),
        });
    }
    let harness = &input.job.intent.harness;
    if !advert
        .harnesses
        .iter()
        .any(|have| same_harness(&have.kind, harness))
    {
        out.push(Exclusion::HarnessMissing {
            kind: harness.clone(),
        });
    }
    if let Some(need) = &input.required_layout
        && *need != advert.layout
    {
        out.push(Exclusion::Layout {
            need: need.clone(),
            have: advert.layout.clone(),
        });
    }
    for name in &probe.secrets_unresolved {
        out.push(Exclusion::Secret { name: name.clone() });
    }
    if advert.capacity.free_disk_bytes < policy.disk_bytes {
        out.push(Exclusion::Disk {
            need: policy.disk_bytes,
            free: advert.capacity.free_disk_bytes,
        });
    }
    if let Some(pinned) = policy.pin
        && pinned != probe.node
    {
        out.push(Exclusion::Pin { pinned });
    }
    out
}

/// Whether an installed harness is the one a job runs under: the same
/// adapter, and for an ACP agent the same program; the arguments are the
/// job's, not the installation's.
fn same_harness(have: &HarnessKind, need: &HarnessKind) -> bool {
    match (have, need) {
        (HarnessKind::ClaudeCode, HarnessKind::ClaudeCode)
        | (HarnessKind::Codex, HarnessKind::Codex) => true,
        (HarnessKind::Acp { program: a, .. }, HarnessKind::Acp { program: b, .. }) => a == b,
        _ => false,
    }
}

/// The total order of the eligible set, lowest first. Booleans are stored
/// inverted so that `true` sorts first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    not_current_healthy: bool,
    no_bundle_chain: bool,
    no_workspace: bool,
    load_1m_milli: u32,
    node: crate::id::NodeId,
}

impl Key {
    /// The key of `node`'s advert. `node` is the node the probe reached,
    /// the authenticated identity; the id the advert claims is never used,
    /// so a peer that names another node can take neither its current
    /// rank nor its place in the identity order.
    fn of(current: Option<CurrentPlacement>, node: crate::id::NodeId, advert: &NodeAdvert) -> Key {
        let current_healthy = current.is_some_and(|c| c.healthy && c.node == node);
        Key {
            not_current_healthy: !current_healthy,
            no_bundle_chain: !advert.warmth.bundle_chain,
            no_workspace: !advert.warmth.workspace,
            load_1m_milli: advert.capacity.load_1m_milli,
            node,
        }
    }

    /// The rank reasons, in the order of weight.
    fn reasons(self) -> Vec<RankReason> {
        let mut out = Vec::with_capacity(5);
        if !self.not_current_healthy {
            out.push(RankReason::CurrentHealthy);
        }
        if !self.no_bundle_chain {
            out.push(RankReason::BundleChain);
        }
        if !self.no_workspace {
            out.push(RankReason::Workspace);
        }
        out.push(RankReason::Load {
            milli: self.load_1m_milli,
        });
        out.push(RankReason::Identity);
        out
    }
}
