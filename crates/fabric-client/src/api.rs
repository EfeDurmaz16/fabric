//! Every message of the client API (ADR 0018), as it crosses the socket.
//!
//! Invariant: a message parses into types that cannot hold an illegal
//! value: ids in their one text form, epochs of at least one, pids above
//! zero, a harness program that is a bare name, and a refusal reason that
//! agrees with its kind. Field names, tags and shapes are the wire
//! contract; `protocol/fixtures` holds an example of each, and the
//! TypeScript SDK reads the same files. What may change without bumping
//! [`API_VERSION`] is the compatibility rule of ADR 0018: new ops, new
//! optional fields, new event kinds and new refusal reasons, which a
//! reader skips or reads as `Unrecognized`.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use fabric_core::harness::HarnessKind;
use fabric_core::id::{EnvSpecId, Epoch, EventId, JobId, MoveId, NodeId, RecordHash, SnapshotId};
use fabric_core::session::SessionId;
use serde::{Deserialize, Serialize};

use crate::text;

/// The version of the client API this crate speaks; a node answers only the same number.
pub const API_VERSION: u32 = 1;

/// The client's first frame on a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The api version the client speaks.
    pub api: u32,
    /// The client's name and version, such as `fabric/0.1.0`, for the node's log.
    pub client: String,
}

/// The node's answer to a hello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelloAnswer {
    /// The node speaks the client's api version; the client sends its request next.
    Welcome(Welcome),
    /// The node speaks another api version and closes the connection.
    Incompatible {
        /// The api version the node speaks.
        api: u32,
        /// The node's software version.
        node_version: String,
    },
}

/// What a node says about itself when it accepts a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The api version the node speaks, equal to the client's.
    pub api: u32,
    /// The node's id.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The node's software version.
    pub node_version: String,
    /// Every op the node serves, by name, then every optional request
    /// field it applies, such as `env.roots.cargo_home`; a client sends no
    /// other (ADR 0018, Compatibility).
    pub capabilities: Vec<String>,
}

impl Welcome {
    /// Whether the node lists `capability`, an op's name or a field's.
    #[must_use]
    pub fn lists(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|name| name == capability)
    }

    /// Whether the node lists `op` among the ops it serves.
    #[must_use]
    pub fn serves(&self, op: Op) -> bool {
        self.lists(op.name())
    }
}

/// The name of each request, which is also the kind of its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// [`Request::NodeId`].
    NodeId,
    /// [`Request::NodeAdd`].
    NodeAdd,
    /// [`Request::NodeList`].
    NodeList,
    /// [`Request::NodePing`].
    NodePing,
    /// [`Request::JobNew`].
    JobNew,
    /// [`Request::JobList`].
    JobList,
    /// [`Request::JobStatus`].
    JobStatus,
    /// [`Request::JobLog`].
    JobLog,
    /// [`Request::JobRun`].
    JobRun,
    /// [`Request::JobResume`].
    JobResume,
    /// [`Request::JobSuspend`].
    JobSuspend,
    /// [`Request::JobMove`].
    JobMove,
    /// [`Request::JobSteal`].
    JobSteal,
    /// [`Request::JobDecide`].
    JobDecide,
    /// [`Request::Doctor`].
    Doctor,
    /// [`Request::HarnessStarted`].
    HarnessStarted,
    /// [`Request::HarnessSession`].
    HarnessSession,
    /// [`Request::HarnessExited`].
    HarnessExited,
}

impl Op {
    /// Every op of this api version.
    pub const ALL: [Op; 18] = [
        Op::NodeId,
        Op::NodeAdd,
        Op::NodeList,
        Op::NodePing,
        Op::JobNew,
        Op::JobList,
        Op::JobStatus,
        Op::JobLog,
        Op::JobRun,
        Op::JobResume,
        Op::JobSuspend,
        Op::JobMove,
        Op::JobSteal,
        Op::JobDecide,
        Op::Doctor,
        Op::HarnessStarted,
        Op::HarnessSession,
        Op::HarnessExited,
    ];

    /// The op's name on the wire: the request's `op` and the reply's `kind`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Op::NodeId => "node_id",
            Op::NodeAdd => "node_add",
            Op::NodeList => "node_list",
            Op::NodePing => "node_ping",
            Op::JobNew => "job_new",
            Op::JobList => "job_list",
            Op::JobStatus => "job_status",
            Op::JobLog => "job_log",
            Op::JobRun => "job_run",
            Op::JobResume => "job_resume",
            Op::JobSuspend => "job_suspend",
            Op::JobMove => "job_move",
            Op::JobSteal => "job_steal",
            Op::JobDecide => "job_decide",
            Op::Doctor => "doctor",
            Op::HarnessStarted => "harness_started",
            Op::HarnessSession => "harness_session",
            Op::HarnessExited => "harness_exited",
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The harness kinds as the client API writes them: `{ "kind": "claude_code" }`,
/// `{ "kind": "codex" }` or `{ "kind": "acp", "program": ..., "args": [...] }`.
///
/// A field this client does not know is refused, not skipped: a harness
/// says what a client launches, and a client that dropped a field would
/// launch something else and report the harness without it, which the
/// node records as the job's (ADR 0018, Compatibility, rule 2).
#[derive(Serialize, Deserialize)]
#[serde(
    remote = "HarnessKind",
    tag = "kind",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum HarnessDef {
    // Written with braces so serde refuses a field next to the tag; it
    // skips any field of a unit variant.
    ClaudeCode {},
    Codex {},
    Acp {
        program: fabric_core::harness::ProgramName,
        args: Vec<String>,
    },
}

/// A harness kind in the client API's form, for an optional field.
mod harness_option {
    use fabric_core::harness::HarnessKind;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire(#[serde(with = "super::HarnessDef")] HarnessKind);

    #[derive(Serialize)]
    struct WireRef<'a>(#[serde(with = "super::HarnessDef")] &'a HarnessKind);

    #[expect(
        clippy::ref_option,
        reason = "serde's `with` passes the field by reference"
    )]
    pub(super) fn serialize<S: Serializer>(
        value: &Option<HarnessKind>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().map(WireRef).serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<HarnessKind>, D::Error> {
        Ok(Option::<Wire>::deserialize(d)?.map(|wire| wire.0))
    }
}

/// What a client asks a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// This node's id and listening address.
    NodeId,
    /// Pairs a peer, at an address when this node can dial it.
    NodeAdd {
        /// The peer.
        #[serde(with = "text")]
        node: NodeId,
        /// Where it listens, `host:port`; absent for a peer behind a NAT that only dials in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        addr: Option<String>,
    },
    /// The paired peers.
    NodeList,
    /// Dials a peer and pings it.
    NodePing {
        /// The peer.
        #[serde(with = "text")]
        node: NodeId,
    },
    /// Makes a job from a repository, rooted at `~/fabric/jobs/<id>/work`.
    JobNew {
        /// The repository to clone, an absolute path on the node; a
        /// relative one would name a repository relative to wherever the
        /// node was started.
        repo: SystemPath,
        /// The goal in the user's words.
        goal: String,
        /// The harness the job starts under.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
    },
    /// Every job this node knows.
    JobList,
    /// One job's state on this node.
    JobStatus {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
    },
    /// One job's ledger.
    JobLog {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
    },
    /// Checks the job may run here and answers what to launch.
    JobRun {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The environment the caller launches the harness in. The node
        /// measures nothing for this request; it keeps the environment, as
        /// it keeps every one a local client sends, for work no client asks
        /// for, such as a peer's pull of the job.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<ClientEnv>,
    },
    /// Pulls the job if another node owns it, restores it if needed, and answers what to launch.
    JobResume {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The node that holds the job, when this node has never seen it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "text::option"
        )]
        on: Option<NodeId>,
        /// A harness to use instead of the job's.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "harness_option"
        )]
        harness: Option<HarnessKind>,
        /// Waive the preflight refusals that can be waived.
        force: bool,
        /// The caller's environment; the node's own when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<ClientEnv>,
    },
    /// Stops the harness, snapshots the workspace and records the suspend.
    JobSuspend {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The caller's environment; the node's own when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<ClientEnv>,
    },
    /// Moves the job to a peer by the cooperative move protocol.
    JobMove {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The target.
        #[serde(with = "text")]
        to: NodeId,
        /// Waive the preflight refusals that can be waived.
        force: bool,
        /// The caller's environment; the node's own when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<ClientEnv>,
    },
    /// Takes the job without the owner's consent (ADR 0003, The steal).
    JobSteal {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The user's reason, or empty.
        reason: String,
        /// Without it, the node refuses with what the steal would do.
        force: bool,
    },
    /// Records the user's decision on the job's work in its ledger.
    JobDecide {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// What the user decided.
        decision: Decision,
        /// The user's words, or empty.
        note: String,
    },
    /// This node's facts, or their comparison with a peer's.
    Doctor {
        /// The peer to compare with.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "text::option"
        )]
        peer: Option<NodeId>,
        /// The job a move would carry, for the rows that depend on it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "text::option"
        )]
        job: Option<JobId>,
        /// The caller's environment; the node's own when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<ClientEnv>,
    },
    /// The client launched the planned harness; the node records it and appends `Resumed`.
    HarnessStarted {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The epoch the plan was made under.
        epoch: Epoch,
        /// The harness process, which leads its own process group.
        pid: NonZeroU32,
        /// The harness that runs.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session it continues or created, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
    /// The harness named the session it runs after it started, as `codex exec` does.
    HarnessSession {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The epoch the plan was made under.
        epoch: Epoch,
        /// The harness process, as registered.
        pid: NonZeroU32,
        /// The harness that runs.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session it named.
        session: SessionId,
    },
    /// The harness exited.
    HarnessExited {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The process that exited.
        pid: NonZeroU32,
    },
}

impl Request {
    /// The capabilities, beyond its op, a node must list for this request
    /// to mean what it says: with an environment, one for every root this
    /// client knows, set or not. A root the client sets must reach the
    /// tool, and one it leaves unset must be removed from the tool's
    /// environment, never the daemon's own; a node that does not know a
    /// root does neither, so without the check an older node would measure
    /// with its own value in place of the client's and nobody would know
    /// (review findings on PR #13).
    #[must_use]
    pub fn needs(&self) -> Vec<&'static str> {
        if self.env().is_some() {
            Roots::capabilities().to_vec()
        } else {
            Vec::new()
        }
    }

    /// The caller's environment, when the request carries one.
    #[must_use]
    pub fn env(&self) -> Option<&ClientEnv> {
        match self {
            Request::JobRun { env, .. }
            | Request::JobResume { env, .. }
            | Request::JobSuspend { env, .. }
            | Request::JobMove { env, .. }
            | Request::Doctor { env, .. } => env.as_ref(),
            Request::NodeId
            | Request::NodeAdd { .. }
            | Request::NodeList
            | Request::NodePing { .. }
            | Request::JobNew { .. }
            | Request::JobList
            | Request::JobStatus { .. }
            | Request::JobLog { .. }
            | Request::JobSteal { .. }
            | Request::JobDecide { .. }
            | Request::HarnessStarted { .. }
            | Request::HarnessSession { .. }
            | Request::HarnessExited { .. } => None,
        }
    }

    /// The request's op.
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Request::NodeId => Op::NodeId,
            Request::NodeAdd { .. } => Op::NodeAdd,
            Request::NodeList => Op::NodeList,
            Request::NodePing { .. } => Op::NodePing,
            Request::JobNew { .. } => Op::JobNew,
            Request::JobList => Op::JobList,
            Request::JobStatus { .. } => Op::JobStatus,
            Request::JobLog { .. } => Op::JobLog,
            Request::JobRun { .. } => Op::JobRun,
            Request::JobResume { .. } => Op::JobResume,
            Request::JobSuspend { .. } => Op::JobSuspend,
            Request::JobMove { .. } => Op::JobMove,
            Request::JobSteal { .. } => Op::JobSteal,
            Request::JobDecide { .. } => Op::JobDecide,
            Request::Doctor { .. } => Op::Doctor,
            Request::HarnessStarted { .. } => Op::HarnessStarted,
            Request::HarnessSession { .. } => Op::HarnessSession,
            Request::HarnessExited { .. } => Op::HarnessExited,
        }
    }
}

/// The environment of the client a request runs for (ADR 0018, The
/// client's environment).
///
/// A node measures facts, picks carry keys and finds harnesses with the
/// tools this environment names, not with its own: a cache is carried or
/// rebuilt by the toolchain the user's shell runs. A root it does not set
/// is unset for the request, never the node's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEnv {
    /// The directories of the client's `PATH`, in order.
    pub path: Vec<PathDir>,
    /// The tool roots the client's environment sets.
    #[serde(default)]
    pub roots: Roots,
}

impl ClientEnv {
    /// The environment of this process, as [`ClientEnv::from_vars`] reads it.
    ///
    /// # Errors
    ///
    /// As [`ClientEnv::from_vars`].
    pub fn current() -> Result<Self, EnvError> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    /// The environment whose variables `var` reads: its `PATH` without the
    /// entries that are not absolute, such as `.`, an empty one or a
    /// relative one, which would name a directory of the node's choosing,
    /// and each root it sets to an absolute path. Every entry and root it
    /// keeps is kept as written, `//`, `..` and a trailing `/` included, so
    /// the node finds the tools the client's shell finds.
    ///
    /// # Errors
    ///
    /// An absolute `PATH` entry or root that is not UTF-8: the socket
    /// carries text, and leaving it out would have the node measure
    /// without it.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, EnvError> {
        let mut path = Vec::new();
        if let Some(value) = var("PATH") {
            for dir in std::env::split_paths(&value) {
                // An entry split from `PATH` holds no `:`.
                if let Some(dir) = system_path("PATH", dir.as_os_str())?
                    .and_then(|dir| PathDir::from_system(dir).ok())
                {
                    path.push(dir);
                }
            }
        }
        let root = |name: &'static str| match var(name) {
            Some(value) => system_path(name, &value),
            None => Ok(None),
        };
        Ok(Self {
            path,
            roots: Roots {
                cargo_home: root(Roots::CARGO_HOME)?,
                rustup_home: root(Roots::RUSTUP_HOME)?,
                pnpm_home: root(Roots::PNPM_HOME)?,
                xdg_data_home: root(Roots::XDG_DATA_HOME)?,
            },
        })
    }

    /// The `PATH` value to run a tool with.
    #[must_use]
    pub fn path_var(&self) -> OsString {
        let dirs: Vec<&str> = self.path.iter().map(PathDir::as_str).collect();
        dirs.join(":").into()
    }
}

/// `value` of the variable `var` as a [`SystemPath`]; `None` when it is not absolute.
fn system_path(var: &'static str, value: &OsStr) -> Result<Option<SystemPath>, EnvError> {
    if value.as_bytes().first() != Some(&b'/') {
        return Ok(None);
    }
    let text = value.to_str().ok_or_else(|| EnvError {
        var,
        value: value.to_owned(),
    })?;
    // The kernel keeps NUL out of every variable, so an absolute UTF-8 value parses.
    Ok(SystemPath::parse(text).ok())
}

/// A variable of the client's environment that names an absolute path
/// that is not UTF-8, which the client API cannot carry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{var} names {}, which is not UTF-8, and the client API carries only UTF-8 paths; rename it or take it out of {var}",
    value.to_string_lossy()
)]
pub struct EnvError {
    /// The variable.
    pub var: &'static str,
    /// The path it names; for `PATH`, the one entry.
    pub value: OsString,
}

/// An absolute path as the operating system resolves it: text that
/// starts with `/` and holds no NUL.
///
/// Unlike the core's [`AbsPath`](fabric_core::path::AbsPath), the one text
/// form of a path Fabric itself writes, it keeps `//`, `.`, `..`, a
/// trailing `/`, a `.git` component and any Unicode form, because the
/// kernel reads it, not Fabric: a tool root or a `PATH` directory names
/// the directory the client's shell names. A relative path is refused,
/// since it would name a directory of the node's choosing.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SystemPath(String);

impl SystemPath {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, String> {
        if !text.starts_with('/') {
            return Err(format!("{text:?} is not an absolute path"));
        }
        if text.contains('\0') {
            return Err(format!("{text:?} holds a NUL"));
        }
        Ok(Self(text.to_owned()))
    }

    /// The path's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl AsRef<Path> for SystemPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl fmt::Display for SystemPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SystemPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SystemPath({:?})", self.0)
    }
}

impl<'de> Deserialize<'de> for SystemPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// One directory of a `PATH`: a [`SystemPath`] without a `:`, which would
/// split it in two.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PathDir(SystemPath);

impl PathDir {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::from_system(SystemPath::parse(text)?)
    }

    /// `dir` as a `PATH` directory, unless it holds a `:`.
    fn from_system(dir: SystemPath) -> Result<Self, String> {
        if dir.as_str().contains(':') {
            return Err(format!(
                "{:?} holds a `:`, which separates PATH entries",
                dir.as_str()
            ));
        }
        Ok(Self(dir))
    }

    /// The directory's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for PathDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PathDir({:?})", self.as_str())
    }
}

impl<'de> Deserialize<'de> for PathDir {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// The tool roots a node takes from a client's environment: the closed
/// set of variables that decide where the tools it measures keep their
/// state. A root that is absent is unset for the request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roots {
    /// `CARGO_HOME`: where cargo keeps its registry and rustup its proxies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_home: Option<SystemPath>,
    /// `RUSTUP_HOME`: where rustup keeps its toolchains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustup_home: Option<SystemPath>,
    /// `PNPM_HOME`: where pnpm lives, and its store when it is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pnpm_home: Option<SystemPath>,
    /// `XDG_DATA_HOME`: where pnpm keeps its store when `PNPM_HOME` is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdg_data_home: Option<SystemPath>,
}

impl Roots {
    /// The variable `cargo_home` stands for.
    pub const CARGO_HOME: &str = "CARGO_HOME";
    /// The variable `rustup_home` stands for.
    pub const RUSTUP_HOME: &str = "RUSTUP_HOME";
    /// The variable `pnpm_home` stands for.
    pub const PNPM_HOME: &str = "PNPM_HOME";
    /// The variable `xdg_data_home` stands for.
    pub const XDG_DATA_HOME: &str = "XDG_DATA_HOME";

    /// Every root with its capability and its variable, set or not.
    fn each(&self) -> [(&'static str, &'static str, Option<&SystemPath>); 4] {
        [
            (
                "env.roots.cargo_home",
                Self::CARGO_HOME,
                self.cargo_home.as_ref(),
            ),
            (
                "env.roots.rustup_home",
                Self::RUSTUP_HOME,
                self.rustup_home.as_ref(),
            ),
            (
                "env.roots.pnpm_home",
                Self::PNPM_HOME,
                self.pnpm_home.as_ref(),
            ),
            (
                "env.roots.xdg_data_home",
                Self::XDG_DATA_HOME,
                self.xdg_data_home.as_ref(),
            ),
        ]
    }

    /// Every root with its variable, set or not, so a tool run for the
    /// client gets each one set to the client's value or removed.
    #[must_use]
    pub fn vars(&self) -> [(&'static str, Option<&SystemPath>); 4] {
        self.each().map(|(_, var, value)| (var, value))
    }

    /// The capability a node lists for each root it applies,
    /// `env.roots.<field>`; a client sends an environment only to a node
    /// that lists every root the client knows.
    #[must_use]
    pub fn capabilities() -> [&'static str; 4] {
        Self::default().each().map(|(capability, _, _)| capability)
    }
}

/// A user's decision on a job's work (ADR 0016, Decisions as ledger events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// `fabric job approve`.
    Approve,
    /// `fabric job reject`.
    Reject,
}

/// What a frame from the node says once it welcomed the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    /// A step of the command, sent as it happens; more frames follow.
    Event {
        /// The step.
        event: Event,
    },
    /// The command succeeded; the last frame.
    Done {
        /// Its result.
        reply: Reply,
    },
    /// The command was refused or failed; the last frame.
    Refused {
        /// Why.
        refusal: Refusal,
    },
}

/// The result of a request; its kind is the request's op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// For [`Request::NodeId`].
    NodeId(NodeIdentity),
    /// For [`Request::NodeAdd`].
    NodeAdd(Paired),
    /// For [`Request::NodeList`].
    NodeList(PeerList),
    /// For [`Request::NodePing`].
    NodePing(Pong),
    /// For [`Request::JobNew`].
    JobNew(JobCreated),
    /// For [`Request::JobList`].
    JobList(JobList),
    /// For [`Request::JobStatus`].
    JobStatus(Box<JobStatus>),
    /// For [`Request::JobLog`].
    JobLog(JobLog),
    /// For [`Request::JobRun`]: what to launch.
    JobRun(RunPlan),
    /// For [`Request::JobResume`]: what to launch.
    JobResume(RunPlan),
    /// For [`Request::JobSuspend`].
    JobSuspend(Suspended),
    /// For [`Request::JobMove`]: the job's lease once the move is decided.
    JobMove(Lease),
    /// For [`Request::JobSteal`]: the job's lease once stolen.
    JobSteal(Lease),
    /// For [`Request::JobDecide`].
    JobDecide(Decided),
    /// For [`Request::Doctor`].
    Doctor {
        /// The facts, or the comparison.
        report: DoctorReport,
    },
    /// For [`Request::HarnessStarted`].
    HarnessStarted(Registered),
    /// For [`Request::HarnessSession`].
    HarnessSession(SessionRecorded),
    /// For [`Request::HarnessExited`].
    HarnessExited,
}

impl Reply {
    /// The op this reply answers.
    #[must_use]
    pub fn op(&self) -> Op {
        match self {
            Reply::NodeId(_) => Op::NodeId,
            Reply::NodeAdd(_) => Op::NodeAdd,
            Reply::NodeList(_) => Op::NodeList,
            Reply::NodePing(_) => Op::NodePing,
            Reply::JobNew(_) => Op::JobNew,
            Reply::JobList(_) => Op::JobList,
            Reply::JobStatus(_) => Op::JobStatus,
            Reply::JobLog(_) => Op::JobLog,
            Reply::JobRun(_) => Op::JobRun,
            Reply::JobResume(_) => Op::JobResume,
            Reply::JobSuspend(_) => Op::JobSuspend,
            Reply::JobMove(_) => Op::JobMove,
            Reply::JobSteal(_) => Op::JobSteal,
            Reply::JobDecide(_) => Op::JobDecide,
            Reply::Doctor { .. } => Op::Doctor,
            Reply::HarnessStarted(_) => Op::HarnessStarted,
            Reply::HarnessSession(_) => Op::HarnessSession,
            Reply::HarnessExited => Op::HarnessExited,
        }
    }
}

/// A node's identity and where it listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    /// The node's id.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The address both transports listen on.
    pub listen: SocketAddr,
}

/// A peer just paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    /// The peer.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The address this node dials it at; absent when this node never dials it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
}

/// The paired peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerList {
    /// One entry per peer, in the peer book's order.
    pub peers: Vec<Peer>,
}

/// One paired peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// The peer.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The addresses the user paired it at, newest first; empty when this node never dials it.
    pub addrs: Vec<String>,
    /// Whether a session with it is open now.
    pub connected: bool,
}

/// A peer's answer to a ping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    /// The peer.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The round trip, in milliseconds.
    pub rtt_ms: u64,
    /// The wire the session runs on.
    pub wire: Wire,
    /// The peer's clock minus this node's, in milliseconds.
    pub clock_skew_ms: i64,
}

/// The wire a session with a peer runs on (ADR 0013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wire {
    /// QUIC.
    Quic,
    /// TCP with TLS and the multiplexer.
    TcpTls,
}

impl fmt::Display for Wire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Wire::Quic => "quic",
            Wire::TcpTls => "tcp+tls",
        })
    }
}

/// A job just made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCreated {
    /// The job.
    #[serde(with = "text")]
    pub job: JobId,
    /// Its root: a clone of the repository under `~/fabric/jobs`.
    pub root: PathBuf,
    /// The repository it was cloned from.
    pub repo: PathBuf,
    /// The harness it starts under.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// The epoch this node owns it at.
    pub epoch: Epoch,
}

/// Every job a node knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobList {
    /// The jobs whose ledgers opened, by id.
    pub jobs: Vec<JobSummary>,
    /// The jobs whose ledgers did not open, which the node leaves closed.
    pub closed: Vec<ClosedJob>,
}

/// One job in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    /// The job.
    #[serde(with = "text")]
    pub job: JobId,
    /// What this node is for it.
    pub standing: Standing,
    /// The job's epoch.
    pub epoch: Epoch,
    /// The goal.
    pub goal: String,
}

/// What a node is for a job, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// It owns the job and may run it.
    Owner,
    /// It started a move and waits for the target's decision.
    Departing,
    /// It handed the job to another node.
    Departed,
    /// It holds a copy of the ledger and nothing else.
    Replica,
    /// A peer's ledger forked from this one, so the job never runs here again.
    Frozen,
    /// The record that says whether the job is frozen could not be read.
    Unreadable,
}

impl fmt::Display for Standing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Standing::Owner => "owner",
            Standing::Departing => "departing",
            Standing::Departed => "departed",
            Standing::Replica => "replica",
            Standing::Frozen => "frozen",
            Standing::Unreadable => "unreadable",
        })
    }
}

/// A job whose ledger did not open on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedJob {
    /// The job.
    #[serde(with = "text")]
    pub job: JobId,
    /// Why it did not open.
    pub error: String,
}

/// One job's state on the node that answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStatus {
    /// The job.
    #[serde(with = "text")]
    pub job: JobId,
    /// The node that answers.
    #[serde(with = "text")]
    pub node: NodeId,
    /// The goal.
    pub goal: String,
    /// The harness the job runs under.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// The owner of record.
    #[serde(with = "text")]
    pub owner: NodeId,
    /// The current epoch.
    pub epoch: Epoch,
    /// What the answering node is for the job.
    pub view: View,
    /// The latest published snapshot, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotState>,
    /// The workspace on the answering node, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// Why the node could not read the job's workspace marker, when it
    /// could not; `workspace` is then absent and says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_unreadable: Option<String>,
    /// Whether a harness the node launched for the job runs.
    pub process: Process,
    /// How many records the ledger holds.
    pub records: u64,
}

/// What a node is for a job, with what it knows about the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum View {
    /// It owns the job.
    Owner,
    /// It started a move and the job is frozen until the target decides.
    Departing {
        /// The open move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The node that decides it.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// It handed the job to `owner`; its copy never runs.
    Departed {
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// It holds a replica of the ledger.
    Replica {
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// A peer's ledger at an epoch at least as high forked from this one.
    Frozen {
        /// The peer to continue on.
        #[serde(with = "text")]
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
    },
    /// The node's fork record for the job could not be read, so whether a
    /// peer froze the job here is not known.
    Unreadable {
        /// Why, in the node's words.
        error: String,
    },
}

/// The latest published snapshot of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotState {
    /// The snapshot.
    #[serde(with = "text")]
    pub id: SnapshotId,
    /// Whether the job is suspended in it, or was resumed since.
    pub suspended: bool,
}

/// The workspace a node holds for a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Its root.
    pub root: PathBuf,
    /// What it holds.
    pub state: WorkspaceState,
}

/// What a workspace holds, as the node's marker and its relation to the job say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceState {
    /// The owner's workspace of this tenure.
    Current {
        /// What the marker says it holds.
        marker: Marker,
    },
    /// The owner's workspace was left by an earlier tenure; a resume sets it aside.
    Stale {
        /// The epoch its marker was written at.
        left_at: Epoch,
    },
    /// Clean at the snapshot offered to `target`, waiting for its decision.
    AwaitingDecision {
        /// The target of the open move.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// Clean at a published snapshot on a node that does not own the job; moved aside at the next settle.
    CleanCopy,
    /// May hold changes made after the last snapshot; nothing deletes it.
    Orphan,
}

/// What a workspace marker says (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Marker {
    /// A harness may have changed it since the last snapshot.
    Live,
    /// A suspend did not finish.
    Suspending,
    /// It holds exactly this snapshot.
    Clean {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
    /// A restore of this snapshot did not finish.
    Restoring {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
}

/// Whether the harness a node launched for a job runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Process {
    /// No harness runs.
    NotRunning,
    /// The recorded harness runs.
    Running {
        /// Its pid, which is its process group.
        pid: NonZeroU32,
        /// The epoch it was launched under.
        epoch: Epoch,
    },
    /// The harness exited and processes it started still run in its group.
    GroupRemains {
        /// The group, named by the harness's pid.
        pid: NonZeroU32,
    },
    /// The node's record of the harness could not be read, so whether one
    /// runs is not known.
    Unreadable {
        /// Why, in the node's words.
        error: String,
    },
}

/// A job's ledger, or its latest records when all of them would not fit in one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLog {
    /// How many of the first records are left out.
    #[serde(default)]
    pub left_out: u64,
    /// The records after those, first to last.
    pub records: Vec<LogRecord>,
}

/// One ledger record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    /// The record's id.
    #[serde(with = "text")]
    pub id: EventId,
    /// The epoch it was written under.
    pub epoch: Epoch,
    /// When its writer wrote it, in Unix milliseconds.
    pub at: u64,
    /// What it says happened.
    pub event: LedgerEvent,
}

/// What a ledger record says happened (ADR 0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerEvent {
    /// The job was created.
    Created {
        /// The node that created it.
        #[serde(with = "text")]
        created_by: NodeId,
        /// The harness it started under.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The goal.
        goal: String,
    },
    /// The owner committed to a move.
    MoveStarted {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The node that decides it.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The target imported the move.
    MoveImported {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The node the job came from.
        #[serde(with = "text")]
        from: NodeId,
    },
    /// The target declined the move.
    MoveAborted {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
    },
    /// `by` took the job from `from` without consent.
    Stolen {
        /// The new owner.
        #[serde(with = "text")]
        by: NodeId,
        /// The owner of record at the time.
        #[serde(with = "text")]
        from: NodeId,
        /// The user's reason, or empty.
        reason: String,
    },
    /// A snapshot is complete in the owner's store.
    SnapshotPublished {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
        /// The snapshot it follows.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "text::option"
        )]
        parent: Option<SnapshotId>,
    },
    /// The harness stopped and the workspace is in `snapshot`.
    Suspended {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
    /// A harness was launched on `node`.
    Resumed {
        /// The node.
        #[serde(with = "text")]
        node: NodeId,
        /// The snapshot restored first, if any.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "text::option"
        )]
        snapshot: Option<SnapshotId>,
        /// The harness.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
    },
    /// The job references another environment spec.
    EnvSpecChanged {
        /// The spec.
        #[serde(with = "text")]
        env_spec: EnvSpecId,
    },
    /// A decision in words.
    DecisionRecorded {
        /// The decision.
        text: String,
    },
    /// Free text.
    Note {
        /// The note.
        text: String,
    },
    /// A kind of record this client does not know; a later ledger format wrote it.
    #[serde(other)]
    Unrecognized,
}

/// What to launch for a job, as the node decided it.
///
/// A field this client does not know is refused, not skipped, here and in
/// [`HarnessConfig`] and [`AcpSettings`]: the launch must apply every
/// setting of the plan, and a client that dropped one would launch a
/// harness configured otherwise than the node captures and restores it,
/// with nothing to tell either side (ADR 0018, Compatibility, rule 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlan {
    /// The job.
    #[serde(with = "text")]
    pub job: JobId,
    /// The epoch the node owns the job at; the launch is fenced on it.
    pub epoch: Epoch,
    /// The workspace root.
    pub root: PathBuf,
    /// The harness to launch.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// The session to continue; a new one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The job's goal, the first message of a new session.
    pub goal: String,
    /// The node's harness configuration, which the launch must use, so a
    /// harness runs under the settings the node captures and restores it with.
    #[serde(default)]
    pub harness_config: HarnessConfig,
}

/// A node's harness configuration, from its `harness.toml` (ADR 0016).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    /// ACP agents' settings, by program name.
    #[serde(default)]
    pub acp: std::collections::BTreeMap<String, AcpSettings>,
}

/// One ACP agent's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpSettings {
    /// Where the agent keeps its sessions; a relative path is under the home.
    pub state_dir: PathBuf,
}

/// A suspend's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspended {
    /// The snapshot the job is suspended in.
    #[serde(with = "text")]
    pub snapshot: SnapshotId,
}

/// Who owns a job, at which epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// The owner.
    #[serde(with = "text")]
    pub owner: NodeId,
    /// The epoch.
    pub epoch: Epoch,
}

/// A decision recorded in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decided {
    /// The text the record holds: `approved` or `rejected`, then the note after a colon.
    pub text: String,
}

/// What `doctor` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DoctorReport {
    /// This node's facts, sorted by name.
    Facts {
        /// The facts.
        facts: Vec<Fact>,
    },
    /// This node's facts compared with a peer's, row by row (ADR 0008).
    Compared {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The rows.
        rows: Vec<PreflightRow>,
    },
}

/// One fact a node measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// Its name.
    pub name: String,
    /// Its value, which may hold several lines.
    pub value: String,
}

/// One row of the preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightRow {
    /// The fact or facts compared.
    pub fact: String,
    /// This node's value; absent when it lacks the fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub here: Option<String>,
    /// The peer's value; absent when it lacks the fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// What the row means for a move.
    pub outcome: Outcome,
    /// Why, in one sentence; empty when the two agree.
    pub note: String,
}

/// What a preflight row means for a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Both nodes agree, or the fact does not matter for this job.
    Equal,
    /// The move goes ahead and the dependent caches are rebuilt.
    Fallback,
    /// The move goes ahead; the user should know.
    Warn,
    /// The move stops unless the user passes `--force`.
    Refuse,
    /// The move stops even with `--force` (ADR 0008).
    RefuseAlways,
}

impl Outcome {
    /// Whether this row stops a move made with `force` or without it.
    #[must_use]
    pub fn blocks(self, force: bool) -> bool {
        match self {
            Outcome::Refuse => !force,
            Outcome::RefuseAlways => true,
            Outcome::Equal | Outcome::Fallback | Outcome::Warn => false,
        }
    }
}

/// A launch the node recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// The epoch `Resumed` was recorded at.
    pub epoch: Epoch,
    /// The harness that runs.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// Its pid.
    pub pid: NonZeroU32,
}

/// A session the node recorded for a running harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecorded {
    /// The harness.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// The session.
    pub session: SessionId,
}

/// A harness and one of its sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSession {
    /// The harness.
    #[serde(with = "HarnessDef")]
    pub harness: HarnessKind,
    /// The session.
    pub session: SessionId,
}

/// What a steal costs, for the user to read before it happens (ADR 0003, The steal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StealWarning {
    /// The owner of record being stolen from.
    #[serde(with = "text")]
    pub owner: NodeId,
    /// The snapshot the thief resumes from.
    #[serde(with = "text")]
    pub snapshot: SnapshotId,
    /// Whether the owner stopped its harness in that snapshot and has not resumed since.
    pub owner_stopped_at_snapshot: bool,
    /// When the owner wrote its last record this node holds, in Unix milliseconds.
    pub last_record_at: u64,
    /// How old that record is by this node's clock, in milliseconds.
    pub age_ms: u64,
}

/// One step of a command, reported as it happens.
///
/// Events never carry a command's result, so a client that does not know a
/// kind skips it; [`crate::Client`] does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The node is still catching up with its peers after a restart; what a read answers may not show what they wrote.
    CatchingUp,
    /// The preflight compared this node with `peer`.
    Preflight {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The rows.
        rows: Vec<PreflightRow>,
    },
    /// A workspace an earlier tenure left is not captured; a resume sets it aside.
    StaleWorkspaceKept {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The workspace root.
        root: PathBuf,
        /// The epoch its marker was written at.
        left_at: Epoch,
    },
    /// The job is already suspended in this snapshot and the tree has not changed.
    AlreadySuspended {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
    /// The workspace changed after this snapshot, by hand or by a harness Fabric did not start; it is captured again.
    WorkspaceChanged {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
    /// The node is stopping the harness.
    StoppingHarness {
        /// Its pid.
        pid: NonZeroU32,
    },
    /// A suspend that published this snapshot before a crash is being finished.
    FinishingSuspend {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
    },
    /// The harness session was captured.
    SessionCaptured {
        /// The harness.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session.
        session: SessionId,
        /// How many files its state holds.
        files: u64,
    },
    /// A snapshot was published.
    SnapshotPublished {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
        /// Files it holds.
        files: u64,
        /// Bytes it holds.
        bytes: u64,
        /// Objects the store did not hold before.
        new_objects: u64,
    },
    /// A file matched a secret pattern and was left out of the snapshot.
    LeftBehind {
        /// Its path in the workspace.
        path: String,
    },
    /// No departure record waits for this session, so its rewritable files stay this node's (ADR 0015).
    NoDepartureRecord {
        /// The session.
        session: SessionId,
    },
    /// The job is suspended in `snapshot`, and this node keeps its lease.
    Suspended {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
        /// The epoch the node owns the job at.
        epoch: Epoch,
    },
    /// The ledger and the snapshot were copied to a peer.
    Replicated {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
    },
    /// The ledger could not be copied to a peer.
    NotReplicated {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// Why.
        error: Refusal,
    },
    /// A move toward the target is open and is offered again.
    MoveReoffered {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The target already owns the job: an earlier answer was lost.
    AlreadyMoved {
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// The epoch it owns the job at.
        epoch: Epoch,
    },
    /// The target holds the ledger and the snapshot.
    TargetHoldsCopy {
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// `MoveStarted` is durable; this copy does not run until the target decides.
    MoveStarted {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The target gave no answer; the node asks for its decision.
    NoAnswer {
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
        /// Why.
        error: Refusal,
    },
    /// The node stopped asking the target; the move stays open.
    MoveGaveUp {
        /// The last thing that went wrong, in the node's words.
        detail: String,
    },
    /// The target imported the job.
    Imported {
        /// The new owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// Its epoch.
        epoch: Epoch,
    },
    /// The target declined the move; this node owns the job again.
    Declined {
        /// The target that declined.
        #[serde(with = "text")]
        target: NodeId,
        /// The epoch this node owns the job at.
        epoch: Epoch,
    },
    /// The target's decision left this node in another standing.
    DecisionLeft {
        /// The standing.
        standing: Standing,
    },
    /// The open move is still undecided.
    MoveUndecided {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The open move's target did not answer.
    MoveUnanswered {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
        /// Why.
        error: Refusal,
    },
    /// At start-up, the open move waits for a target that did not answer.
    MoveWaits {
        /// The target.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The harness's epoch is over for this node, so the node stops it.
    Fenced {
        /// The epoch it was launched under.
        epoch: Epoch,
        /// Its pid.
        pid: NonZeroU32,
    },
    /// This node's clean copy left the root; it is kept and never run.
    CopyMovedAside {
        /// Where it went.
        to: PathBuf,
    },
    /// The node is pulling the job from its holder.
    Pulling {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The holder.
        #[serde(with = "text")]
        holder: NodeId,
    },
    /// The holder started a move toward this node.
    HolderStartedMove {
        /// The holder.
        #[serde(with = "text")]
        holder: NodeId,
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
    },
    /// This node imported the move and owns the job.
    ImportedHere {
        /// The move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The epoch this node owns the job at.
        epoch: Epoch,
    },
    /// The holder has not seen the import yet; it will ask for the decision.
    HolderNotTold {
        /// The holder.
        #[serde(with = "text")]
        holder: NodeId,
        /// Why.
        error: Refusal,
    },
    /// The clean workspace at the root is gone, so the latest snapshot is restored.
    WorkspaceGone {
        /// The root.
        root: PathBuf,
    },
    /// The workspace of this tenure is already at its root.
    WorkspaceInPlace {
        /// The root.
        root: PathBuf,
    },
    /// Where the snapshot restores on this node, and why.
    RootChosen {
        /// The root.
        root: PathBuf,
        /// The root the snapshot recorded, when it is not this job's root
        /// under this node's home and the job lands in the state directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recorded: Option<PathBuf>,
    },
    /// A cache's bytes are written here: its key matches this node's.
    CacheCarried {
        /// The cache's path in the workspace.
        path: String,
    },
    /// A cache is rebuilt here instead of carried.
    CacheRebuilt {
        /// The cache's path in the workspace.
        path: String,
        /// Why, in the node's words.
        why: String,
    },
    /// A snapshot was restored.
    Restored {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
        /// Files written.
        files: u64,
        /// Bytes written.
        bytes: u64,
    },
    /// A workspace an earlier tenure left was moved aside before a restore.
    OrphanSetAside {
        /// The root it was at.
        root: PathBuf,
        /// The epoch its marker was written at.
        left_at: Epoch,
        /// The epoch of this node's tenure.
        epoch: Epoch,
        /// Where it went; kept, never run.
        to: PathBuf,
    },
    /// The harness session was restored.
    SessionRestored {
        /// The harness.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session.
        session: SessionId,
    },
    /// What the steal costs.
    StealWarning {
        /// The warning.
        warning: StealWarning,
    },
    /// The steal is recorded; this node owns the job.
    Stole {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The epoch this node owns it at.
        epoch: Epoch,
    },
    /// The owner the job was stolen from has not seen the steal yet.
    OwnerNotTold {
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// Why.
        error: Refusal,
    },
    /// The launch continues this session.
    ContinuingSession {
        /// The harness.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session.
        session: SessionId,
    },
    /// The launch starts a new session.
    NewSession {
        /// The harness.
        #[serde(with = "HarnessDef")]
        harness: HarnessKind,
        /// The session of another harness that stays in the snapshot (ADR 0006, cross-harness resume).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<HarnessSession>,
    },
    /// The node did not catch up with a peer.
    NotCaughtUp {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// Why.
        error: Refusal,
    },
    /// A peer did not answer in time, so a move or a steal there is not known here.
    PeerSilent {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// How long the node waited, in seconds.
        waited_s: u64,
    },
    /// Settling the job failed; it was left as it was.
    LeftAsItWas {
        /// Why.
        error: Refusal,
    },
    /// The job is frozen on this node: a peer's ledger forked from this one.
    Frozen {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
    },
    /// A kind of event this client does not know; a newer node sent it.
    #[serde(other)]
    Unrecognized,
}

/// The class of a refusal, which decides the CLI's exit code (ADR 0016, Exit codes).
///
/// A closed set: a new class changes what scripts read, so it bumps [`API_VERSION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    /// Another node owns the job, this node does not know it, or the plan's epoch is over.
    NotOwner,
    /// A move this node started waits for its target, or the job is frozen after a fork.
    Frozen,
    /// A peer could not be reached, or gave no answer in time.
    Unreachable,
    /// Refused without `--force`: the preflight, or a steal's dry run.
    NeedsForce,
    /// The job's state stops the command.
    JobState,
    /// Anything else.
    Failed,
}

/// Which unfinished step stops a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// A suspend; `job suspend` finishes it.
    Suspend,
    /// A restore; `job resume` finishes it.
    Restore,
}

/// The fact behind a refusal, with its data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Reason {
    /// This node does not know the job.
    UnknownJob {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
    },
    /// Another node owns the job.
    NotOwner {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// This node handed the job to another node; its copy never runs again.
    Departed {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The owner.
        #[serde(with = "text")]
        owner: NodeId,
        /// The current epoch.
        epoch: Epoch,
    },
    /// The epoch presented is older than the one this node knows.
    StaleEpoch {
        /// The epoch presented.
        yours: Epoch,
        /// The epoch this node knows.
        current: Epoch,
    },
    /// The epoch presented is newer than the one this node knows.
    EpochAhead {
        /// The epoch presented.
        yours: Epoch,
        /// The epoch this node knows.
        current: Epoch,
    },
    /// A fencing token named another job.
    WrongJob {
        /// The job the token named.
        #[serde(with = "text")]
        token: JobId,
        /// The job it was checked against.
        #[serde(with = "text")]
        job: JobId,
    },
    /// A move this node started is open; nothing runs until the target decides.
    Departing {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The open move.
        #[serde(with = "text")]
        move_id: MoveId,
        /// The node that decides it.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// A peer's ledger at an epoch at least as high forked from this node's.
    Forked {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer to continue on.
        #[serde(with = "text")]
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
    },
    /// A peer's ledger forked from this node's at a lower epoch; the peer stops.
    ForkBehind {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The highest epoch in the peer's ledger.
        epoch: Epoch,
    },
    /// A peer could not be reached, or gave no answer in time.
    Unreachable {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
    },
    /// A peer is not paired with an address.
    UnknownPeer {
        /// The peer.
        #[serde(with = "text")]
        node: NodeId,
    },
    /// The preflight found rows that stop the move.
    Preflight {
        /// The facts of those rows.
        facts: Vec<String>,
    },
    /// A steal asked without `--force`.
    StealNeedsForce {
        /// What the steal would cost.
        warning: StealWarning,
    },
    /// A harness for the job already runs on this node.
    HarnessRunning {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// Its pid.
        pid: NonZeroU32,
    },
    /// The launched harness does not lead its own process group, or its start time cannot be read.
    HarnessGone {
        /// The pid the client named.
        pid: NonZeroU32,
    },
    /// A session was named for a harness the node no longer records.
    HarnessNotRecorded {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The pid the client named.
        pid: NonZeroU32,
        /// The session it named.
        session: SessionId,
    },
    /// The job ran on this node while a move copied it.
    RanDuringMove {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The target of the move.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The workspace changed after the snapshot a move copied.
    ChangedDuringMove {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The root.
        root: PathBuf,
        /// The target of the move.
        #[serde(with = "text")]
        target: NodeId,
    },
    /// The workspace at the root was left by an earlier tenure.
    StaleWorkspace {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The root.
        root: PathBuf,
        /// The epoch its marker was written at.
        left_at: Epoch,
    },
    /// The job has no workspace on this node.
    NoWorkspace {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
    },
    /// A suspend or a restore did not finish.
    Unfinished {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// Which step.
        step: Step,
    },
    /// The job has no snapshot yet.
    NoSnapshot {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
    },
    /// The restore root holds files the node has no marker for.
    WorkspaceOccupied {
        /// The root.
        root: PathBuf,
    },
    /// A `host:port` the user typed does not parse.
    BadAddress {
        /// What the user typed.
        text: String,
    },
    /// A peer answered with something the protocol does not allow here.
    Peer {
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
    },
    /// A snapshot could not be fetched whole from a peer.
    Fetch {
        /// The snapshot.
        #[serde(with = "text")]
        snapshot: SnapshotId,
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
    },
    /// A peer knows a newer epoch of the job than this node: the job moved
    /// on without this node, which catches up with `fabric job resume`.
    PeerAhead {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The epoch the peer knows.
        epoch: Epoch,
    },
    /// A peer is at an older epoch of the job and must catch up first.
    PeerBehind {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The epoch the peer is at.
        epoch: Epoch,
    },
    /// A peer's ledger of the job does not continue this node's: the chains forked.
    PeerForked {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer.
        #[serde(with = "text")]
        peer: NodeId,
        /// The hash of the peer's last record.
        #[serde(with = "text")]
        head: RecordHash,
    },
    /// The move a peer started does not link to this node's ledger, which
    /// lacks records it builds on; the pull is run again.
    MoveUnlinked {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer that started the move.
        #[serde(with = "text")]
        peer: NodeId,
        /// The hash of this node's last record.
        #[serde(with = "text")]
        head: RecordHash,
    },
    /// The move a peer started is at an epoch behind the one this node knows.
    MoveStale {
        /// The job.
        #[serde(with = "text")]
        job: JobId,
        /// The peer that started the move.
        #[serde(with = "text")]
        peer: NodeId,
        /// The epoch this node knows.
        epoch: Epoch,
    },
    /// The request did not parse, or names an op the node does not serve.
    BadRequest,
    /// A failure with no structure worth typing: a file, the store, a harness adapter.
    Internal,
    /// A reason this client does not know; a newer node sent it.
    #[serde(other)]
    Unrecognized,
}

impl Reason {
    /// The class of refusal this reason belongs to; `None` for one this client does not know.
    #[must_use]
    pub fn kind(&self) -> Option<RefusalKind> {
        Some(match self {
            Reason::UnknownJob { .. }
            | Reason::NotOwner { .. }
            | Reason::Departed { .. }
            | Reason::StaleEpoch { .. }
            | Reason::EpochAhead { .. }
            | Reason::WrongJob { .. } => RefusalKind::NotOwner,
            Reason::Departing { .. } | Reason::Forked { .. } => RefusalKind::Frozen,
            Reason::Unreachable { .. } | Reason::UnknownPeer { .. } => RefusalKind::Unreachable,
            Reason::Preflight { .. } | Reason::StealNeedsForce { .. } => RefusalKind::NeedsForce,
            Reason::HarnessRunning { .. }
            | Reason::HarnessGone { .. }
            | Reason::HarnessNotRecorded { .. }
            | Reason::RanDuringMove { .. }
            | Reason::ChangedDuringMove { .. }
            | Reason::StaleWorkspace { .. }
            | Reason::NoWorkspace { .. }
            | Reason::Unfinished { .. }
            | Reason::NoSnapshot { .. }
            | Reason::WorkspaceOccupied { .. } => RefusalKind::JobState,
            Reason::ForkBehind { .. }
            | Reason::BadAddress { .. }
            | Reason::Peer { .. }
            | Reason::Fetch { .. }
            | Reason::PeerAhead { .. }
            | Reason::PeerBehind { .. }
            | Reason::PeerForked { .. }
            | Reason::MoveUnlinked { .. }
            | Reason::MoveStale { .. }
            | Reason::BadRequest
            | Reason::Internal => RefusalKind::Failed,
            Reason::Unrecognized => return None,
        })
    }
}

/// Why a node refused or failed a command.
///
/// The kind is the reason's own; only a reason this client does not know
/// keeps the kind the node sent with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RefusalWire", into = "RefusalWire")]
pub struct Refusal {
    kind: RefusalKind,
    reason: Reason,
    message: String,
}

impl Refusal {
    /// A refusal for `reason`, with the node's sentence. Its kind is the
    /// reason's; [`Reason::Unrecognized`], which only a reader makes, is
    /// [`RefusalKind::Failed`].
    #[must_use]
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            kind: reason.kind().unwrap_or(RefusalKind::Failed),
            reason,
            message: message.into(),
        }
    }

    /// The class of refusal.
    #[must_use]
    pub fn kind(&self) -> RefusalKind {
        self.kind
    }

    /// The fact behind it.
    #[must_use]
    pub fn reason(&self) -> &Reason {
        &self.reason
    }

    /// The node's sentence, with the command that resolves it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// A refusal as it crosses the socket, before the kind is checked against the reason.
#[derive(Serialize, Deserialize)]
struct RefusalWire {
    kind: RefusalKind,
    reason: Reason,
    message: String,
}

impl TryFrom<RefusalWire> for Refusal {
    type Error = String;

    fn try_from(wire: RefusalWire) -> Result<Self, Self::Error> {
        match wire.reason.kind() {
            Some(kind) if kind != wire.kind => Err(format!(
                "a refusal of kind {:?} cannot have the reason {:?}",
                wire.kind, wire.reason
            )),
            _ => Ok(Self {
                kind: wire.kind,
                reason: wire.reason,
                message: wire.message,
            }),
        }
    }
}

impl From<Refusal> for RefusalWire {
    fn from(refusal: Refusal) -> Self {
        Self {
            kind: refusal.kind,
            reason: refusal.reason,
            message: refusal.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    fn vars(pairs: &[(&str, OsString)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        }
    }

    /// Review finding on PR #13: `PATH` entries and roots the shell
    /// resolves, such as `/opt/homebrew//bin`, were left out because they
    /// are not in the core's one text form, so the node stopped finding
    /// tools the shell found. Each absolute one is kept as written; only
    /// one that is not absolute is left out. The TypeScript SDK's test
    /// reads the same variables to the same environment.
    #[test]
    fn the_client_env_keeps_every_absolute_entry_and_root_as_written() {
        let env = ClientEnv::from_vars(vars(&[
            (
                "PATH",
                "/usr/local/bin/:.:bin::/opt/x/.git:/opt/homebrew//bin:/usr/bin/../bin:/cafe\u{301}/bin://"
                    .into(),
            ),
            ("CARGO_HOME", "/Users/me//.cargo/".into()),
            ("RUSTUP_HOME", ".rustup".into()),
            ("PNPM_HOME", "".into()),
        ]))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&env).unwrap(),
            serde_json::json!({
                "path": [
                    "/usr/local/bin/",
                    "/opt/x/.git",
                    "/opt/homebrew//bin",
                    "/usr/bin/../bin",
                    "/cafe\u{301}/bin",
                    "//"
                ],
                "roots": { "cargo_home": "/Users/me//.cargo/" }
            })
        );
    }

    /// An absolute entry or root that is not UTF-8 cannot cross the
    /// socket; it fails the request instead of vanishing from it.
    #[test]
    fn an_absolute_entry_that_is_not_utf8_fails_instead_of_vanishing() {
        let path = OsString::from_vec(b"/usr/bin:/opt/\xff/bin".to_vec());
        assert_eq!(
            ClientEnv::from_vars(vars(&[("PATH", path)])),
            Err(EnvError {
                var: "PATH",
                value: OsString::from_vec(b"/opt/\xff/bin".to_vec()),
            })
        );
        let root = OsString::from_vec(b"/opt/\xff".to_vec());
        assert!(matches!(
            ClientEnv::from_vars(vars(&[("CARGO_HOME", root)])),
            Err(EnvError {
                var: "CARGO_HOME",
                ..
            })
        ));
        // A relative one is left out, as every relative entry is.
        let path = OsString::from_vec(b"/usr/bin:\xff/bin".to_vec());
        let env = ClientEnv::from_vars(vars(&[("PATH", path)])).unwrap();
        assert_eq!(env.path_var(), "/usr/bin");
    }
}
