## What

<!-- The change in a few plain sentences. -->

## Why

<!-- The problem it solves. -->

## Design notes

<!-- Decisions and trade-offs. Link the ADRs. -->

## How to review

<!-- Files in reading order, one line each. -->

## Verification

<!-- Exact commands and their summary lines. -->

```text
$ cargo fmt --all --check
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
$ scripts/check-hygiene.sh origin/main
```

## Dependencies added

<!-- One row per crate, or "None". -->

| Crate | Version | Last release | License | Why |
| ----- | ------- | ------------ | ------- | --- |
