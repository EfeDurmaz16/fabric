#!/bin/sh
# Fails when crates/fabric/tests/e2e/budget.toml, the time budget of the hot
# paths, changed since HEAD left base-ref without a new [[change]] entry at
# its end. A raised baseline, a new process count or a looser ratio is a
# decision someone made, and the entry records when and why. The entries
# are append-only: those the base had stay as they were, in order, and new
# ones follow them, each with a date (YYYY-MM-DD) and a why, and nothing
# but entries follows them. CI runs it on every pull request and push to
# main. Run it before pushing.
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
