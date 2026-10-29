# ADR 0022: Reconciliation of provider operations and cleanup authorization

Status: proposed, 2026-10-29.

Adopts the external draft P11 ("Unknown outcomes, reconciliation and cleanup authorization", 2026-10-27, Proposed) with its handoff selection of the same date, for the M4 campaign. It builds on ADR 0020 (the journal records intent before any effect and leaves an unobserved effect `unknown`) and ADR 0021 (the bridge, its `provider_operations` table and protocol v1). The types, the schema, the state machine and the rules are in [docs/managed-jobs.md](../managed-jobs.md), section "Reconciliation and cleanup (M4)"; the wire additions are in [docs/bridge-protocol.md](../bridge-protocol.md), section "Reconciliation (M4)", and in [protocol/bridge/v1](../../protocol/bridge/v1).

## Context

M3 made every provider effect safe to lose: the journal commits a provider operation `dispatched` before its frame is written, a lost worker or a passed deadline leaves it `unknown`, nothing the journal holds is sent twice, and `observe` by the original operation can settle it again when Fabric knows its locator. Three things are still missing for an attempt to recover:

- A create whose answer never reached Fabric, from a worker lost before any answer or an authority restart before the answer was read, has no locator. `observe` needs one, so the operation stays `unknown` with no way back, and nothing in the journal stops a runner from creating a second resource for the same purpose.
- An `unknown` operation has no explicit exit. Fabric can neither say "the effect is known to exist but is not finished" nor let the owner give up on it in a recorded way.
- Nothing ties a replacement attempt to the fate of the previous attempt's effects. `StartAttempt` checks only that no attempt is live, so a destroy that failed or never answered does not stop a second execution from starting beside a first one that may still run. And once an attempt has ended or the job is closed, the journal refuses every provider operation, so the resources the job made cannot be cleaned up at all.

Draft P11 leaves five choices open. This ADR selects each one.

## Decision

### The pre-dispatch intent record is the M3 row, made complete

The durable intent is the `provider_operations` row that M3 commits before the frame. It is not split into an `intent` status followed by `dispatched`: that split would add a committed write, and an fsync, to every effect, and all it would buy is turning some `unknown` operations after a crash between the commit and the write into "never sent", which attach and observe resolve as well. A crash between the commit and the frame therefore leaves the operation `unknown`, which is the truthful state: the effect may or may not have happened.

The row gains what reconciliation needs to act on it without reading anything but the journal: the profile it was dispatched under, its effect class from that profile (`create`, `mutate`, `destroy`), its purpose (an effect of the attempt, or a cleanup of a named create), a resource key for its target and its reported locator, the scope an absent attach searched, and the owner's abandonment reason. The resource key is a digest of the locator's identity computed by the bridge, so the journal compares resources without parsing provider names.

Schema version 2 has never shipped (ADR 0020), so these columns and statuses amend the version 2 table in place, under the rule Efe set on 2026-10-29 for protocol v1: a version that never shipped takes corrections itself. The version 2 fixture database is regenerated with them.

### Reconciliation reads; it never repeats an effect

An operation that is `unknown` can only be read: by `observe` when Fabric holds its locator (M3), and by the new `attach` when it does not. `attach` asks the worker for the resource an earlier create of this operation made, found by the operation's identity, and never creates anything. A profile supports `attach` for exactly the actions whose effect is `create` and whose `dedupe` is `provider_key` or `label_query`: the dedupe statement is already the profile's verified claim that the worker can find a resource by the operation that made it after a restart. In v1 that is `sandbox.create` in both the fixture and the Docker profile. No profile can attach to a running `sandbox.exec`: no exec handle survives the worker, and a Docker exec instance cannot be attached again once its client ended.

An attach that finds nothing answers `unknown` with the reason `absent` and the scope it searched. Absence never settles an operation, because a create may still land (a Docker create runs in a client of its own that outlives the worker) and a worker bound to another scope finds nothing. Fabric never resends a create, under any profile's dedupe.

These are additions to protocol v1, which has not shipped either: one message (`attach`), two reasons (`unsupported`/`attach`, `unknown`/`absent`), one error field (`searched`), a fixture seed state and a Fabric-only scenario header, with fixtures in both repositories and both manifest pins moved together.

### One state machine with explicit exits

A provider operation is in one of: `dispatched`, `unknown`, `attached` (an attach found its resource; not yet terminal), `observed` (a read after `unknown` saw its effect in a non-terminal state), settled (`succeeded`, `failed`, `cancelled`, or `rejected` before any effect), or `abandoned`. `attached`, `observed` and `abandoned` are new. Restart recovery still turns only `dispatched` into `unknown`; the new states hold no request in flight and survive a restart as they are.

`abandoned` is entered only with the owner's approval, from `unknown`, `attached` or `observed`. The approval names the operation, the status the owner saw and what the owner accepts may remain (the locator the row holds, the scope an absent attach searched, or nothing); the journal refuses it when the row no longer matches, so an approval never applies to a state the owner did not see. Abandoning records a decision. It never claims that the effect did not happen, that a resource is gone or that an execution ended.

### A replacement waits for the previous attempt's effects

`StartAttempt` is refused with `ReplacementBlocked` while any provider operation of the job may still act:

- a destroy that has not settled `succeeded`, so `dispatched`, `unknown`, `observed`, `failed`, `cancelled` or `abandoned`, unless a destroy of the same resource has settled `succeeded`;
- an exec that has not settled, `abandoned` included, unless its sandbox has been destroyed by a destroy that settled `succeeded`;
- a create that is `dispatched` or `unknown`, whose resource no one can name yet.

A create that is `attached`, `observed`, settled or abandoned does not block: its resource is named or given up, and an idle resource runs nothing. Abandoning a destroy or an exec never unblocks a replacement, since neither proves that anything stopped; destroying the sandbox does. Within an attempt, a create is refused while the job holds a create `unknown`, so a lost answer is attached or abandoned before another resource is made.

### Cleanup needs a fresh approval every time and outlives the job

A cleanup is a `sandbox.destroy` of a resource that a create of the same job made, recorded with purpose `cleanup` and the create it cleans. It differs from an ordinary effect in two ways. Its fencing: it is allowed after the attempt ended and after the job finished or was cancelled, and it carries the creating operation's own scope, so the worker's owner check still holds. Its approval: every cleanup call carries an approval reference the caller supplies for that call, which the worker resolves to a grant scoped to `sandbox.destroy` on that resource. Fabric stores no approval reference, so it cannot reuse one; a create grant never authorizes a destroy, and an expired grant authorizes nothing (both already refused by the worker in v1). Fabric never starts a cleanup by itself. When no authorized cleanup happens, `residuals` lists precisely what the job's creates may have left: each named resource, and each create whose resource no one can name, with the scope an absent attach searched.

## Alternatives considered

1. Retry a create whose answer was lost when the profile deduplicates on the key. Rejected: Fabric would trust a dedupe window and a parameter-conflict rule it has not verified per provider, and a retry is an effect where attach is a read.
2. A separate `intent` status before `dispatched`. Rejected above: one more fsync per effect for a distinction reconciliation already makes.
3. An explicit `attach` flag per action in the capability profile. Rejected: it restates `dedupe` and would rewrite every handshake fixture of v1; the profile `version` stays 1.
4. Let abandonment of a destroy unblock a replacement. Rejected: it would let the owner assert termination without evidence, which draft P02 and ADR 0020 forbid before real fencing proof.
5. Automatic cleanup when a job is cancelled. Rejected: no approval can be issued automatically (P11 handoff selection), and a destroy under a stale grant is exactly what fresh approval prevents.
6. Store provider transitions as events in the hash-chained `events` table. Not now: provider operations are rows since ADR 0021, and moving them into the event history is a separate decision with its own replay cost.

## Consequences

- No blind duplicate creation: a lost create is attached or stays explicitly `unknown`, and neither a retry nor a replacement starts beside it without the owner's recorded decision.
- Unknown stays explicit: `unknown`, `attached`, `observed` and `abandoned` are distinct statuses a client must show as they are.
- A failed or unanswered stop blocks a replacement, at the cost that a provider that cannot destroy keeps the task waiting until a destroy succeeds.
- No new transaction or fsync per effect. A create's begin reads one more index inside its transaction; `StartAttempt` evaluates the barrier inside its transaction; attach is a new read path. The perf phase measures them against the anchor.
- The worker gains one read-only message, the fixture provider a seed state (`starting`), and both scenario runners a header (`journal_state`) that seeds Fabric's journal.
- Control surfaces are out of M4: `fabricd` does not drive the bridge yet, so attach, cleanup, abandonment and the residual list are library calls of `fabric-bridge` and `fabric-journal`, and the client API and the CLI gain them when placement puts attempts on providers (M5).

## Open questions

- Whether a provider whose dedupe window and parameter-conflict rule are verified may ever retry a lost create instead of attaching. Not needed by either v1 profile.
- How abandonment and cleanup approvals gain a principal and an expiry once remote principals exist (draft P07).
