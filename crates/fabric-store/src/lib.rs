//! The store half of Fabric (ADR 0004): content-addressed objects, the
//! `SQLite` catalog, and workspace capture and restore.
//!
//! Invariant: nothing the catalog names is missing from disk, and nothing a
//! restore writes lands outside the directory it was asked to create. Every
//! object is written to a temp file, flushed and linked into place under its
//! blake3 name, never rewritten; the catalog gets its rows only after the
//! shard directories are flushed; gc deletes a row before its file. Capture
//! turns a workspace into a `SnapshotManifest` from `fabric-core` whose
//! objects are all in the store, and restore turns a manifest back into a
//! directory, with the git state carried as commits and refs, the index
//! split kept, modes and mtimes exact, and secrets never present.
//!
//! The crate does IO and runs `git`; it knows nothing about the network,
//! the job ledger or any harness. A caller publishes a manifest after
//! capture and records the snapshot in the ledger afterwards, so the ledger
//! stays the commit point (ADR 0002).

pub mod capture;
pub mod catalog;
mod checkpoint;
mod durable;
pub mod git;
pub mod objects;
pub mod policy;
pub mod restore;
pub mod store;
pub mod writer;

#[cfg(test)]
mod crash_tests;
