# Protocols

The wire formats Fabric speaks, each specified on its own page:

| Protocol | Between | Page |
|---|---|---|
| Client API, api 1 | A client and its local node, on the control socket | [Client API](client-api.md), [ADR 0018](../adr/0018-client-api.md) |
| Node to node | Two paired nodes, over QUIC or TCP plus TLS with pinned ids | [Network protocol](../net.md), [ADR 0013](../adr/0013-network-protocol-as-built.md) |
| Provider bridge, v1 | Fabric and a Capsule provider worker, over stdio | [Provider bridge](../bridge-protocol.md), [ADR 0021](../adr/0021-capsule-provider-bridge.md) |
| Portable Session Format, v1 | A node and any tool that reads or writes a moving session, as files | [Portable Session Format](../portable-session-format.md), [ADR 0029](../adr/0029-portable-session-format.md) |
| Managed journal, schema 1 and 2 | The authority and its SQLite file | [Managed jobs and the journal](../managed-jobs.md), [ADR 0020](../adr/0020-managed-authority-journal.md) |

How the pieces fit is on the [architecture page](../architecture.md).
