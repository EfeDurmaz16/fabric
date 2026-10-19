//! Harness adapters (ADR 0006).
//!
//! A harness is the agent that does the work: Claude Code, Codex, or any
//! agent that speaks the Agent Client Protocol. Each one keeps its session
//! state in its own files under the user's home directory and keys that
//! state to the absolute path of the workspace. This crate knows those
//! layouts so that nothing else in Fabric has to.
//!
//! The invariant this crate protects: a captured state tree never holds a
//! credential, a restore never destroys state that is already on disk, and
//! a path rewrite never touches a signed block.
//!
//! The crate is organised around one trait, [`Harness`], and three
//! adapters behind cargo features: [`acp`], [`claude`] and [`codex`].
//! [`manifest::StateManifest`] is the file that travels between the two.

pub mod denylist;
pub mod error;
pub mod files;
pub mod harness;
pub mod jsontext;
pub mod manifest;
pub mod process;
pub mod rewrite;
pub mod session;

#[cfg(feature = "acp")]
pub mod acp;
#[cfg(feature = "claude")]
pub mod claude;

pub use error::HarnessError;
pub use harness::{Harness, Installed, Io, Launch, Running};
pub use manifest::StateManifest;
pub use session::SessionId;
