#!/bin/sh
# Records the provider bridge's own latencies (ADR 0021): a worker spawned
# and its handshake answered, and one request round trip to it, with the
# in-memory log and with the journal's two committed writes. The worker is
# the Rust test double, so the numbers are Fabric's side of the bridge.
#
# Every sample is kept, one per line in microseconds, with the commit, the
# machine and its load before and after, under the test profile (the one
# the time budget uses) and the release profile. Run it on a quiet machine
# and do not run another benchmark beside it.
#
# Usage: scripts/bench-bridge.sh <output-dir>
# The output directory must not exist yet, so an earlier record is never
# overwritten.
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/bench-bridge.sh <output-dir>" >&2
  exit 2
fi
out=$1
if [ -e "$out" ]; then
  echo "bench-bridge: $out exists; pick a new directory" >&2
  exit 2
fi

toplevel=$(git rev-parse --show-toplevel)
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$toplevel"

{
  echo "commit: $(git rev-parse HEAD)"
  echo "dirty: $(git status --porcelain | wc -l | tr -d ' ') paths"
  echo "uname: $(uname -srvm)"
  if command -v sysctl >/dev/null 2>&1; then
    echo "cpu: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || true) $(sysctl -n hw.ncpu 2>/dev/null || true) cores"
  fi
  echo "rustc: $(rustc --version)"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "load before: $(uptime)"
} >"$out/machine.txt"

for profile in test release; do
  if [ "$profile" = release ]; then flag=--release; else flag=; fi
  # Build first, so no sample pays for the build.
  cargo test -p fabric-bridge --test bench --locked $flag --no-run >"$out/build-$profile.log" 2>&1
  FABRIC_BRIDGE_BENCH_OUT="$out/$profile" \
    cargo test -p fabric-bridge --test bench --locked $flag -- --ignored --exact bridge_latencies \
    >"$out/run-$profile.log" 2>&1
done

echo "load after: $(uptime)" >>"$out/machine.txt"
cat "$out/test/summary.txt" "$out/release/summary.txt"
