# ADR 0024: The portable work package and continuation fidelity

Status: proposed 2026-11-03, built 2026-11-03 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request).

Adopts the external draft P04 ("Portable work package and continuation fidelity", 2026-10-25, Proposed) for the M5 campaign. Draft P04 left open the package's fields and limits, the allowed context size, how evidence is selected and the default allowed fidelity. Each is selected below. The selections were selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe, and Efe can veto any of them at the pull request. The types are in [docs/managed-jobs.md](../managed-jobs.md), section "Placement and planned continuity (M5)", part "Work package and fidelity (P04)". It extends ADR 0006 (harness adapters) and keeps ADR 0011 (path identity) as it is.

## Context

A native harness session is vendor-specific, path-sensitive and sometimes signed. The existing move carries it as opaque bytes to a node with the same path and home (ADR 0011), and that is the only continuation Fabric knows. A managed job also holds things a session does not: the intent, which tasks are open, what was decided and why, what evidence was accepted and what is still unanswered. When the next attempt runs under another harness, or the session cannot be consumed, that is what survives. Today it is not written anywhere a continuation can read, and a fresh session started in its place would look like the old one continued.

## Decision

### WorkPackage v1 is a projection of accepted job state

The package is derived from the journal and the store, never typed by hand and never a second authority. Its fields: the job id, the work revision it projects, the intent, the acceptance criteria, the decisions (each with its stated reason), the tasks with their class, state and last receipt, the workspace revision (bundle head and ledger head), the accepted evidence refs (digest plus attempt), the unresolved questions, the artifacts (refs by digest), and the permitted next actions. Decisions and questions enter the journal through one command, `RecordNote`, so the package reads them from events like everything else. Native session attachments are separate opaque refs the adapter owns; they are never inside the package.

### Limits

The whole package is at most 256 KiB of canonical JSON; any single text field is at most 16 KiB; evidence travels by digest only, never inlined. The builder refuses a package over a limit with the field named; nothing truncates silently.

### Fidelity ladder and the job's allowance

From highest: `native_session` (the adapter restores the captured session and the harness continues it), `new_native_session_with_package` (the same or another harness starts a new session that is handed the package), `workspace_only` (the workspace is restored and no session is started for the continuation), `unsupported` (a refusal with its reason). `JobPolicy.allowed_fidelity` defaults to `{ native_session, new_native_session_with_package }`; `workspace_only` needs an explicit opt-in; `unsupported` cannot be allowed. Every continuation records the fidelity it actually achieved, with the package digest it used and the native session it made or resumed. A refusal is never turned into a lower fidelity by the system: when the highest allowed fidelity the target supports is none, the continuation is refused with the reason, and the owner widens the allowance or picks another target.

### Evidence selection

The package carries the accepted evidence of the current attempt, plus the accepted evidence of prior attempts by digest. Credentials and blanket approvals are never part of the package: the builder refuses any text field that looks like a credential (`fabric_core::credential::looks_like_credential`), the types hold no approval reference, and a test plants a token and an approval in every text source and asserts the package is refused or free of them.

### Same path and home

ADR 0011 stands: `native_session` needs the same root and the same home on the target. A different layout needs an adapter profile that proves the harness reads its session there; none is claimed in M5, so a layout mismatch is a placement exclusion (ADR 0026) and, for a pinned target, a fidelity below `native_session`.

## Alternatives considered

1. A universal transcript format across harnesses. Rejected: it assumes compatible vendor semantics and would rewrite signed state.
2. An LLM summary of the session as the package. Rejected: it loses provenance and can invent progress; the package is a projection of accepted state only.
3. Pin a job to its first harness forever. Rejected: it blocks the product's stated vision; `new_native_session_with_package` is the supported way across harnesses.
4. Inline small evidence to make the package self-contained. Rejected: evidence stays tied to its snapshot and attempt by digest, and inlining invites the package to become the evidence.
5. Fall back to `workspace_only` when the native session cannot be consumed. Rejected: a silent fallback is what draft P04 forbids; the owner opts in.

## Consequences

- `JobPolicy` gains `allowed_fidelity`; `Intent` gains `acceptance`; `Command` gains `RecordNote`; the journal gains the `ContinuationRecorded` event. All under codec format byte 2.
- `fabric-core` gains a pure package builder with its limits and the credential refusal; `fabricd` feeds it from the journal and the store.
- The harness adapters report which fidelity they can offer for a captured session on a given node (`fidelity_for`), and the continuation records what was achieved.
- A two-harness continuation test (a Claude Code job continued under an ACP agent with the package, and back) is part of the M5 gate.

## Open questions

- Which adapter profiles can prove a different root or home for `native_session`, and what the proof is.
- Whether a package should carry a signed digest chain of its own once it leaves the owner's nodes (draft P07's open questions on principals).

## As built (M5)

Built on 2026-11-03 by owner A. `build_package`, the limits and the credential refusal are as decided; the package's canonical JSON is written by a private writer with sorted keys and every number as a decimal string. `RecordNote` refuses a credential-like text and a decision with an empty reason before the journal records it. A `Continued` observation whose achieved fidelity the continue did not allow is `FidelityRefused`, so a refusal is never recorded as a lower fidelity; the requested fidelity travels in the history as `EffectRequested` and is written into `ContinuationRecorded`. `offered_fidelity` compares the target's harness version with the session's newest writer by semver order. The rest is in docs/managed-jobs.md, "As built (M5): P03 to P05".
