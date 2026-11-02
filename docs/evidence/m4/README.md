# M4 evidence: the seam between Fabric and Capsule

Recorded on 2026-11-02 on `m4/reconciliation`, after Efe's decisions of 2026-10-29 (evening): the provider operation read surface is M5 scope (`f182c93`, ADR 0022 selection 6 and the M5 acceptance row in docs/managed-jobs.md), and Capsule's worker package drops its declaration maps and keeps the 41 KiB pack budget (Capsule `5c6a84a`, `ee189d4`). The host is an Apple M5 Pro, macOS arm64, with other work running at a load average of 7 to 13. Nothing here was timed: no benchmark, A/B run or stress loop ran, and the time budget ran in count mode. The Perf phase measures these commits on its own against the fixed anchor.

## What was checked and what changed

| Item | Result |
|---|---|
| Fixtures | `protocol/bridge/v1` here and in Capsule `m4/reconciliation` are byte-identical (`diff -r` is empty), and both pins name manifest `f96887e7224320b1e250109202e8920b283b1657b362c4822054fe9c018a7010` (`crates/fabric-client/tests/bridge_fixtures.rs` here, `scripts/bridge-fixtures.test.mjs` in Capsule). Nothing in them changed |
| Worker | Capsule `ee189d4`, built in its worktree with `pnpm --filter "@capsule-sdk/bridge-worker..." run build` on Node 22.23.2 (`dist/` holds no `.d.ts.map` any more) and as the single executable with `pnpm bridge:sea` on Node 26.7.0 (143820864 bytes, sha256 `dce5a9d4f2be567f1e1cfb25dc62927f74f37263a34dbf4d3df3ebb97e478551`). The package packs to 37773 bytes, under the 41 KiB budget its pack smoke checks |
| Tests | `455e731` adds three interop tests beside the lost create: a destroy lost with its worker (fixture profile and Docker), and a create whose row was committed and whose frame never left |
| Specification | `a502423`: the status lines of docs/managed-jobs.md and docs/bridge-protocol.md no longer say the worker's attach is pending; the M4 acceptance table names the new tests |
| CI pin | `f480682` moves both `bridge-interop` and `bridge-docker` to Capsule `ee189d4`, the worker head every run below used. It is not pushed yet: Capsule's branch must reach its origin before those jobs can check it out. Capsule's one commit after it (`1e25cd2`) is evidence only |

## The M4 acceptance cases on the real worker

Each case Efe asked for, and the test that runs it against the Capsule worker (`crates/fabric-bridge/tests/capsule_worker.rs`). "Fixture" is the worker's fixture profile; "Docker" is its Docker profile on the local engine with `debian:bookworm-slim`, an image it already held.

| Case | Fixture | Docker | What is checked |
|---|---|---|---|
| The worker killed mid-create | `a_create_lost_with_its_worker_is_attached_and_never_made_twice` | `a_docker_create_lost_with_its_worker_is_attached_and_never_made_twice` | the create is `unknown` with no locator after recovery; the same create is never sent again and a second create of the job waits; attach by the create's own identity finds the one resource (Docker: one container with the operation's label, `containers(own) == 1`); it is settled `succeeded` with the locator |
| The worker killed mid-destroy | `a_destroy_lost_with_its_worker_blocks_a_replacement_until_a_stop_is_confirmed` | `a_docker_destroy_lost_with_its_worker_blocks_a_replacement_until_no_container_is_left` | the destroy is `unknown` and still names its target; the destroy is never sent again; an observe by that target that finds the resource records `observed`; the barrier holds through `unknown`, `observed` and `abandoned` |
| The authority stopped between the intent and the frame | `a_create_whose_frame_never_left_stays_unknown_after_an_absent_attach_until_abandoned` | (the fixture provider lives in the worker, so this is where the scope searched is exact) | the row is `dispatched` with nothing sent; recovery makes it `unknown`; the worker answers the attach `unknown`/`absent` with `searched` = the fixture provider, the bound scope and the kind; the status stays `unknown`, `searched` is stored, a second attach reads the same; the operation is a residual with that search and a `CreateUnresolved` blocker; abandoned acknowledging the search it blocks nothing, stays a residual, and the replacement attempt creates once |
| The authority stopped after the frame | both lost-create tests | the Docker lost create | Fabric drops the in-flight request after the frame was written, kills the worker, reopens the journal and runs recovery, which sends nothing; then the attach above |
| Exactly one container, and a replacement refused while a destroy is unknown | (one sandbox in the fixture provider) | `a_docker_destroy_lost_with_its_worker_blocks_a_replacement_until_no_container_is_left` | after the kill the engine holds one container of the job (`interop/*docker.log`: "after the kill the engine holds 1 container(s) of the job", in both runs); the replacement is refused with `ReplacementBlocked` naming the destroy while it is `unknown`, again when it is `observed` and again when it is `abandoned`; no second container appears; the barrier lifts only once `containers() == 0` |
| Cleanup needs a fresh approval | the lost create (a cleanup under the create grant is `denied`/`approval_scope`, nothing destroyed; under its own destroy grant it removes the residual) and the lost destroy (the same refusal, then the fresh grant) | the lost create and the lost destroy (`apr-destroy`, a grant for `sandbox.destroy` on any resource of the scope) | the refusal reaches Fabric as `rejected` with no effect; while the lost destroy is in flight a cleanup is refused by the journal before anything is written (`CleanupRefusal::InFlight`); only the destroy confirmed gone empties `residuals` and `replacement_blockers` |

The Docker lost-destroy test handles both outcomes of the race between Fabric's kill and the worker's detached `docker rm` client: the container gone (the observe answers `completed`, the destroy is confirmed) or still there. Both runs here took the second path, the one that needs the abandonment and the fresh destroy; the first path was not seen in this run.

## Interop, both ways

Fabric `455e731` (this branch has changed only CI and documents since) against the Capsule worker. The test binary was built first and run directly with `env -i`, so each run's PATH is exactly what the table says. `CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim` names an image OrbStack (Docker 29.4.0) already held; nothing was pulled. `interop/run-interop.sh` is the script.

| Log | Worker | PATH | Docker image | Result |
|---|---|---|---|---|
| `interop/node-docker.log` | `dist/bin.js` built in Capsule's worktree at `ee189d4`, through Node 22.23.2 | Node 22, a `docker` link, `/usr/bin`, `/bin` | set | 13 passed, 0 failed |
| `interop/node-no-image.log` | the same | the same | unset, as on the `bridge-interop` runner | 13 passed, 0 failed (the Docker tests report their skip) |
| `interop/sea-docker.log` | Capsule's single executable, copied to a fresh directory | a `docker` link, `/usr/bin`, `/bin`: none of `node`, `nodejs`, `bun`, `pnpm`, `npm`, `npx` or `deno` reachable, which the script checks before it runs | set | 13 passed, 0 failed |
| `interop/sea-no-image.log` | the same | the same | unset | 13 passed, 0 failed |

No container with a `fabric.job` label was left after any run (`interop/summary.txt`). `interop/heads.txt` records both commits, that both worktrees were clean, and the executable's digest.

Capsule's own proofs of the same worker are in Capsule `benchmarks/after/bridge/m4-reconcile`: its gates on Node 22, the executable's artifact smoke on Node 26 (all 24 shared scenarios, the 8 refused frames, SIGTERM and SIGINT, with an empty PATH), and its Docker smoke on `debian:bookworm-slim` (all 13 checks, the two attach checks among them; its timings left out).

## Fails first

`interop/m3-worker-fails-first.log` runs the three new tests against the M3 worker (Capsule `91d4e0d` built in `/tmp/capsule-wt/m3`, the worker before attach). `a_create_whose_frame_never_left_stays_unknown_after_an_absent_attach_until_abandoned` fails there: the M3 worker answers an attach `protocol_error`, not `absent`. The two lost-destroy tests pass against that worker, because what they pin is Fabric's own rules (the create guard, the barrier, the cleanup refusal in flight, the abandonment bound to the status seen) and the worker's destroy and observe, which M3 had; those rules were each shown red by removal in `../m4-reconcile` (7 journal tests failed with the guard, the barrier and the draft check removed). No rule was removed again in this run.

## Gates

The whole gate list ran once on `f480682` (`gates/run-gates.sh`, `gates/summary.txt` with each exit code and the load average, at 10 to 20 while Capsule's gates ran beside it), nothing timed: `cargo fmt --all --check`; clippy with `-D warnings` for the workspace and for `fabric-harness` with no features and each of `acp`, `claude`, `codex`; `cargo test --workspace --locked -- --skip e2e_` (1222 passed, 0 failed, 26 ignored); `cargo test -p fabric --test e2e --locked` (70 passed, 5 ignored); `FABRIC_BUDGET=count` `budget::hot_paths`; `cargo doc` with `-D warnings`; `cargo deny check`; the hygiene, fixture and budget scripts and their self-tests against `origin/m3/capsule-bridge`; actionlint on both workflows; the TypeScript SDK install, typecheck, build and `bun test` against the node built here ( 62 pass). All passed on the first run. The evidence commit after `f480682` changes documents only; `scripts/check-hygiene.sh origin/m3/capsule-bridge` was run again on it.
