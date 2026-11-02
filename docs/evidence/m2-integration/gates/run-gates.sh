#!/bin/bash
# Runs the CONTRIBUTING gates serially and logs each with timing and exit code.
cd /tmp/fabric-wt/m2
OUT=$1
run() { name=$1; shift; echo "== $name: $*" >> $OUT/summary.txt; start=$(date +%s); "$@" > $OUT/$name.log 2>&1; rc=$?; end=$(date +%s); echo "   rc=$rc secs=$((end-start))" >> $OUT/summary.txt; }
run fmt cargo fmt --all --check
run clippy cargo clippy --workspace --all-targets --locked -- -D warnings
for f in none acp claude codex; do ff=$f; [ $f = none ] && ff=""; run clippy-feature-$f cargo clippy -p fabric-harness --all-targets --locked --no-default-features --features "$ff" -- -D warnings; done
run test cargo test --workspace --locked
run e2e-ignored-list cargo test --workspace --locked -- --list --ignored
run doc env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
run deny cargo deny check
run hygiene scripts/check-hygiene.sh origin/main
run budgetcheck scripts/check-budget.sh origin/main
run test-hygiene scripts/test-check-hygiene.sh
run test-budget-script scripts/test-check-budget.sh
run test-fixtures-script scripts/test-check-fixtures.sh
run fixtures scripts/check-fixtures.sh origin/main
run build-fabric cargo build -p fabric --locked
cd sdk/typescript
run sdk-install bun install --frozen-lockfile
run sdk-typecheck bun run typecheck
run sdk-build bun run build
run sdk-test env FABRIC_BIN="$PWD/../../target/debug/fabric" bun test
echo DONE >> $OUT/summary.txt
