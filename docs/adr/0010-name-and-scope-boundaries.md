# ADR 0010: Name and scope boundaries

Status: accepted, 2026-10-19

## Context

Two kinds of drift threaten a small project: becoming a hub because hubs are visible, and becoming a scheduler because schedulers are fun. The research names both as occupied or unwanted (`2026-10-19-competitor-landscape.md`, sections 7 and 11). The name also collides.

## Decision

### Name

"Fabric" is the working name of a private repository. It collides with at least:

- Microsoft Fabric (data platform);
- Tether QVAC Fabric (named in the research, section 11, question 9);
- Fabric, the Python SSH task library (`fabfile.py`);
- `danielmiessler/fabric`, an AI prompt framework with a large GitHub following;
- Fabric, the Minecraft mod loader.

Rule: the repository stays private under this name. Before any public release, binary name, crate publish or domain, the project is renamed. The crate prefix `fabric-` and the CLI name `fabric` are placeholders and no code may hard-code them in user-facing strings beyond `clap`'s binary name, which is one constant.

### What Fabric is not

- **Not an MCP server.** Fabric is a library and a CLI. An agent may call the CLI like any other tool, and a thin MCP wrapper could exist outside this repository, but the core never speaks MCP and never depends on an agent being present.
- **Not a hub.** No web UI, no account, no server that must stay awake. T3 Code and bb exist for that and are MIT (section 7, W4).
- **Not a scheduler across vendor clouds.** Fabric does not provision E2B, Vercel sandboxes, Codex Cloud or anything with a billing relationship. A cloud VM the user already runs is a node like any other once it is paired; Fabric does not create it.
- **Not a process migrator.** No CRIU, no VM save and restore in v0. The unit moved is the job's state, not a process image (ADR 0001).
- **Not a workspace definition format.** `EnvSpec` is small and internal; `devcontainer.json` is the import source (ADR 0007).
- **Not a mesh network.** Own QUIC with pairing, no relay, no NAT traversal in v0 (ADR 0005).

### Phone and cloud nodes: later

The product vision includes starting, watching, approving and stopping a job from a phone, and running GPU-bound work on a paired GPU VM. Both are nodes in the model already: a phone is a node that holds a ledger replica and never owns a job; a GPU VM is a node with a `Capability` placement input. Neither is in v0. The gate is two Macs. A phone client needs a transport that works from a phone (likely TCP+TLS over a tailnet first) and a read-only ledger view; a GPU node needs `EnvSpec.requires` and placement inputs. Each gets an ADR when it starts.

## Alternatives considered

1. **Rename now.** Costs a day and a bikeshed while nothing is public. Deferred to the public-release gate, with the rule above so nothing accumulates against the old name.
2. **Ship an MCP server because agents are the users.** The research shows every vendor already has a phone and cloud path, and an MCP server would bind Fabric to the agent's lifecycle, which is the opposite of the thesis. Rejected.
3. **Build the phone client early for demo value.** It would be built on a lease and snapshot layer that has not passed a crash test. Rejected until the gate passes.

## Consequences

- Scope questions get answered by this page before they get a design.
- The repository can stay named `fabric` without anyone mistaking that for a decision.
- Every "later" item here has a named ADR trigger: a real job that needs it.

## Open questions

- Candidate names: none chosen. Collect, do not decide, until the gate passes.
- Does the phone client belong in this repository (Rust core compiled for iOS) or in another? Decide when it starts.
