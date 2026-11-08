# ADR 0029: The Portable Session Format v1

Status: proposed 2026-11-08 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). PR 4 of the 2026-11-08 roadmap.

Adopts [docs/portable-session-format.md](../portable-session-format.md) as the public contract for one coding-agent job in transit, and [protocol/psf/v1](../../protocol/psf/v1) as its fixtures. It builds on ADR 0001 (the job as the unit), ADR 0003 (one live copy; the ledger head), ADR 0004 and ADR 0014 (the snapshot and its bundle chain), ADR 0006 and ADR 0015 (the harness state as opaque bytes), ADR 0011 (path identity), ADR 0012 (carry keys), ADR 0018 (the fixture and versioning discipline of the client API) and ADR 0021 (the bridge's canonical JSON and `manifest.sha256`). It is written to carry the WorkPackage v1 and the fidelity ladder of ADR 0024 as they stand on the M5 branch, without depending on that branch. ADRs 0023 to 0028 are taken by the M5 and M6 branches, so this one is 0029.

## Context

Fabric's internal store already captures everything a job needs to continue elsewhere: a content-addressed snapshot with the git bundle, the dirty tree, the ignored caches with their carry keys and the harness state tree; a hash-chained ledger; a redacted session with a state manifest. All of it is postcard and SQLite, readable by Fabric's crates alone. The 2026-11-08 roadmap moves the product from "a tool that moves a job between two Macs" to "the portable format for coding-agent sessions plus the reference runtime that runs it anywhere". That needs a form of the same state that a program in any language can write and read, that another UI (PR 7), another runtime (PR 9), a Linux node (PR 10) or an ACP proposal (PR 6) can target, and that a stranger can check against fixtures without trusting Fabric.

What exists outside Fabric is partial. Every harness keys its session store to the working directory and signs nothing (section 2 of the format document). `claude-sync` and `claude-handoff` move Claude transcripts with path placeholders and leave the workspace to git. `codex-claude-transfer` and OpenAI's `codex transfer` convert transcripts between two vendors. ACP defines `session/load` and `session/list` and no export. None of them states what a reader must refuse, none carries the workspace revision, and none has a shared fixture set.

The M5 branch defines the two pieces a public format must carry without owning them: the WorkPackage v1 with its canonical JSON and the four-level fidelity ladder (ADR 0024), and the source bindings and export policy (ADR 0025). The format must take a package as one section byte for byte, so the package digest of ADR 0024 stays computable, and must declare fidelity in the same words.

## Decision

### A directory with a canonical manifest and content-addressed blobs

The container is a directory: `manifest.json`, the one required file, and `blobs/<sha256>` for each opaque attachment. Workspace bytes do not travel in it; the manifest names the revision by its bundle head and ledger head (M5's `WorkspaceRevision`), and the bytes come from a store that holds the heads or from the source binding and a rebuild. Selected over a single archive because a directory is inspectable with `ls` and `shasum`, needs no archive format decision in v1, and is what a packed form would unpack to; over carrying chunks because that would make the format a second store format with its own transfer and integrity rules, which ADR 0014 already owns.

### Canonical JSON with the M5 writer's rules, identity by sha256

The manifest's bytes are canonical: sorted ASCII keys, no whitespace, every integer a decimal string, a fixed escape set, `null` for every absent optional value. These are the rules of `managed/canon.rs` on the M5 branch and of the bridge fixtures, so a WorkPackage is pasted in as one section without re-encoding. A reader re-serializes and compares bytes; a document with a second byte form is refused. The document's identity is the sha256 of its bytes, and blobs are named by sha256, because the independence check (a Python reader on the standard library) must compute them; references into Fabric's store (bundle head, ledger head, cache and evidence digests, everything inside the package) stay blake3 as Fabric writes them, with the algorithm fixed by the field rather than by a prefix, as the client API fixtures already spell them.

### No signature in v1

A PSF document is not signed and carries no key. Selected because nothing it is compared against is signed either, because the trust a reader needs (that the revision is the one named) is given by the digests the store checks on transfer, and because a signature scheme is a decision with its own key management, revocation and library choice. A later signed form enters through `requires` (below), so a v1 reader refuses it rather than reading an unsigned view of a signed document.

### Fidelity is declared in ADR 0024's words and derived from the attachments

The offered level is `native_session`, `new_native_session_with_package`, `workspace_only` or `unsupported`, the stored names of `Fidelity::name`. It is the highest level among the attachments, `workspace_only` without any, and `unsupported` only with a reason and no attachment; any other combination is a refusal. Each attachment names the harness that reads it, its format, its own fidelity, the session id, and the root and home it is bound to, so a reader computes what it can achieve on its own root and home under ADR 0011 and reports both numbers. Nothing is rewritten: a bound attachment that lands elsewhere falls a level, it is not patched.

### Secrets are never inside, and the policy says so

`secrets.policy` has one value, `never_inside`, and a document with another value is refused before anything else in it is trusted. The manifest names what was left behind and what the job allowed through (ADR 0025), and a reader refuses any text field with a token shape from the shared list of `fabric_core::credential`. The format promises nothing about an attachment's contents beyond its digest; the attachment's writer redacts, as Fabric's harness capture does.

### Fixed limits, a closed refusal list, additive v1 and `requires`

The manifest is at most 1 MiB (the client API's parse cap), a text field 16 KiB and the package 256 KiB (ADR 0024's constants), with counts on every list. Fourteen manifest-level and three container-level refusal codes are closed in v1; a reader stops at the first. A later v1 writer may add keys, `format` names, `harness.kind` names and `key` kinds, which a reader ignores; a change a reader must honour to read safely is named in `requires`, and a reader that does not know the name refuses. Anything that removes, renames, retypes or widens a closed set is v2, which a v1 reader refuses by the version alone. The fixtures pin all of it: `protocol/psf/v1` with a `manifest.sha256` and the pin test in `fabric-client`, in the pattern of `protocol/bridge/v1`, until the `fabric-psf` crate takes the test over.

### What this change ships, and what it does not

The contract is the format document, this ADR, the fixtures (one per fidelity, two with a package, one per refusal code, one over a limit, five containers) and the pin test. The same change builds the three readers of section 13 (Rust `fabric-psf`, the TypeScript SDK, the standard-library `scripts/psf_check.py`), the conformance tests that diff them, the node's `job_export_psf` op behind `fabric job export --psf`, and `fabric job import --psf` and `fabric psf inspect`. The RFC post is not in it.

## Alternatives considered

1. Publish Fabric's snapshot manifest and ledger records as the format. Rejected: they are postcard, versioned by Fabric's crates, and hold chunk lists a stranger cannot use; the format must be readable without the store.
2. Adopt `claude-handoff`'s bundle or OpenCode's export as the format. Rejected: each is one harness's transcript with placeholders, with no workspace revision, no fidelity and no refusal rules; they become attachments with a `format` name instead.
3. One archive file (tar or zip) as the container. Deferred: a directory is what either unpacks to, and the archive choice brings its own path, permission and size rules. It can enter v1 through `requires`.
4. blake3 everywhere, including blobs and the document digest. Rejected for the blob and document identity: the independence check must run on a standard library, and a blob's name is checked by whatever tool the user has; blake3 stays where Fabric already uses it.
5. Sign the manifest with the writing node's ed25519 key (ADR 0005). Deferred as above; `requires` leaves the door open and v1 readers fail closed.
6. Carry the native session rewritten to the target's paths, as `claude-sync` does with `${HOME}`. Rejected: Claude's signed thinking blocks refuse an edited transcript, Codex records its roots, and every rewrite is a place to lose the user's work (ADR 0011); fidelity falls instead.
7. Numbers as JSON numbers. Rejected: JavaScript rounds above 2^53 and the client API, the bridge and the M5 canon already use decimal strings.
8. Optional fields absent rather than `null`. Rejected for v1.0 fields: the M5 canon writes `null`, so the package section would follow another rule than the rest, and `missing_field` would be ambiguous.

## Consequences

- A second program can read a Fabric job in transit without Fabric: the format, not the node, is the product boundary the roadmap asked for.
- The WorkPackage and the fidelity ladder of the M5 branch are now named in a public document; a change to either on that branch is a change to this format and goes through its fixtures.
- Fabric's own harness state tree gets a public name, `fabric.harness_state.v1`, and a tar form; the `fabric-psf` crate writes and reads it.
- The fixture discipline of ADR 0018 and ADR 0021 extends to a third set with its own version: `scripts/check-fixtures.sh` does not watch `protocol/psf`, and the pin test and the three readers' fixture tests are the guard.
- A sha256 implementation already lives in `fabric-client`'s tests; it moves to a shared test module so two pin tests use one copy. No new dependency.

## Open questions

- Whether the packed form is tar, zip or a Fabric frame, and whether it is a `requires` name or a v2.
- Whether `harness.kind` names need a registry beyond this document once a second writer exists.
- How an ACP `session/export` (PR 6) maps onto an attachment: a `format` per agent, or one ACP-defined format.
- Whether the cache `digest` as defined (the blake3 of the manifest's entry list) is what PR 8's standalone sync wants to compare, or whether it needs a tree hash of its own.
