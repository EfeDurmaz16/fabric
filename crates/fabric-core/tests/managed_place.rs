//! `place` (ADR 0026) is a pure function of its input: the same probes in
//! any order give the same decision, every exclusion is listed, no warmth
//! or load admits an excluded node, and the rank is total.

#![allow(
    clippy::unwrap_used,
    reason = "the helpers here are test code; a failed unwrap is the test failing"
)]

use fabric_core::harness::HarnessKind;
use fabric_core::id::{JobId, NodeId};
use fabric_core::managed::placement::{
    Capacity, CurrentPlacement, Exclusion, HarnessInventory, PathLayout, PlacementInput,
    PlacementKind, Probe, RankReason, Verdict, Warmth,
};
use fabric_core::managed::{
    Generation, Intent, JobPolicy, JobState, ManagedJob, NodeAdvert, PlacementPolicy,
    RestartPolicy, SecretName, SourceBinding, TaskId, WorkRevision, place,
};
use fabric_core::path::AbsPath;
use fabric_core::snapshot::Platform;
use fabric_core::version::HarnessVersion;
use proptest::prelude::*;

const NOW: u64 = 1_762_128_000_000;

fn node(n: u8) -> NodeId {
    NodeId::from_bytes([n; 32])
}

fn macos() -> Platform {
    Platform {
        os: "macos".into(),
        arch: "arm64".into(),
    }
}

fn layout() -> PathLayout {
    PathLayout {
        home: AbsPath::parse("/Users/me").unwrap(),
        root: AbsPath::parse("/Users/me/fabric/j").unwrap(),
    }
}

fn job(placement: PlacementPolicy) -> ManagedJob {
    ManagedJob {
        id: JobId::from_bytes([1; 16]),
        revision: WorkRevision::FIRST,
        generation: Generation::FIRST,
        intent: Intent {
            goal: "fix the test".into(),
            harness: HarnessKind::Codex,
            acceptance: Vec::new(),
        },
        source: SourceBinding::Empty,
        policy: JobPolicy {
            placement,
            ..JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
        },
        state: JobState::Active,
        primary: TaskId::from_bytes([2; 16]),
    }
}

fn advert(n: u8, load: u32, free: u64, warmth: Warmth) -> NodeAdvert {
    NodeAdvert {
        node: node(n),
        platform: macos(),
        harnesses: vec![HarnessInventory {
            kind: HarnessKind::Codex,
            version: HarnessVersion::parse("0.50.0").unwrap(),
        }],
        layout: layout(),
        capacity: Capacity {
            load_1m_milli: load,
            free_disk_bytes: free,
        },
        warmth,
        measured_at_ms: NOW,
    }
}

fn probe(advert: NodeAdvert) -> Probe {
    Probe {
        node: advert.node,
        advert: Some(advert),
        secrets_unresolved: Vec::new(),
        probe_ms: 10,
    }
}

const COLD: Warmth = Warmth {
    bundle_chain: false,
    workspace: false,
};
const WARM: Warmth = Warmth {
    bundle_chain: true,
    workspace: true,
};

fn input(
    job: &ManagedJob,
    current: Option<CurrentPlacement>,
    candidates: Vec<Probe>,
) -> PlacementInput<'_> {
    PlacementInput {
        job,
        task: job.primary,
        kind: PlacementKind::Initial,
        required_layout: Some(layout()),
        current,
        candidates,
        now_ms: NOW,
    }
}

fn reasons(decision: &fabric_core::managed::PlacementDecision, n: u8) -> Vec<Exclusion> {
    match &decision
        .candidates
        .iter()
        .find(|c| c.node == node(n))
        .unwrap()
        .verdict
    {
        Verdict::Excluded { reasons } => reasons.clone(),
        Verdict::Eligible { .. } => Vec::new(),
    }
}

#[test]
fn two_eligible_nodes_rank_by_warmth_then_load_then_identity() {
    let job = job(PlacementPolicy::DEFAULT);
    let a = probe(advert(5, 900, 1 << 40, COLD));
    let b = probe(advert(3, 100, 1 << 40, COLD));
    let c = probe(advert(9, 2_000, 1 << 40, WARM));
    let decision = place(&input(&job, None, vec![a.clone(), b.clone(), c.clone()]));
    assert_eq!(decision.chosen, Some(node(9)), "warmth beats load");
    let order: Vec<NodeId> = decision.candidates.iter().map(|c| c.node).collect();
    assert_eq!(order, vec![node(9), node(3), node(5)]);
    assert_eq!(
        decision.candidates[0].verdict,
        Verdict::Eligible {
            rank: vec![
                RankReason::BundleChain,
                RankReason::Workspace,
                RankReason::Load { milli: 2_000 },
                RankReason::Identity,
            ]
        }
    );
    // Equal nodes order by identity.
    let d = probe(advert(7, 100, 1 << 40, COLD));
    let decision = place(&input(&job, None, vec![d, b]));
    assert_eq!(decision.chosen, Some(node(3)));
}

#[test]
fn a_healthy_current_placement_is_never_moved_for_a_warmer_node() {
    let job = job(PlacementPolicy::DEFAULT);
    let current = probe(advert(4, 5_000, 1 << 40, COLD));
    let warmer = probe(advert(2, 10, 1 << 40, WARM));
    let healthy = Some(CurrentPlacement {
        node: node(4),
        healthy: true,
    });
    let decision = place(&input(&job, healthy, vec![warmer.clone(), current.clone()]));
    assert_eq!(decision.chosen, Some(node(4)));
    assert_eq!(
        decision.candidates[0].verdict,
        Verdict::Eligible {
            rank: vec![
                RankReason::CurrentHealthy,
                RankReason::Load { milli: 5_000 },
                RankReason::Identity,
            ]
        }
    );
    let unhealthy = Some(CurrentPlacement {
        node: node(4),
        healthy: false,
    });
    let decision = place(&input(&job, unhealthy, vec![warmer, current]));
    assert_eq!(
        decision.chosen,
        Some(node(2)),
        "an unhealthy current wins nothing"
    );
}

#[test]
fn no_eligible_node_parks_the_submit_with_every_exclusion() {
    let job = job(PlacementPolicy {
        platform: Some(macos()),
        disk_bytes: 4 << 30,
        pin: None,
    });
    let mut linux = advert(1, 100, 1 << 40, COLD);
    linux.platform = Platform {
        os: "linux".into(),
        arch: "x86_64".into(),
    };
    linux.harnesses.clear();
    linux.layout.home = AbsPath::parse("/home/me").unwrap();
    let mut poor = probe(advert(2, 100, 1 << 30, COLD));
    poor.secrets_unresolved = vec![SecretName::parse("API_TOKEN").unwrap()];
    let silent = Probe {
        node: node(3),
        advert: None,
        secrets_unresolved: Vec::new(),
        probe_ms: 5_000,
    };
    let decision = place(&input(&job, None, vec![probe(linux), poor, silent]));
    assert_eq!(decision.chosen, None);
    assert_eq!(
        decision.candidates.len(),
        3,
        "every probe is in the decision"
    );
    let linux = reasons(&decision, 1);
    assert!(matches!(linux[0], Exclusion::Platform { .. }), "{linux:?}");
    assert!(
        matches!(linux[1], Exclusion::HarnessMissing { .. }),
        "{linux:?}"
    );
    assert!(matches!(linux[2], Exclusion::Layout { .. }), "{linux:?}");
    assert_eq!(
        reasons(&decision, 2),
        vec![
            Exclusion::Secret {
                name: SecretName::parse("API_TOKEN").unwrap()
            },
            Exclusion::Disk {
                need: 4 << 30,
                free: 1 << 30
            },
        ]
    );
    assert_eq!(
        reasons(&decision, 3),
        vec![Exclusion::NoAnswer { within_ms: 5_000 }]
    );
}

#[test]
fn a_pin_to_an_ineligible_node_parks_naming_the_pin() {
    let job = job(PlacementPolicy {
        pin: Some(node(8)),
        ..PlacementPolicy::DEFAULT
    });
    let fine = probe(advert(1, 10, 1 << 40, WARM));
    let pinned = probe(advert(8, 10, 1 << 20, COLD));
    let decision = place(&input(&job, None, vec![fine, pinned]));
    assert_eq!(decision.chosen, None);
    assert_eq!(
        reasons(&decision, 1),
        vec![Exclusion::Pin { pinned: node(8) }]
    );
    assert!(matches!(reasons(&decision, 8)[0], Exclusion::Disk { .. }));
    // The pinned node, once eligible, is the one chosen whatever the rest offers.
    let decision = place(&input(
        &job,
        None,
        vec![
            probe(advert(1, 10, 1 << 40, WARM)),
            probe(advert(8, 9_000, 1 << 40, COLD)),
        ],
    ));
    assert_eq!(decision.chosen, Some(node(8)));
}

#[test]
fn a_silent_or_stale_node_is_excluded_for_this_placement_only() {
    let job = job(PlacementPolicy::DEFAULT);
    let mut stale = advert(1, 10, 1 << 40, WARM);
    stale.measured_at_ms = NOW - 6_000;
    let fresh = probe(advert(2, 500, 1 << 40, COLD));
    let decision = place(&input(
        &job,
        None,
        vec![probe(stale.clone()), fresh.clone()],
    ));
    assert_eq!(decision.chosen, Some(node(2)));
    assert_eq!(
        reasons(&decision, 1),
        vec![Exclusion::Stale { age_ms: 6_000 }]
    );
    // The next placement reads fresh facts and the node is back.
    stale.measured_at_ms = NOW;
    let decision = place(&input(&job, None, vec![probe(stale), fresh]));
    assert_eq!(decision.chosen, Some(node(1)));
}

#[test]
fn warmth_never_admits_an_incompatible_node() {
    let job = job(PlacementPolicy::DEFAULT);
    let mut warm = advert(1, 0, 1 << 40, WARM);
    warm.layout.root = AbsPath::parse("/elsewhere").unwrap();
    let cold = probe(advert(2, 9_999, 1 << 40, COLD));
    let decision = place(&input(&job, None, vec![probe(warm), cold]));
    assert_eq!(decision.chosen, Some(node(2)));
    assert!(matches!(reasons(&decision, 1)[0], Exclusion::Layout { .. }));
    // Without a required layout (no native session allowed) the root does not matter.
    let mut no_layout = input(&job, None, vec![probe(advert(1, 0, 1 << 40, WARM))]);
    no_layout.required_layout = None;
    let mut other = no_layout.candidates[0].advert.clone().unwrap();
    other.layout.root = AbsPath::parse("/elsewhere").unwrap();
    no_layout.candidates[0].advert = Some(other);
    assert_eq!(place(&no_layout).chosen, Some(node(1)));
}

fn arbitrary_probe() -> impl Strategy<Value = Probe> {
    (
        1..=16_u8,
        any::<bool>(),
        any::<u32>(),
        any::<u64>(),
        any::<bool>(),
        any::<bool>(),
        0..=10_000_u64,
        any::<bool>(),
    )
        .prop_map(
            |(n, answers, load, free, chain, workspace, age, secret)| Probe {
                node: node(n),
                advert: answers.then(|| {
                    let mut a = advert(
                        n,
                        load,
                        free,
                        Warmth {
                            bundle_chain: chain,
                            workspace,
                        },
                    );
                    a.measured_at_ms = NOW - age;
                    a
                }),
                secrets_unresolved: if secret {
                    vec![SecretName::parse("S").unwrap()]
                } else {
                    Vec::new()
                },
                probe_ms: 1,
            },
        )
}

proptest! {
    /// The decision is a function of the set of probes, not their order:
    /// the chosen node, the eligible ranking and every verdict are the
    /// same under any permutation, and a chosen node is always the first
    /// eligible one.
    #[test]
    fn place_is_a_pure_function_of_its_input(
        drawn in proptest::collection::vec(arbitrary_probe(), 0..12),
        shuffle in any::<prop::sample::Index>(),
    ) {
        // One probe per node, as a placement round makes them.
        let mut probes: Vec<Probe> = Vec::new();
        for probe in drawn {
            if !probes.iter().any(|p| p.node == probe.node) {
                probes.push(probe);
            }
        }
        let job = job(PlacementPolicy::DEFAULT);
        let first = place(&input(&job, None, probes.clone()));
        prop_assert_eq!(&first, &place(&input(&job, None, probes.clone())));
        let mut rotated = probes.clone();
        if !rotated.is_empty() {
            let by = shuffle.index(rotated.len());
            rotated.rotate_left(by);
        }
        let second = place(&input(&job, None, rotated));
        prop_assert_eq!(first.chosen, second.chosen);
        let eligible = |d: &fabric_core::managed::PlacementDecision| -> Vec<NodeId> {
            d.candidates
                .iter()
                .filter(|c| matches!(c.verdict, Verdict::Eligible { .. }))
                .map(|c| c.node)
                .collect()
        };
        prop_assert_eq!(eligible(&first), eligible(&second));
        prop_assert_eq!(first.chosen, eligible(&first).first().copied());
        for candidate in &first.candidates {
            let same = second.candidates.iter().find(|c| c.node == candidate.node).unwrap();
            prop_assert_eq!(&candidate.verdict, &same.verdict);
            if let Verdict::Excluded { reasons } = &candidate.verdict {
                prop_assert!(!reasons.is_empty());
            }
        }
    }
}

/// The rank keys on the node the probe authenticated, never on the id the
/// advert claims: a peer that advertises another node's id neither takes
/// that node's current healthy rank nor its place in the identity order.
#[test]
fn the_rank_keys_on_the_probed_node_not_the_advertised_one() {
    let job = job(PlacementPolicy::DEFAULT);
    let honest = probe(advert(4, 5_000, 1 << 40, COLD));
    // Node 3 answers with node 4's id and a lower load.
    let mut claim = advert(4, 10, 1 << 40, COLD);
    claim.node = node(4);
    let liar = Probe {
        node: node(3),
        ..probe(claim)
    };
    let healthy = Some(CurrentPlacement {
        node: node(4),
        healthy: true,
    });
    let decision = place(&input(&job, healthy, vec![liar.clone(), honest.clone()]));
    assert_eq!(decision.chosen, Some(node(4)));
    let rank_of = |n: u8| match &decision
        .candidates
        .iter()
        .find(|c| c.node == node(n))
        .unwrap()
        .verdict
    {
        Verdict::Eligible { rank } => rank.clone(),
        Verdict::Excluded { .. } => Vec::new(),
    };
    assert!(!rank_of(3).contains(&RankReason::CurrentHealthy));
    // Two nodes alike but for the claimed id: the identity tiebreak is the
    // probed one, so node 3 sorts before node 5 whatever 3 claims.
    let five = probe(advert(5, 10, 1 << 40, COLD));
    let mut claim = advert(9, 10, 1 << 40, COLD);
    claim.node = node(9);
    let three = Probe {
        node: node(3),
        ..probe(claim)
    };
    let decision = place(&input(&job, None, vec![five, three]));
    assert_eq!(decision.chosen, Some(node(3)));
}

/// A failover never chooses the node of the attempt it replaces, however
/// well that node answers and ranks: the attempt may still run there
/// (ADR 0028). Any other placement kind keeps the M5 rank.
#[test]
fn a_failover_excludes_the_unavailable_node_whatever_it_offers() {
    let job = job(PlacementPolicy::DEFAULT);
    let lost = CurrentPlacement {
        node: node(1),
        healthy: false,
    };
    let best = probe(advert(1, 10, 1 << 40, WARM));
    let other = probe(advert(2, 9_000, 1 << 40, COLD));
    let mut failover = input(&job, Some(lost), vec![best.clone(), other.clone()]);
    failover.kind = PlacementKind::Failover;
    let decision = place(&failover);
    assert_eq!(decision.chosen, Some(node(2)));
    assert_eq!(reasons(&decision, 1), vec![Exclusion::Unavailable]);
    // Alone, the lost node parks the failover.
    failover.candidates = vec![best.clone()];
    assert_eq!(place(&failover).chosen, None);
    // A planned handoff from the same node may stay there.
    let mut handoff = input(&job, Some(lost), vec![best, other]);
    handoff.kind = PlacementKind::PlannedHandoff;
    assert_eq!(place(&handoff).chosen, Some(node(1)));
}
