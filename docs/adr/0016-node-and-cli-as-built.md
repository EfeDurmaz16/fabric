# ADR 0016: The node and the CLI as built

Status: accepted, 2026-10-20

Records `crates/fabricd` (the node) and `crates/fabric` (the command), branch `feat/daemon-cli`. Answers the open questions of ADR 0013 about the node crate, decides the ADR 0003 open question about departed copies, and names where the build departs from ADR 0001, 0003, 0004, 0008 and 0011, and why. Everything not named here stands as those ADRs say.

## Context

The core, the store, the network layer, the harness adapters and the environment crate existed as libraries. Nothing joined them: no process held the store, no file held a ledger, and no command moved a job. The architecture page planned two crates, `fabric-node` and `fabric-cli`. They are `fabricd` and `fabric`, so the binary is `fabric` and the daemon library has the name a reader expects of a daemon.

## Decision

### One process per state directory

`fabric serve` runs the node. It takes an exclusive `flock` on `<state>/daemon.lock` before it opens anything, and keeps it for its whole life. The state directory is `$FABRIC_HOME`, else `~/.fabric`, mode 0700:

```text
<state>/
  daemon.lock   node.key   peers   harness.toml (optional)   fabric.sock
  store/        the store of ADR 0014
  locks/        one tree lock per workspace root, so one installer runs per tree (ADR 0007)
  jobs/<job-id>/
    ledger      append-only records (ADR 0002)
    manifest    the derived job manifest
    marker      what the workspace at the root holds (ADR 0004)
    harness     the harness this node launched: pid, kernel start time, epoch
    session     the harness session the job continues here
    harness-state/   capture output and restore input for the adapter
```

The node is the store's single writer, the ledger files' single appender and the supervisor of every harness it recorded. There is no mode in which the CLI takes the lock itself: without a daemon every job command fails with "no fabric daemon ... start one with `fabric serve`". ADR 0003 allows the `resume` command to be the supervisor when no node runs; that path is not built.

The control socket is `<state>/fabric.sock`. One connection carries one request and one response, each a `u32` length and postcard bytes. A path of 104 bytes or more is refused at start with a message, because macOS cannot bind it.

### The ledger file

`LedgerFile` implements `LedgerStore` as ADR 0002 specifies: one `write` of the whole record, `F_FULLFSYNC` on macOS and `fsync` elsewhere, the first record written to a sibling, flushed and renamed into place with the directory flushed, so a failed first write leaves no ledger rather than an empty one, and a `Created` for another job is refused before any file is made. A failed append is cut off again; if that fails too, the file refuses further appends until the node restarts. Open truncates a torn tail and fails closed on any other damage, leaving the file byte for byte as it was. A manifest that remembers more records than the ledger holds fails closed with `BehindManifest`. ADR 0002 chose a file over SQLite; this build keeps that choice.

### Who does what

The CLI never touches the store or a ledger. It sends a request, prints the lines the node answers with, and for `run` and `resume` does what only it can: it runs the setup steps of ADR 0007 in the user's environment under the tree lock in `<state>/locks`, held from the plan to the last step, launches the harness in its own process group with the user's environment, gives the terminal to that group when the harness is interactive, and registers it. The node appends `Resumed` only if it still owns the job at the epoch the plan was made under; otherwise the CLI kills the group at once. When the harness exits, the CLI tells the node. The node registers only a pid that leads its own process group, and its record stands for that group: it stays while a process the harness started still runs in the group, a stop signals the whole group and returns only once the group is empty, and a group that cannot be signalled or outlives `SIGKILL` fails the suspend, the move or the fence with the record kept. A process that leaves the group, with `setsid` for one, is out of reach.

ADR 0007 has the caller take the tree lock before restore writes the tree. Here the node restores and the CLI materializes, so the lock is taken after the restore. The restore writes only into an empty directory it renames into place (ADR 0014), so it never writes over a tree that an installer from an earlier `resume` still holds; that `resume`'s lock makes the next one refuse with the lock's path.

The harness gets the user's environment minus the parent-session markers ADR 0015 lists, so a `fabric job run` typed inside a Claude Code session starts a session of its own. Before a Claude launch the CLI pre-seeds trust (ADR 0006, amended for gate finding F1) and never fails on the outcome: it prints nothing when Claude's key for the workspace was already set, `trusted <key>` when it wrote the key, and a one-line warning when it left the file alone, such as while another `claude` runs. The node confirms the adapter's capture with `confirm_departure` once `SnapshotPublished` is durable, on the first suspend and on the one that finishes it after a crash, so the departure record of ADR 0015 counts only for a published snapshot.

Interactive launches give the terminal to the harness with `tcsetpgrp`. The CLI never takes it back; the shell does when the command exits, as it does for any job.

### Job roots and departed copies

`fabric job new --repo <path>` clones the repository into `~/fabric/jobs/<job-id>/work` (ADR 0011, rule 2). `fabric job init`, which adopts a checkout in place, is not built; ADR 0001's statement that `init` is the only way a job starts is superseded by `new` for this phase.

A restore lands at the root the snapshot recorded only when that root is exactly `~/fabric/jobs/<job-id>/work` under this node's home, because the root arrives from a peer and any other path, such as `~/.ssh`, is a bug or a hostile peer. When it is not, the job lands under `<state>/jobs/<job-id>/work`, and the CLI prints why. The adapters of ADR 0015 still refuse a different root or home, so only a harness without path-keyed state can continue there; doctor refuses the move before it gets this far.

ADR 0003 kept a departed copy at the root as a warm cache and left open whether it should move. It moves. When a node learns that the job departed, or was stolen from it, and its marker says the workspace is clean at a published snapshot, the workspace is renamed to `departed/<epoch>` beside the root and kept. Nothing in it is missing from the job, a user who types `claude` in the old directory finds nothing there, and two nodes that share a disk, as the end-to-end test does, never meet at the root. A workspace with changes made after its last snapshot stays where it is, and `status` names it an orphan; nothing deletes it. The in-place restore of ADR 0004, which updated a departed copy, is therefore not needed and not built: every restore is the fresh restore of ADR 0014.

### The move as built

The push (`fabric job move <job> --to <node>`, typed on the owner) follows ADR 0003 step by step: preflight, suspend, push the ledger with `Append` (the target fetches the latest snapshot over the same session before it answers the last page), `MoveStarted` made durable, `Append` again so the target holds it, `MoveOffer`. The target answers with the one decision for that move id, durable before the answer leaves. The job's lock is released while the target copies, so before `MoveStarted` the source checks again that the job is suspended in the snapshot it pushed and that no run was planned since; a `run` planned marks the workspace live, and a move that finds it so stops with the job still owned here. A lost answer is followed by `MoveStatus` on a fresh connection; five failed tries leave the source frozen and say so with the one command that resolves it.

The pull (`fabric job resume <job>` on a node that does not own the job) fetches the records and the snapshot, sends `MoveRequest`, and the owner suspends (stopping its harness) and appends `MoveStarted` toward the puller. The puller fetches again, decides the offer from its own replica with `Ledger::answer_offer`, and pushes the `MoveImported` back. `--on <node>` names the node to pull from when this node has never seen the job; otherwise the ledger's owner is used. A pull never moves the job to a third node: resume means here (ADR 0008).

`fabric job steal <job> --force` writes `Stolen` and a `Note` with the warning; without `--force` it prints what it would do and fails.

Three protocol details the earlier ADRs left open:

- A node that holds none of a job's records sends `Pull { after: None }` and gets the chain from `Created`. An `after` the answering chain does not hold is answered `NotInChain { head }`: the asker stops when its own chain holds that head, because the peer is only behind, and otherwise the chains forked and the node acts as Forks, below, says.
- A record a peer sends is checked first as the peer's own. If the ledger says another node wrote it, it is accepted as relayed (ADR 0002, amendment 6), except when the writer is this node: a node writes its own records and never learns one from a peer.
- A per-job request whose epoch is behind this node's is answered `StaleEpoch`, except `Pull`, which is a read from a node catching up.

### Recovery and catching up

A node catches up with its paired peers at start, whenever a session with a peer begins, and before a `run` or a `resume` of a job it owns. Catching up pulls each job's records from each peer the node can call, one with a pasted address or one that holds a session open with it, then settles the job: it kills a harness whose epoch is over and moves a clean departed or stolen-from copy aside.

At start each peer is caught up on a task of its own, so the pass takes about one dial however many peers there are, and a peer that does not answer costs one failed dial for the whole pass rather than one per job (about 13 s with QUIC then TCP). The target of an open move is then asked for its decision, unless it did not answer. Commands that change a job wait until this pass has run; `job ls`, `status` and `log` answer from local state at once, never dial, and say so while the pass still runs. Node commands and doctor do not wait.

Before a launch each reachable peer gets three seconds; one that does not answer is named and the run goes ahead on this node's ledger. So a node that slept through a steal does not launch the stolen job when it restarted, or when it can reach the thief at the next run. A node that stays up while the thief is out of reach, as a laptop on another network, keeps its harness running until the two meet: at that check, when either side opens a session, or at the next start. The Fencing section of ADR 0003 already accepts that a harness edits until its node learns the epoch.

### Forks

The fork rule of ADR 0003 is not built, so a node fails closed instead. When a peer's ledger for a job does not continue its own and the peer's ledger reached an epoch at least as high, the node writes `jobs/<job-id>/forked` with the peer, that epoch and the peer's head. It finds this when a pull is answered `NotInChain` with a head its own chain lacks, after which it reads the peer's whole chain for the epoch, or when a page a peer pushes at an epoch at least as high does not link to its head. From then on the job is frozen on that node: `run`, `resume`, `suspend`, `move`, `approve`, `reject` and a peer's move request refuse it, the recorded harness is stopped, the workspace stays where it is as an orphan, and `status` names the peer to continue on. When the peer's epoch is lower, the peer is the side that freezes when it learns this node's records, and this node carries on. Nothing lifts a freeze; the fork rule will. The peer's epoch is read from its records as they arrive, so a paired peer that lies can freeze a job here but cannot make this node run one.

### Doctor

`fabric doctor` prints this node's facts; `fabric doctor --peer <node> [--job <id>]` compares them with the peer's, row by row as ADR 0008's table lists them. `job move` runs the same comparison first with the peer as the target, and a pulling `job resume` runs it with this node as the target; both take `--force`. A peer's facts arrive in a new request, `RequestBody::Facts`, so the protocol version is 2. A node measures its facts with the `PATH` its daemon started with; this node's side uses the `PATH` of the CLI that asked. Three rows depart from the table: the clock row warns above 5 s and never refuses, because a carried cache whose mtimes are off only rebuilds; the CPU and operating system rows warn; and the network row reports the wire without a refusal, since a dial that fails stops the move anyway. The comparison is not yet written into the job record next to the move.

`--force` waives every refusal except a different user or home and a target without a login for the job's harness, as ADR 0008 says. The step 1 brief asked that `--force` waive every row; the ADR's two exceptions are kept, because the result would be a root that does not exist on the target.

### Where a root may not live

The root is refused under `~/Library/Mobile Documents` everywhere, and under `~/Desktop` or `~/Documents` on a node where iCloud syncs them (`defaults read com.apple.finder FXICloudDriveDesktop`). ADR 0011 also refuses `/tmp`. That rule is not enforced: roots are always under the home, the end-to-end test puts its home under `/tmp` to keep the socket path short, and on Linux every temporary directory is there.

### Carry keys

Capture measures carry keys with `fabric-env` detection: a cache detection does not know is `Unmeasured` and never carried. A restore cannot run detection before the workspace exists, so it measures the key of the same shape as the source's: the same package manager, its own versions and paths. Each cache prints one line, carried or rebuilt and why.

### Decisions as ledger events

`fabric job approve` and `reject` append `DecisionRecorded { text: "approved" }` or `"rejected"`, with the user's note after a colon. ADR 0002 makes a new event kind a format version bump; a typed approval record waits for the UI that would read it.

### The network questions of ADR 0013

- The node owns the `Service` implementation. The store gained `Store::manifest_chunks` and `ObjectStore::read_stored` so the node can serve without reading the catalog's internals.
- The peer book holds only the addresses the user pasted at pairing. `seen` records the time of a successful dial and no address, and is never called for an accepted connection, whose source address may sit behind a NAT. Which side can dial is therefore the presence of a pasted address.
- `Endpoint::with_dial_order` takes `QuicThenTcp`, `QuicOnly` or `TcpOnly`; `fabric serve --wire` sets it.

### Fault points

`fabricd` has a `faults` feature, on only in the end-to-end test's build. With it, `FABRIC_FAULT=<point>[:<action>]` arms one of `after_quiesce`, `after_snapshot_publish`, `after_departing_write`, `mid_transfer` and `after_import_receipt`, with `crash` (stop, write the signal file, wait for `SIGKILL`), `stall` (two seconds), `hold` (until the test removes the signal file), `drop` or `dup`. A point fires once per daemon process. The value is read once at start, and one that names an unknown point or action stops the node there. Without the feature the functions are empty and nothing reads the environment.

## Alternatives considered

1. **The CLI as supervisor when no daemon runs** (ADR 0003). Two code paths for every job command, one of them holding the store lock for as long as a harness runs. Deferred; the daemon is cheap to start and the gate runs one on each Mac.
2. **Keep the departed copy at the root and build the in-place restore.** The copy is a warm cache only for git objects the bundle chain already carries, the in-place restore is the most dangerous code ADR 0004 describes, and a copy at the root invites a by-hand resume of a departed session. Rejected for this phase.
3. **A JSON control protocol.** Easier to read with `socat`, but the request types already serialize with postcard and a second encoding is a second thing to keep in step. Rejected.
4. **SQLite for the ledger.** ADR 0002 rejected it; nothing in the build changed the reasons.
5. **A new event kind for approvals.** A format version bump for a record no code reads yet. Deferred.

## Consequences

- The first gate can be run by hand: build on the Pro, copy the binary to the Air, `fabric serve` on both, pair, `job new`, `run`, `move --to`, `resume`. The README's quickstart is that sequence.
- `fabric job move` and a pulling `fabric job resume` suspend the harness on the source: a pull from a node where the harness runs stops it there.
- Tests: the ledger file's torn tail, damaged middle record, manifest ahead of the ledger and a writer killed with `SIGKILL` mid-append; a reused pid; the doctor table row by row; the root rules; carry key comparisons; and the end-to-end test, which runs two daemons over loopback on QUIC and on TCP+TLS, moves a job and back, breaks the move at each fault point with `SIGKILL` of either daemon or a dropped or duplicated answer, and asserts after recovery that both ledgers name one owner at one epoch, the departed copy refuses to run, and the owner resumes the workspace and the harness session whole.

## Open questions

- Should a node that cannot reach the target of its open move offer the user `fabric job move --abort`, which ADR 0003 describes and this build does not have?
- `fabric orphan export` (ADR 0003) is not built; an orphan workspace is reported and left in place.
- The fork rule of ADR 0003 is not built: a node whose ledger forked after a steal, at an epoch no higher than the peer's, freezes the job (Forks, above) until the rule exists.
- The store applies its own secret list, not `fabric_core::secret` (ADR 0004, integration note of 2026-10-20).
- How do two nodes find each other after a network change? On the gate (finding F3, 2026-10-20) the Pro moved to another subnet overnight and the Air's address was unreachable from it. Today the user re-pairs with `fabric node add` and a new address. Candidates: Tailscale as an optional underlay whose addresses survive the change, or the reachable side dialing a rendezvous. Decide after the first move on the gate.
