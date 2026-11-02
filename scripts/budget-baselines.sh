#!/bin/sh
# Prints a [baseline.<platform>] table for crates/fabric/tests/e2e/budget.toml
# from the Time budget step of CI runs: each op's baseline is the median of
# the runs' 25th percentiles (the p25 column of the step's table), rounded to
# a whole ms. A baseline is the anchor's time (CONTRIBUTING.md, Time budget),
# so give it runs of the anchor's code, and name them in the [[change]]
# entry that sets the table.
#
# Usage: scripts/budget-baselines.sh <platform> <log>...
# Each log is a CI run's log as `gh run view <run-id> --log` prints it, or
# the step's own output. The platform is the one the table's heading names,
# such as linux-x86_64.
#
# The script fails, instead of printing a partial table, when a log has no
# table for the platform, or when the logs do not all time the same ops.
set -eu

if [ "$#" -lt 2 ]; then
  echo "usage: scripts/budget-baselines.sh <platform> <log>..." >&2
  exit 2
fi
platform=$1
shift
for log in "$@"; do
  [ -f "$log" ] || { echo "budget-baselines: no file $log" >&2; exit 2; }
done

awk -v platform="$platform" '
  # A CI log prefixes each line with its job, step and time; the table
  # starts at its heading and ends at the first line that is not a row.
  {
    line = $0
    sub(/^[^\t]*\t[^\t]*\t[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9:.]*Z /, "", line)
    sub(/\r$/, "", line)
  }
  FNR == 1 { logs++; table = 0; rows = 0 }
  index(line, "### Time budget, " platform ", ") == 1 {
    if (seen[logs]) { print "budget-baselines: " FILENAME " has two tables for " platform > "/dev/stderr"; bad = 1 }
    seen[logs] = 1
    table = 1
    rows = 0
    next
  }
  table && (line ~ /^\| op \|/ || line ~ /^\|---/) { next }
  table && line ~ /^\|/ {
    rows = 1
    split(line, cell, "|")
    op = cell[2]; at = cell[3]
    gsub(/^ +| +$/, "", op); gsub(/ /, "", at)
    if (at !~ /^[0-9]+$/) { print "budget-baselines: " op " has no time in " FILENAME > "/dev/stderr"; bad = 1; next }
    if (!(op in count)) order[++ops] = op
    count[op]++
    sample[op, count[op]] = at
    next
  }
  # Text above the rows belongs to the table; text after them ends it.
  table && rows { table = 0 }
  END {
    for (i = 1; i <= logs; i++) {
      if (!seen[i]) { print "budget-baselines: a log has no table for " platform > "/dev/stderr"; bad = 1 }
    }
    if (!ops) { print "budget-baselines: no ops for " platform > "/dev/stderr"; bad = 1 }
    for (i = 1; i <= ops; i++) {
      op = order[i]
      if (count[op] != logs) { print "budget-baselines: " op " is timed in " count[op] " of " logs " logs" > "/dev/stderr"; bad = 1 }
    }
    if (bad) exit 1
    printf "[baseline.%s]\n", platform
    for (i = 1; i <= ops; i++) {
      op = order[i]
      # Insertion sort of the few samples, then the median.
      m = count[op]
      for (j = 1; j <= m; j++) s[j] = sample[op, j] + 0
      for (j = 2; j <= m; j++) {
        v = s[j]; k = j - 1
        while (k >= 1 && s[k] > v) { s[k + 1] = s[k]; k-- }
        s[k + 1] = v
      }
      if (m % 2) median = s[(m + 1) / 2]
      else median = int((s[m / 2] + s[m / 2 + 1]) / 2 + 0.5)
      printf "\"%s\" = %d\n", op, median
    }
  }
' "$@"
