# M2 defect register

Revalidated on 2026-10-27 against `176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5` (origin/main, unchanged since the audit). Sources: `fabric-audit.md` section 6, `fabric-technical-annex.md` (every confirmed finding, static candidate and release boundary), `fabric-performance-evidence.md`, and GitHub issues #16, #17, #20, #21, #22 and #28. An ID from the audit is kept; a finding without one gets `M2-NN`.

Status is `confirmed` when the current source still has the cause at the anchor given, `invalid` when it does not. No finding was found invalid: the source has not changed since the audit. Two candidates the audit itself rejected are listed at the end with the evidence.

Scope says which campaign fixes it. Amended 2026-10-29: every finding is now M2 scope. The first version of this register handed some to a later campaign; on 2026-10-28 each of those was given to an M2 group, and each is fixed on `m2/durable-job-foundations` with a test that failed first ("Disposition after the integration" below). Their scope cells say `M2 (moved in 2026-10-28)`, and git history keeps the first wording. Each finding keeps the owner group whose files it touches.

An expected-failure test is `#[ignore = "<ID>: expected failure until fixed"]`. It states the intended behavior and fails on the current source; the owner removes the `ignore` in the commit that fixes it. Run them all with `cargo test --workspace --locked -- --ignored <name>`; each was run on this branch and failed at its intended assertion (see `expected-failures.md`).

## Owner groups

| Group | Owns (files no other group edits in M2) |
|---|---|
| A launch and ACP | `crates/fabric-harness/src/{process.rs,harness.rs,acp/mod.rs,acp/client.rs}`, the launch parts of `crates/fabric-harness/src/{claude,codex}/mod.rs`, `crates/fabric/src/{lib.rs,terminal.rs}`, `crates/fabric-harness/tests/{acp_roundtrip.rs,claude_roundtrip.rs,codex_roundtrip.rs,fake_acp_agent/mod.rs}`, `crates/fabric/tests/e2e/{acp_cli.rs,main.rs,headless.rs}`, `docs/adr/{0006,0015,0016}-*.md` |
| B preflight and facts | `crates/fabricd/src/{doctor.rs,facts.rs,service.rs,ops.rs,job.rs,catchup.rs}`, `crates/fabric-net/src/{wire.rs,rpc.rs}`, `crates/fabric-net/tests/loopback.rs`, `crates/fabric-core/src/version.rs`, `crates/fabric/tests/e2e/preflight.rs`, `docs/adr/{0008,0013}-*.md`, `docs/net.md` |
| C secret redaction | `crates/fabric-core/src/secret.rs`, `crates/fabric-harness/src/{redact.rs,jsontext.rs,jsontext/props.rs}`, `README.md` |
| D environment freshness | `crates/fabric-env/src/{detect.rs,materialize.rs}`, `crates/fabric-env/tests/freshness.rs`, `docs/adr/0007-environment-spec.md` |
| E capture and restore fidelity | `crates/fabric-core/src/snapshot.rs`, `crates/fabric-core/tests/manifest_codec.rs`, `crates/fabric-store/src/{policy.rs,capture.rs,restore.rs,writer.rs}`, `crates/fabric-store/tests/{workspace.rs,policy.rs}`, `crates/fabric-harness/src/{pipeline.rs,place.rs}`, `crates/fabricd/src/carry.rs`, `docs/adr/{0004,0014}-*.md` |
| G1 managed domain | `crates/fabric-core/src/managed/*`, `crates/fabric-core/tests/managed_*.rs` |
| G2 journal | `crates/fabric-journal/*` (new crate) |
| G3 authority wiring | `crates/fabricd/src/{authority.rs,control.rs,node.rs,transfer.rs}`, `crates/fabric-client/*`, `protocol/*`, `sdk/typescript/*`, `crates/fabric/src/managed.rs` |
| H test harness and CI | `crates/fabric/tests/e2e/world.rs`, `.github/workflows/ci.yml`, `crates/fabric/tests/e2e/budget.toml` |

Shared file rules: `crates/fabric-core/src/lib.rs` takes one module line from G1 only. Group E's manifest version bump is the only change to `MANIFEST_VERSION` in M2; if E needs a carry key in `facts.rs`, B makes that change. `crates/fabric/src/lib.rs` gets G3's one managed dispatch line after A's launch change lands.

## Register

| ID | Finding | Status, source anchor | Files to touch | Group | Scope | Expected-failure test |
|---|---|---|---|---|---|---|
| R2 (#17) | A headless or ACP harness starts before the node records it; a CLI killed by `SIGKILL` in between leaves an unrecorded, unfenced harness | confirmed: `crates/fabric-harness/src/process.rs:340` holds only `Wiring::Terminal` on a tty; `crates/fabric/src/lib.rs:688-705` awaits `adapter.launch` and registers after; `crates/fabric-harness/src/acp/mod.rs:418-449` handshakes and prompts inside `launch` | `process.rs`, `harness.rs`, `acp/mod.rs`, `claude/mod.rs`, `codex/mod.rs`, `fabric/src/lib.rs`, ADR 0006, 0015, 0016 | A | M2 (acceptance gate) | `codex_roundtrip::a_headless_launch_runs_nothing_until_it_is_released`, `acp_roundtrip::the_trait_launch_holds_the_agent_until_it_is_released` |
| M2-01 | Headless ACP through the CLI drops the answer, and a failed turn exits 0 | confirmed: `acp/mod.rs:434-447` detaches the turn and only logs its outcome; `fabric/src/lib.rs:666-670` reads output only for Claude and Codex; `lib.rs:1231` hides the missing-answer warning for ACP | `acp/mod.rs`, `harness.rs`, `fabric/src/lib.rs`, fake agent | A | M2 (same code as R2) | `e2e::acp_cli::e2e_a_headless_acp_run_prints_the_answer`, `e2e::acp_cli::e2e_a_failed_acp_turn_fails_the_run` |
| M2-02 (#16) | A push of an ACP job does not check the target for the agent or its `state_dir`; ownership moves to a node that cannot launch it | confirmed: `crates/fabricd/src/doctor.rs:378-405`, `Target::Peer` gives `Outcome::Warn`; `crates/fabricd/src/service.rs:155-160` measures fixed `Probes::ALL` with no job | `fabric-net/src/{wire.rs,rpc.rs}`, `service.rs`, `doctor.rs`, `facts.rs`, `ops.rs`, ADR 0013 | B | M2 | `e2e::preflight::e2e_a_push_refuses_a_target_without_the_jobs_acp_agent` |
| H2 | The secret-assignment rule exempts every value starting with `$` or `[`, not only `${VAR}` and earlier markers | confirmed: `crates/fabric-harness/src/redact.rs:118` (`[^\s'"$\[]`), contract at `redact.rs:42-44` | `redact.rs` | C | M2 | `redact::tests::a_literal_assignment_starting_with_a_dollar_or_a_bracket_is_redacted` |
| H1 | A signed block whose keys are escaped (`type`, `signature`) is edited with its signature kept instead of dropped | confirmed: `crates/fabric-harness/src/jsontext.rs:229-232` compares raw key bytes | `jsontext.rs` | C | M2 | `jsontext::tests::a_signed_block_with_escaped_key_names_is_dropped_whole` |
| CORE-L2 | A secret pattern may hold terminal control characters | confirmed: `crates/fabric-core/src/secret.rs:98-100` refuses whitespace, NUL and glob characters only | `secret.rs` | C | M2 | `secret::tests::a_secret_pattern_with_a_control_character_is_refused` |
| M2-03 | The store's second secret list denies ordinary files the core permits (`src/id_test.go`, `src/credentials.ts`, `.env.example`, a dependency's `credentials.js`) | confirmed: `crates/fabric-store/src/policy.rs:24-65` (`id_*`, `credentials*`, `.env.*`) against `crates/fabric-core/src/secret.rs:161-207` (exact names, templates exempt) | `policy.rs`, `capture.rs`, ADR 0004, 0014 | E | M2 | `crates/fabric-store/tests/policy.rs::the_store_classifies_secrets_as_the_core_does` |
| M2-04 (#20) | Files of `W` restore as 0644 or 0755; a 0600 key comes back readable | confirmed: `crates/fabric-store/src/restore.rs:1071-1077` | `snapshot.rs`, `capture.rs`, `restore.rs`, `manifest_codec.rs`, ADR 0014 | E | M2 | `workspace::a_file_of_the_working_tree_keeps_its_permission_bits` |
| M2-05 (#21) | The workspace root and the harness-state root lose their mode and mtime | confirmed: no root entry in `SnapshotManifest`; staging made under the umask in `restore.rs` | `snapshot.rs`, `capture.rs`, `restore.rs`, `writer.rs` | E | M2 | `workspace::the_workspace_root_keeps_its_mode_and_mtime` (harness-state root covered when its field exists) |
| M2-06 (#22) | A symbolic ref such as `refs/remotes/origin/HEAD` comes back as a direct ref | confirmed: `crates/fabric-store/src/capture.rs:2181-2196`, `restore.rs:918-958` | `snapshot.rs`, `capture.rs`, `restore.rs` | E | M2 | `workspace::a_symbolic_ref_comes_back_symbolic` |
| M2-07 | Restore deletes a session file the job dropped after checking it once, with no recheck before the unlink; an edit in that window is lost | confirmed in source, not executed: `crates/fabric-harness/src/pipeline.rs:334-339` checks in `dropped_files`, then `place_all`, then `remove_durable` unconditionally (`durable.rs:189-196`) | `pipeline.rs`, `place.rs` | E | M2 | none: needs a test seam between placement and removal; the owner adds a fault point and the test with the fix |
| ENV-1 | A changed setup recipe does not invalidate its cache's sentinel | confirmed: `crates/fabric-env/src/materialize.rs:137-149` (sentinel holds only the input digest), `detect.rs:866-874` (declared steps do not enter the digest) | `materialize.rs`, `detect.rs`, ADR 0007 | D | M2 | `freshness::a_changed_setup_recipe_fills_its_cache_again` |
| ENV-2 | A `-r` include of `requirements.txt` is not an input of the `.venv` proof | confirmed: `crates/fabric-env/src/detect.rs:1150-1165` lists only the root `requirements.txt` | `detect.rs` | D | M2 | `freshness::an_edited_included_requirements_file_makes_the_venv_stale` |
| CORE-L1 | A numeric prerelease longer than `u64` sorts as a word; `01.0.0` parses | confirmed: `crates/fabric-core/src/version.rs:101-105`, `number` at `129-133` | `version.rs` | B | M2 | `version::tests::long_numeric_prerelease_identifiers_order_as_numbers` |
| M2-08 (#28 F13) | `claude.version` refuses a newer target, unlike the Codex row | confirmed: `crates/fabricd/src/doctor.rs:716` with the equality note at `296`, `788`, `859` | `doctor.rs`, ADR 0008 | B | M2 | none: `doctor.rs` tests are private; prior partial work in `/tmp/fabric-wt/harness-rows` |
| M2-09 (#28 F12) | Doctor says "no login" when a daemon started over SSH cannot read the Keychain | confirmed as a message gap: `crates/fabricd/src/facts.rs` reports the login probe's exit only | `facts.rs`, `doctor.rs`, README | B | M2 | none: needs a Keychain-less session; not reproducible in CI |
| M2-10 (#28 F14) | `fabric serve` right after a stop refuses instead of waiting for the stopping daemon | confirmed: `crates/fabricd/src/node.rs:121-125` `try_lock` refuses on `WouldBlock` at once | `node.rs` | G3 | M2 | none: the e2e fixture does not expose the daemon's pid to stop it gracefully; the owner adds that helper |
| M2-11 | `git_bundle: null` is refused by the Rust client and accepted by the TypeScript SDK | confirmed: `crates/fabric-client/src/api.rs:2005-2006` (`#[serde(default)]` without `null_as_default`), `sdk/typescript/src/schema.ts:510` | `api.rs`, `protocol/fixtures/tolerated/*` | G3 | M2 | `crates/fabric-client/tests/null_fields.rs::a_null_git_bundle_reads_as_absent` |
| M2-12 | No operation id or idempotency key on any mutating client call | confirmed: no key field in `crates/fabric-client/src/api.rs` `Request` ops | G1, G2, G3 per `docs/managed-jobs.md` | G3 | M2 (design delivered; managed ops only) | covered by the M2 acceptance tests in `docs/managed-jobs.md` |
| R1 | No exactly-one-running guarantee after a forced steal during a partition | confirmed release boundary: `crates/fabricd/src/catchup.rs:220-257` warns and does not refuse | ADR 0020 for managed jobs; standalone unchanged | G3 | M2 for managed jobs (no automatic failover, generation fencing); standalone stays documented | none (design limit, not a bug) |
| R3 | A frozen fork and an orphan have no repair or export command | confirmed release boundary: `catchup.rs:262-344`, `ops.rs:2418-2461` | `ops.rs`, `catchup.rs` | B | M2 (moved in 2026-10-28) | none |
| M2-13 | A facts probe buffers all its output before the size check | confirmed: `crates/fabricd/src/facts.rs:586-607` uses `Command::output()` and checks `MAX_FACT_VALUE` after exit | `facts.rs` | B | M2 (moved in 2026-10-28) | none |
| M2-14 | The control socket's event channel is unbounded for a client that stops reading | confirmed: `crates/fabricd/src/control.rs:111` `unbounded_channel` | `control.rs` | G3 | M2 (moved in 2026-10-28) | none |
| M2-15 | No per-peer bound on queued request bytes across connections | confirmed: `crates/fabric-net/src/rpc.rs:956-968` caps work permits per peer, not queued frames | `rpc.rs` | B | M2 (moved in 2026-10-28) | none |
| M2-16 | `docs/net.md` still says the layer is only a library and names `fabric node pair` | confirmed: `docs/net.md:8-11`, `:37` | `docs/net.md` | B | M2 | none (docs) |
| M2-17 | README says "Secrets stay behind" and "never leave"; tracked and stashed secrets travel by design | confirmed: `README.md:3`, `:54` against `crates/fabric-store/src/capture.rs:12-13` and ADR 0014 | `README.md` | C | M2 | none (docs) |
| M2-18 | Every replication pushes the whole ledger, and each received record is checked by a linear scan | confirmed: `crates/fabricd/src/ops.rs:1238-1267`, `crates/fabricd/src/job.rs:161` | `ops.rs`, `job.rs`, `fabric-core/src/ledger.rs` | B | M2 (moved in 2026-10-28; benchmark first) | none (performance) |
| M2-19 | One store mutex covers whole captures and restores | confirmed: `crates/fabricd/src/node.rs:73`, `transfer.rs:40-51` | `transfer.rs`, `fabric-store` | G3 | M2 (moved in 2026-10-28) | none (performance) |
| M2-20 | A complete venv carried without uv is called runnable on a target that may lack its interpreter | confirmed: `crates/fabricd/src/carry.rs:410-417` | `carry.rs` | E | M2 (moved in 2026-10-28) | none |
| M2-21 | A Codex sandbox profile is reduced to a mode on restore | confirmed: `crates/fabric-harness/src/codex/rollout.rs:512-527` keeps only the sandbox type | `codex/rollout.rs` | A | M2 (moved in 2026-10-28) | none |
| M2-22 | The CLI's headless session queue and line reader are unbounded | confirmed: `crates/fabric/src/lib.rs:1073` `unbounded_channel`, line reads after it | `fabric/src/lib.rs` | A | M2 (moved in 2026-10-28) | none |
| M2-23 | ACP updates are bounded by count (1024) not bytes, with 16 MiB lines | confirmed: `crates/fabric-harness/src/acp/client.rs:48`, `:56` | `acp/client.rs` | A | M2 (moved in 2026-10-28) | none |
| M2-24 | The e2e helper `finish` waits without a deadline after killing the CLI | confirmed: `crates/fabric/tests/e2e/world.rs:778-790` unbounded `waited.recv()` | `world.rs` | H | M2 | none: the fix is in the test helper itself |
| M2-25 | Wall-time budgets only report in CI, and Linux has no baselines | confirmed: `.github/workflows/ci.yml` sets `FABRIC_BUDGET: report`; `budget.toml` `[baseline.linux-x86_64]` is empty | `ci.yml`, `budget.toml` | H | M2 (moved in 2026-10-28) | none |

## Disposition after M2

Recorded on 2026-10-27 on `m2/durable-job-foundations` after the owner branches were integrated. `fixed` means the fix and its test are on the branch and the test passes in `docs/evidence/m2-integration/gates/test.log`. Amended 2026-10-29: the rows this table first handed to a later campaign were moved into M2 on 2026-10-28 and are fixed on this branch; they now point to "Disposition after the integration", which names each fix and its test.

| ID | Status | Fix and proof |
|---|---|---|
| R2 (#17) | fixed | `381230a`: every launch starts held behind the `/bin/sh` trampoline, and the CLI releases it after the node records it; the trait ACP launch handshakes only after release. Tests: `codex_roundtrip::a_headless_launch_runs_nothing_until_it_is_released`, `acp_roundtrip::the_trait_launch_holds_the_agent_until_it_is_released`, e2e `e2e_a_headless_run_killed_before_its_harness_is_recorded_leaves_nothing_running`, `e2e_an_acp_run_killed_before_its_agent_is_recorded_leaves_nothing_running`, `e2e_a_run_ended_before_its_harness_is_recorded_stops_it`. `2b05583` releases a harness not wired to the terminal on the node's `harness_recorded` event, once its record is durable, which brought headless launch back from 1.58 to 1.23 times the baseline (`docs/evidence/m2-integration/perf.md`) |
| M2-01 | fixed | `95895ef`: the CLI prints an ACP turn's answer and exits 1 on a failed turn. Tests: `acp_cli::e2e_a_headless_acp_run_prints_the_answer`, `acp_cli::e2e_a_failed_acp_turn_fails_the_run` |
| M2-02 (#16) | fixed | `0e478e4`, `2e747cd`: `RequestBody::FactsFor`, protocol version 4, a push refused when the target lacks the job's agent. Test: `preflight::e2e_a_push_refuses_a_target_without_the_jobs_acp_agent` |
| H2 | fixed | `a6f9d17`: only a whole `${NAME}`, bare `$NAME` outside single quotes, or marker is exempt. Test: `redact::tests::a_literal_assignment_starting_with_a_dollar_or_a_bracket_is_redacted` |
| H1 | fixed | `3991afa`: escaped key names are decoded. Tests: `jsontext::tests::a_signed_block_with_escaped_key_names_is_dropped_whole`, `props::escaped_key_spellings_are_redacted_as_the_canonical_ones` |
| CORE-L2 | fixed | `25c0048`. Test: `secret::tests::a_secret_pattern_with_a_control_character_is_refused` |
| M2-03 | fixed | `cc336a2`, `3f4346f`: the store asks the core's `SecretPolicy`. Tests: `policy::the_store_classifies_secrets_as_the_core_does`, `workspace::untracked_work_the_core_carries_comes_back` |
| M2-04 (#20) | fixed | `e8d1834`, `a458b96`: manifest format 3 keeps each file's mode. Test: `workspace::a_file_of_the_working_tree_keeps_its_permission_bits` |
| M2-05 (#21) | fixed | `2ef2542`: root mode and mtime of the workspace and harness state. Tests: `workspace::the_workspace_root_keeps_its_mode_and_mtime`, `the_harness_state_root_and_a_read_only_root_keep_their_mode_and_mtime` |
| M2-06 (#22) | fixed | `bf51d26`. Test: `workspace::a_symbolic_ref_comes_back_symbolic`. A restore of a cloned workspace runs one more `git` for `refs/remotes/origin/HEAD`; `budget.toml` records it (`84167d5`) |
| M2-07 | fixed | `dd632aa`: a dropped file is checked again before its unlink. Test: `pipeline::tests::a_dropped_file_edited_before_its_removal_is_kept` |
| ENV-1 | fixed | `449c306`: the sentinel holds a steps digest. Test: `freshness::a_changed_setup_recipe_fills_its_cache_again` |
| ENV-2 | fixed | `3bb0dac`: `-r` and `-c` includes are `.venv` inputs. Test: `freshness::an_edited_included_requirements_file_makes_the_venv_stale` |
| CORE-L1 | fixed | `3b753bd`. Test: `version::tests::long_numeric_prerelease_identifiers_order_as_numbers` |
| M2-08 (#28 F13) | fixed | `bc4f9aa`: only an older harness on the target is refused. Test: `doctor::tests::only_an_older_harness_on_the_target_is_refused` |
| M2-09 (#28 F12) | fixed | `f814d50`, `37d5be1`: fact `claude.keychain` and the README note. Tests: `facts::the_keychain_is_asked_only_when_claude_reports_no_login_outside_the_desktop`, `doctor::a_target_whose_daemon_cannot_read_the_keychain_says_so` (fake `launchctl` and `security`; a real SSH session is not reproducible in CI) |
| M2-10 (#28 F14) | fixed | `56c80cb`: a start waits up to five seconds for a stopping daemon's lock. Tests: `node::tests::a_start_waits_for_a_stopping_daemon_to_let_the_state_directory_go`, e2e `e2e_a_daemon_started_right_after_a_stop_serves` (the stopping daemon held by the new `while_stopping:stall` fault point); both fail with the wait set to zero |
| M2-11 | fixed | `8e253f2`: `git_bundle` reads `null` as left out; fixture `protocol/fixtures/tolerated/git_bundle_written_as_null.json` read by both SDKs. Test: `null_fields::a_null_git_bundle_reads_as_absent` |
| M2-12 | fixed | managed ops carry an operation key (`docs/managed-jobs.md`). Tests: `fabric-journal` `tests::a_duplicate_command_gets_the_same_identity_and_bytes_before_and_after_a_restart`, `crash_tests::a_command_killed_after_its_commit_answers_its_retry_with_the_same_identity`, e2e `managed::e2e_a_managed_job_keeps_identity_and_progress_across_a_daemon_kill`. Standalone ops keep no key by design |
| R1 | fixed for managed jobs | ADR 0020: generation fencing, no automatic failover, `UNIQUE(job_id, generation)`. Tests: `fabric-core/tests/managed_rules.rs`, journal recovery tests. Standalone jobs keep the documented release boundary |
| R3 | fixed after the integration | `4b708e7`, `c0f0a72`, `2dad2d1`; see "Disposition after the integration" |
| M2-13 | fixed after the integration | `ebfa2f5`; see "Disposition after the integration" |
| M2-14 | fixed after the integration | `e823af3`, `4e6c443`; see "Disposition after the integration" |
| M2-15 | fixed after the integration | `d8cedec`, then `cdb1c43` and `f8742a2` from the PR #31 review; see "Disposition after the integration" and "Disposition after the PR #31 review" |
| M2-16 | fixed | `3e53359`, `a840c89`: `docs/net.md` and the peer book's module doc name `fabric node add` |
| M2-17 | fixed | `ea05c56`: the README says which secrets stay and which travel |
| M2-18 | fixed after the integration | `d7c1e15`, `ba51198`, after the benchmark `bd76d69`; see "Disposition after the integration" |
| M2-19 | fixed after the integration | `676426f`, `360dd46`; see "Disposition after the integration" |
| M2-20 | fixed after the integration | `bb0c3f1`; see "Disposition after the integration" |
| M2-21 | fixed after the integration | `7a23746`; see "Disposition after the integration" |
| M2-22 | fixed after the integration | `01c7ee9`; see "Disposition after the integration" |
| M2-23 | fixed after the integration | `608e14b`, `863b927`; see "Disposition after the integration" |
| M2-24 | fixed | `b957ab2`: `finish` waits at most ten seconds for a killed command's output. Test: `world::a_killed_command_whose_child_keeps_its_pipes_fails_in_time` (fails in 40 s with the old wait) |
| M2-25 | fixed after the integration | `29aa540`, `c620d9f`, `b2763b4`, `acc3146`, then `0381081` and `13f29d1` from the PR #31 review; see "Disposition after the integration" |
| M2-26 | new, fixed after the integration (`fd41b9e`; see "Disposition after the integration") | Found while fixing M2-02: `NodeFacts::to_wire` in `crates/fabricd/src/facts.rs` answers no facts at all when a node has more than `MAX_FACTS` (64), through `unwrap_or_default`. About 43 fixed names exist today, and setup-step programs add one each, so a spec with many setup programs can reach it. A peer then sees every fact missing and refuses the move, so it fails closed, but with a misleading reason. Owner B |

## Disposition on the group branches

Recorded on 2026-10-28 by the budget and CI group, once a later campaign stopped being a disposition: every finding the table above had not given to M2 was given to a group, which fixes it with a test that fails first. M2-25 is this group's and is on its branch. The others are on their groups' branches, not yet on `m2/durable-job-foundations`; this table says where each fix was so the integration could check it; "Disposition after the integration" names each fix's commit on this branch. All of them merged without a conflict into one integration, on which the budget's count mode and the counted A/B runs of `docs/evidence/perf-ab/README.md` ran.

| ID | Status | Fix and proof |
|---|---|---|
| M2-25 | fixed | `pd/wip-budget-ci-evidence`. `188fb32`: `linux-x86_64` baselines, the anchor's times on CI's `ubuntu-latest` runner (median of three CI runs of `176622e`'s code, `scripts/budget-baselines.sh` from `ef43373`). `3ba7b21`: CI runs the budget with `FABRIC_BUDGET=enforce`; the macOS runner, whose runs of one commit differ by up to 1.34 times, names a platform with no baselines and holds only process counts. `c5a12e4`: `scripts/check-budget.sh` refuses a raised or removed baseline and a looser ratio, grace, percentile or run count, so a baseline stays the anchor's. `6792db6`: a time over its limit no longer tells to raise the baseline. Tests: `budget::every_op_has_a_baseline_on_linux_and_macos` and `budget::ci_enforces_the_budget` (each failed before), 12 new cases of `scripts/test-check-budget.sh` that failed against the old script, and the message case of `budget::the_verdicts_follow_the_rules` |
| M2-13 | fixed, on `pd/wip-facts-net-carry` then; integrated since | `32fe4bd`. Tests: `a_probe_stops_reading_once_its_output_passes_what_a_fact_holds`, `an_endless_output_is_read_only_until_it_cannot_be_a_fact`, `a_fact_read_as_it_arrives_is_the_fact_read_whole` |
| M2-14 | fixed, on `pd/wip-node-runtime` then; integrated since | `28227da`, `266ce9b`, `922522c`, ADR note `a495cb6`. Test: `a_client_that_stops_reading_holds_a_bounded_backlog_and_gets_the_answer` (`8947330`, first) |
| M2-15 | fixed, on `pd/wip-facts-net-carry` then; integrated since | `c20483f`. Test: `request_bytes_held_for_a_peer_are_bounded_over_its_sessions` |
| M2-18 | fixed, on `pd/wip-cli-protocol-ops` then; integrated since | `e56c6bf`, `1ecc017`, after the benchmark `9fa6324`. Tests: `the_hash_index_finds_every_record_and_nothing_else`, `a_push_sends_what_the_peer_lacks_and_the_whole_ledger_to_a_peer_that_lost_it` |
| M2-19 | fixed, on `pd/wip-node-runtime` then; integrated since | `8b6f90a`, `15e36b8`, after the benchmark `c344206`. Tests: `a_short_store_call_runs_beside_a_long_one` (`6007c13`, first), `captures_restores_and_reads_on_handles_at_once_leave_a_clean_store`, `gc_and_forget_are_refused_while_another_handle_lives` |
| M2-20 | fixed, on `pd/wip-facts-net-carry` then; integrated since | `86fedc8`. Test: `a_finished_venv_without_its_installer_travels_only_to_its_interpreter` |
| M2-21 | fixed, on `pd/wip-launch-hold` then; integrated since | `f2857db`. Tests: `the_session_s_sandbox_travels_whole`, `a_turn_s_writable_roots_and_network_access_are_read`, and four more in that commit |
| M2-22 | fixed, on `pd/wip-cli-protocol-ops` then; integrated since | `c2aea4c`. Tests: `named_sessions_wait_in_order_and_in_bounds`, `a_line_past_the_bound_is_dropped_without_holding_it` |
| M2-23 | fixed, on `pd/wip-launch-hold` then; integrated since | `cba3776`, ADR note `14c2d0f`. Tests: `updates_are_bounded_by_bytes_as_well_as_by_count`, `a_queue_full_of_bytes_drops_the_newest_and_counts_it` |
| M2-26 | fixed, on `pd/wip-facts-net-carry` then; integrated since | `af473fe`. Tests: `an_answer_with_no_room_for_every_fact_keeps_the_measured_ones_and_names_the_rest`, `a_peer_answer_that_left_facts_out_names_that_cause` |
| R3 | fixed, on `pd/wip-cli-protocol-ops` then; integrated since | `f733771`, `9f4fab6`. Tests: `a_frozen_owner_exports_its_orphan_and_takes_the_chain_the_fork_rule_picks`, e2e `e2e_a_frozen_owner_exports_its_orphan_and_repairs_the_fork` |

The time budget now also counts every probe and harness each op starts (`c6840ca`), and `FABRIC_BUDGET=count` checks the counts without timing (`7989b66`). Its counts are those of the integration; two count runs agreed, and a third after `pd/wip-store-git` gained `7939faa`. On a branch alone the counts of the other branches' changes fail it: they hold only once every branch is in.

The performance work items of `docs/evidence/perf-ab/README.md` and the counters of the integration against the anchor are in that README, section "Integration against the anchor, counted".

## Disposition after the integration

Recorded on 2026-10-28 when the integrator cherry-picked the seven deferred-work branches onto `m2/durable-job-foundations` (`8d35276..pd/wip-*`, in the order cli-protocol-ops, launch-hold, store-git, durability, node-runtime, facts-net-carry, budget-ci-evidence; no conflict). Every finding the table above left on a branch is now fixed on this branch; the commits have new ids here, and `docs/evidence/m2-deferred-integration/sha-map.tsv` maps each branch commit to its copy. The full gates pass on the integrated head (`docs/evidence/m2-deferred-integration/README.md`).

| ID | Status | Fix on this branch (branch commit) | Tests |
|---|---|---|---|
| M2-13 | fixed | `ebfa2f5` (`32fe4bd`) | `a_probe_stops_reading_once_its_output_passes_what_a_fact_holds`, `an_endless_output_is_read_only_until_it_cannot_be_a_fact`, `a_fact_read_as_it_arrives_is_the_fact_read_whole` |
| M2-14 | fixed | `e823af3` (`28227da`), `4e6c443` (`922522c`); `a3835b6` and its revert `b37194f` cancel out; ADR note `fe193ec` (`a495cb6`) | `a_client_that_stops_reading_holds_a_bounded_backlog_and_gets_the_answer`, committed first in `f44a039` (`8947330`) |
| M2-15 | fixed | `d8cedec` (`c20483f`) | `request_bytes_held_for_a_peer_are_bounded_over_its_sessions` |
| M2-18 | fixed | `d7c1e15` (`e56c6bf`), `ba51198` (`1ecc017`), after the benchmark `bd76d69` (`9fa6324`) | `the_hash_index_finds_every_record_and_nothing_else`, `a_push_sends_what_the_peer_lacks_and_the_whole_ledger_to_a_peer_that_lost_it` |
| M2-19 | fixed | `676426f` (`8b6f90a`), `360dd46` (`15e36b8`), after the benchmark `38558de` (`c344206`) | `a_short_store_call_runs_beside_a_long_one`, committed first in `964315a` (`6007c13`), `captures_restores_and_reads_on_handles_at_once_leave_a_clean_store`, `gc_and_forget_are_refused_while_another_handle_lives` |
| M2-20 | fixed | `bb0c3f1` (`86fedc8`) | `a_finished_venv_without_its_installer_travels_only_to_its_interpreter` |
| M2-21 | fixed | `7a23746` (`f2857db`) | `codex_roundtrip::a_resume_carries_the_session_s_writable_roots_and_network_access` and the unit tests of that commit |
| M2-22 | fixed | `01c7ee9` (`c2aea4c`) | `named_sessions_wait_in_order_and_in_bounds`, `a_line_past_the_bound_is_dropped_without_holding_it` |
| M2-23 | fixed | `608e14b` (`cba3776`), ADR note `863b927` (`14c2d0f`) | `updates_are_bounded_by_bytes_as_well_as_by_count` |
| M2-25 | fixed | `29aa540` (`188fb32`), `c620d9f` (`3ba7b21`), `b2763b4` (`c5a12e4`), `acc3146` (`6792db6`) | `budget::every_op_has_a_baseline_on_linux_and_macos`, `budget::ci_enforces_the_budget`, `budget::the_verdicts_follow_the_rules`, `scripts/test-check-budget.sh` |
| M2-26 | fixed | `fd41b9e` (`af473fe`) | `an_answer_with_no_room_for_every_fact_keeps_the_measured_ones_and_names_the_rest`, `a_peer_answer_that_left_facts_out_names_that_cause` |
| R3 | fixed | `4b708e7` (`f733771`), CLI `c0f0a72` (`9f4fab6`), ADRs `2dad2d1` (`b41ec48`) | `catchup::fork_tests`, `a_frozen_owner_exports_its_orphan_and_takes_the_chain_the_fork_rule_picks`, e2e `e2e_a_frozen_owner_exports_its_orphan_and_repairs_the_fork` |

## Disposition after the PR #31 review

Recorded on 2026-10-29. A review of `31577c4` found five defects; each was reproduced with a test that failed before its fix and is fixed on this branch (`docs/evidence/m2-review-pr31/README.md`). Two of them finish M2-15 and M2-25 above.

| Finding | Status | Fix | Test, and what it read before the fix |
|---|---|---|---|
| A restore could evict another job's bundle cache between that job's reuse and its copies | fixed | `0c4f287` | `restore::tests::a_cache_another_restore_evicts_after_reuse_costs_a_replay_not_the_restore` (failed with `NotFound`) |
| A second fork repair at one fork point wrote over the ledger the first repair kept | fixed | `346f932` | `ops::tests::a_second_repair_at_one_fork_point_keeps_the_first_kept_ledger`, `job::tests::adopt_never_replaces_another_kept_ledger` |
| M2-15's per-peer request bound was bypassed: the transports read each waiting request off the wire before the room gate | fixed | `cdb1c43`, `f8742a2` | `rpc::tests::request_bytes_read_off_the_wire_for_a_peer_are_bounded` (50,339,502 bytes before, 17,305,038 after), `quic::tests::a_peer_can_leave_at_most_the_connection_window_unread`, `mux::tests::a_stream_the_peer_opened_gets_credit_only_for_what_its_reader_asks`, `pipe::tests::credit_on_demand_is_what_a_read_asked_for_and_no_more` |
| M2-25's `check-budget.sh` compared no process count, and an `anchor` line let every baseline rise unchecked | fixed | `0381081`, `13f29d1` | `scripts/test-check-budget.sh`, 60 cases; the review's two cases passed the old script and fail now |
| A headless ACP turn gathered its whole answer with no bound | fixed | `fc73523` | `acp::tests::a_drained_turn_keeps_at_most_max_answer_bytes_of_answer` (held 25,165,824 bytes before), `an_answer_is_cut_at_a_character_and_stays_a_prefix` |

## Candidates the audit rejected, rechecked

- Tracked edits silently reverted by the store's ignore globs: invalid. Git capture stages tracked changes with `git add -A` before policy filtering (`crates/fabric-store/src/capture.rs`), and the store tests restore tracked edits.
- An interactive terminal launch running before registration: invalid. `process.rs:340` holds it, and `lib.rs:727-751` registers before `release`.

## Prior partial work

Earlier runs left uncommitted or unmerged work in `/tmp/fabric-wt` (read only here): `hold-launch` (R2, M2-01), `acp-push-check` (M2-02, three commits), `store-fidelity` (M2-04 to M2-06), `harness-rows` (M2-08 to M2-10), `perf-open-check` (an lsof narrowing the performance evidence rejects as a safety change). Owners may read them; none is merged.
