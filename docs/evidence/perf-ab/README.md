# A/B of main and the M2 branch

The performance rule of 27 Oct 2026 (CONTRIBUTING.md, Performance rule):
no hot path may be slower, or use more memory, than the anchor, `main` at
`176622e`, and the aim is to beat it. This directory holds the first runs
of `scripts/bench-ab.sh`, which compares the anchor with a candidate on the
same fixture, op by op, in the same session.

| Directory | Profile | Pairs | Counted runs | Candidate |
|---|---|---:|---:|---|
| `m2-head/` | test (the e2e and budget profile) | 25 | 3 | `a25faff`, whose product code is `f381d42`, the M2 head; the commits after it change tests and scripts only |
| `m2-head-release/` | release | 15 | 2 | the same tree; its `1 changed paths` is the untracked `m2-head/` |

Both ran on 2026-10-27 on an Apple M5 Pro (18 cores, macOS arm64, rustc
1.97.1) while another workflow built and tested beside it: load average 7
to 11 (`machine.txt`). The pairs give both builds the same load, so the
ratios hold; the absolute times are higher than a quiet machine gives.

Each directory has `machine.txt` (commits, sizes, load), `run.log`,
`samples.tsv` (every timed sample, counter and resident size), `spawns.tsv`
(every command line a `fabric` process started in the counted runs, with
the fixture's directory as `<root>`) and `report.md`, which
`scripts/bench-ab.sh --analyze <dir>` writes again from the samples.

## Gates

Every gate of CONTRIBUTING passed on `491b931`, the commit that adds these
runs (`gates/summary.txt`): fmt, clippy (workspace and each harness
feature), `cargo test --workspace --locked` with 1072 passed, 0 failed and
8 ignored (the A/B bench and its re-analysis are the two new ignored ones;
its three unit tests run), doc, deny, hygiene, the budget file, the
fixtures and their script tests, shellcheck of `scripts/bench-ab.sh`, and
the TypeScript SDK.

## Method

`ab::paired` (`crates/fabric/tests/e2e/ab.rs`) starts one fixture per
build: two nodes over QUIC, fake probed programs that answer after 50 ms,
git the machine's own. The CLI and both daemons of a fixture run that
build. One run is, per harness, `new`, `launch` (first headless turn) and
`approve`; `ledger append` on a job whose ledger grows a record a run;
`cli version` and `cli ls`; per harness `suspend`, `move`, `resume` and
`pull`; `cold start` and `restart`. The builds take turns op by op, and
the one that goes first alternates from run to run. Each op's two times in
one run are a pair; the report gives the median of the pairs' ratios,
candidate over anchor, with a 95% percentile bootstrap interval (10,000
resamples, fixed seed). An interval above 1 is a regression; a median
above 1 whose interval holds 1 is profiled.

The counted phase, on fixtures of its own, loads
`scripts/perf-ab/counters.c` into every `fabric` process with
`DYLD_INSERT_LIBRARIES`. Each process keeps a page of counters that the
test reads before and after each op: processes started by command, file
flushes by kind (`F_FULLFSYNC`, `F_BARRIERFSYNC`, `fsync`) and by class of
file, opens, renames, unlinks, allocations, bytes and calls of reads and
writes by class of file (journal, ledger, other job files, store, other
state, outside the state directory, pipes and sockets), and each CLI's
peak resident size and CPU time. None of these depend on load.

Two things grow over a run and are the same for both builds of a pair: the
bundle chain of each cycled job (two suspends a run, until the capture
starts a new full bundle every 16 links), so `resume` and `pull` grow from
about 520 ms to 1200 ms and fall back every eight runs; and the node's job
count (three new jobs a run), so `restart` grows from 166 ms to 789 ms.

## Results

| op | test: anchor / candidate ms | ratio [95%] | release: anchor / candidate ms | ratio [95%] |
|---|---:|---|---:|---|
| acp new | 155.9 / 154.2 | 1.003 [0.984-1.009] | 136.4 / 136.9 | 1.001 [0.975-1.029] |
| acp launch | 71.1 / 66.9 | 0.939 [0.903-0.975] faster | 72.2 / 64.6 | 0.896 [0.877-0.943] faster |
| acp approve | 16.1 / 15.9 | 1.001 [0.975-1.046] | 15.0 / 15.1 | 1.009 [0.943-1.057] |
| claude new | 135.2 / 133.2 | 0.993 [0.986-1.018] | 133.1 / 134.0 | 1.011 [0.987-1.035] |
| claude launch | 90.1 / 109.0 | 1.212 [1.202-1.229] **slower** | 87.0 / 106.5 | 1.200 [1.190-1.236] **slower** |
| claude approve | 16.0 / 16.1 | 1.008 [0.970-1.055] | 15.2 / 15.1 | 0.997 [0.936-1.038] |
| codex new | 133.0 / 133.0 | 0.986 [0.974-1.007] | 135.1 / 136.3 | 1.006 [0.991-1.045] |
| codex launch | 102.8 / 121.2 | 1.179 [1.163-1.207] **slower** | 101.9 / 118.2 | 1.184 [1.142-1.222] **slower** |
| codex approve | 16.1 / 16.2 | 1.014 [0.956-1.042] | 15.0 / 15.9 | 1.052 [0.997-1.079] |
| ledger append | 16.0 / 16.1 | 0.997 [0.971-1.017] | 15.1 / 15.0 | 0.978 [0.936-1.057] |
| cli version | 3.0 / 3.2 | 1.008 [0.927-1.076] | 2.9 / 2.9 | 0.984 [0.864-1.116] |
| cli ls | 5.5 / 5.7 | 1.027 [0.998-1.042] | 3.5 / 3.6 | 1.025 [1.006-1.042] **slower** |
| acp suspend | 389.0 / 391.9 | 0.988 [0.967-1.014] | 398.8 / 393.9 | 0.957 [0.926-1.010] |
| acp move | 163.1 / 162.8 | 0.988 [0.969-1.009] | 151.0 / 151.2 | 1.029 [0.942-1.055] |
| acp resume | 665.1 / 693.9 | 0.997 [0.980-1.042] | 655.1 / 717.0 | 1.009 [0.969-1.068] |
| acp pull | 1154.9 / 1184.8 | 1.015 [0.981-1.023] | 1279.0 / 1198.8 | 1.005 [0.978-1.014] |
| claude suspend | 455.8 / 461.9 | 1.017 [0.997-1.036] | 440.4 / 443.2 | 1.000 [0.974-1.058] |
| claude move | 158.0 / 160.0 | 1.010 [0.984-1.022] | 146.9 / 144.8 | 1.003 [0.933-1.033] |
| claude resume | 848.8 / 883.8 | 1.040 [1.020-1.065] **slower** | 873.0 / 835.6 | 0.992 [0.962-1.032] |
| claude pull | 1441.8 / 1465.8 | 1.021 [0.976-1.060] | 1446.7 / 1461.7 | 1.035 [0.980-1.068] |
| codex suspend | 683.7 / 677.0 | 0.990 [0.976-1.026] | 673.0 / 661.0 | 0.996 [0.955-1.006] |
| codex move | 159.6 / 161.2 | 1.004 [1.001-1.021] **slower** | 148.8 / 146.1 | 1.000 [0.953-1.022] |
| codex resume | 1070.1 / 1086.0 | 1.025 [1.007-1.042] **slower** | 1139.1 / 1131.9 | 1.021 [0.998-1.027] |
| codex pull | 1919.7 / 1958.9 | 1.012 [0.999-1.054] | 1907.9 / 1843.7 | 1.009 [0.975-1.029] |
| cold start | 58.1 / 58.6 | 1.008 [0.986-1.028] | 53.8 / 52.0 | 0.983 [0.949-0.994] faster |
| restart | 484.6 / 484.8 | 0.994 [0.969-1.004] | 361.2 / 347.2 | 0.987 [0.959-1.010] |

Memory, resident size in KiB after each timed run, median of the pairs'
ratios:

| | test profile | release |
|---|---|---|
| daemon a | 20128 -> 20624, 1.024 [1.022-1.027] **larger** | 11280 -> 11264, 0.997 [0.990-1.000] |
| daemon b | 30080 -> 30976, 1.038 [1.025-1.043] **larger** | 15776 -> 15968, 1.013 [1.010-1.023] **larger** |
| CLI peak, ops that ask the node (counted) | +300 to +1000 | +16 to +100 |
| binary bytes | 73350296 -> 76676472 | 19773920 -> 20663776 |

The earlier 1.12 to 1.14 of acp suspend and resume, approve and ledger
append (`docs/evidence/m2-integration/perf.md`) do not hold in pairs: all
four have intervals that hold 1, and the counters of approve and ledger
append are equal to the anchor's (3 full flushes, 98 ledger bytes).

## Causes

From the counters and from timelines of every open, flush, rename and
process start with timestamps, taken with a variant of the counter library
that was not committed.

- **Headless launch (Claude 1.20 to 1.21, Codex 1.18).** The counters are
  equal to the anchor's (15 and 17 full flushes, 4 processes); only the
  order changed. The anchor started the harness and registered it beside
  it. The candidate holds it (R2) until `harness_recorded`, so three
  steps that overlapped now run before the harness: the `/bin/sh`
  trampoline's start and the 2 ms polling for its stop (the node got the
  registration 7.4 ms after the spawn, against 0.7 ms on the anchor;
  `/bin/sh` on macOS is a shim that starts bash), two `ps` runs one after
  the other (`-o pgid=`, then `-o lstart=`, about 2 ms each), and the
  harness record's atomic write with two `F_FULLFSYNC` (about 6 ms). In one
  timeline the harness started 16 ms later than on the anchor.
- **Resume and pull (1.02 to 1.04 in the test profile).** The launch above
  at their end, and one more `git symbolic-ref` (M2-06) per restore.
- **`cli ls` (release 1.025).** The CLI makes 16 more allocations; the node
  reads each job's `forked` file from disk for every listing, as the anchor
  did, and the fixture holds 60 to 80 jobs late in the run. Not isolated
  further.
- **Daemon memory (1.3 to 3.8%).** `vmmap` of a fresh daemon: one more
  thread (the managed authority's, started even with no journal; 19 to 20
  stacks, 848 to 928 KiB), 80 KiB more `MALLOC_SMALL`, and code pages of the
  larger binary.
- **CLI memory.** Mostly code pages of the larger debug binary. Every
  client command also starts a Tokio runtime with one worker per core; with
  two workers the release `job ls` peaks at 4.20 MiB instead of 4.75 MiB
  and runs 9% faster (60 interleaved pairs).

Where the time of the ops goes, on both builds:

- Every `F_FULLFSYNC` costs about 5.5 ms here, an `F_BARRIERFSYNC` 2.8 ms
  and a plain `fsync` 0.3 ms. A launch flushes 15 to 17 times, a suspend 39
  to 41, a resume 24, an approval 3. Each atomic write (`write_atomic`)
  flushes the file and then the directory.
- Every process start costs 2 to 10 ms; git runs 24 times in a resume and
  30 in a suspend, one after the other.
- A restore replays the whole bundle chain, and runs `git bundle verify`
  and then `git bundle unbundle` for each link, so a resume grows with the
  job's history: 520 ms at the start of a chain, 1200 ms at 16 links.
- Probes run one group after another: a suspend runs `claude --version`
  (112 ms in the fixture) and only then the `node` and `python3` probes (75
  ms); a resume runs the iCloud `defaults` reads (75 ms), then the `node`
  probes (63 ms), then the git restore, then `claude --version` in the
  daemon (86 ms) and again in the CLI (62 ms).
- A restart rewrites every job's derived manifest with two full flushes,
  even when it is current, so it costs about 7.5 ms per job.
- A move measures every probe (`Probes::ALL`) on both nodes: 42 processes,
  in parallel, so its time is that of the slowest probe.

## Work items

None is built here. Gains are from the fixture on this machine; each item
proves itself with a new A/B run and keeps every check it replaces.

Regressions against the anchor, to fix first:

1. Register while the trampoline starts: send `harness_started` as soon as
   the held shell has a pid, wait for its stop beside it, and release once
   both the stop and `harness_recorded` are in. Nothing of the harness runs
   before the record is durable, as now. `crates/fabric/src/lib.rs`,
   `crates/fabric-harness/src/process.rs`, `harness.rs`. About -5 to -7 ms
   per headless launch, resume and pull.
2. `leads_group` by `rustix::process::getpgid` instead of `ps -o pgid=`,
   the same check without a process. `crates/fabricd/src/local.rs`. One
   process and about -2 ms before every release, and one process fewer
   than the anchor.
3. An atomic write with one full flush: `F_BARRIERFSYNC` on the temporary
   file (its bytes reach the drive before the rename does), the rename,
   then `F_FULLFSYNC` on the directory, which flushes the drive's cache for
   both. Same atomicity and the same durability at return.
   `crates/fabricd/src/durable.rs`, `crates/fabric-harness/src/durable.rs`.
   About -2.5 ms per atomic write: about -3 ms before a launch's release
   and -15 to -30 ms per launch, resume and suspend. Changes how ADR 0002
   reaches "durable before visible"; approved by Efe on 2026-10-29.
4. The trampoline through `/bin/dash` (or `/bin/bash`) where it exists,
   not the `/bin/sh` shim that starts one of them: `sh -c exit` 3.9 ms,
   `dash -c exit` 2.4 ms. `crates/fabric-harness/src/process.rs`. About
   -1.5 ms per launch, mostly hidden once item 1 overlaps it.
5. The symbolic refs and `HEAD` of a restore in the `update-ref --stdin`
   transaction (`symref-update`, git 2.46 and later), one `symbolic-ref`
   per ref below that. `crates/fabric-store/src/restore.rs`, `git.rs`,
   `budget.toml` with a `[[change]]` entry. Two processes fewer per resume
   and pull (about -14 ms), one fewer than the anchor.
6. Start the managed authority's thread only when a journal exists at
   start or at the first managed call. `crates/fabricd/src/authority.rs`,
   `node.rs`. One thread and 80 to 100 KiB fewer per daemon.
7. Client commands on a Tokio runtime with two workers; `serve` keeps
   one per core. `crates/fabric/src/lib.rs`. Release `job ls`: peak 4.75
   to 4.20 MiB (-12%), time ratio 0.906 in 60 pairs.

Beyond the anchor:

8. A restore runs `git bundle unbundle` alone: it runs the same
   `verify_bundle` before `index-pack` (git's `bundle.c`, on `master` and
   on `v2.20.0`; a bundle whose prerequisite is missing fails `unbundle`
   with the same message on git 2.50.1, 2.54 and 2.55).
   `crates/fabric-store/src/restore.rs`. One process per chain link, about
   13 ms each: -65 to -210 ms per resume and pull.
9. Fewer links to replay: a lower `MAX_BUNDLE_CHAIN` (16 now), or a
   per-node object cache of the job so a restore unbundles only the links
   it lacks. `crates/fabric-store/src/capture.rs`, `restore.rs`. Resume
   would stay near the 520 ms of a short chain instead of reaching 1200 ms;
   a design choice (bytes per capture against restore time).
10. A capture reads the refs from `git bundle verify`'s own listing instead
    of a second `git bundle list-heads`, and reuses the result of `git
    ls-files --others --ignored --exclude-standard -z`, which runs twice
    in a row. `crates/fabric-store/src/capture.rs`. Two processes and about
    -15 ms per suspend.
11. A restart rewrites a job's manifest only when it is missing or behind
    the ledger. `crates/fabricd/src/job.rs`, `ledger_file.rs`, `node.rs`.
    Two full flushes fewer per job: restart about flat in the job count
    instead of +7.5 ms per job.
12. A ledger append writes its derived manifest atomically (barrier before
    the rename) without full flushes of its own: a manifest behind the
    ledger is already the tolerated crash window (`check_manifest`), and
    the ledger keeps its full flush. `crates/fabricd/src/ledger_file.rs`,
    `durable.rs`. Approve and ledger append from 3 full flushes to 1 (about
    16 to 9 ms), and every append of the other ops. Approved by Efe on
    2026-10-29, with item 3.
13. A suspend probes the harness's `--version` beside workspace detection
    and the node facts, which do not depend on it.
    `crates/fabricd/src/ops.rs` (`capture`). About -60 to -75 ms per
    suspend.
14. A resume runs the restore root's iCloud check beside the node probes,
    and starts the daemon's harness `--version` with the restore.
    `crates/fabricd/src/ops.rs`, `paths.rs`. About -60 ms and -60 to -85 ms
    per resume and pull.
15. The CLI's resume guard starts its `--version` as soon as the node names
    the harness, through an additive event (ADR 0018), instead of after the
    answer. `crates/fabric/src/lib.rs`, `crates/fabric-client/src/api.rs`,
    `crates/fabricd/src/ops.rs`, the TypeScript schema and fixtures. About
    -60 ms per resume and pull here, more with a real Claude, which starts
    Node.
16. A harness's state capture flushes each file and directory with a plain
    `fsync` and the drive once at its commit point, the method
    `fabric-store` already uses (ADR 0014), instead of a full flush per
    directory level. `crates/fabric-harness/src/durable.rs`,
    `pipeline.rs`. Five to six full flushes and about -30 ms per suspend.
17. At a harness's exit, liveness without `ps` when `kill(pid, 0)` finds no
    process, which is the branch `ps` would take; and the record's removal
    without a directory flush, since a stale record is always checked
    again (approved by Efe on 2026-10-29). `crates/fabricd/src/local.rs`,
    `ops.rs`. One process and about -2 ms, then -4 ms, at every launch end.
18. A restore skips `git rev-parse --git-path info/exclude` after its own
    `git init`, whose layout it knows, and sets the four remote and branch
    settings in fewer processes. `crates/fabric-store/src/restore.rs`. One
    to four processes, -10 to -30 ms per resume.
19. A move measures on both nodes only the probes the snapshot's keys and
    the job's harness need, as capture does since issue #26, instead of
    every probe. `crates/fabricd/src/service.rs`, `ops.rs`. 42 to about 12
    processes per move; a real move no longer waits for another harness's
    `--version` or `pnpm store path`.
20. A job's `forked`, `marker` and `harness` files kept in memory and
    written through, since the daemon is their one writer: a request opens
    them five to six times, a restart three to six times per job, and `job
    ls` once per job. `crates/fabricd/src/job.rs`. `job ls` and restart
    lose a file open per job.

## Integration against the anchor, counted

On 2026-10-28 the budget and CI group merged every M2 deferred-work branch
into one scratch tree, `d12d410` (`pd/wip-budget-ci-evidence` at
`c8e25d6`, `pd/wip-cli-protocol-ops` at `bf14115`, `pd/wip-durability` at
`7d8f2b9`, `pd/wip-facts-net-carry` at `b0f25c0`, `pd/wip-launch-hold` at
`6586ef9`, `pd/wip-node-runtime` at `50df18c`, `pd/wip-store-git` at
`f345556`; no conflict), and ran `scripts/bench-ab.sh --pairs 0` against
the anchor in both profiles: one warm-up and three counted runs each, and
no timed pair. Other workflows kept the load average at 11 to 21, so no
time was taken; nothing below depends on load.

| Directory | Profile | Counted runs |
|---|---|---:|
| `integration-counted/` | test | 3 |
| `integration-counted-release/` | release | 3 |

Release, medians over the counted runs, anchor -> integration:

| op | processes | full flushes | barrier flushes | plain fsyncs | allocations | CLI peak KiB |
|---|---:|---:|---:|---:|---:|---:|
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 5 | 0 -> 0 | 4092 -> 4442 | 6912 -> 7248 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 1 | 0 -> 0 | 1186 -> 784 | 5104 -> 4736 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1507 -> 1131 | 5280 -> 4832 |
| acp suspend | 36 -> 35 | 39 -> 21 | 0 -> 10 | 9 -> 13 | 14958 -> 12293 | 5296 -> 4992 |
| acp move | 42 -> 36 | 15 -> 7 | 0 -> 4 | 0 -> 0 | 8303 -> 7005 | 5696 -> 5408 |
| acp resume | 32 -> 14 | 24 -> 13 | 0 -> 9 | 51 -> 61 | 11134 -> 8207 | 7056 -> 7328 |
| acp pull | 111 -> 84 | 81 -> 42 | 0 -> 24 | 59 -> 76 | 30353 -> 23868 | 7184 -> 7392 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 3 | 0 -> 0 | 17720 -> 9297 | 5296 -> 4928 |

`report.md` in each directory has every op and every counter that differs.
A resume's git starts stay at 10 in every run where the anchor's grow with
the bundle chain (20, 24, 28), and a pull's at 41 (anchor 54, 58, 62).
`budget::hot_paths` in count mode gives the same process counts on its own
fixture, now for every probe and harness too (`budget.toml`).

### What the counters show against the rule

Two regressions, both of memory, for the owners of the code before merge
(on the integrated head the release build has neither: "The integrated
head, timed", below):

1. **Daemon resident size**, after every counted op: release a 13976 ->
   14568 KiB, ratio 1.046 [1.039 to 1.051], b 14672 -> 15480 KiB, 1.059
   [1.057 to 1.061]; test profile 1.048 and 1.063. Right after a restart
   a daemon is only 160 to 176 KiB larger, about the larger binary's code
   (release 19.8 to 21.4 MB); after a suspend, move, resume or pull it is
   600 to 1400 KiB larger. The M2 head was at 0.997 and
   1.013 in release (`m2-head-release/`). Of the daemon sizes in the
   groups' own A/B runs (a size does not depend on load as a time does),
   `pd/wip-cli-protocol-ops` is at 1.029 and 1.054 against the anchor in
   release (`perf-cli-protocol-ops/run2-release` on that branch), and
   launch-hold and facts-net-carry are near 1 against the M2 head or base.
   That points at a cache that branch
   adds (job records kept in memory, `80cde73`; the ledger hash index,
   `e56c6bf`; per-peer heads, `1ecc017`), but it is not isolated: the next
   step is the counted run of each branch head alone against the anchor,
   and `vmmap` of both daemons after a suspend.
2. **CLI peak resident size of an op that runs a headless turn**: release
   launch +336 KiB (ACP), resume +96 to +272 KiB, pull +16 to +208 KiB,
   and `cli version` +96 KiB, which is code alone. Every other op's CLI is
   smaller than the anchor's (two-worker runtime, `3e1ebba`): approve 5104
   -> 4736 KiB, `cli ls` 5280 -> 4832 KiB.

Not regressions by themselves, for the timed run to judge: plain fsyncs
rose where full flushes fell (the barrier design of ADR 0002), and a
store's bytes read rose by about 1 KB per suspend and 5 KB per resume and
pull (the node bundle cache's pack checks, `da5844a`, `02b7dc4`), against
14 fewer processes per resume.

### The work items, by counter

| Item | Commits | In the integration |
|---|---|---|
| 1 register while the trampoline starts | `376148b` | order only; for the timed run |
| 2 `getpgid` for `ps -o pgid=` | `150ee4d` | `ps -o pgid=` 1 -> 0 per launch, resume and pull |
| 3 barrier, then one full flush | `41526fc`, `923aac7`, ADR `6d57d63` | full flushes per launch 15 -> 5, per suspend 39 -> 21 |
| 4 trampoline in `dash` | `84828df` | the held shell is `dash` |
| 5 symbolic refs in the ref transaction | `a4a72e4` | `git symbolic-ref` 1 -> 0 per resume |
| 6 authority thread only with work | `0392968`, `440d56e` | not counted here; `perf-node-runtime/daemon-idle-vs-anchor.tsv`: 19 threads, as the anchor |
| 7 two-worker client runtime | `3e1ebba` | CLI peak lower on every op that does not run a turn |
| 8 unbundle without `bundle verify` | `26a9475` | `git bundle` 10 -> 2 per resume |
| 9 node bundle cache | `da5844a`, `3a03e01`, `7939faa` | resume git flat at 10 (anchor 20 to 28) |
| 10 heads from the bundle header, one ignored listing | `6aa154f`, `b961153` | `git bundle` 3 -> 2 per suspend |
| 11 restart rewrites only a stale manifest | `af041ba` | restart full flushes 38 -> 6, opens 258 -> 87 |
| 12 ledger append without its own full flushes | `9f47c43` | approve and ledger append 3 -> 1 full flush |
| 13 suspend's version probe beside detection | `ecb5b99` | overlap only; for the timed run |
| 14 resume's iCloud check beside the probes | `9cc0ab4`, `1dd8dbc`, `55de9a9` | `defaults` 2 -> 0 per resume; overlap for the timed run |
| 15 resume guard's version early | `7bae3d0`, `df27512`, `5f076b2` | overlap only: the daemon and the CLI still each start the harness's `--version` once per resume; for the timed run |
| 16 capture with plain fsyncs | `3771de8`, `90d79de` | suspend full flushes 39 -> 21 with item 3 |
| 17 liveness without `ps` | `b83c221`, `d448c2c` | `ps -o lstart=` 2 -> 1 per launch |
| 18 restore without `rev-parse` and `config` | `26ec80b` | `git config` 4 -> 0, `git rev-parse` 1 -> 0 per resume |
| 19 a move measures only what it needs | `f05b03f`, then `202a844` on the integrated branch | partly at first: the other harnesses' probes were gone (42 -> 36 processes per ACP move), but every node probe still ran on both nodes. Finished by `202a844` (`FactsFor` names the tools): 42 -> 8 processes per ACP move, 11 for Claude and Codex, a pull 28 or 29 fewer |
| 20 job files in memory | `80cde73` | job file reads per move 4099 -> 119 bytes, per launch 1010 -> 121; opens per `cli ls` 13 -> 0 |

Items 3, 12 and 17 change how ADR 0002 reaches "durable before visible"
and asked for Efe's agreement; the durability branch amended ADR 0002
(`6d57d63`) before he had given it. Efe approved all three on 2026-10-29,
and ADR 0002 and ADR 0016 now say so, with the test that proves the
recovery from what each lets a power loss leave.

### For the integrator: the timed run

The timed half of this comparison waits for a quiet machine and for the
integrated head. Alone on the machine, with nothing else building:

```sh
scripts/bench-ab.sh --candidate <integrated head> --pairs 20 docs/evidence/perf-ab/final-test
scripts/bench-ab.sh --release --candidate <integrated head> --pairs 20 docs/evidence/perf-ab/final-release
FABRIC_BUDGET=enforce cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
```

The first two give every op's ratio to the anchor with its interval and
the daemons' sizes after timed runs; the third holds the times to the
`macos-aarch64` baselines and the process counts to `budget.toml`.

## The integrated head, timed

On 2026-10-28 the integrator cherry-picked the seven deferred-work branches
onto `m2/durable-job-foundations` (`docs/evidence/m2-deferred-integration/`),
added the commits below, and ran `scripts/bench-ab.sh` alone on the machine:
no other bench, build or test of these worktrees ran, each run started only
once the 1-minute load average was at most 6 (4.5 to 5.7 at each start, at
most 7.0 at an end; `machine.txt`), and the runs went one after another. The
candidate is the working tree at `dc8b87c`, whose code is `202a844`'s.

| Directory | Anchor | Profile | Pairs | Counted runs |
|---|---|---|---:|---:|
| `final-test/` | `176622e` | test | 20 | 3 |
| `final-release/` | `176622e` | release | 20 | 3 |
| `vs-m2-head-test/` | `f381d42`, the M2 head before the deferred work | test | 20 | 3 |
| `vs-m2-head-release/` | `f381d42` | release | 20 | 3 |
| `vs-m2-head-counted-test/`, `vs-m2-head-counted-release/` | `f381d42` | test, release | 0 | 1 |
| `aa-test/` | `176622e` against itself, an A/A control | test | 15 | 3 |
| `lto-before-counted-release/` | `176622e`; candidate `35b8929`, before the release profile | release | 0 | 3 |
| `lto-after-counted-release/` | `176622e`; candidate `35b8929` with `9635ea0`'s release profile, then uncommitted | release | 0 | 3 |

Commits of the integrator that change what runs, besides the merge:

- `9635ea0` builds release with fat LTO and one codegen unit (below).
- `7717d07` makes a new job's directory durable with the flushes of its
  files: one full flush fewer per new job (`new` 4 -> 3 against the M2 head).
- `e59c667` lets a capture's pending departure record join the capture's one
  full flush when both are on one device.
- `202a844` finishes item 19 (the table above).
- `0f5b363`, `a60b0a5`, `725dcff` are correctness and tidiness fixes off the
  hot paths: a reaped trampoline's group is not signalled, gc drops the
  bundle cache of a job with no snapshot left, the store names its root.

### Times

The median of the pairs' ratios, head over the build it is compared with,
and its 95% bootstrap interval; `faster` when the interval lies below 1.

| op | test: before / head ms | ratio [95%] | release: before / head ms | ratio [95%] |
|---|---:|---|---:|---|
| acp new | 153.3 / 68.4 | 0.447 [0.442-0.454] faster | 135.1 / 51.5 | 0.379 [0.368-0.387] faster |
| acp launch | 69.1 / 31.5 | 0.459 [0.448-0.465] faster | 64.3 / 27.4 | 0.426 [0.416-0.435] faster |
| acp approve | 15.7 / 8.3 | 0.532 [0.514-0.556] faster | 15.0 / 7.4 | 0.494 [0.469-0.514] faster |
| claude new | 138.3 / 51.6 | 0.374 [0.367-0.380] faster | 133.8 / 50.5 | 0.380 [0.361-0.392] faster |
| claude launch | 93.6 / 91.0 | 0.978 [0.945-1.002] | 90.9 / 88.8 | 0.988 [0.918-1.010] |
| claude approve | 16.9 / 9.2 | 0.535 [0.528-0.558] faster | 15.7 / 7.6 | 0.483 [0.466-0.509] faster |
| codex new | 136.9 / 52.3 | 0.386 [0.373-0.395] faster | 135.0 / 50.9 | 0.384 [0.359-0.400] faster |
| codex launch | 104.7 / 98.2 | 0.940 [0.917-0.960] faster | 103.1 / 94.3 | 0.939 [0.918-0.955] faster |
| codex approve | 16.7 / 8.7 | 0.533 [0.519-0.555] faster | 15.7 / 8.3 | 0.509 [0.495-0.528] faster |
| ledger append | 16.4 / 8.6 | 0.529 [0.522-0.553] faster | 15.5 / 7.7 | 0.499 [0.488-0.526] faster |
| cli version | 3.0 / 3.1 | 1.015 [0.966-1.087] | 2.9 / 3.0 | 1.006 [0.953-1.080] |
| cli ls | 5.1 / 4.7 | 0.888 [0.876-0.907] faster | 3.9 / 3.3 | 0.805 [0.764-0.883] faster |
| acp suspend | 357.5 / 267.0 | 0.741 [0.734-0.749] faster | 351.1 / 260.0 | 0.743 [0.739-0.750] faster |
| acp move | 166.4 / 108.6 | 0.651 [0.634-0.682] faster | 167.2 / 105.1 | 0.632 [0.613-0.656] faster |
| acp resume | 607.5 / 263.3 | 0.436 [0.372-0.501] faster | 599.7 / 259.2 | 0.431 [0.370-0.492] faster |
| acp pull | 1128.2 / 626.0 | 0.560 [0.521-0.595] faster | 1140.5 / 613.2 | 0.539 [0.503-0.582] faster |
| claude suspend | 436.1 / 276.1 | 0.630 [0.623-0.639] faster | 423.6 / 264.1 | 0.625 [0.618-0.642] faster |
| claude move | 166.3 / 112.0 | 0.665 [0.635-0.679] faster | 164.5 / 108.7 | 0.666 [0.644-0.678] faster |
| claude resume | 779.1 / 373.8 | 0.486 [0.416-0.542] faster | 767.3 / 370.6 | 0.484 [0.414-0.548] faster |
| claude pull | 1410.9 / 752.0 | 0.531 [0.507-0.578] faster | 1375.4 / 740.1 | 0.524 [0.509-0.572] faster |
| codex suspend | 649.3 / 496.6 | 0.764 [0.752-0.773] faster | 637.0 / 482.4 | 0.758 [0.747-0.770] faster |
| codex move | 171.1 / 111.7 | 0.656 [0.628-0.669] faster | 166.9 / 107.9 | 0.651 [0.640-0.676] faster |
| codex resume | 998.7 / 536.2 | 0.554 [0.478-0.585] faster | 979.2 / 526.5 | 0.545 [0.491-0.594] faster |
| codex pull | 1832.5 / 1124.8 | 0.621 [0.598-0.656] faster | 1822.8 / 1114.5 | 0.612 [0.586-0.652] faster |
| cold start | 60.3 / 60.0 | 0.991 [0.960-1.038] | 56.0 / 42.2 | 0.934 [0.743-0.995] faster |
| restart | 423.5 / 77.2 | 0.185 [0.151-0.204] faster | 414.4 / 55.4 | 0.129 [0.118-0.162] faster |

No op is slower than the anchor in either profile: no interval lies above 1.
Three ops are not faster, and their intervals hold 1: `claude launch`
(0.978 and 0.988), `cold start` in the test profile (0.991; 0.934 faster in
release), and `cli version` (1.015 and 1.006, medians above 1). `cli version`
was profiled from the counters: it starts no process and touches no state
file in either build; it makes 244 allocations against the anchor's 189, which
are the larger command tree clap builds before it prints the version (the
`managed`, `job repair` and `job export` commands), and in release its peak
resident size is lower than the anchor's (3168 -> 3072 KiB). The A/A control
puts it at 1.048 [0.879-1.123] against itself, so its time is noise at 3 ms.

Against the M2 head every op but three is faster, those three holding 1 too
(`cli version` 1.032 and 0.964, `cold start` 0.997 and 0.959):

| op | test: before / head ms | ratio [95%] | release: before / head ms | ratio [95%] |
|---|---:|---|---:|---|
| acp new | 153.4 / 68.4 | 0.448 [0.442-0.457] faster | 135.5 / 52.3 | 0.382 [0.369-0.389] faster |
| acp launch | 60.9 / 31.7 | 0.515 [0.503-0.528] faster | 56.4 / 27.8 | 0.493 [0.486-0.501] faster |
| acp approve | 15.9 / 8.4 | 0.526 [0.511-0.541] faster | 14.4 / 7.1 | 0.495 [0.482-0.511] faster |
| claude new | 136.4 / 52.3 | 0.388 [0.373-0.404] faster | 134.3 / 50.4 | 0.374 [0.368-0.391] faster |
| claude launch | 110.5 / 92.1 | 0.831 [0.795-0.864] faster | 107.4 / 88.7 | 0.822 [0.781-0.852] faster |
| claude approve | 17.3 / 9.5 | 0.553 [0.529-0.564] faster | 15.8 / 8.2 | 0.524 [0.499-0.535] faster |
| codex new | 137.1 / 52.4 | 0.385 [0.373-0.397] faster | 134.0 / 51.3 | 0.383 [0.361-0.404] faster |
| codex launch | 124.0 / 96.4 | 0.792 [0.763-0.809] faster | 120.8 / 94.3 | 0.767 [0.747-0.820] faster |
| codex approve | 17.4 / 9.6 | 0.548 [0.531-0.559] faster | 16.0 / 8.4 | 0.521 [0.496-0.556] faster |
| ledger append | 16.9 / 9.5 | 0.550 [0.523-0.575] faster | 16.1 / 8.4 | 0.517 [0.492-0.546] faster |
| cli version | 3.1 / 3.2 | 1.032 [0.939-1.158] | 3.1 / 3.0 | 0.964 [0.928-1.045] |
| cli ls | 5.2 / 4.9 | 0.947 [0.889-0.976] faster | 3.7 / 3.1 | 0.836 [0.790-0.916] faster |
| acp suspend | 358.2 / 270.4 | 0.753 [0.745-0.764] faster | 352.0 / 259.7 | 0.738 [0.731-0.755] faster |
| acp move | 169.6 / 108.6 | 0.631 [0.620-0.654] faster | 162.2 / 104.4 | 0.655 [0.631-0.669] faster |
| acp resume | 608.5 / 264.5 | 0.431 [0.373-0.506] faster | 613.9 / 257.5 | 0.437 [0.381-0.487] faster |
| acp pull | 1138.4 / 627.6 | 0.556 [0.494-0.602] faster | 1125.4 / 607.2 | 0.547 [0.490-0.575] faster |
| claude suspend | 434.5 / 276.0 | 0.635 [0.628-0.657] faster | 425.8 / 265.1 | 0.626 [0.617-0.632] faster |
| claude move | 169.0 / 112.5 | 0.653 [0.647-0.678] faster | 165.2 / 108.6 | 0.656 [0.641-0.691] faster |
| claude resume | 794.5 / 367.7 | 0.485 [0.415-0.517] faster | 790.3 / 366.0 | 0.474 [0.399-0.518] faster |
| claude pull | 1407.7 / 743.6 | 0.524 [0.496-0.562] faster | 1399.4 / 735.2 | 0.528 [0.501-0.566] faster |
| codex suspend | 648.8 / 492.8 | 0.754 [0.747-0.759] faster | 635.3 / 484.2 | 0.755 [0.741-0.775] faster |
| codex move | 169.4 / 112.8 | 0.658 [0.644-0.684] faster | 165.2 / 108.7 | 0.665 [0.639-0.681] faster |
| codex resume | 997.0 / 537.0 | 0.536 [0.485-0.578] faster | 1016.5 / 529.9 | 0.521 [0.474-0.570] faster |
| codex pull | 1831.3 / 1137.6 | 0.623 [0.599-0.645] faster | 1841.6 / 1110.9 | 0.613 [0.580-0.639] faster |
| cold start | 61.0 / 58.9 | 0.997 [0.921-1.019] | 43.6 / 43.0 | 0.959 [0.922-1.016] |
| restart | 430.6 / 74.6 | 0.173 [0.154-0.205] faster | 415.2 / 54.9 | 0.138 [0.106-0.175] faster |

### Memory

KiB, the anchor's median -> the head's, the median of the ratios and its
95% interval. "Counted" is after every counted op, with the counter library
in both builds; "timed" is after every timed run.

| | test profile | release |
|---|---|---|
| resident size, counted, daemon a | 25816 -> 26760, 1.028 [1.022-1.042] | 14128 -> 12792, 0.891 [0.889-0.894] |
| resident size, counted, daemon b | 27008 -> 27584, 1.015 [1.012-1.016] | 15064 -> 13248, 0.874 [0.867-0.882] |
| physical footprint, counted, daemon a | 6856 -> 6992, 0.981 [0.945-0.999] | 5520 -> 5480, 0.969 [0.963-0.972] |
| physical footprint, counted, daemon b | 9824 -> 8240, 0.833 [0.818-0.833] | 6784 -> 6304, 0.933 [0.917-0.933] |
| resident size, timed, daemon a | 20048 -> 21008, 1.047 [1.043-1.051] | 11344 -> 9832, 0.867 [0.864-0.870] |
| resident size, timed, daemon b | 28784 -> 28584, 0.985 [0.980-0.992] | 15968 -> 13808, 0.860 [0.855-0.871] |
| physical footprint, timed, daemon a | 5080 -> 5056, 0.994 [0.980-1.007] | 4216 -> 3920, 0.930 [0.922-0.939] |
| physical footprint, timed, daemon b | 12152 -> 9320, 0.772 [0.766-0.780] | 8296 -> 6904, 0.838 [0.821-0.851] |

Against the M2 head (`vs-m2-head-*`) both daemons are smaller in release
(resident size 0.885 and 0.858 counted, 0.874 and 0.847 timed; footprint
0.963 and 0.915 counted, 0.933 and 0.846 timed), and in the test profile
their footprint is smaller (0.948 and 0.803 counted, 0.948 and 0.746 timed)
while daemon a's resident size is 1.010 [1.009-1.015] after the timed runs.

In release, the build a node runs, both daemons are smaller than the
anchor's by both measures, and so is every CLI: the counted CLI peak
resident size is at or below the anchor's on every op. Before the release
profile of `9635ea0` they were larger: the M2 work grew the code by 8.7%
(`__text` 11147588 -> 12120480 bytes) and a process keeps the code pages it
ran resident, so the daemons' resident size was 1.030 and 1.052 of the
anchor's (`lto-before-counted-release/`) while their physical footprint
was 1.005 and 1.024; `vmmap` after that run (`vmmap/` there) put the
difference in resident `__TEXT` (15.0 -> 15.6 MB for daemon b) and not in
the heap. With fat LTO and one codegen unit the binary is 15442464 bytes
against the anchor's 19773920 (`lto-after-counted-release/`: resident size
0.897 and 0.891, footprint 0.988 and 0.990).

The test profile builds no release code, so its binary is still larger than
the anchor's (79277336 against 73350296 bytes), and what that costs shows:
daemon a's resident size is 1.047 [1.043-1.051] of the anchor's after the
timed runs and 1.028 [1.022-1.042] after the counted ops, and the counted
CLI peak resident size is higher on 15 ops, by 16 to 656 KiB (`acp resume`
+656, `acp launch` +624, `acp pull` +544, `claude resume` +496). The
physical footprint, which leaves out clean code pages, is not larger: daemon
a 0.994 [0.980-1.007] and 0.981 [0.945-0.999], daemon b 0.772 and 0.833.

The A/A control (`aa-test/`) calibrates the memory intervals. With the same
binary on both sides, daemon b's footprint after the counted ops is 1.058
[1.043-1.061] and its resident size 1.018 [1.014-1.021]: each side's daemon
is one process for the whole phase, so a ratio between two daemons carries
that process's own variance, and an interval that excludes 1 by a few
percent is not by itself a difference between builds. The test profile's
1.047 for daemon a's resident size is beyond what the control shows; the
release ratios are well beyond it on the other side. Of the 26 timed ops
the control called one faster (`acp launch` 0.974 [0.953-0.999]) and none
slower.

### What is left against the rule

- Time: nothing; no op of either profile is slower than the anchor.
- Memory, release: nothing; both daemons and every CLI are smaller.
- Memory, test profile only: daemon a's resident size (1.047) and the CLI
  peak resident size of the ops that run a headless turn, from the clean
  code pages of an unoptimized binary 8% larger than the anchor's; the
  physical footprint is not larger. A test-profile build is not what a node
  runs, but the rule as written did not say which profile it holds, so this
  was left for Efe to rule on rather than called no regression. Efe ruled
  on 2026-10-29: the performance rule gates the release build only, and the
  test profile is tracked, not gated (CONTRIBUTING, ADR 0009). These
  test-profile numbers stay in the record as tracked; they gate nothing.
