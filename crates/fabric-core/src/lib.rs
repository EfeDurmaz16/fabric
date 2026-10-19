//! Core types of Fabric: the job record, its append-only ledger, the
//! versioned manifest, the lease state machine, and the `Store`, `Transport`
//! and `Harness` traits that the other crates implement.
//!
//! This crate depends on nothing else in the workspace and knows nothing about
//! QUIC, a database, or any agent vendor. Everything above it is an adapter.
