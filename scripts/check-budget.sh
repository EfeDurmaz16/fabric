#!/bin/sh
# Fails when crates/fabric/tests/e2e/budget.toml, the time budget of the hot
# paths, changed since HEAD left base-ref without a new [[change]] entry at
# its end, or when the change loosens a limit. A new baseline, a lower one
# or a new process count is a decision someone made, and the entry records
# when and why. The entries are append-only: those the base had stay as
# they were, in order, and new ones follow them, each with a date
# (YYYY-MM-DD) and a why, and nothing but entries follows them.
#
# A baseline is the anchor's time (CONTRIBUTING.md, Performance rule), so
# it only goes down: a tolerance raised once per pull request compounds.
# A raised or removed baseline fails, and so do a higher ratio or grace_ms
# and a lower percentile or reps. The one exception is baselines timed
# again from the anchor itself, as on a new CI runner: a new entry then
# says so with anchor = "<the anchor below>", and may raise baselines,
# never the other limits.
#
# CI runs it on every pull request and push to main. Run it before pushing.
#
# Usage: scripts/check-budget.sh <base-ref>
# The change checked is the one made since HEAD left base-ref, as a pull
# request shows it, so a budget change the base made later does not count.
#
# The script fails, instead of passing, when it cannot do its job: outside a
# git repository, with a base it cannot find, or without the budget file.
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/check-budget.sh <base-ref>" >&2
  exit 2
fi

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"

fork=$(git merge-base "$1" HEAD)
budget=crates/fabric/tests/e2e/budget.toml
# The anchor of the performance rule, as scripts/bench-ab.sh names it.
anchor=176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if ! git show "HEAD:$budget" > "$work/after" 2>/dev/null; then
  echo "error: $budget is missing; the time budget test reads it" >&2
  exit 1
fi
if git show "$fork:$budget" > "$work/before" 2>/dev/null; then
  if cmp -s "$work/before" "$work/after"; then
    exit 0
  fi
else
  # The change that adds the budget brings its first entry.
  : > "$work/before"
fi

# The entries run from the first [[change]] line to the end of the file.
sed -n '/^\[\[change\]\]$/,$p' "$work/before" > "$work/old"
sed -n '/^\[\[change\]\]$/,$p' "$work/after" > "$work/all"
kept=$(wc -l < "$work/old" | tr -d ' ')
awk -v n="$kept" 'NR <= n' "$work/all" > "$work/kept"
if ! cmp -s "$work/old" "$work/kept"; then
  diff "$work/old" "$work/kept" >&2 || true
  echo "error: a [[change]] entry of $budget was edited, moved or removed; entries are append-only, so add a new one at the end instead" >&2
  exit 1
fi
awk -v n="$kept" 'NR > n' "$work/all" > "$work/new"

# Each new entry needs a date and a why, and only entries may follow the old
# ones, so a table added after them cannot pass for one.
status=0
awk '
  /^\[\[change\]\]$/ { if (n && !(d && w)) bad = 1; n++; d = 0; w = 0; next }
  /^\[/ { bad = 1 }
  /^date = "[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]"$/ { d = 1 }
  /^why = ".*[^ "].*"$/ { w = 1 }
  END {
    if (n && !(d && w)) bad = 1
    if (!n) exit 2
    exit bad
  }
' "$work/new" || status=$?
case $status in
  0) ;;
  2)
    echo "error: $budget changed without a new [[change]] entry; add one at its end that says why, with date = \"YYYY-MM-DD\" and why = \"...\" (CONTRIBUTING.md, Time budget)" >&2
    exit 1
    ;;
  *)
    sed 's/^/new: /' "$work/new" >&2
    echo "error: each new [[change]] entry of $budget needs date = \"YYYY-MM-DD\" and a why = \"...\" that says something, and nothing but entries may follow the old ones" >&2
    exit 1
    ;;
esac

# Each limit the budget sets, one per line, its fields split by tabs:
# "knob <name> - <value>" for ratio, grace_ms, percentile and reps, and
# "baseline <platform> <op> <ms>" for each baseline. A line of a baseline
# table it cannot read, or a baseline outside one, fails it, so a limit
# written in another form of TOML cannot slip past the comparison.
limits() {
  awk '
    function fail(why) {
      print "error: " label ":" FNR ": " why ": " $0 > "/dev/stderr"
      bad = 1
    }
    /^[ \t]*(#.*)?$/ { next }
    /^[ \t]*\[\[/ { section = "entries"; next }
    /^[ \t]*\[/ {
      section = $0
      sub(/^[ \t]*\[[ \t]*/, "", section)
      sub(/[ \t]*\][ \t]*(#.*)?$/, "", section)
      if (section == "baseline") fail("each platform needs a table of its own, [baseline.<platform>]")
      next
    }
    section == "" && /^[ \t]*(ratio|grace_ms|percentile|reps)[ \t]*=/ {
      name = $0
      sub(/^[ \t]*/, "", name)
      sub(/[ \t]*=.*$/, "", name)
      value = $0
      sub(/^[^=]*=[ \t]*/, "", value)
      sub(/[ \t]*(#.*)?$/, "", value)
      printf "knob\t%s\t-\t%s\n", name, value
      next
    }
    section == "" && /^[ \t]*"?baseline/ { fail("a baseline belongs in a [baseline.<platform>] table"); next }
    section ~ /^baseline\./ {
      if ($0 !~ /^"[^"]+"[ \t]*=[ \t]*[0-9]+[ \t]*(#.*)?$/) {
        fail("write each baseline as \"<op>\" = <ms>")
        next
      }
      op = $0
      sub(/^"/, "", op)
      sub(/".*$/, "", op)
      ms = $0
      sub(/^"[^"]*"[ \t]*=[ \t]*/, "", ms)
      sub(/[ \t]*(#.*)?$/, "", ms)
      printf "baseline\t%s\t%s\t%s\n", substr(section, 10), op, ms
    }
    END { exit bad }
  ' label="$2" "$1"
}

limits "$work/before" "$budget at the base" > "$work/limits-before"
limits "$work/after" "$budget" > "$work/limits-after"

# An entry that names an anchor names the performance rule's.
remeasured=
if grep -q '^anchor *=' "$work/new"; then
  if grep '^anchor *=' "$work/new" | grep -qv "^anchor = \"$anchor\"\$"; then
    grep '^anchor *=' "$work/new" | sed 's/^/new: /' >&2
    echo "error: an entry of $budget may only name the anchor of the performance rule, anchor = \"$anchor\" (CONTRIBUTING.md)" >&2
    exit 1
  fi
  remeasured=1
fi

awk -F '\t' -v remeasured="$remeasured" -v anchor="$anchor" '
  FILENAME == ARGV[1] { was[$1 FS $2 FS $3] = $4; next }
  { now[$1 FS $2 FS $3] = $4 }
  END {
    for (key in was) {
      split(key, k, FS)
      what = k[1] == "baseline" ? "the baseline of " k[3] " on " k[2] : k[2]
      if (!(key in now)) {
        print "error: " what " was removed; a limit stays, and a baseline only goes down" > "/dev/stderr"
        bad = 1
        continue
      }
      old = was[key] + 0
      new = now[key] + 0
      if (k[1] == "baseline" && new > old && !remeasured) {
        print "error: " what " rose from " was[key] " to " now[key] " ms. A baseline is the time of the anchor and only goes down, so make the op as fast again instead (CONTRIBUTING.md, Performance rule). If the baselines were timed again from the anchor itself, as on a new CI runner, a new entry says so with anchor = \"" anchor "\"" > "/dev/stderr"
        bad = 1
      }
      if (k[1] == "knob" && (k[2] == "ratio" || k[2] == "grace_ms") && new > old) {
        print "error: " what " rose from " was[key] " to " now[key] "; a looser limit is a tolerance that compounds" > "/dev/stderr"
        bad = 1
      }
      if (k[1] == "knob" && (k[2] == "percentile" || k[2] == "reps") && new < old) {
        print "error: " what " fell from " was[key] " to " now[key] "; a faster sample of the batch stands for it, which loosens every limit" > "/dev/stderr"
        bad = 1
      }
    }
    exit bad
  }
' "$work/limits-before" "$work/limits-after"
