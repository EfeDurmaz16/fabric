# ADR 0001: The job is the unit of work

Status: accepted, 2026-10-19

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
| Compute | A different node claims the lease | The snapshot contents, the EnvSpec |
| Environment | The EnvSpec is edited or re-detected | The ledger, the harness state |
| Harness | `fabric resume <job> --harness codex` | Storage, compute, environment |

The user never answers "which folder, which machine, which settings, which environment". Policy answers (ADR 0008). The user answers "which job" and, when they care, "which harness" and "which node".

Fabric is a library and a CLI. It is not an MCP server and not a hub with a web UI (ADR 0010).

## Alternatives considered

1. **A session mover.** Move a Claude Code session from one host to another and stop there. This is what cmux 0.65 and claude-handoff 0.3.0 already do for Claude only (landscape section 7, W1). It binds the unit of work to one vendor's session format, so `--harness codex` is impossible. Rejected as the core model; kept as one adapter inside it.
2. **A hub that owns environments**, like T3 Code or bb. Both are MIT, both are active, both need the hub awake and neither moves a thread between hosts (landscape section 7, W4). Building a third hub competes with two that already have users. Rejected. If a hub integration is wanted later it is an adapter, not the model.
3. **Workspace definition first** (a `devcontainer.json` superset). The environment is one of four parts, not the whole. `devcontainer.json` is an import source for `EnvSpec` later (ADR 0007). Rejected as the unit.
4. **Process checkpoint** (CRIU, VM save and restore). On macOS no developer runtime exposes it (landscape W5). It also moves the symptom (a process image) rather than the thing the user cares about (the job). Rejected for now; it could become a snapshot kind on Linux later.

## Consequences

- Everything in the core is addressed by job id, snapshot id, chunk id or node id. No host paths in any persisted structure except inside opaque harness state, which the adapter rewrites on restore.
- Moving a job is: suspend here, resume there. Both verbs are first class and both are idempotent.
- The schema for "what a harness needs to continue" is not designed up front. Each adapter declares it and the core stores it as bytes (ADR 0006).
- A job can outlive every machine it has run on, as long as one node still holds its ledger and latest snapshot.

## Open questions

- Should a job hold one workspace or several (monorepo with two checkouts)? v0: one. Revisit when a real job needs two.
- Is "goal" free text forever, or does it grow structure (pending decisions, approvals)? v0: free text plus `DecisionRecorded` events. Structure comes from harness needs, not design.
