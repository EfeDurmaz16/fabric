# Client API

The client API is how a program other than the CLI drives a node: versioned, length-prefixed JSON on the node's control socket, `$FABRIC_HOME/fabric.sock` ([ADR 0018](../adr/0018-client-api.md)). The CLI itself is a client of it.

## The exchange

One connection carries one exchange:

```text
client: Hello      { api: 1, client: "<name>/<version>" }
node:   Welcome    { type: "welcome", api: 1, node, node_version, capabilities: [...] }
      | Incompatible { type: "incompatible", api, node_version }        and the node closes
client: Request    { op: ..., ... }
node:   Event      { type: "event", event: { kind: ..., ... } }         zero or more
node:   Done       { type: "done", reply: { kind: <the request's op>, ... } }
      | Refused    { type: "refused", refusal: { kind, reason: { code, ... }, message } }
```

Each frame is a `u32` little-endian length, 1 byte to 16 MiB, then one UTF-8 JSON object. A node parses a hello or a request only up to 1 MiB.

## Requests in api 1

`node_id`, `node_add`, `node_list`, `node_ping`; `job_new`, `job_list`, `job_status`, `job_log`, `job_run`, `job_resume`, `job_suspend`, `job_move`, `job_steal`, `job_decide`, `job_repair`, `job_export`; `doctor`; `launch_check`, `harness_started`, `harness_session`, `harness_exited`; `managed_create`, `managed_cancel`, `managed_status`, `managed_list`. ADR 0018 gives every field.

`job_run` and `job_resume` answer a run plan; they do not start the harness. The client launches it in its own process group and reports `harness_started`, `harness_session` and `harness_exited`, as the CLI does.

## Rules every message follows

- Enums are objects tagged by one field (`op`, `type`, `kind`, `code`); names are `snake_case`. A number where a tag belongs is refused.
- Ids travel as text: ULIDs, `fab1...` node ids, 64-hex snapshot ids.
- Managed counters (revision, generation, sequence) travel as decimal strings, so JavaScript never rounds them; epochs, counts and times are integers below 2^53.
- An optional field is absent, never `null`; readers accept `null` as absent.
- A version only grows. A fixture under `protocol/fixtures` that the base already had changes only with a bump of `API_VERSION`, and `scripts/check-fixtures.sh` fails otherwise.

## Fixtures and clients

[`protocol/fixtures`](../../protocol/fixtures) holds one example of every message. Two clients test against it:

- `crates/fabric-client`: the Rust client, used by the CLI.
- `sdk/typescript`: the [TypeScript SDK](ts-sdk.md).

The socket is trusted only when it and its parent directories belong to the user and no one else can write them; both clients check that before they connect.
