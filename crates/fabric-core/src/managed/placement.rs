//! The types of a placement decision (ADR 0026): what a node advertises,
//! what a placement is asked with, and the decision it records with every
//! candidate, every exclusion and every rank reason. The function that
//! decides is `place.rs`.
//!
//! Invariant: a decision names every candidate that was probed, so a
//! parked job shows why each node was excluded and a placed one shows why
//! its node won.

use serde::{Deserialize, Serialize};

use super::ids::TaskId;
use super::model::ManagedJob;
use super::secret_ref::SecretName;
use crate::harness::HarnessKind;
use crate::id::NodeId;
use crate::path::AbsPath;
use crate::snapshot::Platform;
use crate::version::HarnessVersion;

/// How long a placement waits for a node's answer, in milliseconds.
pub const PROBE_BOUND_MS: u32 = 5_000;
/// The most candidates one placement considers.
pub const MAX_CANDIDATES: usize = 64;
/// The free disk a job needs when its policy names no other: 2 GiB.
pub const DEFAULT_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One harness a node has installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessInventory {
    /// The harness.
    pub kind: HarnessKind,
    /// Its version.
    pub version: HarnessVersion,
}

/// The paths a session is bound to (ADR 0011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathLayout {
    /// The user's home.
    pub home: AbsPath,
    /// The workspace root.
    pub root: AbsPath,
}

/// What a node has to spare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    /// The one-minute load average in thousandths.
    pub load_1m_milli: u32,
    /// Free bytes on the volume that holds the state directory.
    pub free_disk_bytes: u64,
}

/// What a node already holds of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warmth {
    /// Its store holds the job's bundle chain.
    pub bundle_chain: bool,
    /// Its filesystem holds the job's workspace.
    pub workspace: bool,
}

/// What a node says about itself for one placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAdvert {
    /// The node.
    pub node: NodeId,
    /// Its operating system and architecture.
    pub platform: Platform,
    /// The harnesses it has installed.
    pub harnesses: Vec<HarnessInventory>,
    /// Its home and the root it would run the job at.
    pub layout: PathLayout,
    /// What it has to spare.
    pub capacity: Capacity,
    /// What it already holds of the job.
    pub warmth: Warmth,
    /// When it measured these facts, Unix milliseconds.
    pub measured_at_ms: u64,
}

/// Why a placement is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlacementKind {
    /// A submit's first attempt.
    Initial,
    /// A `Continue`.
    PlannedHandoff,
    /// A recovery after an unexpected loss (M6).
    Failover,
}

impl PlacementKind {
    /// The name the journal's `placements.kind` stores.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PlacementKind::Initial => "initial",
            PlacementKind::PlannedHandoff => "planned_handoff",
            PlacementKind::Failover => "failover",
        }
    }
}

/// Where the job's live attempt runs, if one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentPlacement {
    /// The node.
    pub node: NodeId,
    /// Whether the attempt there is healthy.
    pub healthy: bool,
}

/// One node's answer to a probe, or its silence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The node.
    pub node: NodeId,
    /// Its advert; `None` when it did not answer within `PROBE_BOUND_MS`.
    pub advert: Option<NodeAdvert>,
    /// The job's secrets its own resolver could not resolve.
    pub secrets_unresolved: Vec<SecretName>,
    /// How long the probe took.
    pub probe_ms: u32,
}

/// What a placement is decided from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementInput<'a> {
    /// The job.
    pub job: &'a ManagedJob,
    /// The task to place.
    pub task: TaskId,
    /// Why.
    pub kind: PlacementKind,
    /// The captured root and home, when a native session is allowed.
    pub required_layout: Option<PathLayout>,
    /// Where the live attempt runs, if one does.
    pub current: Option<CurrentPlacement>,
    /// Every probe, at most `MAX_CANDIDATES`.
    pub candidates: Vec<Probe>,
    /// Now, Unix milliseconds.
    pub now_ms: u64,
}

/// Why a node is not eligible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exclusion {
    /// It did not answer in time.
    NoAnswer {
        /// The bound it missed.
        within_ms: u32,
    },
    /// Its facts are older than the probe bound.
    Stale {
        /// How old.
        age_ms: u64,
    },
    /// Its platform is not the job's.
    Platform {
        /// What the job needs.
        need: Platform,
        /// What the node has.
        have: Platform,
    },
    /// It lacks a required harness.
    HarnessMissing {
        /// The harness.
        kind: HarnessKind,
    },
    /// Its harness is older than the session's newest writer.
    HarnessVersion {
        /// The harness.
        kind: HarnessKind,
        /// The version needed.
        need: HarnessVersion,
        /// The version it has.
        have: HarnessVersion,
    },
    /// Its layout is not the captured one.
    Layout {
        /// The captured layout.
        need: PathLayout,
        /// The node's.
        have: PathLayout,
    },
    /// It cannot resolve a secret the job needs.
    Secret {
        /// The secret.
        name: SecretName,
    },
    /// It has less free disk than the job needs.
    Disk {
        /// Needed.
        need: u64,
        /// Free.
        free: u64,
    },
    /// The user pinned another node.
    Pin {
        /// The pinned node.
        pinned: NodeId,
    },
}

/// Why an eligible node ranks where it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RankReason {
    /// It runs the healthy current attempt.
    CurrentHealthy,
    /// It holds the bundle chain.
    BundleChain,
    /// It holds the workspace.
    Workspace,
    /// Its load.
    Load {
        /// Thousandths.
        milli: u32,
    },
    /// The final tiebreak by node identity.
    Identity,
}

/// A candidate's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Eligible, with why it ranks where it does.
    Eligible {
        /// The rank reasons, in order of weight.
        rank: Vec<RankReason>,
    },
    /// Excluded, with every reason.
    Excluded {
        /// The reasons.
        reasons: Vec<Exclusion>,
    },
}

/// One probed node and its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The node.
    pub node: NodeId,
    /// Its verdict.
    pub verdict: Verdict,
}

/// A placement decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementDecision {
    /// Why it was made.
    pub kind: PlacementKind,
    /// When, Unix milliseconds.
    pub decided_at_ms: u64,
    /// Every probe: the eligible ones in rank order, then the excluded ones.
    pub candidates: Vec<Candidate>,
    /// The node chosen; `None` parks the job.
    pub chosen: Option<NodeId>,
}
