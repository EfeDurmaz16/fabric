# Bridge protocol v1: Fabric and a Capsule provider worker

Status: proposed, 2026-10-27 (ADR 0021). This document is the single source of truth for bridge protocol version 1. The golden fixtures in [`protocol/bridge/v1`](../protocol/bridge/v1) are part of it: where this text and a fixture disagree, that is a bug in one of them, and both implementations must fail their fixture tests until it is fixed. Capsule keeps a byte-identical copy of the fixtures and a short pointer to this file.

## What the bridge is

Fabric (Rust) runs managed jobs. Capsule (TypeScript) maps governed actions onto providers. The bridge lets Fabric ask a Capsule worker process to perform one provider action at a time on behalf of one managed operation, and to report exactly what it observed.

Ownership does not move across the bridge:

| Concern | Owner |
| --- | --- |
| Job, task, attempt and operation identity; the operation journal; placement; generations and fencing | Fabric |
| Provider mapping, provider calls and observations, receipts, Capsule policy evaluation | Capsule worker |

The worker is not a second job controller. It keeps no durable job state. Losing it is transport loss, never cancellation, and never a reason to create a new operation identity.

Out of scope for v1: any remote transport, any listener, any authentication service, the client API of ADR 0018 (which this protocol neither exports nor changes), and arbitrary provider method calls.

## Failure modes first

These rules decide every case the fixtures cover. An implementation that gets the happy path right and any of these wrong is wrong.

1. **Refusal happens before effect.** A message that does not parse, an action outside the negotiated profile, a scope mismatch, a policy or approval refusal and an idempotency conflict all answer with `effect: "none"`, and the provider sees no call. The worker validates in this order and stops at the first failure: frame, JSON, message schema, action in profile, locator scope, idempotency, resource owner, policy, approval. Idempotency comes before policy because the fixtures require it: in `duplicate_idempotency_key` a changed request under an answered key is an `idempotency_conflict`, although image policy would refuse it too. Only then does it dispatch.
2. **Worker loss is not cancellation.** If the worker exits, closes stdout or breaks framing while a request is in flight, Fabric marks every dispatched operation of that worker `unknown` in the journal. It does not mark them cancelled or failed, does not resend them, and does not change the job, task, attempt or generation (scenario `worker_loss_is_not_cancellation`).
3. **A timeout is not permission to replay.** When Fabric's own deadline for a request passes, it may send `cancel`, and it records `unknown` unless the worker answered. It never sends the same operation again as a new `request`. The only way back to certainty is `observe` with the original operation id and locator, where the profile supports it.
4. **Terminal-only actions stay terminal-only.** An action the profile marks `terminal_only` answers `result` only after a terminal state was observed. If the deadline expires first, it answers `error` with `code: "pending"`, the scoped locator, the last observed native state, the number of observations and the deadline. This mirrors Capsule M1's `PendingOperationError`. Observation is bounded by the request's `deadline_ms` and, for `observe`, by `wait_ms`.
5. **Unknown is explicit.** When the worker cannot tell whether the provider applied an action (the reply was lost after the request was sent), it answers `code: "unknown"` with the best known locator. This mirrors Capsule M1's `UnknownOutcomeError`. Nothing retries it.
6. **Cancel acknowledgement is not termination.** `cancel_ack` says only that the worker received the cancel. The original request still gets its own answer, which may be `cancelled`, `completed`, `pending` or `unknown`. Cancel never destroys a resource; destroying is its own action with its own fresh approval.
7. **Fresh approval is additive and per operation.** An approval names one action on one resource. It can authorize what policy leaves to approval; it never overrides a policy deny. A create grant never authorizes destroy, and an expired grant authorizes nothing.
8. **Secrets: a present block with omitted `allowed` denies all.** When both Fabric's request policy and the worker's configured policy have a secrets block, the effective allowlist is their intersection. An absent block adds no restriction. Secret values never cross the bridge; only names do.
9. **Scope travels with every effect.** Every `request` that names a resource, every `result`, every `pending` and every `unknown` carries the full locator: provider, scope (for example account and project), kind and native id. A native id alone never routes an action.
10. **Evidence failure does not erase the outcome.** If writing a receipt fails after the provider action completed, the `result` keeps its state and locator and adds `evidence_error`.

## Transport

Fabric spawns the worker as a child process. Fabric writes frames to the worker's stdin; the worker writes frames to its stdout. Stdout carries frames and nothing else. Stderr is diagnostic text only; Fabric reads at most `max_stderr_bytes` of it per worker and drops the rest. The worker has no listener and opens no network port for the protocol.

### Framing

Each frame is a `u32` little-endian length `N`, then exactly `N` bytes of UTF-8 holding one JSON object and nothing after it. This is the framing of `fabric-client/src/frame.rs` and `sdk/typescript/src/frame.ts` (ADR 0018), reused as is; the bridge has its own envelope and version.

| Rule | Value |
| --- | --- |
| Fabric to worker, every frame (hello and requests included) | at most 1 MiB (1048576 bytes) |
| Worker to Fabric, `hello_ack` and `hello_refused` | at most 1 MiB |
| Worker to Fabric, every other frame | at most 16 MiB (16777216 bytes) |
| Length 0 | refused as `empty` |
| Fewer than 4 length bytes, or fewer than `N` body bytes, before end of stream | refused as `truncated` |
| Body not UTF-8, not one JSON object, or bytes after the object | refused as `malformed` |

A reader checks the declared length against its cap before it allocates or reads the body. A refused length is refused unread (fixtures with `"read_body": false`). The caps are inclusive: a frame of exactly the cap is accepted by length. A profile or `hello_ack` may lower a cap, never raise it.

A framing error cannot be resynchronized. The worker answers it with one `protocol_error` frame, if it still can, and exits with status 2. Fabric answers a framing error from the worker by closing the worker's stdin, sending SIGTERM, and after a grace period SIGKILL; every dispatched operation of that worker becomes `unknown` (rule 2).

### Encoding

The rules of ADR 0018 apply, tightened for the bridge:

- Every object has named fields. Unknown fields are refused (`invalid`/`unknown_field` from the worker; `protocol_violation` in Fabric). A variant is a string, never a number or a map.
- Integers are JSON integers in `0..=2^53-1` unless a field says otherwise. A float, an exponent form, `-0` or a value out of range is refused as `bad_integer`.
- Revisions and generations are canonical decimal strings: `"1"` to `"18446744073709551615"`, no sign, no leading zero, never `"0"` for a generation. A JSON number in their place is refused.
- Ids: `job`, `task`, `attempt` and `operation.id` are ULIDs in canonical text, 26 characters of uppercase Crockford base32. `operation.key` is Fabric's `OperationKey`: 1 to 128 bytes of `[A-Za-z0-9._:-]`. `request_id` uses the same charset and is unique per connection. `operation.digest` is 64 lowercase hex characters. Anything else is refused as `bad_id`.
- Strings that become argv, paths or names hold no NUL and no other control character except `\n` and `\t` in `stdin` and output text (`bad_string`).
- Optional fields are absent, not `null`, unless the table says the field is required and nullable. Required nullable fields are written as `null` so the slot is visible.

### Locator

```json
{ "provider": "fixture", "scope": { "account": "acct-a", "project": "proj-1" }, "kind": "sandbox", "native_id": "fx-key-create-1" }
```

| Field | Type | Rule |
| --- | --- | --- |
| `provider` | string | the profile's `provider` |
| `scope` | object of string to string | exactly the profile's `scope_keys`, each 1 to 512 characters; required even when a key set is one entry |
| `kind` | string | one of the profile's `resource_kinds` |
| `native_id` | string | 1 to 512 characters, no control character, no `?` or `#`, no URL userinfo (`bad_locator`) |
| `native_operation_id` | string, optional | the provider's long-running operation id, kept apart from the resource id |

This is Capsule's `ResourceLocator` (M1, CD-DELETE) on the wire, in snake case. Capsule's `resourceLocator()` sanitizes; the bridge parser refuses instead, because a request that needs sanitizing was built wrong.

## Handshake

The first frame from Fabric is `hello`. The worker performs no provider call, reads no credential and resolves no approval before it has sent `hello_ack`. Any other first frame is a `protocol_error` (`unexpected`). Fabric sends nothing but `hello` until it has read `hello_ack`, and kills a worker that has not answered within 10 seconds.

### `hello` (Fabric to worker)

```json
{
  "type": "hello",
  "versions": { "min": 1, "max": 1 },
  "fabric": "fabric/0.1.0",
  "profile": "fixture",
  "limits": { "max_request_bytes": 1048576, "max_frame_bytes": 16777216, "max_in_flight": 8, "max_stderr_bytes": 65536 }
}
```

| Field | Rule |
| --- | --- |
| `versions` | inclusive range of bridge protocol majors Fabric speaks; `min <= max`, both at least 1 |
| `fabric` | Fabric's name and version, 1 to 64 characters |
| `profile` | the capability profile Fabric wants, by name |
| `limits.max_request_bytes` | at most 1048576 |
| `limits.max_frame_bytes` | at most 16777216 |
| `limits.max_in_flight` | 1 to 64 |
| `limits.max_stderr_bytes` | 0 to 1048576 |

### `hello_ack` (worker to Fabric)

```json
{
  "type": "hello_ack",
  "protocol": 1,
  "worker": "capsule-worker/0.1.0",
  "profile": { "profile": "fixture", "version": 1, "...": "the full capability profile, see below" },
  "limits": { "max_request_bytes": 1048576, "max_frame_bytes": 16777216, "max_in_flight": 4, "max_stderr_bytes": 65536 }
}
```

`protocol` is the highest version in both ranges. `profile` is the complete capability profile document the worker will enforce, and its `profile` name must equal the one asked for. Each `limits` field is at most the value in `hello`; Fabric refuses an ack that raises one (`limit_raised`) or names another profile (`profile_mismatch`), kills the worker and records why.

### `hello_refused` (worker to Fabric)

```json
{ "type": "hello_refused", "versions": { "min": 1, "max": 1 }, "reason": { "code": "unsupported_version", "message": "worker speaks bridge protocol 1 to 1" } }
```

`reason.code` is `unsupported_version`, `unsupported_profile` or `invalid`. `versions` is the worker's own range. The worker exits with status 2 after writing it. This message, its fields and its frame never change in any later protocol version, so that every Fabric can learn which version a worker speaks.

## Session messages

After the handshake Fabric may have up to `max_in_flight` requests and observes outstanding. A request beyond that is answered `unavailable`/`busy` with `effect: "none"`. Answers may arrive in any order; `request_id` correlates them.

### `request` (Fabric to worker)

```json
{
  "type": "request",
  "request_id": "r-1",
  "operation": { "id": "01K8W3A00000000000000000P1", "key": "key-create-1", "digest": "1111...1111" },
  "scope": { "job": "01K8W3A00000000000000000J1", "task": "01K8W3A00000000000000000T1", "attempt": "01K8W3A00000000000000000A1", "generation": "1" },
  "action": "sandbox.create",
  "params": { "image": "fixture/ok", "env": {}, "secret_names": [], "labels": {} },
  "approval": null,
  "deadline_ms": 30000
}
```

| Field | Required | Rule |
| --- | --- | --- |
| `request_id` | yes | transport correlation, unique per connection; a repeat is `invalid`/`duplicate_request_id` |
| `operation.id` | yes | the `OperationId` Fabric committed in the journal before sending (ADR 0020, durable intent first) |
| `operation.key` | yes | the operation's idempotency key, the journal's `OperationKey` |
| `operation.digest` | yes | Fabric's request digest for this operation. The worker never computes it; it compares it for equality |
| `scope.job`, `scope.task`, `scope.attempt` | yes | the managed identities this effect belongs to |
| `scope.generation` | yes | the attempt's fencing generation, canonical decimal string. Fabric fences; the worker records it on what it creates |
| `action` | yes | one of the actions below; outside the negotiated profile it is `unsupported` |
| `target` | per action | locator of the existing resource the action acts on |
| `params` | yes | the action's own object, below; no other fields |
| `approval` | yes, nullable | the fresh approval slot: `null`, or `{ "ref": "<1..128 of [A-Za-z0-9._:-]>" }`. A ref names a grant the worker resolves locally; it is never a bearer credential and never appears in logs or receipts |
| `policy` | no | Fabric's narrowing of Capsule policy for this operation. v1 defines only `{ "secrets": { "allowed": ["NAME"] } }` with `allowed` optional; rule 8 |
| `deadline_ms` | yes | milliseconds from when the worker reads the frame, 1 to the profile's `max_deadline_ms` |

### Actions in v1

| Action | `target` | `params` | `result.output` |
| --- | --- | --- | --- |
| `sandbox.create` | absent | `image` (string), `env` (object, plain non-secret values), `secret_names` (array of names), `labels` (object) | `{ "status": "ready" }` |
| `sandbox.exec` | required | `argv` (1 to `max_argv` strings), `stdin` (string or null, at most `max_stdin_bytes`), `cwd` (string or null), `timeout_ms`, `max_output_bytes` (at most the profile's), `secret_names` | `{ "exit_code": int or null, "stdout": stream, "stderr": stream }` |
| `sandbox.destroy` | required | `{}` | `{ "status": "destroyed" }` |

A stream is `{ "text": string, "captured_bytes": int, "truncated": bool, "lossy": bool }`: the captured bytes decoded as UTF-8 with invalid sequences replaced (then `lossy` is true), capped while reading, not after. A nonzero exit is a `completed` result, not an error, and the sandbox stays. `exit_code` is `null` only when no exit was observed, as after a cancel.

The worker labels every resource it creates with the operation id, the job, the attempt and the generation, so that a later `observe` can find it and an action on a resource another job created is refused (`denied`/`owner_mismatch`).

### `result` (worker to Fabric)

```json
{
  "type": "result",
  "request_id": "r-1",
  "operation_id": "01K8W3A00000000000000000P1",
  "state": "completed",
  "resource": { "provider": "fixture", "scope": { "account": "acct-a", "project": "proj-1" }, "kind": "sandbox", "native_id": "fx-key-create-1" },
  "output": { "status": "ready" },
  "replayed": false,
  "receipts": []
}
```

| Field | Rule |
| --- | --- |
| `state` | `completed` (terminal state observed), `cancelled` (terminal cancellation observed), or for actions that are not `terminal_only`, `accepted` or `running` |
| `resource` | required; the full locator. A result without it, or with its scope dropped, is a `protocol_violation` |
| `output` | the action's output object |
| `replayed` | true when this answers a repeated key and digest from the worker's memory or the provider's dedupe, with no new provider call |
| `receipts` | at most `max_receipts` of `{ "algorithm": "sha256", "digest": "<64 hex>", "ref": "<string>" }` |
| `evidence_error` | optional `{ "reason": "receipt_write_failed", "message": string }`; rule 10 |

### `error` (worker to Fabric)

```json
{
  "type": "error",
  "request_id": "r-3",
  "operation_id": "01K8W3A00000000000000000P6",
  "error": {
    "code": "pending",
    "reason": "observation_deadline",
    "message": "pending: observation_deadline",
    "effect": "pending",
    "resource": { "provider": "fixture", "scope": { "account": "acct-a", "project": "proj-1" }, "kind": "sandbox", "native_id": "fx-key-create-1" },
    "state": "stopping",
    "observations": 3,
    "deadline_ms": 3000
  }
}
```

| `code` | `effect` | Locator | Meaning | Journal status Fabric records |
| --- | --- | --- | --- | --- |
| `invalid` | `none` | absent | the request is malformed or conflicts with an earlier one | `rejected` |
| `unsupported` | `none` | absent | action, observe or capability not in the negotiated profile | `rejected` |
| `denied` | `none` | absent | policy, approval, secret or scope refusal | `rejected` |
| `unavailable` | `none` | absent | `busy` (in-flight cap) or `shutting_down` | `rejected`, and Fabric may submit a new operation later |
| `provider_failed` | `none` or `applied` | required when `applied` | the provider gave a definite failure | `failed` |
| `pending` | `pending` | required | accepted and not terminal within the bound; rule 4 | stays `dispatched` |
| `unknown` | `unknown` | required when any identity was observed | rule 5 | `unknown` |

`reason` values in v1:

| `code` | `reason` |
| --- | --- |
| `invalid` | `unknown_field`, `missing_field`, `bad_integer`, `bad_id`, `bad_string`, `bad_locator`, `limit`, `duplicate_request_id`, `idempotency_conflict` |
| `unsupported` | `action`, `observe`, `cancel` |
| `denied` | `policy_denied`, `approval_required`, `approval_expired`, `approval_scope`, `secret_not_allowed`, `scope_mismatch`, `owner_mismatch` |
| `unavailable` | `busy`, `shutting_down` |
| `provider_failed` | `provider_refused`, `not_found`, `quota` |
| `pending` | `observation_deadline`, `shutting_down` |
| `unknown` | `reply_lost`, `observation_failed` |

A code outside the table, or an `effect` that contradicts it (for example `denied` with `applied`), is a `protocol_violation`. `message` is a safe diagnostic of at most 1024 characters with no secret, credential or URL query; fixtures compare it for presence only. Optional fields: `resource`, `state` (last observed native state), `observations`, `deadline_ms`, and for `provider_failed` with `applied` a `cleanup` object `{ "status": "not-attempted" | "succeeded" | "failed" | "pending" | "unknown", "residual": [locator] }` (Capsule M1's `CleanupOutcome`).

### Idempotency

The worker keeps a bounded per-connection table of answered `operation.key` values with their digest and answer. A request with a known key:

- same digest: the stored answer again, with the new `request_id` and `replayed: true`, no provider call (scenario `duplicate_idempotency_key`);
- other digest: `invalid`/`idempotency_conflict`, `effect: "none"`;
- while the first is still in flight: `invalid`/`idempotency_conflict` as well; Fabric never does this on purpose.

The table does not survive the worker. Across worker restarts the profile's `dedupe` says what holds: `provider_key` (the provider deduplicates on the key within its scope), `label_query` (the worker finds a resource it created by its operation label before creating another), or `none` (Fabric must not resend; rule 3).

### `observe` (Fabric to worker)

```json
{ "type": "observe", "request_id": "r-10", "operation_id": "01K8W3A00000000000000000P3", "action": "sandbox.destroy", "resource": { "...": "locator" }, "wait_ms": 5000 }
```

Reads the state of an earlier operation's resource and waits at most `wait_ms` (1 to `max_observe_wait_ms`) for it to become terminal. It makes no mutating provider call and needs no approval. It answers `result` when the terminal state is observed, `error`/`pending` with the new observation count when it is not, `error`/`unknown`/`observation_failed` when it cannot read, and `error`/`unsupported`/`observe` when the profile says `observe: false` for that action. The `operation_id` must be the original operation's.

### `cancel` and `cancel_ack`

```json
{ "type": "cancel", "request_id": "r-9", "cancels": "r-2", "operation_id": "01K8W3A00000000000000000P2" }
{ "type": "cancel_ack", "request_id": "r-9", "cancels": "r-2", "state": "requested" }
```

`state` is `requested` (the worker is stopping its wait and asking the provider to stop where the profile says `cancel: true`), `not_found` (no such request in flight), `already_answered`, or `unsupported` (the action has `cancel: false`; the worker keeps waiting). The cancelled request still gets exactly one answer of its own (rule 6).

### `progress` (worker to Fabric)

```json
{ "type": "progress", "request_id": "r-2", "operation_id": "01K8W3A00000000000000000P2", "seq": 1, "state": "running", "resource": { "...": "locator" } }
```

Optional, at most 16 per request, `seq` from 1 increasing by 1. It lets Fabric record the locator as soon as it is known (ADR 0020, record the observed locator early). It carries no output.

### `shutdown` and `shutdown_ack`

```json
{ "type": "shutdown", "request_id": "r-99", "drain_ms": 5000 }
{ "type": "shutdown_ack", "request_id": "r-99", "unresolved": [ { "request_id": "r-3", "operation_id": "01K8W3A00000000000000000P6", "state": "pending", "resource": { "...": "locator" } } ] }
```

After `shutdown` the worker refuses new requests with `unavailable`/`shutting_down`, lets in-flight requests finish for at most `drain_ms` (1 to 30000), answers each still open one with `pending` or `unknown`, writes `shutdown_ack` listing them, closes stdout and exits 0. It never reports a resource destroyed or stopped that it did not observe. SIGTERM without `shutdown` is a shutdown with `drain_ms` 5000 whose ack Fabric may never read. Closing the worker's stdin is a shutdown with `drain_ms` 0.

### `protocol_error` (worker to Fabric)

```json
{ "type": "protocol_error", "code": "oversize", "message": "frame of 1048577 bytes exceeds the request cap of 1048576" }
```

`code` is `oversize`, `truncated`, `empty`, `malformed` or `unexpected`. It is the worker's last frame.

## Capability profile v1

A profile is a document the worker sends whole in `hello_ack` and enforces. It describes what the worker can invoke and verify, not what the provider could do in principle. Absence of a capability blocks the workflow that needs it; it is never worked around with a raw provider call.

| Field | Meaning |
| --- | --- |
| `profile`, `version` | name and profile version (1) |
| `provider`, `scope_keys`, `resource_kinds` | what every locator of this profile looks like |
| `clock` | `virtual` (the fixture: each observation advances 1 `observe_interval_ms`, so deadlines are deterministic) or `wall` |
| `observe_interval_ms` | the interval between observations |
| `actions[]` | per action: `effect` (`create`, `mutate`, `destroy`), `terminal_only`, `observe`, `cancel`, `approval` (`optional`: policy decides and a supplied approval must be valid; `required`: no valid fresh approval means `denied`), `dedupe` (`provider_key`, `label_query`, `none`), `secrets` (whether `secret_names` may be nonempty) |
| `limits` | `max_deadline_ms`, `max_observe_wait_ms`, `max_output_bytes`, `max_stdin_bytes`, `max_argv`, `max_receipts` |

The two v1 profiles are [`profiles/fixture.json`](../protocol/bridge/v1/profiles/fixture.json) and [`profiles/docker.json`](../protocol/bridge/v1/profiles/docker.json):

| Action | fixture | docker |
| --- | --- | --- |
| `sandbox.create` | terminal-only, observe, no cancel, approval optional, dedupe `provider_key` | terminal-only (running container observed), observe, no cancel, approval optional, dedupe `label_query`, `image_pull: never` |
| `sandbox.exec` | not terminal-only, no observe, cancel, approval optional | not terminal-only, no observe, no cancel, approval optional |
| `sandbox.destroy` | terminal-only, observe, no cancel, approval required | terminal-only (container gone observed), observe, no cancel, approval required |

The fixture profile is a deterministic, stateful synthetic provider (draft P12 layer L1). Its state is the scenario's `provider_state`: a binding (account and project), a policy, approvals, existing sandboxes and a script of faults. Native ids are `fx-<operation key>`. It counts mutating provider calls, which is what `provider_calls` asserts. Docker (layer L3) uses scope `{ "engine": "<docker context>" }`, kind `container`, the container id as `native_id`, never pulls an image, and labels each container `fabric.operation`, `fabric.job`, `fabric.attempt`, `fabric.generation`. Neither name appears in Fabric's job domain; Fabric reads only the profile.

## Fixtures

[`protocol/bridge/v1`](../protocol/bridge/v1) holds four kinds of file and a manifest:

| Path | Content | What each implementation checks |
| --- | --- | --- |
| `frames/*.json` | `hex` of raw bytes on the stream, `direction`, `phase` and `expect` (`accept` with the message type, or `reject` with `code` and `read_body`) | the frame reader on that side refuses or accepts exactly so, and refuses `read_body: false` cases without reading past the length |
| `cases/*.json` | one `message`, its `direction`, `phase` and `expect` (`accept`, or `reject` with `code` and `reason`) | the parser on the receiving side accepts it, or refuses it with that code and reason; accepted messages write back to the same JSON |
| `scenarios/*.json` | `provider_state` and ordered `steps`, run after the canonical handshake (`cases/hello_valid.json`, `cases/hello_ack_valid.json`) unless the first step is `{ "handshake": false }` | Capsule runs the worker against the fixture provider and compares each `receive` exactly, except `message` strings; Fabric runs its client against a scripted worker that replays the `receive` steps and checks `journal` steps |
| `profiles/*.json` | the v1 capability profiles | the worker sends exactly this document; Fabric parses it |
| `manifest.sha256` | `sha256sum` lines of every other file, sorted by path | both repos' tests check every file against it and that no file is missing or extra |

Scenario step forms: `send` (Fabric to worker), `receive` (worker to Fabric), `provider_calls` (count of mutating provider calls so far), `journal` (operation status, or unchanged identity, Fabric records), `sandbox` (fixture provider state), `advance` (`settle`: scripted transitional states become terminal), `kill_worker`, `respawn`, `worker_exit` (exit status).

The same fixture set is copied byte for byte into Capsule at the same relative path. Each repository has a test that checks its copy against `manifest.sha256` and pins the manifest's own sha256, so a change in one repository fails the other until both carry it. To regenerate the manifest after an edit:

```sh
cd protocol/bridge/v1
find . -type f ! -name manifest.sha256 | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256
shasum -a 256 manifest.sha256
```

Then update the pinned manifest digest in `crates/fabric-client/tests/bridge_fixtures.rs` here and in Capsule's `scripts/bridge-fixtures.test.mjs`, in linked pull requests.

## Versioning

Bridge protocol v1 is independent of the client API version (ADR 0018). Within v1, fixtures only grow: a new action, a new profile or a new optional field arrives with new fixtures and keeps every existing fixture byte-identical. A changed or removed fixture, a new `reject` case for a value v1 accepted, a new error code, or a raised limit is v2. `hello_refused` and its fixtures never change. A profile's own `version` moves when its action table changes meaning.

## Fabric's client as built (M3)

`crates/fabric-bridge` is Fabric's side: `wire` (every message, its parser and its encoder), `frames` (the framing above), and `Bridge`, the client. It records into the managed journal through `OperationLog`; schema version 2 adds `provider_operations` for this (docs/managed-jobs.md). The failure rules above become these behaviors, each pinned by a test in `crates/fabric-bridge/tests`.

### Before anything is written

| Check | When it fails | Journal |
| --- | --- | --- |
| Worker gone, bridge shutting down, `max_in_flight` reached | `Lost`, `ShuttingDown`, `Busy` | nothing; the operation may be sent later |
| Anything the worker would refuse on reading it, checked with the worker-side parser under the negotiated profile: an action not in the profile, a limit over the profile's (deadline, argv, stdin, output), a target that does not fit the profile, any encoding fault such as a NUL in an argument; and a frame over `max_request_bytes` | `RefusedLocally` with the code and reason the worker would answer | `rejected`, nothing written |
| The journal already holds the key or the id | `Log(Used)` | unchanged; nothing is ever sent twice |
| The attempt is not the job's current live grant at its generation, or the job is closed | `Log(Fenced)` | nothing |
| Otherwise | the operation is committed `dispatched`, then the frame is written | `dispatched` |

`operation.digest` is the caller's: a node computes it with `wire::request_digest`, blake3 under the domain `fabric.bridge.request.v1` over the canonical JSON (keys sorted) of the request without `type`, `request_id`, `operation` and `deadline_ms`. The client sends what it is given and the journal keeps it.

### Answers

| Answer to a `request` | Journal |
| --- | --- |
| `result` `completed` / `cancelled` | `succeeded` / `cancelled`, with the locator |
| `result` `accepted` / `running` (not terminal-only) | stays `dispatched`, with the locator |
| `result` not terminal on a terminal-only action, an answer under another operation id, any `protocol_violation`, `protocol_error`, a refused frame, end of stdout | the worker is lost and terminated (stdin closed, SIGTERM, SIGKILL after the grace); every outstanding operation is `unknown` |
| `error` `invalid`, `unsupported`, `denied`, `unavailable` | `rejected` |
| `error` `provider_failed` | `failed`, with the locator when `applied` |
| `error` `pending` | stays `dispatched`, with the answer and the locator |
| `error` `unknown` | `unknown`, with the best known locator |
| `progress` | the locator is stored; the status stays |
| no answer within `deadline_ms` plus the grace (2 s by default) | `unknown`; a later answer is dropped unread |

`observe` is sent only for an operation the journal holds as `dispatched` or `unknown`, by its original id, and only for an action the profile observes within its `max_observe_wait_ms`; otherwise it is refused before sending and the journal is unchanged. Its `result` moves the operation to `succeeded` or `cancelled`, `unknown` moves a `dispatched` one to `unknown`, `pending` stores the locator, and a refused or failed observe changes nothing: it says nothing new about the operation it read. `cancel` of a request that is not outstanding is `not_found`, and of an action the profile does not cancel `unsupported`, both without a frame. A `cancel_ack` changes nothing in the journal. After `shutdown` new requests are refused before the journal is touched.

### The fixtures from Fabric's side

The scenario test runs every scenario against a scripted in-process worker and a real journal holding the fixtures' job, task and attempt. Each `send` is made through the client's API and the frame it writes must equal the fixture after Fabric's own request ids are mapped onto the fixture's. A `send` Fabric refuses itself must refuse with the code and reason, or the cancel state, of the `receive` the worker answers it with. Three kinds of `send` are never written by Fabric, and the worker's answer to them is only parsed; the Capsule run of the same fixtures still exercises them:

- `duplicate_idempotency_key` r-2 and r-3, `secrets_deny_all_and_intersection` r-3 and `fresh_approval_required` r-2 repeat an operation id the journal already holds (r-3 and r-2 of the last two with a new key). Fabric never sends an operation twice; a worker must still answer a repeat as the fixture says.
- `scope_retained` r-2 names job `...J2` with the task and attempt ids of job `...J1`. A journal cannot hold that scope, so Fabric fences it before any effect; the worker refuses it before any effect too (`owner_mismatch`).

These are observations about the fixtures, not changes: v1's fixtures stay byte for byte as they are.

## Forward compatibility with reconciliation (draft P11)

v1 already carries what M4 needs and promises nothing more: the journal's operation id and key on every request, the locator on every effect, `pending` and `unknown` as distinct answers, `observe` by original identity, labels on created resources and a per-profile `dedupe` statement. Automatic reconciliation, cleanup authorization after job cancellation and attach to a running execution are not v1 behaviors; a worker that does not support them says so through its profile.
