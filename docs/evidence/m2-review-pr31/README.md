# Review of PR #31: five findings on the integrated M2 head

Five findings a review of `31577c4` raised, each checked against the code,
reproduced with a test that failed before its fix, and fixed on
`m2/durable-job-foundations` on 2026-10-29.

| Finding | Fix | Regression test, and what it read before the fix |
|---|---|---|
| A restore of one job could evict another job's bundle cache between that job's `reuse` and its copies, so its resume, move or pull failed with an IO error on a cache path | `0c4f287`: `place_cached` treats a cached file gone at its copy as a miss: the files of its link already copied come out of the repository, the cache drops that link and the ones after, and the restore unpacks them | `restore::tests::a_cache_another_restore_evicts_after_reuse_costs_a_replay_not_the_restore`: failed with `NotFound` on `pack-0.idx` |
| A second fork repair at the same fork point wrote over the ledger the first repair kept, the only one that published the first repair's pinned snapshot | `346f932`: the kept file is `ledger.forked-<fork point>-<head>`, and `Job::adopt` refuses to write over a kept file that holds other bytes | `ops::tests::a_second_repair_at_one_fork_point_keeps_the_first_kept_ledger`: both repairs returned one path; `job::tests::adopt_never_replaces_another_kept_ledger` |
| M2-15's per-peer request bound was bypassed: the transports read each waiting request off the wire before the room gate | `cdb1c43`, `f8742a2`: on TCP a stream's opener starts with 16 KiB of credit and gets more only for what the acceptor's reader asks for; on QUIC the connection receive window is about 6 MB instead of unbounded | `rpc::tests::request_bytes_read_off_the_wire_for_a_peer_are_bounded`: 50,339,502 bytes before, 17,305,038 after; `quic::tests::a_peer_can_leave_at_most_the_connection_window_unread`: 12,888,213 bytes before, at most 6.7 MB after; `mux::tests::a_stream_the_peer_opened_gets_credit_only_for_what_its_reader_asks`, `pipe::tests::credit_on_demand_is_what_a_read_asked_for_and_no_more` |
| `check-budget.sh` compared no process count, and an `anchor` line let every baseline rise unchecked | `0381081`, `13f29d1`: counts only go down against the base, with the counted programs named in `budget.toml`; a baseline raised under the anchor needs at least three CI runs of the anchor's code, read with `gh`, whose logs give exactly that baseline | `scripts/test-check-budget.sh`, 60 cases; the review's two cases (a resume's git from [9, 10] to [30, 31], cold start given git, and the Linux codex resume baseline from 446 to 2000 under an anchor line) passed the old script and fail now |
| A headless ACP turn gathered every `agent_message_chunk` into one answer with no bound | `fc73523`: the driver keeps the first `MAX_ANSWER` bytes, 16 MiB, cut at a character, and counts the rest in `TurnReport::answer_left_out`, which the CLI names | `acp::tests::a_drained_turn_keeps_at_most_max_answer_bytes_of_answer`: held 25,165,824 bytes of 24 chunks before; `an_answer_is_cut_at_a_character_and_stays_a_prefix` |

## Gates

Every gate passed on `f8742a2`, the last commit that changes code
(`gates/summary-f8742a2.txt`): fmt, clippy of the workspace and of each
harness feature, `cargo test --workspace --locked` (1149 passed, 0 failed,
10 ignored), the end-to-end test (70 passed, 5 ignored), rustdoc with `-D
warnings`, `cargo deny check`, the hygiene, fixture and budget scripts and
their tests against `origin/main`, shellcheck, the budget in count mode, and
the TypeScript SDK (62 tests against a node built from the tree).

## Performance

`scripts/bench-ab.sh --pairs 20` in the test profile, 20 timed pairs in
alternating order and 3 counted runs, on an 18-core Apple silicon Mac under
a load average of 7 to 14 from other work.

Against the anchor `176622e` (`ab-vs-anchor/`): no op is slower. Every op
is faster with its 95% interval below 1 but `cli version`, 1.048 (0.974 to
1.129), whose interval holds 1 and whose code this round did not touch; it
read 1.015 on the M2 head as well. The load-insensitive counters are equal
or lower on every op: process starts, full flushes, journal and ledger
bytes, allocations. Daemon physical footprint is 1.002 (0.977 to 1.018) and
0.763 of the anchor's after the timed runs. Daemon resident size in this
profile is 1.047 and 1.026 of the anchor's, as on the M2 head before this
round: it holds the clean code pages of a debug binary 8% larger, and the
release profile's is below the anchor's (`docs/evidence/perf-ab`).

Against the M2 head `31577c4`, to see what this round alone changed
(`ab-vs-m2-head/`, `ab-vs-m2-head-2/`, and `ab-swap-m2-head-over-fixes/`
with the builds on the other sides), pooled over the 60 pairs: 25 ops have
medians from 0.978 to 1.019, every interval holding 1 but claude pull's,
which is faster, 0.990 (0.977 to 0.999). Daemon resident size and footprint
are 0.967 to 1.003 of the M2 head's. `acp new` reads 1.018 (1.010 to 1.042).
Its load-insensitive counters are the same in both builds: one `git
clone`, 3 full flushes, 4 barriers, the same bytes, 37,906 allocations, and
less CLI CPU time. It is the first op of each run, and the gap comes from
the runs where it follows its own build's daemon restart at once: the build
that goes first is slower then, whichever it is, and this one by about 2%
more. No code this round changed runs on that path: a job is made by a
clone and ledger writes. A build of the M2 head with only the
QUIC window change (`ab-quic-window-only/`) reads 1.004 (0.984 to 1.035).
The anchor is the bar, and against it `acp new` is 0.488 (0.471 to 0.533).

## The release build against the anchor

On 2026-10-29 Efe ruled that the performance rule gates the release build
only; the test profile is tracked, not gated. `ab-vs-anchor-release/` is
the gated run: `scripts/bench-ab.sh --release --pairs 20`, the working
tree at `8a3b1f9` (the code of `f8742a2`) against `176622e`, 20 timed
pairs in alternating order and 3 counted runs, under a load average of 8
to 10 from other work, which reaches both builds of a pair alike.

- Time: no op is slower. 24 of 26 are faster with the interval below 1;
  Claude launch 0.968 (0.938 to 1.011) and `cli version` 0.953 (0.849 to
  1.104) hold 1, both with medians below 1. A new job 0.43 to 0.46, a
  resume 0.39 to 0.57, a pull 0.51 to 0.61, a move 0.62 to 0.63, a
  restart 0.142 (0.127 to 0.184).
- Memory: daemon resident size 0.874 and 0.887 after the timed runs,
  0.892 and 0.889 after the counted ops; physical footprint 0.928 and
  0.892 timed, 0.964 and 0.953 counted. Every CLI's peak resident size is
  below the anchor's, `cli version` 3168 to 2992 KiB and `acp launch` 6912
  to 5968. The binary is 15443120 bytes against 19773920.
- Counters that do not depend on load: processes started and full
  flushes are equal or fewer on every op, and allocations fewer on every
  op but `cli version`, 189 to 244 for the larger command tree. Plain `fsync`s rise on suspend,
  resume and pull (resume 51 to 61) where they replace full flushes (24 to
  13), the split of ADR 0002, amendment item 3. Bytes read from the store
  rise on resume and pull (resume 16142 to 21702): the node bundle cache
  reads each cached pack to check its digest before it uses it, in the
  fabric process, where the anchor ran `git bundle unbundle` processes the
  counters do not see (8 fewer per resume). Suspend and restart read 2 to
  3% more of the store; they are not traced further, and both are faster
  (0.68 to 0.79, and 0.142).

## The durability decisions of 2026-10-29

Efe approved items 3, 12 and 17 of `docs/evidence/perf-ab/README.md` on
2026-10-29; ADR 0002 and ADR 0016 had said 2026-10-27, written before he
decided, and now give the right date (`fd67bba`). Two of them let a power
loss leave state that it could not leave before, so `5135ddc` adds a test
that writes that state back over a stopped node's files and starts a node
over it:

- `ops::tests::a_manifest_a_power_loss_left_behind_is_written_again_at_start`
  (item 12). Red when `Job::open` does not write a manifest behind its
  ledger again.
- `ops::tests::a_gone_harness_record_a_power_loss_brings_back_blocks_nothing`
  (item 17). Red when the registration, the status or the suspend treats
  the record of a gone harness as running, each mutation on its own.
