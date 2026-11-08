#!/bin/sh
# Runs a built release archive the way a user would, on a machine that did
# not build it: the checksum, the version, the CLI contract against a node
# it starts, and on macOS the quarantine behaviour README.md describes. The
# release workflow runs it on clean runners for every target before anything
# is hosted (.github/workflows/release-smoke.yml); run it locally after
# `dist build` the same way.
#
# Usage: scripts/release-smoke.sh <dir> <target>
# <dir> holds fabric-<target>.tar.gz and its .sha256 file, as dist writes
# them to target/distrib. The expected version is the one in Cargo.toml.
#
# Needs only a POSIX shell, tar, git and a sha256 tool: no Rust toolchain.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <dir> <target>" >&2
  exit 2
fi
dir=$1
target=$2

toplevel=$(git rev-parse --show-toplevel)
# The one version, from [workspace.package] in Cargo.toml.
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$toplevel/Cargo.toml")
[ -n "$version" ] || { echo "error: no version in Cargo.toml" >&2; exit 1; }

archive=fabric-$target.tar.gz
fail() {
  echo "error: $*" >&2
  exit 1
}
step() {
  echo "smoke: $*" >&2
}

work=$(mktemp -d)
node_pid=
cleanup() {
  if [ -n "$node_pid" ]; then
    kill "$node_pid" 2>/dev/null || true
    wait "$node_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

step "checksum of $archive"
cp "$dir/$archive" "$work/"
grep -F "$archive" "$dir/$archive.sha256" >"$work/sum" || fail "$archive.sha256 names no $archive"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$work" && sha256sum -c sum) >&2
else
  (cd "$work" && shasum -a 256 -c sum) >&2
fi

tar -xzf "$work/$archive" -C "$work"
bin=$work/fabric-$target/fabric
[ -x "$bin" ] || fail "the archive has no executable fabric-$target/fabric"

if [ "$(uname -s)" = Darwin ]; then
  # A file fetched by curl, as Homebrew fetches a formula's archive, carries
  # no quarantine attribute, so Gatekeeper never checks it and brew install
  # needs no extra step. A browser download is quarantined; README.md says
  # to clear the attribute with the command below.
  step "macOS: no quarantine on a file fetched without a browser"
  for f in "$work/$archive" "$bin"; do
    if xattr -p com.apple.quarantine "$f" >/dev/null 2>&1; then
      fail "$f is quarantined"
    fi
  done
  # Apple silicon runs only signed code; the linker signs ad hoc, which is
  # all an unsigned release has.
  step "macOS: ad hoc signature verifies"
  codesign --verify "$bin" || fail "the binary's ad hoc signature does not verify"
  step "macOS: the README command clears a browser's quarantine"
  xattr -w com.apple.quarantine "0083;$(printf '%x' "$(date +%s)");Safari;" "$bin"
  xattr -p com.apple.quarantine "$bin" >/dev/null || fail "quarantine was not set"
  xattr -d com.apple.quarantine "$bin"
  if xattr -p com.apple.quarantine "$bin" >/dev/null 2>&1; then
    fail "xattr -d left the quarantine attribute"
  fi
fi

# Runs the binary, keeping stdout, stderr and the exit code apart.
run() {
  set +e
  "$bin" "$@" >"$work/out" 2>"$work/err"
  code=$?
  set -e
}
expect_exit() {
  [ "$code" -eq "$1" ] || { cat "$work/err" >&2; fail "$2: exit $code, want $1"; }
}

export FABRIC_HOME="$work/home"
export CI=1

step "--version is $version"
run --version
expect_exit 0 "--version"
[ "$(cat "$work/out")" = "$version" ] || fail "--version printed $(cat "$work/out"), want $version"
run --version --json
expect_exit 0 "--version --json"
[ "$(cat "$work/out")" = "{\"version\":\"$version\"}" ] || fail "--version --json printed $(cat "$work/out")"

step "help on stdout, exit 0"
run --help
expect_exit 0 "--help"
[ -s "$work/out" ] || fail "--help printed nothing on stdout"

step "a usage error exits 2 with one JSON error on stderr"
run --no-such-option --json
expect_exit 2 "an unknown option"
[ ! -s "$work/out" ] || fail "a failure under --json wrote to stdout"
tail -n 1 "$work/err" | grep -q '"reason":"unknown_option"' || fail "no unknown_option reason on stderr"

step "no node: exit 80, reason no_daemon"
run node id --json
expect_exit 80 "node id without a node"
tail -n 1 "$work/err" | grep -q '"reason":"no_daemon"' || fail "no no_daemon reason on stderr"

step "serve, then the commands that talk to it"
mkdir -p "$FABRIC_HOME"
"$bin" serve --listen 127.0.0.1:0 >"$work/serve.out" 2>"$work/serve.err" &
node_pid=$!
tries=0
until [ -S "$FABRIC_HOME/fabric.sock" ]; do
  tries=$((tries + 1))
  if [ "$tries" -gt 300 ] || ! kill -0 "$node_pid" 2>/dev/null; then
    cat "$work/serve.err" >&2
    fail "serve did not open its control socket"
  fi
  sleep 0.1
done

run node id
expect_exit 0 "node id"
[ "$(wc -l <"$work/out" | tr -d ' ')" = 1 ] || fail "node id printed more than one line"

run job ls
expect_exit 0 "job ls on a new node"
[ ! -s "$work/out" ] || fail "an empty job list wrote to stdout"

run doctor --json
expect_exit 0 "doctor --json"
[ ! -s "$work/err" ] || fail "doctor --json wrote to stderr on success"
grep -q '^{' "$work/out" || fail "doctor --json did not print a JSON document"

echo "smoke: fabric $version for $target passed" >&2
