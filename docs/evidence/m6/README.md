# M6 evidence: the partition proof on this base

Recorded on 2026-11-03 on `m6/loss-recovery` at the M6 implementation commits (ADR 0028, `docs/managed-jobs.md` "Unexpected-loss recovery (M6)" and its "As built (M6)"), built on the M4 head `5e38dfa`; M5 is not in this base. The host is an Apple M5 Pro (18 cores, 48 GiB), macOS arm64 (Darwin 25.6.0), rustc 1.97.1, with other work running at a load average of 5 to 7. Nothing here was timed. The Perf step measures these commits on its own.

What this is: two `fabricd` processes on one host, each with its own state root (`FABRIC_HOME`), paired over a real QUIC link that runs through a small userspace relay the test owns (`fabric-relay`, `crates/fabric/fixtures/relay.rs`, std only). Pausing the relay drops every datagram between the two in both directions and holds every TCP byte (the relay stops reading, so flow control holds the sender and the bytes arrive in order after resume) while the sockets stay open; resuming forwards again. This is a two-process partition on one host, not two hosts. No firewall rule and no system setting was touched; the relay is an ordinary process on two loopback ports, started and stopped by the test. Every process the runs started was gone when they ended (`pgrep -fl "fabricd|fabric-relay"` found none after each run).

The four runs in the table below were recorded on 2026-11-02 with the relay as it then was, which read and discarded TCP bytes while paused (its counter was `tcp_dropped`); the review of 2026-11-03 found that a TLS fallback stream could not resume after such a pause, and the relay now holds the bytes instead (`tcp_held` counts the polls a stream spent waiting). The two daemons speak QUIC, so the UDP counters of the runs stand; the relay's own test was rerun after the change (below).

## What ran, exactly

```
cd <worktree m6>
CARGO_TARGET_DIR=<scratch>/target cargo test -j 6 -p fabric --test e2e --locked -- relay:: partition:: --nocapture
CARGO_TARGET_DIR=<scratch>/target cargo test -j 6 -p fabric --test e2e --locked -- partition::two_daemons --nocapture   (three more times)
```

`scripts/m6-partition.sh` runs the same two tests and writes `runs/<date>/` with the host, the commit, the test log and the relay's counters. `runs/2026-11-02T220505Z/` holds the first run below (`meta.txt`, `run.log` scrubbed of the scratch and temporary paths) and the counters of all four (`counters.txt`).

| Run | Started (UTC) | Load before | Result | Relay counters, `[up, down]`, up is toward the authority |
|---|---|---|---|---|
| 1 (`relay::` and `partition::`) | 2026-11-02T22:05:05Z | 6.80 6.94 6.17 | 2 passed, 3 ignored, 38.42 s | `udp_forwarded [64, 58]`, `udp_dropped [26, 19]`, TCP 0 |
| 2 (`partition::two_daemons`) | 2026-11-02T22:06 | 6.31 6.75 6.15 | 1 passed, 38.90 s | `udp_forwarded [70, 66]`, `udp_dropped [26, 19]`, TCP 0 |
| 3 (`partition::two_daemons`) | 2026-11-02T22:08 | 4.88 6.32 6.02 | 1 passed, 37.88 s | `udp_forwarded [67, 58]`, `udp_dropped [24, 19]`, TCP 0 |
| 4 (`partition::two_daemons`) | 2026-11-02T22:09 | 5.58 6.34 6.04 | 1 passed, 37.73 s | `udp_forwarded [66, 59]`, `udp_dropped [26, 19]`, TCP 0 |

Four of four runs passed. What the two tests check:

- `relay::the_relay_forwards_pauses_and_counts`: against a UDP and TCP echo the test owns, the relay forwards one datagram and one TCP line each way; paused, it drops the datagram and holds the TCP line (the client's reads time out, the sockets stay open, `tcp_forwarded` does not move); after resume the same sockets carry traffic again and the held line arrives first, in order, then a new one; the counters say so: `udp_forwarded [2, 2]`, `udp_dropped [1, 0]`, `tcp_forwarded [18, 18]`, `tcp_held` above zero in both directions. (In the 2026-11-02 runs the relay dropped the TCP line instead and counted `tcp_dropped [5, 0]`.)
- `partition::two_daemons_talk_through_the_relay_and_lose_each_other_while_it_is_paused`: the runner daemon dials the authority daemon through the relay (datagrams forwarded in both directions), the runner moves a standalone job to the authority through it, the relay is paused and a baseline is taken once the counters stand still, a second move cannot complete while the authority still takes a `managed create` of its own, no datagram crosses the paused relay (`udp_forwarded` unchanged from that baseline while `udp_dropped` grows), and after resume the stopped move completes on a fresh dial. The TCP counters stay at zero because the two daemons speak QUIC; the TCP path is the TLS fallback and is covered by the relay's own test.

## The five scenarios, one by one

The step asked for five attempt-level scenarios on the two daemons. None of them can run on this base, for one reason, stated once: on the M4 base no `fabricd` ever grants an attempt. `Command::StartAttempt` is reached only from `fabricd`'s `authority::tests`; no CLI, client API or wire request issues it; no runner loop exists that would heartbeat, capture a checkpoint into the runner's store or append to a managed ledger over the link; and the authority never dials the runner for the verifier's `fetch_snapshot`. All of that is the M5 executor, placement and facade (ADR 0023 to 0027), built in the M5 worktree now; M6 is restacked on M5 at the PR step. The M6 code those scenarios exercise is on this branch and proved one layer down (next section); what is missing is the attempt the scenarios act on. The decision not to carry a second runner loop in M6 was selected by the implementation agent on 2026-11-02 under Efe's delegation of 2026-11-02, and is the question below.

| Scenario | Needs on this base and does not have | What is here instead |
|---|---|---|
| (1) relay paused, authority supersedes and regrants under `restart_from_acknowledged`, relay resumed, the old attempt's writes refused | an attempt granted to the runner node, a runner that heartbeats and reports `Captured`, the authority's fetch from the runner | the partition primitive above; `fabricd` `authority::tests::restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner` (in-process: silence, `AttemptUnavailable`, `GrantSuperseded`, `AttemptGranted` at the next generation, the old runner's heartbeat answered `Superseded`, its covered writes refused and recorded); the node's `ManagedJob` and `Heartbeat` handlers (`fabricd::service`) that run the fence before the body over the real wire |
| (2) `SIGSTOP` the old runner, regrant, `SIGCONT`, covered writes refused, one live attempt | a runner process and the process group of its attempt | the same in-process test, which asserts exactly one live attempt and that the `UNIQUE` on `(job, generation)` holds; the executor's stop of a superseded group is M5's `on_verdict` |
| (3) `SIGKILL` variant | the runner's local attempt marker and its restart path | nothing on this base; written as an ignored test with the reason |
| (4) `block` policy: the job waits and shows the state | an attempt that goes silent | `fabric-journal` `tests::recover_never_marks_lost_from_a_missed_heartbeat` and `fabricd` `authority::tests::presence_marks_overdue_on_the_authority_clock_and_writes_no_heartbeat`: the attempt is `unknown`, `unavailable_since_ms` is set, `RecoveryBlocked` is recorded once per silence, nothing is superseded |
| (5) captured but not acknowledged: restart refused with the reason | a runner that reports `Captured` | `fabric-core` `managed_rules::a_captured_checkpoint_restarts_nothing_until_acknowledged` and `managed_rules::a_start_after_the_first_attempt_resumes_only_from_an_acknowledged_checkpoint` (the two-step operator path is gated too); `fabricd` `recovery::tests::a_corrupt_chunk_is_never_acknowledged` (the pure `acknowledgement` decision: a fetch that failed `Corrupt` or `Unreachable`, or a cut that differs, acknowledges nothing; it alters no chunk and writes no event, since the fetch that would refuse one, `fetch_snapshot`, is driven by the M5 executor) |

The three ignored tests in `crates/fabric/tests/e2e/partition.rs` (`relay_paused_runner_alive`, `old_runner_sigstop_then_sigcont`, `old_runner_sigkill_then_restart`) are stubs: each body returns this reason and nothing else, so run with `--ignored` they fail and cannot be mistaken for passing, and nothing in them pauses the relay, stops or kills a process, or asserts an event. Scenarios (4) and (5) are the negative halves the specification asks of each of them and are finished in the same step as the three.

Question for Efe: should the five scenarios be finished at the restack on top of M5's executor and placement (the recommendation: one runner loop in the codebase, no duplicate, the proof written once against the real attempt), or should M6 carry a minimal runner loop of its own in `fabricd` now and take the merge conflict with M5? Until answered, M6 is blocked on this item and on nothing else.

## What proves what, without the runner

Each test fails without the M6 code it names.

| Claim | Test |
|---|---|
| A write under a superseded or stale generation is refused on every covered path and recorded | `fabric-journal` `tests::a_refused_covered_write_changes_nothing_else`; `fabric-core` `managed_rules::fence_only_the_live_attempt_from_its_node_is_live` |
| A captured but unacknowledged checkpoint is the base of no restart | `fabric-core` `managed_rules::a_captured_checkpoint_restarts_nothing_until_acknowledged`; `fabric-journal` `tests::acknowledge_writes_the_event_the_evidence_and_the_column` |
| Heartbeat loss alone never marks the old attempt stopped; under `block` the job waits with the full state visible | `fabricd` `authority::tests::presence_marks_overdue_on_the_authority_clock_and_writes_no_heartbeat`; `fabric-journal` `tests::recover_never_marks_lost_from_a_missed_heartbeat` |
| `restart_from_acknowledged` places a new attempt from the acknowledged checkpoint once the attempt has been `unknown` for the threshold, the old runner's heartbeat is answered superseded and its later writes are refused | `fabricd` `authority::tests::restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner` |
| The authority's own `Recover` replays under its derived key after a restart | `fabric-journal` `tests::an_automatic_recover_replays_under_its_derived_key`, and the restart at the end of the `fabricd` test above |
| A fetch that failed, or a cut that differs, is never acknowledged (the pure decision; the fetch's own chunk checks are ADR 0014's and are driven at the restack) | `fabricd` `recovery::tests::a_corrupt_chunk_is_never_acknowledged` |
| The write a fence allows runs inside the authority's turn; a local write is checked against the granted node; with no journal every covered path says `UnknownJob`; a refusal is recorded once per attempt, path and verdict and never for a made-up attempt | `fabricd` `authority::tests::a_fenced_write_runs_inside_the_authority_s_turn`, `a_write_made_on_this_node_is_checked_against_the_granted_node`, `with_no_journal_a_covered_write_is_unknown_job_and_starts_no_thread`; `fabric-journal` `tests::a_repeated_refusal_is_recorded_once_and_a_made_up_attempt_never` |
| An attempt found `unknown` at a restart comes back on its heartbeat and still dwells and recovers; a covered write from the granted node brings an `unknown` attempt back; silence is measured on the monotonic clock | `fabricd` `authority::tests::an_attempt_unknown_at_a_restart_comes_back_on_its_heartbeat_and_still_recovers`, `a_covered_write_from_the_granted_node_brings_an_unknown_attempt_back`, `authority::presence::tests` |
| The automatic restart waits the dwell, runs only where a node runs managed attempts, and otherwise records `NoEligibleNode` and keeps the grant; an attempt is judged by its own task | `fabricd` `authority::tests::restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner` (with the executor flag), `without_an_executor_the_automatic_restart_is_blocked_and_the_grant_kept`; `fabric-core` `managed_rules::an_attempt_is_judged_by_its_own_task_not_the_primary` |
| The previous journal shape still opens, the pre-M6 draft is refused unwritten | `fabric-journal` `tests/fixtures.rs` |
| The wire carries the grant and never an epoch for a managed request; heartbeats are bounded | `fabric-net` `wire::tests::a_managed_request_carries_a_grant_and_never_an_epoch` |
| Two daemons through the relay lose each other while it is paused and find each other after | e2e `partition::two_daemons_talk_through_the_relay_and_lose_each_other_while_it_is_paused`, `relay::the_relay_forwards_pauses_and_counts` |

Termination coverage: the executor's stop of a superseded attempt's process group is the M5 executor's, and the `SIGSTOP`, `SIGCONT` and `SIGKILL` scenarios assert it; the harness's own files inside the workspace are not covered by the fence and nothing in Fabric stops a superseded runner that is alive from editing them until its executor does (ADR 0028).

## Gates

The full gate list ran once at the end of this step on the working tree of the final commit; `gates.txt` holds each command and its last line.
