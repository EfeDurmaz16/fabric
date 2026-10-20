//! The one error type of this crate.
//!
//! Invariant: every variant names a failure the caller can act on, with the
//! path, program or session it concerns, so the CLI can print what to do
//! next without parsing a message.

use std::path::PathBuf;

use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;

use crate::session::SessionId;

/// Why an adapter operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// The harness binary is not on `PATH` and no path was configured.
    #[error("{program} is not installed (not found on PATH)")]
    NotInstalled {
        /// The program name that was looked up.
        program: String,
    },
    /// The binary ran but its version output had no version in it.
    #[error("{program} --version printed something unexpected: {output:?}")]
    UnexpectedVersionOutput {
        /// The program that was run.
        program: String,
        /// What it printed, trimmed.
        output: String,
    },
    /// The workspace path is not absolute or does not exist as a directory.
    #[error("workspace {path} is not an existing absolute directory")]
    BadWorkspace {
        /// The path that was given.
        path: PathBuf,
    },
    /// A directory an adapter was configured with is not under the home it serves.
    #[error("{path} is not under home {home}")]
    NotUnderHome {
        /// The directory that was given.
        path: PathBuf,
        /// The home directory of the adapter.
        home: PathBuf,
    },
    /// A manifest captured under one workspace path is restored under another (ADR 0011).
    #[error(
        "state was captured for workspace {from}, not {to}; the root must be the same path on every node"
    )]
    RootMismatch {
        /// The workspace path at capture.
        from: AbsPath,
        /// The workspace path given to restore.
        to: AbsPath,
    },
    /// A manifest captured under one home directory is restored under another (ADR 0011).
    #[error(
        "state was captured under home {from}, not {to}; the home must be the same path on every node"
    )]
    HomeMismatch {
        /// The home at capture.
        from: AbsPath,
        /// The home of the restoring adapter.
        to: AbsPath,
    },
    /// The workspace path is so long that Claude's project key would carry an undocumented hash.
    #[error(
        "the Claude project key of {path} would be {len} characters; keys over 200 are not supported"
    )]
    ProjectKeyTooLong {
        /// The workspace path.
        path: PathBuf,
        /// The key length.
        len: usize,
    },
    /// A manifest lists a file outside the places the adapter captures from.
    #[error("refusing to restore {path}: the {kind} adapter never captures a file there")]
    UnexpectedFile {
        /// The harness restoring.
        kind: HarnessKind,
        /// The tree path from the manifest.
        path: String,
    },
    /// A file operation failed.
    #[error("{op} {path}: {source}")]
    Io {
        /// What was being done: `read`, `write`, `create dir`, `walk`, `rename`, `fsync`, `lock`, `canonicalize`.
        op: &'static str,
        /// The path concerned.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Starting a process failed.
    #[error("spawn {program}: {source}")]
    Spawn {
        /// The program that was started.
        program: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The thread that ran a capture or restore ended without an answer.
    #[error("the task running the operation was cancelled or panicked: {source}")]
    BlockingTask {
        /// The underlying error.
        #[source]
        source: tokio::task::JoinError,
    },
    /// A file an adapter wanted to carry is on the credential and host-state denylist.
    #[error("refusing to carry {path}: {rule}")]
    Denied {
        /// The path, relative to the state root it lives under.
        path: String,
        /// The denylist rule it matched.
        rule: crate::denylist::DenyRule,
    },
    /// Two files of one capture or restore map to the same path.
    #[error("two files would be written as {path}")]
    DuplicateTreePath {
        /// The tree path.
        path: String,
    },
    /// A symlink sits inside harness state, where none belongs.
    #[error("{path} is a symlink; harness state is carried as regular files only")]
    SymlinkInState {
        /// The symlink.
        path: PathBuf,
    },
    /// A file carried byte for byte holds a secret-shaped string.
    #[error("{path} line {line} holds a secret; it cannot be carried byte for byte")]
    SecretInState {
        /// The file concerned.
        path: PathBuf,
        /// One-based line number.
        line: usize,
    },
    /// State already on disk is neither equal to nor a prefix of the incoming file.
    #[error("harness state at {path} has diverged from the incoming copy; nothing was restored")]
    StateDiverged {
        /// The file on this node that would have been overwritten.
        path: PathBuf,
    },
    /// A staged file does not match the length and digest its manifest records.
    #[error("staged file {path} is corrupt: {reason}")]
    StateCorrupt {
        /// The staged file.
        path: PathBuf,
        /// What did not match.
        reason: String,
    },
    /// The named session has no state on this node.
    #[error("{kind} session {session} not found under {searched}")]
    SessionNotFound {
        /// The harness that was asked.
        kind: HarnessKind,
        /// The session that was looked for.
        session: SessionId,
        /// The directory that was searched.
        searched: PathBuf,
    },
    /// A paginated Codex thread refers to a prefix rollout that is missing.
    #[error("rollout lineage of {session} is broken: prefix rollout {missing} is missing")]
    LineageBroken {
        /// The thread whose lineage was followed.
        session: SessionId,
        /// The rollout id that could not be found.
        missing: SessionId,
    },
    /// Two rollout files share one rollout id and neither is a prefix of the other.
    #[error("rollout {rollout} of {session} exists as two different files: {paths:?}")]
    LineageAmbiguous {
        /// The thread whose lineage was followed.
        session: SessionId,
        /// The rollout id that appears twice.
        rollout: SessionId,
        /// The files that disagree.
        paths: Vec<PathBuf>,
    },
    /// Another process holds the harness's own writer lock for the session.
    #[error("{kind} session {session} has an active writer on this node; stop it first")]
    ActiveWriter {
        /// The harness.
        kind: HarnessKind,
        /// The session.
        session: SessionId,
    },
    /// A harness process is running, so its files must not be edited now.
    #[error("{program} is running (pids {pids:?}); retry when it has exited")]
    HarnessRunning {
        /// The program seen running.
        program: String,
        /// Its process ids.
        pids: Vec<u32>,
    },
    /// A captured manifest or a vendor file did not parse.
    #[error("{what} at {path} does not parse: {reason}")]
    Malformed {
        /// What was being parsed: `manifest`, `rollout line`, `config json`, `state path`, `workspace path`.
        what: &'static str,
        /// The file concerned.
        path: PathBuf,
        /// The parser's message.
        reason: String,
    },
    /// The manifest was captured by another kind of harness than the one restoring it.
    #[error("manifest was captured by {captured}, not {restoring}")]
    WrongHarness {
        /// The harness that captured the state.
        captured: HarnessKind,
        /// The harness asked to restore it.
        restoring: HarnessKind,
    },
    /// A secret matched inside a signed block that is not an array element, so it cannot be dropped.
    #[error("{path} line {line}: a secret sits inside a signed block that cannot be dropped")]
    SecretInSignedBlock {
        /// The file concerned.
        path: PathBuf,
        /// One-based line number.
        line: usize,
    },
    /// A headless run was asked for without the prompt the harness needs to start a turn.
    #[error("a headless {program} run needs a prompt; pass one or use the terminal")]
    HeadlessNeedsPrompt {
        /// The harness program.
        program: String,
    },
    /// A protocol agent was asked to run in a terminal, which it cannot do.
    #[error("{program} speaks a protocol on its stdio; it has no terminal mode, attach a client")]
    NeedsClient {
        /// The agent program.
        program: String,
    },
    /// The ACP agent does not advertise `session/load` or `session/resume`.
    #[error(
        "{program} advertises neither session/load nor session/resume; it cannot continue a session"
    )]
    LoadNotSupported {
        /// The agent program.
        program: String,
    },
    /// An ACP agent has no declared state directory, so its sessions cannot be captured.
    #[error("no state directory is declared for {program}; its sessions cannot be captured")]
    StateDirNotDeclared {
        /// The agent program.
        program: String,
    },
    /// A standard redaction rule does not compile, so nothing is captured.
    #[error(transparent)]
    BadRedactionRule(#[from] crate::redact::BadRule),
    /// The Agent Client Protocol exchange failed.
    #[cfg(feature = "acp")]
    #[error("acp: {0}")]
    Acp(#[from] crate::acp::client::AcpError),
}

impl HarnessError {
    /// Wraps an IO error with the operation and the path it concerned.
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        HarnessError::Io {
            op,
            path: path.into(),
            source,
        }
    }
}
