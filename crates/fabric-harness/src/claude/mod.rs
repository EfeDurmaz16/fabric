//! The Claude Code adapter.
//!
//! Invariant: a Claude session is carried as the files Claude itself reads
//! on `claude --resume <session-id>`: the transcript and sidecars under
//! every `~/.claude/projects/<key>/` that holds them, the project memory,
//! `file-history`, `tasks` and `uploads` for the session, each one
//! redacted, JSON through the signed-block rule and everything else line
//! by line. Files inside the workspace (`.claude/`, `CLAUDE.local.md`) are
//! the workspace snapshot's to carry (ADR 0004), never this adapter's.
//! Credentials, the global `~/.claude.json`, the process registry and the
//! supervisor state never travel (see `denylist`), and a symlink in
//! session state fails the capture instead of being followed. On
//! restore the project key and every path-bearing string outside a signed
//! block are mapped to the new workspace and home; signed blocks are never
//! edited (ADR 0006). Login is never copied and no per-job config
//! directory is set: `CLAUDE_CONFIG_DIR` keys the login (ADR 0006, D15).
//!
//! Surfaces used, checked against `claude --help` of 2.1.294 on
//! 2026-10-19: `--resume <id>`, `--session-id <uuid>`, `-p` with
//! `--output-format json`, and pass-through flags such as `--settings`,
//! `--mcp-config` and `--permission-mode`.

pub mod slug;
pub mod trust;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fabric_core::BoxFuture;
use fabric_core::env::EnvVarName;
use fabric_core::harness::HarnessKind;
use tracing::{info, warn};

use crate::error::HarnessError;
use crate::files::{self, Source};
use crate::harness::{self, Harness, Installed, Io, Launch, Restored, Running};
use crate::manifest::{HOME_PREFIX, MANIFEST_FORMAT, Redaction, Rewrite, StateManifest};
use crate::process;
use crate::rewrite::{PathMap, RedactionCounts};
use crate::session::SessionId;

/// The program name on `PATH`.
pub const PROGRAM: &str = "claude";

/// The Claude Code adapter for one home directory.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    home: PathBuf,
    config_dir: PathBuf,
    config_json: PathBuf,
    binary: Option<PathBuf>,
    path_var: Option<OsString>,
    flags: Vec<String>,
}

impl ClaudeCode {
    /// An adapter whose state lives under `home/.claude` and `home/.claude.json`.
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        ClaudeCode {
            config_dir: home.join(".claude"),
            config_json: home.join(".claude.json"),
            home,
            binary: None,
            path_var: std::env::var_os("PATH"),
            flags: Vec::new(),
        }
    }

    /// Uses this binary instead of looking `claude` up on `PATH`.
    #[must_use]
    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    /// Looks `claude` up on this `PATH` value instead of the process's.
    #[must_use]
    pub fn with_path_var(mut self, path_var: OsString) -> Self {
        self.path_var = Some(path_var);
        self
    }

    /// Flags passed on every launch, such as `--settings <file>`,
    /// `--mcp-config <file>` or `--permission-mode acceptEdits`; resume
    /// restores none of them (ADR 0006).
    #[must_use]
    pub fn with_flags(mut self, flags: Vec<String>) -> Self {
        self.flags = flags;
        self
    }

    /// Where this adapter reads Claude's state.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    fn binary(&self) -> Result<PathBuf, HarnessError> {
        if let Some(binary) = &self.binary {
            return Ok(binary.clone());
        }
        process::find_on_path(PROGRAM, self.path_var.as_deref()).ok_or_else(|| {
            HarnessError::NotInstalled {
                program: PROGRAM.to_owned(),
            }
        })
    }

    fn key_of(root: &Path) -> Result<String, HarnessError> {
        slug::slug(root).map_err(|e| HarnessError::Malformed {
            what: "workspace path",
            path: root.to_path_buf(),
            reason: e.to_string(),
        })
    }

    /// The newest session whose transcript sits under the workspace's project key.
    pub fn latest_session(&self, workspace: &Path) -> Result<Option<SessionId>, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let dir = self.config_dir.join("projects").join(Self::key_of(&root)?);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(HarnessError::io("walk", &dir, e)),
        };
        let mut newest: Option<(std::time::SystemTime, SessionId)> = None;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io("walk", &dir, e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(session) = SessionId::parse(stem) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_err(|e| HarnessError::io("read", &path, e))?;
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, session));
            }
        }
        Ok(newest.map(|(_, s)| s))
    }

    /// Sets `projects["<root>"].hasTrustDialogAccepted` in `~/.claude.json`
    /// while no `claude` process runs. Returns true when the file changed.
    pub async fn preseed_trust(&self, workspace: &Path) -> Result<bool, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary().ok();
        let pids = process::running_pids(PROGRAM, binary.as_deref()).await?;
        if !pids.is_empty() {
            return Err(HarnessError::HarnessRunning {
                program: PROGRAM.to_owned(),
                pids,
            });
        }
        trust::preseed_file(&self.config_json, &root)
    }

    fn rewrite_for(path: &Path) -> Rewrite {
        match path.extension().and_then(|e| e.to_str()) {
            Some("jsonl" | "json") => Rewrite::JsonText,
            _ => Rewrite::Text,
        }
    }

    fn add_tree(
        sources: &mut Vec<Source>,
        dir: &Path,
        prefix: &[&str],
        tree_base: &Path,
    ) -> Result<(), HarnessError> {
        match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(HarnessError::SymlinkInState {
                    path: dir.to_path_buf(),
                });
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(HarnessError::io("read", dir, e)),
        }
        for file in files::walk(dir)? {
            let abs = dir.join(&file);
            let rel_to_base = abs.strip_prefix(tree_base).unwrap_or(&file);
            let parts = files::rel_components(rel_to_base)?;
            let mut all: Vec<&str> = prefix.to_vec();
            all.extend(parts.iter().map(String::as_str));
            sources.push(Source {
                rel: files::rel_path(&all)?,
                rewrite: Self::rewrite_for(&abs),
                abs,
            });
        }
        Ok(())
    }

    fn add_file(
        sources: &mut Vec<Source>,
        abs: &Path,
        prefix: &[&str],
        tree_base: &Path,
    ) -> Result<bool, HarnessError> {
        match std::fs::symlink_metadata(abs) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(HarnessError::SymlinkInState {
                    path: abs.to_path_buf(),
                });
            }
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(HarnessError::io("read", abs, e)),
        }
        let rel_to_base = abs.strip_prefix(tree_base).unwrap_or(abs);
        let parts = files::rel_components(rel_to_base)?;
        let mut all: Vec<&str> = prefix.to_vec();
        all.extend(parts.iter().map(String::as_str));
        sources.push(Source {
            rel: files::rel_path(&all)?,
            rewrite: Self::rewrite_for(abs),
            abs: abs.to_path_buf(),
        });
        Ok(true)
    }

    fn sources_for(&self, root: &Path, session: &SessionId) -> Result<Vec<Source>, HarnessError> {
        let home_prefix = [HOME_PREFIX];
        let mut sources = Vec::new();
        let projects = self.config_dir.join("projects");
        let mut found_transcript = false;
        if let Ok(entries) = std::fs::read_dir(&projects) {
            let mut keys: Vec<PathBuf> = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| HarnessError::io("walk", &projects, e))?;
                let path = entry.path();
                let kind = entry
                    .file_type()
                    .map_err(|e| HarnessError::io("walk", &path, e))?;
                if kind.is_dir() {
                    keys.push(path);
                } else if kind.is_symlink()
                    && (path.join(format!("{session}.jsonl")).exists()
                        || path.join(session.as_str()).exists())
                {
                    return Err(HarnessError::SymlinkInState { path });
                }
            }
            keys.sort();
            for key_dir in keys {
                let transcript = key_dir.join(format!("{session}.jsonl"));
                if Self::add_file(&mut sources, &transcript, &home_prefix, &self.home)? {
                    found_transcript = true;
                }
                Self::add_tree(
                    &mut sources,
                    &key_dir.join(session.as_str()),
                    &home_prefix,
                    &self.home,
                )?;
            }
        }
        if !found_transcript {
            return Err(HarnessError::SessionNotFound {
                kind: HarnessKind::ClaudeCode,
                session: session.clone(),
                searched: projects,
            });
        }
        let key = Self::key_of(root)?;
        Self::add_tree(
            &mut sources,
            &projects.join(&key).join("memory"),
            &home_prefix,
            &self.home,
        )?;
        Self::add_tree(
            &mut sources,
            &self.config_dir.join("file-history").join(session.as_str()),
            &home_prefix,
            &self.home,
        )?;
        let prefix8: String = session.as_str().chars().take(8).collect();
        Self::add_tree(
            &mut sources,
            &self
                .config_dir
                .join("tasks")
                .join(format!("session-{prefix8}")),
            &home_prefix,
            &self.home,
        )?;
        Self::add_tree(
            &mut sources,
            &self.config_dir.join("uploads").join(session.as_str()),
            &home_prefix,
            &self.home,
        )?;
        Ok(sources)
    }

    async fn capture_impl(
        &self,
        workspace: &Path,
        session: &SessionId,
        out: &Path,
    ) -> Result<StateManifest, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let sources = self.sources_for(&root, session)?;
        let mut counts = RedactionCounts::new();
        let carried = files::stage(&sources, out, &mut counts)?;
        let version = match self.detect_impl().await {
            Ok(Some(installed)) => installed.version,
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "claude: version probe failed; manifest records none");
                None
            }
        };
        let manifest = StateManifest {
            format: MANIFEST_FORMAT,
            kind: HarnessKind::ClaudeCode,
            session: session.clone(),
            root: harness::abs_path(&root)?,
            home: harness::abs_path(&self.home)?,
            harness_version: version,
            captured_at_ms: process::now_ms(),
            files: carried,
            redactions: counts
                .into_iter()
                .map(|(rule, count)| Redaction {
                    rule: rule.to_owned(),
                    count,
                })
                .collect(),
        };
        manifest.write(out)?;
        info!(%session, files = manifest.files.len(), "claude: captured");
        Ok(manifest)
    }

    fn restore_impl(&self, workspace: &Path, state: &Path) -> Result<Restored, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let manifest = StateManifest::read(state)?;
        if manifest.kind != HarnessKind::ClaudeCode {
            return Err(HarnessError::WrongHarness {
                captured: manifest.kind.clone(),
                restoring: HarnessKind::ClaudeCode,
            });
        }
        let old_root = PathBuf::from(manifest.root.as_str());
        let old_home = PathBuf::from(manifest.home.as_str());
        let old_key = Self::key_of(&old_root)?;
        let new_key = Self::key_of(&root)?;
        let new_home_text = harness::abs_path(&self.home)?;
        let new_root_text = harness::abs_path(&root)?;
        let map = PathMap::new([
            (
                manifest.root.as_str().to_owned(),
                new_root_text.as_str().to_owned(),
            ),
            (
                manifest.home.as_str().to_owned(),
                new_home_text.as_str().to_owned(),
            ),
            (old_key.clone(), new_key.clone()),
        ]);
        let old_key_prefix = format!(".claude/projects/{old_key}/");
        let mut placed = Vec::new();
        for file in &manifest.files {
            let src = state.join(file.rel_path.as_str());
            let text = file.rel_path.as_str();
            let dest = if let Some(rest) = text.strip_prefix("home/") {
                let rest = match rest.strip_prefix(&old_key_prefix) {
                    Some(tail) => format!(".claude/projects/{new_key}/{tail}"),
                    None => rest.to_owned(),
                };
                self.home.join(rest)
            } else if let Some(rest) = text.strip_prefix("workspace/") {
                root.join(rest)
            } else {
                continue;
            };
            let bytes = match file.rewrite {
                Rewrite::JsonText => files::rewrite_json_text(&src, &map)?,
                Rewrite::None | Rewrite::Text => {
                    std::fs::read(&src).map_err(|e| HarnessError::io("read", &src, e))?
                }
            };
            files::place(&dest, &bytes)?;
            placed.push(dest);
        }
        let _ = old_home;
        info!(session = %manifest.session, files = placed.len(), rules = map.rules().len(), "claude: restored");
        Ok(Restored {
            manifest,
            placed,
            rules: map.rules().to_vec(),
        })
    }

    async fn detect_impl(&self) -> Result<Option<Installed>, HarnessError> {
        let path = match self.binary() {
            Ok(path) => path,
            Err(HarnessError::NotInstalled { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let raw = process::probe(&path, &["--version"]).await?;
        let version =
            process::version_token(&raw).ok_or_else(|| HarnessError::UnexpectedVersionOutput {
                program: PROGRAM.to_owned(),
                output: raw.clone(),
            })?;
        Ok(Some(Installed {
            path,
            version: Some(version.to_owned()),
            raw,
        }))
    }

    /// The argument list for `launch`, without the binary.
    pub fn args_for(&self, launch: &Launch, io: Io) -> Result<Vec<String>, HarnessError> {
        let headless = !matches!(io, Io::Terminal);
        let mut args = Vec::new();
        if headless {
            args.push("-p".to_owned());
            args.push("--output-format".to_owned());
            args.push("json".to_owned());
        }
        let prompt = match launch {
            Launch::New { prompt } => {
                args.push("--session-id".to_owned());
                args.push(uuid::Uuid::new_v4().to_string());
                Some(prompt.clone())
            }
            Launch::Resume { session, prompt } => {
                args.push("--resume".to_owned());
                args.push(session.as_str().to_owned());
                prompt.clone()
            }
        };
        args.extend(self.flags.iter().cloned());
        match prompt {
            Some(prompt) => {
                args.push("--".to_owned());
                args.push(prompt);
            }
            None if headless => {
                return Err(HarnessError::HeadlessNeedsPrompt {
                    program: PROGRAM.to_owned(),
                });
            }
            None => {}
        }
        Ok(args)
    }

    fn launch_impl(
        &self,
        workspace: &Path,
        launch: &Launch,
        io: Io,
        env: &[(EnvVarName, String)],
    ) -> Result<Running, HarnessError> {
        let root = harness::physical_workspace(workspace)?;
        let binary = self.binary()?;
        let args = self.args_for(launch, io)?;
        let session = match launch {
            Launch::New { .. } => args
                .iter()
                .position(|a| a == "--session-id")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| SessionId::parse(s).ok()),
            Launch::Resume { session, .. } => Some(session.clone()),
        };
        let child = process::spawn(&binary, &args, &root, env, io)?;
        info!(?session, pid = child.id(), "claude: launched");
        Ok(Running::new(child, session))
    }
}

impl Harness for ClaudeCode {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
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

    fn restore<'a>(
        &'a self,
        workspace: &'a Path,
        state: &'a Path,
    ) -> BoxFuture<'a, Result<Restored, HarnessError>> {
        Box::pin(async move { self.restore_impl(workspace, state) })
    }

    fn launch<'a>(
        &'a self,
        workspace: &'a Path,
        launch: Launch,
        io: Io,
        env: &'a [(EnvVarName, String)],
    ) -> BoxFuture<'a, Result<Running, HarnessError>> {
        Box::pin(async move { self.launch_impl(workspace, &launch, io, env) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_follow_the_cli_surface() {
        let adapter = ClaudeCode::new(PathBuf::from("/h")).with_flags(vec![
            "--permission-mode".to_owned(),
            "acceptEdits".to_owned(),
        ]);
        let session = SessionId::parse("9c9796f0-2682-4546-a963-d314b3ad543f").unwrap();
        let resume = Launch::Resume {
            session: session.clone(),
            prompt: None,
        };
        assert_eq!(
            adapter.args_for(&resume, Io::Terminal).unwrap(),
            vec![
                "--resume",
                "9c9796f0-2682-4546-a963-d314b3ad543f",
                "--permission-mode",
                "acceptEdits"
            ]
        );
        assert!(matches!(
            adapter.args_for(&resume, Io::Piped),
            Err(HarnessError::HeadlessNeedsPrompt { .. })
        ));
        let with_prompt = Launch::Resume {
            session,
            prompt: Some("what is your cwd?".to_owned()),
        };
        assert_eq!(
            adapter.args_for(&with_prompt, Io::Piped).unwrap(),
            vec![
                "-p",
                "--output-format",
                "json",
                "--resume",
                "9c9796f0-2682-4546-a963-d314b3ad543f",
                "--permission-mode",
                "acceptEdits",
                "--",
                "what is your cwd?"
            ]
        );
        let new = adapter
            .args_for(
                &Launch::New {
                    prompt: "hi".to_owned(),
                },
                Io::Terminal,
            )
            .unwrap();
        assert_eq!(new[0], "--session-id");
        assert_eq!(new[1].len(), 36);
        assert_eq!(&new[2..], ["--permission-mode", "acceptEdits", "--", "hi"]);
    }
}
