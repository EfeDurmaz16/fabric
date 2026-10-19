# Glossary

Terms used in the architecture page and the ADRs. One meaning each. If a word is not here, it has its ordinary English meaning.

**ACP** (Agent Client Protocol). A JSON-RPC protocol for driving coding agents. Fabric uses it as the generic harness path. Methods Fabric cares about: `session/new`, `session/load`, `session/prompt`, `session/cancel`.

**Arbiter.** The one node allowed to append a claim for a job: the node that wrote the latest `LeaseClaimed` or `LeaseReleased`, or the creator before any claim.

**Blob.** Bytes stored by hash. Chunks and manifests are both blobs.

**Catalog.** The SQLite database in a store that maps snapshot ids to manifests and chunk ids to on-disk files, and records which chunks each snapshot references.

**Chunk.** A content-defined piece of a file produced by FastCDC, compressed with zstd and named by its blake3 hash.

**Claim.** A request to the arbiter for a job's lease at the next epoch. The arbiter appends the `LeaseClaimed` record and the claimant copies it.

**Epoch.** A counter on a job's lease. It increases by one on every claim. Any message or write carrying an older epoch than the current one is stale and is refused.

**EnvSpec.** The environment specification of a job: tool versions, setup steps, env var names and cache paths. It describes how to make a node able to run the job. It never contains secret values.

**Event.** One append-only record in a job's ledger. Events are never edited or deleted.

**Fencing.** Refusing work from a stale epoch. The mechanism that keeps exactly one live copy.

**Harness.** The agent program that does the work: Claude Code, Codex, or any ACP agent.

**Harness adapter.** The Fabric code that knows how to detect, capture, restore and launch one harness.

**Harness record.** The file under a job's directory that names the running harness process (pid, process group, start time, or the harness's own session id), its epoch and its supervisor. What a restarted supervisor reads to find the process.

**Harness state directory.** The files a harness needs to continue a conversation. Fabric treats it as opaque bytes owned by the adapter.

**Job.** The unit of development work. A goal plus a ledger plus snapshots plus an EnvSpec plus a lease.

**Job init.** `fabric job init`: adopting the current directory, in place, as a job's workspace, or creating a fresh one with `--new`. The only way a job starts.

**Job record.** The ledger and the manifest together.

**Ledger.** The append-only, hash-chained log of a job's events.

**Lease.** The right to run a job. At most one node holds it at a time. It has an epoch and an expiry time.

**Live copy.** A workspace plus running harness for a job. There must be at most one.

**Manifest.** A versioned description of a job (job manifest) or of a snapshot (snapshot manifest). Manifests are the only files whose format Fabric promises to keep readable across versions.

**Marker.** The node-local record under a job's directory that names a workspace root and says whether every managed byte in it is published (`Clean`), a harness has run since (`Dirty`), a suspend is in flight (`Suspending`), or a restore is in flight (`Restoring`). The guard on the only path where Fabric removes files. Never stored inside the workspace.

**Materialize.** Run the EnvSpec setup steps on a node so that the job can run there.

**Node.** A machine running the Fabric node process. Identified by an ed25519 public key.

**Node id.** The ed25519 public key of a node. Also the TLS identity pinned on every connection.

**Orphan snapshot.** A snapshot that is not on its job's chain, made by `fabric orphan export` from an orphan workspace and pinned in the store so gc keeps it.

**Orphan workspace.** A workspace left on a node that lost the lease before it suspended. Kept in place forever until the user exports or removes it.

**Pairing.** Telling one node the id and address of another, out of band. Pairing is how trust is established. Discovery is never trust.

**Placement.** Choosing which node resumes a job and at which path. In v0 the node is the one where `resume` is typed and the path is the recorded root.

**Release.** The act of the lease holder giving up the lease at suspend time.

**Restore.** Writing a snapshot into a fresh workspace on a node, or in place into a workspace whose marker says it holds only published bytes. No path rewrites when the roots are equal, which is the v0 rule.

**Root.** The absolute path of a job's workspace. Recorded in every snapshot manifest and used as the target path on every node, so nothing inside the workspace or the harness state needs rewriting.

**Snapshot commit.** One of the two git commits a capture makes with a private index: `I` for the index and `W` for the working tree. They carry uncommitted work inside a bundle without touching the user's index or refs.

**Snapshot.** An immutable, content-addressed capture of a workspace and its harness state at one moment.

**Store.** A node's local blob directory plus catalog. Written by one process at a time, under the data directory lock.

**Supervisor.** The process that renews a job's lease while its harness runs and kills the harness when a higher epoch is learned. Always the holder of the data directory lock: the node process, or else the `fabric resume` command that launched the harness, which stays resident until the harness exits.

**Suspend.** Stop the harness cleanly, publish a snapshot, release the lease.

**Transport.** The trait that connects two nodes by id and gives them byte streams. QUIC first, TCP+TLS as fallback.

**Trigger.** Whatever starts a suspend or a move. Manual in v0.

**Unreferenced chunk.** A chunk or manifest that no live snapshot reaches. Invisible to readers; the only thing `fabric store gc` removes.

**Warm cache.** A node already holds most chunks of a job's latest snapshot, so a resume there moves little data.

**Workspace.** The checkout directory a harness works in. A snapshot captures it. A restore recreates it.
