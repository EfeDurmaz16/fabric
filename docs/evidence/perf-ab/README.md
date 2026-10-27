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
   reaches "durable before visible"; needs Efe's agreement.
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
    16 to 9 ms), and every append of the other ops. Needs Efe's agreement
    with item 3.
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
    again (needs Efe's agreement). `crates/fabricd/src/local.rs`,
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
