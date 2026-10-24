# ADR 0012: Dependency carry

Status: accepted, 2026-10-19

Written 2026-10-19 after step 1 research (`2026-10-19-step1-design-input.md`, sections 4.4, 4.5 and 8.0, D6, D7 and D23, assumptions A10 to A12 and A27; `step1/H5-workspace-mobility.md`, sections 1, 5 and 6; `step1/preflight-2026-10-19.md`).

Amended 2026-10-20 after the review of PR `feat/env`: a cache the user declares in `.fabric/env.toml` gets a carry key by rule, not by the store's guess. The row for build outputs says they take the key of the ecosystem that produced them, so `[[cache]]` takes an optional `ecosystem` (`node`, `rust`, `python` or `other`). Without it, the declared cache takes the one ecosystem detection found; with none found it is `other`; with two found detection stops and asks for the key. `node` needs a detected package manager, because the `Node` key names one. `other` is carried when the platform matches (the `Other` row below): the user declared it and nothing measures its toolchain. Corrected on 2026-10-20 after the third review of PR `feat/env`: this note said a wrong carry of a declared cache costs a rebuild, never lost work, because a declared cache is never a precious path. Nothing enforces that. A typo such as `web` for `web/node_modules` takes the `Node` key, is not carried between the gate Macs (Node 22 and Node 26), and every ignored file under `web` that no setup step recreates, such as local data or notes, is dropped on the move. Detection cannot tell, because it runs no git. Capture can, and must: it refuses a declared cache that holds a tracked file or an untracked file git does not ignore, and it carries the ignored files under a declared cache as precious unless a declared setup step fills that path. Until the capture PR lands this rule, a declared cache is only as safe as its declaration.

Amended 2026-10-21 after the first move between the two Macs (gate findings F6 and F7, `2026-10-20-gate-findings.md`). The job held a `node_modules` made by hand with no lockfile and a `dist/` beside `package.json`. Detection said the tree travels as the user's files; the carry rule dropped it, because a cache detection did not know got `CarryKey::Unmeasured`, which never matches, and the Node majors differed anyway (22 against 26). Nothing on the Air could install it again, so it was lost, and `dist/` went with it although both Macs are macos-aarch64. Two parts of Fabric decided one path two ways. Now one function decides, and the rule reads what can make a path again, not only whether its key matches:

- The capturing node records, per cache row, what makes the path again, `Remake`: the setup steps that fill it from a lockfile (`Setup`), the next build or tool run (`NextBuild`), or nothing (`Nothing`). A dependency tree detection found no lockfile for, and a declared cache no step fills, is `Nothing`; this is the capture rule the 2026-10-20 note above asked for. So is a tree whose steps the target could not learn: when a file its steps read (`Detection::input_files`: its lockfile, `package.json`, its manager's configuration) or `.fabric/env.toml` stays on the source under `.fabricignore` or a secret pattern, the target's detection plans no step for the path, so that row is recorded as `Nothing` (review of PR #11; per cache since issue #24, below). The row also records how many native addons (`*.node`) lie under the path, and the manifest lists every path that stayed behind, secret or excluded, by name (ADR 0004, 2026-10-21).
- A `node_modules` detection does not know keys on the Node addon ABI (`Other { tool: "node.abi" }`), a `.venv` on its interpreter, and any other cache it does not know on the platform (`Other { tool: "platform" }`).
- The decision is a typed value per path: `Carry`, `Rebuild { by }`, `Refuse { reason }` or `LeaveBehind { reason }`, where a `LeaveBehind` reason is `Secret`, `Excluded`, `NothingCaptured` or `OtherPlatform`. `fabricd::carry::decide` computes it from the row and the target's facts. `fabric doctor --peer <node> --job <id>` (on the latest snapshot), `fabric job move` (on the snapshot it takes, with the target's facts from the preflight) and `fabric job resume` (with its own facts) call the same function and print one line per path.
- `Nothing`: always carried, to any os and arch. Native addons built for another ABI or platform add a warning, and so does another os or arch for anything but a tree of plain JavaScript, such as a venv, whose interpreter link and compiled modules are the source's; nothing else could make the path, so leaving it out would lose it.
- `Setup`: carried when the keys match and the secret policy left no hole. Else rebuilt when the target can run the same steps: the program is installed and, for a Node tree, it is the package manager that wrote the lockfile at the same major. Else carried when the os and arch match, nothing inside is native, nothing under it stayed behind (a restore drops a `Partial` tree's install record, so resume would run the steps over it; issue #24) and the row says a finished install vouched for the bytes at capture (`installed`: the cache held the sentinel materialize writes for the inputs it had). Else refused. Without that sentinel, as for a tree installed by hand, materialize on the target runs the steps over the carried bytes anyway, so a target that cannot run them could not resume the job (review of PR #11).
- `NextBuild`: carried when the keys match. Left behind on another os or arch, with the reason printed. Rebuilt by the next build when the target has the toolchain at another version, as `target/` under another rustc. Carried on the same os and arch when the target lacks the toolchain. Build output and tool caches detection does not know (`dist`, `build`, `out`, `.next`, `.turbo`, `.cache`, the Python caches) key on the platform, so between the two Macs they are carried (F7): they are cheap in a content-addressed store, and the next build replaces what is stale. An ignored path in no class is precious and always carried (ADR 0004); a secret never is.
- A refusal stops `fabric job move` after its suspend and before anything changes hands, and makes `fabric doctor --job` fail; both name the path and the reason. A pull, `fabric job resume` on the target, decides with the puller's facts once it holds the snapshot and aborts the move before `MoveImported`, so the job stays with the holder (review of PR #11). `--force` carries the path as it is. A restore that meets a refusal, after `--force` or on the node that already owns the job, writes the bytes too, since nothing there can make them again.

This changes four lines of the text below. "A mismatch never fails a move" now holds for every path something on the target can rebuild or use; a path it can do neither with refuses. The Node row's "Otherwise" applies only to a tree with a lockfile whose package manager runs on the target at the same major, so on the gate Macs as measured, npm 10 against npm 11, the fixture's pure npm tree travels as it is and the install path needs npm 10 on the Air. "A suspend with no named target carries everything" still holds for the bytes, which every snapshot holds whole; the decision is not frozen into the snapshot, because one snapshot is restored on more than one node (a pull back, a steal) and a decision made for one target would be wrong on another. What the snapshot freezes is everything the decision reads from the source. The pnpm store row's "Never" is about the machine-wide store; a `.pnpm-store` inside the root has no lockfile step and is carried as the user's files.

Amended 2026-10-24 after three findings Codex left on PR #11 (issue #24). Each let the decision promise what materialize on the target could not keep, so the job would move and then fail to resume:

- A cargo step was judged by `rustc`. rustup installs the two as separate components, so doctor measures `cargo --version` as `cargo.version`, and a cargo step needs it.
- A file detection reads that stayed behind forgot every rebuild of the snapshot. Detection now names, per cache, the files its digest covers (`Detection::input_files`), and a capture forgets the rebuild of a cache only when one of those files, or `.fabric/env.toml`, stayed behind: a `requirements.txt` left out turns `.venv` into the user's files and leaves `node_modules` to its install. A `Partial` tree also never arrives with the sentinel materialize writes after a finished install. Where the cache's own inputs travelled, as when only `.fabric/env.toml` stayed behind, or after `--force`, the target's detection trusted that sentinel and never filled the holes. The restore that writes a `Partial` tree now removes it before the marker calls the tree clean, so materialize runs the steps that fill it. A `Partial` tree the target cannot rebuild is refused because resume there would run those steps first; `--force` carries it, and the tool must be installed there before the job resumes.
- A step that runs on every resume was not checked when the keys matched: one declared `when = "always"`, detection's `go mod download`, and every step of a spec that names no cache. `fabric_env::detect::every_resume` names them from the snapshot's spec, and a move adds a preflight row for each program of those steps that nodes measure, refused when the target lacks it and waived by `--force`: a push after its suspend and before `MoveStarted`, a pull before its import, as for the carry decision. `fabric doctor --job` shows the rows for the latest snapshot. A program no node measures, `go` among them, gets no row; resume names it when it is missing (ADR 0007).

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
| A declared cache with `ecosystem = "other"`, or declared where no ecosystem was detected | Same OS and arch | Dropped; whatever made it makes it again | Nothing measures its toolchain; the user declared it a cache, so dropping or carrying it costs time, not work |
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

- The 2026-10-21 rule rebuilds a Node tree only under the package manager major that wrote its lockfile, which is right for pnpm, whose store layout moves per major, and stricter than npm needs: `npm ci` under npm 11 reads an npm 10 lockfile. Relax it per manager once a gate run measures it, or keep it, since carrying a pure tree costs nothing on the same platform.
- The comparison doctor prints is still not written into the job record next to the move (ADR 0016). The decision lines would be the record to keep.

- Relax the Node rule from "same major" to "same ABI, or every carried `.node` file exports `napi_register_module`"? The scan is cheap if done once at capture and stored in the key. Decide after the gate, with the fixture given one non-N-API addon.
- Whether to carry `target/` without `incremental/` by default, or only above a size the user sets. Measure on meridian.
- Whether hard links inside a pnpm `node_modules` (link count above 1) should be preserved on restore. The measured tree is APFS-cloned with link count 1, so nothing is lost today; revisit when a tree with real hard links is measured.
