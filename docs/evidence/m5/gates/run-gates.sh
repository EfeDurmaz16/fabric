#!/bin/bash
# Runs the repository gates serially on the M5 worktree and logs each with its exit code, time and load.
# Usage: run-gates.sh <out-dir>; CARGO_TARGET_DIR must name a scratch target directory.
# The time budget runs with FABRIC_BUDGET=count: process counts only, no timing.
cd /tmp/fabric-wt/m5
OUT=$1
BASE=5e38dfa
export CARGO_BUILD_JOBS=4
: > $OUT/summary.txt
git rev-parse HEAD > $OUT/commit.txt
echo "load at start: $(sysctl -n vm.loadavg)" >> $OUT/summary.txt
run() { name=$1; shift; start=$(date +%s); "$@" > $OUT/$name.log 2>&1; rc=$?; end=$(date +%s); echo "$name: exit $rc ($((end-start)) s) load $(sysctl -n vm.loadavg)" >> $OUT/summary.txt; }
run fmt cargo fmt --all --check
run clippy cargo clippy --workspace --all-targets --locked -- -D warnings
run clippy-features bash -c 'set -e; for f in "" acp claude codex; do echo "== fabric-harness features [$f]"; cargo clippy -p fabric-harness --all-targets --locked --no-default-features --features "$f" -- -D warnings; done; echo "== fabric-net features [tap]"; cargo clippy -p fabric-net --all-targets --locked --features tap -- -D warnings; echo "== fabricd features [faults]"; cargo clippy -p fabricd --all-targets --locked --features faults -- -D warnings'
run test cargo test --workspace --locked -- --skip e2e_
run e2e cargo test -p fabric --test e2e --locked
run doc env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
run deny cargo deny check
run hygiene bash -c "scripts/check-hygiene.sh $BASE && scripts/test-check-hygiene.sh"
run budget-check bash -c "scripts/check-budget.sh $BASE && scripts/test-check-budget.sh"
run fixtures bash -c "scripts/check-fixtures.sh $BASE && scripts/test-check-fixtures.sh"
run actionlint actionlint .github/workflows/ci.yml .github/workflows/deny.yml
run build-fabric cargo build -p fabric --locked
run cli-reference env DOCS_CLI="$CARGO_TARGET_DIR/debug/fabric" scripts/docs/cli-reference.sh --check
run sdk bash -c 'cd sdk/typescript && bun install --frozen-lockfile && bun run typecheck && bun run build && FABRIC_BIN="$CARGO_TARGET_DIR/debug/fabric" bun test'
run budget-count env FABRIC_BUDGET=count cargo test -p fabric --test e2e --locked -- --ignored --exact budget::hot_paths --nocapture
echo DONE >> $OUT/summary.txt
