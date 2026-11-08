# Portable Session Format v1

Status: proposed 2026-11-08 (ADR 0029); the fixtures in [`protocol/psf/v1`](../protocol/psf/v1) are part of this document. Where the text and a fixture disagree, one of them has a bug, and every reader must fail its fixture test until both agree. v1 takes corrections until a reader first ships; after that, the rules under Compatibility apply.

The Portable Session Format (PSF) is the public form of what Fabric already moves between two machines: one coding-agent job, its workspace revision, the caches that travel with it, the agent's native session as opaque bytes, and, for a managed job, the work package of ADR 0024. It is a container any program can write and any program can read without Fabric's crates, with no signature, no vendor's blessing and no secret inside. Fabric's node is one writer and one reader of it; the format is not Fabric's internal store, and it does not depend on the M5 branch.

## 1. The hard part first

1. **Nothing a coding agent writes today is portable by itself.** Every harness keys its session store to the absolute working directory or a hash of it, writes `cwd` into the records, and most write absolute paths into tool inputs and outputs. Claude Code signs its thinking blocks, so an edited transcript is refused on the next turn. PSF therefore never rewrites a native session. It carries the bytes opaquely, records the root and home they are bound to, and lets the reader decide what fidelity it can achieve there (ADR 0011).
2. **A session is not a workspace.** `claude-sync`, `claude-handoff` and `codex-claude-transfer` move transcripts and leave the dirty tree, the ignored caches and the question of two live copies to the user. PSF puts the workspace revision first: a bundle head and a ledger head name one exact state, and the session is an attachment to it.
3. **No secrets, by construction and by check.** A document declares the one policy v1 knows, `never_inside`, names what the writer left behind, and a reader refuses any text field that has a known token shape. This is a guard, not a scanner (the same rule as `fabric_core::credential`): a value in a file no pattern names travels as the file does, and a reader says so.
4. **Fidelity is derived, never free.** A writer offers what its attachments can give; a reader achieves at most that, and less when a bound attachment lands on another root or home. The four levels are the ladder of ADR 0024: `native_session`, `new_native_session_with_package`, `workspace_only`, `unsupported`.
5. **One document, one byte form.** The manifest is canonical JSON with the rules of the M5 canonical writer and the bridge fixtures: sorted keys, no whitespace, every integer a decimal string. Its identity is the sha256 of its bytes, which a standard library can compute.

## 2. What the harnesses store today

Read on 2026-11-08 from the installed tools on one Mac (read only) and from each vendor's documentation. Nothing listed here signs its session files. "Bound" means the store or the records name the working directory, so the bytes resume only under the same path.

| Harness | Where | Format | Bound to | Resume surface | Export or transfer today |
|---|---|---|---|---|---|
| Claude Code 2.1.294 | `~/.claude/projects/<cwd with non-alphanumerics as `-`>/<session uuid>.jsonl`; sidecars `<session>/subagents/*.jsonl`, `<session>/tool-results/`, `file-history/<session>/`, `plans/`, `tasks/`; registry `sessions/<pid>.json` | JSONL; records carry `type`, `uuid`, `parentUuid`, `sessionId`, `cwd`, `version`, `gitBranch`, `timestamp`; thinking blocks carry a model signature | the project key and `cwd`; the Keychain login is per config directory and never travels | `claude --resume <id>`, `--continue`, `--fork-session`; files age out after `cleanupPeriodDays` (30) | `claude-sync` (whole `~/.claude` scopes, gzip and age, `${HOME}` token rewriting); `claude-handoff` (`.claude-shared/sessions/<id>/` or one checksummed `.handoff`, `{{PROJECT_ROOT}}` placeholders, redaction at export); OpenAI's `codex transfer` (Claude to Codex only, through `externalAgentSessionImport`) |
| Codex 0.163.0 | `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<thread uuid>.jsonl`, `archived_sessions/*.jsonl.zst`, `session_index.jsonl`, `thread_history_1.sqlite`; `external_agent_session_imports.json`, `vendor_imports/` | JSONL; first record `session_meta` with `cwd`, `cli_version`, `originator`, `source`, `model_provider`, `runtime_workspace_roots`; then `turn_context`, `response_item`, `event_msg` | `cwd` and the workspace roots; `--last` is scoped to the current directory unless `--all` | `codex resume <id>`, `codex fork <id>`, `codex exec resume`, `--ephemeral` writes nothing | `codex-claude-transfer` (`cct export` and `import --to claude` or `codex`, local only) |
| Cursor Agent 2026.10.01 | `~/.cursor/chats/<workspace hash>/<agent uuid>/{meta.json,store.db}`; `~/.cursor/acp-sessions/<uuid>/meta.json` | `meta.json {schemaVersion: 1, cwd, createdAtMs, updatedAtMs, hasConversation}`; SQLite `blobs(id, data)`, `meta(key, value)`; undocumented | the workspace hash and `cwd` | `--resume [chatId]`, `--continue`, `agent ls` | none documented |
| OpenCode 1.18.34 | SQLite `~/.local/share/opencode/opencode.db` (`opencode db path`), tables `session` (`id` `ses_...`, `project_id`, `directory`, `version`), `message`, `part`, `project` (`worktree`) | SQLite rows with JSON columns | `directory` and the project worktree | `--continue`, `--session <id>`, `--fork` | `opencode export [id]` to JSON (`--sanitize`), `opencode import <file>` |
| Gemini CLI | `~/.gemini/tmp/<project hash>/chats/`, checkpoints `~/.gemini/tmp/<project hash>/checkpoints` plus a shadow git repository `~/.gemini/history/<project hash>` | JSON | the project hash | `--resume`, `/chat save <tag>`, `/restore`; 30 day retention | none; the docs say the CLI was replaced by Antigravity CLI on 2026-06-18 |
| Amp | the thread lives at `ampcode.com/threads/T-...`; local cache path unconfirmed | server side | not bound: "a thread is not tied to the device you started it on" | `amp threads continue T-...` | `amp threads export`, `markdown`; JSON from the web |
| pi 0.80.3 | `~/.pi/agent/sessions/<cwd with `/` as `-`, wrapped in `--`>/<ISO time>_<uuid>.jsonl` | JSONL; first record `{type: "session", version: 3, id, timestamp, cwd}`, then entries with `id` and `parentId` (a tree: `model_change`, `thinking_level_change`, `message`) | the session directory and `cwd` | `--continue`, `--resume` | none documented |
| Any ACP agent | the agent's own store | the agent's own | `cwd` is a parameter of `session/load` | `session/new`, `session/load` (replays history as `session/update`, needs `loadSession`), `session/resume` (no replay, `sessionCapabilities.resume`), `session/list` (`cwd` filter), `session/close` | none: ACP defines no export, import or transfer of session state (PR 6 proposes one) |

What Fabric's store already captures (ADR 0014, ADR 0015): a `SnapshotManifest` (format 3) with the root, the platform, the git state as a bundle plus two private commits, the ignored files by class with their carry keys and remake steps, the harness state as an opaque tree, and the paths left behind; a hash-chained ledger whose records are `Created`, `MoveStarted`, `MoveImported`, `MoveAborted`, `Stolen`, `SnapshotPublished`, `Suspended`, `Resumed`, `EnvSpecChanged`, `DecisionRecorded` and `Note`; and per harness a `StateManifest` (format 1) with the kind, the session id, the root and home the tree was captured under, the harness version, and every file with its digest and the redactions applied. PSF projects these: the snapshot id is the bundle head, the last record's hash is the ledger head, the harness state tree is one attachment, and the cache rows become `caches`.

## 3. The container

A PSF container is a directory:

```text
<name>.psf/
  manifest.json        the manifest, canonical JSON, required
  blobs/<sha256 hex>   zero or more blobs the manifest names, each by the sha256 of its bytes
```

- `manifest.json` alone is a valid container (and a valid document when sent on its own). Its identity, the **document digest**, is the sha256 of its bytes.
- A blob is present when `blobs/<digest>` exists. A reader checks every present blob's bytes against its name and its declared `bytes`, and refuses a blob the manifest does not name. A blob the manifest names and the directory lacks is **detached**: the document is read, the reader lists the attachment as detached, and the fidelity it can achieve is computed without that attachment.
- Nothing else is in the directory. A packed form (one archive of the directory) is out of v1; a writer that wants one archives the directory as it is, and the reader unpacks first.
- Workspace bytes do not travel in the container. `workspace` names a revision by its bundle head and ledger head, and `caches` name what the revision holds; a reader obtains the bytes from a Fabric store that holds the heads, or from the `source` binding plus a rebuild. This is the same split as a move: the manifest is small and self-describing, the bytes go over a transfer that checks them by digest.

## 4. Canonical JSON

The manifest's bytes are the canonical JSON of its value. The rules are those of the M5 canonical writer (`crates/fabric-core/src/managed/canon.rs` on the M5 branch) and the bridge fixtures, so a WorkPackage pasted in as a section keeps its own bytes:

1. UTF-8, no byte order mark, one JSON object, no bytes before or after it (a trailing newline is not canonical).
2. No whitespace outside strings.
3. Object keys are unique, ASCII `[a-z0-9_]`, and sorted by their bytes. A key given twice is not canonical.
4. **No JSON numbers.** Every integer is a decimal string: `"0"`, or `[1-9][0-9]*` up to `18446744073709551615`. No sign, no leading zero, no fraction, no exponent. (The counters rule of the client API and the bridge.)
5. Booleans are `true` and `false`. An optional value that is absent is `null`; every field this document defines for v1.0 is present.
6. Strings escape `"` as `\"`, `\` as `\\`, newline as `\n`, carriage return as `\r`, tab as `\t`, every other control character below U+0020 as `\u00xx` with lowercase hex, and nothing else. Non-ASCII is literal UTF-8. `/` is not escaped. No `\b`, `\f` or `\uXXXX` for anything else.
7. Digests are 64 lowercase hex characters. The algorithm is given by the field: a store reference (`bundle_head`, `ledger_head`, `source.snapshot`, `caches[].digest`, `evidence[].digest`, everything inside `package`) is blake3 as Fabric writes it; a blob (`attachments[]`) names its `algorithm`, `sha256` in v1.
8. Ids keep Fabric's text forms: a job, attempt or task id is a ULID in uppercase Crockford base32, 26 characters; a node id is `fab1` and 52 characters of lowercase base32; a session id is 1 to 128 characters of `[A-Za-z0-9._:-]` not starting with `-` or `.`; a git commit is 40 or 64 lowercase hex characters.
9. Paths keep Fabric's rules (`fabric_core::path`): an absolute path starts with `/`; a relative path does not; no empty, `.` or `..` component, no `.git` component in any case, NFC normalized, no NUL.

A reader checks canonical form by re-serializing the value it parsed under these rules and comparing bytes. A document that parses but is not byte-identical is refused `not_canonical`, because it would have a second name.

## 5. The manifest

Top level, keys in canonical order. R means required and never `null`; N means required and nullable.

| Key | | Type | Meaning |
|---|---|---|---|
| `attachments` | R | array | native session attachments, section 5.5; may be empty |
| `caches` | R | array | the carried caches, section 5.4; may be empty |
| `evidence` | R | array | accepted evidence by digest, section 5.7; may be empty |
| `fidelity` | R | object | the offered fidelity, section 6 |
| `harness` | R | object | the harness the job ran under, section 5.3 |
| `job` | R | object | job identity, section 5.1 |
| `package` | N | object | the WorkPackage v1, section 9, or `null` |
| `produced` | R | object | who wrote the document and when, section 5.1 |
| `psf` | R | string | the format version, `"1"` |
| `requires` | R | array of strings | extensions the reader must understand, section 11; empty in v1.0 |
| `secrets` | R | object | the secrets policy, section 8 |
| `source` | R | object | the source binding, section 5.2 |
| `workspace` | R | object | the workspace revision, section 5.2 |

### 5.1 `job` and `produced`

| Key | | Type | Meaning |
|---|---|---|---|
| `job.id` | R | ULID | the job (ADR 0001) |
| `job.goal` | R | text | the goal in the user's words; a text field (section 7) |
| `job.created_by` | R | node id | the node that created the job |
| `produced.at_ms` | R | integer string | Unix milliseconds when the document was written; informational |
| `produced.by` | R | node id | the node that wrote it; for a writer that is not a Fabric node, a node id it minted for itself |
| `produced.writer` | R | string, 1 to 128 bytes | the writing program and version, such as `fabric/0.1.0` or `claude-handoff/0.3.0` |

### 5.2 `workspace` and `source`

| Key | | Type | Meaning |
|---|---|---|---|
| `workspace.root` | R | absolute path | the directory the revision was captured in; the target root on every node under ADR 0011 |
| `workspace.bundle_head` | R | blake3 hex | the `SnapshotId` at the head of the bundle chain: the snapshot whose manifest holds the git bundle, the index and working-tree commits, the files and the cache rows |
| `workspace.ledger_head` | R | blake3 hex | the hash of the job's latest ledger record when the revision was taken (`RecordHash`); the pair is M5's `WorkspaceRevision` |
| `workspace.epoch` | R | integer string, at least `"1"` | the lease epoch at the revision (ADR 0003) |
| `workspace.platform.os` | R | string | the capturing node's operating system name, such as `macos` |
| `workspace.platform.arch` | R | string | its architecture, such as `arm64` |
| `source.kind` | R | one of `git`, `snapshot`, `empty` | where the workspace came from (ADR 0025); a `repository { path }` on the writer's host names nothing elsewhere and is not a v1 kind |
| `source.uri`, `source.commit` | R for `git` | string; commit hex | the repository and the exact commit, never a branch or tag |
| `source.snapshot` | R for `snapshot` | blake3 hex | the published snapshot the job started from |

The two heads name one exact state. A reader that holds a Fabric store looks the bundle head up and walks the chain; one that does not can still check that two documents describe the same revision, and can rebuild from `source` what `caches` say was not carried.

### 5.3 `harness`

| Key | | Type | Meaning |
|---|---|---|---|
| `harness.kind` | R | `[a-z][a-z0-9_]{0,31}` | the harness the job ran under, in the client API's spelling: `claude_code`, `codex`, `acp`, and for other agents a name the writer chooses from the same alphabet, such as `cursor_agent`, `opencode`, `gemini_cli`, `amp`, `pi`. The set is open: a reader that has no adapter for a kind reads the document and achieves `workspace_only` |
| `harness.program`, `harness.args` | N; R | string or `null`; array of strings, at most 64 | for `acp`, the program looked up on `PATH` and its arguments (ADR 0006); `program` is `null` for every other kind |
| `harness.version` | N | string or `null` | the harness version that last wrote the session, as the harness prints it (`2.1.294`, `0.163.0`); `null` when unknown. A reader whose harness is older refuses to continue the session natively (gate finding F10) and falls to the next fidelity |

### 5.4 `caches`

One row per cache root the snapshot manifest holds (ADR 0012, ADR 0014), sorted by `path`, no duplicates, at most 256.

| Key | | Type | Meaning |
|---|---|---|---|
| `path` | R | relative path | the cache root under `workspace.root` |
| `class` | R | one of `deps`, `build`, `tool_cache` | its class |
| `carry` | R | one of `carried`, `partial`, `not_carried` | whether its bytes are in the revision; `partial` is read as missing |
| `key` | N | object or `null` | the carry key, exact strings the source measured: `{"kind": "node", "node_major", "package_manager", "pm_major", "pnpm_store_path"}`, `{"kind": "rust", "rustc_vv", "cargo_home"}`, `{"kind": "python", "python_version", "python_home"}`, `{"kind": "other", "tool", "version"}` or `{"kind": "unmeasured"}`. A reader compares a key for equality with its own and never interprets it; an unknown `kind` compares unequal |
| `digest` | N | blake3 hex or `null` | the blake3 of the canonical JSON of the cache's entries as the snapshot manifest lists them (`rel_path`, kind, size, chunk ids in order), so two documents can say "the same bytes" without the bytes; `null` when `carry` is `not_carried` |

### 5.5 `attachments`

One entry per opaque blob a harness or a converter can read, sorted by `name`, no duplicates, at most 64.

| Key | | Type | Meaning |
|---|---|---|---|
| `name` | R | string, 1 to 128 bytes of `[A-Za-z0-9._-]` | the attachment's name in this document |
| `harness` | R | harness kind | the harness that can read it |
| `format` | R | string, 1 to 128 bytes | what the bytes are: `fabric.harness_state.v1` for a Fabric harness state tree (a tar of the tree with its `manifest.json`), `opencode.export.v1`, `claude.transcript.jsonl.v1`, and so on. A reader that does not know a format cannot use the attachment |
| `fidelity` | R | `native_session` or `new_native_session_with_package` | what the attachment gives when the reader can use it: the same session continues, or a new session is started from it and the package |
| `session` | N | session id or `null` | the harness's own id for the session: a Claude session UUID, a Codex thread id, an OpenCode `ses_...`, an Amp `T-...`, an ACP `sessionId` |
| `bound` | N | `{"root": abs, "home": abs}` or `null` | the workspace root and home the bytes were captured under, when the harness keys its store to them (every harness in section 2 but Amp); `null` for bytes that are not path-bound |
| `algorithm` | R | `sha256` | the digest algorithm of the blob; a closed set in v1 |
| `digest` | R | hex | the blob's digest; its file name under `blobs/` |
| `bytes` | R | integer string | the blob's length; a reader allocates nothing from it and checks a present blob against it |

### 5.6 Rules across sections

- A `native_session` attachment has `harness` equal to `harness.kind`; one for another harness can only be `new_native_session_with_package`.
- `fidelity.level` is the highest `fidelity` among the attachments, `workspace_only` when there are none, except that a writer may declare `unsupported` with a reason when it has no attachment (section 6). Any other combination is `fidelity_mismatch`.
- When `fidelity.level` is `new_native_session_with_package`, `package` is not `null`: the package is what the new session is handed.
- When `package` is not `null`: `package.version` is `"1"`, `package.job` equals `job.id`, and `package.workspace.bundle_head` and `package.workspace.ledger_head` equal the manifest's. Otherwise `package_mismatch`.

### 5.7 `evidence`

Accepted evidence by digest, never the evidence itself (ADR 0023, ADR 0024): `{"attempt": ULID, "digest": blake3 hex, "kind": "checkpoint" | "exit" | "receipt"}`, at most 1024 entries, in the writer's order (this attempt's first). A standalone job has none.

## 6. Fidelity

The ladder is ADR 0024's, with its stored names:

| Level | Offered when | A reader achieves it when |
|---|---|---|
| `native_session` | an attachment of the job's harness carries the captured session | it has that harness, not older than `harness.version`, the attachment is present and of a format it reads, and the restore root and home equal `bound` (ADR 0011) |
| `new_native_session_with_package` | an attachment can seed a new session and `package` is present, or a `native_session` attachment exists and the reader cannot use it natively | it has any harness that takes the package and, if used, the attachment |
| `workspace_only` | always, once the revision is restorable | it can restore the revision; no session is started |
| `unsupported` | the writer could not capture, with the reason in `fidelity.reason` | never; a reader reports the reason |

`fidelity` is `{"level": <name>, "reason": null}` for the first three and `{"level": "unsupported", "reason": <text>}` for the last. A document at `unsupported` has no attachments. The reader's result names both the offered level and the achieved one, and why it fell, so a user sees "offered native_session, achieved workspace_only: the attachment is bound to /Users/me and this node's home is /home/me".

## 7. Limits

| What | Limit | Why |
|---|---|---|
| `manifest.json` | 1,048,576 bytes | the client API's request parse cap; a reader refuses longer bytes unparsed |
| any text field (`job.goal`, `fidelity.reason`, and every string inside `package` the package rules name) | 16,384 bytes of UTF-8 | `TEXT_MAX_BYTES` of the work package |
| `package` | 262,144 bytes of canonical JSON | `PACKAGE_MAX_BYTES` |
| `produced.writer`, `attachments[].name`, `attachments[].format` | 128 bytes | names, not prose |
| `attachments` | 64 entries | |
| `caches` | 256 entries | |
| `evidence`, `secrets.left_behind`, `secrets.allowlist` | 1,024 entries each | |
| `requires` | 16 entries | |
| `harness.args` | 64 entries | |

A limit is a refusal (`too_large`), never a truncation. A writer that would exceed one leaves the field out of the document (a goal cut to 16 KiB with a note, evidence by the newest 1,024) and says so in the document's own text, not silently.

## 8. Secrets

`secrets` is `{"allowlist": [relative paths], "left_behind": [relative paths], "policy": "never_inside"}`.

- `policy` has one value in v1, `never_inside`. A document with another value is refused `secret_policy` before anything else in it is trusted. This is deliberate: a later version that allowed a secret inside would be v2, and a v1 reader must not read such a document under v1 rules.
- `left_behind` lists the workspace paths a secret pattern or `.fabricignore` kept on the writing node (the snapshot manifest's `left_behind`), so the reader can tell the user what to recreate. `allowlist` lists the paths the job allowed to travel although a pattern matched them (ADR 0025); they are in the revision, named, not hidden.
- A reader refuses `credential_like` any text field that holds a token with one of the shared prefixes and bodies: `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` (36 or more alphanumerics), `github_pat_` (22 or more of `[A-Za-z0-9_-]`), `glpat-` (20 or more), `npm_` (36 or more alphanumerics), `xoxb-`, `xoxp-` (10 or more of `[A-Za-z0-9_-]`), `sk-` (20 or more), `AKIA` (16 or more of `[A-Z0-9]`), `sk_live_`, `sk_test_`, `rk_live_`, `rk_test_` (24 or more alphanumerics), `AIza` (35 or more of `[A-Za-z0-9_-]`), `hf_` (34 or more alphanumerics), plus a URL whose userinfo holds a password and a PEM private key header. This is `fabric_core::credential::looks_like_credential`, and the three readers implement the same list; the fixture `refuse_credential_like` pins one shape.
- No reader promises to find a secret inside an attachment. The attachment's own writer redacts (Fabric's harness capture redacts by pattern and records the redactions in the state manifest); PSF records nothing about the blob's contents but its digest and length.
- A writer never puts a login, a credentials file, a Keychain item, `~/.claude.json`, `auth.json` or `config.toml` into an attachment. Fabric's denylist (ADR 0006) is the rule for its own attachments; another writer's attachment is trusted as far as its format's documentation says.

## 9. The WorkPackage section

`package` is the WorkPackage v1 of ADR 0024 (`crates/fabric-core/src/managed/package.rs` on the M5 branch), as one section, byte for byte the canonical JSON that `WorkPackage::canonical_json` writes: keys `artifacts`, `decisions`, `evidence`, `intent {acceptance, goal, harness}`, `job`, `next_actions`, `questions`, `revision`, `tasks`, `version`, `workspace {bundle_head, ledger_head}`, every number a decimal string, every digest bare blake3 hex. Two things inside it keep M5's spelling, not this document's: `intent.harness` is `HarnessKind`'s display form (`claude-code`, `codex`, `acp:<program> <args>`), and `tasks[].receipt` is `null` or the receipt object (`task`, `class`, `status`, `snapshot`, `reason`, `exit`, `interrupted`, `workspace`). PSF does not redefine any of it; it pins the section to the manifest by the rules of section 5.6 and its size by section 7. The package digest of ADR 0024 (blake3 under `fabric.managed.package.v1\0`) is therefore computable from the section alone.

The section is optional because a standalone job (M1) has no package, and because a document a converter writes from another harness's export has one only when the converter can state the intent and tasks. Without it, `new_native_session_with_package` is not available (section 5.6).

## 10. What a reader must refuse

A reader checks in this order and stops at the first failure. Each code is closed in v1; a new code is a v2 change. The `field` is a dotted path with `[i]` for an array index, or empty for the whole document.

| Code | When |
|---|---|
| `too_large` | the bytes are over 1,048,576 before parsing; a text field, a list or the package is over its limit (section 7) |
| `not_json` | the bytes are not UTF-8 or not one JSON object |
| `not_canonical` | the value re-serialized under section 4 is not the bytes read: whitespace, key order, a duplicate key, a JSON number, an escape outside the rules, bytes after the object |
| `unsupported_version` | `psf` is not `"1"` |
| `missing_field` | a required key is absent, or a required non-nullable value is `null` |
| `unknown_requirement` | `requires` names an extension this reader does not implement (section 11) |
| `bad_id` | a ULID, node id, session id, commit or digest is not in its text form (section 4, rules 7 and 8) |
| `bad_path` | a path breaks section 4, rule 9, or `bound.root` or `bound.home` is not absolute |
| `bad_enum` | a closed-set value is outside its set: `source.kind`, `caches[].class`, `caches[].carry`, `attachments[].fidelity`, `attachments[].algorithm`, `evidence[].kind`, `fidelity.level`, or `harness.kind` outside its alphabet |
| `duplicate` | two attachments share a `name`, two caches share a `path`, or a list that must be sorted is not |
| `secret_policy` | `secrets.policy` is not `never_inside` |
| `credential_like` | a text field holds a token shape of section 8 |
| `fidelity_mismatch` | section 5.6: the level is not the highest attachment's, `unsupported` with attachments or without a reason, a reason given below `unsupported`, a `native_session` attachment for another harness, or `new_native_session_with_package` without a package |
| `package_mismatch` | section 5.6: the package's version, job or workspace heads differ from the manifest's |

Container level, after the manifest was read:

| Code | When |
|---|---|
| `manifest_missing` | the directory has no `manifest.json` |
| `blob_mismatch` | a file under `blobs/` does not hash to its name, or its length is not the `bytes` the manifest declares |
| `blob_unlisted` | a file under `blobs/`, or anywhere else in the directory, that the manifest does not name |

An absent blob is not a refusal (section 3): the attachment is reported detached.

A reader never repairs a document: no key order fix, no number to string, no path rewrite. A writer that wants its document read writes it canonically.

## 11. Compatibility

- **v1 grows by addition.** A later v1 writer may add a key to any object, a value to a `key` kind, a `format` name, or a `harness.kind` name. A v1 reader ignores a key it does not know, except that it must still re-serialize the document byte for byte to check canonical form, so unknown keys take part in the canonical check with their values as read. A key added after v1.0 is absent in older documents, and a reader treats absence as `null`.
- **Extensions a reader must honour go in `requires`.** When a new key changes what a reader must do to read the document safely (a second digest algorithm for blobs, a packed container, an encrypted attachment), the writer lists its name in `requires`, and a reader that does not implement it refuses `unknown_requirement` instead of reading the document wrongly. Names are `[a-z][a-z0-9_-]{0,31}`; the registry of names is this document.
- **v2 only for breaking changes:** removing or renaming a key, changing a type or a meaning, a new member of a closed set (`source.kind`, `fidelity.level`, the cache and evidence enums, `secrets.policy`, the refusal codes), a new canonical rule, or a change to the container layout. `psf` becomes `"2"` and a v1 reader refuses it `unsupported_version` without reading further.
- **The fixtures are the pin.** Once a fixture is on `main`, a change to it comes with the version change that justifies it, as `protocol/fixtures` and `protocol/bridge/v1` already do. Every reader runs the same fixture set; a reader that fails one is wrong, or the fixture is, and the fix lands in both places in one change.
- **Relation to the client API.** PSF is not a message of the client API and has its own version. A node's `job export` reply names the snapshot chain today; a later op that writes or reads a PSF container is an additive client API change under ADR 0018.

## 12. Fixtures

[`protocol/psf/v1`](../protocol/psf/v1) holds:

| Path | Contents |
|---|---|
| `cases/<case>.json` | one document each: `case` (the file's stem), `summary`, `input` (the exact bytes a reader is given, as one JSON string) and `expect`: `{"outcome": "accept", "psf": "1", "digest": <sha256 of input>, "fidelity": <offered level>, "package": <bool>}` or `{"outcome": "refuse", "code": <code>, "field": <path>}` |
| `containers/<name>/` | a container as a reader finds it on disk (`manifest.json`, `blobs/`), plus `expect.json` beside it: `{"container", "summary", "expect": {"outcome": "accept", "digest", "detached": [names]}}` or `{"outcome": "refuse", "code", "path"}`. `expect.json` is not part of the container; a reader is given the directory and told to ignore that one file |
| `manifest.sha256` | `sha256sum` lines of every other file, sorted by path |

The cases cover every fidelity (`valid_native_session`, `valid_new_native_session_with_package`, `valid_workspace_only`, `valid_unsupported`), two documents with a WorkPackage (`valid_native_session_with_package`, `valid_new_native_session_with_package`), every manifest-level refusal code at least once, and one document over a limit (`refuse_too_large_text`). The containers cover every container-level code and the detached case.

`crates/fabric-client/tests/psf_fixtures.rs` pins the manifest's own sha256, checks every file against it and that none is missing or extra, and checks the shape of every case: the stem, the outcome, the digest of each accepted input, that each accepted input is canonical under section 4, that the covered codes are exactly the closed sets above and the covered fidelities exactly the four. It is not a reader; the readers come with their crates and test against these files.

To regenerate the manifest after an edit, from `protocol/psf/v1`:

```sh
find . -type f ! -name manifest.sha256 | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256
shasum -a 256 manifest.sha256
```

and move the pin in `psf_fixtures.rs`. An accepted case's `expect.digest` is `printf '%s' "<input>" | shasum -a 256`.

## 13. Readers

Three independent readers are the acceptance of PR 4 of the 2026-11-08 roadmap; none is in this change. Their shared contract is: read bytes, return the manifest and the document digest, or one refusal code with its field; read a directory, return the manifest, the present and detached attachments, or one container code with its path; and compute the achieved fidelity for a given root, home and set of installed harnesses.

- `crates/fabric-psf` (Rust, depends on `fabric-core` for the id and path types and on `serde_json`; no other workspace crate): `read_manifest(bytes) -> Result<Manifest, Refusal>`, `read_container(dir) -> Result<Container, ContainerRefusal>`, `Manifest::digest() -> [u8; 32]`, `Manifest::achievable(&Target) -> Achieved`, and `write_manifest(&Manifest) -> Vec<u8>` producing canonical bytes.
- `sdk/typescript` (zod at the boundary, the same fixtures in `test/psf.test.ts`): `readManifest(bytes: Uint8Array): Manifest`, `readContainer(dir: string): Promise<Container>`, `manifestDigest(bytes): string`, `achievable(manifest, target): Achieved`, with refusals as a typed `PsfError { code, field }`.
- `scripts/psf-read.py` (Python 3.9, standard library only, the independence check): `python3 -I scripts/psf-read.py <manifest.json | dir>` prints the digest, the offered fidelity and the detached attachments, or exits 1 with `<code> <field>`; `--fixtures protocol/psf/v1` runs every case and container and exits non-zero on the first disagreement.

## 14. Sources

Local, read only on 2026-11-08: `~/.claude/projects/*/*.jsonl` and `~/.claude/` layout under Claude Code 2.1.294; `~/.codex/sessions/2026/11/08/*.jsonl`, `session_index.jsonl`, `external_agent_session_imports.json`, `thread_history_1.sqlite` under Codex 0.163.0; `~/.cursor/chats/*/*/{meta.json,store.db}` and `~/.cursor/acp-sessions/*/meta.json` under Cursor Agent 2026.10.01; `~/.local/share/opencode/opencode.db` under OpenCode 1.18.34; `~/.pi/agent/sessions/*/*.jsonl` under pi 0.80.3. Gemini CLI and Amp were not installed; their rows come from their documentation.

Documentation: Claude Code `how-claude-code-works` and `claude-directory`; Codex CLI `developer-commands`; Cursor CLI `reference/parameters`; OpenCode `docs/cli`; Gemini CLI `session-management` and `checkpointing`; Amp `docs/threads`; ACP `protocol/session-setup` and `protocol/schema`; `tawanorg/claude-sync`, `NeoAcar/claude-handoff`, `ahmojo/codex-claude-transfer` and `openai/codex-plugin-cc` READMEs. Fabric: ADRs 0001, 0003, 0004, 0006, 0011, 0012, 0014, 0015, 0018, 0021, and on the M5 branch 0024 and 0025 with `managed/package.rs`, `managed/canon.rs` and `managed/lifecycle.rs`; `crates/fabric-client/tests/bridge_fixtures.rs` for the fixture pin pattern.
