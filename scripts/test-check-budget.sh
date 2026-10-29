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
  # The tables after the macOS one, each line its own argument of printf.
  tables='[baseline.linux-x86_64]
"acp suspend" = 140
"cold start" = 30'
}
defaults

# Writes the budget's settings, the macOS baselines with the given suspend
# baseline, the tables in $tables, then the given entries, each argument
# one line.
write_budget() {
  baseline=$1
  shift
  mkdir -p "$repo/$(dirname "$budget")"
  {
    printf 'ratio = %s\ngrace_ms = %s\nreps = %s\npercentile = %s\n\n' "$ratio" "$grace" "$reps" "$percentile"
    printf '[baseline.macos-aarch64]\n"acp suspend" = %s\n"cold start" = 57\n\n' "$baseline"
    if [ -n "$tables" ]; then
      printf '%s\n\n' "$tables"
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
expect pass 'a baseline raised with an entry that names the anchor' "$repo" base

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
