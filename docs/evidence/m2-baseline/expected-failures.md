# M2 expected failures: each run on this branch

Each intended-behavior test of `defects.md` was run alone with `--ignored` on 2026-10-27, on main `176622e` plus the tests, and failed at the assertion that states the intended behavior (not at setup). The command and the failing assertion for each:

| ID | Command | Fails at |
|---|---|---|
| R2 | `cargo test -p fabric-harness --test codex_roundtrip -- --ignored a_headless_launch_runs_nothing_until_it_is_released` | `assert!(running.is_held())` |
| R2 | `cargo test -p fabric-harness --test acp_roundtrip -- --ignored the_trait_launch_holds_the_agent_until_it_is_released` | `assert!(running.is_held())` |
| M2-01 | `cargo test -p fabric --test e2e -- --ignored acp_cli::` | the answer `echo: hello` is missing from the output; a failed turn exits 0 |
| M2-02 | `cargo test -p fabric --test e2e -- --ignored preflight::` | the push with `--force` to a target without the agent succeeds |
| H2 | `cargo test -p fabric-harness --lib -- --ignored a_literal_assignment_starting_with_a_dollar_or_a_bracket_is_redacted` | `DB_PASSWORD=$literal-password` does not match |
| H1 | `cargo test -p fabric-harness --lib -- --ignored a_signed_block_with_escaped_key_names_is_dropped_whole` | `dropped_signed_blocks` is 0 for `type` |
| CORE-L2 | `cargo test -p fabric-core --lib -- --ignored a_secret_pattern_with_a_control_character_is_refused` | `x\u{1b}c` parses |
| CORE-L1 | `cargo test -p fabric-core --lib -- --ignored long_numeric_prerelease_identifiers_order_as_numbers` | `alpha.99999999999999999999 < alpha.100000000000000000000` is false |
| M2-03 | `cargo test -p fabric-store --test policy -- --ignored` | `src/id_test.go` is a secret |
| M2-04 | `cargo test -p fabric-store --test workspace -- --ignored a_file_of_the_working_tree_keeps_its_permission_bits` | `deploy_key` restores 0644, not 0600 |
| M2-05 | `cargo test -p fabric-store --test workspace -- --ignored the_workspace_root_keeps_its_mode_and_mtime` | the root restores 0755, not 0700 |
| M2-06 | `cargo test -p fabric-store --test workspace -- --ignored a_symbolic_ref_comes_back_symbolic` | `git symbolic-ref refs/remotes/origin/HEAD` fails on the target: not a symbolic ref |
| ENV-1 | `cargo test -p fabric-env --test freshness -- --ignored a_changed_setup_recipe_fills_its_cache_again` | the cache is `Healthy` after the recipe changed |
| ENV-2 | `cargo test -p fabric-env --test freshness -- --ignored an_edited_included_requirements_file_makes_the_venv_stale` | `.venv` is `Healthy` after `requirements/base.txt` changed |
| M2-11 | `cargo test -p fabric-client --test null_fields -- --ignored` | `invalid type: null, expected a boolean` |

All commands take `--locked`. With every one of them still ignored, the gates of `gates.md` pass unchanged on this branch.

## Status on `m2/durable-job-foundations` (2026-10-27)

Every ID above is fixed. Each test's `ignore` was removed in the commit that fixed it, and each passes in the full workspace run of `docs/evidence/m2-integration/gates/test.log`. No expected-failure marker is left in the workspace: the six ignored tests that remain are the two fixture generators, two benchmarks, the time budget (run by its own CI step) and the child process of a ledger crash test. The disposition of every register ID is in `defects.md`, section Disposition after M2.
