#!/bin/sh
# Writes docs/reference/cli.md from the --help output of the CLI binary,
# walking every subcommand, so the reference cannot drift from the code.
#
# Usage: scripts/docs/cli-reference.sh [--check]
# The binary is $DOCS_CLI when set, else target/debug/<CLI_BIN> (build it
# with `cargo build -p <CLI_PACKAGE> --locked`). With --check, nothing is
# written and the script fails when the committed page differs.
set -eu

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"
. scripts/docs/names.env

bin=${DOCS_CLI:-target/debug/$CLI_BIN}
out=docs/reference/cli.md
if [ ! -x "$bin" ]; then
  echo "error: no CLI binary at $bin; run cargo build -p $CLI_PACKAGE --locked or set DOCS_CLI" >&2
  exit 1
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

# Prints the subcommands a --help output lists under "Commands:".
subcommands() {
  awk '/^Commands:/ { on = 1; next } on && /^$/ { exit } on && /^  [a-z]/ { print $1 }' | grep -v '^help$' || true
}

# Writes one section for the command named by the arguments, then one for
# each of its subcommands.
section() {
  name="$CLI_BIN${1:+ $*}"
  depth=$(( $# + 2 ))
  [ "$depth" -gt 4 ] && depth=4
  hashes=$(printf '%*s' "$depth" '' | tr ' ' '#')
  # shellcheck disable=SC2068
  help=$("$bin" $@ --help | sed 's/[[:space:]]*$//')
  {
    printf '%s `%s`\n\n```text\n%s\n```\n\n' "$hashes" "$name" "$help"
  } >> "$tmp"
  for sub in $(printf '%s\n' "$help" | subcommands); do
    # shellcheck disable=SC2068
    section $@ "$sub"
  done
}

cat > "$tmp" <<HEADER
<!-- This page is written by scripts/docs/cli-reference.sh from \`$CLI_BIN --help\`. Do not edit it; run the script. -->

# CLI reference

Every command of \`$CLI_BIN\` on \`main\`, as its own \`--help\` prints it. CI regenerates this page and fails when it differs, so it says what the binary does today.

Exit codes (ADR 0016, Exit codes): 0 for success, the harness's own code when it exits, 128 plus the signal when a signal stopped it, 80 when no node answers on the control socket, 81 not the owner, 82 frozen, 83 a peer unreachable, 84 refused without \`--force\` or \`--yes\`, 85 the job's state stops the command, 2 a usage error, and 1 any other failure. See [ADR 0016](../adr/0016-node-and-cli-as-built.md) for each one.

HEADER
section

if [ "${1:-}" = "--check" ]; then
  if ! diff -u "$out" "$tmp"; then
    echo "error: $out is out of date; run scripts/docs/cli-reference.sh" >&2
    exit 1
  fi
else
  mkdir -p "$(dirname "$out")"
  cp "$tmp" "$out"
fi
