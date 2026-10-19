#!/bin/sh
# Fails when a tracked file, or a commit on this branch, carries an em dash or
# an AI attribution line. CI runs it on every pull request and push to main.
# Run it before pushing.
#
# Usage: scripts/check-hygiene.sh [base-ref]
# With a base ref, the messages of the commits in base-ref..HEAD are checked
# as well as the tracked files.
set -eu

status=0
em_dash=$(printf '\342\200\224')
file_trailer='^(Co-Authored-By:|.*Generated with \[)'
commit_trailer='^(co-authored-by:|[^a-z0-9]*generated with )'

if git grep -nI -e "$em_dash" -- . ; then
  echo "error: em dash in a tracked file" >&2
  status=1
fi

if git grep -nIE -e "$file_trailer" -- . ; then
  echo "error: AI attribution line in a tracked file" >&2
  status=1
fi

if [ "$#" -ge 1 ]; then
  messages=$(git log --format='%H%n%B' "$1..HEAD")
  if printf '%s\n' "$messages" | grep -n -e "$em_dash" ; then
    echo "error: em dash in a commit message" >&2
    status=1
  fi
  if printf '%s\n' "$messages" | grep -niE -e "$commit_trailer" ; then
    echo "error: AI attribution line in a commit message" >&2
    status=1
  fi
fi

exit "$status"
