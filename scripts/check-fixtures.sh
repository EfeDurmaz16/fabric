#!/bin/sh
# Fails when a fixture in protocol/fixtures that the base already had was
# edited, renamed or removed without a bump of API_VERSION. ADR 0018: within
# one api version every change to the client API is additive, so its
# fixtures only grow; a renamed field, a changed type or a removed variant
# shows up here as an edited fixture. A new shape gets a new fixture. CI
# runs it on every pull request and push to main. Run it before pushing.
#
# Usage: scripts/check-fixtures.sh <base-ref>
# The change checked is the one made since HEAD left base-ref, as a pull
# request shows it, so fixtures the base added later do not count.
#
# The script fails, instead of passing, when it cannot do its job: outside a
# git repository, with a base it cannot find, or without an API_VERSION.
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/check-fixtures.sh <base-ref>" >&2
  exit 2
fi

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"

fork=$(git merge-base "$1" HEAD)
api=crates/fabric-client/src/api.rs
version() {
  sed -n 's/^pub const API_VERSION: u32 = \([0-9][0-9]*\);$/\1/p'
}

now=$(git show "HEAD:$api" | version)
if [ -z "$now" ]; then
  echo "error: no API_VERSION in $api" >&2
  exit 1
fi
before=$(git show "$fork:$api" 2>/dev/null | version)
if [ -n "$before" ] && [ "$before" != "$now" ]; then
  echo "API_VERSION $before -> $now: the fixtures of api $before may change"
  exit 0
fi

# Without rename detection a renamed fixture is a deletion and an addition.
changed=$(git diff --name-status --no-renames --diff-filter=DMT "$fork" HEAD -- protocol/fixtures)
if [ -n "$changed" ]; then
  printf '%s\n' "$changed" >&2
  echo "error: a fixture of api $now changed without an API_VERSION bump; within one api version a change is additive (ADR 0018, Compatibility), so add a fixture for the new shape, or bump API_VERSION in api.rs and schema.ts if the change is not additive" >&2
  exit 1
fi
