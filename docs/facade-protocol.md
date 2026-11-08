# Facade protocol v1: remote job operations over the node link

Status: proposed, 2026-11-03 (ADR 0027). This document is the single source of truth for facade protocol version 1. The fixtures in [`protocol/facade/v1`](../protocol/facade/v1) are part of it: where this text and a fixture disagree, that is a bug in one of them, and both clients must fail their fixture tests until it is fixed. The Rust client (`fabric-client`) and the TypeScript SDK (`sdk/typescript`) test against the same files; the manifest is pinned on both sides. The types behind the messages are in [docs/managed-jobs.md](managed-jobs.md), section "Placement and planned continuity (M5)", part "Facade (P07)".

## What the facade is

The facade is how a client reaches a managed job's authority from any registered node of the same owner: `submit`, `inspect`, `watch`, `cancel`, `checkpoint` and `continue`, and nothing else. A client speaks the client API of ADR 0018 to its local node (`facade_call`); the local node forwards the facade message, unchanged, to the authority node over the authenticated node link of ADR 0013 (`RequestBody::Facade`), and returns the authority's answer unchanged. A client on the authority's node takes the same path without the link. The message a client writes and the message the authority reads are the same bytes.

Ownership does not move:

| Concern | Owner |
| --- | --- |
| Token minting and revocation; the journal; placement; grants and generations | the authority node |
| Forwarding a message and its answer; nothing else | the local node |
| Choosing a key for a command, resuming a watch from its cursor | the client |

Out of scope for v1: any public listener, any browser or mobile client, any owner-only socket operation (host paths, launch plans, process registration), minting a token for another owner.

## Failure modes first

1. **The check order is fixed.** The authority checks, in this order and stops at the first failure: frame, JSON, message schema, `facade` version, token known, token not revoked, token not expired, job scope, resource scope, then the message. A message that fails before "the message" answers `refused` and the journal is unchanged.
2. **A command is accepted after durable commit only.** `accepted` is written only once the authority's transaction committed; a disconnect before it leaves the client not knowing, and a repeat with the same key tells it.
3. **Same key, same request: one operation.** A repeat answers the stored answer with `replayed: true`. Same key, another request: `conflict`, nothing written. A key is scoped to the token's actor.
4. **Expected revision is checked by the authority, not the client.** A command naming a revision the job has moved past is `revision_mismatch` with the current one, stored under its key like any refusal.
5. **Disconnect is not cancel.** A command keeps running when the link or the client goes. Cancel is a command with its own key and its own outcome.
6. **Cancel claims nothing about external effects.** It stops the attempt's groups and records the tasks interrupted; a provider operation stays what the journal holds (ADR 0022).
7. **Revocation is immediate.** A revoked or expired token gets `refused` on its next message; an open watch under it ends with its next delivery, with `closed: "revoked"`, within one wait.
8. **A watch never costs the authority memory past the window.** An answer carries at most 256 events. A reader slower than the broadcast window gets `lagged: true` and the cursor to resume from; the gap is read from the journal. The authority never waits on a reader before it commits or hands off.
9. **A refusal is never lowered.** A `continue` whose allowance and target offer no fidelity is `refused` with reason `fidelity`; nothing starts at a lower one.
10. **No socket-only operation has a facade form.** A message type outside the six is `bad_request`/`unknown_variant`; `repository { path }` as a source is `bad_request`/`source_local`.

## Transport

On the node link a facade message travels as the bytes of `RequestBody::Facade` and its answer as `ResponseBody::Facade` (protocol 4, additive). The bounds are the link's: a request at most 1 MiB, an answer at most 16 MiB. On the local socket the same JSON is the `message` field of `facade_call` and of its reply, inside one client API frame (ADR 0018). A watch request's `wait_ms` is at most 25000, so a facade request always fits the link's 30 s reply window.

## Encoding

The rules of the bridge (docs/bridge-protocol.md, Encoding), which are those of ADR 0018 tightened:

- Every object has named fields. A variant is a string. An unknown `type` or `code` is refused (`bad_request`/`unknown_variant` by the authority, `protocol_violation` in a client). An unknown field in an object is refused by the authority (`bad_request`/`unknown_field`) and skipped by a client reading an answer, so an authority may add fields to answers within v1.
- Integers are JSON integers in `0..=2^53-1` unless a field says otherwise (`bad_integer`).
- Revisions, generations and event sequences are canonical decimal strings, as on the client API: `"1"` to `"18446744073709551615"`, never a JSON number.
- Ids (`job`, `task`, `attempt`, `operation`, `token_id`, a node id) are their text forms as the client API writes them. `key` is an `OperationKey`: 1 to 128 bytes of `[A-Za-z0-9._:-]`. `token` is the bearer: 64 lowercase hex characters.
- Optional fields are absent, not `null`, except where a table says a field is required and nullable (`after`, `task`, `pin`), which are written as `null` so the slot is visible.
- Free text (`goal`, `acceptance`, `reason`) holds no control character except `\n` and `\t`, and no single field is over 16 KiB.

## Messages from a client

Every message carries `"facade": 1` and `"token"`. A command carries `"key"`; a command on an existing job carries `"job"` and `"expected_revision"` (required, nullable: `null` means any).

### `submit`

```json
{
  "type": "submit", "facade": 1, "token": "<64 hex>", "key": "key-submit-1",
  "intent": { "goal": "Make the tests pass", "harness": { "kind": "claude_code" }, "acceptance": ["cargo test passes"] },
  "source": { "kind": "git", "uri": "https://example.com/org/repo.git", "commit": "<40 or 64 hex>" },
  "policy": { "drain_budget_ms": 30000, "allowed_fidelity": ["native_session", "new_native_session_with_package"],
              "secrets": [ { "name": "API_TOKEN", "attempt": "any", "resource_kinds": ["node"] } ],
              "export_allowlist": [], "disk_bytes": 2147483648, "pin": null }
}
```

`harness` is the client API's harness form: `{ "kind": "claude_code" }`, `{ "kind": "codex" }` or `{ "kind": "acp", "program": ..., "args": [...] }`. `source.kind` is `git` (with `uri` and `commit`), `snapshot` (with `snapshot`) or `empty`; `repository` is refused as `source_local`. Every `policy` field is optional and takes its default (docs/managed-jobs.md, `JobPolicy`); `allowed_fidelity` is the list of allowed names and may not be empty; `pin` is required and nullable when `policy` is present. Answered with `accepted`. The job is placed or parked by the authority after the commit; `inspect` shows which.

### `inspect`

```json
{ "type": "inspect", "facade": 1, "token": "<64 hex>", "job": "<JobId>" }
```

Answered with `inspection`.

### `watch`

```json
{ "type": "watch", "facade": 1, "token": "<64 hex>", "job": "<JobId>", "after": null, "wait_ms": 10000 }
```

`after` is required and nullable: the last event sequence the client has, or `null` for the job's first event. `wait_ms` is optional, 0 to 25000, default 10000: how long the authority waits for a first new event when none is after the cursor. Answered with `events`.

### `cancel`

```json
{ "type": "cancel", "facade": 1, "token": "<64 hex>", "key": "key-cancel-1", "job": "<JobId>", "expected_revision": "4", "task": null, "reason": "stop" }
```

`task` is required and nullable: `null` cancels the job. Answered with `accepted`.

### `checkpoint`

```json
{ "type": "checkpoint", "facade": 1, "token": "<64 hex>", "key": "key-cp-1", "job": "<JobId>", "expected_revision": null }
```

Answered with `accepted`. Refused with reason `not_live` when no attempt of the job is granted or running.

### `continue`

```json
{ "type": "continue", "facade": 1, "token": "<64 hex>", "key": "key-cont-1", "job": "<JobId>", "expected_revision": "6",
  "fidelity": ["native_session", "new_native_session_with_package"], "pin": null }
```

`fidelity` is optional and defaults to the job's allowance; empty is `bad_request`/`empty_fidelity`. `pin` is required and nullable; a pin outside the token's resource scope, or no pin under a token limited to listed nodes, is `refused`/`scope`/`resource`. Answered with `accepted`.

## Messages from the authority

### `accepted`

```json
{ "type": "accepted", "facade": 1, "operation": "<OperationId>", "key": "key-submit-1", "replayed": false, "job": "<JobId>", "revision": "1" }
```

`revision` is the job's revision after the command. `replayed` is `true` when the key was seen before and this is its stored answer again.

### `inspection`

```json
{ "type": "inspection", "facade": 1, "job": { "...": "the client API's ManagedJob, each task with its class and last receipt" },
  "parked": false, "placements": [ { "seq": "3", "kind": "initial", "chosen": "<NodeId>", "candidates": [ { "node": "<NodeId>", "verdict": { "eligible": { "rank": ["bundle_chain", { "load": 120 }, "identity"] } } } ] } ],
  "continuations": [], "notes": [], "provider_operations": [], "blockers": [], "residuals": [] }
```

One object, the same the client API's `managed_inspect` answers. An exclusion is `{ "excluded": { "reasons": [ { "secret": { "name": "API_TOKEN" } }, "no_answer" ] } }`. The fixture `inspection_valid` is the smallest complete one; a client keeps a status, verdict or reason it does not know as text.

### `events`

```json
{ "type": "events", "facade": 1, "job": "<JobId>",
  "events": [ { "seq": "7", "revision": "3", "at_ms": 1762128000000, "event": { "kind": "attempt_observed", "...": "the event as the client API writes it" } } ],
  "next": "7", "lagged": false, "closed": null }
```

At most 256 events, in sequence order, all after `after`. `next` is the sequence to ask after on the next call, the last event's or, with no event, the one asked. `lagged: true` says the reader fell behind the broadcast window and must continue from `next`, which is always right; nothing was lost, only not delivered in this answer. `closed` is required and nullable: `null`, `"revoked"` (the token was revoked or expired while the watch waited), or `"finished"` (the job is finished or cancelled and no event will follow).

### `refused`

```json
{ "type": "refused", "facade": 1, "code": "scope", "reason": "job", "message": "scope: job", "current_revision": "5" }
```

`code` and `reason` are what a client switches on; `message` is for a person and is never compared by a fixture. `current_revision` is present exactly for `revision_mismatch`.

| `code` | `reason` | Meaning |
| --- | --- | --- |
| `unsupported_version` | `facade` | the `facade` field is not 1 |
| `bad_request` | `unknown_variant`, `unknown_field`, `bad_integer`, `bad_id`, `bad_string`, `missing_field`, `source_local`, `empty_fidelity`, `wait_above_limit` | the message does not parse under this document |
| `unauthorized` | `unknown_token`, `revoked`, `expired` | the token, in the check order |
| `scope` | `job`, `resource` | the token does not cover the job, or the pin or target node; a token limited to listed jobs may not `submit` (a new job is in no list), and a token limited to listed nodes may not `submit` or `continue` without a pin to one of them (the authority could choose any node) |
| `unknown_job` | `job` | the job is not in this authority's journal |
| `conflict` | `key` | the key was used for another request |
| `revision_mismatch` | `revision` | `expected_revision` is not the job's current one |
| `refused` | a domain refusal's snake-case name (`attempt_still_live`, `task_closed`, `replacement_blocked`, `fidelity`, `not_live`, `unsupported_source`, `bad_policy`, ...) | the authority's domain refused the command; stored under the key |
| `unavailable` | `authority`, `link` | the authority is not open or the link to it failed; nothing is known about the command, and a repeat with the same key tells |

## Placements in an inspection

`placements[]` is `{ "seq", "kind", "chosen", "candidates": [ { "node", "verdict" } ] }`, with `kind` one of `initial`, `planned_handoff`, `failover` and `chosen` the node or `null`. A verdict is `{ "eligible": { "rank": [...] } }` or `{ "excluded": { "reasons": [...] } }`. A rank reason is a name, `current_healthy`, `bundle_chain`, `workspace` or `identity`, or `{ "load": <thousandths> }`. An exclusion reason is the name `no_answer`, or an object keyed by its name with the values compared: `{ "stale": { "age_ms" } }`, `{ "platform": { "need": { "os", "arch" }, "have": { ... } } }`, `{ "harness_missing": { "harness": <the client API's harness form> } }`, `{ "harness_version": { "harness": ..., "need", "have" } }`, `{ "layout": { "need": { "home", "root" }, "have": { ... } } }`, `{ "secret": { "name" } }`, `{ "disk": { "need", "free" } }`, `{ "pin": { "pinned": <node> } }`. A client keeps a reason it does not know as it came. The same forms are in `protocol/fixtures/reply/managed_inspect.json`.

## As built (M5)

Built on 2026-11-03 by owner B against the text above; the fixtures are unchanged. A message that is not JSON is `bad_request`/`bad_string`. A command's key is stored on the authority under the token's actor (ADR 0027, As built). `events.next` is `null` only when the job has no event after a `null` cursor; a watch with nothing after its cursor on a finished or cancelled job answers `closed: "finished"`. A `checkpoint` of a job with no attempt at all is `refused`/`not_live` without a stored key. A `continue` acts on the live attempt's task, or the primary task when none is live. The local node forwards a facade message over the link with a deadline of the message's `wait_ms` (the default when absent) plus five seconds, and answers `unavailable`/`link` when the link fails.

## Fixtures

[`protocol/facade/v1`](../protocol/facade/v1) holds one kind of file and a manifest:

| Path | Content | What each client checks |
| --- | --- | --- |
| `cases/*.json` | one `message`, its `direction` (`client_to_authority` or `authority_to_client`), an optional `given` (the authority's state before the message: `tokens`, `jobs`, `operations`, `events`), and `expect` | the parser on the receiving side accepts or refuses the message as `expect` says; an accepted message writes back to the same JSON. For a `client_to_authority` case with `given`, the authority test seeds that state and compares the answer field by field with `expect.answer`; a field the fixture leaves out of `answer` is not compared (an id the authority makes, `message`) |
| `manifest.sha256` | `shasum -a 256` lines of every other file, sorted by path | both clients check every file against it and that no file is missing or extra, and pin the manifest's own digest |

`expect` is `{ "outcome": "accept" }`, optionally with `answer`, or `{ "outcome": "reject", "code": "...", "reason": "..." }`; for a rejected `client_to_authority` case the authority answers `refused` with that code and reason, and for a rejected `authority_to_client` case the client's parser refuses the frame with that code. `given.tokens[]` is `{ "token_id", "bearer", "actor", "jobs": "any" | [ids], "resources": "any" | [node ids], "expires_ms", "revoked_ms" }`; `given.now_ms` is the authority's clock for the case; `given.jobs[]` is `{ "job", "revision", "state", "live_attempt": bool }`; `given.operations[]` is `{ "key", "actor", "digest_of": <the message that made it> }`; `given.events` is `{ "head": "<seq>" }`.

To regenerate the manifest after an edit:

```sh
cd protocol/facade/v1
find . -type f ! -name manifest.sha256 | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256
shasum -a 256 manifest.sha256
```

Then update the pinned digest in `crates/fabric-client/tests/facade_fixtures.rs` and in `sdk/typescript/test/facade_fixtures.test.ts`, in the same commit.

## Versioning

Facade protocol v1 is independent of the client API version (ADR 0018) and of the bridge (ADR 0021). Within v1, fixtures only grow: a new optional field, a new reason or a new answer field arrives with new fixtures and keeps every existing fixture byte-identical. A changed or removed fixture, a new `reject` case for a message v1 accepted, a new `code` or a raised limit is v2. Until v1 first ships it exists on draft pull requests only, and an additive or corrective change goes into v1 itself under the rule Efe set for the bridge on 2026-10-29: it arrives with its fixtures, the regenerated manifest and both pins in the same commit. The first fixture set was written 2026-11-03 with this document, ahead of both clients, which fail on it until they carry M5.
