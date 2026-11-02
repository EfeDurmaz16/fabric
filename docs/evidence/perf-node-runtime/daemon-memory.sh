#!/bin/sh
# Measures what a daemon holds, anchor against candidate, in pairs. Each
# run starts `fabric serve` on a fresh state directory and home, asks it
# `node id` and `job ls` over the control socket and, with `ops`, makes a
# job from a small repository and suspends it twice with an edit between,
# so the store captures and publishes and the control socket streams
# events. It then reads the daemon's resident size, its physical footprint
# (the private memory macOS charges it, without clean code pages), the
# resident code pages of the fabric binary itself, the bytes its default
# malloc zone holds, and its thread count, and stops it. The builds take
# turns, the one that goes first alternating from pair to pair (ABAB), so
# the machine's load reaches both alike. Prints every run, then each
# measure's median ratio of the pairs, candidate over anchor, with a 95%
# percentile bootstrap interval (10,000 resamples, fixed seed). macOS
# only: it reads `vmmap`.
#
# Usage: daemon-memory.sh <anchor fabric> <candidate fabric> [pairs] [idle|ops]
set -eu

[ "$#" -ge 2 ] || {
  echo "usage: daemon-memory.sh <anchor fabric> <candidate fabric> [pairs] [idle|ops]" >&2
  exit 2
}
anchor=$1
candidate=$2
pairs=${3:-20}
mode=${4:-idle}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch" 2>/dev/null || true' EXIT

# The repository a job is made from: 40 files of 16 KiB, committed.
repo=$scratch/repo
mkdir -p "$repo"
i=0
while [ "$i" -lt 40 ]; do
  head -c 16384 /dev/urandom >"$repo/f$i.bin"
  i=$((i + 1))
done
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$repo" init -q -b main
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$repo" add -A
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=bench \
  GIT_AUTHOR_EMAIL=bench@example.invalid GIT_COMMITTER_NAME=bench \
  GIT_COMMITTER_EMAIL=bench@example.invalid git -C "$repo" commit -q -m one

# KiB of a vmmap size such as 3088K or 2.5M.
kib() {
  awk '{ v = $1; u = substr(v, length(v)); n = substr(v, 1, length(v) - 1)
         if (u == "M") n *= 1024; else if (u == "G") n *= 1048576; else if (u != "K") n = v / 1024
         print int(n) }'
}

# One run of build $1: prints resident, footprint, code and heap KiB, and threads.
measure() {
  # A directory of its own: a run's files can still be settling when the
  # next one starts.
  run=$(mktemp -d "$scratch/run.XXXXXX")
  mkdir -p "$run/home"
  export HOME="$run/home" FABRIC_HOME="$run/state"
  "$1" serve --listen 127.0.0.1:0 --wire tcp-tls >"$run/out" 2>"$run/err" &
  pid=$!
  tries=0
  until grep -q '^listening on' "$run/out" 2>/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 500 ]; then
      echo "daemon-memory: $1 did not start" >&2
      cat "$run/err" >&2
      exit 1
    fi
    sleep 0.01
  done
  "$1" node id >/dev/null
  "$1" job ls >/dev/null
  if [ "$mode" = ops ]; then
    job=$("$1" job new --repo "$repo" --goal bench --harness codex | head -n 1)
    work=$run/home/fabric/jobs/$job/work
    "$1" job suspend "$job" >/dev/null
    head -c 16384 /dev/urandom >"$work/f0.bin"
    echo edit >"$work/new.txt"
    "$1" job suspend "$job" >/dev/null
    "$1" job log "$job" >/dev/null
    "$1" job status "$job" >/dev/null
  fi
  sleep 0.5
  rss=$(ps -o rss= -p "$pid" | tr -d ' ')
  threads=$(($(ps -M -p "$pid" | wc -l) - 1))
  vmmap "$pid" >"$run/vmmap" 2>/dev/null || true
  footprint=$(awk '/^Physical footprint:/ { print $3; exit }' "$run/vmmap" | kib)
  # The resident column of the binary's own __TEXT mapping.
  code=$(awk '$1 == "__TEXT" && $NF ~ /\/fabric$/ { sub(/.*\[ */, ""); print $2; exit }' "$run/vmmap" | kib)
  heap=$(awk '/^DefaultMallocZone/ { print $7; exit }' "$run/vmmap" | kib)
  kill -TERM "$pid"
  wait "$pid" || true
  echo "$rss ${footprint:-0} ${code:-0} ${heap:-0} $threads"
}

printf 'pair\tarm\trss_kib\tfootprint_kib\tcode_kib\theap_kib\tthreads\n'
results=$scratch/results
: >"$results"
pair=0
while [ "$pair" -lt "$pairs" ]; do
  if [ $((pair % 2)) -eq 0 ]; then
    order="anchor candidate"
  else
    order="candidate anchor"
  fi
  for arm in $order; do
    if [ "$arm" = anchor ]; then bin=$anchor; else bin=$candidate; fi
    measured=$(measure "$bin")
    printf '%s\t%s\t%s\n' "$pair" "$arm" "$measured" | tr ' ' '\t'
    printf '%s %s %s\n' "$pair" "$arm" "$measured" >>"$results"
  done
  pair=$((pair + 1))
done

median() { sort -n | awk '{ v[NR] = $1 } END { print (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2 }'; }

# The pairs' ratios of column $1 of the results, then the medians of
# 10,000 resamples of them, and the line that reports them as $2.
summary() {
  awk -v col="$1" '
    { if ($2 == "anchor") { a[$1] = $col } else { c[$1] = $col } }
    END { for (p in a) if (a[p] > 0) print c[p] / a[p] }
  ' "$results" >"$scratch/ratios"
  awk -v seed=20261027 '
    { r[NR] = $1 }
    END {
      srand(seed)
      n = NR
      for (b = 1; b <= 10000; b++) {
        for (i = 1; i <= n; i++) {
          t = r[int(rand() * n) + 1]
          for (j = i - 1; j >= 1 && x[j] > t; j--) x[j + 1] = x[j]
          x[j + 1] = t
        }
        print (n % 2) ? x[(n + 1) / 2] : (x[n / 2] + x[n / 2 + 1]) / 2
      }
    }
  ' "$scratch/ratios" | sort -n >"$scratch/medians"
  printf '%s (medians): anchor %s, candidate %s; candidate/anchor median %s [%s-%s] over %s pairs\n' "$2" \
    "$(awk -v col="$1" '$2 == "anchor" { print $col }' "$results" | median)" \
    "$(awk -v col="$1" '$2 == "candidate" { print $col }' "$results" | median)" \
    "$(median <"$scratch/ratios")" "$(sed -n 251p "$scratch/medians")" "$(sed -n 9750p "$scratch/medians")" "$pairs"
}

summary 3 'resident KiB'
summary 4 'footprint KiB'
summary 5 'code KiB'
summary 6 'heap KiB'
summary 7 'threads'
