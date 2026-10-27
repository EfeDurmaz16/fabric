# Glossary

Terms used in the architecture page and the ADRs. One meaning each. If a word is not here, it has its ordinary English meaning.

**ACP** (Agent Client Protocol). A JSON-RPC protocol for driving coding agents. Fabric uses it as the generic harness path. Methods Fabric cares about: `session/new`, `session/load`, `session/prompt`, `session/cancel`.

**Attempt.** One execution of one task of a managed job on one resource, under one generation. A retry is a new attempt; an attempt that disappears is `lost`, never `cancelled` (ADR 0019).

**Blob.** Bytes stored by hash. Chunks and manifests are both blobs.

**Bridge.** The framed stdio protocol between Fabric and a Capsule provider worker, version 1 (ADR 0021, docs/bridge-protocol.md). Fabric keeps job identity and the journal; the worker maps one governed action at a time onto a provider and reports what it observed.

**Capability profile.** The document a bridge worker sends in its handshake and enforces: which actions it can invoke, which are terminal-only, which support observe, cancel and fresh approval, and how its resource locators are scoped. A missing capability blocks the workflow that needs it.

**Carry key.** The exact strings a cache depends on (Node major and package manager, `rustc -vV` and `CARGO_HOME`, Python version and home). Recorded per cache in the snapshot; the target compares them with its own before trusting the bytes.

**Catalog.** The SQLite database in a store that maps snapshot ids to manifests and chunk ids to on-disk files, and records which chunks each snapshot references.

**Class (of an ignored file).** One of secrets, deps, build, tool cache, precious. Decides whether an ignored file travels: secrets never, precious always, the rest when the carry key matches.

**Chunk.** One stored object: a whole file of 8 MiB or less, or one FastCDC piece of a larger file. Compressed with zstd and named by its blake3 hash.

**Epoch.** A counter on a job's lease. It starts at 1 and increases by one on every `MoveImported` or `Stolen` record. Any message or write carrying an older epoch than the current one is stale and is refused.

**Doctor.** `fabric doctor`: the preflight that compares exact strings (user, home, tool versions, logins as yes or no, clock offset, synced folders) between two nodes and refuses a move on a mismatch the table marks as refuse.

**EnvSpec.** The environment specification of a job: tool pins, setup steps, env var names, cache paths and the secret patterns the job added. It describes how to make a node able to run the job. It never contains secret values.

**Event.** One append-only record in a job's ledger. Events are never edited or deleted.

**Fencing.** Refusing work from a stale epoch. The fencing token is `(job_id, epoch)`. The mechanism that keeps exactly one live copy among Fabric processes; native tombstones cover the harness's own writers.

**Generation.** The fencing counter of a managed job's canonical execution domain. It increases by one with every new attempt the authority grants; work reported under an older generation is refused (ADR 0020). Not an epoch, which belongs to standalone jobs.

**Harness.** The agent program that does the work: Claude Code, Codex, or any ACP agent.

**Harness adapter.** The Fabric code that knows how to detect, capture, restore and launch one harness.

**Harness record.** The file under a job's directory that names the running harness process (pid, process group, start time, or the harness's own session id), its epoch and its supervisor. What a restarted supervisor reads to find the process.

**Harness state directory.** The files a harness needs to continue a conversation. Fabric treats it as opaque bytes owned by the adapter.

**Job.** The unit of development work. A goal plus a ledger plus snapshots plus an EnvSpec plus a lease.

**Job init.** `fabric job init`: adopting the current directory, in place, as a job's workspace. Planned; not built (ADR 0016).

**Job new.** `fabric job new --repo <path>`: making a job whose root is a clone of the repository at `~/fabric/jobs/<job-id>/work`. How a job starts in this phase.

**Departed copy.** The workspace a node held for a job that has since moved to another node. It never runs again; when it holds nothing the job lacks, it is renamed out of the root to `departed/<epoch>` beside it and kept (ADR 0016).

**Node.** One `fabric serve` process and its state directory: the store, the ledgers, the peer book and the control socket the CLI talks to.

**Job record.** The ledger and the manifest together.

**Ledger.** The append-only, hash-chained log of a job's events.

**Lease.** The right to run a job. Either `Owned` by one node at an epoch, or `Departing` toward a target under a move id. It has no expiry in this phase.

**Live copy.** A workspace plus running harness for a job. There must be at most one.

**Manifest.** A versioned description of a job (job manifest) or of a snapshot (snapshot manifest). Manifests are the only files whose format Fabric promises to keep readable across versions.

**Managed job.** A job whose authority is the managed journal rather than an owner node's ledger: its commands carry operation keys and its progress is a work revision (ADR 0019, ADR 0020). A job made the existing way is a standalone job; the two modes never convert in M2.

**Marker.** The node-local record under a job's directory that names a workspace root and says whether every managed byte in it is published (`Clean`), a harness has run since (`Dirty`), a suspend is in flight (`Suspending`), or a restore is in flight (`Restoring`). The guard on the only path where Fabric removes files. Never stored inside the workspace.

**Materialize.** Run the EnvSpec setup steps on a node so that the job can run there.

**Move.** Handing a job from its owner to a target: stop, snapshot, push, `MoveStarted`, the target's one decision (`MoveImported` or `MoveAborted`), the source appends it. Push with `fabric job move --to`, pull with `fabric resume`.

**Move id.** A ULID the owner creates when it starts a move. The target writes exactly one decision per move id, and every later question about the move is answered with that same record.

**Node.** A machine running the Fabric node process. Identified by an ed25519 public key.

**Node id.** The ed25519 public key of a node. Also the TLS identity pinned on every connection.

**Operation key.** The name a caller gives one command on a managed job. The authority keeps the first answer under it, so the same key and command sent again get the same answer and no second effect.

**Orphan snapshot.** A snapshot that is not on its job's chain, made by `fabric orphan export` from an orphan workspace and pinned in the store so gc keeps it.

**Orphan workspace.** A workspace left on a node that lost the lease before it suspended. Kept in place forever until the user exports or removes it.

**Departed copy.** The workspace a move left behind on the source. Marker `Clean`, owner elsewhere, never launched, kept as a warm cache for the next move back.

**Pairing.** Telling one node the id and address of another, out of band. Pairing is how trust is established. Discovery is never trust.

**Placement.** Choosing which node runs a job. In this phase the user names it: `fabric job move --to` on the owner, or `fabric resume` on the node that wants it. The root is the recorded root.

**Restore.** Writing a snapshot into a fresh workspace on a node, or in place into a workspace whose marker says it holds only published bytes. No path rewrites, because the root is the same on every node.

**Root.** The absolute path of a job's workspace. Recorded in every snapshot manifest and used as the target path on every node, so nothing inside the workspace or the harness state needs rewriting. `~/fabric/jobs/<job-id>/work` for a job made with `--new`; the adopted checkout's own path otherwise.

**Snapshot commit.** One of the two git commits a capture makes with a private index: `I` for the index and `W` for the working tree. They carry uncommitted work inside a bundle without touching the user's index or refs.

**Snapshot.** An immutable, content-addressed capture of a workspace and its harness state at one moment.

**Steal.** `fabric job steal --force`: bumping a job's epoch without the owner's consent, from a complete local snapshot, with a printed warning. The only way past an owner or a target that does not answer.

**Store.** A node's local blob directory plus catalog. Written by one process at a time, under the data directory lock.

**Supervisor.** The process that kills a job's harness when a higher epoch is learned and keeps the harness record current. Always the holder of the data directory lock: the node process, or else the `fabric resume` command that launched the harness, which stays resident until the harness exits.

**Suspend.** Stop the harness cleanly, publish a snapshot, push it to paired peers. The owner does not change.

**Tombstone (native).** The mark a harness's own store accepts for "continued elsewhere": Claude's `continued-in` record, Codex `thread/archive`. Written during capture so the harness's own picker or supervisor does not reopen the departed session.

**Transport.** The trait that connects two nodes by id and gives them byte streams. QUIC first, TCP+TLS as fallback.

**Trigger.** Whatever starts a suspend or a move. Manual in this phase; there is no watcher.

**Unreferenced chunk.** A chunk or manifest that no live snapshot reaches. Invisible to readers; the only thing `fabric store gc` removes.

**Warm cache.** A node already holds most chunks of a job's latest snapshot, so a resume there moves little data.

**Work revision.** The count of accepted changes to one managed job, starting at 1. A command names the revision it expects and is refused if the job has moved on.

**Workspace.** The checkout directory a harness works in. A snapshot captures it. A restore recreates it.
