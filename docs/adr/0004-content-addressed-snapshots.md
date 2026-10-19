# ADR 0004: Content-addressed snapshots

Status: accepted, 2026-10-19

## Context

A move must carry everything the job needs, including what git ignores. The research is explicit that nobody ships this: T3 Code's bundle excludes ignored files and runs no install on arrival; claude-handoff and session-roam carry transcripts only; the one unfilled piece is "same-OS-and-arch carry of the pnpm store or `node_modules` and `target/`, content-addressed and deduplicated, with a rebuild fallback" (`2026-10-19-t3code-vs-fabric.md`, section 2). The first gate requires `node_modules` to arrive.

Workspaces are large and mostly unchanged between snapshots. A full copy each time would make suspend slow enough that people stop doing it.

## Decision

### Store layout

Each node has one store at `<fabric-data>/store/`:

```text
store/
  chunks/ab/cdef...    one file per chunk, zstd-compressed, named by blake3 of the uncompressed bytes
  manifests/<hash>     snapshot manifests, postcard, named by blake3 of their bytes
  catalog.sqlite       rusqlite (bundled); tables: chunks, snapshots, snapshot_chunks, jobs
  tmp/                 staging for in-flight writes, cleared on open
```

### Chunking and hashing

- FastCDC content-defined chunking, min 16 KiB, average 64 KiB, max 256 KiB. Small files are one chunk.
- blake3 over the uncompressed chunk is the `ChunkId`.
- zstd level 3 on each chunk before writing. Compression is per chunk so a chunk can be verified and served alone.
- A file's identity is its ordered chunk list; two files with the same bytes share every chunk.

### What a workspace snapshot contains

```text
SnapshotManifest {
  version:       u16,
  job_id:        JobId,
  parent:        Option<SnapshotId>,
  created_at:    u64,
  created_by:    NodeId,
  platform:      { os: String, arch: String },
  git:           { bundle: [ChunkId], head: String, index: [ChunkId] },
  files:         [Entry],      untracked and policy-selected ignored files, relative to workspace root
  harness_state: [Entry],      opaque, owned by the harness adapter, relative to a state root
  env_spec:      Option<EnvSpecId>,
}
Entry { rel_path: String, kind: File | Dir | Symlink { target: String }, mode: u32, size: u64, chunks: [ChunkId] }
```

- **Git:** `git bundle create - --all` plus `refs/stash`, so every local ref and every unpushed commit travels. The index file travels as bytes so staged and unstaged state survives (T3 drops the split; we keep it). The `.git/objects` directory is not copied; the bundle is the transport for objects.
- **Untracked files:** everything `git status --porcelain` lists as untracked, by content.
- **Ignored files, by policy:** caches in, secrets out. v0 policy, in order: deny any path matching a secret pattern (`.env`, `.env.*`, `*.pem`, `*.key`, `id_*`, `*.p12`, `credentials*`, `.netrc`); allow the cache paths listed in the `EnvSpec` (`node_modules`, `.pnpm-store`, `target`, `.next`, `.venv`, `__pycache__`, `.turbo`); deny all other ignored paths. The policy is a data file in the job, not code, so the user can widen it per job. A denied path is listed in `fabric suspend` output so nothing silently stays behind.
- **Harness state:** whatever the adapter returns from `capture` (ADR 0006). Hashed and chunked like any other file.
- **Platform:** recorded so that restore knows whether carried caches are usable (ADR 0007).

### Publish: atomic, crash-safe

1. For each new chunk: write to `tmp/<random>`, `fsync` the file, `rename` to `chunks/ab/cdef...`. Rename on the same filesystem is atomic; a crash leaves either nothing or a complete chunk under its final name. An existing final name means the chunk is already present and the temp file is deleted.
2. Insert chunk rows into the catalog in one transaction after all renames.
3. Write the manifest to `tmp/`, `fsync`, rename into `manifests/`.
4. Insert the snapshot row and `snapshot_chunks` rows in one transaction.
5. `fsync` the `manifests/` and `chunks/` directories.
6. Only now append `SnapshotPublished` to the job ledger (ADR 0002).

A snapshot exists if and only if its ledger event exists. Chunks or manifests without a ledger event are orphans and are removed by `fabric store gc`, never on open and never automatically.

### Restore: path-relative with rewrites

1. Resolve the target workspace root by policy (ADR 0008). The user never types it.
2. Write everything into `<root>.fabric-restoring/`, verify every chunk hash as it is written, then rename the directory to `<root>`. A crash leaves a `.fabric-restoring` directory that the next restore deletes and starts again.
3. Restore git: `git init`, `git bundle unbundle`, write the index, check out `head`. Then write untracked and ignored files.
4. Rewrites: the manifest stores relative paths only. Harness state may contain absolute paths pointing at the source workspace root; the adapter's `restore` receives `(source_root, target_root)` and performs its own rewrites (ADR 0006). The core never guesses which bytes are paths.
5. If `platform` differs from the node or a cache path is missing, mark the environment as needing materialization (ADR 0007).

Restore is idempotent: running it again over an existing identical workspace writes nothing.

### Transfer

A node fetches a snapshot by manifest id, then asks the peer for the chunk ids it does not have, in batches. Each chunk is verified on arrival. A dropped connection resumes from the catalog's view of what is present (ADR 0005).

## Alternatives considered

1. **`git bundle` only, as T3 Code does.** Loses ignored files and the index split; 50 MiB cap in T3. Rejected as insufficient; kept as the git component.
2. **rsync or Syncthing or Mutagen.** All document "exclude `.git` and `node_modules`", which is the opposite of the requirement (landscape section 8.1). No content addressing across snapshots. Rejected.
3. **iroh-blobs.** BLAKE3-verified streaming, mature. It fixes the transfer format and pulls in iroh's networking. Named in ADR 0005 as the alternative if the own transport fails. Rejected for v0 by the no-vendor-dependency rule.
4. **Whole-file hashing without chunking.** Simpler. A one-line change to a 20 MB lockfile or a rebuilt `target/` directory would re-send everything. Rejected.
5. **Fixed-size chunking.** Insertions shift every later chunk. FastCDC exists for this reason. Rejected.
6. **Filesystem snapshots (APFS, btrfs).** Node-local and not content-addressed across machines. Rejected.
7. **Rebuild caches on the target instead of carrying them** (Flox, sccache, Turborepo; landscape W7). This is the fallback when platform differs, not the primary path. A `pnpm install` on a cold Mac is minutes; a chunk diff is seconds.

## Consequences

- Second and later snapshots of a job cost roughly the changed bytes.
- Two jobs on the same node that share `node_modules` content share chunks.
- Secrets never enter a snapshot unless the user edits the policy for that job, and the suspend output always lists what was excluded.
- Catalog corruption is recoverable by rescanning `chunks/` and `manifests/` since the files are self-named by hash.
- Crash tests will re-exec the publisher and `kill -9` it at each step boundary, then assert that `fabric store check` reports either no snapshot or a complete one.

## Open questions

- Chunk size targets are a guess. Measure dedup ratio on a real `node_modules` and `target/` before freezing.
- Hard links inside `node_modules` (pnpm) restore as copies in v0. Preserving them needs inode tracking; decide after measuring size impact.
- Extended attributes and macOS quarantine flags: dropped in v0. Revisit if a tool breaks.
- Large binary files that change every build (`.next/cache`): maybe exclude by default. Measure.
