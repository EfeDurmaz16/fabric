//! OpenCode: a job under `--harness opencode` is suspended on one node and
//! resumed on the other with a fake `opencode` that keeps the agent's real
//! on-disk layout, and the refusals of docs/harness-adapters.md hold. Owner
//! A of PR 5 fills this file, and installs its fake from here through
//! `Env::install`, so `world.rs` stays untouched.
