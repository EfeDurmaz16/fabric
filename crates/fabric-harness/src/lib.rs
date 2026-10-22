//! Harness adapters (ADR 0006, ADR 0015).
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
//! would lose bytes that did not leave with the job: a file the harness
//! rewrites in place is replaced only when this node left it untouched.
//! Until every file passes, nothing is written.
//!
//! The crate is organised around one trait, [`Harness`], and three
//! adapters behind cargo features: [`acp`], [`claude`] and [`codex`].
//! [`manifest::StateManifest`] is the file that travels between the two.

mod crashpoint;
pub mod denylist;
pub(crate) mod departure;
pub(crate) mod durable;
mod error;
mod harness;
pub(crate) mod jsontext;
pub mod manifest;
pub(crate) mod markers;
#[cfg(any(feature = "acp", feature = "claude", feature = "codex"))]
pub(crate) mod pipeline;
pub(crate) mod place;
pub(crate) mod process;
pub(crate) mod redact;
mod session;
pub(crate) mod stage;
mod version;

#[cfg(feature = "acp")]
pub mod acp;
#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "codex")]
pub mod codex;

pub use error::{BadRule, HarnessError, Input, IoOp};
pub use harness::{
    Harness, Installed, Launch, LaunchSettings, MAX_MODEL_NAME_LEN, ModelName, ModelNameError,
    NotCarried, Origin, PassedSetting, Restored, Running, SettingName, TurnReport,
};
pub use session::{MAX_SESSION_ID_LEN, SessionId, SessionIdError};
pub use version::{HarnessVersion, VersionError};
