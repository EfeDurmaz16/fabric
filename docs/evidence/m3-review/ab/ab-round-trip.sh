#!/bin/bash
# Round trip with the in-memory log only, alternating binaries named on the command line.
OUT=$1; N=$2; shift 2
mkdir -p $OUT
for i in $(seq 1 $N); do
  for pair in "$@"; do
    name=${pair%%=*}; bin=${pair#*=}
    FABRIC_BRIDGE_BENCH_OUT=$OUT/$name-$i FABRIC_BRIDGE_BENCH_SPAWNS=5 FABRIC_BRIDGE_BENCH_JOURNAL_TRIPS=5 FABRIC_BRIDGE_BENCH_TRIPS=2000 \
      $bin --ignored --exact --test-threads=1 bridge_latencies > $OUT/$name-$i.log 2>&1 || echo "$name-$i FAILED"
  done
done
