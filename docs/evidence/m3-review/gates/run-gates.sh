#!/bin/bash
# Runs the repository gates serially on /tmp/fabric-wt/m3 and logs each with its exit code and time.
cd /tmp/fabric-wt/m3
OUT=$1
BASE=origin/m2/durable-job-foundations
: > $OUT/summary.txt
git rev-parse HEAD > $OUT/commit.txt
run() { name=$1; shift; start=$(date +%s); "$@" > $OUT/$name.log 2>&1; rc=$?; end=$(date +%s); echo "$name: exit $rc (${start}..${end}, $((end-start)) s)" >> $OUT/summary.txt; }
run fmt cargo fmt --all --check
run clippy cargo clippy --workspace --all-targets --locked -- -D warnings
run clippy-features bash -c 'set -e; for f in "" acp claude codex; do echo "== features [$f]"; cargo clippy -p fabric-harness --all-targets --locked --no-default-features --features "$f" -- -D warnings; done'
run test cargo test --workspace --locked -- --skip e2e_
run e2e cargo test -p fabric --test e2e --locked
run doc env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
run deny cargo deny check
run hygiene bash -c "scripts/check-hygiene.sh $BASE && scripts/test-check-hygiene.sh"
run budget-check bash -c "scripts/check-budget.sh $BASE && scripts/test-check-budget.sh"
run fixtures bash -c "scripts/check-fixtures.sh $BASE && scripts/test-check-fixtures.sh"
run actionlint actionlint .github/workflows/ci.yml .github/workflows/deny.yml
run build-fabric cargo build -p fabric --locked
run sdk bash -c 'cd sdk/typescript && bun install --frozen-lockfile && bun run typecheck && bun run build && FABRIC_BIN="$PWD/../../target/debug/fabric" bun test'
echo DONE >> $OUT/summary.txt
