<!-- This page is written by scripts/docs/gen.py. Do not edit it; run the script. -->

# Decisions

Every architecture decision record on `main`, with the status line the ADR itself carries. An ADR that says `proposed` and `as built on main` records code that is on `main` while the decision is still open for amendment; the status line says which parts.

No ADR on `main` has the number 0017, 0028.

How to write one is in [CONTRIBUTING, Decisions](../../CONTRIBUTING.md#decisions).

| ADR | Title | Status |
|---|---|---|
| [0001](0001-job-as-unit-of-work.md) | The job is the unit of work | accepted, 2026-10-19 |
| [0002](0002-job-ledger-and-record.md) | Job ledger and record | accepted, 2026-10-19 |
| [0003](0003-exactly-one-live-copy.md) | Exactly one live copy | accepted, 2026-10-19 |
| [0004](0004-content-addressed-snapshots.md) | Content-addressed snapshots | accepted, 2026-10-19; the store layout, the store lock, publish, gc, the file writer and the capture mechanics named in [ADR 0014](0014-store-as-built.md) are superseded by it, 2026-10-20 |
| [0005](0005-transport-and-node-identity.md) | Transport and node identity | accepted, 2026-10-19; the `Transport` trait and where iroh plugs in, the message shapes and the 1 MiB control frame figure under Framing, the multiplexer header under the TCP+TLS fallback, the dependency list under Consequences, and the open question on `rcgen` and `ed25519-dalek` are superseded by [ADR 0013](0013-network-protocol-as-built.md), 2026-10-20 |
| [0006](0006-harness-adapters.md) | Harness adapters | accepted, 2026-10-19 |
| [0007](0007-environment-spec.md) | Environment spec | accepted, 2026-10-19 |
| [0008](0008-placement-and-triggers.md) | Placement and triggers | accepted, 2026-10-19 |
| [0009](0009-engineering-conventions.md) | Engineering conventions | accepted, 2026-10-19 |
| [0010](0010-name-and-scope-boundaries.md) | Name and scope boundaries | accepted, 2026-10-19 |
| [0011](0011-path-identity.md) | Path identity | accepted, 2026-10-19 |
| [0012](0012-dependency-carry.md) | Dependency carry | accepted, 2026-10-19 |
| [0013](0013-network-protocol-as-built.md) | The network protocol as built | accepted, 2026-10-20 |
| [0014](0014-store-as-built.md) | The store as built | accepted, 2026-10-20 |
| [0015](0015-harness-trait-as-built.md) | Harness trait as built | accepted, 2026-10-20 |
| [0016](0016-node-and-cli-as-built.md) | The node and the CLI as built | accepted, 2026-10-20 |
| [0018](0018-client-api.md) | The client API | accepted, 2026-10-21 |
| [0019](0019-managed-job-task-attempt.md) | Managed jobs, tasks, attempts and operations | proposed, 2026-10-27; as built on `main` since 2026-11-03 (PR #31) |
| [0020](0020-managed-authority-journal.md) | The managed authority, its journal and fencing eligibility | proposed, 2026-10-27; as built on `main` since 2026-11-03 (PR #31, schema version 2 in PR #32). Amended 2026-10-29: schema version 2 adds the `provider_operations` table of ADR 0021, which Efe approved that day; version 2 has not shipped in any release. |
| [0021](0021-capsule-provider-bridge.md) | Versioned provider bridge to a Capsule worker | proposed, 2026-10-27; the bridge client and protocol v1 are on `main` since 2026-11-03 (PR #32). The `provider_operations` journal table (schema version 2) was approved by Efe on 2026-10-29; the rest of this ADR stays proposed. |
| [0022](0022-reconciliation-and-cleanup-authorization.md) | Reconciliation of provider operations and cleanup authorization | proposed, 2026-10-29; as built on `main` since 2026-11-08 (PR #36). |
| [0023](0023-managed-task-lifecycle-and-checkpoint-cut.md) | Managed task lifecycle, containment and the checkpoint cut | proposed 2026-11-03, built 2026-11-03 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). |
| [0024](0024-work-package-and-continuation-fidelity.md) | The portable work package and continuation fidelity | proposed 2026-11-03, built 2026-11-03 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). |
| [0025](0025-source-bindings-and-secret-references.md) | Source bindings, secret references and the export policy | proposed 2026-11-03, built 2026-11-03 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). |
| [0026](0026-resource-placement.md) | Resource placement among registered nodes | proposed, 2026-11-03. |
| [0027](0027-job-facade-protocol.md) | The job facade: authenticated remote job operations over the node link | proposed, 2026-11-03. |
| [0029](0029-portable-session-format.md) | The Portable Session Format v1 | proposed 2026-11-08 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). PR 4 of the 2026-11-08 roadmap. |
| [0030](0030-residuals-and-gc.md) | Residuals and `fabric gc` | proposed 2026-11-09 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). PR 11 of the 2026-11-08 roadmap, the Fabric side; the Capsule side merged as capsule#321. |
