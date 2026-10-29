#!/bin/sh
# Fails when crates/fabric/tests/e2e/budget.toml, the time budget of the hot
# paths, changed since HEAD left base-ref without a new [[change]] entry at
# its end, or when the change loosens a limit. A new baseline, a lower one
# or a lower process count is a decision someone made, and the entry
# records when and why. The entries are append-only: those the base had
# stay as they were, in order, and new ones follow them, each with a date
# (YYYY-MM-DD) and a why, and nothing but entries follows them.
#
# A baseline is the anchor's time (CONTRIBUTING.md, Performance rule), so
# it only goes down: a tolerance raised once per pull request compounds.
# A raised or removed baseline fails, and so do a higher ratio or grace_ms
# and a lower percentile or reps. A process count is the load-insensitive
# half of the same rule, so it only goes down too: a higher count of a
# program the base counted fails, and so does an op that starts a counted
# program it did not start at the base, an op whose counts were removed,
# and a program the budget no longer counts. A program the budget counts
# for the first time (its counted list) and a new op bring their counts.
#
# The one exception is baselines timed again from the anchor itself, as on
# a new CI runner. A new entry then says so with anchor = "<the anchor
# below>" and names at least three CI runs of the anchor's code with
# runs = "<id> <id> <id>", and may raise baselines, never the other limits
# or a process count. Each run is read with gh: it must be a finished run
# of .github/workflows/ci.yml on a commit whose crates, Cargo files and
# toolchain are the anchor's, and every raised baseline must be the one
# scripts/budget-baselines.sh gives from the runs' logs. So the exception
# takes CI's own times, not a number someone typed. Where gh cannot read
# the runs the check fails.
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

scripts=$(cd "$(dirname "$0")" && pwd)
toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"

fork=$(git merge-base "$1" HEAD)
budget=crates/fabric/tests/e2e/budget.toml
# The anchor of the performance rule, as scripts/bench-ab.sh names it.
anchor=176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5
# scripts/test-check-budget.sh runs this script in repositories of its own,
# which hold another anchor.
anchor=${FABRIC_BUDGET_TEST_ANCHOR:-$anchor}

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
# "knob <name> - <value>" for ratio, grace_ms, percentile and reps,
# "baseline <platform> <op> <ms>" for each baseline, "counted <program> -
# -" for each program the counted list names, "op <op> - -" for each op
# of [processes] and "process <op> <program> <first> <second>" for each
# of its counts. A line of a baseline table or of [processes] it cannot
# read, a counted list on more than one line, or a baseline or a count
# outside its table, fails it, so a limit written in another form of TOML
# cannot slip past the comparison.
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
      if (section ~ /^"?processes"?[ \t]*\./) fail("the process counts are one table, [processes]")
      next
    }
    section == "" && /^[ \t]*"?counted"?[ \t]*[.=]/ {
      if ($0 !~ /^counted[ \t]*=[ \t]*\[[ \t]*("[A-Za-z0-9_.-]+"[ \t]*,[ \t]*)*"[A-Za-z0-9_.-]+"[ \t]*,?[ \t]*\][ \t]*(#.*)?$/) {
        fail("write the counted programs on one line, counted = [\"<program>\", ...]")
        next
      }
      list = $0
      sub(/^[^\[]*\[/, "", list)
      sub(/\].*$/, "", list)
      n = split(list, names, ",")
      for (i = 1; i <= n; i++) {
        name = names[i]
        gsub(/[ \t"]/, "", name)
        if (name != "") printf "counted\t%s\t-\t-\n", name
      }
      next
    }
    section == "" && /^[ \t]*"?processes"?[ \t]*[.=]/ { fail("the process counts belong in the [processes] table"); next }
    section == "processes" {
      if ($0 !~ /^"[^"]+"[ \t]*=[ \t]*\{.*\}[ \t]*(#.*)?$/) {
        fail("write each op as \"<op>\" = { <program> = [<first>, <second>], ... }")
        next
      }
      op = $0
      sub(/^"/, "", op)
      sub(/".*$/, "", op)
      printf "op\t%s\t-\t-\n", op
      inner = $0
      sub(/^[^{]*\{/, "", inner)
      sub(/\}[ \t]*(#.*)?$/, "", inner)
      item = "^[ \t]*[A-Za-z0-9_.-]+[ \t]*=[ \t]*\\[[ \t]*[0-9]+[ \t]*,[ \t]*[0-9]+[ \t]*\\][ \t]*(,|$)"
      while (match(inner, item)) {
        count = substr(inner, RSTART, RLENGTH)
        inner = substr(inner, RSTART + RLENGTH)
        program = count
        sub(/^[ \t]*/, "", program)
        sub(/[ \t]*=.*$/, "", program)
        runs = count
        sub(/^[^\[]*\[/, "", runs)
        sub(/\].*$/, "", runs)
        gsub(/[ \t]/, "", runs)
        split(runs, at, ",")
        printf "process\t%s\t%s\t%s %s\n", op, program, at[1], at[2]
      }
      if (inner !~ /^[ \t]*$/) fail("write each count as <program> = [<first>, <second>]")
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

# The comparison prints each baseline that rose under an anchor entry as
# "<platform> <op> <ms>", split by tabs, for the check of the runs below.
awk -F '\t' -v remeasured="$remeasured" -v anchor="$anchor" '
  FILENAME == ARGV[1] { was[$1 FS $2 FS $3] = $4; next }
  { now[$1 FS $2 FS $3] = $4 }
  END {
    # The programs the base counted: its counted list, or, in a budget
    # from before the list, every program its [processes] names.
    for (key in was) {
      split(key, k, FS)
      if (k[1] == "counted") { listed = 1; counted[k[2]] = 1 }
    }
    for (key in was) {
      split(key, k, FS)
      if (!listed && k[1] == "process") counted[k[3]] = 1
    }
    for (key in was) {
      split(key, k, FS)
      if (k[1] == "process") continue
      if (k[1] == "counted") {
        if (!(key in now)) {
          print "error: the budget no longer counts " k[2] "; a count stays, and only goes down" > "/dev/stderr"
          bad = 1
        }
        continue
      }
      if (k[1] == "op") {
        if (!(key in now)) {
          print "error: the process counts of " k[2] " were removed; a count stays, and only goes down" > "/dev/stderr"
          bad = 1
        }
        continue
      }
      what = k[1] == "baseline" ? "the baseline of " k[3] " on " k[2] : k[2]
      if (!(key in now)) {
        print "error: " what " was removed; a limit stays, and a baseline only goes down" > "/dev/stderr"
        bad = 1
        continue
      }
      old = was[key] + 0
      new = now[key] + 0
      if (k[1] == "baseline" && new > old) {
        if (remeasured) {
          print k[2] "\t" k[3] "\t" now[key]
        } else {
          print "error: " what " rose from " was[key] " to " now[key] " ms. A baseline is the time of the anchor and only goes down, so make the op as fast again instead (CONTRIBUTING.md, Performance rule). If the baselines were timed again from the anchor itself, as on a new CI runner, a new entry says so with anchor = \"" anchor "\" and the CI runs that timed it, runs = \"<id> <id> <id>\"" > "/dev/stderr"
          bad = 1
        }
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
    # A count of a program the base counted, for an op the base had, only
    # goes down; one the op did not start at the base was zero there. A
    # program counted for the first time, and a new op, bring their counts.
    for (key in now) {
      split(key, k, FS)
      if (k[1] != "process" || !(("op" FS k[2] FS "-") in was) || !(k[3] in counted)) continue
      before = (key in was) ? was[key] : "0 0"
      split(before, b, " ")
      split(now[key], n, " ")
      if (n[1] + 0 > b[1] + 0 || n[2] + 0 > b[2] + 0) {
        print "error: " k[2] " starts " k[3] " [" n[1] ", " n[2] "] times in the counted runs, where the base started it [" b[1] ", " b[2] "]. A process count is the anchor rule measured without the load, so it only goes down: start fewer processes, or do the same check in one already started (CONTRIBUTING.md, Performance rule)" > "/dev/stderr"
        bad = 1
      }
    }
    exit bad
  }
' "$work/limits-before" "$work/limits-after" > "$work/raised"

[ -s "$work/raised" ] || exit 0

# Baselines rose under an anchor entry: they must be CI's times of the
# anchor's code, from runs the new entries name.
fail() {
  echo "error: $*" >&2
  exit 1
}
runs=$(sed -n 's/^runs = "\([0-9][0-9 ]*\)"$/\1/p' "$work/new" | tail -n 1)
# shellcheck disable=SC2086 # one argument per run id
set -- $runs
if [ "$#" -lt 3 ]; then
  sed 's/^/raised: /' "$work/raised" >&2
  fail "baselines raised under anchor = \"$anchor\" need the CI runs of the anchor's code that timed them, at least three, as runs = \"<id> <id> <id>\" in the new entry (CONTRIBUTING.md, Time budget)"
fi
command -v gh > /dev/null 2>&1 || fail "baselines raised under the anchor are checked against the CI runs that timed them, which needs gh"
for id in "$@"; do
  meta=$(gh api "repos/{owner}/{repo}/actions/runs/$id" --jq '[.head_sha, .path, .status] | join(" ")') ||
    fail "gh could not read CI run $id"
  sha=${meta%% *}
  rest=${meta#* }
  path=${rest%% *}
  status=${rest#* }
  [ "$path" = .github/workflows/ci.yml ] || fail "run $id is a run of $path, not of .github/workflows/ci.yml"
  [ "$status" = completed ] || fail "run $id has not finished ($status)"
  case $sha in
    *[!0-9a-f]* | '') fail "run $id names no commit" ;;
  esac
  if ! git cat-file -e "$sha^{commit}" 2> /dev/null; then
    git fetch -q origin "$sha" 2> /dev/null || fail "the commit $sha that run $id timed is not here, and could not be fetched"
  fi
  git diff --quiet "$anchor" "$sha" -- crates Cargo.toml Cargo.lock rust-toolchain.toml ":(exclude)$budget" ||
    fail "run $id timed $sha, whose crates, Cargo files or toolchain differ from the anchor $anchor"
  gh run view "$id" --log > "$work/run-$id.log" || fail "gh could not read the log of CI run $id"
done
logs=
for id in "$@"; do
  logs="$logs $work/run-$id.log"
done
tab=$(printf '\t')
cut -f 1 "$work/raised" | sort -u > "$work/platforms"
while read -r platform <&3; do
  # shellcheck disable=SC2086 # one argument per log
  "$scripts/budget-baselines.sh" "$platform" $logs > "$work/derived" ||
    fail "the runs give no baselines for $platform"
  grep "^$platform$tab" "$work/raised" > "$work/raised-here"
  while IFS=$tab read -r _ op ms; do
    derived=$(sed -n "s/^\"$op\" = \([0-9]*\)\$/\1/p" "$work/derived")
    [ "$derived" = "$ms" ] ||
      fail "the baseline of $op on $platform rose to $ms ms, but the runs $* give ${derived:-none}"
  done < "$work/raised-here"
done 3< "$work/platforms"
