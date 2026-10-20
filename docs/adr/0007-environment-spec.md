# ADR 0007: Environment spec

Status: accepted, 2026-10-19

Amended 2026-10-19 after step 1 research. Step 1 of Materialize now reads the per-cache carry key of ADR 0012 instead of the `{ os, arch }` pair alone. The exact strings `fabric doctor` compares (ADR 0008) live in the carry key, not in `tools`, which holds the workspace's own pins as written (corrected in place by the `feat/env` amendment, point 1). Nothing else changed.

Amended 2026-10-19 while writing `fabric-core` (PR `feat/core`): `SetupStep.cwd` is `Option<RelPath>`, where `None` is the workspace root, because a `RelPath` cannot name the root itself (ADR 0004 forbids empty and `.` components). The edited-spec record is `EnvSpecChanged` (ADR 0002), not a free-text `Note`.

Amended 2026-10-19 after the adversarial review of PR `feat/core`: a setup step is `{ program, args, cwd, when }` rather than `{ argv, cwd, when }`, so the shape matches `HarnessKind::Acp` (ADR 0006). The second review found that this alone did not remove the empty command: `program` was a bare string and `""`, `/tmp/x`, `../bin/x` and `-rf` all decoded. `program` is now `ProgramName`, a parsed type that is non-empty and holds no slash, no NUL and no leading `-`, checked inside `Deserialize`, so "looked up on `PATH`" is true by type. The format version is the first varint of the spec's bytes, not a field of the struct (ADR 0002, review amendment 5).

Amended 2026-10-19 after the second adversarial review of PR `feat/core`: `program` is a `ProgramName` (amendment above, corrected in place), `env_var_names` holds `EnvVarName` values that cannot carry a `=`, and `tools`, `env_var_names` and `cache_paths` are strictly sorted so that one content has one `EnvSpecId`; a spec that breaks either rule is refused on encode and on decode.

Amended 2026-10-19 while writing `fabric-env` (PR `feat/env`), again on 2026-10-20 after the second review of that PR (points 11 to 15), and after its third review (points 16 onward). Detection and materialize are corrected in place; what changed and why:

1. `Tool.version` holds a pin as its file wrote it (`22`, `1.97.1`, `>=18`). Detection reads files and never runs a program, so the same files give the same spec on every node; the strings a tool reports about itself (`rustc -vV`, `node --version`) are measured by `fabric doctor` and stored in the carry key (ADR 0008, ADR 0012), which is where an exact comparison is made. Step 5 of materialize is therefore informational: doctor prints the pins beside the measured strings and compares neither, because ADR 0008 allows no range parsing and a pin such as `>=18` or `v20` would never equal a measured `v20.11.1`.
2. The user's edits live in `.fabric/env.toml`, not in an edited copy of the spec (below). The spec is rebuilt from the files and the override file, so "edit the spec" and "detect again" cannot disagree.
3. `.pnpm-store` is not a detected cache path. ADR 0012 never carries pnpm store files, and a project with the usual global store would see the path missing on every resume and install every time.
4. `requirements.txt` gives `uv venv --allow-existing` then `uv pip install -r requirements.txt`, not `pip install -r`: a bare `pip install` writes into whichever Python is first on `PATH`, not into the workspace's `.venv`. `uv.lock` gives `uv sync --frozen`, as in ADR 0012. A `pyproject.toml` without `uv.lock` gives nothing, because a sync would write a new lockfile.
5. `cargo fetch` takes `--locked` when `Cargo.lock` exists, so a resume never rewrites the lockfile, as the frozen installs do for JavaScript.
6. A `package.json` with no lockfile gives no step and no cache path: nothing can rebuild that `node_modules`, so it travels as precious files (ADR 0004). Two lockfiles with nothing to choose between them, or a chosen manager whose lockfile is absent while another's is present, stop detection with a message that names the fix.
7. "Is it warm" is the installer's own marker, or the sentinel of amendment 15 where the installer writes none. Restore writes a carried cache only when its key matches and it is not partial (ADR 0012), so materialize inspects the disk instead of re-reading the snapshot's `caches`.
8. Env var names come only from `env` in `.fabric/env.toml`. Scanning `.env.example` and source code for names is not built; a scan that guesses would put names in the spec that the user never chose.
9. A spec with no cache paths runs its `CachesMissing` steps on every resume. "Is any cache missing" is vacuously false over an empty list, and under that reading `go mod download` (whose cache lives outside the workspace) and any `[[setup]]` declared without a `[[cache]]` would never run. Nothing in an empty list proves the workspace warm, so the steps run; a user who wants them skipped declares the path they rebuild as a `[[cache]]`.
10. Detection reads regular files only. A symbolic link in its fixed list, in any component of the path (`.fabric -> ../other/.fabric` included, since the second review), is refused with the file named, and the opened handle must be the same file the link check saw, because `.nvmrc -> .env` would read a secret into the spec and `.nvmrc -> ../../.nvmrc` would give two nodes two specs from one workspace (found by the review of PR `feat/env`). Every pin, detected or declared, must be at most 64 printable ASCII characters and not shaped like a credential, the same guard setup arguments get, since pins travel in the spec too.
11. Tool names are checked like pins (review of PR `feat/env`, 2026-10-20). Before this only versions were checked, so a credential, a terminal escape sequence or a 5000-character string used as a `[tools]` key, a `.tool-versions` word or a `mise.toml` key went into the spec, and a peer's spec decoded with no check at all. `Tool.name` is now `ToolName` and `Tool.version` is `ToolVersion`, parsed types in `fabric-core` checked inside `Deserialize`, and the credential guard moved to `fabric_core::credential` so the decoder can apply it. Errors name a pin by its file and position, never by its text.
12. A job's secret patterns are kept in its spec, in `secret_patterns` (review of PR `feat/env`, 2026-10-20). They lived only in `.fabric/env.toml`, a working-tree file the agent can rewrite, so deleting a line shrank the list and the next suspend carried what it had denied. `detect` now takes the job's previous spec and keeps the union of its patterns and the file's; a pattern the file dropped stays in force and a warning says so. `SecretPattern` is a parsed type in `fabric-core`, checked inside `Deserialize`, so the list travels with the job and a peer's capture applies it too.
13. Every cache in the detection report has an ecosystem that picks its carry key, and an origin, `Detected` or `Declared` (review of PR `feat/env`, 2026-10-20). A declared cache had no ecosystem at all, while ADR 0012 gives a build output the key of the ecosystem that made it. `[[cache]]` takes an optional `ecosystem` (`node`, `rust`, `python`, `other`); without it the cache takes the single detected ecosystem, `other` when none was detected, and detection stops when two were (ADR 0012, 2026-10-20 amendment). A declared `ecosystem` that differs from the one detection found for the same path is refused.
14. Each detected setup step is tied to the cache it fills, in the detection report rather than the spec (review of PR `feat/env`, 2026-10-20). Every `CachesMissing` step used to run when any cache was not whole, and `cargo fetch` never creates `target`, so an npm and Cargo workspace without `target` ran `npm ci` on every resume and deleted a healthy carried `node_modules` each time. The open question that named this cost is closed: the report is rebuilt from the same files on every node, so the link needs no spec format change.
15. `pyvenv.cfg` is not proof of a whole `.venv` (review of PR `feat/env`, 2026-10-20, verified with uv): `uv venv` writes it first, before any package, so a `kill -9` between `uv venv` and `uv pip install` left a `.venv` that every later plan called healthy, and `uv sync --frozen` fails the same way. Bun writes no marker at all, and a declared cache was judged by content. Each of these is now proven by the sentinel of point 2, which only a step that exited 0 leaves behind; a carried cache keeps the sentinel its source wrote.
16. Installer markers prove only the `node_modules` detection found (third review of PR `feat/env`, 2026-10-20). The proof followed the cache's ecosystem, so a declared `.output` keyed `node` was asked for `.package-lock.json`, never had it, and never got a sentinel either: every resume ran the declared steps, and a declared `npm ci` deleted a healthy carried `node_modules` each time. Detection now decides, once per cache, how it is proven whole and which steps fill it, and materialize reads that instead of working the relation out again.
17. A declared step waits on the caches it fills, not on every cache (third review of PR `feat/env`, 2026-10-20). A declared step counted a missing `target` as a reason to run, although no declared step fills it, so in an npm and Cargo workspace a declared `npm ci` deleted a healthy carried `node_modules` whenever `target` was not carried: the failure point 14 closed for detected steps, back through the override file.

## Context

The job's workspace arrives with its caches when the platform matches (ADR 0004). When it does not, or when a cache was excluded, the node must be able to make itself ready without the user typing anything. The standard for describing environments is `devcontainer.json` (about 1.69M monthly npm downloads of `@devcontainers/cli` per the landscape, section 7). But a devcontainer is a container recipe, and v0 runs harnesses directly on two Macs with no container.

Also: environments contain secrets, and secrets must not travel in snapshots (ADR 0004 policy).

## Decision

### `EnvSpec` v0

```text
EnvSpec {                        the bytes start with the format version as a varint; it is not a field
  tools:         [Tool],         detected or declared pins, informational, printed beside measured versions, never compared; strictly sorted by name
  setup:         [SetupStep],    commands that make the workspace runnable, in the order they run
  env_var_names: [EnvVarName],   names only, never values; `[A-Za-z_][A-Za-z0-9_]*`, strictly sorted
  cache_paths:   [RelPath],      what counts as a cache for snapshot policy and for "is it warm"; strictly sorted
  secret_patterns: [SecretPattern], patterns the job added to the built-in secret list; only grows; strictly sorted
}
Tool      { name: ToolName, version: ToolVersion, source: Detected | Declared }   version is the pin as its file wrote it, never parsed into parts; both are 1 to 64 printable ASCII characters (no space in a name), not credential-shaped, checked on decode
SetupStep { program: ProgramName, args: [String], cwd: Option<RelPath>, when: CachesMissing | Always }   cwd None is the root; ProgramName is a bare name, never a path or an option
```

`program` is looked up on `PATH` and run with `args` directly, never through a shell. A spec travels with the job and a peer can send one, so there is no string for quoting bugs or a hostile `packageManager` field to hide in. Before the node runs a spec whose id differs from the one it last ran or wrote for that job, it prints the steps and asks, unless the command was given `--yes`.

`EnvSpecId` is the blake3 of the postcard bytes. A job references an `EnvSpec` by id; editing produces a new id and an `EnvSpecChanged` record in the ledger. The id is only an identity if the same content has one byte form, so `tools`, `env_var_names` and `cache_paths` are strictly sorted by their key and a spec that is not, from a peer or from a caller, is refused with `EnvSpecError::OutOfOrder { list, index }`; `setup` keeps the order it runs in. An environment variable name is a parsed `EnvVarName` that cannot hold `=`, so a value cannot travel inside a name (second review of PR `feat/core`, 2026-10-19).

### Detection

`fabric job init` (and `fabric suspend` when no spec exists) detects from the files at the workspace root. It reads a fixed list of regular files, follows no link, never reads a file the secret policy denies or an environment variable, and runs no program.

- `package.json` plus a lockfile: the manager is `package_manager` in `.fabric/env.toml`, else `packageManager` in `package.json`, else the only lockfile (`pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lock`, `bun.lockb`). Setup is its frozen install with `when: CachesMissing`: `pnpm install --frozen-lockfile`, `npm ci`, `yarn install --frozen-lockfile` for Yarn 1, `yarn install --immutable` for Yarn 2 and later (from `packageManager`, else `.yarnrc.yml`), `bun install --frozen-lockfile`. Cache path `node_modules`. No lockfile: no step and no cache path.
- `Cargo.toml`: setup `cargo fetch`, plus `--locked` when `Cargo.lock` exists; cache path `target`.
- `uv.lock`: setup `uv sync --frozen`. Else `requirements.txt`: setup `uv venv --allow-existing`, then `uv pip install -r requirements.txt`. Cache path `.venv` for both.
- `go.mod`: setup `go mod download`; no cache path in the workspace.
- Tool pins, first wins per tool: `[tools]` in `.fabric/env.toml` (`Declared`), `mise.toml`, `.tool-versions` (`nodejs` read as `node`, `golang` as `go`), `rust-toolchain.toml`, `.node-version`, `.nvmrc`, `.python-version`, the `go` directive of `go.mod`, then `packageManager` and `engines.node` of `package.json`. The detection report keeps every pin with its file, so a disagreement is visible.
- `.devcontainer/devcontainer.json` or `.devcontainer.json`: recorded in the report, not interpreted.
- Env var names: only those `.fabric/env.toml` declares.

The detection report also gives each cache path its class (ADR 0004) and its ecosystem, which picks the carry key the store measures (ADR 0012), and the secret policy widened by the override file.

### The override file

`.fabric/env.toml` is what the user declares. A declared tool pin replaces a detected one. `package_manager` decides between lockfiles. Any `[[setup]]` entry replaces every detected step, and `setup = []` declares that nothing runs. `[[cache]]` adds a cache path with its class and, optionally, the ecosystem that makes it (amendment 13); a path detection also found keeps its ecosystem, so a declared class cannot send a `node_modules` to a node with another Node major. A cache path under `.fabric`, or one that lies inside or contains another declared or detected cache, is refused: a cache is dropped on a carry mismatch, and a directory that holds the user's work must not become one by accident. A declared cache keeps the full secret list; only the caches detection found get the narrowed one (ADR 0004) (`.git` cannot be a `RelPath` at all). `env` lists variable names. `secret_patterns` widens the secret list. Unknown keys are refused.

The file names things and never carries a value. A `NAME=value` in `env` is refused. A setup argument shaped like a credential (a GitHub, GitLab, npm or Slack token, an `OpenAI` or Anthropic key, an AWS key id, a PEM header, a URL with a password) is refused, because the spec travels to other nodes. No error quotes a value from the file; errors carry key paths, indexes and line numbers, and quote an unknown key's name only when it is at most 64 printable characters and not credential-shaped (review of PR `feat/env`, 2026-10-20: a token used as a key was echoed). Detection skips any file in its list that the job's secret patterns deny, so `secret_patterns = ["mise.toml"]` keeps that file's pins out of the spec.

### Materialize on restore, only when needed

After restore, `fabric-env` checks, in order:

1. Restore has already written each carried cache whose platform and carry key match this node and which is not partial, and skipped the rest (ADR 0012). A cache the secret policy punched a hole in, or one measured under a different pnpm store path or rustc, is therefore absent on disk, however full it was on the source.
2. For each cache path in the detection report: is it a non-empty directory holding proof that whatever filled it finished? For the `node_modules` detection found under npm, pnpm or Yarn the proof is the marker the installer itself writes as it finishes, chosen by the manager detection recorded, not by the lockfiles on disk: `.package-lock.json` for npm, `.modules.yaml` for pnpm, `.yarn-integrity` for Yarn 1, `.yarn-state.yml` for Yarn 2 and later. For a cache a setup step fills whose installer writes no such marker (Bun, `.venv`, a declared cache with declared steps) the proof is `.fabric-installed`, a sentinel materialize removes before the first step that fills the cache starts and writes after the last one exits 0. A cache no step fills, such as a detected `target` that a build makes, needs only content. An install that fails or is killed halfway leaves no proof, so the next resume fills the cache again. `plan` therefore takes the detection report, which is rebuilt from the same files on every node, and not the spec alone.
3. Run each `setup` step with `when: CachesMissing` whose cache is not whole, in order, in the workspace. The detection report records what each step fills: a detected install fills its own cache and runs only when that cache is not whole; `cargo fetch` runs when `target` is not; a step the user declared runs when a cache it fills is not whole, where a declared step counts as filling every cache but a detected `target`; and a step that fills no cache in the workspace, such as `go mod download`, runs when any cache is not whole or the report has none. Steps run with the node's environment plus the job's env var names resolved from the node's own secret source (v0: the user's shell environment on that node; the step fails with the missing names listed if any is absent). The steps run in the `fabric resume` process, not in the daemon, which is why the user's shell environment is available (ADR 0004, one writer). Before any step runs, every planned program must be on `PATH` and every declared variable must be set, or nothing runs and the error lists what is missing. `PATH` entries inside the workspace are skipped, so a restored workspace cannot replace the installer binary. It can still run code through the installer: lifecycle scripts, `.pnpmfile.cjs`, source builds and declared steps all come with the snapshot, so a restored workspace is trusted as far as the peer that signed its snapshot (ADR 0004 threat model), and the prompt before a spec id this node has not run (above) is where the user approves that code. Each line a step prints goes to `tracing` under the target `fabric_env::setup`; computing the plan is the dry run and changes nothing.
4. `Always` steps run on every resume.
5. The spec's pins are informational. `fabric doctor` prints each pin beside the string it measured for that tool (`node --version`, `rustc -vV`), so the user sees `22` next to `v26.1.0`, and compares nothing: ADR 0008 compares exact strings only, and a pin such as `>=18` or `v20` is not one, so an equality test would warn on every resume. The measured strings that do decide anything are in the carry key (ADR 0012). Nothing in materialize reads the pins.

If all caches are present, nothing runs and resume is fast. This is the common case for the two-Mac gate.

### Secrets

Names travel, values do not. Each node resolves values locally. v0 reads the shell environment. Later sources (1Password Environments, Tailscale Aperture, a per-node secrets file) are each a small resolver behind one trait; the research lists both services as integrate-not-rebuild (landscape section 8.1).

### `devcontainer.json` import, later

A `fabric env import devcontainer.json` command will map `features`, `postCreateCommand`, `remoteEnv` names and `mounts` into an `EnvSpec`. Not in v0 because v0 runs no containers and the mapping is lossy; doing it badly would be worse than not doing it.

## Alternatives considered

1. **`devcontainer.json` as the native format.** Container-shaped, large schema, and running it needs Docker or a devcontainer CLI, neither of which the first gate has. Import later instead.
2. **Nix or devenv.** Reproducible, heavy for a user who just has `pnpm` installed. The research marks devenv as reuse for toolchain reproducibility (landscape 8.1). Could become a `Tool.source` later. Not v0.
3. **Carry the whole toolchain in the snapshot** (Node binary, Rust toolchain). Huge, platform-bound, and tool managers already cache them per machine. Rejected.
4. **No spec, always run install.** Makes every resume minutes long on a warm machine. Rejected; the whole point of carrying caches is to skip this.

## Consequences

- A resume on a matching platform with a warm cache runs zero commands.
- A resume on a cold or different platform is slower but needs no user action.
- A spec is small, text, diffable and stored by content hash.
- The secret boundary is simple to audit: one struct field of names, one resolver per node.

## Open questions

- Should detection run on every suspend to notice new tools, or only when asked? v0: only when no spec exists or when `fabric env detect` is run.
- How to express "this job needs a GPU" or "needs Docker"? Probably `requires: [Capability]` on the spec, consumed by placement (ADR 0008). Add when a job needs it.
- Yarn 2 and later with Plug'n'Play writes no `node_modules`, so a PnP workspace runs its install on every resume. Detect `nodeLinker` and use `.pnp.cjs` as the marker when a PnP job appears.
- A cache a user declares that no setup step rebuilds, such as a `dist` the build writes, makes every resume where it is missing run setup. Should a declared `build` or `tool-cache` path count toward "missing"?
- The installer is not tied to the `fabric resume` process. After a `kill -9` of resume during `pnpm install` (the first gate's own case) the child keeps running; the next resume sees `node_modules` without its marker and starts a second install, or an `npm ci` that first deletes the tree, next to the orphan. macOS has no `PR_SET_PDEATHSIG`, and a new process group would stop Ctrl-C from reaching the installer, so the guard belongs above this crate: a pid file under `.fabric/` written before a step runs and checked by `plan`, or the job lease covering materialize. Decide in the store or CLI PR; until then two installers may write one tree.
