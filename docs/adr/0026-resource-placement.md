# ADR 0026: Resource placement among registered nodes

Status: proposed, 2026-11-03.

Adopts the external draft P06 ("Resource placement and compute provider boundary", 2026-10-25, Proposed) for the M5 campaign. Draft P06 left open the first ranking rule and user limits, target deployment, capacity observation bounds and provider access. Each is selected below. The selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe, and Efe can veto any of them at the pull request. The types and the pure placement function are in [docs/managed-jobs.md](../managed-jobs.md), section "Placement and planned continuity (M5)", part "Placement (P06)". It supersedes the placement deferral of ADR 0008 for managed jobs and the scheduler exclusion of ADR 0010 to the extent below; it does not authorize inference routing, a global mesh or a provisioner.

## Context

Placement today is the user's choice by name: `fabric job move --to` or `fabric resume` on the node that wants the job (ADR 0008). The product statement asks for intent first: the user submits work and Fabric picks a resource it can prove is able to run it. M2 to M4 give the pieces to prove it: node identity and the peer book (ADR 0005, 0013), the facts a node measures about itself and the question a peer asks for them (`FactsFor`, ADR 0013 amended), the harness known-writer rule (ADR 0006), the path identity rule (ADR 0011), the store that knows which snapshots and chunks it holds (ADR 0014), and the grant that fences an attempt to one resource (ADR 0020). None of them chooses.

## Decision

### Admitted resources: registered nodes

A placement considers registered Fabric nodes only: the authority's own node and the peers in its peer book, each already authenticated by its pinned node identity. No provider resource, no VM provisioning and no OpenCompute in M5. A provider resource (ADR 0021) stays reachable through the bridge from the node an attempt was placed on.

### What a node advertises

A node answers a placement question with: its identity; its platform (`os`, `arch`, as the facts already carry them); its harness inventory (each harness kind it has and its version, from the existing detection); its path layout (home, and the job's root as it would be on that node); its capacity (the 1-minute load average and the free bytes of the store's volume); its warmth for the job (whether its store holds the job's bundle chain, and whether it holds the job's workspace); and the time it measured these. The facts used for one placement are probed at placement time, within a 5 s bound per node; a node that does not answer within it is ineligible for that placement with the exclusion `no_answer`, and is not removed from the peer book. No fact is reused from an earlier placement.

### Hard filters, all required

A node is eligible only when every one of these holds: its platform matches the job's requirement; every harness the job requires is present and version-compatible under the known-writer rule of ADR 0006; its path layout is compatible under ADR 0011 for the fidelity the job allows (ADR 0024); every `SecretRef` of the job resolves on it (ADR 0025); its free disk is at least the job's declared need, 2 GiB unless the policy says more; it is the user's pin when the job has one; and the budget inputs hold. M5 has no budget input beyond disk and load, so none can be waived, and no warm cache makes an incompatible node eligible.

### Deterministic ranking

The eligible set is ranked, and the first wins:

1. The current placement, when it is eligible and the attempt there is healthy: stability wins, and healthy work is never moved for a warmer target.
2. Warmest: a node whose store holds the job's bundle chain ranks above one that does not; among those, one that holds the workspace ranks above one that does not.
3. Lowest 1-minute load.
4. Node identity, as the final tiebreak, so two equal nodes are ordered the same way every time.

The decision records every candidate, each exclusion with its reason and the values compared, and the rank reasons of every eligible node in order. It is a journal event, so `inspect` shows why a job runs where it runs.

### No eligible node

The submit is accepted and committed, and the job is parked: its primary task is open, no attempt is granted, and the decision with its structured exclusions is recorded. Nothing runs, and nothing asks the user to name an arbitrary machine. The next placement (a retry, a `continue`, or a node that comes back) evaluates again from fresh facts.

### Three operations, three tests

Initial placement (at submit), planned handoff (`continue`, after a quiescence and a cut under ADR 0023) and unexpected-loss failover (M6, ADR 0028) are separate operations with separate tests. The placement function is shared; what each operation requires before it may grant is not.

## Alternatives considered

1. Keep explicit machine choice as the only path. Rejected: it fails the stated experience; the pin keeps it available.
2. Build a compute marketplace or a provisioner now. Rejected: no real integration earns the interface yet (draft P06).
3. Let the harness choose a host. Rejected: it confuses reasoning with execution authority; no opaque score waives a filter.
4. Cache facts between placements with a TTL. Rejected by draft P06: fresh facts are checked before launch; a 5 s probe is cheap next to a launch.
5. Rank warmth above stability. Rejected: moving healthy work for a warmer cache costs a quiescence and a cut for nothing the user asked for.

## Consequences

- `fabric-net` gains one request and one answer (`Advertise`, `Advert`) in protocol 4, which has not shipped; `fabricd` gains a `placement` module that probes, filters, ranks and records.
- The journal gains the `PlacementDecided` event; a placement decision is at most 64 candidates so the event stays small.
- A pinned job can be parked when its pin is ineligible, with the exclusion naming why.
- `ResourceRef::Node` stays the one form an attempt is granted on in M5.

## As built (M5)

Built on 2026-11-03 by owner B. Where the code differs from the decision above, docs/managed-jobs.md, "As built (M5): P06 and P07", says how and why. In short: `fabric_core::managed::place` is the pure function of the decision, with a property test over the order of the probes; `fabricd::placement` measures this node's advert now (the platform and harness versions from the facts, the job's root as it would be here, the 1-minute load from the kernel, the free bytes of the state directory's volume, warmth from the store and the job root, and the secrets the executor's own resolver cannot read), probes every paired peer in parallel within the 5 s bound, and decides. The known-writer version check is not a placement filter: the placement input carries no writer, and the fidelity a target can offer is decided at the continuation (`offered_fidelity`), so `Exclusion::HarnessVersion` is defined and unused by `place`. An ACP agent found on the target's `PATH` is advertised at the placeholder version `0.0.0`, since an agent reports none. The authority's dispatch of a create's and a continue's placement through `Node::place_job` is owner A's wiring and had not landed when this was written; the decision's recording (`PlacementDecided`) and its reading (`inspect`) are in place.

## Open questions

- Budget inputs beyond disk and load (cost, data placement permissions) once a provider resource can be placed.
- Whether a node may advertise for a job it has never seen a snapshot of, with a transfer cost estimate as a rank input.

The dispatch landed in the acceptance step (`fabricd::dispatch`): an accepted create from a `snapshot`, `git` or `empty` source is placed through `Node::place_job` after its commit and after a restart, and the decision is recorded with its grant or the park; a `repository` create keeps the M2 path. A harness is advertised by the version in its `--version` output, so a real Codex or Claude Code is admitted. The launch of the grant on the chosen node is not built; docs/managed-jobs.md, "As built (M5): the initial placement's dispatch", says what is missing.
