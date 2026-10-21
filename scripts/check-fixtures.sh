#!/bin/sh
# Fails when a fixture in protocol/fixtures that the base already had was
# edited, renamed or removed without a bump of API_VERSION. ADR 0018: within
# one api version every change to the client API is additive, so its
# fixtures only grow; a renamed field, a changed type or a removed variant
# shows up here as an edited fixture. A new shape gets a new fixture.
# protocol/fixtures/closed pins the variants of each closed enum the same
# way, so a new variant, which a new fixture alone would let through, needs
# the bump too.
#
# It also fails when protocol/required.txt, the keys each message fixture
# cannot do without, lost a line, or gained one for a fixture the base had,
# without the bump: a field made optional breaks an older reader once a
# node leaves it out, and one made required breaks an older writer. Lines
# for a new fixture are fine. CI runs it on every pull request and push to
# main. Run it before pushing.
#
# A few fixtures never change, bump or not: the framed hello, the
# incompatible answer and the closed list of hello answer types, and so do
# their lines in protocol/required.txt. They are how a client of one api
# learns that a node speaks another, so a client of every earlier api must
# still read them; changed at a bump, they would turn the message that
# says to upgrade into a frame no older client parses, and so would a
# field of the incompatible answer made optional. The hello itself may
# change with a bump, because a node answers a hello it cannot read with
# the incompatible answer.
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

frozen=$(git diff --name-status --no-renames --diff-filter=DMT "$fork" HEAD -- \
  protocol/fixtures/frame \
  protocol/fixtures/hello_answer/incompatible.json \
  protocol/fixtures/closed/hello_answer.json)
if [ -n "$frozen" ]; then
  printf '%s\n' "$frozen" >&2
  echo "error: a fixture of the version handshake changed; the framed hello, the incompatible answer and the hello answer types stay as they are in every api version, bump or not, since they are how an older client learns that it must upgrade (ADR 0018, Compatibility)" >&2
  exit 1
fi

# The pins of the frozen fixtures the base had, compared as sets, so a pin
# file sorted anew passes and one that lost or gained such a line does not.
# A frozen fixture the base did not have brings its lines, as any new one.
pins=protocol/required.txt
frozen_pins() {
  git show "$1:$pins" 2>/dev/null |
    sed -n '/^frame\//p; /^hello_answer\/incompatible\.json /p' |
    while IFS= read -r line; do
      if git cat-file -e "$fork:protocol/fixtures/${line%% *}" 2>/dev/null; then
        printf '%s\n' "$line"
      fi
    done |
    LC_ALL=C sort
}
if [ "$(frozen_pins "$fork")" != "$(frozen_pins HEAD)" ]; then
  echo "error: the required keys of a fixture of the version handshake changed in $pins; the framed hello and the incompatible answer need the same fields in every api version, bump or not, since they are how an older client learns that it must upgrade (ADR 0018, Compatibility)" >&2
  exit 1
fi

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

# Each line is `<dir>/<file> <pointer>` and starts with a letter, so the
# diff's own `---` and `+++` lines never match.
pin_diff=$(git diff -U0 --no-renames "$fork" HEAD -- "$pins")
removed=$(printf '%s\n' "$pin_diff" | sed -n 's/^-\([a-z]\)/\1/p')
added=$(printf '%s\n' "$pin_diff" | sed -n 's/^+\([a-z]\)/\1/p')
of_old_fixtures=$(printf '%s\n' "$added" | while IFS= read -r line; do
  if [ -n "$line" ] && git cat-file -e "$fork:protocol/fixtures/${line%% *}" 2>/dev/null; then
    printf '%s\n' "$line"
  fi
done)
if [ -n "$removed" ] || [ -n "$of_old_fixtures" ]; then
  printf '%s\n' "$removed" | sed '/^$/d; s/^/no longer required: /' >&2
  printf '%s\n' "$of_old_fixtures" | sed '/^$/d; s/^/newly required: /' >&2
  echo "error: the fields a message of api $now requires changed without an API_VERSION bump (ADR 0018, Compatibility): keep each pinned line of $pins, add lines only for new fixtures, or bump API_VERSION in api.rs and schema.ts" >&2
  exit 1
fi
