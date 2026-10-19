//! The environment of a Fabric job (ADR 0007, ADR 0012).
//!
//! Detection reads the files a workspace already has, plus the user's
//! `.fabric/env.toml`, and builds an `EnvSpec`: tool pins, setup steps,
//! environment variable names and cache paths. The secret policy is the one
//! list of path patterns that never leave a node; snapshot capture applies
//! it too.
//!
//! The crate never stores or logs the value of an environment variable and
//! never copies a credential.

pub mod detect;
pub mod overrides;
pub mod secret;
