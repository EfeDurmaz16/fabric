//! The environment of a Fabric job (ADR 0007, ADR 0012).
//!
//! The secret policy is the one list of path patterns that never leave a
//! node. Snapshot capture applies it to ignored files and to the
//! working-tree commit (ADR 0004).
//!
//! The crate never stores or logs the value of an environment variable and
//! never copies a credential.

pub mod secret;
