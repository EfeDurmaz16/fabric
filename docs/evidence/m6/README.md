# M6 evidence: loss recovery on the merged tree

Recorded on 2026-11-09 on `m6/loss-recovery` at `281635c`, after M6 merged `main` with the M5 launch path (`ad744de`, merge `db7f63f`). The host is an Apple Silicon Mac, macOS arm64 (Darwin 25.6.0), with other work running (load before the run: 7.30 7.74 7.57, in `meta.txt`). Nothing here was timed for performance; perf runs in CI. Every selection behind M5 and M6 was selected by the implementation agent under Efe's delegation of 2026-11-02; none was approved by Efe.

What this is, plainly: two `fabricd` processes on one host, each with its own state root (`FABRIC_HOME`), paired over a real QUIC link that runs through a small userspace relay the test owns (`fabric-relay`, `crates/fabric/fixtures/relay.rs`, std only). Pausing the relay drops every datagram between the two in both directions and holds every TCP byte while the sockets stay open; resuming forwards again. This is a two-process partition on one host, not two hosts. No firewall rule and no system setting was touched. No `fabricd` or `fabric-relay` process was left after the run (`pgrep -x`, in `meta.txt`).

## How to run it

```
cargo build -p fabric -p fabricd --locked
sh scripts/m6-partition.sh
```

The script runs `cargo test -p fabric --test e2e --locked -- relay:: partition:: --test-threads 1 --nocapture` and writes `meta.txt`, `run.log` (paths scrubbed) and `counters.txt` under `runs/<date>/`. The five scenarios also run in the ordinary test suite; none is ignored.

## The run: `runs/20261108T231706Z/`

7 passed, 0 failed, 188.9 s.

The set-up every scenario shares: the authority creates a managed job (`fabric managed create --empty --harness codex --secret M6_SITE --recovery <policy>`). Only the runner holds the secret `M6_SITE` at the first placement, so the M5 placement chooses the runner and excludes the authority by `secret`; the authority's dispatch launches the attempt there over the relayed session, and the runner's loop starts the fake Codex held with `FAKE_CODEX_HOLD` and heartbeats every 2 s. `fabric managed checkpoint` cuts it on the runner; the fenced `Checkpointed` report records the cut as the job's captured checkpoint, and the authority's verifier fetches the snapshot's chain over the relay, checks it by content and commits `checkpoint_acknowledged`. Then the secret is provisioned on the authority, so a failover has somewhere to go other than the lost node.

| Scenario | Test | What it shows |
|---|---|---|
| (1) relay paused, runner alive | `partition::relay_paused_runner_alive` | `attempt_unavailable`, then `grant_superseded` and a `failover` placement that excludes the runner and chooses the authority, the failover running at generation 2, 26.3 s after the pause. The runner's harness group still runs during the pause. Resumed: the runner's heartbeat is answered `superseded`, it logs that the grant ended and stops the group. The runner's covered writes under the old grant (its exit as a report, and a ledger append), sent under its own node key over the relay, are refused `Superseded` and `GrantRefused` and recorded `covered_write_refused`. Exactly one attempt may run. Relay while paused: `udp_forwarded [32, 28]`, `udp_dropped [21, 19]`, TCP 0. |
| (2) old runner stopped and woken | `partition::old_runner_sigstop_then_sigcont` | `SIGSTOP` to the runner's daemon and its harness group; the authority supersedes and places the failover on itself; `SIGCONT`: the woken runner is told `superseded`, stops the group, and its writes are refused and recorded. One live attempt. |
| (3) old runner killed and restarted | `partition::old_runner_sigkill_then_restart` | `SIGKILL` to the runner's daemon (its harness survives as an orphan group); the failover runs on the authority; the runner's daemon starts again in the same state root, stops the orphan group, presents the grant it had taken and is answered `superseded`. Its marker (`runs/taken/<attempt>.json`) says `GrantEnded { verdict: "superseded" }`, the journal says `superseded`, nothing of the attempt is resumed, one live attempt. |
| (4) block with a held harness | `partition::block_policy_waits_until_an_operator_recovers` | Relay paused: `unknown`, and through the window in which `restart_from_acknowledged` acts (14 s past the unavailability) nothing moves: one attempt, no `grant_superseded`, no `recovery_blocked`, the state visible in the inspection and the events. `fabric managed recover` then supersedes it and the failover runs on the authority; resumed, the runner stops. |
| (5) captured, not acknowledged | `partition::captured_but_not_acknowledged_is_never_restarted_and_says_why` | The authority's verifier is held before its fetch (`FABRIC_FAULT=before_acknowledge:hold`), so the cut is `checkpoint_captured` and never acknowledged. Relay paused: under `restart_from_acknowledged` the restart is refused and recorded `recovery_blocked { no_acknowledged_checkpoint }`, the attempt stays `unknown`; `fabric managed recover` is refused (no acknowledged checkpoint); `fabric managed supersede` takes the grant and grants nothing. |

Why the old runner's writes are sent by the test: the runner itself never writes after it learns its grant ended. Its reports wait for a heartbeat answer when the link is down, and the first answer after the resume or the wake is the verdict, so it stops and sends nothing (the log line and the stopped group show it). The writes it would have sent are then sent under its node key, over QUIC through the relay, with its old grant, which is what a runner that has not heard yet would do; the authority's fence refuses and records them. The harness is held with `FAKE_CODEX_HOLD`, one process, rather than `FAKE_CODEX_WAIT`, whose escaped child blocks every cut by design (ADR 0023).

Counters the code keeps: the runner's `stale_obeyed` (a verdict or refusal that ended a grant, obeyed by a stop) and `presented` (grants presented after a restart), and the authority's commit counter, which a heartbeat or a tick moves only when it wrote. No load figure is claimed beyond the load averages in `meta.txt`.

## Red first

`runs/20261108T231706Z/red-first.txt`, from one scratch copy of `281635c` built into the same target and deleted after:

- With the fence forced to `Live`, scenarios (1), (2), (3) and (4) fail: the runner is never told its grant ended, and in (3) the restarted runner's marker does not say superseded.
- With the acknowledgement check removed (a captured checkpoint taken as a restart point), scenario (5) fails: no `recovery_blocked` is recorded, the authority restarts from the captured cut.
- Unit tests, same copy: `every_heartbeat_verdict_that_ends_the_grant_stops_the_attempt` and `every_report_refusal_that_ends_the_grant_stops_the_attempt` fail when only `Stale` ends a grant, `a_failover_excludes_the_unavailable_node_whatever_it_offers` fails without the exclusion, and `a_restarted_runner_presents_its_taken_grants_and_resumes_none` fails when nothing is presented.

## What this does not prove

- Two hosts. Both processes run on one Mac; the relay stands in for the network.
- A managed ledger. No node holds one, so the managed ledger append is refused after the fence even for a live grant; what the scenarios prove for it is the refusal of the old grant. A bundle becomes the job's checkpoint only through the fenced cut report, and no runner writes a facade outcome (docs/managed-jobs.md, "As built on the merged tree (M6)").
- The facade's own `recover` and `supersede` messages. The operator commands are in the CLI, the client API and the TS SDK; facade protocol v1 does not carry them yet.

## Gates

`gates.txt`: one full gate run, at the commit it names.

## Earlier runs

`runs/2026-11-02T220505Z/`, `runs/20261108T134838Z/` and `runs/20261108T164203Z/` are the runs on the M4 base and on the M5 base before the launch path, when four of the five scenarios were blocked. They are kept as they were.
