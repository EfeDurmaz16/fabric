# ADR 0025: Source bindings, secret references and the export policy

Status: proposed, 2026-11-03.

Adopts the external draft P05 ("Source access import and secret references", 2026-10-25, Proposed) for the M5 campaign. Draft P05 left open the source registration surface, the export rules, the first resolver, retention and the active-checkout support matrix. Each is selected below. The selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe, and Efe can veto any of them at the pull request. The types are in [docs/managed-jobs.md](../managed-jobs.md), section "Placement and planned continuity (M5)", part "Source bindings and secrets (P05)". It extends ADR 0004 (content-addressed snapshots), ADR 0007 (environment spec and secrets), ADR 0011 (path identity) and ADR 0012 (dependency carry).

## Context

A managed job's source today is a repository path on the authority's host or a published snapshot (`SourceBinding` of ADR 0019). A job submitted from another node, or for a machine that is asleep, cannot name a path on the authority. A task that needs a token to fetch, build or test has no way to say so, and the only secret rule Fabric has is the capture policy of which paths never leave a node. Placement (ADR 0026) must know, before launch, that the target can read the source and resolve what the tasks need.

## Decision

### Source bindings v1: git, snapshot, empty

Three forms are added beside the M2 ones:

- `git`: a URI and an exact revision (a full commit id, never a branch or tag name), read through the existing clone path. The node that runs the attempt clones it; the authority never needs to.
- `snapshot`: a published snapshot id. A local directory enters as one through the existing capture path, run by the node the directory is on, which leaves the original directory untouched (import, not adoption); the submit then names the snapshot, and the authority pulls its chunks over the existing store transfer.
- `empty`: a fresh workspace with nothing in it.

Adoption of an active checkout as a managed job's workspace is explicitly unsupported in M5: a submit that asks for it is refused with that reason. The M2 forms, `repository { path }` and `snapshot`, keep their meaning on the local socket; `repository` is refused on the facade (ADR 0027), where a path on the authority's host names nothing.

### SecretRef v1 and the local resolver

A job declares the secrets its tasks need as `SecretRef { name, scope }`, where `scope` is `{ attempt: any | current, resource_kinds[] }` and the job is the one the ref belongs to; together these are the `{ job, attempt?, resource_kinds[] }` of the selection. A name is 1 to 64 characters of `[A-Z0-9_]`.

The first and only resolver in M5 is the executing node's local resolver: an environment variable `FABRIC_SECRET_<name>` of the daemon, else a file `<config>/secrets/<name>` under the node's own configuration directory, which must be mode 0600 and owned by the daemon's user. The value is handed to the task's process environment under `name` at spawn and nowhere else. A secret value never crosses the live link, never enters the journal, the ledger, a checkpoint, a package, a receipt or a log; the type that holds one does not serialize and prints redacted. A `SecretRef` the target node cannot resolve makes that node ineligible for the placement before launch (a structured exclusion naming the ref, ADR 0026), never an error at run time.

### Export policy: an explicit allowlist, empty by default

Each job carries an allowlist of workspace paths that may leave the node in a capture although a secret pattern matches them; it is empty by default. The patterns are the existing ones of `fabric_core::secret` (`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `.npmrc`, `.netrc` and the rest), and the allowlist is the consent the capture already takes for a tracked secret (`TrackedSecrets`). No general secret detection is promised: a value in a file the patterns do not name travels as the file does.

### Retention and revocation

An imported snapshot follows the store's existing retention: it is a snapshot like any other and `fabric store gc` keeps what a job reaches. SecretRef eligibility is rechecked at every placement from the node's current environment and files; nothing is cached across operations, so a rotated or removed secret excludes the node at the next placement.

## Alternatives considered

1. Require a GitHub App installation for every source. Rejected: it repeats the user's source restrictions and adds a hosted dependency; a URI plus a revision the node can clone is enough.
2. Copy the user's login and config directories to the target. Rejected: it widens credential exposure and proves nothing about the target's authorization.
3. Treat import as adoption of the directory. Rejected: adoption needs the root mapping and writer checks of draft P05's separate capability; import leaves the directory alone.
4. A secret broker or a secret manager in M5. Rejected: no local resource needs one, and choosing one is a dependency decision of its own.
5. Secret detection by content. Rejected: it promises what it cannot keep; the explicit allowlist and the path patterns are the contract.

## Consequences

- `SourceBinding` gains `Git`, and `Empty`; the facade accepts `git`, `snapshot` and `empty` only.
- `JobPolicy` gains `secrets: Vec<SecretRef>` and `export_allowlist: Vec<RelPath>`; both under codec format byte 2.
- `fabricd` gains a `secrets` module (the local resolver and its eligibility probe) and a `sources` module (clone at the exact revision, import through capture, empty workspace).
- A test plants `FABRIC_SECRET_*` and a secrets file, runs a task that reads it, and asserts the value is in no journal row, event, snapshot, package, receipt or log line.
- A job with a secret no node can resolve is accepted and parked with the exclusion visible (ADR 0026).

## Open questions

- Which short-lived broker becomes the second resolver, and how a ref names it.
- The active-checkout adoption matrix: which roots, which writers, which markers.
