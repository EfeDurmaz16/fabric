//! Harness adapters (ADR 0006).
//!
//! A harness is the agent that does the work: Claude Code, Codex, or any
//! agent that speaks the Agent Client Protocol. Each one keeps its session
//! state in its own files under the user's home directory and keys that
//! state to the absolute path of the workspace. This crate knows those
//! layouts so that nothing else in Fabric has to.
//!
//! The invariant this crate protects: a captured state tree holds no file
//! on the credential denylist and no string the standard redactor matches,
//! because Claude state is redacted (a signed block that holds a secret is
//! dropped whole, never edited) and Codex and ACP state, which travels byte
//! for byte, is refused when a secret shape is found in it. A restore
//! writes only under the captured workspace path and home (ADR 0011), only
//! where the adapter captures from, and only when no file already on disk
//! would lose bytes; until every file passes, nothing is written.
//!
//! The crate is organised around one trait, [`Harness`], and three
//! adapters behind cargo features: [`acp`], [`claude`] and [`codex`].
//! [`manifest::StateManifest`] is the file that travels between the two.
//! [`jsontext`] and [`redact`] are public because the property test in
//! `tests/jsontext_props.rs` states the signed-block rule against them.

pub mod denylist;
pub mod error;
pub(crate) mod files;
pub mod harness;
pub mod jsontext;
pub mod manifest;
pub(crate) mod process;
pub mod redact;
pub mod session;

#[cfg(feature = "acp")]
pub mod acp;
#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "codex")]
pub mod codex;

pub use error::HarnessError;
pub use harness::{Harness, Installed, Io, Launch, Restored, Running};
pub use manifest::StateManifest;
pub use session::SessionId;
