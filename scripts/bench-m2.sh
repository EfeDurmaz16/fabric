#!/bin/sh
# Records the performance baseline of the M2 durable job work, and the same
# numbers again after it, under the same conditions: the time budget
# (`budget::hot_paths`: suspend, move, resume and pull under each harness,
# daemon cold start and restart) run several times in report mode, then
# `bench::m2_paths` (job creation, first headless launch, ledger appends,
# CLI cold start and a CLI round trip to the node). Both use the e2e
# fixture: two nodes over QUIC on loopback, test profile, fake probed
# programs that answer after 50 ms.
#
# Every run's whole output is kept in the output directory, with the
# commit, the machine and its load before and after, so a later run on a
# changed tree compares against raw data and not only a summary. Run it on
# a quiet machine and do not run another benchmark beside it.
#
# Usage: scripts/bench-m2.sh <output-dir> [budget-runs] [bench-reps]
# budget-runs defaults to 3 and bench-reps to 15. The output directory
# must not exist yet, so an earlier record is never overwritten.
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
  echo "usage: scripts/bench-m2.sh <output-dir> [budget-runs] [bench-reps]" >&2
  exit 2
fi
out=$1
runs=${2:-3}
reps=${3:-15}
if [ -e "$out" ]; then
  echo "bench-m2: $out exists; pick a new directory" >&2
  exit 2
fi

toplevel=$(git rev-parse --show-toplevel)
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$toplevel"

machine() {
  {
    echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "load: $(uptime)"
  } >>"$out/machine.txt"
}

{
  echo "commit: $(git rev-parse HEAD)"
  echo "dirty: $(git status --porcelain | wc -l | tr -d ' ') paths"
  echo "uname: $(uname -a)"
  if command -v sysctl >/dev/null 2>&1; then
    echo "cpu: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || true) $(sysctl -n hw.ncpu 2>/dev/null || true) cores"
  fi
  echo "rustc: $(rustc --version)"
  echo "budget runs: $runs, bench reps: $reps, profile: test"
} >"$out/machine.txt"

# Build first, so no run pays for the build.
cargo test -p fabric --test e2e --locked --no-run >"$out/build.log" 2>&1

i=1
while [ "$i" -le "$runs" ]; do
  machine
  echo "budget run $i of $runs" >&2
  FABRIC_BUDGET=report cargo test -p fabric --test e2e --locked -- \
    --ignored --exact budget::hot_paths --nocapture >"$out/budget-$i.log" 2>&1
  i=$((i + 1))
done

machine
echo "bench, $reps runs" >&2
FABRIC_BENCH_REPS=$reps cargo test -p fabric --test e2e --locked -- \
  --ignored --exact bench::m2_paths --nocapture >"$out/bench.log" 2>&1
machine

# The raw samples of the bench, op, run and microseconds, tab separated.
grep '^RAW' "$out/bench.log" | cut -f2- >"$out/bench-raw.tsv"
# Each budget run's table rows, prefixed with the run.
i=1
: >"$out/budget-rows.md"
while [ "$i" -le "$runs" ]; do
  grep '^| [a-z]' "$out/budget-$i.log" | grep -v '^| op ' | sed "s/^/run $i /" >>"$out/budget-rows.md"
  i=$((i + 1))
done
echo "bench-m2: wrote $out" >&2
