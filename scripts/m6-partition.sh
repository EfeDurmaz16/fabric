#!/bin/sh
# Runs the M6 partition proof (ADR 0028) on this host and writes its
# evidence: two fabricd processes with separate state roots, a real QUIC
# link through the userspace relay (fabric-relay), the relay paused and
# resumed. It is a two-process partition on one host, not two hosts; it
# touches no firewall and no system setting.
#
# What runs today: the relay's own test and the two-daemon partition
# primitive. The three attempt-level scenarios (relay_paused_runner_alive,
# old_runner_sigstop_then_sigcont, old_runner_sigkill_then_restart) are
# ignored until the M5 executor lands at the restack; pass --all to run
# them anyway and see the reason they are blocked.
#
# Usage: scripts/m6-partition.sh [--all] [<evidence dir>]
# The evidence dir defaults to docs/evidence/m6-partition/runs/<date>.
set -eu

all=0
if [ "${1:-}" = "--all" ]; then
  all=1
  shift
fi
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/docs/evidence/m6-partition/runs/$(date -u +%Y%m%dT%H%M%SZ)"}
mkdir -p "$out"

cd "$root"
log="$out/run.log"
{
  echo "host: $(uname -srm)"
  echo "commit: $(git rev-parse HEAD)"
  echo "started: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "note: two fabricd processes on one host through a userspace relay; no firewall or system setting touched"
} > "$out/meta.txt"

cargo test -p fabric --test e2e --locked -- relay:: partition::two_daemons --nocapture 2>&1 | tee "$log"
if [ "$all" = 1 ]; then
  cargo test -p fabric --test e2e --locked -- --ignored partition:: --nocapture 2>&1 | tee -a "$log" || true
fi

grep -h "relay counters" "$log" > "$out/counters.txt" || true
echo "finished: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/meta.txt"
echo "evidence written to $out"
