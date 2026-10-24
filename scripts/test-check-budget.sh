#!/bin/sh
# Proves that scripts/check-budget.sh fires. Each named case below must
# pass or fail as stated; a gate that stops matching would pass silently.
#
# Usage: scripts/test-check-budget.sh
set -eu

script=$(cd "$(dirname "$0")" && pwd)/check-budget.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
repo=$work/repo
budget=crates/fabric/tests/e2e/budget.toml

git_() {
  git -C "$repo" "$@"
}

save() {
  git_ add -A
  git_ commit -q -m "$1"
}

# Writes the budget's settings with the given suspend baseline, then the
# given entries, each argument one line.
write_budget() {
  baseline=$1
  shift
  mkdir -p "$repo/$(dirname "$budget")"
  {
    printf 'ratio = 1.3\ngrace_ms = 50\n\n[baseline.macos-aarch64]\n"acp suspend" = %s\n\n' "$baseline"
    printf '%s\n' "$@"
  } > "$repo/$budget"
}

first_entry='[[change]]
date = "2026-10-24"
why = "first budget"'

# A fresh repository whose main holds the budget with its first entry and
# another file, with a branch `base` at that commit.
new_repo() {
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
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "suspend checks the new trust store"'
save 'perf: accept a slower suspend'
expect pass 'a baseline raised with a new entry' "$repo" base

new_repo
write_budget 900 "$first_entry"
save 'perf: raise a baseline'
expect fail 'a baseline raised without an entry' "$repo" base
mkdir -p "$repo/sub"
expect fail 'a baseline raised without an entry, run from a subdirectory' "$repo/sub" base

new_repo
write_budget 900 '[[change]]' 'date = "2026-10-24"' 'why = "first budget, reworded"' '' '[[change]]' 'date = "2026-11-02"' 'why = "slower"'
save 'perf: reword an entry'
expect fail 'an old entry edited' "$repo" base

new_repo
write_budget 900 '[[change]]' 'date = "2026-11-02"' 'why = "slower"'
save 'perf: replace the entries'
expect fail 'an old entry removed and a new one added' "$repo" base

new_repo
write_budget 900 '[[change]]' 'date = "2026-11-02"' 'why = "slower"' '' "$first_entry"
save 'perf: put the new entry first'
expect fail 'a new entry above the old ones' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"'
save 'perf: an entry with no why'
expect fail 'a new entry with no why' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = ""'
save 'perf: an entry with an empty why'
expect fail 'a new entry with an empty why' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "next week"' 'why = "slower"'
save 'perf: an entry with no date'
expect fail 'a new entry whose date is not a date' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "slower"' '' '[[change]]' 'date = "2026-11-03"'
save 'perf: two entries, one whole'
expect fail 'two new entries, the second with no why' "$repo" base

new_repo
write_budget 900 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' '' '[[change]]' 'date = "2026-11-03"' 'why = "slower"'
save 'perf: two entries, one whole'
expect fail 'two new entries, the first with no why' "$repo" base

new_repo
write_budget 700 "$first_entry" '' '[[change]]' 'date = "2026-11-02"' 'why = "linux numbers"' '' '[baseline.linux-x86_64]' '"acp suspend" = 900'
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
