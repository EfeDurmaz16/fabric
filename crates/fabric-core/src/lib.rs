//! Core types of Fabric. Empty until the first job and lease types land; the
//! architecture ADRs describe the intended shape.
//!
//! This crate depends on nothing else in the workspace and knows nothing about
//! QUIC, a database, or any agent vendor. Everything above it is an adapter.
