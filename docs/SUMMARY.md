# Summary

[Overview](README.md)

# Start here

- [Install](start/install.md)
- [First move](start/first-move.md)
- [What moves and what does not](start/what-moves.md)

# Concepts

- [Job](concepts/job.md)
- [Snapshot](concepts/snapshot.md)
- [Ledger](concepts/ledger.md)
- [Node](concepts/node.md)
- [Authority](concepts/authority.md)
- [Managed job](concepts/managed-job.md)
- [Attempt](concepts/attempt.md)
- [Fence](concepts/fence.md)
- [Glossary](glossary.md)

# Guides

- [Two Macs](guides/two-macs.md)
- [Headless turns](guides/headless.md)
- [Managed jobs](guides/managed-jobs.md)

# Reference

- [CLI](reference/cli.md)
- [Client API](reference/client-api.md)
- [TypeScript SDK](reference/ts-sdk.md)
- [Protocols](reference/protocols.md)
  - [Architecture](architecture.md)
  - [Network protocol](net.md)
  - [Managed jobs and the journal](managed-jobs.md)
  - [Provider bridge](bridge-protocol.md)
- [Evidence](reference/evidence.md)

# Decisions

- [ADR index](adr/README.md)
<!-- adr:begin -->
  - [ADR 0001: The job is the unit of work](adr/0001-job-as-unit-of-work.md)
  - [ADR 0002: Job ledger and record](adr/0002-job-ledger-and-record.md)
  - [ADR 0003: Exactly one live copy](adr/0003-exactly-one-live-copy.md)
  - [ADR 0004: Content-addressed snapshots](adr/0004-content-addressed-snapshots.md)
  - [ADR 0005: Transport and node identity](adr/0005-transport-and-node-identity.md)
  - [ADR 0006: Harness adapters](adr/0006-harness-adapters.md)
  - [ADR 0007: Environment spec](adr/0007-environment-spec.md)
  - [ADR 0008: Placement and triggers](adr/0008-placement-and-triggers.md)
  - [ADR 0009: Engineering conventions](adr/0009-engineering-conventions.md)
  - [ADR 0010: Name and scope boundaries](adr/0010-name-and-scope-boundaries.md)
  - [ADR 0011: Path identity](adr/0011-path-identity.md)
  - [ADR 0012: Dependency carry](adr/0012-dependency-carry.md)
  - [ADR 0013: The network protocol as built](adr/0013-network-protocol-as-built.md)
  - [ADR 0014: The store as built](adr/0014-store-as-built.md)
  - [ADR 0015: Harness trait as built](adr/0015-harness-trait-as-built.md)
  - [ADR 0016: The node and the CLI as built](adr/0016-node-and-cli-as-built.md)
  - [ADR 0018: The client API](adr/0018-client-api.md)
  - [ADR 0019: Managed jobs, tasks, attempts and operations](adr/0019-managed-job-task-attempt.md)
  - [ADR 0020: The managed authority, its journal and fencing eligibility](adr/0020-managed-authority-journal.md)
  - [ADR 0021: Versioned provider bridge to a Capsule worker](adr/0021-capsule-provider-bridge.md)
  - [ADR 0022: Reconciliation of provider operations and cleanup authorization](adr/0022-reconciliation-and-cleanup-authorization.md)
  - [ADR 0029: The Portable Session Format v1](adr/0029-portable-session-format.md)
<!-- adr:end -->

# Project

- [Roadmap](roadmap.md)
- [Contributing](contributing.md)
