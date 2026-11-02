# ADR 0028: Capability-gated unexpected-loss recovery

Status: proposed, 2026-11-02; implemented on the M6 branch on 2026-11-03 against the M4 base (docs/managed-jobs.md, "As built (M6)" records where the code differs and what waits for the M5 executor). Every selection below was selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02 ("kararları sen verebilirsin"). None was approved by Efe; he can veto any of them at the PR.

Adopts the M6 row of the roadmap ("Capability-gated unexpected-loss recovery") and the parts of the external drafts P02 ("Independent authority and fencing", 2026-10-25) and P03 ("Managed task lifecycle and checkpoint cut", 2026-10-25) that M5 left open: what makes a checkpoint independent of the runner that made it, which writes the authority fences, and what happens when a runner stops answering. It builds on ADR 0020 (one authority, one generation per grant, a report at an older generation refused), ADR 0022 (the replacement barrier) and the M5 decisions of ADR 0023 to 0027 (task kinds and outcome policies, the checkpoint cut, placement of registered nodes, the facade protocol). The types, the rules and the proof harness are in [docs/managed-jobs.md](../managed-jobs.md), section "Unexpected-loss recovery (M6)".

## Context

After M5 a managed job can be placed on a registered node without naming it and handed over on purpose: the runner drains its tasks, captures a checkpoint, the authority grants the next attempt once the bytes are verified and the old writers are stopped. What M5 does not cover is the runner that stops answering without anyone asking it to: a laptop lid, a dead network, a killed process. Four things are missing for that case:

- A checkpoint the runner captured says nothing about where its bytes are. `Observation::Checkpointed` records a snapshot id; the bytes may exist only in the lost runner's store. P03 names the distinction (local capture is not independent storage acknowledgement) and nothing in the journal records it.
- The generation fences the journal alone. `apply_observation` refuses a report at an older generation, and `provider_begin` refuses an effect from an attempt that is not the current grant, but a bundle published into the authority's store, a record appended to a job ledger and the outcome of a facade command carry no generation and are checked by nothing.
- A runner that stops answering has no state. `Observation::Lost` is a verified stop (replay rule 4: the process group is gone), and the authority's probe can only verify its own host. A silent peer is neither `lost` nor `running`, and the M2 wait rule keeps every new grant behind an attempt that `may_run`, so the job waits forever with no recorded reason.
- There is no policy for what the authority may do on its own. P02 forbids automatic failover without verified termination or enforceable fencing of every covered mutation; it does not say which jobs may opt in once the fencing exists, or what the default is.

## Decision

### A checkpoint is acknowledged only when the authority holds and verified its bytes

A checkpoint has two states that the journal keeps apart. It is captured when the runner published it into its own store and reported `Observation::Captured` (a checkpoint taken while the attempt runs; `Checkpointed` stays the M2 report of an attempt that ended at one). It is acknowledged when the authority node has fetched every manifest and chunk of its bundle chain through the existing store transfer (`fetch_snapshot`, which keeps a chunk only when its bytes hash to the id it asked for and publishes a manifest only after every object it names is on disk, ADR 0014), checked that the published id equals the reported id, checked the cut's digest against the receipts, facts and evidence refs the cut names (ADR 0024), and committed a `CheckpointAcknowledged` event with what it verified. Only an acknowledged checkpoint is a restart point. A captured checkpoint that is never acknowledged is listed as such and restarts nothing.

The acknowledgement is written to the journal, not to the store: the store's row says the bytes are complete on this node, the journal's event says the authority accepted them as the job's checkpoint under the generation that made them. The two are different claims and the second one is what recovery reads.

### Every covered write carries the grant and is checked against the live grant

A grant token is the triple `(job, attempt, generation)`. The covered write set is exactly four paths, named in the specification and asserted by a test that lists them: a ledger append for a managed job, a bundle publish accepted as the job's checkpoint, a journal operation (every observation, acknowledgement and operation mark an executor reports) and the outcome of a facade command. Each path's entry function takes a grant token and runs one rule, `fence`, before it writes anything: the token's attempt must be the job's live attempt at the token's generation, in status `granted` or `running`, and, when the write comes over the node link, from the node the attempt was granted to. Any other verdict refuses the write, records it (`StaleObservationRefused` for a report, `CoveredWriteRefused` for the other three paths) and changes nothing else.

The harness's own files inside the workspace are not covered. A superseded runner that is alive can keep editing its working tree, its git refs and its harness state directory until its executor stops it, and nothing in Fabric prevents that: those bytes reach the job only through a bundle publish, which is covered. The ADR says this plainly so no one reads "fenced" as "the old machine cannot write".

### Loss is an availability observation, never a stop

An executor sends a heartbeat for every live attempt it runs, every 2 s, over the node link. A heartbeat is not a journal write: the authority keeps the last one in memory and writes an event only when the state changes. An attempt whose heartbeat has been missing for 10 s gets `Observation::ResourceUnavailable`, which moves it to `unknown`, the M2 status for "the authority cannot tell". A heartbeat that returns before anything else happened moves it back to `running` with `ResourceReachable`. Neither says the old attempt stopped, and `unknown` still `may_run`, so the M2 wait rule holds unchanged: nothing is granted behind it by `StartAttempt`.

What the heartbeat absence enables is one new command, `Recover`, which in one transaction supersedes the old grant (`GrantSuperseded`, status `superseded`, which does not `may_run`) and grants the next attempt at the next generation from an acknowledged checkpoint. The old attempt may be alive while this happens. That is safe because of the previous section: every covered write it makes from then on names a generation that is not the job's live grant and is refused, and its executor stops the attempt's process group when it reads a `superseded` verdict. A `Recover` is refused by the M4 replacement barrier like a `StartAttempt`, so a provider effect of the old attempt that may still act blocks it.

`Recover`, and `Supersede` alone, take only an attempt that is `unknown`: a reachable attempt is handed over by the M5 planned handoff and never recovered (`AttemptReachable`). The heartbeat's answer carries the fence verdict of every token it named, so a reachable runner learns within one interval that an attempt of its was superseded and stops it.

The two mechanisms are separate on purpose. Fencing is what keeps the job correct; the heartbeat only decides when the authority is allowed to act.

### Recovery policy per job, default block

`JobPolicy` gains `recovery: RecoveryPolicy`, with two values. `block`, the default, means the authority records the unavailability and waits: the job shows the unavailable attempt, its last heartbeat, its acknowledged checkpoints and its blockers, and an operator issues `Recover` (or `Supersede` without a restart) when they decide to. `restart_from_acknowledged` lets the authority issue `Recover` itself once the attempt has been unavailable for the threshold, and only when every condition holds: the task's kind is one whose outcome policy is cancel-and-restart (build, test) or harness (ADR 0023); for harness there is an acknowledged checkpoint of the task; the replacement barrier is clear; M5 placement finds an eligible node, with the unavailable node excluded for this placement. When a condition fails the job falls back to `block` with the failed condition recorded as the reason (`RecoveryBlocked`), never to a weaker restart. An `opaque` task is never restarted automatically, and a job whose primary task is opaque cannot be created with `restart_from_acknowledged` (refused at admission, `RecoveryUnsupported`).

Nothing resumes under an old generation. A restart is a new attempt, a new generation, placed by the M5 rules, from the newest acknowledged checkpoint; the fidelity it achieves is recorded as ADR 0024 says.

### The schema change is a version 2 amendment with the previous shape kept as a fixture

The journal gains two event kinds (`checkpoint_acknowledged`, `covered_write_refused`, plus the recovery kinds listed in the specification), which need no DDL, and three projection changes that do: the attempt status `superseded`, the evidence kind `checkpoint_ack`, and two projection columns on `attempts`. Version 2 has never shipped (ADR 0020, ADR 0022), so these amend it in place under the rule Efe set on 2026-10-29. The shape before M6 is kept as `tests/fixtures/v2-pre-m6.sqlite`; `open` refuses it as `DraftSchema`, naming the amendment, and writes nothing, as M4 did for M3's draft. `JobPolicy` gains its field under the codec rule of ADR 0020: a frame of format 1 decodes with `recovery: Block`.

### Proof is local and real, and says what it is

The acceptance of M6 is a real partition and an old-runner wake. The proof runs two `fabricd` processes on one host with separate state roots, paired through a small userspace relay the test owns, which forwards UDP datagrams and TCP bytes between two loopback ports and can drop everything (pause) and forward again (resume). The runner is the node behind the relay; the authority is the other, and is also the only eligible node for the replacement. Three scenarios: the relay paused with the runner alive; the runner stopped with `SIGSTOP` and woken with `SIGCONT` after the authority has regranted; the runner killed with `SIGKILL` and started again after the regrant. In each the evidence is the event list (unavailable, superseded, granted at the next generation, then the old runner's refused writes), the authority's checkpoint head unchanged by anything the old runner sent after the supersede, and the old runner's process group gone within the stop bound. The evidence note states that this is a two-process partition on one host, not two hosts, and that no firewall or system setting was touched.

## Alternatives considered

1. Treat a missed heartbeat as a stop and mark the attempt `lost`. Rejected: P02 and ADR 0020 forbid it, and the SIGSTOP scenario shows exactly the runner that comes back and writes.
2. Fence with a lease the runner must renew (a time-bound grant). Rejected for M6: it depends on the two clocks, and a paused process holds a lease it cannot know expired. The generation on every covered write needs no clock.
3. Acknowledge a checkpoint when the runner's store publish succeeded. Rejected: that is local capture, the thing P03 says is not independent storage.
4. Make `restart_from_acknowledged` the default. Rejected: deny by default; the restart moves work on the authority's own initiative, and the operator opts in per job.
5. Cover the harness's workspace writes with a tombstone, as standalone moves do. Rejected for M6: a native tombstone stops the harness's own picker, not a running process; the executor's stop on a `superseded` verdict is the bound, and the workspace bytes stay uncovered and documented as such.
6. Record heartbeats in the journal. Rejected: one fsync every 2 s per attempt for no replay value; only the transitions are history.
7. Migrate the pre-M6 version 2 shape in place instead of refusing it. Rejected: SQLite cannot alter a CHECK constraint without rebuilding the table, and version 2 has no release to migrate from.

## Consequences

- An operator can tell, from the journal alone, which checkpoints the authority holds and which only the runner held.
- A covered write from a runner that lost its grant is refused and recorded, whether the runner knows it lost it or not. The workspace's own files are not covered, and the specification says so.
- A silent runner leaves its job `unknown` with a recorded reason, and under `block` nothing moves until someone decides. Under `restart_from_acknowledged` the authority moves the work itself, only within the conditions above, and records why it did or did not.
- One more command (`Recover`, and `Supersede` as its half), one more policy field, two heartbeat messages on the node link and one more report (`Captured`), two facade commands on the unshipped M5 facade protocol, a version 2 amendment with its fixture, and a test relay. No new dependency.
- The executor stops an attempt's process group when a covered write or a heartbeat of it is answered `superseded` or `stale`. A runner that is partitioned is never told; its attempt stays `superseded` in the journal and its process keeps running until the partition ends (its next heartbeat or covered write) or the node's own recovery on restart reads the verdict.

## Open questions

- The heartbeat interval and the unavailability threshold are constants in M6 (2 s and 10 s). Whether they become per-job policy waits for a job that needs it.
- Automatic checkpoint cadence while an attempt runs. M6 acknowledges the checkpoints M5 takes (the facade `checkpoint` command and the harness drain); a timer that takes them is a later gate.
- How a superseded runner that is partitioned for a long time is told, other than by its next covered write or its restart.
