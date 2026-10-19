# ADR 0007: Environment spec

Status: accepted, 2026-10-19

## Context

The job's workspace arrives with its caches when the platform matches (ADR 0004). When it does not, or when a cache was excluded, the node must be able to make itself ready without the user typing anything. The standard for describing environments is `devcontainer.json` (about 1.69M monthly npm downloads of `@devcontainers/cli` per the landscape, section 7). But a devcontainer is a container recipe, and v0 runs harnesses directly on two Macs with no container.

Also: environments contain secrets, and secrets must not travel in snapshots (ADR 0004 policy).

## Decision

### `EnvSpec` v0

```text
EnvSpec {
  version:       u16,
  tools:         [Tool],         detected, informational, used to warn on mismatch
  setup:         [SetupStep],    commands that make the workspace runnable
  env_var_names: [String],       names only, never values
  cache_paths:   [RelPath],      what counts as a cache for snapshot policy and for "is it warm"
}
Tool      { name: String, version: String, source: Detected | Declared }
SetupStep { run: String, cwd: RelPath, when: CachesMissing | Always }
```

`EnvSpecId` is the blake3 of the postcard bytes. A job references an `EnvSpec` by id; editing produces a new id and a `Note` in the ledger.

### Detection

`fabric job init` (and `fabric suspend` when no spec exists) detects from the workspace:

- `package.json` plus a lockfile: tool `node` (from `.nvmrc`, `.node-version` or `engines`), package manager from `packageManager` or the lockfile name; setup `pnpm install --frozen-lockfile` (or npm, yarn, bun equivalents) with `when: CachesMissing`; cache paths `node_modules`, `.pnpm-store`.
- `Cargo.toml`: tool `rust` from `rust-toolchain.toml`; setup `cargo fetch` with `when: CachesMissing`; cache path `target`.
- `pyproject.toml` or `requirements.txt`: tool `python`; setup `uv sync` or `pip install -r`; cache `.venv`.
- `go.mod`: tool `go`; setup `go mod download`; no cache path in the workspace.
- `.env.example` and `process.env.X` or `std::env::var("X")` patterns: env var names only. Values are never read.

Detection writes the spec and prints it. The user can edit the file. Fabric does not guess twice over an edited spec.

### Materialize on restore, only when needed

After restore, `fabric-env` checks, in order:

1. Does the snapshot `platform` equal this node's? If not, every cache path is treated as missing (a Linux `node_modules` is not a macOS one).
2. For each `cache_path`: does it exist and is it non-empty?
3. If any is missing, run the `setup` steps with `when: CachesMissing` in order, in the workspace, with the node's environment plus the job's env var names resolved from the node's own secret source (v0: the user's shell environment on that node; the step fails with the missing names listed if any is absent).
4. `Always` steps run on every resume.
5. Compare detected tool versions to the spec; a mismatch is a warning in the resume output, not a failure.

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
