# M5 evidence: intent-first placement and the observed job

Recorded on 2026-11-08 on `m5/placement-continuity`, after owners A and B merged (`80e2762`). The selections behind M5 stand as selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02; none was approved by Efe. The host is an Apple Silicon Mac, macOS arm64, with other work running (load averages are in each log). Both daemons of every test run on this one host with separate state roots and a TCP and TLS node link between them: this is two processes on one host, not two hosts. No cloud, no paid resource and no real credential was used; the harness on `PATH` is the e2e fixture's fake Codex. Nothing here was timed beyond what the logs print; the time budget ran in count mode.

## What this step found and changed

| Item | Result |
|---|---|
| The submit was never placed | `Node::place_job` had no caller outside its unit tests: a create was accepted and stayed `accepted` with no decision. `735aa2c` adds `fabricd::dispatch`, which places every accepted create from a `snapshot`, `git` or `empty` source after its commit and after a restart, and records the decision with its grant or the park. A `repository` create keeps the M2 path |
| Every real harness was excluded | the advert carried Codex as `codex-cli 0.155.1`, which the asking node cannot parse, so both nodes were excluded `harness_missing` (seen red in the first two-daemon run: `"reasons":[{"harness_missing":{"harness":{"kind":"codex"}}}]` on both candidates). `335aaec` advertises the version in the output |
| Watch across an authority restart | no test covered it; `205013f` adds one. It passed on its first run: it pins behaviour the journal already had |

## The acceptance cases through real daemons

`acceptance/acceptance.log` is the run, `acceptance/run.txt` its commit, load, and the `fabric` process count before and after (0 and 0). Command, from the worktree, with a scratch `CARGO_TARGET_DIR`:

```
cargo test -j 6 --locked -p fabric --test e2e -- --exact \
  placement::e2e_a_submit_names_no_machine_and_the_decision_is_recorded \
  placement::e2e_a_create_accepted_before_a_crash_is_placed_once_after_the_restart \
  facade::e2e_the_same_job_is_observed_after_a_reconnect_from_another_node \
  facade::e2e_a_watch_cursor_holds_across_an_authority_restart --test-threads 1
```

| Case | Test | What is checked |
|---|---|---|
| Two eligible nodes and the decision record | `placement::e2e_a_submit_names_no_machine_and_the_decision_is_recorded` | the job is submitted from `b` to the authority `a` through the facade with no node named; the inspection read from `b` holds one `initial` placement with both nodes `eligible`, the chosen node first in rank order, `parked: false`, one attempt `granted`, and the watch's `attempt_granted` names the chosen node and that attempt |
| A secret only one node can resolve | the same test | `b`'s daemon alone has `FABRIC_SECRET_DEPLOY_KEY`; a submit with `--secret DEPLOY_KEY` chooses `b`, `a` is excluded with exactly `secret`, and the value appears nowhere in the inspection |
| No eligible node parks the submit | the same test | `--secret NOBODY_HAS_THIS`: accepted, `chosen: null`, `parked: true`, the job `active` with no attempt, both nodes excluded with `secret` |
| A pin | the same test | `--pin a` chooses `a` with `b` excluded `pin`, and `--pin b` the reverse, whatever the ranking says |
| A create accepted before a crash | `placement::e2e_a_create_accepted_before_a_crash_is_placed_once_after_the_restart` | `a` armed with `before_placement` answers the submit `accepted`, reaches the point and is killed with `SIGKILL`; after the restart the job has exactly one placement and one attempt, and a later commit places nothing again |
| The same job observed after a client reconnect | `facade::e2e_the_same_job_is_observed_after_a_reconnect_from_another_node` | from `b`: the history in order, a reader killed mid-wait, the next read from its cursor gets only later events, then a revoke closes the next call |
| The watch cursor across an authority restart | `facade::e2e_a_watch_cursor_holds_across_an_authority_restart` | `a` killed with `SIGKILL` and started again between two reads from `b`; the second read from the old cursor returns only later events (the note made after the restart among them); the whole history keeps its prefix, order and count |

Counters for that run: daemons started 10 (2, 3, 2, 3), `SIGKILL`s 2, placement probe rounds 8, each over both nodes (one per create: the 6 facade submits and the 2 `managed create --empty` the facade tests make on `a`). Each placement test fails with the dispatch removed (the mutation run timed out waiting for a placement in both tests, `0 passed; 2 failed`).

## The launch path, proved

The rows that were blocked on 2026-11-08 because no granted attempt was ever launched. The launch path (option (a), selected 2026-11-08 under Efe's delegation of 2026-11-02 and not approved by Efe; the contract is docs/managed-jobs.md, "Launch path (M5, as built)") landed with owners A and B and was merged at `305e65b`. This step ran it across two daemons, found two defects, and fixed them:

| Item | Result |
|---|---|
| A continue's pin was dropped | `managed continue --pin` and the facade's `continue` carried the pin in the effect request and `dispatch::place_handoff` placed with the job's policy, so the work could not be moved to a named node. `ffe8a2b` places the handoff with the continue's pin; `dispatch::tests::a_continue_s_pin_places_the_handoff_where_it_names` failed first (`left: [None]`) |
| A continuation could not start on a node new to the job | the two daemons of every earlier test shared one home, so the job root's parents (`<home>/fabric/jobs/<job>`) already existed on the target. With a home per daemon, as on two hosts, the restore staged beside a root whose parent did not exist and the launch ended `blocked` with no such file. `69e9c3d` makes the parents first; `sources::tests::a_snapshot_restores_where_no_parent_of_the_root_exists` failed first |
| No capture of the link | `c2fd4e1` adds `FABRIC_NET_TAP` behind the `tap` feature of fabric-net, which only fabricd's `faults` feature turns on: a node appends every frame payload it writes and every chunk it takes, decompressed, before TLS. No build without the feature can write a frame anywhere |

`acceptance/launch-path.log` is the run, `acceptance/launch-path-run.txt` its commit, load, and the `fabric` process count before and after (0 and 0). Command, from the worktree, with a scratch `CARGO_TARGET_DIR`:

```
cargo test -j 6 --locked -p fabric --test e2e -- --exact \
  launch::e2e_a_job_submitted_to_a_runs_on_b_and_continues_on_a_with_its_package \
  launch::e2e_a_handoff_an_escaped_child_blocks_leaves_the_attempt_running_on_b \
  placement::e2e_a_submit_runs_on_the_eligible_node_without_naming_it \
  continuity::e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved \
  secrets::e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log \
  managed::e2e_a_cancel_stops_a_live_attempt_and_records_the_tasks_interrupted \
  managed::e2e_a_checkpoint_cuts_a_live_attempt_and_it_goes_on \
  continuity::e2e_a_native_session_continues_on_a_node_with_the_same_layout \
  --test-threads 1 --nocapture
```

Output: `test result: ok. 7 passed; 0 failed; 1 ignored`. The `launch` tests run each daemon with a home of its own, so its job roots are its own as on two hosts, and the TCP and TLS link between them.

| Row | Test | What is checked |
|---|---|---|
| A job submitted through the facade is placed on the other node and starts there | `launch::e2e_a_job_submitted_to_a_runs_on_b_and_continues_on_a_with_its_package` | submitted from `b` to the authority `a`'s facade with `--pin b` and `--secret DEPLOY_KEY`: the placement chooses `b`, `a` launches it there over the link, the attempt is `running`, `b`'s executor records the harness and nothing of it is on `a`. The fake Codex runs its turn |
| A build task runs its program | the same test | `managed register --class build -- sh -c 'echo <marker> > built.txt && sleep 30'` on `a` is spawned on `b`: `b` records `Build` and the marker is in `b`'s job root |
| A test task runs, and an interrupted test is recorded interrupted | the same test, and `continuity::e2e_a_planned_handoff_records_child_outcomes_and_the_fidelity_achieved` | the test runs only with the job's secret (`test -n "$DEPLOY_KEY" && sleep 30`); at the cut its receipt is `interrupted`, `interrupted: true`, never passed |
| The harness's turn is captured | the same test, and `managed::e2e_a_checkpoint_cuts_a_live_attempt_and_it_goes_on` | `managed checkpoint`: the harness receipt is `checkpointed` and the same attempt goes on `running` on `b` |
| Planned handoff of each kind, with its outcome in the cut | the same test, and `launch::e2e_a_handoff_an_escaped_child_blocks_leaves_the_attempt_running_on_b` | `managed continue --pin a`: the first attempt ends `checkpointed`; in the cut the harness is `checkpointed` and the build and the test `interrupted`. A harness that left a child outside its group (`FAKE_CODEX_WAIT`) blocks the handoff: `handoff_blocked` in the facade's watch, no continuation, no handoff placement, one attempt, still `running` on `b`. An opaque task has no surface that registers one (the CLI takes `build` or `test`); its blocking rule is proven by the executor's unit tests only |
| Continue on the other node with the work package and the fidelity achieved | the same test | the handoff placement is `planned_handoff` and chooses `a`; the continuation names the first attempt, `achieved: new_native_session_with_package`, a 64-hex package digest; `a` records the harness, the build and the test of the new attempt, `b` holds nothing of either attempt, and the build's marker is in `a`'s own job root, restored from the cut `a` pulled from `b` |
| A secret value appears nowhere | the same test, and `secrets::e2e_a_secret_value_appears_in_no_journal_snapshot_package_receipt_or_log` | both daemons hold `FABRIC_SECRET_DEPLOY_KEY`; after the run the value is searched for in every file under the test's root: both state roots (journal, store, packages, run records, taken launches, the daemon logs), both homes (job roots and their set-aside copies), and both link captures. It is in none, except the fake harness's own dump of its environment (`codex-env`), which the test requires to hold `DEPLOY_KEY=<value>` on both nodes: the harness got it by name. The captures are checked to have seen the link in the clear: `a`'s holds the launch's secret name, the goal and the build's marker from the pulled chunk |
| Cancel of a live attempt | `placement::e2e_a_submit_runs_on_the_eligible_node_without_naming_it`, `managed::e2e_a_cancel_stops_a_live_attempt_and_records_the_tasks_interrupted` | a cancel from the other node stops the live attempt, `cancelled`, every task's receipt `interrupted` |

Counters for that run: daemons started 11 (2, 2, 2, 2, 1, 1, 1), link captures 1291 bytes on `a` and 2339 on `b`, `fabric` processes left 0. Mutation: with the pin fix reverted the launch test fails at the handoff's chosen node (`launch.rs:302`); with the root fix reverted it fails `not continued`, the harness receipt `blocked` naming the missing `work.fabric-restoring`.

## Blocked

A native-session continuation. `continuity::e2e_a_native_session_continues_on_a_node_with_the_same_layout` stays ignored: the launch restores the workspace from the cut and not the harness's own state, so `offered_fidelity` is asked with the layout never the same and no node offers `native_session` (docs/managed-jobs.md, "Launch path", owner B as built). The question for Efe: should M5 restore the harness's state on the target (the capturer already captures it through the adapter's suspend path) and offer `native_session` when the layout matches, or should M5 ship with `new_native_session_with_package` as the best fidelity and the native session move to M6 or later?

Not run here: a run on two hosts. Every test above is two processes on one host; a run with the second Mac needs its address paired with `fabric node add`.

## Gates

`gates/run-gates.sh` ran every gate once, serially, at the commit in `gates/commit.txt`; `gates/summary.txt` has each exit code, time and load, and each gate's log is beside it.
