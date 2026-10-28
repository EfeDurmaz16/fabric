# The store-git group: counters against the anchor

Five items of `docs/evidence/perf-ab/README.md` (Work items), all in
`fabric-store`, under the performance rule of CONTRIBUTING.md. The anchor
is `main` at `176622e`.

| item | change | commits |
|---|---|---|
| 8 | a restore unpacks each link with `git bundle unbundle` alone; `unbundle` runs the same `verify_bundle` first | `26a9475` |
| 18 | a restore writes `.git/info/exclude` of its own `git init` without `rev-parse --git-path`, and appends the carried config's sections without `git config` | `26ec80b` |
| 5 | symbolic refs and `HEAD` set with `symref-update` in the ref transaction on git 2.46 and later, `symbolic-ref` before | `a4a72e4` |
| 10 | a capture reads the bundle's heads from its header instead of `git bundle list-heads`; the ignored listings run beside work that does not need them | `6aa154f`, `b961153` |
| 9 | a node's bundle cache: a restore unpacks only the links of the chain the node has not unpacked before | `da5844a`, `3a03e01`, `02b7dc4`, `7939faa`; bench `1685654` |

`red-before.md` holds what each regression test printed against the code
before its fix.

## No timings here

The A/B runs of this group ran while other workflows built and tested on
the same machine, so their timings are not evidence and were deleted.
What stays are the counters of their counted runs, which do not depend on
load: processes, git commands, flushes, bytes read and written,
allocations and resident sizes. The times of the commits above are the
integrator's to take, alone on a quiet machine.

| directory | anchor | candidate | counted runs |
|---|---|---|---:|
| `head/` | `176622e` | `02b7dc4`, every item | 3 |
| `vs-base/` | `8d35276`, the base of this branch | `02b7dc4` | 3 |
| `items-5-8-10-18/` | `176622e` | `b961153`, items 5, 8, 10 and 18 | 3 |

Each holds `report.md` (medians, anchor -> candidate), `counters.tsv`
(every counted sample), `spawns.tsv` (every command line a counted run
started) and `machine.txt`. `7939faa`, after these runs, changes the
Linux build only: macOS copied the cache's files with a clone before it
and after it.

## Processes

Git processes per op in the three counted runs of `head/`, whose chain
grows by two links a run (`spawn:git`, anchor / candidate):

| op | run 1 | run 2 | run 3 |
|---|---|---|---|
| acp, claude, codex resume | 20 / 10 | 24 / 10 | 28 / 10 |
| acp, claude, codex pull | 54 / 41 | 58 / 41 | 62 / 41 |
| acp, claude, codex suspend | 30 / 29 | 30 / 29 | 30 / 29 |

The anchor's resume grows by four git processes a run, a `bundle verify`
and a `bundle unbundle` per new link; the candidate's stays at 10. The
`git bundle` processes of one acp resume: anchor 6, 10, 14; candidate 2,
2, 2 (the two links captured since this node last restored the job).

The git commands of a resume, anchor -> candidate (medians): `bundle`
10 -> 2, `config` 4 -> 0, `rev-parse` 1 -> 0, `symbolic-ref` 1 -> 0. A
suspend: `bundle` 3 -> 2, the `list-heads` gone. Every process of an op
(`spawns`): resume 32 -> 18 (acp), 34 -> 20 (claude), 35 -> 21 (codex);
pull 111 -> 94, 114 -> 97, 117 -> 100; suspend 36 -> 35, 35 -> 34,
37 -> 36. Moves hold 42.

## Flushes, bytes and allocations

Medians of the counted runs of `head/`, anchor -> candidate:

| op | full flushes | fsyncs | store bytes read | allocations | bytes allocated |
|---|---:|---:|---:|---:|---:|
| acp suspend | 39 -> 39 | 9 -> 9 | 37749 -> 38744 | 15013 -> 12731 | 3551466 -> 3518101 |
| acp resume | 24 -> 24 | 51 -> 61 | 12457 -> 18235 | 11166 -> 8814 | 4354428 -> 3689063 |
| acp pull | 81 -> 81 | 59 -> 72 | 24739 -> 32079 | 30396 -> 25376 | 8816115 -> 7849933 |
| claude resume | 24 -> 24 | 55 -> 65 | 12973 -> 18548 | 11104 -> 8703 | 4373139 -> 3690763 |
| codex resume | 24 -> 24 | 59 -> 69 | 13523 -> 18953 | 11235 -> 8826 | 4478437 -> 3796931 |

What rose, and why:

- Plain `fsync` calls of a resume, 51 -> 61 (median; 55, 61 and 67 over
  the three runs). Under `Durability::PowerLoss` a restore flushes each
  pack file it copies from the bundle cache, as it flushes every file it
  writes: three files a cached link (`.pack`, `.idx`, `.rev`), so this
  count grows with the cached chain. The anchor's `git bundle unbundle`
  wrote those files itself with `core.fsync=all` and
  `core.fsyncMethod=fsync`, which git does on macOS with an `F_FULLFSYNC`
  per file (`git_fsync` in git's `wrapper.c`), in a git process the
  counters do not see. The candidate does a plain `fsync` there and
  starts no process, and the full flushes the counters see are equal
  (24 -> 24).
- Bytes read from the store by a resume, about +6 KB: the cached files
  read for their digest before use, the check that replaces the
  `index-pack` of every object of every link, which read the same bytes,
  and more, in the uncounted git process. Opens rose by 24 for the same
  files; unlinks fell, 23 -> 20.
- The first `head/` run, on `e4e14f3`, showed a resume allocating 15 MB
  more than the anchor: the digest allocated a 1 MiB buffer per file.
  `02b7dc4` reads through one 64 KiB buffer per restore, and a resume now
  allocates 15% fewer bytes than the anchor, a pull 11% fewer.

## Memory

The daemons' resident size and the CLI's peak resident size of the
counted runs are in `report.md` and `counters.tsv`. Against the base of
this branch (`vs-base/`), a resume's CLI peak is 15648 -> 15776 KiB and a
suspend's 11680 -> 11728 KiB; the CLI runs no restore and no capture, and
its peak is bimodal from one run to the next on both builds, about 14 or
15.5 MiB. Against the anchor (`head/`) the CLI's peak rises on every op,
those this group does not touch included: that is the M2 branch under
this one (`docs/evidence/perf-ab/m2-head/`).

## Links unpacked by chain length

`restore_time_by_chain_length` in `crates/fabric-store/tests/bench.rs`
(ignored; the integrator runs it for the times) restores each snapshot of
a 17-link chain of a 300-file repository as a node does, first with the
bundle cache empty and then right after a restore of the link below. The
number of links git unpacked, which does not depend on load, was the
chain's length with the cache empty and 1 after the link below, at every
length from 1 to 17.

## Budget

`budget::hot_paths` counts these git processes (first run / second run)
on this branch; `budget.toml` holds the anchor's counts until the budget
owner's `[[change]]` entry lands (`pd/wip-budget-ci-evidence`, `4c175ce`,
which counted the same numbers on this change alone):

| op | anchor | this branch |
|---|---|---|
| suspend (acp, claude, codex) | git 28/30 | git 27/29 |
| resume (acp, claude, codex) | git 17/21 | git 9/10 |
| pull (acp, claude, codex) | git 51/55 | git 41/41 |
| move | git 2/2 | git 2/2 |

Every other count (kill, lsof, pgrep, ps) is unchanged.
