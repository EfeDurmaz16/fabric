#!/bin/sh
# The perf job's gate (.github/workflows/perf.yml). Judges the A/B run in
# <output-dir>; when it fails the gate, runs both builds again into
# <output-dir>/confirm with the candidate's fixture started first, and
# judges both runs together, so a check fails only when it fails in both
# (CONTRIBUTING.md, Performance rule). When the confirmation run does not
# finish, verdict.md says so above the first run's verdict, which is not
# a verdict on its own, and the script exits with the run's status.
#
# Usage: scripts/perf-gate.sh <output-dir> <bench-ab.sh run flags>...
# The flags are those of the first run (--release, --pairs, --counted,
# --anchor, --candidate). scripts/test-perf-gate.sh sets
# FABRIC_PERF_BENCH to a stand-in for scripts/bench-ab.sh.
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: scripts/perf-gate.sh <output-dir> <bench-ab.sh run flags>..." >&2
  exit 2
fi
out=$1
shift
bench=${FABRIC_PERF_BENCH:-$(cd "$(dirname "$0")" && pwd)/bench-ab.sh}

status=0
"$bench" --release --gate "$out" || status=$?
[ "$status" -eq 1 ] || exit "$status"

echo "::notice::the first run fails the gate; both builds run again to confirm"
status=0
"$bench" "$@" --candidate-first "$out/confirm" || status=$?
if [ "$status" -ne 0 ]; then
  {
    echo "### Perf gate: no verdict"
    echo
    echo "The first run failed the gate and its confirmation run did not finish (exit $status), so no check is confirmed. The first run's verdict follows; on its own it is not the gate's verdict."
    echo
    sed 's/^### Perf gate:/### First run:/' "$out/verdict.md"
  } >"$out/verdict.md.new"
  mv "$out/verdict.md.new" "$out/verdict.md"
  echo "::error::the confirmation run did not finish (exit $status)"
  exit "$status"
fi
"$bench" --release --gate --confirm "$out/confirm" "$out"
