#!/bin/sh
# Alternating A/B of the micro benches, the first build alternating per pair.
cd "$(dirname "$0")"
out=$1; pairs=$2
: > "$out"
for i in $(seq 0 $((pairs - 1))); do
  if [ $((i % 2)) -eq 0 ]; then order="base head"; else order="head base"; fi
  for side in $order; do
    if [ $side = base ]; then c=8d35276; else c=e61f197; fi
    f=$(./fabricd-$c --ignored --exact facts::tests::scratch_bench_measure_memory --nocapture 2>&1 | grep BENCH)
    echo "$i $side facts $f" >> "$out"
    r=$(./fabric-net-$c --ignored --exact rpc::tests::scratch_bench_rpc_memory --nocapture 2>&1 | grep BENCH)
    echo "$i $side rpc $r" >> "$out"
  done
done
echo DONE >> "$out"
