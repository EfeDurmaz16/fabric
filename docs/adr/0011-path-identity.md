# ADR 0011: Path identity

Status: accepted, 2026-10-19

Written 2026-10-19 after step 1 research (`2026-10-19-step1-design-input.md`, section 4.1 and D1; `step1/H5-workspace-mobility.md`, section 2; `step1/H1-claude-code.md`, sections 1 and 11.1; `step1/preflight-2026-10-19.md`). It takes over the root rule that ADR 0008 held and gives it the evidence it rested on.

## Context

Every tool a job depends on keys something to the absolute path of the workspace, and none of them can be told otherwise cheaply:

- Claude Code stores the transcript under `~/.claude/projects/<slug>/`, where the slug is the workspace path, and writes `cwd` on every record and absolute paths into tool inputs, outputs and thinking. Thinking blocks carry a signature; a modified block is rejected with a 400 on the next tool turn. Since 2.1.300 the transcript also records the first request's system prompt with the working directory in it and reuses it until compaction.
- Codex writes `cwd`, `writable_roots` and `runtime_workspace_roots` into the rollout and keys trust to the exact path in `config.toml`.
- pnpm writes the store path as an absolute string into `node_modules/.modules.yaml` and compares it for equality on the next install; pnpm 11 bin shims bake an absolute `NODE_PATH`. Cargo's dep-info is absolute by default and its fingerprints rest on paths and mtimes; moving `CARGO_HOME` rebuilds everything. Every sampled Python venv has an absolute shebang and an absolute `home`.
- Six of the eight secondary harnesses key their session store by a hash or encoding of the working directory.

The preflight of 2026-10-19 measured the two gate machines: both have user `efebarandurmaz` and `HOME=/Users/efebarandurmaz`. Everything else differs (ADR 0008, Context), but the one string that decides this ADR is equal.

## Decision

### Data

```text
SnapshotManifest.root : AbsPath      the directory the job was captured in (ADR 0004)
target root            = SnapshotManifest.root       on every node, in this phase
```

There is no per-node root override in this phase. ADR 0008's earlier override for non-Claude adapters is withdrawn; nothing uses it before a second home layout exists.

### The three options and the choice

| Option | Meaning | What it buys | What it costs |
|---|---|---|---|
| P1, same absolute path everywhere | The root, `CARGO_HOME`, the pnpm store, the Python install and the harness state directories resolve to identical strings on every node | Claude's slug, `cwd` fields and signed thinking stay valid; pnpm's store check passes; Cargo fingerprints and venv shebangs stay valid; zero rewrite code | The same username and home on every node; no Linux node (`/home/x` against `/Users/x`); no root under `/tmp` or a synced folder |
| P2, different paths, rewrite per tool | Each tool's relocation feature, or a Fabric rewrite, per ecosystem and per harness | Works across usernames and operating systems | One rewrite per tool: pnpm 12.6+ relocation still requires the same store path string; Cargo has none; uv `--relocatable` covers new venvs only; Claude needs a rewrite outside signed blocks plus the recorded system prompt; every rewrite is a place to lose a user's work |
| P3, a mount namespace or VM per job | The path is always `/work` inside | Path identity without equal usernames | Heavy on macOS; changes the environment the harness sees; not what the user runs today |

P1 is the decision for this phase. It is the only option with zero rewrite code, the preflight shows it is feasible on the gate, and every rewrite P2 needs is a documented place where a tool breaks (ADR 0006 names the Claude ones). P3 is rejected for macOS.

### Rules

1. The target root is `SnapshotManifest.root`, always, and the resume output prints it with the reason.
2. A root made by `fabric job init --new` is `~/fabric/jobs/<job-id>/work`. A root adopted by `fabric job init` keeps its path. Both are the same string on every node with the same home.
3. `fabric doctor` compares `id -un` and `$HOME` and refuses a move when they differ (ADR 0008). `--force` does not override this row, because the result would be a root that does not exist on the target and a Claude session keyed to a path the target never computes; the Claude adapter refuses such a restore with `RootMismatch { from, to }` before anything is written (ADR 0006).
4. A root is never under a synced folder, `~/Desktop` or `~/Documents` on a node where those sync, `~/Library/Mobile Documents`, or `/tmp` (ADR 0004, Where a root may live).
5. A root is never a symlink: Claude resolves the physical path for its slug, so a link gives a different key.

### P2, when a second home layout arrives

The research names the fallback for the day a node has a different home: a per-node `FABRIC_HOME/jobs/<job-id>/work` root plus one rewrite per tool, applied only when `HOME` differs. Its pieces and their status today:

- Claude: rewrite the project directory key, `cwd` fields and sidecar references at path-component boundaries, never a signed block, and rewrite the recorded system prompt or pass `--system-prompt-snapshot off` on the first resume (ADR 0006).
- Codex: pass the new `cwd` on `codex exec resume`; the rollout needs no rewrite (H2, experiment E2).
- pnpm: carry needs pnpm 12.6+ on both nodes and the same store path string; otherwise install. The owner's own relocation work in pnpm 12 (PRs #15041 and #15112) is what makes the moved-tree case possible at all.
- Cargo: no relocation exists; rebuild.
- Python: `uv sync --frozen` from a warm cache; venvs do not move.

None of it is built in this phase. It becomes a decision when a Linux node or a second user is paired.

## Alternatives considered

1. **A neutral root outside home, such as `/fabric/jobs/<id>` or `/opt/fabric/jobs/<id>`.** Removes the username precondition and matches cmux's "same path or bind mount" rule. On macOS `/` is read-only and the path needs `/etc/synthetic.conf` plus a reboot; `/opt` needs ownership changes. Deferred to the P2 decision; it is the shape to prefer there.
2. **Per-job Claude config directory with `CLAUDE_CODE_PROJECT_DIR_NAME`** (H1, strategy B). Makes the slug independent of the path at the cost of a login per config directory, which the credential rule forbids (ADR 0006). Rejected.
3. **Rewrite on import, claude-handoff style** (H1, strategy C). Needed under P2 and written down there; under P1 it is code that can only lose work. Not in this phase.

## Consequences

- The first gate needs no rewrite code, no path templating and no per-node root override.
- Two nodes with different usernames cannot share a job in this phase, and `fabric doctor` says so before any byte moves.
- A job rooted at an adopted checkout, such as `~/oss/x`, lands at `~/oss/x` on the target; an existing clone there is `WorkspaceOccupied` (ADR 0004), and the message says to move it aside or adopt it.
- One unit test per rule above, with synthetic facts; one restore test asserting that a manifest whose `root` differs from the node's computed target root is refused before any write.

## Open questions

- Should `~/fabric/jobs/<job-id>/` also hold a human-readable `job.toml` naming the goal and the owner, so that Finder shows what the directory is? It would be derived from the ledger and never read back; decide when the CLI is written.
- Whether the neutral root (`/opt/fabric/jobs/<id>`) should become the default for `--new` before P2 is needed, so that the two layouts never diverge. Measure the setup cost on the Air first.
