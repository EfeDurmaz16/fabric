//! The generic Agent Client Protocol adapter.
//!
//! Invariant: any agent that speaks ACP and advertises `session/load` or
//! `session/resume` can be continued in a new workspace by its session id
//! alone; Fabric carries, from the agent's declared state directory, only
//! the files whose path has a component that is the session id or the id
//! plus an extension, as opaque bytes it never
//! parses, and refuses a file that holds a secret-shaped string. An agent
//! whose state directory is not declared cannot be captured or restored,
//! and the adapter says so instead of guessing. Restore accepts only the
//! workspace path and home of the capture (ADR 0011) and only files under
//! the declared state directory whose path names the session. The adapter
//! cannot tell whether a generic agent is writing a session while it
//! restores one; the caller owns that guard, through the handle the
//! launch returned. The trait's launch returns the agent held, before its
//! exec, and its driver sends the first message only once the caller
//! releases it, so the node records the agent before it can act (issue
//! #17). A handshake that fails kills the agent's process group, so no
//! agent runs on without a session.

pub mod client;

use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::{HarnessKind, ProgramName};
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::departure::{FABRIC_DIR, Records};
use crate::error::HarnessError;
use crate::harness::{
    Harness, Installed, Launch, MAX_ANSWER, Restored, Running, TurnEvent, TurnFeed, TurnReport,
};
use crate::manifest::{Content, HOME_PREFIX, Mutability, StateManifest, under_home};
use crate::pipeline::{self, Capturer, Destination};
use crate::process::{self, Wiring};
use crate::redact::{RedactionCounts, Redactor};
use crate::stage::{Carry, Source};

use self::client::{AcpClient, AcpError, PermissionPolicy, StopReason};

/// True when a path component is the session's own: the id itself (a
/// directory) or the id followed by an extension (`<sid>.json`). Session
/// `s1` does not own `s10.json` or `xs1y`.
fn names_session(part: &str, session: &SessionId) -> bool {
    part.strip_prefix(session.as_str())
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Refuses launch arguments that hold a secret shape, one by one and
/// joined, so `--api-key sk-...` is caught either way: the arguments are
/// part of the harness kind that the job's ledger and the manifest record,
/// and neither is ever redacted. A caller that records a kind checks it
/// here before it writes anything.
pub fn refuse_secret_args(program: &ProgramName, args: &[String]) -> Result<(), HarnessError> {
    let redactor = Redactor::standard()?;
    let joined = args.join(" ");
    if redactor.matches(&joined) || args.iter().any(|arg| redactor.matches(arg)) {
        return Err(HarnessError::SecretInArguments {
            program: program.to_string(),
        });
    }
    Ok(())
}

/// An ACP agent started as `program args...`.
#[derive(Debug, Clone)]
pub struct AcpHarness {
    program: ProgramName,
    args: Vec<String>,
    home: AbsPath,
    state_dir: Option<PathBuf>,
    policy: PermissionPolicy,
    path_var: OsString,
}

/// An ACP agent this node started, with the client that owns its pipes.
/// Dropping the client closes the agent's stdin.
pub struct AcpRunning {
    /// The process handle the supervisor records.
    pub running: Running,
    /// The protocol connection.
    pub client: AcpClient,
    /// The first prompt turn, when `launch_acp` was given a prompt; its
    /// result says how the turn ended.
    pub turn: Option<JoinHandle<Result<StopReason, AcpError>>>,
}

impl std::fmt::Debug for AcpRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpRunning")
            .field("running", &self.running)
            .field("turn_pending", &self.turn.is_some())
            .finish_non_exhaustive()
    }
}

impl AcpHarness {
    /// An adapter for `program args...` under `home`, finding the program
    /// on the `PATH` value `path_var`, with no state directory declared and
    /// the deny permission policy.
    #[must_use]
    pub fn new(program: ProgramName, args: Vec<String>, home: AbsPath, path_var: OsString) -> Self {
        AcpHarness {
            program,
            args,
            home,
            state_dir: None,
            policy: PermissionPolicy::Deny,
            path_var,
        }
    }

    /// Declares where the agent keeps its sessions; refused when `dir` is
    /// not under the home, or when it is the home or lies under
    /// `~/.fabric`, in any letter case: either way capture would walk
    /// Fabric's own departure records, which never travel.
    pub fn with_state_dir(mut self, dir: &AbsPath) -> Result<Self, HarnessError> {
        let dir = pipeline::fs_path(dir).to_path_buf();
        let Ok(rel) = dir.strip_prefix(self.home_dir()) else {
            return Err(HarnessError::NotUnderHome {
                path: dir,
                home: self.home_dir().to_path_buf(),
            });
        };
        let holds_fabric = rel
            .components()
            .next()
            .is_none_or(|first| first.as_os_str().eq_ignore_ascii_case(FABRIC_DIR));
        if holds_fabric {
            return Err(HarnessError::StateDirHoldsFabric {
                path: dir,
                fabric: self.home_dir().join(FABRIC_DIR),
            });
        }
        self.state_dir = Some(dir);
        Ok(self)
    }

    fn home_dir(&self) -> &Path {
        pipeline::fs_path(&self.home)
    }

    /// Refuses, before any session is placed or taken, a state directory
    /// that capture and restore would refuse: none declared, or one reached
    /// through a link below the home. A target checks this before it takes
    /// a job it would then be unable to resume.
    pub fn check_state_dir(&self) -> Result<(), HarnessError> {
        let (dir, _) = self.state_rel()?;
        pipeline::refuse_symlinks(self.home_dir(), &dir)
    }

    /// Sets how `session/request_permission` is answered.
    #[must_use]
    pub fn with_policy(mut self, policy: PermissionPolicy) -> Self {
        self.policy = policy;
        self
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        process::find_on_path(self.program.as_str(), &self.path_var).ok_or_else(|| {
            HarnessError::NotInstalled {
                program: self.program.to_string(),
            }
        })
    }

    fn state_rel(&self) -> Result<(PathBuf, Vec<String>), HarnessError> {
        let dir = self
            .state_dir
            .clone()
            .ok_or_else(|| HarnessError::StateDirNotDeclared {
                program: self.program.to_string(),
            })?;
        let rel = dir
            .strip_prefix(self.home_dir())
            .map_err(|_| HarnessError::NotUnderHome {
                path: dir.clone(),
                home: self.home_dir().to_path_buf(),
            })?;
        Ok((dir.clone(), pipeline::rel_components(rel)?))
    }

    /// Where this node records the session's files at capture. An ACP
    /// agent's state is opaque, so every file is treated as rewritable.
    /// The records are keyed by the whole harness kind, program and
    /// arguments, so two agents started through one program, such as
    /// `node`, never share a record for a session id they both use.
    fn departure(&self, session: &SessionId) -> Records {
        let mut kind = blake3::Hasher::new();
        for part in
            std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str))
        {
            kind.update(&(part.len() as u64).to_le_bytes());
            kind.update(part.as_bytes());
        }
        let kind = kind.finalize().to_hex();
        let label = format!("acp-{}-{}", self.program, &kind[..16]);
        Records::of(self.home_dir(), &label, session)
    }

    fn refuse_secret_args(&self) -> Result<(), HarnessError> {
        refuse_secret_args(&self.program, &self.args)
    }

    fn capture_sync(
        &self,
        root: &AbsPath,
        session: &SessionId,
        out: &Path,
        version: Option<String>,
    ) -> Result<StateManifest, HarnessError> {
        self.refuse_secret_args()?;
        let (dir, rel_parts) = self.state_rel()?;
        // The walk refuses symlinks below the directory but follows the
        // directory itself, so a linked state root or a linked directory
        // above it would carry files from outside the home.
        pipeline::refuse_symlinks(self.home_dir(), &dir)?;
        let sources = Self::session_sources(&dir, &rel_parts, session)?;
        if sources.is_empty() {
            return Err(HarnessError::SessionNotFound {
                kind: self.kind(),
                session: session.clone(),
                searched: dir,
            });
        }
        let records = self.departure(session);
        let capturer = Capturer {
            kind: self.kind(),
            session,
            root,
            home: &self.home,
            version,
            departure: Some(&records),
        };
        let manifest = pipeline::capture(capturer, &sources, RedactionCounts::new(), out)?;
        info!(%session, agent = %self.program, files = manifest.files().len(), "acp: captured");
        Ok(manifest)
    }

    /// Every file under the state directory `dir` that is named for
    /// `session`, as capture takes it; `rel_parts` is `dir` under the home.
    fn session_sources(
        dir: &Path,
        rel_parts: &[String],
        session: &SessionId,
    ) -> Result<Vec<Source>, HarnessError> {
        let mut sources = Vec::new();
        for file in pipeline::walk(dir)? {
            let file_parts = pipeline::rel_components(&file)?;
            if !file_parts.iter().any(|part| names_session(part, session)) {
                continue;
            }
            let mut parts: Vec<&str> = vec![HOME_PREFIX];
            parts.extend(rel_parts.iter().map(String::as_str));
            parts.extend(file_parts.iter().map(String::as_str));
            sources.push(Source {
                abs: dir.join(&file),
                rel: pipeline::rel_path(&parts)?,
                carry: Carry::OpaqueRefuseSecrets,
                mutability: Mutability::Replaceable,
            });
        }
        Ok(sources)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let version =
            process::installed_version(self.binary().ok(), &self.path_var, self.program.as_str())
                .await;
        let adapter = self.clone();
        let session = session.clone();
        let out = out.to_path_buf();
        pipeline::blocking(move || adapter.capture_sync(&root, &session, &out, version)).await
    }

    /// True when a tree path lies under the declared state directory and
    /// one of its components below it names the session, which is the rule
    /// capture selects files by.
    fn restorable(rel: &str, state_parts: &[String], session: &SessionId) -> bool {
        let Some(rest) = under_home(rel) else {
            return false;
        };
        let mut parts = rest.split('/');
        for expected in state_parts {
            if parts.next() != Some(expected.as_str()) {
                return false;
            }
        }
        parts.any(|part| names_session(part, session))
    }

    fn restore_sync(
        &self,
        state: &Path,
        manifest: StateManifest,
    ) -> Result<Restored, HarnessError> {
        let (dir, state_parts) = self.state_rel()?;
        let session = manifest.session().clone();
        let records = self.departure(&session);
        pipeline::refuse_symlinks(self.home_dir(), &dir)?;
        let held = if dir.exists() {
            Self::session_sources(&dir, &state_parts, &session)?
        } else {
            Vec::new()
        };
        let restored = pipeline::restore(manifest, state, Some(&records), &held, |rel, _| {
            let rest = under_home(rel).filter(|_| Self::restorable(rel, &state_parts, &session));
            Ok(rest.map(|rest| Destination {
                path: self.home_dir().join(rest),
                content: Content::Opaque,
                mutability: Mutability::Replaceable,
            }))
        })?;
        info!(%session, agent = %self.program, files = restored.placed.len(), "acp: restored");
        Ok(restored)
    }

    async fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let manifest = pipeline::open(state, &self.kind(), &root, &self.home)?;
        let adapter = self.clone();
        let state = state.to_path_buf();
        pipeline::blocking(move || adapter.restore_sync(&state, manifest)).await
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        process::detect(self.binary().ok(), &self.path_var).await
    }

    /// Starts the agent held, before its exec, and attaches the protocol
    /// client to its pipes. The client writes nothing until the handle is
    /// released, and then first the line the agent's trampoline waits for.
    async fn spawn_attached(
        &self,
        root: &AbsPath,
        env: &[(EnvVarName, String)],
    ) -> Result<(Running, AcpClient), HarnessError> {
        let binary = self.binary()?;
        let mut spawned = process::spawn(
            &binary,
            &self.args,
            pipeline::fs_path(root),
            env,
            Wiring::Rpc,
        )
        .await?;
        let (Some(stdin), Some(stdout)) = (spawned.child.stdin.take(), spawned.child.stdout.take())
        else {
            // Dropping the held handle kills its group.
            drop(Running::new(None, spawned));
            return Err(HarnessError::Spawn {
                program: self.program.to_string(),
                source: std::io::Error::other("agent started without stdio pipes"),
            });
        };
        let client = match spawned.go.take() {
            Some(go) => AcpClient::attach_held(stdin, stdout, self.policy, go),
            None => AcpClient::attach(stdin, stdout, self.policy),
        };
        Ok((Running::new(None, spawned), client))
    }

    /// Starts the agent, completes the handshake, continues `resume` or
    /// creates a session, and returns the process with its protocol client,
    /// for a caller that drives the session itself. The agent is released
    /// at once, since no node records it first: a launch a node supervises
    /// goes through `Harness::launch`, which returns the agent held. A
    /// prompt is sent on a task of its own, whose handle is on the result.
    pub async fn launch_acp(
        &self,
        workspace: &Path,
        resume: Option<SessionId>,
        prompt: Option<String>,
        env: &[(EnvVarName, String)],
    ) -> Result<AcpRunning, HarnessError> {
        let root = pipeline::physical_workspace(workspace)?;
        let (mut running, client) = self.spawn_attached(&root, env).await?;
        running.release()?;
        // Until the session is open, an error or a dropped future kills the
        // agent's group, so no agent is left running without a session.
        let guard = process::GroupGuard::new(running.pid());
        let session = self.open_session(&client, &root, resume).await?;
        guard.disarm();
        let turn = prompt.map(|prompt| {
            let client = client.clone();
            let session = session.clone();
            tokio::spawn(async move { client.prompt(&session, &prompt).await })
        });
        running.set_session(session);
        Ok(AcpRunning {
            running,
            client,
            turn,
        })
    }

    /// Runs the handshake and continues `resume`, or opens a new session
    /// when there is none; returns the session.
    async fn open_session(
        &self,
        client: &AcpClient,
        cwd: &AbsPath,
        resume: Option<SessionId>,
    ) -> Result<SessionId, HarnessError> {
        let capabilities = client.initialize().await?;
        match resume {
            None => {
                let session = client.new_session(cwd).await?;
                info!(%session, agent = %self.program, "acp: new session");
                Ok(session)
            }
            Some(session) => {
                if capabilities.resume {
                    client.resume_session(&session, cwd).await?;
                } else if capabilities.load_session {
                    client.load_session(&session, cwd).await?;
                } else {
                    return Err(HarnessError::LoadNotSupported {
                        program: self.program.to_string(),
                    });
                }
                info!(%session, agent = %self.program, "acp: session continued");
                Ok(session)
            }
        }
    }

    /// The trait's launch, headless only: an ACP agent has no terminal
    /// mode, so `Launch::Interactive` is refused (`NeedsClient`). The agent
    /// comes back held, with the session it continues, or none for a new
    /// one, and its driver sends nothing before `Running::release` lets it
    /// run, so a node records it before it can act (issue #17). The driver
    /// then runs the handshake, opens or continues the session, sends the
    /// prompt, gathers the agent's answer from the turn's updates, closes
    /// the agent's stdin when the turn ends, and says each step on the
    /// handle's turn feed (`Running::take_turn`). Updates past
    /// `client::UPDATE_BUFFER`, or past `client::UPDATE_BUFFER_BYTES` of
    /// their lines, unread are dropped, and the driver reads them as they
    /// come.
    async fn launch_impl(
        &self,
        workspace: &Path,
        launch: Launch,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let Launch::Headless { resume, prompt } = launch else {
            return Err(HarnessError::NeedsClient {
                program: self.program.to_string(),
            });
        };
        let root = pipeline::physical_workspace(workspace)?;
        let (running, client) = self.spawn_attached(&root, env).await?;
        let (gate, released) = oneshot::channel();
        let (events, feed) = TurnFeed::channel();
        let driver = Driver {
            adapter: self.clone(),
            pid: running.pid(),
            client,
            root,
            resume: resume.clone(),
            prompt,
            events,
        };
        tokio::spawn(driver.run(released));
        let mut running = running.with_driver(gate, feed);
        if let Some(session) = resume {
            running.set_session(session);
        }
        Ok(running)
    }
}

/// The turn the trait's launch of an ACP agent runs once the agent is
/// released.
struct Driver {
    adapter: AcpHarness,
    pid: NonZeroU32,
    client: AcpClient,
    root: AbsPath,
    resume: Option<SessionId>,
    prompt: String,
    events: mpsc::Sender<TurnEvent>,
}

impl Driver {
    /// Waits for `released`, then runs the turn. A handle dropped while
    /// held killed the agent's group and drops the gate, so the driver
    /// ends with nothing sent. A handshake that fails kills the group.
    async fn run(self, released: oneshot::Receiver<()>) {
        if released.await.is_err() {
            return;
        }
        let Driver {
            adapter,
            pid,
            client,
            root,
            resume,
            prompt,
            events,
        } = self;
        let program = adapter.program.to_string();
        let mut report = TurnReport {
            session: resume.clone(),
            ..TurnReport::default()
        };
        // Until the session is open, an error, or the runtime dropping
        // this task, kills the agent's group.
        let guard = process::GroupGuard::new(pid);
        let session = match adapter.open_session(&client, &root, resume).await {
            Ok(session) => {
                guard.disarm();
                session
            }
            Err(e) => {
                drop(guard);
                warn!(agent = %program, error = %e, "acp: the session could not be opened, so the agent was stopped");
                report.failure = Some(format!(
                    "{program} could not open the session, so it was stopped: {e}"
                ));
                let _ = events.send(TurnEvent::Ended(report)).await;
                return;
            }
        };
        report.session = Some(session.clone());
        // The receiver may be gone, as when the caller never asked; the turn runs on.
        let _ = events.send(TurnEvent::Session(session.clone())).await;
        let (answer, outcome) = turn(&client, &session, &prompt).await;
        if answer.left_out > 0 {
            warn!(agent = %program, left_out = answer.left_out, "acp: the answer ran past {MAX_ANSWER} bytes; the rest was left out");
        }
        report.answer_left_out = answer.left_out;
        report.answer = (!answer.text.is_empty()).then_some(answer.text);
        report.failure = match outcome {
            Ok(StopReason::EndTurn) => {
                info!(agent = %program, "acp: turn ended");
                None
            }
            Ok(reason) => {
                warn!(agent = %program, ?reason, "acp: turn stopped early");
                Some(format!(
                    "{program} stopped the turn: {}",
                    stop_words(&reason)
                ))
            }
            Err(e) => {
                warn!(agent = %program, error = %e, "acp: turn failed");
                Some(format!("{program} failed the turn: {e}"))
            }
        };
        let dropped = client.dropped_updates();
        if dropped > 0 {
            warn!(agent = %program, dropped, "acp: updates were dropped, so the answer may be short");
        }
        let _ = events.send(TurnEvent::Ended(report)).await;
        // The last handle on the client: dropping it closes the agent's
        // stdin, so it exits.
        drop(client);
    }
}

/// The answer a turn gathers: the first `MAX_ANSWER` bytes of its text,
/// and how many bytes came after them.
#[derive(Debug, Default)]
struct Answer {
    text: String,
    left_out: u64,
}

impl Answer {
    /// Appends `text`, as much of it as fits in `max` bytes of answer, cut
    /// at a character; once a piece is cut, every later one is left out,
    /// so the answer is always the turn's first bytes.
    fn keep(&mut self, text: &str, max: usize) {
        let room = max.saturating_sub(self.text.len());
        if self.left_out == 0 && text.len() <= room {
            self.text.push_str(text);
            return;
        }
        let kept = if self.left_out == 0 {
            text.floor_char_boundary(room)
        } else {
            0
        };
        self.text.push_str(&text[..kept]);
        self.left_out += (text.len() - kept) as u64;
    }
}

/// Sends `prompt` in `session` and gathers the text of the agent's
/// message chunks for that session until the turn ends, at most
/// `MAX_ANSWER` bytes of it; returns the answer and how the turn ended.
async fn turn(
    client: &AcpClient,
    session: &SessionId,
    prompt: &str,
) -> (Answer, Result<StopReason, AcpError>) {
    let mut answer = Answer::default();
    let asked = client.prompt(session, prompt);
    tokio::pin!(asked);
    let outcome = loop {
        tokio::select! {
            // Updates first: the agent sends them before its answer to the
            // prompt, and the client queues them in that order.
            biased;
            update = client.next_update() => match update {
                Some(update) if &update.session == session => note_chunk(&mut answer, &update.update),
                Some(_) => {}
                None => break (&mut asked).await,
            },
            outcome = &mut asked => break outcome,
        }
    };
    // Updates queued before the answer, read while it was being taken.
    while let Ok(Some(update)) = tokio::time::timeout(Duration::ZERO, client.next_update()).await {
        if &update.session == session {
            note_chunk(&mut answer, &update.update);
        }
    }
    (answer, outcome)
}

/// Appends the text of an `agent_message_chunk` update to `answer`, as
/// far as `MAX_ANSWER` allows; leaves out every other update and content
/// that is not text.
fn note_chunk(answer: &mut Answer, update: &Value) {
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("agent_message_chunk") {
        return;
    }
    let content = update.get("content");
    if content.and_then(|c| c.get("type")).and_then(Value::as_str) == Some("text")
        && let Some(text) = content.and_then(|c| c.get("text")).and_then(Value::as_str)
    {
        answer.keep(text, MAX_ANSWER);
    }
}

/// A stop reason as the protocol names it.
fn stop_words(reason: &StopReason) -> &str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        StopReason::Other(other) => other,
    }
}

impl Harness for AcpHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Acp {
            program: self.program.clone(),
            args: self.args.clone(),
        }
    }

    fn detect(&self) -> BoxFuture<'_, Result<Option<Installed>, HarnessError>> {
        Box::pin(self.detect_impl())
    }

    fn capture<'a>(
        &'a self,
        workspace: &'a Path,
        session: &'a SessionId,
        out: &'a Path,
    ) -> BoxFuture<'a, Result<StateManifest, HarnessError>> {
        Box::pin(self.capture_impl(workspace, session, out))
    }

    fn confirm_departure<'a>(
        &'a self,
        manifest: &'a StateManifest,
    ) -> BoxFuture<'a, Result<(), HarnessError>> {
        let records = self.departure(manifest.session());
        let manifest = manifest.clone();
        let kind = self.kind();
        Box::pin(pipeline::blocking(move || {
            pipeline::confirm_departure(&manifest, &kind, Some(&records))
        }))
    }

    fn restore<'a>(
        &'a self,
        workspace: &'a Path,
        state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>> {
        Box::pin(self.restore_impl(workspace, state))
    }

    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(self.launch_impl(workspace, launch, env))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(args: &[&str]) -> AcpHarness {
        AcpHarness::new(
            ProgramName::parse("node").unwrap(),
            args.iter().map(|a| (*a).to_owned()).collect(),
            AbsPath::parse("/h").unwrap(),
            OsString::new(),
        )
    }

    #[test]
    fn departure_records_are_keyed_by_program_and_arguments() {
        let session = SessionId::parse("s1").unwrap();
        let a = adapter(&["agent-a.js"]).departure(&session);
        assert_eq!(a, adapter(&["agent-a.js"]).departure(&session));
        assert_ne!(a, adapter(&["agent-b.js"]).departure(&session));
        assert_ne!(
            adapter(&["a", "b"]).departure(&session),
            adapter(&["ab"]).departure(&session),
            "argument boundaries count"
        );
    }

    /// The lines of an agent that answers the prompt, request 1 of its
    /// client, after `chunks` `agent_message_chunk` updates of `text`.
    fn chunking_agent(dir: &Path, chunks: usize, text: &str) -> tokio::process::Child {
        let mut line = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text }
                }
            }
        }))
        .unwrap();
        line.push('\n');
        let update = dir.join("update.jsonl");
        std::fs::write(&update, &line).unwrap();
        let answer = r#"{"jsonrpc":"2.0","id":1,"result":{"stopReason":"end_turn"}}"#;
        let script = format!(
            "read -r prompt; i=0; while [ $i -lt {chunks} ]; do cat \"$0\"; i=$((i+1)); done; echo '{answer}'; read -r rest"
        );
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg(&update)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    /// Review of PR #31: M2-23 bounds the updates nobody reads, but a
    /// headless turn reads them as they come and gathered every chunk's
    /// text into one answer, so an agent that streams a long turn made
    /// the command hold all of it. The answer keeps its first
    /// `MAX_ANSWER` bytes and counts the rest, as a Claude or Codex
    /// answer is bounded by the line it comes on.
    #[tokio::test]
    async fn a_drained_turn_keeps_at_most_max_answer_bytes_of_answer() {
        const CHUNKS: usize = 24;
        const CHUNK: usize = 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let mut child = chunking_agent(dir.path(), CHUNKS, &"a".repeat(CHUNK));
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let client = AcpClient::attach(stdin, stdout, PermissionPolicy::Deny);
        let session = SessionId::parse("s1").unwrap();
        let (answer, outcome) =
            tokio::time::timeout(Duration::from_mins(1), turn(&client, &session, "hi"))
                .await
                .expect("the turn did not end");
        assert!(matches!(outcome, Ok(StopReason::EndTurn)), "{outcome:?}");
        assert!(
            answer.text.len() <= MAX_ANSWER,
            "the turn held {} bytes of answer, {CHUNKS} chunks of {CHUNK} bytes",
            answer.text.len()
        );
        // An update the queue dropped never reached the turn; the rest did.
        let dropped = usize::try_from(client.dropped_updates()).unwrap();
        assert_eq!(
            answer.text.len() as u64 + answer.left_out,
            ((CHUNKS - dropped) * CHUNK) as u64
        );
        if dropped == 0 {
            assert_eq!(answer.text.len(), MAX_ANSWER);
        }
        drop(client);
        child.kill().await.unwrap();
    }

    /// The answer is always the turn's first bytes: a piece that does not
    /// fit is cut at a character, and every later piece is left out, even
    /// one short enough for the bytes the cut left free.
    #[test]
    fn an_answer_is_cut_at_a_character_and_stays_a_prefix() {
        let mut answer = Answer::default();
        answer.keep("abc", 7);
        answer.keep("d\u{e9}\u{e9}", 7);
        assert_eq!((answer.text.as_str(), answer.left_out), ("abcd\u{e9}", 2));
        answer.keep("x", 7);
        assert_eq!((answer.text.as_str(), answer.left_out), ("abcd\u{e9}", 3));

        let mut whole = Answer::default();
        whole.keep("abc", 3);
        whole.keep("", 3);
        assert_eq!((whole.text.as_str(), whole.left_out), ("abc", 0));
    }
}
