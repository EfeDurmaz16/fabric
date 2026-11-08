//! Placement among registered nodes (ADR 0026): what this node advertises
//! for a job, how the authority probes every peer within the bound, and
//! the decision the pure `place` function makes from the probes.
//!
//! Invariant: facts used for a placement are measured at placement time;
//! nothing is cached between placements, and a peer that does not answer
//! within `PROBE_BOUND_MS` is excluded for this placement alone and stays
//! in the peer book. A secret is checked on the node that would run the
//! job, by the same two sources the executor's resolver reads, so a node
//! that cannot resolve one is excluded before anything launches.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::harness::HarnessKind;
use fabric_core::id::{NodeId, SnapshotId};
use fabric_core::managed::placement::{
    Capacity, CurrentPlacement, HarnessInventory, MAX_CANDIDATES, PROBE_BOUND_MS, PathLayout,
    PlacementInput, PlacementKind, Probe, Warmth,
};
use fabric_core::managed::{
    ManagedJob, NodeAdvert, PlacementDecision, SecretName, SecretRef, SecretScope, TaskId, place,
};
use fabric_core::path::AbsPath;
use fabric_core::snapshot::Platform;
use fabric_core::version::HarnessVersion;
use fabric_net::rpc::Advertise;
use fabric_net::wire::{AdvertWire, BoundedList, PlatformText, SecretNameText, VersionText};

use crate::clock::now_ms;
use crate::facts::{NodeFacts, Probes, Tool, find_on_path, name};
use crate::node::Node;
use crate::secrets::LocalResolver;
use crate::transfer::is_complete;

/// The probe bound as a duration.
pub(crate) const PROBE_BOUND: Duration = Duration::from_millis(PROBE_BOUND_MS as u64);

/// The version an ACP agent advertises: the agent reports none, and the
/// known-writer rule does not apply to it, so a found agent is listed as
/// present at this placeholder.
const UNVERSIONED: &str = "0.0.0";

/// What a placement is for, beside the probes: the pure input without
/// candidates, and what the probes ask each node about.
#[derive(Debug, Clone)]
pub(crate) struct PlacementAsk {
    /// The task placed.
    pub(crate) task: TaskId,
    /// Why.
    pub(crate) kind: PlacementKind,
    /// The captured layout a native session needs; `None` when any layout does.
    pub(crate) required_layout: Option<PathLayout>,
    /// Where the live attempt runs, if one does, and whether it is healthy.
    pub(crate) current: Option<CurrentPlacement>,
    /// The snapshot the attempt would start from or continue, for warmth.
    pub(crate) snapshot: Option<SnapshotId>,
}

/// The secrets of `names` this node cannot resolve now, asked of the
/// executor's own resolver (`secrets::LocalResolver`), which reads anew
/// every call: the environment, else a file under the node's config
/// directory that is this user's alone (ADR 0025).
pub(crate) fn unresolved_secrets(config_dir: &Path, names: &[SecretName]) -> Vec<SecretName> {
    let refs: Vec<SecretRef> = names
        .iter()
        .map(|name| SecretRef {
            name: name.clone(),
            scope: SecretScope::node_any(),
        })
        .collect();
    LocalResolver::open(config_dir).unresolved(&refs)
}

/// The 1-minute load average in thousandths, as the kernel reports it;
/// `u32::MAX` when it cannot be read, so an unmeasurable node ranks last.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded, finite, non-negative and below u32::MAX before the cast"
)]
async fn load_1m_milli() -> u32 {
    let text = if cfg!(target_os = "linux") {
        tokio::fs::read_to_string("/proc/loadavg").await.ok()
    } else {
        let output = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::process::Command::new("sysctl")
                .args(["-n", "vm.loadavg"])
                .output(),
        )
        .await;
        match output {
            Ok(Ok(output)) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            _ => None,
        }
    };
    text.as_deref()
        .and_then(|text| {
            text.split_whitespace().find_map(|token| {
                token
                    .trim_matches(|c| c == '{' || c == '}')
                    .parse::<f64>()
                    .ok()
            })
        })
        .filter(|load| load.is_finite() && *load >= 0.0)
        .map_or(u32::MAX, |load| {
            // Thousandths of a load average fit a u32 for any load a node survives.
            let milli = (load * 1000.0).round();
            if milli >= f64::from(u32::MAX) {
                u32::MAX
            } else {
                milli as u32
            }
        })
}

/// The free bytes of the volume that holds `path`.
fn free_disk_bytes(path: &Path) -> u64 {
    rustix::fs::statvfs(path).map_or(0, |stat| stat.f_bavail.saturating_mul(stat.f_frsize))
}

impl Node {
    /// This node's advert for one job, measured now (ADR 0026): what it
    /// answers a peer's `Advertise` and what it enters as a candidate
    /// for its own placements.
    pub(crate) async fn own_advert(&self, ask: &Advertise) -> AdvertWire {
        let env = self.node_env();
        let facts = NodeFacts::measure(
            &self.cfg.home,
            &env,
            Probes::NONE.with(Tool::Claude).with(Tool::Codex),
        )
        .await;
        let platform = facts.platform();
        let mut harnesses: Vec<(HarnessKind, VersionText)> = Vec::new();
        if let Some(version) = facts.get(name::CLAUDE) {
            harnesses.push((HarnessKind::ClaudeCode, advertised_version(version)));
        }
        if let Some(version) = facts.get(name::CODEX) {
            harnesses.push((HarnessKind::Codex, advertised_version(version)));
        }
        if let HarnessKind::Acp { program, .. } = &ask.harness
            && find_on_path(program.as_str(), env.path_var()).is_some()
        {
            harnesses.push((ask.harness.clone(), VersionText::truncated(UNVERSIONED)));
        }
        let root = crate::paths::job_root(&self.cfg.home, ask.job);
        let workspace = root.is_dir();
        let bundle_chain = match ask.snapshot {
            Some(snapshot) => is_complete(&self.store, snapshot).await.unwrap_or(false),
            None => false,
        };
        let names: Vec<SecretName> = ask
            .secrets
            .iter()
            .filter_map(|text| SecretName::parse(text.as_str()).ok())
            .collect();
        let unresolved = unresolved_secrets(self.cfg.state.root(), &names);
        let home =
            AbsPath::parse(&self.cfg.home.to_string_lossy()).unwrap_or_else(|_| ask.root.clone());
        let root = AbsPath::parse(&root.to_string_lossy()).unwrap_or_else(|_| ask.root.clone());
        AdvertWire {
            node: self.id,
            os: PlatformText::truncated(&platform.os),
            arch: PlatformText::truncated(&platform.arch),
            harnesses: BoundedList::new(harnesses).unwrap_or_default(),
            home,
            root,
            load_1m_milli: load_1m_milli().await,
            free_disk_bytes: free_disk_bytes(self.cfg.state.root()),
            bundle_chain,
            workspace,
            measured_at_ms: now_ms(),
            secrets_unresolved: BoundedList::new(
                unresolved
                    .iter()
                    .map(|n| SecretNameText::truncated(n.as_str()))
                    .collect(),
            )
            .unwrap_or_default(),
        }
    }

    /// Probes this node and every paired peer for `job` in parallel, each
    /// within `PROBE_BOUND` of the round's start, and returns one probe per
    /// node and the time of the decision (see [`probe_round`]). A peer that
    /// does not answer in time is a probe without an advert.
    pub(crate) async fn probe_candidates(self: &Arc<Self>, ask: &Advertise) -> (Vec<Probe>, u64) {
        let mut peers: Vec<NodeId> = self
            .peers
            .snapshot()
            .iter()
            .map(fabric_net::peers::PeerEntry::node)
            .collect();
        peers.sort();
        peers.truncate(MAX_CANDIDATES.saturating_sub(1));
        let mut asks: Vec<(NodeId, BoxFuture<'static, Option<AdvertWire>>)> =
            Vec::with_capacity(peers.len() + 1);
        let own = Arc::clone(self);
        let own_ask = ask.clone();
        asks.push((
            self.id,
            Box::pin(async move { Some(own.own_advert(&own_ask).await) }),
        ));
        for peer in peers {
            let node = Arc::clone(self);
            let ask = ask.clone();
            asks.push((
                peer,
                Box::pin(async move {
                    match node
                        .call(peer, |session| {
                            let ask = ask.clone();
                            async move { session.advertise(ask, PROBE_BOUND).await }
                        })
                        .await
                    {
                        Ok(advert) => Some(advert),
                        Err(e) => {
                            tracing::info!(%peer, error = %e, "a peer did not advertise for a placement");
                            None
                        }
                    }
                }),
            ));
        }
        probe_round(asks).await
    }

    /// Decides where `job`'s task runs: one probe round, then `place`.
    pub(crate) async fn place_job(
        self: &Arc<Self>,
        job: &ManagedJob,
        ask: &PlacementAsk,
    ) -> PlacementDecision {
        let root = crate::paths::job_root(&self.cfg.home, job.id);
        let root = AbsPath::parse(&root.to_string_lossy())
            .ok()
            .or_else(|| AbsPath::parse("/").ok());
        let Some(root) = root else {
            // No absolute root to name: no probe can be made, so the job parks.
            return place(&PlacementInput {
                job,
                task: ask.task,
                kind: ask.kind,
                required_layout: ask.required_layout.clone(),
                current: ask.current,
                candidates: Vec::new(),
                now_ms: now_ms(),
            });
        };
        let advertise = Advertise {
            job: job.id,
            harness: job.intent.harness.clone(),
            root,
            snapshot: ask.snapshot,
            secrets: job
                .policy
                .secrets
                .iter()
                .map(|s| SecretNameText::truncated(s.name.as_str()))
                .collect(),
        };
        let (candidates, decided_ms) = self.probe_candidates(&advertise).await;
        place(&PlacementInput {
            job,
            task: ask.task,
            kind: ask.kind,
            required_layout: ask.required_layout.clone(),
            current: ask.current,
            candidates,
            now_ms: decided_ms,
        })
    }
}

/// One probe round: every ask runs at once and is cut at `PROBE_BOUND`
/// from the round's start. Returns one probe per node, in the order
/// asked, and the decision's time.
///
/// Every advert is stamped with this node's clock when its answer
/// arrived, never with the time the answering node wrote in it: a peer's
/// clock may be far from this one, and freshness is this node's to judge.
/// The decision's time is now, but never later than the round's bound,
/// so a peer that stays silent for the whole bound makes no answer that
/// came within it stale; it is `no_answer` alone.
pub(crate) async fn probe_round(
    asks: Vec<(NodeId, BoxFuture<'static, Option<AdvertWire>>)>,
) -> (Vec<Probe>, u64) {
    let started = tokio::time::Instant::now();
    let round_start_ms = now_ms();
    let answers = futures_join_all(asks.into_iter().map(|(node, ask)| async move {
        let advert = tokio::time::timeout(PROBE_BOUND, ask).await.ok().flatten();
        let received_ms = now_ms();
        (node, advert, elapsed_ms(started), received_ms)
    }))
    .await;
    let probes = answers
        .iter()
        .map(|(node, advert, probe_ms, received_ms)| {
            let mut probe = probe_of(*node, advert.as_ref(), *probe_ms);
            if let Some(advert) = probe.advert.as_mut() {
                advert.measured_at_ms = (*received_ms).max(round_start_ms);
            }
            probe
        })
        .collect();
    let decided_ms = now_ms().min(round_start_ms.saturating_add(u64::from(PROBE_BOUND_MS)));
    (probes, decided_ms.max(round_start_ms))
}

fn elapsed_ms(started: tokio::time::Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Runs every future at once and collects the answers in order.
async fn futures_join_all<F>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futures.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(value) = handle.await {
            out.push(value);
        }
    }
    out
}

/// A probe from a peer's answer, or its silence.
pub(crate) fn probe_of(node: NodeId, advert: Option<&AdvertWire>, probe_ms: u32) -> Probe {
    let secrets_unresolved = advert
        .map(|a| {
            a.secrets_unresolved
                .as_slice()
                .iter()
                .filter_map(|text| SecretName::parse(text.as_str()).ok())
                .collect()
        })
        .unwrap_or_default();
    Probe {
        node,
        advert: advert.map(advert_from_wire),
        secrets_unresolved,
        probe_ms,
    }
}

/// The version a harness's `--version` output names, as the advert
/// carries it: `codex-cli 0.155.1` is `0.155.1`, the form the asking node
/// parses. Output with no version in it is carried as it came, and the
/// asking node then lists the harness as absent.
fn advertised_version(output: &str) -> VersionText {
    HarnessVersion::from_output(output).map_or_else(
        |_| VersionText::truncated(output),
        |version| VersionText::truncated(&version.to_string()),
    )
}

/// The advert in the core's form; a harness whose version does not parse
/// is left out, as if not installed, so no node is admitted on a version
/// nobody can compare.
pub(crate) fn advert_from_wire(wire: &AdvertWire) -> NodeAdvert {
    NodeAdvert {
        node: wire.node,
        platform: Platform {
            os: wire.os.as_str().to_owned(),
            arch: wire.arch.as_str().to_owned(),
        },
        harnesses: wire
            .harnesses
            .as_slice()
            .iter()
            .filter_map(|(kind, version)| {
                HarnessVersion::parse(version.as_str())
                    .ok()
                    .map(|version| HarnessInventory {
                        kind: kind.clone(),
                        version,
                    })
            })
            .collect(),
        layout: PathLayout {
            home: wire.home.clone(),
            root: wire.root.clone(),
        },
        capacity: Capacity {
            load_1m_milli: wire.load_1m_milli,
            free_disk_bytes: wire.free_disk_bytes,
        },
        warmth: Warmth {
            bundle_chain: wire.bundle_chain,
            workspace: wire.workspace,
        },
        measured_at_ms: wire.measured_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use fabric_core::id::JobId;
    use fabric_core::managed::placement::{Exclusion, Verdict};
    use fabric_core::managed::{
        Generation, Intent, JobPolicy, JobState, PlacementPolicy, RestartPolicy, SourceBinding,
        WorkRevision,
    };

    use super::*;
    use crate::ops::tests::node;

    fn job_on(node: &Node, pin: Option<NodeId>) -> ManagedJob {
        let _ = node;
        ManagedJob {
            id: JobId::from_bytes([1; 16]),
            revision: WorkRevision::FIRST,
            generation: Generation::FIRST,
            intent: Intent {
                goal: "fix the test".into(),
                harness: HarnessKind::Acp {
                    program: fabric_core::harness::ProgramName::parse("sh").unwrap(),
                    args: Vec::new(),
                },
                acceptance: Vec::new(),
            },
            source: SourceBinding::Empty,
            policy: JobPolicy {
                placement: PlacementPolicy {
                    pin,
                    ..PlacementPolicy::DEFAULT
                },
                ..JobPolicy::m2_defaults(RestartPolicy::ExplicitOnly)
            },
            state: JobState::Active,
            primary: TaskId::from_bytes([2; 16]),
        }
    }

    fn ask(task: TaskId) -> PlacementAsk {
        PlacementAsk {
            task,
            kind: PlacementKind::Initial,
            required_layout: None,
            current: None,
            snapshot: None,
        }
    }

    /// A node advertises what it measures now: its platform, the harness
    /// the job names when it is on the PATH, the job's root as it would be
    /// here, and the secrets it cannot resolve; nothing is cached, so a
    /// secret set between two adverts shows in the second alone.
    #[tokio::test]
    async fn an_advert_is_measured_now_and_names_the_unresolved_secrets() {
        let (_dir, node) = node();
        let job = job_on(&node, None);
        let advertise = Advertise {
            job: job.id,
            harness: job.intent.harness.clone(),
            root: AbsPath::parse("/elsewhere").unwrap(),
            snapshot: None,
            secrets: vec![SecretNameText::truncated("PLACEMENT_TEST_SECRET")],
        };
        let advert = node.own_advert(&advertise).await;
        assert_eq!(advert.node, node.id);
        assert!(!advert.os.as_str().is_empty() && !advert.arch.as_str().is_empty());
        assert!(
            advert
                .harnesses
                .as_slice()
                .iter()
                .any(|(kind, _)| *kind == job.intent.harness),
            "sh is on the PATH: {:?}",
            advert.harnesses
        );
        assert_eq!(
            advert.root.as_str(),
            crate::paths::job_root(&node.cfg.home, job.id).to_string_lossy()
        );
        assert!(!advert.workspace, "no workspace yet");
        assert!(!advert.bundle_chain, "no snapshot asked about");
        assert_eq!(
            advert.secrets_unresolved.as_slice()[0].as_str(),
            "PLACEMENT_TEST_SECRET"
        );
        // A file under <state>/secrets resolves it when it is this user's alone.
        let secrets = node.cfg.state.root().join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        let file = secrets.join("PLACEMENT_TEST_SECRET");
        std::fs::write(&file, "value").unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
        let name = SecretName::parse("PLACEMENT_TEST_SECRET").unwrap();
        assert_eq!(
            unresolved_secrets(node.cfg.state.root(), std::slice::from_ref(&name)),
            vec![name.clone()],
            "a file others can read does not resolve"
        );
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        assert!(unresolved_secrets(node.cfg.state.root(), std::slice::from_ref(&name)).is_empty());
        let again = node.own_advert(&advertise).await;
        assert!(again.secrets_unresolved.as_slice().is_empty());
        assert!(again.measured_at_ms >= advert.measured_at_ms);
    }

    /// A probe round over this node alone: the node is its own candidate,
    /// a job it can run is placed on it, a pin to another node parks the
    /// job with the pin named, and an unresolvable secret excludes it
    /// before anything launches.
    #[tokio::test]
    async fn a_node_alone_places_on_itself_or_parks_with_the_reason() {
        let (_dir, node) = node();
        let job = job_on(&node, None);
        let decision = node.place_job(&job, &ask(job.primary)).await;
        assert_eq!(decision.chosen, Some(node.id), "{decision:?}");
        assert_eq!(decision.candidates.len(), 1);

        let pinned = job_on(&node, Some(NodeId::from_bytes([9; 32])));
        let decision = node.place_job(&pinned, &ask(job.primary)).await;
        assert_eq!(decision.chosen, None);
        assert!(matches!(
            &decision.candidates[0].verdict,
            Verdict::Excluded { reasons } if reasons == &[Exclusion::Pin { pinned: NodeId::from_bytes([9; 32]) }]
        ));

        let mut needs_secret = job_on(&node, None);
        needs_secret.policy.secrets = vec![fabric_core::managed::SecretRef {
            name: SecretName::parse("NEVER_SET_FOR_THIS_TEST").unwrap(),
            scope: fabric_core::managed::SecretScope::node_any(),
        }];
        let decision = node.place_job(&needs_secret, &ask(job.primary)).await;
        assert_eq!(decision.chosen, None);
        assert!(matches!(
            &decision.candidates[0].verdict,
            Verdict::Excluded { reasons } if matches!(reasons[0], Exclusion::Secret { .. })
        ));
    }

    /// A wire advert for `node` that can run `job_on`'s job, measured at `at_ms`.
    fn runnable(node: NodeId, at_ms: u64) -> AdvertWire {
        AdvertWire {
            node,
            os: PlatformText::truncated("macos"),
            arch: PlatformText::truncated("arm64"),
            harnesses: BoundedList::new(vec![(
                HarnessKind::Acp {
                    program: fabric_core::harness::ProgramName::parse("sh").unwrap(),
                    args: Vec::new(),
                },
                VersionText::truncated(UNVERSIONED),
            )])
            .unwrap(),
            home: AbsPath::parse("/Users/me").unwrap(),
            root: AbsPath::parse("/Users/me/fabric/j").unwrap(),
            load_1m_milli: 120,
            free_disk_bytes: 1 << 40,
            bundle_chain: false,
            workspace: false,
            measured_at_ms: at_ms,
            secrets_unresolved: BoundedList::default(),
        }
    }

    fn decide(job: &ManagedJob, probes: Vec<Probe>, decided_ms: u64) -> PlacementDecision {
        place(&PlacementInput {
            job,
            task: job.primary,
            kind: PlacementKind::Initial,
            required_layout: None,
            current: None,
            candidates: probes,
            now_ms: decided_ms,
        })
    }

    /// A peer that never answers (a laptop asleep) costs the round its
    /// bound and nothing more: the node that answered at once is placed,
    /// not excluded as stale, and the silent one is `no_answer`.
    #[tokio::test]
    async fn a_silent_peer_does_not_make_the_answering_nodes_stale() {
        let (_dir, node) = node();
        let job = job_on(&node, None);
        let (me, asleep) = (node.id, NodeId::from_bytes([9; 32]));
        let own = runnable(me, now_ms());
        let (probes, decided_ms) = probe_round(vec![
            (me, Box::pin(async move { Some(own) })),
            (asleep, Box::pin(std::future::pending())),
        ])
        .await;
        let decision = decide(&job, probes, decided_ms);
        assert_eq!(decision.chosen, Some(me), "{decision:?}");
        assert!(matches!(
            &decision.candidates[1].verdict,
            Verdict::Excluded { reasons } if matches!(reasons[..], [Exclusion::NoAnswer { .. }])
        ));
    }

    /// A peer whose clock is a minute behind this node's is judged by when
    /// its answer arrived here, not by the time it wrote in the advert.
    #[tokio::test]
    async fn a_peer_with_a_skewed_clock_is_not_stale() {
        let (_dir, node) = node();
        let job = job_on(&node, None);
        let peer = NodeId::from_bytes([4; 32]);
        let behind = runnable(peer, now_ms().saturating_sub(60_000));
        let (probes, decided_ms) =
            probe_round(vec![(peer, Box::pin(async move { Some(behind) }))]).await;
        let decision = decide(&job, probes, decided_ms);
        assert_eq!(decision.chosen, Some(peer), "{decision:?}");
    }

    /// A harness's `--version` output is advertised as the version in it,
    /// so the asking node admits a real Codex or Claude Code.
    #[test]
    fn a_version_output_is_advertised_as_its_version() {
        assert_eq!(advertised_version("codex-cli 0.155.1").as_str(), "0.155.1");
        assert_eq!(advertised_version("2.1.3 (Claude Code)").as_str(), "2.1.3");
        assert!(HarnessVersion::parse(advertised_version("codex-cli 0.155.1").as_str()).is_ok());
        assert_eq!(
            advertised_version("no version here").as_str(),
            "no version here"
        );
    }

    /// A wire advert whose harness version does not parse lists that
    /// harness as absent, and a silent peer is a probe without an advert.
    #[test]
    fn a_wire_advert_reads_back_without_an_unparsable_version() {
        let wire = AdvertWire {
            node: NodeId::from_bytes([3; 32]),
            os: PlatformText::truncated("macos"),
            arch: PlatformText::truncated("arm64"),
            harnesses: BoundedList::new(vec![
                (HarnessKind::Codex, VersionText::truncated("0.50.0")),
                (
                    HarnessKind::ClaudeCode,
                    VersionText::truncated("not a version"),
                ),
            ])
            .unwrap(),
            home: AbsPath::parse("/Users/me").unwrap(),
            root: AbsPath::parse("/Users/me/fabric/j").unwrap(),
            load_1m_milli: 120,
            free_disk_bytes: 1 << 40,
            bundle_chain: true,
            workspace: false,
            measured_at_ms: 5,
            secrets_unresolved: BoundedList::new(vec![SecretNameText::truncated("API_TOKEN")])
                .unwrap(),
        };
        let probe = probe_of(wire.node, Some(&wire), 7);
        let advert = probe.advert.unwrap();
        assert_eq!(advert.harnesses.len(), 1);
        assert_eq!(advert.harnesses[0].kind, HarnessKind::Codex);
        assert_eq!(probe.secrets_unresolved[0].as_str(), "API_TOKEN");
        assert_eq!(probe.probe_ms, 7);
        let silent = probe_of(
            wire.node,
            None,
            u32::try_from(PROBE_BOUND.as_millis()).unwrap(),
        );
        assert!(silent.advert.is_none());
    }
}
