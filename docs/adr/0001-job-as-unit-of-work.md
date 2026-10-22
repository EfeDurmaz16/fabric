# ADR 0001: The job is the unit of work

Status: accepted, 2026-10-19

Amended 2026-10-19 after step 1 research. Two sentences changed to match ADR 0003 (no arbiter, `Created` makes the creator the owner at epoch 1) and ADR 0011 (`--new` roots live at `~/fabric/jobs/<job-id>/work`). Nothing else changed.

## Context

Every tool in the space binds work to a machine. In T3 Code the database is the environment: one `t3 serve` process owns its SQLite file, its checkouts, its provider logins, and nothing replicates any of it (research: `2026-10-19-t3code-vs-fabric.md`, section 1). Claude Code keys a resumable session to a working directory on one host (claude-code issues #5768 and #28745, cited in the same note). Codex handoff moves a chat between a laptop and a connected host but the host must stay awake. Every vendor's answer to "my laptop is going to sleep" is "keep it awake" (`2026-10-19-competitor-landscape.md`, section 7, W2).

The result is that a developer's unit of work is secretly a tuple of (machine, folder, agent, settings). Changing any one of the four loses the others.

## Decision

The unit of work is a job. A job is:

- a goal, in the user's words;
- an append-only ledger of what happened (ADR 0002);
- zero or more content-addressed snapshots of the workspace and harness state (ADR 0004);
- an `EnvSpec` describing what the job needs to run (ADR 0007);
- a lease naming which node may run it right now (ADR 0003).

A job is identified by a ULID. It is not identified by a path, a hostname, a vendor session id or a git branch. Those are all properties a job has at some moment on some node; they change when the job moves.

Four things about a job are decoupled and may change independently:

| Part | Changes when | Must not affect |
|---|---|---|
| Storage | A snapshot is published or fetched | Which node runs, which harness drives |
| Compute | The job is moved to a different node | The snapshot contents, the EnvSpec |
| Environment | The EnvSpec is edited or re-detected | The ledger, the harness state |
| Harness | `fabric resume <job> --harness codex` | Storage, compute, environment |

The user never answers "which folder, which machine, which settings, which environment". Policy answers where it can and says why (ADR 0008, ADR 0011). The user answers "which job" and, when they care, "which harness" and "which node".

Fabric is a library and a CLI. It is not an MCP server and not a hub with a web UI (ADR 0010).

### How a job starts

A job is born where the work already is. `fabric job init [--goal <text>]`, run inside a directory, adopts that directory as the job's workspace:

1. It refuses if the directory is inside another job's root, is a linked git worktree (ADR 0004, `GitLinkedWorktree`), or is already named by a marker for another job.
2. It records the directory's absolute path as the job's root in the node-local marker (ADR 0004), with state `Dirty` and no base: the directory holds work that no snapshot has yet, which is the truth.
3. It runs each harness adapter's `detect` (ADR 0006). A session found for this directory becomes the job's harness; a running one is recorded as `External` in the harness record (ADR 0003) and is supervised from then on by the node process if one runs. Nothing is stopped and nothing is moved: the user keeps working in the terminal they already have. `suspend` runs `detect` again, so a session started after init is found then.
4. It appends `Created { goal, harness, created_by }` at epoch 1, which makes this node the owner (ADR 0003), and appends `Resumed`. The job is live here. The lease has no expiry, so nothing needs renewing; `fabric suspend` produces the first snapshot and `fabric job move` is what hands the job to another node.
5. It detects and writes the first `EnvSpec` (ADR 0007) and prints it.

The directory is not moved, cloned or symlinked. Moving it breaks the harness's own key for the session, since Claude Code keys sessions by the real path (research H1, section 1), and a symlink breaks it the same way. The first snapshot records the path as `SnapshotManifest.root` (ADR 0004), and every later restore on every node uses that path (ADR 0011). This is why the first gate, a job started under Claude Code in an ordinary checkout, needs no step the user would notice beyond `fabric job init`.

`fabric job init --new [--goal <text>]` is the other way in: it creates an empty `~/fabric/jobs/<job-id>/work`, which becomes the root (ADR 0011), and runs the same steps there. Either form refuses a root under a synced folder (ADR 0004, Where a root may live). Both forms are step 0 of the data flow on the architecture page.

## Alternatives considered

1. **A session mover.** Move a Claude Code session from one host to another and stop there. This is what cmux 0.65 and claude-handoff 0.3.0 already do for Claude only (landscape section 7, W1). It binds the unit of work to one vendor's session format, so `--harness codex` is impossible. Rejected as the core model; kept as one adapter inside it.
2. **A hub that owns environments**, like T3 Code or bb. Both are MIT, both are active, both need the hub awake and neither moves a thread between hosts (landscape section 7, W4). Building a third hub competes with two that already have users. Rejected. If a hub integration is wanted later it is an adapter, not the model.
3. **Workspace definition first** (a `devcontainer.json` superset). The environment is one of four parts, not the whole. `devcontainer.json` is an import source for `EnvSpec` later (ADR 0007). Rejected as the unit.
4. **Process checkpoint** (CRIU, VM save and restore). On macOS no developer runtime exposes it (landscape W5). It also moves the symptom (a process image) rather than the thing the user cares about (the job). Rejected for now; it could become a snapshot kind on Linux later.

## Consequences

- Everything in the core is addressed by job id, snapshot id, chunk id or node id. One host path is persisted on purpose: `SnapshotManifest.root`, the directory the job was captured in and is restored to (ADR 0004, ADR 0011). Host paths inside opaque harness state are the adapter's to own on restore. There are no others.
- Moving a job is: suspend here, resume there. Both verbs are first class and both are idempotent.
- The schema for "what a harness needs to continue" is not designed up front. Each adapter declares it and the core stores it as bytes (ADR 0006).
- A job can outlive every machine it has run on, as long as one node still holds its ledger and latest snapshot.

## Open questions

- Should a job hold one workspace or several (monorepo with two checkouts)? v0: one. Revisit when a real job needs two.
- Is "goal" free text forever, or does it grow structure (pending decisions, approvals)? v0: free text plus `DecisionRecorded` events. Structure comes from harness needs, not design.
