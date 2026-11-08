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

## Blocked

The third acceptance row, "a planned handoff with child-task outcomes and actual fidelity", and the work package carried to the second node, are not run end to end, because a granted attempt is never launched. What is missing in the code, as found on 2026-11-08:

1. No node runs a grant. fabric-net protocol 4 carries `Advertise`/`Advert` and the facade, and no message that asks the chosen node to start a granted attempt (with its generation and initial checkpoint) or carries the attempt's observations (`Started`, `Exited`, `Receipts`, `Checkpointed`, `HandoffBlocked`, `Continued`) back to the authority. The authority's `observe` has callers only in its tests and in the placement dispatch.
2. A build or test task names nothing to run. `RegisterTask` carries a label, a restart policy and a class; the executor spawns a `std::process::Command` and the model has no field for it, and no surface (client API, facade, harness) registers a child.
3. The node has no `Capturer` (the executor's harness checkpoint and workspace capture): only the executor tests' fake implements it. No `WorkPackage` is built or sent by `fabricd`.

The executor's per-kind outcome rules (an interrupted test or build is `interrupted`, a harness task checkpoints or blocks, an opaque task blocks, escaped children and uncontrolled writers block) and the domain's continue transitions are proven by the unit and crash tests that owners A and B recorded in docs/managed-jobs.md; none of them runs across two daemons.

The question for Efe: M5's acceptance needs a remote launch path. Should M5 add it as (a) one new node-link request in protocol 4, `Launch { job, attempt, generation, task, initial_checkpoint, package }`, answered at once, with the target reporting observations to the authority by a second request `Report { job, observation }` fenced by generation, and the snapshot fetched with the existing pull transfer, plus a `command` field on `RegisterTask` for build and test children (codec format 2, not shipped); or (b) should M5 ship with placement and the facade only, and the launch, the cut and the carry move to a milestone of their own before M6, whose old-runner wake needs a remote runner too?

Answered 2026-11-08, under Efe's delegation of 2026-11-02 and not approved by Efe: option (a). The selection is recorded in the decisions file as "M5 launch path, selected 2026-11-08" and the contract is docs/managed-jobs.md, "Launch path (M5, as built)": `Launch`, `Instruct`, `Report` and `Heartbeat` on protocol 4, `RegisterTask` with a program for build and test tasks, the runner loop on the node, the work package built by the authority at the cut, and the dispatch of `Checkpoint`, `Continue`, `CancelJob` and `CancelTask` to the live attempt's node. #38 merges only after the launch path is proved end to end and reviewed; M6's five scenarios run against it before M6 merges.

## Gates

`gates/run-gates.sh` ran every gate once, serially, at the commit in `gates/commit.txt`; `gates/summary.txt` has each exit code, time and load, and each gate's log is beside it.
