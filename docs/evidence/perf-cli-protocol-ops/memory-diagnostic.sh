#!/bin/sh
# The daemon memory diagnostic of README.md: starts one daemon per build
# side by side, runs the same rounds against each, each round `job new`,
# `job suspend`, the workspace removed and `job resume` (a restore), and
# prints each daemon's resident size, its threads and its vmmap regions
# as resident/dirty. macOS only (vmmap).
#
# Usage: memory-diagnostic.sh <rounds> <label>=<fabric binary> ...
set -u
rounds=$1
shift
root=$(mktemp -d /tmp/frss.XXXX)
mkdir -p "$root/repo"
(
  cd "$root/repo" &&
    git init -q &&
    git config user.email t@t &&
    git config user.name t &&
    echo '{"name":"x"}' >package.json &&
    echo hi >a &&
    git add . &&
    git commit -qm init
)
for spec in "$@"; do
  label=${spec%%=*}
  bin=${spec#*=}
  dir=$root/$label
  mkdir -p "$dir/h" "$dir/s"
  chmod 700 "$dir/s"
  HOME=$dir/h FABRIC_HOME=$dir/s "$bin" serve --listen 127.0.0.1:0 >"$dir/log" 2>&1 &
  echo $! >"$dir/pid"
done
sleep 3
round=0
while [ "$round" -lt "$rounds" ]; do
  round=$((round + 1))
  for spec in "$@"; do
    label=${spec%%=*}
    bin=${spec#*=}
    dir=$root/$label
    job=$(HOME=$dir/h FABRIC_HOME=$dir/s "$bin" job new --repo "$root/repo" --goal g --harness codex 2>&1 |
      sed -n '1s/^job //p' | awk '{print $1}')
    HOME=$dir/h FABRIC_HOME=$dir/s "$bin" job suspend "$job" >/dev/null 2>&1
    rm -rf "$dir/h/fabric/jobs/$job/work"
    HOME=$dir/h FABRIC_HOME=$dir/s "$bin" job resume "$job" >/dev/null 2>&1
  done
done
sleep 1
for spec in "$@"; do
  label=${spec%%=*}
  pid=$(cat "$root/$label/pid")
  threads=$(ps -M -p "$pid" | tail -n +2 | wc -l | tr -d ' ')
  printf '%s rss=%s threads=%s ' "$label" "$(ps -o rss= -p "$pid" | tr -d ' ')" "$threads"
  vmmap --summary "$pid" 2>/dev/null |
    awk '/^(MALLOC_SMALL|MALLOC_TINY|Stack|__TEXT|__DATA|__DATA_CONST|__DATA_DIRTY|__LINKEDIT|TOTAL) / {printf "%s=%s/%s ", $1, $3, $4} END {print ""}'
done
for spec in "$@"; do
  label=${spec%%=*}
  kill "$(cat "$root/$label/pid")"
done
rm -rf "$root"
