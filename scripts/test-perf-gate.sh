#!/bin/sh
# Proves that scripts/perf-gate.sh confirms a failing run and says so when
# the confirmation does not finish. A stand-in for scripts/bench-ab.sh
# logs its arguments and plays the outcome each case names.
#
# Usage: scripts/test-perf-gate.sh
# check evals its condition, in single quotes, after run sets status.
# shellcheck disable=SC2016,SC2034
set -eu

scripts=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failures=0

# The stand-in: --gate writes a verdict and exits with $GATE (or
# $CONFIRM_GATE with --confirm); a run exits with $RUN.
cat >"$work/bench" <<'BENCH'
#!/bin/sh
echo "$*" >>"$LOG"
case " $* " in
  *" --confirm "*)
    for last; do :; done
    echo "### Perf gate: confirmed" >"$last/verdict.md"
    exit "$CONFIRM_GATE" ;;
  *" --gate "*)
    for last; do :; done
    mkdir -p "$last"
    if [ "$GATE" -eq 0 ]; then
      echo "### Perf gate: pass" >"$last/verdict.md"
    else
      echo "### Perf gate: fail" >"$last/verdict.md"
    fi
    exit "$GATE" ;;
  *) exit "$RUN" ;;
esac
BENCH
chmod +x "$work/bench"

# run <name> <gate> <run> <confirm-gate>: the gate's status and log.
run() {
  out=$work/$1
  LOG=$work/$1.log GATE=$2 RUN=$3 CONFIRM_GATE=$4 FABRIC_PERF_BENCH=$work/bench \
    "$scripts/perf-gate.sh" "$out" --release --pairs 15 --anchor abc >/dev/null 2>&1 && status=0 || status=$?
}

check() {
  if eval "$2"; then
    echo "ok   $1"
  else
    echo "FAIL $1" >&2
    failures=$((failures + 1))
  fi
}

run pass 0 0 0
check "a passing run is not run again" \
  '[ "$status" -eq 0 ] && [ "$(wc -l <"$work/pass.log")" -eq 1 ]'

run confirmed 1 0 1
check "a failing run is run again with the first run's flags, candidate first" \
  'grep -qx -- "--release --pairs 15 --anchor abc --candidate-first $work/confirmed/confirm" "$work/confirmed.log"'
check "a failure confirmed fails the gate with the confirmed verdict" \
  '[ "$status" -eq 1 ] && [ "$(cat "$work/confirmed/verdict.md")" = "### Perf gate: confirmed" ]'

run unconfirmed 1 0 0
check "a failure not confirmed passes the gate" '[ "$status" -eq 0 ]'

run crashed 1 101 0
check "a confirmation that does not finish fails the gate with its status" '[ "$status" -eq 101 ]'
check "a confirmation that does not finish heads the verdict as no verdict" \
  '[ "$(head -n 1 "$work/crashed/verdict.md")" = "### Perf gate: no verdict" ] &&
   grep -q "confirmation run did not finish (exit 101)" "$work/crashed/verdict.md" &&
   grep -qx "### First run: fail" "$work/crashed/verdict.md"'

run broken 2 0 0
check "a gate that cannot judge stops without a confirmation" \
  '[ "$status" -eq 2 ] && [ "$(wc -l <"$work/broken.log")" -eq 1 ]'

if [ "$failures" -ne 0 ]; then
  echo "test-perf-gate: $failures case(s) failed" >&2
  exit 1
fi
echo "test-perf-gate: all cases pass"
