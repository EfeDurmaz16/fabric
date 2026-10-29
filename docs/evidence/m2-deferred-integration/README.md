# Integration of the M2 deferred work

On 2026-10-28 the seven deferred-work branches were cherry-picked onto
`m2/durable-job-foundations`, each as `8d35276..pd/wip-<group>`, in this
order: cli-protocol-ops, launch-hold, store-git, durability, node-runtime,
facts-net-carry, budget-ci-evidence. No two branches touched the same file
outside `docs/evidence/`, and no pick conflicted. The 94 picked commits have
new ids here; `sha-map.tsv` maps each branch commit to its copy, so the
branch ids that each group's evidence and commit messages quote stay
resolvable after the branches are removed.

| Group | Branch head | Commits |
|---|---|---:|
| cli-protocol-ops | `bf14115` | 24 |
| launch-hold | `6586ef9` | 7 |
| store-git | `1aad81b` | 16 |
| durability | `7d8f2b9` | 9 |
| node-runtime | `50df18c` | 16 |
| facts-net-carry | `66d5a3b` | 9 |
| budget-ci-evidence | `db83aec` | 13 |

## What the integration added

The requests the groups made for files outside their own, and what the
integrated tree still showed, each with a test that fails without it:

| Commit | What | Test |
|---|---|---|
| `35b8929` | the A/B bench reads each daemon's physical footprint beside its resident size (facts-net-carry's request) | `ab::a_run_reports_the_daemons_physical_footprint` |
| `9635ea0` | release builds use fat LTO and one codegen unit: the daemon and CLI resident sizes that were above the anchor came from the code pages of a binary 8.7% larger, not from the heap | the counted A/B before and after, `docs/evidence/perf-ab/lto-*` |
| `7717d07` | a new job's directory is ordered with a barrier, not a full flush (durability's request) | `a_new_jobs_directory_is_made_durable_by_the_flushes_of_its_files` |
| `0f5b363` | a dropped launch handle whose trampoline the caller reaped signals no process group (launch-hold's note) | `dropping_a_handle_whose_trampoline_was_reaped_signals_no_group` |
| `725dcff` | `Store::root` instead of the parent of `tmp/` (store-git's request) | refactor |
| `a60b0a5` | gc removes the bundle cache of a job with no snapshot left (store-git's request) | `gc_removes_the_bundle_cache_of_a_job_with_no_snapshot_left` |
| `3a95113` | ADR 0014 says where pins live (cli-protocol-ops's request) | docs |
| `e59c667` | a capture's pending departure record joins its one full flush on the same device (durability's request) | `a_pending_departure_on_the_trees_device_joins_its_one_full_flush` |
| `f0392af` | the counting tools run once at install, so a loaded machine's first-run check of a new executable no longer outlasts a probe's timeout (the flake durability saw) | `a_counting_tool_has_run_once_before_a_measure_and_logged_nothing` |
| `6488838` | an e2e move and way back of a repository on `master` (store-git's request) | `e2e_a_job_on_master_moves_and_resumes`, which fails on the tree before the store's HEAD fix |
| `4a386ef` | the e2e doc comment and ADR 0016 say how a held headless or ACP launch ends when its CLI is killed (launch-hold's request) | docs |
| `202a844` | item 19 finished: a preflight measures only the tools its job reads, and `FactsFor` names them (cli-protocol-ops's and budget-ci-evidence's finding) | `a_preflight_of_the_jobs_tools_stops_where_one_of_every_tool_does`, `a_move_measures_only_the_tools_its_job_reads`, `a_capture_that_names_more_than_the_preflight_measured_is_measured_then` |
| `2a07e5b`, `dc8b87c` | CONTRIBUTING names the footprint; the defect register records every deferred finding fixed here | docs |

The process counts in `crates/fabric/tests/e2e/budget.toml` already held the
ps change cli-protocol-ops asked for (budget-ci-evidence wrote it); the
count run of `budget::hot_paths` on the integrated tree agreed with every
entry until `202a844`, which adds its own `[[change]]`.

## Gates

Every gate of CONTRIBUTING passed on `202a844`, the last commit that changes
code (`gates/summary-202a844.txt`), run one after another in a scratch
worktree of that commit: fmt, clippy of the workspace and of each harness
feature alone, `cargo test --workspace --locked` (1138 passed, 0 failed, 10
ignored), the end-to-end test (69 passed, 5 ignored), rustdoc with `-D
warnings`, `cargo deny check`, the hygiene, fixture and budget scripts and
their own tests against `origin/main`, `budget::hot_paths` in count mode,
shellcheck of `scripts/`, and the TypeScript SDK (install from the lockfile,
typecheck, build, 62 tests against a node built from the tree). The commits
after it change only documents and evidence.

## Performance

`docs/evidence/perf-ab/README.md`, section "The integrated head, timed",
has the A/B runs against the anchor `176622e` and against the M2 head
`f381d42`, in the test and the release profile, and an A/A control, all
taken alone on the machine. In short: no op is slower than the anchor in
either profile, and 23 of 26 are faster in both (restart 0.13 to 0.19, a new
job 0.37 to 0.45, a resume 0.43 to 0.55, a move 0.63 to 0.67); in release
both daemons and every CLI use less memory than the anchor's. In the test
profile, which builds no optimized code, daemon a's resident size is 1.047
of the anchor's and some CLI peaks are up to 656 KiB higher, from the code
pages of an unoptimized binary 8% larger; the physical footprint is not
larger. This was left for Efe to rule on, and he ruled on 2026-10-29 that
the performance rule gates the release build only: the test profile is
tracked, not gated.
