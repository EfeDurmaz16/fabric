# Durability flushes: items 3, 12 and 16, and the restore

Items 3, 12 and 16 of `docs/evidence/perf-ab/README.md`, and the same
method carried to the files a restore places. `F_FULLFSYNC` drains the
drive's whole write cache and is the slowest flush macOS has; before these
commits a launch made 15 to 17 of them, a suspend 39 to 41 and a commit
(approve, ledger append) 3.

| Commit | What |
|---|---|
| `41526fc` | item 3, `fabricd`: an atomic write flushes its temporary with `F_BARRIERFSYNC` before the rename and the directory with `F_FULLFSYNC` after it; one full flush where it made two |
| `9f47c43` | item 12: the job manifest after an append is written with the barrier and no full flush of its own; the append keeps its `F_FULLFSYNC` |
| `923aac7` | item 3, `fabric-harness`: the same for every harness file; the parent of each new directory gets a plain `fsync` |
| `3771de8` | item 16: a capture stages its files with plain `fsync`s; the state manifest, written last, is the commit point with the barrier and one full flush |
| `90d79de` | a restore keeps the barrier on each file it places and flushes each directory once at its end, one per device with `F_FULLFSYNC` |
| `6d57d63` | ADR 0002 amended |
| `3904a60` | a ledger file hands its records to the ledger instead of keeping a copy of every record |
| `0b12238` | the ADR and four comments no longer cite flush timings taken on a loaded machine |

Linux keeps `fsync` for every flush, as before; nothing changes there.

## Why the guarantees hold

fcntl(2) on macOS:

- `F_BARRIERFSYNC` does what `fsync` does, then issues a barrier: the I/O
  flushed with `fsync` on the same device before it is persisted before
  any I/O that follows it. A file's bytes flushed with the barrier before
  its rename can never reach the medium after the rename, so after a power
  loss the name holds the old bytes or all of the new ones.
- `F_FULLFSYNC` drains the device's whole queue, so data `fsync`ed on the
  same device before it is persisted when it returns. One full flush after
  the rename makes the file, its name and everything flushed before it
  durable.

What each write promised at return is unchanged: `write_atomic` (both
crates), `StateManifest::write`, a capture and `place_all` return only
after a full flush that covers everything they wrote. The job manifest
after an append is the one write that no longer is durable at return: the
append's full flush comes first, so the manifest is never ahead of the
ledger on the medium, and a power loss can only leave it behind, which
`check_manifest` accepts and the next open writes again (ADR 0002). A file
system without barriers gets `F_FULLFSYNC` in place of the barrier. No
check was removed: the capture still writes its manifest last, restore
still verifies every staged file against the manifest's digests and the
destination's stat before its rename, and the ledger append still flushes
before a record is shown or sent.

The tests (`durable::record` in both crates) log every flush asked for
with its kind and the path its descriptor had then, and pin each commit
point call by call; `tests-red.md` has each new test failing with its
change put back.

## Counters

No timing is kept here. Every timed run of these commits was taken while
other workflows built and benchmarked on the same machine, so the times
are not evidence and were deleted; the integrator measures the times on a
quiet machine. What is kept are the load-insensitive counters of the
counted runs of two of those A/B runs, from `scripts/bench-ab.sh` on an
Apple M5 Pro (macOS arm64, rustc 1.97.1), release builds:

| Directory | Anchor | Candidate | Counted runs |
|---|---|---|---:|
| `counters-anchor-3904a60/` | `176622e`, the pre-campaign anchor | `3904a60` | 3 |
| `counters-base-6d57d63/` | `8d35276`, the base of this branch | `6d57d63`: these flush changes alone | 2 |

Each has `samples.tsv` (every counter of every counted run, the timing
columns removed), `spawns.tsv` (every process a side started, per op) and
`report.md` (medians, anchor to candidate).

Full flushes per op, base to `6d57d63`, these commits alone: launch 12 to
6 (ACP), 15 to 8 (Claude), 17 to 9 (Codex); approve and ledger append 3
to 1; new job 8 to 4; suspend 39 to 41 down to 19 to 21; move 15 to 7;
resume 24 to 14; pull 81 to 83 down to 41 to 43; restart 35 to 6. Each
removed full flush is a barrier or a plain `fsync` now (`barrierfsync`
and `fsync` in the counters): a suspend makes 4 to 8 more plain `fsync`s
and 9 or 10 barriers, a pull 4 to 8 more plain `fsync`s and 23 or 24
barriers. Spawns are the same on every op. Cold start is unchanged, 8
full flushes. The release binary grows by 16,240 bytes (20,663,776 to
20,680,016), the new flush code and `nix`'s `fcntl`.

## Memory

`3904a60` makes a node hold each ledger record once: before it, a ledger
file kept every record it scanned at open and a clone of every record it
appended, and `Ledger::open` took another clone of them all. Its effect on
the daemon's resident size was not counted apart from the rest; the
integrator's A/B of the head measures it.

The counted runs above also give the CLI's peak RSS per op, and it is not
settled. Base to `6d57d63`, the candidate's median is higher in 22 of 26
ops, by 132 KiB at the median (from 352 KiB lower to 824 KiB higher), with
two runs a side; anchor to `3904a60`, higher in 21 of 26, by 48 KiB at the
median, with three. No mechanism in these commits is known: the two
release binaries of the base comparison have `__DATA` and `__DATA_CONST`
of the same size and a `__text` 6,620 bytes larger, and `cli ls`, which
runs no code these commits change, moved by 96 KiB. Those runs were taken
under memory pressure, which moves resident size too, so the integrator's
quiet A/B must count the CLI's peak RSS of the head against the anchor
before these commits are called no regression in memory.

## The flush micro A/B (`micro/`)

`flush-ab.rs` is the `main.rs` of a scratch package with `rustix` 1.1.5
and `nix` 0.31.3, both with `fs`. It runs the anchor's and the head's
sequence of flush calls for one op on one directory, interleaved in pairs,
as `flush-ab <dir> <pairs> <ops...>` with the ops `write_atomic`,
`commit`, `capture`, `restore` and `primitive`, and prints one row per
sample; `analyze.py` gives the median ratio and its 95% bootstrap
interval. No result of it is kept: its runs were taken under load. It is
here for the integrator to run on a quiet machine.

## Notes

- Items 3 and 12 were approved by Efe on 2026-10-29; ADR 0002 records
  them (`6d57d63`, corrected on 2026-10-29: this note and the ADR first
  said 2026-10-27, which was written before Efe had decided). Item 16 and
  the restore keep every guarantee at the return of the call that ends
  them.
- `Records::write_pending` in `crates/fabric-harness/src/departure.rs` is
  written between the staged files and the state manifest with a durable
  atomic write, so a capture of a harness with rewritable files makes one
  full flush for it and one at the manifest. Written as one write of the
  capture's batch it would save a full flush and a barrier per such
  capture, but only where the departure records and the state tree share
  a device, since the manifest's full flush covers only its own. It is
  outside this branch's files.
- `crates/fabricd/src/job.rs`, `Job::create`, flushes `jobs/` with a full
  flush when it makes the job's directory, then the ledger's atomic write
  flushes the job's directory with another; an ordered directory creation
  would save one full flush per new job. Outside this branch's files.
