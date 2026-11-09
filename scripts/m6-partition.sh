#!/bin/sh
# Runs the M6 partition proof (ADR 0028) on this host and writes its
# evidence: two fabricd processes with separate state roots, a real QUIC
# link through the userspace relay (fabric-relay), the relay paused and
# resumed. It is a two-process partition on one host, not two hosts; it
# touches no firewall and no system setting.
#
# What runs: the relay's own test, the two-daemon partition primitive
# and the five scenarios (crates/fabric/tests/e2e/partition.rs), each a
# managed job the M5 placement puts on the runner node and the launch
# path starts there.
#
# Usage: scripts/m6-partition.sh [<evidence dir>]
# The evidence dir defaults to docs/evidence/m6/runs/<date>.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/docs/evidence/m6/runs/$(date -u +%Y%m%dT%H%M%SZ)"}
mkdir -p "$out"

cd "$root"
log="$out/run.log"
{
  echo "host: $(uname -srm)"
  echo "commit: $(git rev-parse HEAD)"
  echo "started: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "note: two fabricd processes on one host through a userspace relay; no firewall or system setting touched"
} > "$out/meta.txt"

{ echo "load before: $(uptime | sed 's/.*load average[s]*: //')"; } >> "$out/meta.txt"
cargo test -p fabric --test e2e --locked -- relay:: partition:: --test-threads 1 --nocapture 2>&1 | tee "$log"

grep -hE "relay counters|relay while paused|^recovered |^test " "$log" > "$out/counters.txt" || true
echo "fabricd and fabric-relay processes after: $(pgrep -x fabricd | wc -l | tr -d " ") and $(pgrep -x fabric-relay | wc -l | tr -d " ") (any process on the host by those names)" >> "$out/meta.txt"
sed -i.bak -e "s#/private$root#<worktree>#g" -e "s#$root#<worktree>#g" -e "s#${CARGO_TARGET_DIR:-$root/target}#<target>#g" "$log" && rm -f "$log.bak"
echo "finished: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/meta.txt"
echo "evidence written to $out"
