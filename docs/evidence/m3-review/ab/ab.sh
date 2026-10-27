#!/bin/bash
# Alternates the base and head release bench binaries on the bridge paths
# (Rust double and the Capsule worker's fixture profile), keeping every sample.
W=$1; OUT=$2; N=$3
BASE=$W/target-base/release/deps/bench-7958c459f9b27d44
HEAD=/tmp/fabric-wt/m3/target/release/deps/bench-7958c459f9b27d44
mkdir -p $OUT
{ echo "base binary: $BASE (commit b3d0311)"; echo "head binary: $HEAD (commit $(git -C /tmp/fabric-wt/m3 rev-parse --short HEAD))"; echo "capsule worker: /tmp/capsule-wt/m3/packages/bridge-worker/dist/bin.js ($(git -C /tmp/capsule-wt/m3 rev-parse --short HEAD))"; echo "node: $(node --version)"; echo "uname: $(uname -srm)"; echo "load before: $(uptime)"; } > $OUT/machine.txt
for i in $(seq 1 $N); do
  for side in base head; do
    bin=$BASE; [ $side = head ] && bin=$HEAD
    echo "$side-$i start: $(uptime)" >> $OUT/machine.txt
    FABRIC_BRIDGE_BENCH_OUT=$OUT/$side-$i CAPSULE_BRIDGE_WORKER=/tmp/capsule-wt/m3/packages/bridge-worker/dist/bin.js \
      $bin --ignored --exact --test-threads=1 bridge_latencies capsule_worker_latencies > $OUT/$side-$i.log 2>&1 || echo "$side-$i FAILED" >> $OUT/machine.txt
  done
done
echo "load after: $(uptime)" >> $OUT/machine.txt
