# M2 review findings: fixes, gates and performance

Five review findings on `m2/durable-job-foundations`, verified and fixed on 2026-10-27.

| Finding | Fix | Regression test |
|---|---|---|
| Recovery offered a lost or running attempt's grant for dispatch, and `mark(Dispatched)` accepted it (two reports of one root cause) | `0190ada`: the dispatch list is read after the probes and holds only grants whose attempt is still `granted` at the current generation; `mark_operation` refuses `Dispatched` for any other grant. Replay is unchanged. | `fabric-journal` `recovery_never_offers_a_lost_grant_and_it_cannot_run_next_to_its_replacement`, `recovery_never_offers_the_grant_of_an_attempt_it_saw_running` (both fail without the fix) |
| `managed_create` recorded a secret in ACP arguments | `242c396`: `refuse_secret_kind` before the envelope is built | `fabricd` `a_managed_create_whose_harness_arguments_hold_a_secret_is_never_recorded` (fails without the fix) |
| A standalone lookup waited behind recovery and `managed_list` on the authority queue | `e355fae`: the journal's ids are published through a watch channel once the file opens, before `verify`; a journal that does not open fails the lookup closed | `authority::tests::the_managed_ids_are_published_outside_the_queue_and_survive_a_restart`, `a_journal_that_does_not_open_leaves_every_id_refused` |
| A peer append could make a standalone job under a managed id | `6089ea2`: `accept_record` and `answer_append` refuse a managed id, or any new id while the ids cannot be read | `ops::tests::a_peer_record_never_makes_a_standalone_job_under_a_managed_id`, `a_peer_record_makes_no_job_while_the_managed_ids_cannot_be_read` (both fail without the fix) |

Lookup probe (scratch test, release build, 5,000 jobs, not committed): the first lookup of an unknown id after a start took 1.2 to 2.2 ms through the published ids and 247 to 275 ms through the queue (`authority.job`), which waits for verify and recovery. A `managed_list` took 35 to 43 ms; a lookup after the open took about 1 microsecond.

## Gates

Run by `docs/evidence/m2-integration/gates/run-gates.sh` on commit `6089ea2` (`gates/`). Every gate passed: fmt, clippy (workspace and each harness feature), `cargo test --workspace --locked` with 1069 passed, 0 failed, 6 ignored, doc, deny, hygiene, budget file, fixtures and their script tests, and the TypeScript SDK.

## Performance

`scripts/bench-m2.sh docs/evidence/m2-review/perf 3 15` on commit `6089ea2`, same machine and method as `docs/evidence/m2-integration/perf.md`, baseline `docs/evidence/m2-baseline/perf`. Load average 4.9 to 7.8 (`perf/machine.txt`). Every budget op was within its limit with process counts equal to `budget.toml`.

| op | baseline median p25 (ms) | after (ms) | ratio |
|---|---:|---:|---:|
| acp / claude / codex suspend | 344 / 403 / 623 | 345 / 404 / 601 | 1.00 / 1.00 / 0.96 |
| acp / claude / codex move | 140 / 142 / 139 | 140 / 141 / 142 | 1.00 / 0.99 / 1.02 |
| acp / claude / codex resume | 538 / 703 / 894 | 533 / 694 / 904 | 0.99 / 0.99 / 1.01 |
| acp / claude / codex pull | 992 / 1206 / 1640 | 966 / 1220 / 1596 | 0.97 / 1.01 / 0.97 |
| cold start / restart | 56 / 74 | 57 / 72 | 1.02 / 0.97 |

Bench medians (ms): acp / claude / codex launch 63.7 / 86.9 / 98.8 before, 58.8 / 101.9 / 116.0 after (0.92 / 1.17 / 1.17, as in the integration run, which had 1.23 / 1.20); new, approve, ledger append, cli ls and cli version all within 0.98 to 1.09. No op is over 1.5 times its baseline.
