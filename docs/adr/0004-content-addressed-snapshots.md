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
  catalog.sqlite       rusqlite (bundled); tables: chunks, snapshots, snapshot_chunks, jobs, pins
  tmp/                 staging for in-flight writes, cleared by the lock holder at open
```

A manifest is a blob like any file. Its bytes are chunked with the same FastCDC parameters and stored under `chunks/`, and the `snapshots` row records the manifest's byte length and its ordered chunk list next to the `SnapshotId`, which is blake3 of the whole manifest bytes. The file under `manifests/` is the assembled copy that readers decode. This is what lets a manifest cross the network on the bulk path (ADR 0005): a `node_modules` tree has tens of thousands of entries, so a manifest is megabytes, and the control stream carries at most 1 MiB per frame. A reader decodes a manifest only after the assembled bytes hash to the `SnapshotId`, and only up to 64 MiB; a longer manifest is `ManifestError::TooLarge { len }` before any entry is decoded. gc treats a snapshot's manifest chunks as reachable from the snapshot.

### One writer per data directory

Exactly one process writes `<fabric-data>` at a time. It takes an exclusive `flock` on `<fabric-data>/lock` at open and holds it until exit. While `fabric node serve` runs, it holds the lock, and every CLI command that writes (`suspend`, `resume`, `store gc`, `orphan export`) is sent to it over the Unix socket `<fabric-data>/node.sock`; the CLI prints the node's answer. When no node is running, the CLI takes the lock itself and acts as the node for the duration of the command. For `resume` that duration is until the harness exits, because the lock holder is the harness supervisor (ADR 0003) and a supervisor that exits early leaves a harness nobody can fence. A second opener that cannot get the lock fails with `StoreLocked { pid }`; it never waits silently and never proceeds, so a CLI opening the store mid-publish cannot clear the daemon's staged files. `tmp/` is cleared by the lock holder at open and by nobody else.

The split of work between the two processes is fixed. The lock holder does everything that reads or writes the store or the job directory: claim, fetch, publish, restore, marker and harness record writes, renewals, gc. The CLI does the two things that need the user's terminal and shell: it runs the `EnvSpec` setup steps (ADR 0007) and it launches the harness, then registers the result with the lock holder. When the CLI is the lock holder it does both halves in one process.

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
  git:           Option<GitState>,  None when the workspace is not a git repository
  files:         [Entry],      ignored files selected by policy, relative to the workspace root; every path when git is None
  harness_state: [Entry],      opaque, owned by the harness adapter, relative to the adapter's declared state root
  env_spec:      Option<EnvSpecId>,
  partial_caches: [RelPath],   cache paths from the EnvSpec under which the secret policy denied something
}
GitState {
  bundle:          [ChunkId],            git bundle, full or incremental
  bundle_base:     Option<SnapshotId>,   the snapshot whose refs this bundle excludes; None means full
  refs:            [(String, String)],   every ref name and object id at capture time
  head:            Branch(String) | Detached(String),
  index_commit:    String,               commit I: its tree is the index at capture time
  worktree_commit: String,               commit W: its tree is the working tree at capture time
  worktree_mtimes: [(RelPath, u64)],     mtime in nanoseconds for every path in W
  config:          [(String, String)],   allowlisted projection of the repository config (below), never the file
  exclude:         Option<[ChunkId]>,    .git/info/exclude
}
Entry { rel_path: RelPath, kind: File | Dir | Symlink { target: String }, mode: u32, mtime_ns: u64, size: u64, chunks: [ChunkId] }
```

`mtime_ns` is read with `lstat` at capture and written back with `utimensat` after the bytes, with `AT_SYMLINK_NOFOLLOW` for links; a directory's mtime is set after its contents. The same applies to every path in `W` through `worktree_mtimes`. This is not cosmetic: Cargo decides fresh or stale from mtimes and paths, not content (research H5, item 3), so a `target/` carried without the source files' original mtimes rebuilds every local crate. pnpm does not care, which is why the first gate would pass without this and a Rust job would not.

`RelPath` is a parsed newtype in `fabric-core`, never a bare string. Decoding a manifest rejects, with `ManifestError::BadPath { index, reason }`, any path that is empty, starts with `/`, contains a NUL byte, has an empty, `.` or `..` component, or is not NFC-normalized. A `.git` component is rejected everywhere, including in the paths `git ls-tree` reports for `W`: `git add` refuses such a path, but a tree object inside a bundle from a compromised peer can hold one, and the file writer would otherwise write into the target's `.git`. Two entries with the same path are `ManifestError::DuplicatePath`. A symlink `target` is an unparsed string written verbatim with `symlink(2)`; it is safe because restore never writes through a link (below). `mode` is applied as `mode & 0o777`; setuid, setgid and sticky bits never travel.

- **Git, by commits, not by files.** Uncommitted work is the common state of a job in flight, and a bundle carries only what a ref reaches. Capture therefore builds two commits with private indexes and never touches the real index, the working tree or the user's refs:
  - `I`: copy the index file (`git rev-parse --git-path index`) to a temporary file, then `git write-tree` with `GIT_INDEX_FILE` pointing at the copy, then `git commit-tree <tree> -p HEAD`. Its tree is the staged state, so staged blobs become reachable. `write-tree` on the real index is not read only: it rewrites the cache-tree extension under `index.lock`, and it dies when another git process holds that lock, which the harness's own git commands often do. The copy is attempted a few times while `index.lock` exists, then capture fails with `GitBusy`.
  - `W`: with `GIT_INDEX_FILE` pointing at a temporary index, `git read-tree I`, `git add -A`, `git write-tree`, `git commit-tree <tree> -p HEAD -p I`. Its tree is the working tree: modified, deleted, renamed and mode-changed tracked files, symlinks, and untracked files that are not ignored. This is the shape of `git stash -u` (research H5, option R1).
  - `git update-ref refs/fabric/<job-id>/snap/<ulid> W`, so the objects survive `git gc` on the source.
  - On an unborn branch, `I` and `W` have no `HEAD` parent and `head` records the branch name.
- **Bundle:** `git bundle create <tmp> --all --not <tips>`, where `<tips>` are the object ids in the parent snapshot's `refs`, so a later snapshot carries only new objects. The bundle is full (`bundle_base: None`) when there is no parent, when the chain of incremental bundles since the last full one is 16 long, or when `git bundle verify` fails on the incremental one. `--all` includes `refs/fabric/*`, `refs/stash`, tags and remote-tracking refs. Only the newest stash travels; older entries live in the reflog, which is not carried, and `fabric suspend` warns when `git stash list` has more than one entry. `git stash list` reads the reflog too, so a fetched `refs/stash` alone is invisible to it; restore step 4.2 recreates the reflog entry.
- **Carried as a projection, never as bytes:** the repository config. `.git/config` is executable: `core.fsmonitor`, `core.hooksPath`, `core.sshCommand`, `credential.helper = !cmd`, `diff.external` and `filter.*.clean|smudge` all run commands, and a `remote.*.url` or `http.*.extraheader` can hold a token that the secret policy never sees because config is a struct field, not a path. Capture therefore reads the config with `git config --list` and keeps only these keys: `remote.<n>.url` and `remote.<n>.pushurl` with any userinfo stripped (and the strip listed in `fabric suspend` output), `remote.<n>.fetch`, `branch.<n>.remote`, `branch.<n>.merge`, `branch.<n>.rebase`, `push.default`, `core.ignorecase`, `core.autocrlf`, `core.symlinks`. Every other key stays on the source node. Restore writes the projection with `git config` as its last step (below), after every git command Fabric runs has finished. `.git/info/exclude` is carried as bytes; it is patterns, not code.
- **Not carried:** `.git/objects` as files, reflogs, hooks (executable code, installed per node), and submodule repositories. The gitlink commit travels in the tree; the target leaves submodules uninitialized and lists them. A submodule with a dirty working tree or local commits makes capture fail with `GitSubmoduleDirty`. A tree whose `.gitattributes` declares `filter=lfs` makes capture fail with `GitLfsUnsupported`: `git add` would store pointers and the restored working tree would hold pointers where the agent had content.
- **Where git's files are:** never at a literal `.git/<name>`. In a linked worktree `.git` is a file, and the per-worktree state lives under `<common>/worktrees/<name>/`. Every path Fabric reads or writes in a repository is resolved with `git rev-parse --git-path <name>` (`index`, `MERGE_HEAD`, `info/exclude`, and the rest) or `git rev-parse --git-common-dir`, so the checks below look in the right place for either shape.
- **Refused states:** capture fails with `GitBusy` when `rebase-merge`, `rebase-apply`, `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD` or `BISECT_LOG` exists at its resolved path, or when the index has conflict entries. These live outside refs and no bundle can carry them; the user finishes or aborts the operation first.
- **Linked worktrees are refused in v0.** When `--git-dir` and `--git-common-dir` resolve to different directories, capture fails with `GitLinkedWorktree { common_dir }` before writing anything. The reason is ownership, not mechanics: a linked worktree shares its refs and config with every other worktree of that repository, which may belong to other jobs. A snapshot of one job would carry their branches in `--all`, and the ref reconciliation on restore (below) would delete or move them on the target. `fabric job init` (ADR 0001) says so on the spot and names the alternative: a primary clone, which a local `git clone` of the repository makes cheaply by hardlinking objects. Whether a linked worktree can be captured with a restricted ref set (`HEAD`, its branch and upstream, `refs/fabric/<job-id>/*`) is an open question below; a restore always produces a primary repository on the target either way.
- **Untracked files** travel inside `W`, so `files` holds only ignored files selected by policy. When `git` is `None`, `files` holds every path under the root and the same secret policy applies.
- **Ignored files, by policy:** caches in, secrets out. v0 policy, in order: deny any path with a basename or a component matching a secret pattern; allow the cache paths listed in the `EnvSpec` (`node_modules`, `.pnpm-store`, `target`, `.next`, `.venv`, `__pycache__`, `.turbo`); deny all other ignored paths. The v0 secret patterns: `.env`, `.env.*`, `*.env`, `.envrc`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`, `*.tfstate`, `*.tfstate.*`, `id_*`, `credentials*`, `.credentials*`, `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.aws`, `.ssh`, `.gnupg`, `.docker/config.json`. Matching against components as well as the basename is what makes `config/.aws/credentials` and `.ssh/id_ed25519` both denied. The policy is a data file in the job, not code, so the user can widen it per job. A denied path is listed in `fabric suspend` output so nothing silently stays behind.

  Inside a cache path the patterns narrow, because many npm packages ship `*.pem` and `*.key` test fixtures and a `node_modules` with holes is worse than a `node_modules` left behind: it looks warm and is broken. Under a cache path only the high-confidence patterns apply: `.env`, `.env.*`, `.envrc`, `id_*`, `credentials*`, `.credentials*`, `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.aws`, `.ssh`, `.gnupg`. If anything under a cache path is still denied, that cache path is recorded in `partial_caches`, and materialize treats it as missing (ADR 0007) and reinstalls over it. The agent never meets a package with a file missing and no signal.
- **Secrets and `W`:** the same patterns apply to the working-tree commit. An untracked `.env` the user forgot to ignore would otherwise travel inside `W`, so capture runs `git add -A` with `-c core.excludesFile=<tmp>` holding the patterns, and lists what that excluded. A secret already committed to git is in `HEAD` and travels in the bundle; Fabric warns when a tracked path matches a pattern and does not rewrite history.
- **Harness state:** whatever the adapter returns from `capture` (ADR 0006). Hashed and chunked like any other file.
- **Platform:** recorded so that restore knows whether carried caches are usable (ADR 0007). `{ os, arch }` is a v0 approximation that holds only for same-user, same-layout machines: native Node addons also depend on the Node ABI, and pnpm refuses a tree whose recorded `storeDir` or `nodeLinker` differs (research H5, item 2). Equal on the two gate Macs, so it is accepted now and listed below.

### Publish: atomic, crash-safe

1. For each new chunk, including the chunks of the manifest itself: write to `tmp/<random>`, `fsync` the file, `rename` to `chunks/ab/cdef...`, and remember the `chunks/ab/` directory it landed in. Rename on the same filesystem is atomic; a crash leaves either nothing or a complete chunk under its final name. An existing final name means the chunk is already present and the temp file is deleted.
2. Insert chunk rows into the catalog in one transaction after all renames.
3. Write the assembled manifest to `tmp/`, `fsync`, rename into `manifests/`.
4. Insert the snapshot row (id, byte length, manifest chunk list) and `snapshot_chunks` rows in one transaction.
5. `fsync` every `chunks/ab/` directory remembered in step 1, then `manifests/`. A rename is durable only once its parent directory is, so the parent of each renamed file is the directory that gets the call; `chunks/` itself gains nothing from it.
6. Only now append `SnapshotPublished` to the job ledger (ADR 0002), with the full flush described there.

**What this survives.** A process kill at any point, always: the kernel finishes the writes that were issued, and the ordering above means the ledger never names a snapshot whose chunks were not renamed first. Power loss or a kernel panic, only with full flushes: on macOS `fsync` does not flush the drive's write cache, so every commit point (the ledger append, and the directory `fsync` after each rename under `chunks/`, `manifests/` and `jobs/<job-id>/`) issues `fcntl(F_FULLFSYNC)` instead, the call SQLite makes under its `fullfsync` pragma. Without it a ledger that names a snapshot could outlive a chunk file that was still in the cache, which is a published snapshot that cannot be restored. Linux `fsync` already covers the device cache. The catalog runs with `synchronous=FULL` and `fullfsync=ON` for the same reason.

The ledger event is the single commit point. A snapshot is live on a node if a ledger event on that node references it or a row in `pins` names it. Chunks and manifests that no live snapshot reaches are unreferenced: invisible to every reader, and removed only by `fabric store gc`.

`pins` holds snapshot ids that must survive gc although no ledger event names them. `fabric orphan export` (ADR 0003) writes one for the snapshot it produces, because the node that exports is fenced and cannot append to the chain. `fabric status` lists pinned snapshots as orphan snapshots. A later `fabric orphan restore <snapshot> --into <dir>` materializes one into a directory the user names; recovery is the one place a path is typed.

### Suspend: a resumable sequence

Suspend is several durable steps, and a `kill -9` can land between any two of them. The steps are ordered so that the marker and the ledger head together say exactly how far it got, and every command on the job finishes an interrupted suspend before doing its own work.

1. Read the harness record (ADR 0003). If the harness is running, call `stop` with the deadline. If it does not stop, suspend fails here, writes nothing, and the job stays live.
2. Write the marker as `Suspending { epoch, head }` with the current epoch and ledger head. From this point nothing relaunches a harness in this root: `resume` on a `Suspending` marker finishes the suspend first and then claims as usual.
3. Capture and publish. The `SnapshotPublished { S }` append is the snapshot's commit point.
4. Append `Suspended { S }`.
5. Append `LeaseReleased`.
6. Write the marker as `Clean` with `base: S`.
7. Push the records and the snapshot to paired peers (ADR 0003, Replication) and print the result.

Finishing an interrupted suspend reads the marker and the ledger. If the epoch in the ledger is higher than `Suspending.epoch`, this node was fenced in the meantime; the sequence stops, the root is an orphan workspace, and `fabric orphan export` is the way out. Otherwise the head decides, and the harness is known to be stopped because step 1 ran before the marker was written and nothing relaunches under `Suspending`:

- head equals `Suspending.head`: the crash was before the publish. Continue at step 3.
- head is `SnapshotPublished { S }` whose `prev` chain reaches `Suspending.head`: continue at step 4 with that `S`. No second capture, so no duplicate snapshot and no chance to publish edits that were never made.
- head is `Suspended { S }`: continue at step 5.
- head is `LeaseReleased`: continue at step 6 with the `S` the preceding `Suspended` names.

A snapshot published while the marker was `Dirty`, by a command other than suspend, never confuses this: the sequence only ever looks at records after the head it recorded. The SIGKILL matrix for suspend has one row per boundary above, and each asserts that a rerun ends with the same `S` in `Suspended`, a released lease and a `Clean` marker at `S`.

`fabric store gc` runs inside the lock holder and is serialized with publish and fetch by an in-process guard: gc refuses to start while a publish or fetch is in flight (`GcBusy`), and a publish or fetch refuses to start during gc. It computes the reachable set from every job ledger on the node plus every row in `pins`, and removes only chunks and manifests outside that set. It never runs on open and never automatically.

### Restore: path-relative, never over unpublished work

Every workspace Fabric manages is described by a marker at `<fabric-data>/jobs/<job-id>/marker`, outside the workspace:

```text
Marker {
  root:   AbsPath,              the workspace directory this marker describes, on this node
  job_id: JobId,
  base:   Option<SnapshotId>,   the last snapshot the root was brought to; None for a root adopted by `fabric job init` before its first snapshot
  state:  Clean | Dirty | Suspending { epoch: Epoch, head: [u8; 32] } | Restoring { targets: [SnapshotId] },
}
```

`Restoring.targets` is every target the in-flight restore has aimed at, newest last. `Suspending.head` is the ledger head when the harness was stopped; it is what lets an interrupted suspend tell what it already did.

Nothing Fabric owns lives under the root. A marker inside the root would be swept into `W` by `git add -A`, appear in the user's `git status` where the agent can commit it, and be refused by the path rules on the next restore. The marker names the root it describes instead, and `root` is the one host path in a node-local structure; it never enters a snapshot.

`Clean` means every byte under `root` that Fabric manages is in `base`. `restore` writes `Clean` when it finishes; `launch` flips it to `Dirty` before the harness starts; `suspend` writes `Clean` with `base` set to the snapshot it just published, after the ledger event. The marker is written by temp, `fsync`, rename, by the lock holder only.

1. Resolve the target workspace root by policy (ADR 0008). The user never types it.
2. If `<root>` does not exist, the restore is a fresh one, whatever the marker says: a marker whose root is gone describes nothing. Write the marker as `Restoring { targets: [target] }` first, then write everything into `<root>.fabric-restoring/`, verify every chunk hash as it is written, `fsync`, rename the directory to `<root>`, then write the marker as `Clean` with `base: target`. The rename is only ever onto a path that does not exist, so it cannot fail on a non-empty target. A crash before the rename leaves a `.fabric-restoring` directory that the next restore removes and starts again; it never held anything that was not in the store. A crash after the rename leaves a root whose marker says `Restoring`, which continues in place (below) and finishes at once because every byte is already there. Writing the marker before the rename is what keeps a crash between the two from producing an unmarked root.
3. If `<root>` exists, the marker decides. Nothing is ever written into a root whose marker does not allow it.
   - No marker, or a marker whose `root` is a different directory: refuse with `WorkspaceOccupied { root }`. The user moves the directory, or runs `fabric job init` there to adopt it, or runs `fabric orphan export` on it.
   - `Dirty`: a harness ran here and nothing was published since. If `base` is the target and this node holds the lease at the current epoch (a resume after a crash), skip restore and relaunch; the edits are kept. Otherwise refuse with `WorkspaceOccupied` and name `fabric orphan export`.
   - `Clean` with `base` equal to the target: skip restore and relaunch.
   - `Clean` with `base` different from the target, or `Restoring`: update in place. `Clean` with a `base` that is newer than the target, or not on the job's chain at all, is this case too: after a fork resolution the root sits at an orphan snapshot, which is published and pinned, so its bytes are as replaceable as any other published bytes. Write `Restoring { targets }`, where `targets` is the previous list with the new target appended, or `[target]` from `Clean`. Then bring the root to the target: fetch the bundles the root has not seen, set `HEAD`, `git read-tree <I>`, write every path of `W` and `files` whose on-disk bytes do not already hash to the expected blob or chunks, remove every path that `base` or any earlier entry of `targets` lists and the target does not, then write `Clean` with `base: target`. The union matters: a restore interrupted on the way to `T1` may have written files that are in neither `base` nor `T2`, and a removal set computed from `base` alone would leave them behind under a marker that says `Clean`. Paths that no snapshot in that set lists, such as a denied `.env`, are left alone. A `Restoring` root holds no unpublished work by construction, so continuing after a crash is safe, and with the union it is safe toward a different target too. This path is what makes bouncing between two Macs cheap: `node_modules` stays and only the diff is written.

   The only files Fabric ever removes are in a root whose marker says every managed byte is in a published snapshot.
4. Restore git, in this order. `git bundle unbundle` is not used anywhere: it writes objects but creates no refs, so `HEAD` would stay unborn.
   1. `git init`. Write `.git/info/exclude` from its chunks. The config projection is not written yet.
   2. For each bundle in the chain, from the newest full one to this snapshot, oldest first: `git bundle verify`, then `git fetch --update-head-ok <bundle> '+refs/*:refs/*'`. A fetch only creates and moves refs, so the target's ref set is then reconciled to exactly `GitState.refs`: every ref in the list is set to its recorded object id with `git update-ref`, and every ref under `refs/` that the list does not name is deleted. A branch deleted between two snapshots disappears on the target too, and a branch that moved backwards does not keep its old objects reachable. If `refs/stash` is in the list, it is written as `git update-ref -d refs/stash` followed by `git update-ref --create-reflog -m 'fabric restore' refs/stash <sha>`, because `git stash list` and `git stash pop` read `stash@{0}` from the reflog and a ref without one looks like no stash at all (checked on git 2.54: without the reflog, `git stash list` is empty; with it, `stash@{0}` is back).
   3. Set `HEAD`: `git symbolic-ref HEAD refs/heads/<branch>`, or `git update-ref --no-deref HEAD <sha>` for a detached head.
   4. `git read-tree <I>`. The index is now the staged state; the working tree is still empty.
   5. Write the tree of `W` into the root with Fabric's own file writer (`git ls-tree -r -z W` for paths, modes and blob ids, `git cat-file --batch` for bytes). It is the same loop that writes `files`, so modes, symlinks and path rules are enforced in one place, and the index is untouched. Then `git update-index --refresh`, ignoring its exit status, which is nonzero whenever a file differs from the index, which is expected here.
   6. Only now write `files`. Apply `worktree_mtimes` and every `Entry.mtime_ns` as each path is written, so the relative order of source and build output is the one the source machine had.
   7. Last, write `GitState.config` with one `git config` call per key. Nothing Fabric runs reads the repository config after this point, so a key in the projection can never steer one of Fabric's own git calls.

   **How Fabric runs git.** Every git command Fabric invokes during capture and restore, on the source and on the target, runs with `-c core.fsmonitor=false -c core.hooksPath=/dev/null -c core.sshCommand=/usr/bin/false -c credential.helper= -c protocol.allow=never -c protocol.file.allow=always`, with `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_TERMINAL_PROMPT=0` and `GIT_OPTIONAL_LOCKS=0` in its environment, and with `GIT_AUTHOR_*` and `GIT_COMMITTER_*` set explicitly for the snapshot commits. Neither the carried projection, nor the repository's own config, nor the user's global config can inject a command into a git process that Fabric started. This was checked on the real failure: with `core.fsmonitor` pointing at a script, a plain `git update-index --refresh` (restore step 5) executes it; with the flags above it does not. The user's own git, run by the user or the harness in the workspace, is not Fabric's to sandbox and keeps the user's config.

   The file writer defends the root against the manifest, because the manifest can come from a buggy adapter, a corrupted peer store or a paired node that is itself compromised, and the hash chain of ADR 0002 is not security. Within each of `W`, `files` and `harness_state` it creates directories first, then files, then symlinks last, so no link exists yet when the bytes are written. Every open goes through a directory handle, component by component with `O_NOFOLLOW`, never through a path string joined to the root; `cap-std` is the planned dependency for this and `openat2` with `RESOLVE_BENEATH` is used where the kernel has it. This covers the in-place path too, where links from the previous snapshot already exist. Before writing anything the writer probes the volume for case folding by creating two names that differ only in case under the staging directory; on a case-insensitive volume two entries that collide under case folding are refused with `RestoreError::PathCollision { a, b }` rather than one silently overwriting the other. Each of these rules gets one restore test with a manifest that breaks it.

   After step 5, `git status` on the target equals `git status` on the source: staged is `I` against `HEAD`, unstaged is the disk against `I`, untracked is what `W` has that `I` does not.
5. Rewrites: the manifest stores relative paths only. Harness state may contain absolute paths pointing at the source workspace root; the adapter's `restore` receives `(source_root, target_root)` and performs its own rewrites, which in v0 means none because the roots are equal (ADR 0006, ADR 0008). The core never guesses which bytes are paths.
6. If `platform` differs from the node or a cache path is missing, mark the environment as needing materialization (ADR 0007).

Restore is idempotent by marker: a second restore of the same snapshot into the same root is a no-op, and a crashed in-place restore continues where it stopped.

### Transfer

A node fetches a snapshot by id: it asks the peer for the manifest's byte length and chunk list, fetches those chunks like any file, assembles the manifest, checks that the bytes hash to the id, decodes it under the size cap, and only then asks for the chunk ids it does not have, in batches. Each chunk is verified on arrival. A dropped connection resumes from the catalog's view of what is present (ADR 0005).

## Alternatives considered

1. **Full `git bundle --all` on every move, as T3 Code does.** Loses ignored files and the index split, re-sends history on every snapshot (46.5 MB per move for sardis in the research), 50 MiB cap in T3. Rejected; the incremental bundle plus snapshot commits above keeps plain git on both ends and carries only new objects.
2. **Carry the index file as bytes.** Staged blobs are loose objects no ref reaches, so the bundle leaves them out and the target gets an index that points at missing objects. Rejected; `I` makes them reachable.
3. **`git diff --binary` patches.** Need the exact base on the target; mode, rename and symlink edge cases; no history for unpushed merges. Rejected.
4. **Copy `.git` at the file level.** Most faithful (reflogs, hooks, in-progress operations) but unsafe while git runs, and a `.git` shared by many worktrees belongs to many jobs at once. Rejected.
5. **rsync or Syncthing or Mutagen.** All document "exclude `.git` and `node_modules`", which is the opposite of the requirement (landscape section 8.1). No content addressing across snapshots. Rejected.
6. **iroh-blobs.** BLAKE3-verified streaming, mature. It fixes the transfer format and pulls in iroh's networking. Named in ADR 0005 as the alternative if the own transport fails. Rejected for v0 by the no-vendor-dependency rule.
7. **Whole-file hashing without chunking.** Simpler. A one-line change to a 20 MB lockfile or a rebuilt `target/` directory would re-send everything. Rejected.
8. **Fixed-size chunking.** Insertions shift every later chunk. FastCDC exists for this reason. Rejected.
9. **Filesystem snapshots (APFS, btrfs).** Node-local and not content-addressed across machines. Rejected.
10. **Rebuild caches on the target instead of carrying them** (Flox, sccache, Turborepo; landscape W7). This is the fallback when platform differs, not the primary path. A `pnpm install` on a cold Mac is minutes; a chunk diff is seconds.

## Consequences

- Second and later snapshots of a job cost roughly the changed bytes.
- Two jobs on the same node that share `node_modules` content share chunks.
- Fabric excludes files matching the secret policy from `files` and from `W`, and relies on each adapter to redact its own state (ADR 0006). The suspend output lists excluded paths and redaction counts. A secret already committed to git travels in the bundle, and the output says so.
- Catalog corruption is recoverable by rescanning `chunks/` and `manifests/` since the files are self-named by hash.
- Crash tests will re-exec the publisher and `kill -9` it at each step boundary, then assert that `fabric store check` reports either no snapshot or a complete one.
- A crash test kills an in-place restore toward `T1`, continues it toward `T2`, and asserts that no file unique to `T1` remains under a root marked `Clean` at `T2`.
- A bounce test moves one workspace from a first node to a second and back to the first, in place, three times, and asserts after each hop that the restored root is byte-identical to the captured one and that the user's `git status` shows nothing Fabric owns.
- A git fidelity test (research H5, experiment E7) captures a scratch repository holding staged, unstaged, untracked, deleted, renamed, mode-changed, symlink and intent-to-add entries, restores it, and compares `git status --porcelain=v2`, `git diff`, `git diff --cached`, `git for-each-ref` and `git stash list` byte for byte. It also covers a repository with no remote, an unborn branch, a branch deleted between two snapshots, and a second snapshot whose bundle is incremental.

## Open questions

- Intent-to-add, skip-worktree and assume-unchanged are index flags, not tree content, and `git write-tree` may omit or reject intent-to-add entries. E7 decides whether `I` needs a side list of flags or whether v0 refuses those states like `GitBusy`.
- Linked worktrees. Efe's own convention is a worktree per unit of work (ADR 0009), so `GitLinkedWorktree` will be the first refusal a real job sees. The candidate is a restricted ref set per job (`HEAD`, its branch and upstream, `refs/fabric/<job-id>/*`) with reconciliation limited to that set. Decide after the first gate, with E7 extended to a branch checked out in another worktree.
- The platform key needs the Node major version (or `node_modules/.modules.yaml`'s `storeDir` and `nodeLinker`) and the toolchain hash before a third machine joins. Add them when the first mismatch is observed, not before.
- Chunk size targets are a guess. Measure dedup ratio on a real `node_modules` and `target/` before freezing.
- Hard links inside `node_modules` (pnpm) restore as copies in v0. Preserving them needs inode tracking; decide after measuring size impact.
- Extended attributes and macOS quarantine flags: dropped in v0. Revisit if a tool breaks.
- Large binary files that change every build (`.next/cache`): maybe exclude by default. Measure.
