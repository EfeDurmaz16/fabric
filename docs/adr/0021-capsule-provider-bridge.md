# ADR 0021: Versioned provider bridge to a Capsule worker

Status: proposed, 2026-10-27

Adopts external drafts P09 ("Scoped provider identity and truthful operation outcomes") and P10 ("Capsule worker protocol and standalone distribution"), both of 2026-10-27 and both Proposed, with the handoff selections of the same date, for the M3 campaign. Draft P12 ("Provider-neutral conformance and layered evidence") sets the test layers. Draft P11 ("Unknown outcomes, reconciliation and cleanup authorization") is read only so that v1 does not block it; nothing in this ADR implements reconciliation. The protocol itself, its message catalogue and its fixtures are in [docs/bridge-protocol.md](../bridge-protocol.md) and [protocol/bridge/v1](../../protocol/bridge/v1).

## Context

ADR 0019 and ADR 0020 give a managed job durable identity and a journal that records intent before any effect and leaves an unobserved effect `unknown`. In M2 the only place an attempt can run is the local node (`ResourceRef::Node`). M3 needs a second kind of place: a resource a provider owns, such as a container or a cloud sandbox.

Capsule already maps governed actions onto many providers, in TypeScript. Its M1 work (Capsule PR #316) added the types this seam needs: a scoped `ResourceLocator`, `PendingOperationError` for an accepted action that did not finish within its bound, `UnknownOutcomeError` for an action whose effect cannot be known, the deny-all secrets rule and fresh per-operation approval. Fabric is Rust. Linking the two in one process would tie their runtimes, release cycles and failure domains together.

## Decision

### A long-lived local worker over stdio

Fabric talks to an optional Capsule worker executable, spawned as a child process, through framed messages on its stdin and stdout. The framing is Fabric's existing one (ADR 0018): a `u32` little-endian length, then one UTF-8 JSON object. Frames from Fabric, hello included, are at most 1 MiB; frames from the worker at most 16 MiB, except its handshake answer, at most 1 MiB. Lengths are checked before allocation. Stderr is bounded diagnostics. There is no listener, no remote transport and no authentication service in v1; the client API of ADR 0018 is neither exported nor changed.

### A versioned handshake that fixes the capability profile

`hello` names a range of protocol majors, a profile and limits; `hello_ack` picks the version, returns the whole capability profile the worker will enforce and may only lower the limits. A worker that cannot serve answers `hello_refused`, a message that never changes shape. No provider call, credential read or approval resolution happens before the ack. A capability profile describes invocable, verified operations: which actions, which of them are terminal-only, which support observe and cancel, which need a fresh approval, and how duplicates are handled across worker restarts.

### Ownership stays where it is

Fabric owns job, task, attempt and operation identity, the journal, placement and fencing. The worker owns the provider mapping, provider calls, observations and receipts. Every request carries the journal's operation id and idempotency key, the job, task and attempt, the generation as a canonical decimal string, a fresh approval slot and a deadline. Every result and every effectful error carries the full resource locator: provider, scope, kind and native id. The worker keeps no durable job state and never decides that an operation should be retried.

### Truthful outcomes, refusals before effects

Selections adopted from the handoff, each pinned by a fixture:

- Refusals (`invalid`, `unsupported`, `denied`, `unavailable`) carry `effect: "none"` and happen before any provider call.
- Worker loss is not cancellation: in-flight dispatched operations become `unknown`, identity does not change.
- A timeout is not permission to replay; certainty returns only through `observe` by the original operation.
- Terminal-only actions stay terminal-only, with bounded observation and a typed `pending` answer carrying a safe locator, the shape of Capsule M1's `PendingOperationError`. `unknown` carries the best known locator, the shape of `UnknownOutcomeError`.
- Fresh per-operation authorization is additive: it can grant what policy leaves to approval, never what policy denies, and a create grant never authorizes destroy.
- A secrets block that is present with `allowed` omitted denies all secrets, and two allowlists merge by intersection.
- Cancel acknowledgement is not termination.

### Fixture provider first, Docker second

The first provider proof is a capability-profile contract. A deterministic, stateful fixture provider with a virtual clock comes first and runs every scenario; local Docker follows as the physical proof of process, exit and cleanup behavior, never pulling an image. Emulate (`vercel-labs/emulate`) is optional test infrastructure only and not part of either profile. Neither provider name enters Fabric's job domain; Fabric reads the profile.

### One source of truth, two byte-identical copies

`docs/bridge-protocol.md` and `protocol/bridge/v1` in this repository are the source of truth. Capsule carries a byte-identical copy of the fixtures at the same path and a pointer to this document. A sha256 manifest lists every fixture file, and each repository's test pins the manifest's own digest, so a fixture change lands in both repositories or fails in one.

## Alternatives considered

1. Rewrite the provider adapters in Rust. Rejected for M3: it duplicates Capsule's provider work and SDK tracking without a measured need.
2. Load Capsule in process through FFI or an embedded JavaScript runtime. Rejected: native packaging, shared failure domain and async ownership problems, and it would let SDK objects cross a boundary that should carry data only.
3. A JSON-lines protocol. Rejected: it needs a second parser with an unbounded pending line; the framed reader already exists in both languages.
4. A local socket or a network service. Rejected for v1: it adds an addressable endpoint and an authentication decision (draft P07) that one parent and one child process do not need.
5. Let the worker deduplicate and retry on its own. Rejected: it would make the worker a second job controller and hide unknown outcomes.

## Consequences

- Fabric gains a provider seam without a new runtime dependency; the worker is optional and a node without it runs managed jobs on itself as in M2.
- Two implementations must agree on one byte-exact fixture set, and a protocol change is a linked pair of pull requests.
- `ResourceRef` gains a provider form when placement can choose a provider for an attempt, an open tagged object an older reader refuses (docs/managed-jobs.md). The M3 client does not need it: a provider operation names the attempt it acts for, and the attempt keeps its own resource.
- The worker artifact must be proven outside the repository without Node, Bun or pnpm on `PATH` (draft P10 acceptance); that proof and the round-trip and spawn latency measurements belong to the implementation commits of M3, not to this ADR.
- Reconciliation, cleanup authorization after cancellation and attach (draft P11) stay later work. v1 carries the identities and answers they need.

## Implementation notes (M3 client)

- The client is the crate `fabric-bridge`; its behavior per answer is in docs/bridge-protocol.md, "Fabric's client as built".
- A bridge effect is a provider operation in its own journal table, `provider_operations` (schema version 2), not a new command kind. Version 1's `operations` table checks its `kind` and `status` against fixed lists, which SQLite changes only by rebuilding a table that `attempts` and `events` reference, and it ties `rejected` to an operation that was never accepted, while the bridge records `rejected` after dispatch when the worker refuses before effect. The new table shares the key and id space of `operations`, names the job, task, attempt and generation it was fenced on, and is recovered like an operation: `dispatched` becomes `unknown` on restart.
- The journal stores the worker's locator as the JSON text it came in and never parses it. Fabric reads only the capability profile.
- The Rust test double (`src/bin/double.rs`) parses with the crate's worker-side parser, so it refuses as the protocol says; it is test infrastructure, not a provider.
- Measured on the double, with raw samples in docs/evidence/m3-bridge: a worker spawned with its handshake answered, one request round trip with an in-memory log, and with the journal's two committed writes. The worker artifact proof outside the repository, the Linux artifact on CI and the Docker smoke belong to the Capsule worker and stay open for P10 acceptance.
