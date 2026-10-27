# ADR 0019: Managed jobs, tasks, attempts and operations

Status: proposed, 2026-10-27

Adopts the parts of the external draft P01 ("Durable job, task, attempt and evidence", 2026-10-25) that the M2 campaign needs, with the handoff selections of 2026-10-27. It extends ADR 0001 (job as unit of work) and ADR 0002 (job ledger and record) without changing either: a job made before this ADR, and every job made without asking for a managed one, stays a standalone job under those ADRs, byte for byte. The schema, the states and the replay rules live in [docs/managed-jobs.md](../managed-jobs.md); this ADR records the decisions and why.

## Context

A standalone job today is a `JobId`, a hash-chained ledger and a lease owned by one node at an epoch (ADRs 0001 to 0003). It survives a cooperative move. It does not separate three things a durable job needs to keep apart:

- the work the user asked for, which must outlive any one machine or process;
- one logical piece of that work, such as the primary agent turn or a test run, which may need several tries;
- one concrete try on one resource, which can end, fail, be lost or come back with an unknown result.

Without that split, a process that disappears either erases the job's progress or is mistaken for a finished one, and a client that lost an answer cannot retry a command without risking a second effect. The audit of 2026-10-25 (`fabric-audit.md`, section 6, and the client API findings of its annex) found no operation identity and no idempotency key on any mutating call.

## Decision

### Four identities, four meanings

- `JobId` (existing ULID): the durable work. A managed job keeps its `JobId` across every task, attempt and authority restart.
- `TaskId` (new ULID): one logical piece of a job's work. M2 has exactly one kind that runs: the primary harness task every managed job gets at creation. Child tasks are registered explicitly by a command; Fabric never infers a task from a process name.
- `AttemptId` (new ULID): one concrete execution of one task on one resource, under one fencing generation. A retry is a new attempt with a new `AttemptId`; an attempt id is never reused.
- `OperationId` (new ULID): one accepted command, made by the authority when it commits the command. A client names its command with an `OperationKey` it chooses; the authority deduplicates on that key.

A `WorkRevision` counts accepted changes to one managed job, starting at 1 when the job is created. A `Generation` is the fencing generation of the job's canonical execution domain (ADR 0020). Neither is an epoch: the epoch of ADR 0003 stays the standalone lease counter and is never written for a managed job.

### The mode is explicit

Every job has one mode, `standalone` or `managed`, fixed when it is made. A managed job lives in the managed journal (ADR 0020) and has no ledger file; a standalone job has a ledger file and no journal rows. A command for one mode refuses a job of the other with a typed refusal. There is no silent fallback from managed to standalone, and M2 has no conversion either way: promotion of a standalone job is the separately gated decision of draft P08 and is not part of M2.

### Idempotent commands

Every mutating command on a managed job carries an `OperationKey` from the caller and, except creation, the `WorkRevision` the caller expects. The authority computes a `RequestDigest` over the command's canonical bytes. In one journal transaction it:

1. looks the key up; an existing key with the same digest returns the stored result and commits nothing new; the same key with a different digest is refused as `OperationConflict`;
2. checks the expected revision; a mismatch is stored as a rejected operation under the key, so a retry gets the same refusal;
3. appends the command's events, updates the projections, stores the operation and its result, and commits.

The answer is sent only after the commit returns. A client that loses the answer sends the same key and command again and gets the same result, never a second effect.

### Outcomes stay distinguishable

An attempt ends as `succeeded`, `failed`, `cancelled`, `checkpointed`, `lost` or `unknown`, and a task never becomes completed because an attempt stopped. A lost worker is `lost`, not `cancelled`. A timeout leaves an operation `unknown`, which is a valid state; it is not permission to replay the operation or to start a replacement attempt (ADR 0020).

### Evidence is a reference, not a claim

Evidence rows name the revision, the attempt and the artifact (a snapshot id, a test result digest, a log object). Text an agent printed is not evidence that a test passed. M2 records evidence references for checkpoints and attempt outcomes only; richer evidence kinds wait for the campaigns that produce them.

### Purity

The types, the transition function and the replay function live in `fabric-core` and do no IO and read no clock: ids, times and observations enter as arguments from the journal adapter (ADR 0009).

## Alternatives considered

1. Add fields to the existing ledger records. Rejected: every new field is a new record format version, the ledger's hash chain would mix managed and standalone rules, and old readers would either refuse or, worse, read a managed job as standalone. Draft P08 says old bytes are never rewritten.
2. A goal plus a vendor thread id. Smaller, but a retry or a child task has nowhere to live, and the job's truth becomes the vendor's.
3. A general workflow DAG engine. More than the primary task, the registered children and quiescence need.
4. Let the harness planner own task state. Ties job truth to one vendor and to unverified generated state.

## Consequences

- A second kind of job exists. `fabric job ls` and the client API show the mode; every existing command keeps its meaning for standalone jobs.
- Clients get a retry rule they can follow: same key, same command, same answer.
- `fabric-core` grows a `managed` module; its codec rules (canonical bytes, exact decode) are those of ADR 0002.
- The control API grows managed ops in a new API version (ADR 0018); the fixtures of the current version do not change.

## Open questions

- The bounds of the task graph and the retention of evidence, once child tasks run.
- The canonical publication model for parallel writers (draft P01): M2 has one canonical execution domain per job and no parallel writer.
- The exact names of the remote API, which waits for draft P07 and its authentication decision.
