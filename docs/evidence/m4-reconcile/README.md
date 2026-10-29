# M4 evidence: reconciliation and cleanup in Fabric

Recorded on 2026-10-29 on `m4/reconciliation`, on the M4 specification commits (ADR 0022, `docs/managed-jobs.md` and `docs/bridge-protocol.md`, the v1 attach fixtures pinned at manifest `f96887e7`). The host is an Apple M5 Pro, macOS arm64, with other work running at a load average of 4 to 16. Nothing here was timed: no benchmark, A/B run or stress loop ran, and the time budget ran in count mode. The Perf phase measures these commits on its own against the fixed anchor.

## What M4 adds in Fabric

| Item | Where | Pinned by |
|---|---|---|
| The intent row completed: profile, effect class, purpose, target and locator keys; one committed write per dispatch | `fabric-journal` `provider.rs`, schema version 2 amended in place | `provider::tests`, `tests/fixtures.rs` with `v2.sqlite` regenerated |
| `attached`, `observed`, `abandoned`; abandonment bound to the status and what the owner saw | `provider_record`, `provider_searched`, `provider_abandon` | `statuses_move_only_along_the_machine` (every pair), `an_abandonment_needs_the_state_the_owner_saw`, `recovery_leaves_attached_observed_and_abandoned_as_they_are` |
| No create while a create of the job is unknown | `provider_begin` | `a_create_waits_while_a_create_of_the_job_is_unknown`, `recovery_makes_a_dispatched_create_unknown_and_it_is_attached_not_resent` |
| Replacement barrier, `ReplacementBlocked`, `replacement_blocked` in `managed_refused` | `provider::accept` in `Journal::submit`, fabric-core, fabricd | `a_failed_or_unknown_destroy_blocks_a_replacement_until_a_destroy_succeeds`, `an_unsettled_exec_blocks_a_replacement_until_its_sandbox_is_destroyed`, `abandoning_a_destroy_or_an_exec_never_unblocks_a_replacement`, `an_attached_or_abandoned_create_does_not_block` |
| Cleanup and residuals | `provider_begin` (purpose cleanup), `residuals`, `Bridge::cleanup` | `a_cleanup_runs_after_the_job_is_cancelled`, `a_cleanup_needs_a_named_resource_of_this_job`, `residuals_list_what_no_cleanup_removed`, `tests/process.rs` `a_cleanup_without_a_named_resource_is_refused_before_anything_is_written` |
| `attach`, `absent` with `searched`, observe of `attached` and `observed` | `fabric-bridge` `wire.rs`, `client.rs` | golden (58 cases, 24 scenarios), the scripted run of all 24 scenarios with `journal_state` seeded, `tests/process.rs` `a_create_lost_with_its_worker_is_attached_and_never_made_twice` and `an_attach_is_a_read_refused_where_the_profile_or_the_journal_says` |
| Locator compared by key | `wire::resource_key`, `wire::locator_record` | `a_locator_record_is_its_canonical_text_and_the_key_of_its_identity` (pins the key bytes the journal fixture holds) |
| A draft version 2 journal refused at open | `Journal::open`, `OpenError::DraftSchema` | `a_draft_version_2_journal_is_refused_at_open` |

## Fails first

The M4 journal and bridge tests use types the M3 code does not have, so none of them builds on M3. Three rules were also removed one at a time from the M4 code (the create guard, the barrier, the draft schema check): 7 journal tests failed, `a_create_waits_while_a_create_of_the_job_is_unknown`, the four barrier tests, `recovery_makes_a_dispatched_create_unknown_and_it_is_attached_not_resent` and `a_draft_version_2_journal_is_refused_at_open`, and all passed again once the rules were back.

## Counters

`tests/off_runtime.rs` is unchanged and passes: a dispatch is still one log call and one committed write, a settle one read and one write, each on the log thread. The create guard is one indexed read inside the dispatch's own transaction, and only for a create. Journal reads and writes on the provider path now use cached statements.

## Interop

Against the M3 Capsule worker (`a4d597a`, the CI pin, built from a clean clone with the CI job's steps, offline): 7 of the 8 M3 interop tests pass, the Docker one on `debian:bookworm-slim`, already held by the engine, with no container left behind. `every_shared_scenario_over_the_workers_stdio` stops at `attach_absent_stays_unknown`: the M3 worker answers an attach `protocol_error`/`unexpected`. The two new interop tests (`a_create_lost_with_its_worker_is_attached_and_never_made_twice` on the fixture profile and `a_docker_create_lost_with_its_worker_is_attached_and_never_made_twice`) need the worker's attach. These three are expected red until Capsule `m4/reconciliation` lands its worker side; the CI pin moves then.

## Gates

On `eff52af`, once each:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `-p fabric-harness` with no features and each of `acp`, `claude`, `codex` | pass |
| `cargo test --workspace --locked -- --skip e2e_` | pass, after one rerun of `fabricd --lib`: `local::tests::a_stop_reaches_the_group_after_its_leader_exited` failed once at a load average of 16 (a background `sleep` not yet reaped after its group was killed; `crates/fabricd/src/local.rs` is not touched here) and passed on the rerun, 253 passed |
| `cargo test -p fabric --test e2e --locked` | pass: 69 passed, 5 ignored |
| `FABRIC_BUDGET=count` `budget::hot_paths` | pass (process counts only, nothing timed) |
| `cargo doc --workspace --no-deps --locked` with `-D warnings` | pass |
| `cargo deny check` | pass |
| `scripts/test-check-hygiene.sh`, `scripts/check-hygiene.sh` on the M3 head | pass |
| `scripts/test-check-fixtures.sh`, `scripts/check-fixtures.sh` on the M3 head | pass |
| `scripts/test-check-budget.sh`, `scripts/check-budget.sh` on the M3 head | pass |
| `fabric-client` `bridge_fixtures` (manifest pin) | pass |
| TypeScript SDK: `bun install --frozen-lockfile`, `typecheck`, `build`, `bun test` against the node built here | pass: 62 passed |

## After the merge of `m3/capsule-bridge` at `52b29a8`

`50d71b1` merges `m3/capsule-bridge` at `52b29a8`, which holds `m2/durable-job-foundations` at `3da4b4d`: the PR #31 review fixes, the power-loss recovery tests, the fake harness flake fix and the ADR approval dates. Git found no conflict. The only file both sides changed is `docs/managed-jobs.md`, in different rows. The fixes change `fabric-harness`, `fabric-net`, `fabric-store`, `fabricd` (`job.rs`, `ops.rs`, `catchup.rs`, `paths.rs`), the e2e budget and the CI scripts. M4 changes `fabric-journal`, `fabric-bridge`, `fabric-core` and one line of `fabricd/src/managed.rs`, so no fix lands on code M4 changed, and every test the fixes added runs on the merged code below.

The whole gate list ran once on `50d71b1`, at a load average of 10 to 12, nothing timed: `cargo fmt --all --check`; clippy with `-D warnings` for the workspace and for `fabric-harness` with no features and each of `acp`, `claude`, `codex`; `cargo test --workspace --locked -- --skip e2e_` (1218 passed, 23 ignored); `cargo test -p fabric --test e2e --locked` (70 passed, 5 ignored); `FABRIC_BUDGET=count` `budget::hot_paths`; `cargo doc` with `-D warnings`; `cargo deny check`; the hygiene, fixture and budget scripts and their self-tests against `origin/m3/capsule-bridge`; the TypeScript SDK install, typecheck, build and `bun test` against the node built here (62 passed). All passed on the first run.

## After the merge of `m3/capsule-bridge` at `b302357`

`9afbd26` merges `m3/capsule-bridge` at `b302357`, which holds `m2/durable-job-foundations` at `929fba1` (the repair fixture shows the kept ledger by fork point and head) and four bridge fixes made on M3: new work after a shutdown is refused as shutting down even once the worker exited, the test double runs once before any timed handshake, a log thread call is counted before its caller hears the result, and a request admitted before a shutdown is never written after it.

Git stopped on three files. `tests/off_runtime.rs` and `tests/process.rs` conflicted in their imports only: M4's imports stay and the shared `common::double()` replaces the local copy, as on M3. In `docs/bridge-protocol.md` the observe paragraph keeps M4's attach sentence and takes M3's shutdown sentences after it, naming attach. Two changes had no textual conflict but were needed on M4's code: `Bridge::attach` now queues its frame through the new `enlist`, under the lock `shutdown` sets its flag under, as observe does; and the `Gated` log of M3's new stream test implements M4's `OperationLog` (`record` and `locate` take a `LocatorRecord`, plus `searched` and `cleanup_target`). M4's cleanup goes through `send_request`, so the shutdown fix covers it, and the journal already allows `dispatched` to `rejected`. The M2 fix changes `fabricd/src/paths.rs`, ADR 0003 and a reply fixture, none of which M4 changed.

`eada7d0` adds to the shutdown test an attach of a create the shutdown left unknown, after the worker exited: it is refused as shutting down. With attach left out of the shutting-down check it failed 2 of 2 runs with `Lost("the worker closed its output")`.

The whole gate list ran once on `eada7d0`, at a load average of 5 to 12, nothing timed: `cargo fmt --all --check`; clippy with `-D warnings` for the workspace and for `fabric-harness` with no features and each of `acp`, `claude`, `codex`; `cargo test --workspace --locked -- --skip e2e_` (1222 passed, 23 ignored); `cargo test -p fabric --test e2e --locked` (70 passed, 5 ignored); `FABRIC_BUDGET=count` `budget::hot_paths`; `cargo doc` with `-D warnings`; `cargo deny check`; the hygiene, fixture and budget scripts and their self-tests against `origin/m3/capsule-bridge`; the TypeScript SDK install, typecheck, build and `bun test` against the node built here (62 passed). All passed on the first run.
