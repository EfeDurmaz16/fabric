#!/bin/sh
# Proves that scripts/check-fixtures.sh fires. Each named case below must
# pass or fail as stated; a gate that stops matching would pass silently.
#
# Usage: scripts/test-check-fixtures.sh
set -eu

script=$(cd "$(dirname "$0")" && pwd)/check-fixtures.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
repo=$work/repo
api=crates/fabric-client/src/api.rs

git_() {
  git -C "$repo" "$@"
}

# Writes the api version into api.rs.
set_version() {
  printf 'pub const API_VERSION: u32 = %s;\n' "$1" > "$repo/$api"
}

# Writes a fixture with the given content.
fixture() {
  mkdir -p "$(dirname "$repo/protocol/fixtures/$1")"
  printf '%s\n' "$2" > "$repo/protocol/fixtures/$1"
}

save() {
  git_ add -A
  git_ commit -q -m "$1"
}

# A fresh repository whose main holds api 1 and one fixture, with a branch
# `base` at that commit.
new_repo() {
  rm -rf "$repo"
  mkdir -p "$repo/$(dirname "$api")"
  git_ init -q -b main
  git_ config user.name test
  git_ config user.email test@example.invalid
  git_ config commit.gpgsign false
  set_version 1
  fixture reply/node_ping.json '{"kind":"node_ping","rtt_ms":3}'
  save 'feat: api 1'
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
fixture reply/node_ping_tcp.json '{"kind":"node_ping","rtt_ms":3,"wire":"tcp_tls"}'
save 'test: add a fixture'
expect pass 'a new fixture' "$repo" base

new_repo
fixture reply/node_ping.json '{"kind":"node_ping","round_trip_ms":3}'
save 'refactor: rename a field'
expect fail 'an existing fixture edited' "$repo" base
mkdir -p "$repo/sub"
expect fail 'an existing fixture edited, run from a subdirectory' "$repo/sub" base

new_repo
git_ rm -q protocol/fixtures/reply/node_ping.json
save 'refactor: drop a fixture'
expect fail 'an existing fixture removed' "$repo" base

new_repo
git_ mv protocol/fixtures/reply/node_ping.json protocol/fixtures/reply/ping.json
save 'refactor: rename a fixture'
expect fail 'an existing fixture renamed' "$repo" base

new_repo
fixture reply/node_ping.json '{"kind":"node_ping","round_trip_ms":3}'
set_version 2
save 'feat!: api 2'
expect pass 'an existing fixture edited with an api bump' "$repo" base

new_repo
git_ checkout -q -b feature
fixture reply/node_list.json '{"kind":"node_list","peers":[]}'
save 'test: add a fixture'
git_ checkout -q base
fixture reply/job_list.json '{"kind":"job_list","jobs":[],"closed":[]}'
save 'test: the base adds a fixture later'
git_ checkout -q feature
expect pass 'a fixture the base added after the branch left it' "$repo" base

new_repo
expect fail 'a base that does not exist' "$repo" nowhere

new_repo
printf 'pub const VERSION: u32 = 1;\n' > "$repo/$api"
save 'refactor: lose the version'
expect fail 'no API_VERSION to compare' "$repo" base

mkdir -p "$work/norepo"
expect fail 'outside a git repository' "$work/norepo" base

if [ "$failures" -ne 0 ]; then
  echo "error: $failures fixture check test(s) failed" >&2
  exit 1
fi
