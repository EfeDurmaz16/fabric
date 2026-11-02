#!/bin/bash
# Fabric's capsule_worker interop tests against the Capsule worker built in /tmp/capsule-wt/m3:
#   node:   packages/bridge-worker/dist/bin.js through Node 22 (the CI job's Node)
#   sea:    the single executable, copied to a fresh directory, with no node, nodejs, bun, pnpm,
#           npm, npx or deno on PATH (PATH holds only a docker link, /usr/bin and /bin)
# each with CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim (already on the engine) and without it.
# The test binary is built first and then run directly, so cargo needs no PATH at run time.
set -u
OUT=$1
BIN=$2
CAP=/tmp/capsule-wt/m3
FAB=/tmp/fabric-wt/m3
echo "capsule $(git -C $CAP rev-parse HEAD) fabric $(git -C $FAB rev-parse HEAD)" > $OUT/heads.txt
echo "capsule worktree clean: $(test -z "$(git -C $CAP status --porcelain)" && echo yes || echo no); fabric worktree clean: $(test -z "$(git -C $FAB status --porcelain)" && echo yes || echo no)" >> $OUT/heads.txt
SEA_DIR=$(mktemp -d)
cp $CAP/packages/bridge-worker/sea/capsule-bridge-worker $SEA_DIR/capsule-bridge-worker
PATH_DIR=$(mktemp -d)
ln -s "$(command -v docker)" $PATH_DIR/docker
MIN_PATH="$PATH_DIR:/usr/bin:/bin"
for tool in node nodejs bun pnpm npm npx deno; do
  if PATH=$MIN_PATH command -v $tool >/dev/null 2>&1; then echo "refusing: $tool is on the minimal PATH"; exit 1; fi
done
echo "sea: $(shasum -a 256 $SEA_DIR/capsule-bridge-worker | cut -d' ' -f1) $(stat -f %z $SEA_DIR/capsule-bridge-worker) bytes; minimal PATH has none of node nodejs bun pnpm npm npx deno" >> $OUT/heads.txt
NODE22=$HOME/.nvm/versions/node/v22.23.2/bin
run() {
  name=$1; shift
  start=$(date +%s)
  (cd $FAB/crates/fabric-bridge && env -i HOME=$HOME "$@" $BIN --ignored --nocapture) > $OUT/$name.log 2>&1
  rc=$?
  end=$(date +%s)
  left=$(docker ps -a -q --filter label=fabric.job | wc -l | tr -d ' ')
  echo "$name: exit $rc ($((end-start)) s) load $(sysctl -n vm.loadavg) containers with fabric.job left: $left; $(grep -E '^test result' $OUT/$name.log | tail -1)" >> $OUT/summary.txt
}
: > $OUT/summary.txt
run node-docker   PATH="$NODE22:$PATH_DIR:/usr/bin:/bin" CAPSULE_BRIDGE_WORKER=$CAP/packages/bridge-worker/dist/bin.js CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim
run node-no-image PATH="$NODE22:$PATH_DIR:/usr/bin:/bin" CAPSULE_BRIDGE_WORKER=$CAP/packages/bridge-worker/dist/bin.js
run sea-docker    PATH="$MIN_PATH" CAPSULE_BRIDGE_WORKER=$SEA_DIR/capsule-bridge-worker CAPSULE_BRIDGE_DOCKER_IMAGE=debian:bookworm-slim
run sea-no-image  PATH="$MIN_PATH" CAPSULE_BRIDGE_WORKER=$SEA_DIR/capsule-bridge-worker
rm -rf $SEA_DIR $PATH_DIR
echo DONE >> $OUT/summary.txt
