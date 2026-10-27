//! The bridge from a Fabric node to a provider worker (ADR 0021,
//! docs/bridge-protocol.md): protocol v1's messages and their framing.
//!
//! Ownership does not cross the bridge. Fabric owns job, task, attempt
//! and operation identity, the journal, placement and fencing; the worker
//! owns the provider mapping, provider calls, observations and receipts.

pub mod frames;
pub mod wire;
