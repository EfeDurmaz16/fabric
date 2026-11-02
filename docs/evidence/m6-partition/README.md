# M6 partition proof

What this is: two `fabricd` processes on one host, each with its own state root, paired over a real QUIC link that runs through a small userspace relay the test owns (`fabric-relay`, `crates/fabric/fixtures/relay.rs`). Pausing the relay drops every datagram and byte between the two in both directions while the sockets stay open; resuming forwards again. This is a two-process partition on one host, not two hosts. No firewall rule and no system setting was touched; the relay is an ordinary process on two loopback ports.

How to run it: `scripts/m6-partition.sh`. It writes `runs/<date>/` with the host, the commit, the test log and the relay's counters. Every number below comes from such a run on this Mac; the authority-side behaviour it depends on is proved by the unit and integration tests listed under "What proves what".

## What runs on this base (2026-11-03)

- `relay::the_relay_forwards_pauses_and_counts`: against a UDP and TCP echo the test owns, the relay forwards one datagram and one TCP line each way, drops both while paused (the client's reads time out, the sockets stay open), forwards again on the same sockets after resume, and its counters say so: `udp_forwarded [2, 2]`, `udp_dropped [1, 0]`, `tcp_forwarded [12, 12]`, `tcp_dropped [5, 0]`.
- `partition::two_daemons_talk_through_the_relay_and_lose_each_other_while_it_is_paused`: the runner daemon dials the authority daemon through the relay (datagrams forwarded in both directions), the runner moves a standalone job to the authority through it, the relay is paused, a second move cannot complete while the authority still takes a `managed create` of its own, and after resume the stopped move completes on a fresh dial. On the run of 2026-11-03 the relay counted `udp_forwarded [65, 58]`, `udp_dropped [26, 20]` (up is toward the authority); every later run writes its own numbers to `runs/<date>/counters.txt`.

## What is blocked, and the question for Efe

The three attempt-level scenarios of the specification (`relay_paused_runner_alive`, `old_runner_sigstop_then_sigcont`, `old_runner_sigkill_then_restart`) need an attempt that runs on the runner node, heartbeats to the authority, captures a checkpoint into the runner's store and appends to the job's ledger over the link. That is the M5 executor and facade (ADR 0023, ADR 0027), which are built in the M5 worktree and are not in this base; on this base a managed job has no `StartAttempt` on the wire and no runner loop. The scenarios are written against the harness in `crates/fabric/tests/e2e/partition.rs` and marked ignored with this reason.

Question for Efe: should the three scenarios be finished in the restack step on top of M5's executor (the recommendation: one runner loop, no duplicate), or should M6 carry a minimal runner loop of its own in `fabricd` now and take the merge conflict with M5?

## What proves what, without the runner

The partition behaviour the scenarios assert is covered one layer down, in tests that fail without the M6 code:

| Claim | Test |
|---|---|
| A write under a superseded or stale generation is refused on every covered path and recorded | `fabric-journal` `tests::a_refused_covered_write_changes_nothing_else`; `fabric-core` `managed_rules::fence_only_the_live_attempt_from_its_node_is_live` |
| A captured but unacknowledged checkpoint is the base of no restart | `fabric-core` `managed_rules::a_captured_checkpoint_restarts_nothing_until_acknowledged`; `fabric-journal` `tests::acknowledge_writes_the_event_the_evidence_and_the_column` |
| Heartbeat loss alone never marks the old attempt stopped; under `block` the job waits with the full state visible | `fabricd` `authority::tests::presence_marks_overdue_on_the_authority_clock_and_writes_no_heartbeat`; `fabric-journal` `tests::recover_never_marks_lost_from_a_missed_heartbeat` |
| `restart_from_acknowledged` places a new attempt from the acknowledged checkpoint, the old runner's heartbeat is answered superseded and its later writes are refused | `fabricd` `authority::tests::restart_from_acknowledged_recovers_onto_this_node_and_fences_the_old_runner` |
| The authority's own `Recover` replays under its derived key after a restart | `fabric-journal` `tests::an_automatic_recover_replays_under_its_derived_key`, and the restart at the end of the `fabricd` test above |
| A corrupt chunk or a cut that differs is never acknowledged | `fabricd` `recovery::tests::a_corrupt_chunk_is_never_acknowledged` |
| The previous journal shape still opens, the pre-M6 draft is refused unwritten | `fabric-journal` `tests/fixtures.rs` |
| The wire carries the grant and never an epoch for a managed request; heartbeats are bounded | `fabric-net` `wire::tests::a_managed_request_carries_a_grant_and_never_an_epoch` |

Termination coverage: the executor's stop of a superseded attempt's process group is the M5 executor's, and the `SIGSTOP`, `SIGCONT` and `SIGKILL` scenarios assert it; the harness's own files inside the workspace are not covered by the fence and nothing in Fabric stops a superseded runner that is alive from editing them until its executor does (ADR 0028).
