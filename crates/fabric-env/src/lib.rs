//! The environment of a Fabric job (ADR 0007, ADR 0012).
//!
//! Detection reads the files a workspace already has, plus the user's
//! `.fabric/env.toml`, and builds an `EnvSpec`: tool pins, setup steps,
//! environment variable names and cache paths. The materializer checks a
//! restored workspace against that spec and runs setup steps only when a
//! cache is missing or torn. Detection never opens a file the job's secret
//! policy (`fabric_core::secret`) denies.
//!
//! The crate reads `PATH` to find programs and checks only whether a
//! declared environment variable is set; it never stores or logs a
//! variable's value, never copies a credential and never runs a program
//! through a shell. What a setup step prints is another matter: each line goes to
//! the log as the step printed it, so a declared `sh -c` step or an
//! installer that echoes a token puts that token in the log. It targets
//! Unix: the materializer reads execute bits to find a program.

pub mod detect;
pub mod materialize;
pub mod overrides;
