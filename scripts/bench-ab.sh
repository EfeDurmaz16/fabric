#!/bin/sh
# Compares two builds of fabric op by op on the e2e fixture: the anchor,
# main before the campaign by default, and a candidate, this working tree
# by default. Both run the same ops in the same order, taking turns op by
# op, with the build that goes first alternating from run to run, so the
# machine's load reaches both alike (crates/fabric/tests/e2e/ab.rs says
# what each op is). Two phases: a counted one, where every fabric process
# keeps counters of the work that does not depend on load (processes
# started, file flushes, bytes read and written by class of file,
# allocations, peak resident size, CPU time), and a timed one of paired
# runs. The report gives each op's median ratio, candidate over anchor,
# with a 95% bootstrap interval, then the counters.
#
# The performance rule: no op may be slower than the anchor. A ratio whose
# interval lies above 1 is a regression to fix, whatever the machine's load,
# since load reaches both builds of a pair alike.
#
# The anchor is built once per commit, profile and toolchain, from a copy of
# that commit, and kept under FABRIC_AB_CACHE (default
# $XDG_CACHE_HOME/fabric-ab, or ~/.cache/fabric-ab). A candidate given as a
# commit is built and kept the same way; otherwise it is this working tree,
# built in its own target directory. The test binary of this working tree
# drives both builds. Both are copied before the run, so a build during the
# run changes nothing. The counters need macOS and a C compiler; elsewhere
# the counted phase is skipped.
#
# Usage:
#   scripts/bench-ab.sh [options] <output-dir>
#   scripts/bench-ab.sh --analyze <output-dir>
# Options:
#   --anchor <commit>     the anchor (default 176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5)
#   --candidate <commit>  a commit instead of the working tree
#   --pairs <n>           timed pairs (default 15, the fewest the rule accepts);
#                         0 runs only the counted phase, whose counters do not
#                         depend on load, so it is the one to run on a busy
#                         machine
#   --counted <n>         counted runs (default 3)
#   --release             build both in the release profile instead of test
# The output directory must not exist yet, so an earlier record is never
# overwritten. --analyze writes report.md again from its samples.tsv.
set -eu

anchor=176622eb4d4e5b5fdfaaae7241b678ce1c60b5d5
candidate=
pairs=15
counted=3
release=
analyze=

usage() {
  echo "usage: scripts/bench-ab.sh [--anchor <commit>] [--candidate <commit>] [--pairs <n>] [--counted <n>] [--release] <output-dir>" >&2
  echo "       scripts/bench-ab.sh --analyze <output-dir>" >&2
  exit 2
}

while [ "$#" -gt 1 ]; do
  case $1 in
    --anchor) anchor=$2; shift 2 ;;
    --candidate) candidate=$2; shift 2 ;;
    --pairs) pairs=$2; shift 2 ;;
    --counted) counted=$2; shift 2 ;;
    --release) release=--release; shift ;;
    --analyze) analyze=1; shift ;;
    *) usage ;;
  esac
done
[ "$#" -eq 1 ] || usage
case $1 in -*) usage ;; esac
out=$1

toplevel=$(git rev-parse --show-toplevel)
cd "$toplevel"

if [ -n "$release" ]; then
  profile=release
else
  profile=debug
fi

# The e2e test binary of this working tree, built if needed.
driver() {
  cargo test -p fabric --test e2e --locked --no-run $release --message-format=json 2>/dev/null |
    sed -n 's/.*"executable":"\([^"]*\/e2e-[^"]*\)".*/\1/p' | tail -n 1
}

if [ -n "$analyze" ]; then
  [ -f "$out/samples.tsv" ] || { echo "bench-ab: no $out/samples.tsv" >&2; exit 2; }
  e2e=$(driver)
  out=$(cd "$out" && pwd)
  FABRIC_AB_OUT=$out "$e2e" --ignored --exact ab::analyze --nocapture --test-threads=1 >/dev/null
  echo "bench-ab: wrote $out/report.md" >&2
  exit 0
fi

if [ -e "$out" ]; then
  echo "bench-ab: $out exists; pick a new directory" >&2
  exit 2
fi
case $pairs in '' | *[!0-9]*) usage ;; esac
case $counted in '' | *[!0-9]*) usage ;; esac
# The counted phase needs macOS, for DYLD_INSERT_LIBRARIES, and a C compiler.
can_count=
if [ "$(uname -s)" = Darwin ] && command -v cc >/dev/null 2>&1; then
  can_count=1
fi
if [ "$pairs" -eq 0 ]; then
  if [ -z "$can_count" ]; then
    echo "bench-ab: the counters need macOS and cc, and with no timed pairs there is nothing to run" >&2
    exit 2
  fi
  echo "bench-ab: no timed pairs; only the counted phase runs, and the report has no times" >&2
elif [ "$pairs" -lt 15 ]; then
  echo "bench-ab: $pairs pairs is fewer than the 15 the rule accepts; the report is not evidence" >&2
fi

cache=${FABRIC_AB_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/fabric-ab}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# Builds `fabric` at commit $1, as the e2e test builds it (the fixture
# features on), unless the cache has it, and prints its path in the cache.
build_commit() {
  sha=$(git rev-parse --verify "$1^{commit}")
  src=$scratch/src-$sha
  mkdir -p "$src"
  git archive "$sha" | tar -x -C "$src"
  toolchain=$(cd "$src" && rustc -V | tr ' ' '-' | tr -cd 'A-Za-z0-9.-')
  kept=$cache/$sha-$profile-$toolchain
  if [ ! -x "$kept/fabric" ]; then
    echo "bench-ab: building $sha ($profile, $toolchain); it is kept in $kept" >&2
    (cd "$src" && CARGO_TARGET_DIR=$scratch/target-$sha cargo test -p fabric --test e2e --locked --no-run $release) \
      >"$scratch/build-$sha.log" 2>&1 || {
      tail -n 30 "$scratch/build-$sha.log" >&2
      echo "bench-ab: building $sha failed" >&2
      exit 1
    }
    mkdir -p "$kept"
    cp "$scratch/target-$sha/$profile/fabric" "$kept/fabric.new"
    mv "$kept/fabric.new" "$kept/fabric"
    rm -rf "$scratch/target-$sha"
  fi
  rm -rf "$src"
  echo "$kept/fabric"
}

anchor_sha=$(git rev-parse --verify "$anchor^{commit}")
anchor_bin=$(build_commit "$anchor_sha")

echo "bench-ab: building this working tree's e2e test" >&2
e2e=$(driver)
[ -x "$e2e" ] || { echo "bench-ab: no e2e test binary; run cargo test -p fabric --test e2e --no-run" >&2; exit 1; }
if [ -n "$candidate" ]; then
  candidate_sha=$(git rev-parse --verify "$candidate^{commit}")
  candidate_bin=$(build_commit "$candidate_sha")
  candidate_desc="$candidate_sha"
else
  target=$(cargo metadata --format-version 1 --no-deps --locked | sed 's/.*"target_directory":"\([^"]*\)".*/\1/')
  candidate_bin=$target/$profile/fabric
  candidate_desc="working tree at $(git rev-parse HEAD), $(git status --porcelain | wc -l | tr -d ' ') changed paths"
fi

# Copies, so a build during the run cannot swap a binary under it. Each
# keeps the name fabric, which the counter library looks for.
mkdir -p "$scratch/anchor" "$scratch/candidate"
cp "$anchor_bin" "$scratch/anchor/fabric"
cp "$candidate_bin" "$scratch/candidate/fabric"
"$scratch/anchor/fabric" --version >/dev/null
"$scratch/candidate/fabric" --version >/dev/null

counters=
if [ -n "$can_count" ]; then
  cc -dynamiclib -O2 -Wall -Wextra -Werror -o "$scratch/libcounters.dylib" scripts/perf-ab/counters.c
  counters=$scratch/libcounters.dylib
else
  echo "bench-ab: the counters need macOS and cc; no counted phase" >&2
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)
size() { wc -c <"$1" | tr -d ' '; }
{
  echo "anchor: $anchor_sha"
  echo "candidate: $candidate_desc"
  echo "driver: e2e test of the working tree at $(git rev-parse HEAD)"
  echo "profile: $profile, pairs: $pairs, counted runs: $counted"
  echo "binary bytes: anchor $(size "$scratch/anchor/fabric"), candidate $(size "$scratch/candidate/fabric")"
  echo "uname: $(uname -srm)"
  if command -v sysctl >/dev/null 2>&1; then
    echo "cpu: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || true) $(sysctl -n hw.ncpu 2>/dev/null || true) cores"
  fi
  echo "rustc: $(rustc -V)"
  echo "start: $(date -u +%Y-%m-%dT%H:%M:%SZ), load: $(uptime | sed 's/.*load average[s]*: //')"
} >"$out/machine.txt"

echo "bench-ab: running $pairs pairs and $counted counted runs; see $out/run.log" >&2
status=0
# env, not plain assignments: the optional counters assignment is an
# expansion, which only env reads as one.
env FABRIC_AB_ANCHOR="$scratch/anchor/fabric" \
  FABRIC_AB_CANDIDATE="$scratch/candidate/fabric" \
  FABRIC_AB_OUT="$out" \
  FABRIC_AB_PAIRS="$pairs" \
  FABRIC_AB_COUNTED="$counted" \
  ${counters:+FABRIC_AB_COUNTERS=$counters} \
  "$e2e" --ignored --exact ab::paired --nocapture --test-threads=1 >"$out/run.log" 2>&1 || status=$?
echo "end: $(date -u +%Y-%m-%dT%H:%M:%SZ), load: $(uptime | sed 's/.*load average[s]*: //')" >>"$out/machine.txt"
if [ "$status" -ne 0 ]; then
  tail -n 40 "$out/run.log" >&2
  echo "bench-ab: the run failed; its output is in $out/run.log" >&2
  exit "$status"
fi
echo "bench-ab: wrote $out/report.md" >&2
