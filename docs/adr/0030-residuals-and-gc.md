# ADR 0030: Residuals and `fabric gc`

Status: proposed 2026-11-09 (selections by the implementation agent under Efe's delegation of 2026-11-02; Efe can veto any of them at the pull request). PR 11 of the 2026-11-08 roadmap, the Fabric side; the Capsule side merged as capsule#321.

It builds on ADR 0003 (one live copy; departed and orphan copies are kept), ADR 0014 (the store's restore staging), ADR 0016 (the node and the CLI), ADR 0018 (the client API and its fixtures), ADR 0020 (the authority is the journal's one writer), ADR 0021 (the bridge) and ADR 0022 (cleanup under a fresh approval).

## Context

A node killed with `SIGKILL` leaves work behind: a harness capture written beside the last one and never renamed into place, a restore staging directory beside a root, a harness process group a fenced tenure started, the launch path's records and copies of an attempt that ended, and provider resources an ended attempt made through the bridge. Each one costs disk, a process slot or money. Recovery already settles every job's ownership and stops a fenced harness, but nothing listed what was left, and nothing could tell a leftover of a superseded tenure from the files of the live one. The roadmap row asks that after `kill -9` the residuals are found by label and removed under a fresh approval.

## Decision

### A label on every restore staging directory

`RestoreRequest` carries an optional label, opaque to the store, which `restore` writes as `fabric-label` beside its owner file before any tree, flushed with it under `PowerLoss`. The node writes:

```text
fabric staging label v1
job <job id>
node <node id>
epoch <epoch>
```

The first line names the version; a reader refuses any other first line, any missing or extra line and anything over 4 KiB, and keeps such a directory as `unlabeled`. Selected over reading ownership from the marker alone because the marker names only the tenure of the latest restore, and a staging directory may outlive several markers; over a journal row because the standalone path has no journal and the label has to survive the process that wrote it.

### The live set

Nothing in the live set is listed or removed. It is:

- the tenures this node owns: a standalone job whose ledger names this node as owner, not frozen, at its epoch;
- every job a move this node started holds (`moving`) and every job an operation runs on now (`busy`, the job's lock is taken);
- every managed attempt the journal holds as one that may still run: `granted`, `running` or `unknown`, at any generation, as `AttemptStatus::may_run` says;
- every attempt this node's runner or executor runs now, read under the lock a launch takes before it writes anything of the attempt.

The journal's part and the provider creates are read in one call on the authority's thread (`Authority::read_journal`), so an attempt granted between two reads can never be reported as a residual. A job's files are judged and removed under that job's lock, which every operation that writes them holds. A launch-path file is removed only when the journal on this node holds its attempt ended, which no later event undoes; an attempt the journal here does not hold, as one this node runs for another authority, is kept as `unknown_attempt`.

### What gc cannot classify, it keeps

Every kind gc knows is a variant of `Kind` in `crates/fabricd/src/gc.rs`, documented with where it lives, its label, its owner and when it is garbage; `docs/concepts/residuals.md` is generated from those docs. Anything else under `<state>/jobs` or `<state>/runs`, a label gc cannot read, a label naming another node and a job this node does not hold are reported among `kept` with a reason, never removed. Departed and orphan copies are kept by design (ADR 0003), and so are the runner's taken records, which answer a launch sent again.

### A fresh approval per provider resource, never stored

A provider resource an ended attempt made is removed only through `Bridge::cleanup` (a `sandbox.destroy`, ADR 0022), through the worker the request names, and only with an approval given in that request for that one create. The node stores no approval and the CLI stores none, so a second run asks again. The cleanup awaits the worker on the runtime and records each step through the authority's queue (`fabric_bridge::Hand`), one call at a time, so the authority answers every other call while the worker acts; before this, the cleanup ran on the authority's thread and held it for up to the cleanup's 60 s deadline. Capsule lists its own labelled resources (`listResiduals`) for a caller that names which owners ended; Fabric does not need that listing, since its journal holds every create, and the removal is the same v1 destroy on both sides.

### The command

`fabric gc` is a dry run: the residuals as rows on standard output, what it keeps and the size of the live set as notes. `--yes` removes exactly the set the report lists. Under `--json` the document is the report's own fields (`removing`, `live`, `residuals`, `kept`), with `"outcome": "removed"` for a removal. A removal that failed exits 1 with reason `gc_incomplete`; a provider resource that got no approval exits 84 with reason `approval_required`; in both the report is the failure's data. `--approve <OPERATION>=<APPROVAL>`, `--worker`, `--profile` and `--worker-config` name the approvals and the worker. The op is additive to api 2 (ADR 0018): a node that does not list `gc` in its welcome is never asked.

## Alternatives considered

- Removing residuals during recovery, with no command. Rejected: recovery runs at every start without a human, and a provider resource needs an approval; a removal nobody asked for is the failure mode ADR 0022 exists to prevent.
- Treating a staging directory with no label as garbage. Rejected: a staging directory from a build before this one, or one another tool wrote beside a root, is not provably Fabric's; it is reported and kept.
- Holding the authority's thread for the destroy, bounded by a shorter deadline. Rejected: any bound is still time the authority answers nothing, and a slow provider is normal; routing each journal step through the queue removes the wait instead of shortening it.
- Removing launch-path files of attempts the local journal does not hold. Rejected: the authority may be another node, and an `unknown` attempt there is still live.

## Consequences

- A kill at any restore checkpoint leaves a staging directory that says whose it is; the store's crash test checks the label at every checkpoint, and the e2e `gc::e2e_gc_after_a_kill_lists_and_removes_only_what_superseded_tenures_left` runs the whole path with two nodes.
- The bridge gains a third way to reach a log, `Slot::handed`, for a log another component owns on its own thread.
- On a node that runs attempts for another authority, launch-path leftovers stay until that node's journal or a later version can judge them.

## Open questions

- Whether gc should judge a remote authority's attempts by asking it, over protocol 4.
- Whether `harness-state.old` without a `harness-state` should be put back in place by gc instead of kept.
