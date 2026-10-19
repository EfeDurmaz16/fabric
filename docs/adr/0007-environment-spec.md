# ADR 0007: Environment spec

Status: accepted, 2026-10-19

Amended 2026-10-19 after step 1 research. Step 1 of Materialize now reads the per-cache carry key of ADR 0012 instead of the `{ os, arch }` pair alone, and `tools` records the exact strings `fabric doctor` compares (ADR 0008). Nothing else changed.

Amended 2026-10-19 while writing `fabric-core` (PR `feat/core`): `SetupStep.cwd` is `Option<RelPath>`, where `None` is the workspace root, because a `RelPath` cannot name the root itself (ADR 0004 forbids empty and `.` components). The edited-spec record is `EnvSpecChanged` (ADR 0002), not a free-text `Note`.

Amended 2026-10-19 after the adversarial review of PR `feat/core`: a setup step is `{ program, args, cwd, when }` rather than `{ argv, cwd, when }`, so the shape matches `HarnessKind::Acp` (ADR 0006). The second review found that this alone did not remove the empty command: `program` was a bare string and `""`, `/tmp/x`, `../bin/x` and `-rf` all decoded. `program` is now `ProgramName`, a parsed type that is non-empty and holds no slash, no NUL and no leading `-`, checked inside `Deserialize`, so "looked up on `PATH`" is true by type. The format version is the first varint of the spec's bytes, not a field of the struct (ADR 0002, review amendment 5).

Amended 2026-10-19 after the second adversarial review of PR `feat/core`: `program` is a `ProgramName` (amendment above, corrected in place), `env_var_names` holds `EnvVarName` values that cannot carry a `=`, and `tools`, `env_var_names` and `cache_paths` are strictly sorted so that one content has one `EnvSpecId`; a spec that breaks either rule is refused on encode and on decode.

Amended 2026-10-19 while writing `fabric-env` (PR `feat/env`). Detection and materialize are corrected in place; what changed and why:

1. `Tool.version` holds a pin as its file wrote it (`22`, `1.97.1`, `>=18`). Detection reads files and never runs a program, so the same files give the same spec on every node; the strings a tool reports about itself (`rustc -vV`, `node --version`) are measured by `fabric doctor` and stored in the carry key (ADR 0008, ADR 0012), which is where an exact comparison is made. Step 5 of materialize, the version warning, therefore belongs to doctor and is not in `fabric-env`.
2. The user's edits live in `.fabric/env.toml`, not in an edited copy of the spec (below). The spec is rebuilt from the files and the override file, so "edit the spec" and "detect again" cannot disagree.
3. `.pnpm-store` is not a detected cache path. ADR 0012 never carries pnpm store files, and a project with the usual global store would see the path missing on every resume and install every time.
4. `requirements.txt` gives `uv venv --allow-existing` then `uv pip install -r requirements.txt`, not `pip install -r`: a bare `pip install` writes into whichever Python is first on `PATH`, not into the workspace's `.venv`. `uv.lock` gives `uv sync --frozen`, as in ADR 0012. A `pyproject.toml` without `uv.lock` gives nothing, because a sync would write a new lockfile.
5. `cargo fetch` takes `--locked` when `Cargo.lock` exists, so a resume never rewrites the lockfile, as the frozen installs do for JavaScript.
6. A `package.json` with no lockfile gives no step and no cache path: nothing can rebuild that `node_modules`, so it travels as precious files (ADR 0004). Two lockfiles with nothing to choose between them, or a chosen manager whose lockfile is absent while another's is present, stop detection with a message that names the fix.
7. "Is it warm" is the installer's own marker. Restore writes a carried cache only when its key matches and it is not partial (ADR 0012), so materialize inspects the disk instead of re-reading the snapshot's `caches`.
8. Env var names come only from `env` in `.fabric/env.toml`. Scanning `.env.example` and source code for names is not built; a scan that guesses would put names in the spec that the user never chose.

## Context

The job's workspace arrives with its caches when the platform matches (ADR 0004). When it does not, or when a cache was excluded, the node must be able to make itself ready without the user typing anything. The standard for describing environments is `devcontainer.json` (about 1.69M monthly npm downloads of `@devcontainers/cli` per the landscape, section 7). But a devcontainer is a container recipe, and v0 runs harnesses directly on two Macs with no container.

Also: environments contain secrets, and secrets must not travel in snapshots (ADR 0004 policy).

## Decision

### `EnvSpec` v0

```text
EnvSpec {                        the bytes start with the format version as a varint; it is not a field
  tools:         [Tool],         detected, informational, used to warn on mismatch; strictly sorted by name
  setup:         [SetupStep],    commands that make the workspace runnable, in the order they run
  env_var_names: [EnvVarName],   names only, never values; `[A-Za-z_][A-Za-z0-9_]*`, strictly sorted
  cache_paths:   [RelPath],      what counts as a cache for snapshot policy and for "is it warm"; strictly sorted
}
Tool      { name: String, version: String, source: Detected | Declared }   version is the pin as its file wrote it, never parsed into parts
SetupStep { program: ProgramName, args: [String], cwd: Option<RelPath>, when: CachesMissing | Always }   cwd None is the root; ProgramName is a bare name, never a path or an option
```

`program` is looked up on `PATH` and run with `args` directly, never through a shell. A spec travels with the job and a peer can send one, so there is no string for quoting bugs or a hostile `packageManager` field to hide in. Before the node runs a spec whose id differs from the one it last ran or wrote for that job, it prints the steps and asks, unless the command was given `--yes`.

`EnvSpecId` is the blake3 of the postcard bytes. A job references an `EnvSpec` by id; editing produces a new id and an `EnvSpecChanged` record in the ledger. The id is only an identity if the same content has one byte form, so `tools`, `env_var_names` and `cache_paths` are strictly sorted by their key and a spec that is not, from a peer or from a caller, is refused with `EnvSpecError::OutOfOrder { list, index }`; `setup` keeps the order it runs in. An environment variable name is a parsed `EnvVarName` that cannot hold `=`, so a value cannot travel inside a name (second review of PR `feat/core`, 2026-10-19).

### Detection

`fabric job init` (and `fabric suspend` when no spec exists) detects from the files at the workspace root. It reads a fixed list of files, never one the secret policy denies, never an environment variable, and runs no program.

- `package.json` plus a lockfile: the manager is `package_manager` in `.fabric/env.toml`, else `packageManager` in `package.json`, else the only lockfile (`pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lock`, `bun.lockb`). Setup is its frozen install with `when: CachesMissing`: `pnpm install --frozen-lockfile`, `npm ci`, `yarn install --frozen-lockfile` for Yarn 1, `yarn install --immutable` for Yarn 2 and later (from `packageManager`, else `.yarnrc.yml`), `bun install --frozen-lockfile`. Cache path `node_modules`. No lockfile: no step and no cache path.
- `Cargo.toml`: setup `cargo fetch`, plus `--locked` when `Cargo.lock` exists; cache path `target`.
- `uv.lock`: setup `uv sync --frozen`. Else `requirements.txt`: setup `uv venv --allow-existing`, then `uv pip install -r requirements.txt`. Cache path `.venv` for both.
- `go.mod`: setup `go mod download`; no cache path in the workspace.
- Tool pins, first wins per tool: `[tools]` in `.fabric/env.toml` (`Declared`), `mise.toml`, `.tool-versions` (`nodejs` read as `node`, `golang` as `go`), `rust-toolchain.toml`, `.node-version`, `.nvmrc`, `.python-version`, the `go` directive of `go.mod`, then `packageManager` and `engines.node` of `package.json`. The detection report keeps every pin with its file, so a disagreement is visible.
- `.devcontainer/devcontainer.json` or `.devcontainer.json`: recorded in the report, not interpreted.
- Env var names: only those `.fabric/env.toml` declares.

The detection report also gives each cache path its class (ADR 0004) and its ecosystem, which picks the carry key the store measures (ADR 0012), and the secret policy widened by the override file.

### The override file

`.fabric/env.toml` is what the user declares. A declared tool pin replaces a detected one. `package_manager` decides between lockfiles. Any `[[setup]]` entry replaces every detected step, and `setup = []` declares that nothing runs. `[[cache]]` adds a cache path with its class; a path detection also found keeps its ecosystem, so a declared class cannot send a `node_modules` to a node with another Node major. `env` lists variable names. `secret_patterns` widens the secret list. Unknown keys are refused.

The file names things and never carries a value. A `NAME=value` in `env` is refused. A setup argument shaped like a credential (a GitHub, GitLab, npm or Slack token, an `OpenAI` or Anthropic key, an AWS key id, a PEM header, a URL with a password) is refused, because the spec travels to other nodes. No error quotes text from the file; errors carry key paths, indexes and line numbers.

### Materialize on restore, only when needed

After restore, `fabric-env` checks, in order:

1. Restore has already written each carried cache whose platform and carry key match this node and which is not partial, and skipped the rest (ADR 0012). A cache the secret policy punched a hole in, or one measured under a different pnpm store path or rustc, is therefore absent on disk, however full it was on the source.
2. For each cache path in the detection report: is it a non-empty directory holding the marker its installer writes last? The marker follows the cache's ecosystem, which is the manager detection chose, not the lockfiles on disk: `node_modules` needs `.modules.yaml` for pnpm, `.package-lock.json` for npm, `.yarn-integrity` for Yarn 1, `.yarn-state.yml` for Yarn 2 and later, and nothing for Bun, so a Bun tree next to a leftover `package-lock.json` is whole when it has content. `.venv` needs `pyvenv.cfg`, any other path needs content. A first install killed halfway leaves no marker, so the next resume installs again. `plan` therefore takes the detection report, which is rebuilt from the same files on every node, and not the spec alone.
3. If any is missing, run the `setup` steps with `when: CachesMissing` in order, in the workspace, with the node's environment plus the job's env var names resolved from the node's own secret source (v0: the user's shell environment on that node; the step fails with the missing names listed if any is absent). The steps run in the `fabric resume` process, not in the daemon, which is why the user's shell environment is available (ADR 0004, one writer). Before any step runs, every planned program must be on `PATH` and every declared variable must be set, or nothing runs and the error lists what is missing. `PATH` entries inside the workspace are skipped, so a restored workspace cannot supply its own installer. Each line a step prints goes to `tracing` under the target `fabric_env::setup`; computing the plan is the dry run and changes nothing.
4. `Always` steps run on every resume.
5. Compare the measured tool versions to the spec's pins; a mismatch is a warning in the resume output, not a failure. This is `fabric doctor`'s comparison (amendment 1) and is not in `fabric-env`.

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
- A missing cache runs every `CachesMissing` step, because a step does not name the caches it rebuilds. A spec with `node_modules` and `target` runs `pnpm install` when only `target` is missing. Linking a step to its caches is a spec format change; decide when the cost shows up.
- Yarn 2 and later with Plug'n'Play writes no `node_modules`, so a PnP workspace runs its install on every resume. Detect `nodeLinker` and use `.pnp.cjs` as the marker when a PnP job appears.
- A cache a user declares that no setup step rebuilds, such as a `dist` the build writes, makes every resume where it is missing run setup. Should a declared `build` or `tool-cache` path count toward "missing"?
