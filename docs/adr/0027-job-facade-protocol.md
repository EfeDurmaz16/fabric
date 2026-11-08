# ADR 0027: The job facade: authenticated remote job operations over the node link

Status: proposed, 2026-11-03.

Adopts the external draft P07 ("Authenticated remote job operations", 2026-10-25, Proposed) for the M5 campaign. Draft P07 left open authentication, reachability, transport and the first client integration. Each is selected below. The selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe, and Efe can veto any of them at the pull request. The wire contract is [docs/facade-protocol.md](../facade-protocol.md) with the fixtures in [protocol/facade/v1](../../protocol/facade/v1); the token and operation types are in [docs/managed-jobs.md](../managed-jobs.md), section "Placement and planned continuity (M5)", part "Facade (P07)". It extends ADR 0018 (the client API) with a separate remote boundary and keeps ADR 0020's rule that the authority alone accepts commands.

## Context

The client API of ADR 0018 is owner-only: it runs on a local socket and its operations carry host paths, launch plans and process registration. That authority cannot be handed to a remote client. The managed authority of ADR 0020 accepts commands only on that socket ("Local only"), which leaves a job unreachable from the owner's other machines. ADR 0022's selection 6 carried the read surface for provider operations to M5 as well. What is needed is a second, narrower boundary with its own authentication and scope, not an export of the first.

## Decision

### Transport: the existing authenticated node link

In M5 the facade travels on the node-to-node link of `fabric-net` (QUIC first, TCP with TLS as fallback, pinned ed25519 identities, protocol 4), as one new request kind whose payload is a facade message in JSON, and one new answer kind. It is a protocol distinct from the owner-only socket protocol: no socket operation is reachable through it. There is no public listener, no browser client and no mobile client in M5. The facade is reachable from any registered node of the same owner: a client speaks to its local node over the client API, which forwards the facade message to the authority node over the link and returns the authority's answer unchanged. A client on the authority's own node takes the same path without the link.

### Authentication and scope: a facade token minted by the owner

The owner mints a token on the authority node, over the local socket: `{ actor, allowed jobs (ids or any owned), allowed resources (node ids or any), expiry, revocation id }`. The bearer secret is 32 random bytes shown once; the authority stores its blake3 digest with the token's scope, so a journal backup leaks no bearer. Every facade message carries the bearer. The authority checks, before anything else and in this order: the facade version, the token's digest is known, not revoked, not expired, then the job and resource scope of the message. Revocation is one local command naming the revocation id; it is immediate for new messages, and an open watch ends within one bounded delivery with the reason `revoked`.

### Operations

| Operation | Kind | Answer |
| --- | --- | --- |
| `submit(intent, source binding, policy)` | command | `accepted` with the operation id, after durable commit only; the job is placed or parked (ADR 0026) |
| `inspect(job)` | read | the job as the journal holds it: tasks, attempts, operations, the latest placement decision, continuations, provider operations, blockers and residuals (ADR 0022, selection 6) |
| `watch(job, cursor, wait)` | read | a bounded page of events after the cursor, the next cursor, and whether the reader lagged |
| `cancel(job, task?)` | command | `accepted`; the cancel is a command with its own outcome, and never claims rollback of completed or unknown external effects |
| `checkpoint(job)` | command | `accepted`; the quiescence and the cut of ADR 0023 run under the operation |
| `continue(job, fidelity, pin?)` | command | `accepted`; the planned handoff: a placement, a grant and a continuation recorded with its achieved fidelity (ADR 0024) |

Commands carry an operation key and the expected work revision. A repeat with the same key and the same request reconciles the same operation and gets the same answer marked `replayed`; the same key with another request is a `conflict`. A read carries no key. Disconnect leaves the operation running.

### Watch: durable cursor, fixed window

The cursor is the journal's event sequence, so a reader resumes from the journal after any disconnect or restart. One answer carries at most 256 events. The authority keeps a bounded broadcast of 256 events per job for readers that wait; a reader slower than that gets a `lagged` marker with the sequence to resume from, and reads the gap from the journal. A reader never holds memory past the window on the authority, and the authority never waits on a reader before it commits or hands off.

### Legacy clients

A client API operation of api 1 that names a managed job is refused as `managed_job` (M2); that refusal is the explicit "unsupported for managed jobs" answer of draft P07, and the facade's own fixtures pin that no socket-only operation has a facade form.

### Shared fixtures

The Rust client (`fabric-client`) and the TypeScript SDK test against the same fixtures in `protocol/facade/v1`, under the numeric and version rules of the bridge fixtures (ADR 0021): counters as canonical decimal strings, ULIDs in text, unknown message types and codes refused. A manifest pins the set in both.

## Alternatives considered

1. Expose the existing trusted-peer protocol to clients. Rejected: it carries the wrong authority boundary (draft P07).
2. A public HTTPS listener with its own login in M5. Rejected: it needs a reachability and credential decision the local-resources rule of this campaign excludes; the link already authenticates nodes.
3. Signed capability tokens verified offline. Rejected for M5: a digest lookup on the authority gives immediate revocation without a signature scheme; signing is the open question for principals that are not the owner.
4. A per-reader queue for watch. Rejected: a slow reader would grow the authority's memory; the journal is the queue and the window is fixed.
5. Treat disconnect as cancel. Rejected by draft P07 and ADR 0021: cancel is a command.

## Consequences

- `fabric-net` gains `RequestBody::Facade` and `ResponseBody::Facade` in protocol 4 (unshipped); `fabricd` gains a `facade` module that authenticates, scopes and forwards to the authority, and a bounded broadcast per job for watchers.
- The journal gains the `facade_tokens` table in schema version 3 (docs/managed-jobs.md, "Codec and schema version 3").
- The client API gains `facade_call`, `facade_mint` and `facade_revoke`, the Rust client and the TypeScript SDK their methods, and the CLI `fabric facade` and `fabric managed inspect`.
- A revoked or expired token ends every open watch; a test proves it within one delivery.
- The operation key's scope becomes `(principal, key)` as ADR 0020 foresaw: a facade command's key is stored under the token's actor.

## As built (M5)

Built on 2026-11-03 by owner B. Where the code differs from the decision above, docs/managed-jobs.md, "As built (M5): P06 and P07", says how and why. In short: the facade's parsers live in `fabric_client::facade` and `sdk/typescript/src/facade.ts`, each tested against every case of `protocol/facade/v1`; the authority side (`fabricd::facade`) checks in the order above, stores a command's key as `<actor>:<key>` (or `<actor>:h:<digest>` when the pair is too long for one key), and answers `accepted` after the commit. The watch keeps no per-job broadcast: the journal is the queue, a reader waits on the authority's commit counter and reads again when it moves, so a slow reader costs no memory and an answer carries at most the window with `lagged` when more was behind the cursor; a watch on a finished or cancelled job with nothing after its cursor closes `finished`. The bearer is 32 bytes from the kernel's random device, shown once; a token can be minted before any job exists, which makes the journal. The local socket's `managed_create` keeps its required `repo` (pinned by `protocol/required.txt` under api 1), so a source that is not a repository on the node travels in a new `managed_submit` op with the same body as the facade's `submit`.

## Open questions

- Who may mint a token for another actor, and how a token for a principal that is not the owner is signed and audited.
- Reachability from outside the owner's nodes (a relay, a public listener), and the browser client the TypeScript SDK would need for it.
