//! Core types of Fabric: ids, the job ledger and its lease, the move
//! protocol, snapshot manifests and the environment spec.
//!
//! This crate is pure. It does no IO, holds no clock and opens no socket.
//! It depends on nothing else in the workspace and knows nothing about QUIC,
//! a database or any agent vendor. Everything above it is an adapter that
//! feeds it bytes and persists what it returns.

pub mod id;
pub mod lease;
pub mod path;
