# ADR 0023: Managed task lifecycle, containment and the checkpoint cut

Status: proposed 2026-11-03, built 2026-11-03 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request).

Adopts the external draft P03 ("Managed task lifecycle and checkpoint cut", 2026-10-25, Proposed) for the M5 campaign. Draft P03 left five choices open: containment profiles, drain budgets, outcome policies per task kind, parent and child outcomes, and terminal attachment. Each is selected below. The selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe, and Efe can veto any of them at the pull request. The types and signatures are in [docs/managed-jobs.md](../managed-jobs.md), section "Placement and planned continuity (M5)", part "Task lifecycle (P03)". It builds on ADR 0019 (tasks and attempts), ADR 0020 (one authority, grants and generations) and ADR 0022 (provider effects that may still act block a replacement).

## Context

M2 to M4 made a managed job durable and its provider effects safe to lose. What runs an attempt is still the M2 picture: the local node launches the primary harness task, as the standalone path does, under a process group the node records. Three things are missing before a planned handoff can be trusted:

- A task has no behavioral class. The authority knows `Primary` and `Child { label }`, which say who registered a task, not what an interruption means for it. An interrupted test and an interrupted agent turn need different answers.
- Nothing bounds quiescence. A handoff needs a moment where registered work is settled or deliberately cut, and a child that left the group, or a writer the executor does not know, must be visible rather than assumed gone.
- A checkpoint is a snapshot alone. It does not say which tasks it cuts, what state each was in, under which environment, or which evidence it stands on, so a continuation cannot tell a finished test from one the cut interrupted.

## Decision

### Containment profile v1: one process group per task, children registered at spawn

A managed task runs in a process group of its own, created by `fabricd`'s executor when it spawns the task, as the bridge worker has since M3 (ADR 0021). The executor registers every child it spawns, with its pid, group, start time and task. A process found in a task's group that no registered task owns is an escaped child: it is reported, never adopted and never silently killed as part of a handoff. The profile covers macOS and Linux; no other platform is claimed in M5.

The task classes are `build`, `test`, `harness` (an ACP, Claude Code or Codex turn through `fabric-harness`) and `opaque`. The class is a field of the task, named when the task is registered; the primary task is `harness`. The structural kind of ADR 0019 (`Primary`, `Child { label }`) is unchanged and says who registered the task.

### Drain budget: 30 s per task, set per job

Quiescence gives each task a drain budget, 30 s unless the job's policy sets another, between 1 s and 10 min (`JobPolicy.drain_budget_ms`). The budget bounds how long the executor waits for a task to settle on its own; it never bounds the task itself, which runs as long as it runs when no handoff is asked for.

### Outcome policy per class at the end of the budget

| Class | Policy | What is recorded |
| --- | --- | --- |
| `build`, `test` | cancel and restart | the task's receipt says `interrupted`; an interrupted build or test is never recorded passed and never recorded failed |
| `harness` | supported checkpoint | the existing suspend path captures the harness state; if the capture fails, the handoff is blocked with the capture's reason |
| `opaque` | block | the handoff is blocked naming the task; nothing is guessed about what it was doing |

An escaped child or an uncontrolled writer in the workspace (a process outside every registered group that holds a file under the root open for writing, as `lsof` reports it) blocks a strong handoff with a named reason. Neither fails the job on its own: the job waits for the owner's decision, and the executor keeps the attempt running.

### Parent and child

A parent task cannot complete while a registered leaf of it is unsettled. A child's outcome is recorded on the child; the parent's class and policy decide whether the parent continues, blocks or is cut. The executor settles leaves before parents during quiescence, and spawning is frozen for the attempt from the moment quiescence starts.

### Checkpoint cut v1

A checkpoint cut is the existing capture (the bundle chain and the ledger head, through `fabric-store` and the harness adapter's capture) plus what a continuation needs to read it correctly: one receipt per task of the attempt (class, status, exit code, interrupted flag, workspace revision), the environment facts the node measured for the job, and the accepted evidence of the attempt by digest. The cut is one canonical JSON document stored content-addressed in the node's store; its digest is the checkpoint's evidence digest in the journal, and the cut's receipts are recorded in the journal event beside the snapshot, so the authority can answer "what did this checkpoint interrupt" without the store. No process memory is checkpointed; a receipt says what was cut, not how to resume it.

### Terminal attachment: unchanged

The CLI attaches to a task's terminal through the existing path (`fabric run`, the headless launch and hold of M2). The M5 gate is headless: every M5 test runs tasks without a terminal, and no new attachment boundary is built.

## Alternatives considered

1. Kill the process group at quiescence and assume everything stopped. Rejected by draft P03: a detached worker outlives the group, and an interrupted test would look finished.
2. Checkpoint process memory (CRIU, VM snapshots). Rejected: not portable across the native machines the product targets, and a harness session's own files are what a continuation reads.
3. Infer a task's class from its command line (`cargo test`, `npm run build`). Rejected: Fabric never infers a task from a process name (ADR 0019); the class is declared when the task is registered.
4. One drain budget for the whole attempt. Rejected: a build and an agent turn settle on different clocks, and one budget would be either too short for the harness or too long for the test.
5. Make an escaped child fail the job. Rejected: a stray process is a reason to wait for a decision, not evidence that the work failed.

## Consequences

- `Task` gains a class and the journal's `tasks` table a `class` column; `JobPolicy` gains `drain_budget_ms`. Both are carried by codec format byte 2 (docs/managed-jobs.md, "Codec and schema version 3"); every format 1 frame decodes as before and widens to the M2 defaults.
- `fabricd` gains an executor module that owns launch, child registration, supervision, quiescence and receipts for managed attempts. The standalone launch path is unchanged.
- A new event, `CheckpointCutRecorded`, carries the cut's digest and receipts; the checkpoint evidence row carries the cut's digest.
- A handoff can be blocked for reasons that are not failures (`HandoffBlocker`), and the client surfaces must show them as blockers, never as a failed job.
- Interrupted builds and tests are a third outcome beside passed and failed; nothing downstream may fold `interrupted` into either.

## Open questions

- Containment on platforms other than macOS and Linux, and under a provider resource (draft P09), where the group is the worker's and the receipt is the provider's.
- Whether a harness class can ever be cancel-and-restart (a turn that made no tool call), which needs the adapter to say so per turn.

## As built (M5)

Built on 2026-11-03 by owner A. Where the code differs from the decision above, docs/managed-jobs.md, "As built (M5): P03 to P05", says how and why. In short: the task class and the drain budget are as decided; the executor's leaves-before-parents order is the reverse spawn order, since it keeps no parent map; a quiescence never signals a harness task (the `Capturer` trait is the suspend path) and never signals an opaque task; an escaped child is listed by process group through `ps` and an uncontrolled writer through `lsof`; the cut's evidence refs are the receipts by digest; the launch boundaries are the two states of a durable launch record that `Executor::recover` clears. The `CommandAccepted` event now says whether a command has an effect, and an accepted effect is kept as `EffectRequested`, so a cancel that stops a live attempt (new in M5) and a checkpoint that leaves it running are both replayable.
