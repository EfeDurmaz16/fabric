#!/bin/sh
# Proves that scripts/check-budget.sh fires. Each named case below must
# pass or fail as stated; a gate that stops matching would pass silently.
#
# Usage: scripts/test-check-budget.sh
set -eu

scripts=$(cd "$(dirname "$0")" && pwd)
script=$scripts/check-budget.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
repo=$work/repo

# The anchor of the performance rule, which an entry names when it sets
# baselines timed again from the anchor itself. The A/B bench compares
# with the same commit.
anchor=$(sed -n 's/^anchor=\([0-9a-f]\{40\}\)$/\1/p' "$script")
bench_anchor=$(sed -n 's/^anchor=\([0-9a-f]\{40\}\)$/\1/p' "$scripts/bench-ab.sh")
if [ -n "$anchor" ] && [ "$anchor" = "$bench_anchor" ]; then
  echo "ok   check-budget.sh and bench-ab.sh name the same anchor"
else
  echo "FAIL check-budget.sh names anchor '$anchor', bench-ab.sh '$bench_anchor'" >&2
  failures=$((failures + 1))
fi
budget=crates/fabric/tests/e2e/budget.toml

git_() {
  git -C "$repo" "$@"
}

save() {
  git_ add -A
  git_ commit -q -m "$1"
}

# The settings and tables write_budget writes; new_repo resets them.
defaults() {
  ratio=1.3
  grace=50
  reps=7
  percentile=25
  counted='counted = ["git", "ps", "node"]'
  # The tables after the macOS one, each line its own argument of printf.
  tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 30'
  processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1] }
"cold start" = {}'
}
defaults

# Writes the budget's settings and counted list, the macOS baselines with
# the given suspend baseline, the tables in $tables, the process counts in
# $processes, then the given entries, each argument one line.
write_budget() {
  baseline=$1
  shift
  mkdir -p "$repo/$(dirname "$budget")"
  {
    printf 'ratio = %s\ngrace_ms = %s\nreps = %s\npercentile = %s\n' "$ratio" "$grace" "$reps" "$percentile"
    if [ -n "$counted" ]; then
      printf '%s\n' "$counted"
    fi
    printf '\n[baseline.macos-aarch64]\n"acp suspend" = %s\n"cold start" = 57\n\n' "$baseline"
    if [ -n "$tables" ]; then
      printf '%s\n\n' "$tables"
    fi
    if [ -n "$processes" ]; then
      printf '%s\n\n' "$processes"
    fi
    printf '%s\n' "$@"
  } > "$repo/$budget"
}

first_entry='[[change]]
date = "2026-10-24"
why = "first budget"'

# A fresh repository whose main holds the budget with its first entry and
# another file, with a branch `base` at that commit.
new_repo() {
  defaults
  rm -rf "$repo"
  mkdir -p "$repo"
  git_ init -q -b main
  git_ config user.name test
  git_ config user.email test@example.invalid
  git_ config commit.gpgsign false
  printf 'hello\n' > "$repo/README.md"
  write_budget 700 "$first_entry"
  save 'test: add the budget'
  git_ branch -q base
}

expect() {
  want=$1
  name=$2
  dir=$3
  shift 3
  if (cd "$dir" && "$script" "$@") > /dev/null 2>&1; then
    got=pass
  else
    got=fail
  fi
  if [ "$got" = "$want" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: expected $want, got $got" >&2
    failures=$((failures + 1))
  fi
}

new_repo
printf 'more\n' >> "$repo/README.md"
save 'docs: touch another file'
expect pass 'the budget unchanged' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "suspend is faster, so its baseline is lower"'
save 'perf: lower a baseline'
expect pass 'a baseline lowered with a new entry' "$repo" base

new_repo
write_budget 500 "$first_entry"
save 'perf: lower a baseline'
expect fail 'a baseline lowered without an entry' "$repo" base
mkdir -p "$repo/sub"
expect fail 'a baseline lowered without an entry, run from a subdirectory' "$repo/sub" base

# A baseline is the anchor's time and only goes down: a slower op is a bug
# to fix, never a tolerance to record, since raised baselines compound.
new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "suspend checks the new trust store"'
save 'perf: accept a slower suspend'
expect fail 'a baseline raised with a new entry' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "the anchor timed again on a new runner"' "anchor = \"$anchor\""
save 'perf: time the anchor again'
expect fail 'a baseline raised with an entry that names the anchor and no runs' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "timed again"' 'anchor = "0123456789abcdef0123456789abcdef01234567"'
save 'perf: time another commit'
expect fail 'a baseline raised with an entry that names another commit' "$repo" base

new_repo
tables='[baseline.linux-x86_64]
"cold start" = 30'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "drop the suspend"'
save 'perf: drop a baseline'
expect fail 'a baseline removed with a new entry' "$repo" base

new_repo
tables=
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "no linux"'
save 'perf: drop a platform'
expect fail 'a platform removed with a new entry' "$repo" base

new_repo
tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 30

[baseline.linux-aarch64]
"acp suspend" = 900'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "an arm runner"'
save 'perf: add a platform'
expect pass 'a platform added with a new entry' "$repo" base

new_repo
tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 900'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "slower on linux"'
save 'perf: raise a linux baseline'
expect fail 'a baseline of a second platform raised' "$repo" base

new_repo
tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 30
"restart" = 36'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "restart timed"'
save 'perf: add a baseline'
expect pass 'a baseline added with a new entry' "$repo" base

for knob in 'ratio 1.4' 'grace 60' 'reps 5' 'percentile 10'; do
  new_repo
  name=${knob% *}
  eval "$name=${knob#* }"
  write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' "why = \"a looser $name\""
  save "perf: loosen $name"
  expect fail "a looser $name with a new entry" "$repo" base
done

for knob in 'ratio 1.2' 'grace 40' 'reps 9' 'percentile 50'; do
  new_repo
  name=${knob% *}
  eval "$name=${knob#* }"
  write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' "why = \"a tighter $name\""
  save "perf: tighten $name"
  expect pass "a tighter $name with a new entry" "$repo" base
done

# A process count is the anchor rule without the load: it only goes down,
# and a program the base counted that an op did not start was zero there.
new_repo
processes='[processes]
"acp suspend" = { git = [8, 10], ps = [1, 1] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "one git fewer"'
save 'perf: lower a process count'
expect pass 'a process count lowered' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 11], ps = [1, 1] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a resume runs more git now"'
save 'perf: raise a process count'
expect fail 'a process count raised with a new entry' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 10] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "no ps"'
save 'perf: start a program no more'
expect pass 'a program an op no longer starts' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1] }
"cold start" = { git = [5, 5] }'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "cold start reads git"'
save 'perf: start git at cold start'
expect fail 'a counted program an op starts anew' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1], node = [2, 2] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "suspend probes node"'
save 'perf: probe node at suspend'
expect fail 'a counted program an op names for the first time' "$repo" base

new_repo
counted='counted = ["git", "ps", "node", "sh"]'
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1], sh = [2, 2] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "the budget counts sh"'
save 'test: count sh'
expect pass 'a program counted for the first time' "$repo" base

new_repo
counted='counted = ["git", "ps"]'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "node is not counted"'
save 'test: stop counting node'
expect fail 'a program no longer counted' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1] }'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "no cold start"'
save 'test: drop an op'
expect fail 'the counts of an op removed' "$repo" base

new_repo
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1] }
"cold start" = {}
"restart" = { git = [3, 3] }'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "restart counted"'
save 'test: count restart'
expect pass 'a new op with its counts' "$repo" base

# A base from before the counted list counts every program its table names.
new_repo
counted=
write_budget 700 "$first_entry"
git_ commit -q --amend --no-edit -a
git_ branch -q -f base
counted='counted = ["git", "ps", "node"]'
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1], node = [2, 2] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "the budget counts node"'
save 'test: count node'
expect pass 'a program the base did not name, under a base with no counted list' "$repo" base
processes='[processes]
"acp suspend" = { git = [9, 10], ps = [1, 1], node = [2, 2] }
"cold start" = { ps = [1, 1] }'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "the budget counts node"' '' '[[change]]' 'date = "2026-11-03"' 'why = "cold start runs ps"'
save 'perf: run ps at cold start'
expect fail 'a program the base named, started anew, under a base with no counted list' "$repo" base

# Counts written in a form the check cannot read fail, instead of being
# skipped by the comparison.
new_repo
processes='[processes]
"acp suspend" = { git = [9, 10] ps = [1, 1] }
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a missing comma"'
save 'test: counts the check cannot read'
expect fail 'process counts the check cannot read' "$repo" base

new_repo
processes='[processes]
"cold start" = {}

[processes."acp suspend"]
git = [30, 31]'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a table per op"'
save 'test: a table per op'
expect fail 'process counts as a table of their own' "$repo" base

new_repo
ratio='1.3
processes."acp suspend".git = [30, 31]'
processes='[processes]
"cold start" = {}'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a dotted key"'
save 'test: counts as a dotted key'
expect fail 'process counts as a dotted key above the tables' "$repo" base

new_repo
counted='counted = ["git", "ps",
  "node"]'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a list on two lines"'
save 'test: a counted list on two lines'
expect fail 'a counted list on two lines' "$repo" base

# A baseline raised under the anchor must be CI's time of the anchor's
# code: the entry names the runs, and gh, a fake here that answers from
# $FAKE_GH, reads them.
mkdir -p "$work/bin" "$work/gh"
cat > "$work/bin/gh" << 'EOF_GH'
#!/bin/sh
case "$1 $2" in
  "api repos/{owner}/{repo}/actions/runs/"*) cat "$FAKE_GH/${2##*/}.meta" ;;
  "run view") cat "$FAKE_GH/$3.log" ;;
  *) echo "fake gh: $*" >&2; exit 2 ;;
esac
EOF_GH
chmod +x "$work/bin/gh"
PATH=$work/bin:$PATH
FAKE_GH=$work/gh
export PATH FAKE_GH

# Writes run $1 of commit $2 of workflow $4 (ci.yml by default), whose
# table times the suspend at $3 ms on macos-aarch64.
fake_run() {
  printf '%s %s completed\n' "$2" "${4:-.github/workflows/ci.yml}" > "$FAKE_GH/$1.meta"
  {
    printf 'check\tTime budget\t2026-11-02T10:00:00.0000000Z ### Time budget, macos-aarch64, enforce\n'
    printf 'check\tTime budget\t2026-11-02T10:00:00.0000000Z | op | p25 | limit |\n'
    printf 'check\tTime budget\t2026-11-02T10:00:00.0000000Z |---|---|---|\n'
    printf 'check\tTime budget\t2026-11-02T10:00:00.0000000Z | acp suspend | %s | - |\n' "$3"
    printf 'check\tTime budget\t2026-11-02T10:00:00.0000000Z | cold start | 57 | - |\n'
    printf 'check\tTime budget\t2026-11-02T10:00:01.0000000Z done\n'
  } > "$FAKE_GH/$1.log"
}

# A repository whose base is the anchor, and runs 101 to 103 of it that
# time the suspend at 880, 900 and 950 ms: their baseline is 900.
anchor_repo() {
  new_repo
  FABRIC_BUDGET_TEST_ANCHOR=$(git_ rev-parse base)
  export FABRIC_BUDGET_TEST_ANCHOR
  fake_run 101 "$FABRIC_BUDGET_TEST_ANCHOR" 880
  fake_run 102 "$FABRIC_BUDGET_TEST_ANCHOR" 900
  fake_run 103 "$FABRIC_BUDGET_TEST_ANCHOR" 950
}

timed_again() {
  write_budget "$1" "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "the anchor timed again on a new runner"' "anchor = \"$FABRIC_BUDGET_TEST_ANCHOR\"" "runs = \"$2\""
  save 'perf: time the anchor again'
}

anchor_repo
timed_again 900 '101 102 103'
expect pass 'a baseline raised to the time CI runs of the anchor give' "$repo" base

anchor_repo
timed_again 950 '101 102 103'
expect fail 'a baseline raised above the time CI runs of the anchor give' "$repo" base

anchor_repo
timed_again 900 '101 102'
expect fail 'a baseline raised under the anchor with two runs' "$repo" base

anchor_repo
timed_again 900 '101 102 104'
expect fail 'a baseline raised under the anchor with a run gh cannot read' "$repo" base

anchor_repo
fake_run 103 "$FABRIC_BUDGET_TEST_ANCHOR" 900 .github/workflows/deny.yml
timed_again 900 '101 102 103'
expect fail 'a baseline raised under the anchor with a run of another workflow' "$repo" base

anchor_repo
git_ checkout -q -b slower
mkdir -p "$repo/crates/fabric/src"
printf 'fn slower() {}\n' > "$repo/crates/fabric/src/lib.rs"
save 'perf: slower code'
fake_run 103 "$(git_ rev-parse HEAD)" 950
git_ checkout -q main
timed_again 900 '101 102 103'
expect fail 'a baseline raised under the anchor with a run of other code' "$repo" base
unset FABRIC_BUDGET_TEST_ANCHOR

# A limit written in a form the check cannot read fails, instead of being
# skipped by the comparison.
new_repo
tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 30
"restart" = { ms = 36 }'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "an inline table"'
save 'perf: a baseline as an inline table'
expect fail 'a baseline the check cannot read' "$repo" base

new_repo
tables='[baseline.linux-x86_64]
"acp suspend" = 140
cold.start = 900'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a dotted key"'
save 'perf: a baseline as a dotted key'
expect fail 'a baseline as a dotted key' "$repo" base

new_repo
ratio='1.3
baseline.linux-x86_64."cold start" = 900'
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "a baseline above the tables"'
save 'perf: a baseline outside a table'
expect fail 'a baseline outside a platform table' "$repo" base

new_repo
write_budget 500 '[[change]]' 'date = "2026-10-24"' 'why = "first budget, reworded"' '' '[[change]]' 'date = "2026-11-02"' 'why = "faster"'
save 'perf: reword an entry'
expect fail 'an old entry edited' "$repo" base

new_repo
write_budget 500 '[[change]]' 'date = "2026-11-02"' 'why = "faster"'
save 'perf: replace the entries'
expect fail 'an old entry removed and a new one added' "$repo" base

new_repo
write_budget 500 '[[change]]' 'date = "2026-11-02"' 'why = "faster"' '' "$first_entry"
save 'perf: put the new entry first'
expect fail 'a new entry above the old ones' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "2026-11-02"'
save 'perf: an entry with no why'
expect fail 'a new entry with no why' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = ""'
save 'perf: an entry with an empty why'
expect fail 'a new entry with an empty why' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "next week"' 'why = "faster"'
save 'perf: an entry with no date'
expect fail 'a new entry whose date is not a date' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "faster"' '' '[[change]]' 'date = "2026-11-03"'
save 'perf: two entries, one whole'
expect fail 'two new entries, the second with no why' "$repo" base

new_repo
write_budget 500 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' '' '[[change]]' 'date = "2026-11-03"' 'why = "faster"'
save 'perf: two entries, one whole'
expect fail 'two new entries, the first with no why' "$repo" base

new_repo
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "linux numbers"' '' '[baseline.linux-aarch64]' '"acp suspend" = 900'
save 'perf: a table after the entries'
expect fail 'a table after a new entry' "$repo" base

new_repo
git_ rm -q "$budget"
save 'test: drop the budget'
expect fail 'the budget removed' "$repo" base

new_repo
git_ checkout -q -b later
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "slower"'
save 'perf: accept a slower suspend on main'
git_ checkout -q base
git_ checkout -q -b topic
printf 'more\n' >> "$repo/README.md"
save 'docs: touch another file'
expect pass 'a budget change the base made after the fork' "$repo" later

rm -rf "$repo"
mkdir -p "$repo"
git_ init -q -b main
git_ config user.name test
git_ config user.email test@example.invalid
git_ config commit.gpgsign false
printf 'hello\n' > "$repo/README.md"
save 'docs: start'
git_ branch -q base
write_budget 700 "$first_entry"
save 'test: add the budget'
expect pass 'the budget added with its first entry' "$repo" base

git_ checkout -q base
git_ checkout -q -b bare
write_budget 700
save 'test: add a budget with no entry'
expect fail 'the budget added with no entry' "$repo" base

expect fail 'no base given' "$repo"

mkdir -p "$work/norepo"
expect fail 'outside a git repository' "$work/norepo" base

if [ "$failures" -ne 0 ]; then
  echo "error: $failures budget check test(s) failed" >&2
  exit 1
fi
