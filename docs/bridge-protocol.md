# Bridge protocol v1: Fabric and a Capsule provider worker

Status: proposed, 2026-10-27 (ADR 0021). Amended 2026-10-29: an exec `timeout_ms` or `max_output_bytes` of 0 is invalid (decided by Efe that day), and v1 takes corrections until it first ships (Versioning); under that rule, an `observe` names the original operation's scope, both profiles observe `sandbox.exec`, and a fixture seed that names a job names the operation that made it. Amended again 2026-10-29 by ADR 0022 (M4): the `attach` message, the reasons `unsupported`/`attach` and `unknown`/`absent`, the error field `searched`, the fixture seed status `starting` and the scenario header `journal_state` (section "Reconciliation (M4)"), additions to v1 under the same rule. This document is the single source of truth for bridge protocol version 1. The golden fixtures in [`protocol/bridge/v1`](../protocol/bridge/v1) are part of it: where this text and a fixture disagree, that is a bug in one of them, and both implementations must fail their fixture tests until it is fixed. Capsule keeps a byte-identical copy of the fixtures and a short pointer to this file.

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

The worker leads a process group of its own, which Fabric creates when it starts it. When the worker is lost or closed (a framing or protocol error, its input not read, the end of its stdout, closing its stdin, or Fabric dropping it), Fabric ends that group: the worker has the kill grace to exit, what is left of its group has the rest of that grace, and then the group gets SIGKILL. So nothing the worker started in its group, such as an attached `docker exec` client, outlives it. Ending the group is not a cancel (rule 2), because a provider effect is never in it: a worker keeps out of its group every process whose end would end or abort an effect, by starting that process in a group of its own, and what the provider runs (a container, or the command the engine runs for `docker exec`) belongs to the provider. An operation in flight when the group ends is `unknown`, as for any lost worker.

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

A framing error cannot be resynchronized. The worker answers it with one `protocol_error` frame, if it still can, and exits with status 2. Fabric answers a framing error from the worker by closing the worker's stdin, sending it SIGTERM, and after a grace period SIGKILL to its process group; every dispatched operation of that worker becomes `unknown` (rule 2).

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

After the handshake Fabric may have up to `max_in_flight` requests, observes and attaches outstanding. A request beyond that is answered `unavailable`/`busy` with `effect: "none"`. Answers may arrive in any order; `request_id` correlates them.

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
| `sandbox.exec` | required | `argv` (1 to `max_argv` strings), `stdin` (string or null, at most `max_stdin_bytes`), `cwd` (string or null), `timeout_ms` (at least 1), `max_output_bytes` (1 to the profile's), `secret_names` | `{ "exit_code": int or null, "stdout": stream, "stderr": stream }` |
| `sandbox.destroy` | required | `{}` | `{ "status": "destroyed" }` |

A stream is `{ "text": string, "captured_bytes": int, "truncated": bool, "lossy": bool }`: the captured bytes decoded as UTF-8 with invalid sequences replaced (then `lossy` is true), capped while reading, not after. A nonzero exit is a `completed` result, not an error, and the sandbox stays. `exit_code` is `null` only when no exit was observed, as after a cancel.

An exec `timeout_ms` or `max_output_bytes` of 0 is refused as `invalid`/`limit` with `effect: "none"` (cases `request_exec_timeout_zero` and `request_exec_output_bytes_zero`). A 0 asks for no time or no output, which no command can meet, and neither side guesses another bound for it. Fabric refuses it before sending, as it does every value the worker would refuse on reading.

The worker labels every resource it creates with the operation id, the job, the attempt and the generation, so that a later `observe` can find it and an action on a resource another job created is refused (`denied`/`owner_mismatch`). A fixture seed (`provider_state.sandboxes[]`) that names its `owner_job` names the `operation` that made it as well, as every container the Docker profile creates carries both labels; the fixture provider refuses a seed with one and not the other.

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
| `unsupported` | `action`, `observe`, `cancel`, `attach` |
| `denied` | `policy_denied`, `approval_required`, `approval_expired`, `approval_scope`, `secret_not_allowed`, `scope_mismatch`, `owner_mismatch` |
| `unavailable` | `busy`, `shutting_down` |
| `provider_failed` | `provider_refused`, `not_found`, `quota` |
| `pending` | `observation_deadline`, `shutting_down` |
| `unknown` | `reply_lost`, `observation_failed`, `absent` (an answer to `attach` only) |

A code outside the table, or an `effect` that contradicts it (for example `denied` with `applied`), is a `protocol_violation`. `message` is a safe diagnostic of at most 1024 characters with no secret, credential or URL query; fixtures compare it for presence only. Optional fields: `resource`, `state` (last observed native state), `observations`, `deadline_ms`, for `provider_failed` with `applied` a `cleanup` object `{ "status": "not-attempted" | "succeeded" | "failed" | "pending" | "unknown", "residual": [locator] }` (Capsule M1's `CleanupOutcome`), and for `unknown`/`absent` the required `searched` object of "Reconciliation (M4)", which no other error carries.

### Idempotency

The worker keeps a bounded per-connection table of answered `operation.key` values with their digest and answer. A request with a known key:

- same digest: the stored answer again, with the new `request_id` and `replayed: true`, no provider call (scenario `duplicate_idempotency_key`);
- other digest: `invalid`/`idempotency_conflict`, `effect: "none"`;
- while the first is still in flight: `invalid`/`idempotency_conflict` as well; Fabric never does this on purpose.

The table does not survive the worker. Across worker restarts the profile's `dedupe` says what holds: `provider_key` (the provider deduplicates on the key within its scope), `label_query` (the worker finds a resource it created by its operation label before creating another), or `none` (Fabric must not resend; rule 3).

### `observe` (Fabric to worker)

```json
{ "type": "observe", "request_id": "r-10", "operation_id": "01K8W3A00000000000000000P3", "scope": { "job": "01K8W3A00000000000000000J1", "task": "01K8W3A00000000000000000T1", "attempt": "01K8W3A00000000000000000A1", "generation": "1" }, "action": "sandbox.destroy", "resource": { "...": "locator" }, "wait_ms": 5000 }
```

Reads the state of an earlier operation's resource and waits at most `wait_ms` (1 to `max_observe_wait_ms`) for it to become terminal. It makes no mutating provider call and needs no approval. It answers `result` when the terminal state is observed, `error`/`pending` with the new observation count when it is not, `error`/`unknown`/`observation_failed` when it cannot read, and `error`/`unsupported`/`observe` when the profile says `observe: false` for that action. The `operation_id`, `scope` and `action` must be the original operation's: `scope` is the request's `scope` under the same rules, required (without it the observe is `invalid`/`missing_field`, case `observe_missing_scope`).

The worker reads the resource's owner labels before it reports anything about it. A resource another job made, or no job, is `denied`/`owner_mismatch` with `effect: "none"` for every action, so a destroy observe never settles on another job's resource; a create observe also needs the resource's operation label to be the observing operation (scenario `observe_checks_owner`).

For `sandbox.exec` the `resource` is the exec's target, and the worker answers from what it holds of the exec it sent under that operation id: the exec's own `result` (`completed` or `cancelled`, with its output) once the command ended; `error`/`pending`/`observation_deadline` with `state: "running"` while it runs past `wait_ms`; and `error`/`unknown`/`observation_failed` with the locator for an exec it did not send or no longer holds, as after a worker restart (scenarios `exec_observed_until_exit`, `exec_observe_after_worker_loss`). Nothing runs the command again. An exec still running at `deadline_ms` is answered `result` `running`, since exec is not terminal-only, and this observe is how its end is learned.

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
| `sandbox.exec` | not terminal-only, observe, cancel, approval optional | not terminal-only, observe, no cancel, approval optional |
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

A scenario may carry a `journal_state` header beside `provider_state` (M4): `{ "provider_operations": [ { "operation_id", "key", "digest", "scope", "action", "target", "status", "locator" } ] }`, the provider operations Fabric's journal holds before the first step, beside the scenario's job, task and attempt, as an authority restart leaves them. Fabric's runs write each row as `provider_begin` under the fixture profile and then record its status; the Capsule run ignores the header, since the worker keeps no journal.

The same fixture set is copied byte for byte into Capsule at the same relative path. Each repository has a test that checks its copy against `manifest.sha256` and pins the manifest's own sha256, so a change in one repository fails the other until both carry it. To regenerate the manifest after an edit:

```sh
cd protocol/bridge/v1
find . -type f ! -name manifest.sha256 | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256
shasum -a 256 manifest.sha256
```

Then update the pinned manifest digest in `crates/fabric-client/tests/bridge_fixtures.rs` here and in Capsule's `scripts/bridge-fixtures.test.mjs`, in linked pull requests.

## Versioning

Bridge protocol v1 is independent of the client API version (ADR 0018). Within v1, fixtures only grow: a new action, a new profile or a new optional field arrives with new fixtures and keeps every existing fixture byte-identical. A changed or removed fixture, a new `reject` case for a value v1 accepted, a new error code, or a raised limit is v2. `hello_refused` and its fixtures never change. A profile's own `version` moves when its action table changes meaning.

These rules bind from the first release of v1. Until then v1 exists only on draft pull requests, no Fabric or worker that speaks it has shipped, and an additive or corrective change goes into v1 itself, not into a v2 (decided by Efe on 2026-10-29): it arrives with its fixtures, the regenerated manifest and both repositories' pins in linked pull requests, and both implementations change with it. The first such change made 0 an invalid exec `timeout_ms` and `max_output_bytes`. The second, the same day, made `scope` a required field of `observe`, refused an observe of another job's resource for every action, turned on `observe` for `sandbox.exec` in both profiles, and gave every fixture seed that names a job its operation label (cases `observe_missing_scope` and `observe_exec_valid`; scenarios `exec_observed_until_exit`, `exec_observe_after_worker_loss` and `observe_checks_owner`; existing observe cases and scenarios gained the field). Capsule wrote these fixtures and Fabric copied them byte for byte, with the manifest pinned at `af9a31d70699b1e0fc1ffec8ede50eafbb3bd8fcb35d7b1dd99a9826098d7556` in both. The third, also on 2026-10-29 with ADR 0022 (M4), added the `attach` message, the reasons `unsupported`/`attach` and `unknown`/`absent`, the `searched` field of an `absent` error, the fixture seed status `starting` and the scenario header `journal_state` (section "Reconciliation (M4)": eight cases and six scenarios, no existing fixture changed). Fabric wrote these fixtures ahead of both implementations, which fail on them until they carry M4, and Capsule copied them byte for byte, with the manifest pinned at `f96887e7224320b1e250109202e8920b283b1657b362c4822054fe9c018a7010` in both.

## Fabric's client as built (M3)

`crates/fabric-bridge` is Fabric's side: `wire` (every message, its parser and its encoder), `frames` (the framing above), and `Bridge`, the client. It records into the managed journal through `OperationLog`; schema version 2 adds `provider_operations` for this (docs/managed-jobs.md). The failure rules above become these behaviors, each pinned by a test in `crates/fabric-bridge/tests`.

The client never calls the journal on the async runtime. It reaches its log through a `Recorder`: `LogThread` runs the journal on a thread of its own, so each SQLite transaction and its fsync block that thread and nothing else, and `MemoryLog` (tests and the bench) records in place. A call is queued whole once it was given room: recording a request `dispatched` and queueing its frame are one call, so the record still precedes the frame, and a caller that stops waiting leaves either nothing recorded or the operation recorded and its frame sent. Moving the calls changed none of them: a create, an exec or a timeout is one write to dispatch and one read and one write to settle, as before, each write one committed transaction; each call is now two thread hops (`tests/off_runtime.rs` pins the counts).

### Before anything is written

| Check | When it fails | Journal |
| --- | --- | --- |
| Worker gone, bridge shutting down, `max_in_flight` reached | `Lost`, `ShuttingDown`, `Busy` | nothing; the operation may be sent later |
| Anything the worker would refuse on reading it, checked with the worker-side parser under the negotiated profile: an action not in the profile, a limit over the profile's (deadline, argv, stdin, output), a target that does not fit the profile, any encoding fault such as a NUL in an argument or an exec `timeout_ms` or `max_output_bytes` of 0; and a frame over `max_request_bytes` | `RefusedLocally` with the code and reason the worker would answer | `rejected`, nothing written |
| The journal already holds the key or the id | `Log(Used)` | unchanged; nothing is ever sent twice |
| The attempt is not the job's current live grant at its generation, or the job is closed | `Log(Fenced)` | nothing |
| Otherwise | the operation is committed `dispatched` with its action and its `target` locator, then the frame is written | `dispatched`, naming its target from the start |

`operation.digest` is the caller's: a node computes it with `wire::request_digest`, blake3 under the domain `fabric.bridge.request.v1` over the canonical JSON (keys sorted) of the request without `type`, `request_id`, `operation` and `deadline_ms`. The client sends what it is given and the journal keeps it.

### Answers

| Answer to a `request` | Journal |
| --- | --- |
| `result` `completed` / `cancelled` | `succeeded` / `cancelled`, with the locator |
| `result` `accepted` / `running` (not terminal-only) | stays `dispatched`, with the locator |
| `result` not terminal on a terminal-only action, an answer under another operation id, any `protocol_violation`, `protocol_error`, a refused frame, end of stdout | the worker is lost and terminated (stdin closed, SIGTERM to the worker, SIGKILL to its process group after the grace); every outstanding operation is `unknown`. At the end of stdout alone the worker gets the grace to exit and is not sent SIGTERM; what it left in its group is ended the same way |
| `error` `invalid`, `unsupported`, `denied`, `unavailable` | `rejected` |
| `error` `provider_failed` | `failed`, with the locator when `applied` |
| `error` `pending` | stays `dispatched`, with the answer and the locator |
| `error` `unknown` | `unknown`, with the best known locator |
| `progress`, at most 16 per request with `seq` 1, 2, 3 and so on | the locator is stored; the status stays. Any other `progress` breaks the protocol like the row above, so a worker cannot queue frames without bound or hold a request past its deadline with them |
| no answer within `deadline_ms` plus the grace (2 s by default) | `unknown`. A later answer does not change the status, but the locator it names is recorded, by `record_late` or the next `request`, `observe` or step the journal is handed to, so the operation can be observed by it; the same holds for a request whose caller dropped it |
| the request's own frame not read by the worker within `deadline_ms` plus the grace | the worker is lost and terminated, as for a framing error, since part of the frame may be on its input; every outstanding operation is `unknown` |

Fabric writes frames from one task, whole and in the order they were recorded, each by its request's deadline; `request`, `observe`, `cancel` and `shutdown` record and queue, and never wait on the worker. A caller that stops waiting on one therefore leaves either nothing recorded or the operation recorded and its frame queued whole, never part of a frame, and a worker that stops reading holds nothing past the deadline: not the next request, not `close`, which closes the worker's input once the queued frames are written and stops waiting at the kill grace.

`observe` is sent only for an action the profile observes within its `max_observe_wait_ms`, checked before the journal is read, and only for an operation the journal holds as `dispatched` or `unknown`, by its original id; otherwise it is refused before sending and the journal is unchanged. It carries the scope the journal recorded the operation with, never one a caller names, so the worker judges the resource's owner by the job that made the operation. An exec answered `running` stays `dispatched` with its target and is settled by observing it. It must also name the operation's own action and resource as the journal holds them: the target the operation was dispatched with, or, for an action without one such as a create, the locator its worker reported. Anything else is `ObserveMismatch` before sending, because the answer would settle the operation from another resource's state, such as a destroy recorded done while its target still runs, or one job's create settled on another job's sandbox. A create whose row names no locator cannot be observed and stays `unknown`; M4 finds it by its identity with `attach` (section "Reconciliation (M4)"). Its `result` moves the operation to `succeeded` or `cancelled`, `unknown` moves a `dispatched` one to `unknown`, `pending` stores the locator, and a refused or failed observe changes nothing: it says nothing new about the operation it read. `cancel` of a request that is not outstanding is `not_found`, and of an action the profile does not cancel `unsupported`, both without a frame. A `cancel_ack` changes nothing in the journal. After `shutdown` new requests are refused before the journal is touched.

### The fixtures from Fabric's side

The scenario test runs every scenario against a scripted in-process worker and a real journal holding the fixtures' job, task and attempt. Each `send` is made through the client's API and the frame it writes must equal the fixture after Fabric's own request ids are mapped onto the fixture's. A `send` Fabric refuses itself must refuse with the code and reason, or the cancel state, of the `receive` the worker answers it with. Three kinds of `send` are never written by Fabric, and the worker's answer to them is only parsed; the Capsule run of the same fixtures still exercises them:

- `duplicate_idempotency_key` r-2 and r-3, `secrets_deny_all_and_intersection` r-3 and `fresh_approval_required` r-2 repeat an operation id the journal already holds (r-3 and r-2 of the last two with a new key). Fabric never sends an operation twice; a worker must still answer a repeat as the fixture says.
- `scope_retained` r-2 names job `...J2` with the task and attempt ids of job `...J1`. A journal cannot hold that scope, so Fabric fences it before any effect; the worker refuses it before any effect too (`owner_mismatch`).
- `observe_checks_owner` observes operations the journal does not hold (r-1, and r-2 and r-3 of the seed's creating operation, r-2 under another job's scope). Fabric observes only its own operations, with their own scope, so it refuses each as `NotObservable`; the worker's owner check is what the Capsule run exercises.
- `attach_checks_owner` (M4) attaches operations the journal does not hold, under another job's scope or for another operation's resource. Fabric attaches only its own `unknown` creates, with their own scope and key, so it refuses each as `NotAttachable`; the Capsule run exercises the worker's owner check.

These are observations about the fixtures, not changes: v1's fixtures stay byte for byte as they are.

### Against the Capsule worker

`crates/fabric-bridge/tests/capsule_worker.rs` runs Fabric's side against the real worker built from Capsule's `packages/bridge-worker`, over the worker's own stdio. Its tests are ignored by default, because the worker is built from another repository:

```sh
# in a Capsule checkout
pnpm install --frozen-lockfile
pnpm --filter "@capsule-sdk/bridge-worker..." run build
# here
CAPSULE_BRIDGE_WORKER=<capsule>/packages/bridge-worker/dist/bin.js \
  cargo test -p fabric-bridge --test capsule_worker -- --ignored
```

| Test | What it proves |
| --- | --- |
| every shared scenario over the worker's stdio | each scenario's `send` written with Fabric's framing, each answer read under Fabric's caps, parsed by Fabric's parser and equal to the fixture except `message` strings; kills, restarts and exit statuses are real. `provider_calls`, `sandbox` and `journal` steps belong to the other two runs, and `exec_observed_until_exit` and `pending_destroy_with_locator` stop at their `advance`, because settling is the in-process provider's |
| handshake | the worker sends the v1 fixture profile whole and lowers `max_in_flight` to 4; a hello for versions 2 to 3 is refused with `unsupported_version` |
| create, exec, observe, destroy | each held `dispatched` while in flight and `succeeded` once answered (that the record precedes the write is pinned against the double); a nonzero exit is a completed result; output is capped while captured; a create whose reply is lost is `unknown` until observed by its original id; destroy needs its own grant; every journal row keeps the full scoped locator |
| refusals | policy, missing, out-of-scope and expired grants, another project, another job's sandbox and secrets (deny-all and intersection) each recorded `rejected` with effect `none` and no locator; an exec probe shows the refused create left nothing behind; an action outside the profile, in a request and in an observe, is refused before anything is written |
| a repeated key | refused by the journal before a frame is written, with the same operation id or another |
| observe binding | a pending destroy observed by a locator that is not its target, or as another action, and a lost create observed on another job's sandbox are refused before any frame (`ObserveMismatch`), though the worker would answer `completed`; the destroy stays `dispatched` while its target, read by observe, is still stopping; a destroy whose answer Fabric never read keeps its target through a restart and is observed by it |
| a killed worker | an exec in flight when the worker is killed stays `unknown` with the locator its progress reported; the job, attempt and generation do not change; the restarted worker is never sent the operation again, and observing the exec by its original id is answered `unknown`, since that worker never ran it; a create whose reply was lost is reconciled by `observe`, not created twice |
| the Docker profile | with `CAPSULE_BRIDGE_DOCKER_IMAGE` naming an image the engine already holds: refusals leave no container, a missing image is a definite failure and is not pulled, the container carries the four `fabric.*` labels and the engine scope, an artifact is written and read back within `max_output_bytes`, an exec still running at its `deadline_ms` is answered `running` and settled by observing it, a worker killed mid-exec leaves the container and the exec running while its `docker exec` client ends with the worker's process group, the exec runs once after a restart, and destroy needs its own grant and is observed gone |

CI builds the worker from a pinned Capsule commit and runs these tests on Linux (job `bridge-interop`), where the Docker test skips: no image is on that runner and nothing pulls one. Job `bridge-docker` runs the Docker test against the runner's own engine with the same worker: it pulls one tiny public image by digest first (busybox 1.37.0), because the worker never pulls, and fails if any container with a `fabric.job` label is left. `scripts/bench-bridge.sh` measures the same worker when `CAPSULE_BRIDGE_WORKER` is set.

## Reconciliation (M4)

Status: proposed with ADR 0022, 2026-10-29, as additions to v1 (Versioning). The journal side, the reconciliation state machine, the replacement barrier and the cleanup flow are in docs/managed-jobs.md, section "Reconciliation and cleanup (M4)".

v1 already carried most of what reconciliation needs: the journal's operation id and key on every request, the locator on every effect, `pending` and `unknown` as distinct answers, `observe` by original identity, owner and operation labels on created resources and a per-profile `dedupe` statement. One case had no way back: a create whose answer never reached Fabric, so that Fabric holds no locator to observe it by. `attach` closes it. Reconciliation only reads: Fabric never sends an operation's `request` again, whatever the profile's `dedupe`, and nothing in this section mutates a provider.

### `attach` (Fabric to worker)

```json
{ "type": "attach", "request_id": "r-1", "operation_id": "01K8W3A0000000000000000P30", "key": "key-attach-1", "scope": { "job": "01K8W3A00000000000000000J1", "task": "01K8W3A00000000000000000T1", "attempt": "01K8W3A00000000000000000A1", "generation": "1" }, "action": "sandbox.create", "wait_ms": 1000 }
```

Finds the resource an earlier create of this operation made, by the operation's identity, and reports its state. It makes no mutating provider call, needs no approval and names no resource: an attach with a `resource` is `invalid`/`unknown_field`, because a caller that has a locator observes instead.

| Field | Required | Rule |
| --- | --- | --- |
| `request_id` | yes | as for `request` |
| `operation_id` | yes | the original create's `operation.id` |
| `key` | yes | the original create's `operation.key`, under the `OperationKey` rule |
| `scope` | yes | the original operation's scope, under the request's rules |
| `action` | yes | the original action. Outside the profile it is `unsupported`/`action`; in the profile but not attachable it is `unsupported`/`attach` |
| `wait_ms` | yes | 1 to the profile's `max_observe_wait_ms` (`invalid`/`limit`) |

An action is attachable exactly when its profile entry has `effect: "create"` and `dedupe` `provider_key` or `label_query`. That `dedupe` statement is the profile's claim that the worker can find a resource by the operation that made it after a restart, so no other profile field is needed. In v1 that is `sandbox.create` in both profiles. `sandbox.exec` is never attachable: no exec handle survives the worker, and a `docker exec` instance cannot be attached again after its client ended. `sandbox.destroy` is not a create.

The worker checks in the order of failure rule 1 (frame, JSON, schema, action in the profile), then that the action is attachable and the wait within the limit. Then it looks for the resource at elapsed 0, `observe_interval_ms`, twice that and so on while the elapsed time is below `wait_ms`, as an observe does. A look finds the resource under the profile's `dedupe`: for `provider_key` the resource the provider keys by `key` (the fixture: native id `fx-<key>`); for `label_query` the resource labelled `fabric.operation` = `operation_id` in the worker's bound scope (Docker: `docker ps` with that label filter). A resource the provider has destroyed is not found. Once found, the worker reads its owner labels before it reports anything: a resource whose job label is not `scope.job`, or whose operation label is not `operation_id`, is `denied`/`owner_mismatch` with `effect: "none"`, as for an observe (scenario `attach_checks_owner`). Then it reads its state as an observe of the action does, until the action's terminal state or `wait_ms`. `observations` counts the looks.

| Answer | When | Fabric records (docs/managed-jobs.md) |
| --- | --- | --- |
| `result` `completed`, the locator, the action's output, `replayed: false`, `receipts: []` | the terminal state was observed | `succeeded`, with the locator |
| `error` `pending`/`observation_deadline`, `effect: "pending"`, the locator, `state`, `observations`, `deadline_ms` (the `wait_ms`) | found, not terminal within `wait_ms` | `attached`, with the locator |
| `error` `unknown`/`absent`, `effect: "unknown"`, no `resource`, `observations`, `deadline_ms` (the `wait_ms`) and `searched` | no look found a resource made by this operation | stays `unknown`; `searched` stored |
| `error` `unknown`/`observation_failed`, with the locator when one was found | a look or a read failed | `attached` with a locator, else unchanged |
| a refusal (`invalid`, `unsupported`, `denied`, `unavailable`, all `effect: "none"`) | as for every message | unchanged |

`absent` never settles anything. A create may still land after the worker answered (a Docker create runs in a client of its own that outlives the worker), and a worker bound to another scope finds nothing; so absence is evidence about one scope at one time, and `searched` says which:

```json
{ "provider": "fixture", "scope": { "account": "acct-a", "project": "proj-1" }, "kind": "sandbox" }
```

`searched` is the profile's `provider`, the worker's bound scope (exactly the profile's `scope_keys`) and the action's resource kind. It is required with `absent` (without it, `protocol_violation`/`missing_field`) and refused on every other error (`protocol_violation`/`unknown_field`); an `absent` error with a `resource` is `protocol_violation`/`unknown_field`. Fabric treats as a `protocol_violation` a `result` that is not `completed` answering an attach, a `provider_failed` answering an attach (an attach makes no effect to fail) and an `absent` answering anything but an attach.

An attach counts against `max_in_flight` like an observe, and at `shutdown` an attach still open is answered as an observe is: `pending`/`shutting_down` when its resource was found, else `unknown`/`absent` with the looks made so far.

### Fabric's side

Before anything is sent, `Bridge::attach` refuses with the worker's code and reason an action that is not in the profile or not attachable, or a `wait_ms` over the profile's (`RefusedLocally`, nothing written). Then it reads the journal and refuses with `NotAttachable` an operation that is not `unknown`, that has a target or a locator (it is observed instead), or that was dispatched under another profile. Otherwise it sends the attach with the operation's own `operation_id`, `key`, `scope` and `action` as the journal holds them, never ones the caller names. A worker lost or a deadline passed during an attach changes nothing: it was a read.

### Cleanup on the wire

A cleanup is an ordinary `request` for `sandbox.destroy`: target the create's locator, scope the create's own, `approval` the reference the caller supplied for this cleanup. The worker cannot tell it from any other destroy and needs no new rule: the grant must name `sandbox.destroy` on that resource and must not have expired, a create grant never authorizes it, and the owner labels must name the scope's job (scenario `cleanup_after_attach_needs_fresh_approval`). What makes it a cleanup is on Fabric's side: the journal accepts it after the attempt ended or the job closed, and records which create it cleans.

### Fixture provider additions

- A seed sandbox may have `status: "starting"`: a create the provider accepted and has not finished. It is not terminal for `sandbox.create`, it runs no exec, and `advance: settle` makes it `ready` (scenario `attach_then_observe_until_ready`).
- Its `findCreated` is the `provider_key` look above: `fx-<key>` unless destroyed.

### M4 fixtures

| File | Pins |
| --- | --- |
| `cases/attach_valid.json` | an attach as above is accepted and writes back unchanged |
| `cases/attach_missing_scope.json`, `cases/attach_missing_key.json` | `invalid`/`missing_field` |
| `cases/attach_wait_above_profile.json` | `invalid`/`limit` |
| `cases/attach_with_resource.json` | `invalid`/`unknown_field` |
| `cases/error_unknown_absent_valid.json` | an `absent` answer with `searched` is accepted |
| `cases/error_absent_without_searched.json` | `protocol_violation`/`missing_field` |
| `cases/error_absent_with_resource.json` | `protocol_violation`/`unknown_field` |
| `scenarios/attach_after_lost_create.json` | the journal holds a create `unknown` with no locator; the provider holds the sandbox it made; attach answers `completed` and Fabric records `succeeded`; no provider call |
| `scenarios/attach_absent_stays_unknown.json` | nothing carries the operation's identity, though the job has another sandbox; attach answers `absent` with `searched`; the operation stays `unknown` |
| `scenarios/attach_then_observe_until_ready.json` | attach finds a `starting` sandbox: `pending`, recorded `attached`; after `settle` an observe by the locator answers `completed`, recorded `succeeded` |
| `scenarios/attach_checks_owner.json` | another job's scope, another operation's resource by the same key, and another job's resource are each `owner_mismatch`; the owner's own attach is `completed` |
| `scenarios/attach_unsupported.json` | attach of `sandbox.exec` and `sandbox.destroy` is `unsupported`/`attach`, of an action outside the profile `unsupported`/`action`; no provider call |
| `scenarios/cleanup_after_attach_needs_fresh_approval.json` | after an attach, a destroy under a create grant is `approval_scope`, under an expired destroy grant `approval_expired`, each `rejected` with no call; under a fresh destroy grant it is `completed` and the sandbox is destroyed |

What stays out of v1: retrying a lost create under a provider's own dedupe, attaching to a running execution, and any cleanup Fabric starts by itself. A worker that cannot attach says so through its profile's `dedupe`.
