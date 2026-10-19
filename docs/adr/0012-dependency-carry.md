# ADR 0012: Dependency carry

Status: accepted, 2026-10-19

Written 2026-10-19 after step 1 research (`2026-10-19-step1-design-input.md`, sections 4.4, 4.5 and 8.0, D6, D7 and D23, assumptions A10 to A12 and A27; `step1/H5-workspace-mobility.md`, sections 1, 5 and 6; `step1/preflight-2026-10-19.md`).

## Context

Dependencies and build outputs dominate the bytes of a move and their freshness is bound to strings outside the files. The research measured the sizes and found the couplings:

- pnpm compares the store path as an exact string (`modules.store_dir == config.store_dir`) and fails `pnpm install` with `ERR_PNPM_UNEXPECTED_STORE` when the store version embedded in that path differs; the Pro holds `v3`, `v10` and `v11` stores side by side. pnpm 11 bin shims bake an absolute `NODE_PATH`. Three strings must match for a pnpm tree to carry: the username, the store path and the pnpm major.
- Native Node addons are built per ABI unless they are N-API; both addons in the measured tree are N-API and one is a fat binary. npm trees carry no absolute paths and are more portable than pnpm 11 trees.
- Cargo decides fresh or stale from mtimes and absolute paths, and its fingerprints include the `rustc -vV` output; a carried `target/` is useful only with the same rustc, the same `CARGO_HOME` path and the original mtimes. Meridian's `target/debug` is 23 GB, 6.8 GB compressed, 70 seconds to compress.
- Every sampled venv has an absolute shebang and an absolute `home`.
- A moved lockstep root `node_modules` is 95 MB, 21 MB compressed, 2.2 seconds to unpack. A cold `pnpm install` is minutes.

The preflight shows the gate machines agree on none of the version strings: pnpm 11.23.0 against not installed, Node v22.23.2 (ABI 127) against v26.0.0 (ABI 147), rustc 1.97.1 against 1.96.0. The first real job will hit the fallback path before it hits the carry path.

## Decision

### Data

```text
CarryKey =
  | Node   { node_major: String, package_manager: String, pm_major: String, pnpm_store_path: Option<String> }
  | Rust   { rustc_vv: String, cargo_home: AbsPath }
  | Python { python_version: String, python_home: AbsPath }
  | Other  { tool: String, version: String }
```

Each `CacheEntry` in a snapshot (ADR 0004) carries the `CarryKey` the source measured when it captured. The strings are the tools' own output, kept whole. The target measures the same key for its own node and compares for equality field by field. Equality means the bytes are usable; anything else means the entry is not carried, or, if it arrived, is treated as missing and rebuilt (ADR 0007). The job record keeps every measured key next to the move it was measured for, so the answer to "why did this rebuild" is in the ledger.

### Rules per ecosystem

| Ecosystem | Carried when | Otherwise | Why that rule |
|---|---|---|---|
| Node, `node_modules` | `node_major`, `package_manager` and `pm_major` equal on both nodes, and, for pnpm, `pnpm_store_path` equal as a string | Not carried; the `EnvSpec` runs the lockfile install (`pnpm install --frozen-lockfile`, or the npm, yarn, bun equivalent) | pnpm's exact-string store check and the absolute shims; a different major pnpm fails the next install rather than reinstalling. The Node major is the conservative rule for addons (Open questions). |
| Node, pnpm store files | Never | The target's own store, warm or cold | The store is machine-wide and 18 GB on the Pro; carrying a slice of it is a later option (research D3). |
| Rust, `target/` | `rustc_vv` equal, whole output, and `cargo_home` equal | Not carried; `cargo build` rebuilds. `cargo fetch` runs if the registry cache is cold | Fingerprints include rustc and absolute paths; mtimes are restored by ADR 0004 for the carried case; clock offset above 5 s blocks a carry (ADR 0008) |
| Rust, `target/*/incremental` | Same as `target/`, and only when the carried size matters less than the first build time | Dropped from the entry; listed in the output | Incremental data was 7.8 of 23 GB in the measured tree and rustc rebuilds it |
| Python, `.venv` | `python_version` equal and `python_home` exists on the target at the same path | Not carried; `uv sync --frozen` or `pip install -r` from the lockfile | Absolute shebangs and `home` |
| Go | Nothing in the workspace is carried | `go mod download` from the per-node module cache | `GOMODCACHE` is outside the workspace and content-checked |
| Build outputs the `EnvSpec` names (`dist`, `.next`, `build`, `out`, `.turbo`) | Same key as the ecosystem that produced them | Dropped; the next build recreates them | Some outputs embed absolute paths or machine state; the measured cost of dropping them is a build, never lost work |
| Precious ignored files | Always (ADR 0004, Ignored files by class) | | They are not caches |

A mismatch never fails a move. It switches that entry to the install path and prints one line saying which key differed, with both values. The move is slower and still correct.

### What the carry decision touches

- At capture, the source measures the key for each entry, writes it into `CacheEntry.key`, and sets `carried` from the last doctor comparison with the target when the move is a push; a suspend with no named target carries everything, since the decision can be made at restore.
- At restore, the target compares every entry's key with its own measurement before writing a byte of it. An entry whose key matches and whose `partial` is false is written and counts as warm. Any other entry is skipped and reported, and materialize (ADR 0007) runs the install step for it.
- `fabric doctor` prints the comparison before the move so the user sees the install coming (ADR 0008).

### The first gate fixture

Given the measured mismatch, the gate fixture is a small npm-installed, pure-JavaScript project with no native addon and no pnpm. Its `CarryKey` is `Node { node_major: "22", package_manager: "npm", pm_major: "10", pnpm_store_path: None }` on the Pro, and the Node major rule means the Air rebuilds it today. The gate therefore proves both paths with one fixture: the carry path when the Air is given Node 22 through `mise` or `nvm`, and the install path as the machines stand. Both runs are in the gate's test matrix; a gate that only ever took the fallback would not have tested the feature.

## Alternatives considered

1. **Always rebuild on the target** (research D1, C3, Y1). Correct everywhere and zero Fabric code. Rejected as the primary path because a cold `pnpm install` is minutes and a chunk diff is seconds, and the carry is the one piece nobody in the landscape ships. Kept as the fallback for every mismatch.
2. **Carry only the pnpm store files the lockfile needs, then `pnpm install --offline`** (D3). Reuses pnpm's own integrity checks and helps even across operating systems for pure-JavaScript packages. Needs a lockfile-to-store-file listing that does not exist yet. A later option, measured by research E-4-3.
3. **pnpm 12 relocation when paths differ** (D4). Still needs the same store path string and pnpm 12.6+ on both nodes; the owner runs 11.23. Part of the P2 path in ADR 0011.
4. **Shared compile caches instead of carrying** (sccache, Turborepo remote cache, Nix). Need a server or an always-on node and cannot cache linker-invoking crates. Later, alongside the witness ADR.
5. **Pin the Node ABI instead of the Node major, or scan `.node` files for N-API and allow any major when all are N-API.** More permissive and probably right for the measured trees. Not taken now because the carry key must be computable without walking 100,000 files on every move; see Open questions.
6. **Cross-OS or cross-architecture carry.** Outside "same OS, same arch, same user" carrying saves little; the portable part is lockfiles plus warm per-node caches. Out of scope until same-platform carry is proven (ADR 0004).

## Consequences

- A move between matched nodes carries `node_modules` and `target/` and runs no install; a move between the gate machines as they stand runs the install and says why on one line per entry.
- The carry key is data in the snapshot, so a later relaxation of a rule is a comparison change, not a format change.
- One unit test per rule row, with synthetic keys: equal keys carry, each differing field alone switches to the fallback, and the output names the field. One integration test restores a snapshot whose `node_modules` key differs and asserts that the directory is not written and the install step is scheduled.

## Open questions

- Relax the Node rule from "same major" to "same ABI, or every carried `.node` file exports `napi_register_module`"? The scan is cheap if done once at capture and stored in the key. Decide after the gate, with the fixture given one non-N-API addon.
- Whether to carry `target/` without `incremental/` by default, or only above a size the user sets. Measure on meridian.
- Whether hard links inside a pnpm `node_modules` (link count above 1) should be preserved on restore. The measured tree is APFS-cloned with link count 1, so nothing is lost today; revisit when a tree with real hard links is measured.
