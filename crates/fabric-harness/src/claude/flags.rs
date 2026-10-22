//! The flags a Claude launch passes on every start.
//!
//! Invariant: only the flags ADR 0006 lists can be expressed, each with a
//! parsed value, so a job record cannot ask for `--bg` (whose supervisor
//! restarts a fenced session), a second `--resume`, a bare `--` that would
//! turn the flags after it into the prompt, or a mode that skips every
//! permission check. `bypassPermissions` is left out on purpose: adding it
//! is a decision, not a missing case. Surfaces checked against
//! `claude --help` of 2.1.294 on 2026-10-20.

use fabric_core::path::AbsPath;

use crate::harness::ModelName;

/// A `--permission-mode` this adapter passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// `acceptEdits`: file edits are accepted without a prompt.
    AcceptEdits,
    /// `auto`: Claude decides which actions need a prompt.
    Auto,
    /// `manual`: every action that needs permission prompts.
    Manual,
    /// `dontAsk`: actions that would prompt are denied instead.
    DontAsk,
    /// `plan`: Claude plans and does not edit.
    Plan,
}

impl PermissionMode {
    fn as_flag(self) -> &'static str {
        match self {
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Auto => "auto",
            PermissionMode::Manual => "manual",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Plan => "plan",
        }
    }
}

/// The flags passed on every launch, because resume restores none of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchFlags {
    /// `--settings`: the job's settings overlay file.
    pub settings: Option<AbsPath>,
    /// `--mcp-config`: the job's MCP configuration file.
    pub mcp_config: Option<AbsPath>,
    /// `--add-dir`, once per directory.
    pub add_dirs: Vec<AbsPath>,
    /// `--plugin-dir`, once per directory.
    pub plugin_dirs: Vec<AbsPath>,
    /// `--permission-mode`.
    pub permission_mode: Option<PermissionMode>,
    /// `--fallback-model`.
    pub fallback_model: Option<ModelName>,
}

impl LaunchFlags {
    /// The overlay files Claude reads at start, which must hold no literal secret.
    pub(crate) fn overlays(&self) -> impl Iterator<Item = &AbsPath> {
        self.settings.iter().chain(&self.mcp_config)
    }

    /// The argument list, in a fixed order.
    pub(crate) fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let mut push = |flag: &str, value: &str| {
            args.push(flag.to_owned());
            args.push(value.to_owned());
        };
        if let Some(path) = &self.settings {
            push("--settings", path.as_str());
        }
        if let Some(path) = &self.mcp_config {
            push("--mcp-config", path.as_str());
        }
        for dir in &self.add_dirs {
            push("--add-dir", dir.as_str());
        }
        for dir in &self.plugin_dirs {
            push("--plugin-dir", dir.as_str());
        }
        if let Some(mode) = self.permission_mode {
            push("--permission-mode", mode.as_flag());
        }
        if let Some(model) = &self.fallback_model {
            push("--fallback-model", model.as_str());
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_flag_renders_as_flag_and_value() {
        let path = |p: &str| AbsPath::parse(p).unwrap();
        let flags = LaunchFlags {
            settings: Some(path("/j/settings.json")),
            mcp_config: Some(path("/j/mcp.json")),
            add_dirs: vec![path("/a"), path("/b")],
            plugin_dirs: vec![path("/p")],
            permission_mode: Some(PermissionMode::DontAsk),
            fallback_model: Some(ModelName::parse("sonnet").unwrap()),
        };
        assert_eq!(
            flags.args(),
            vec![
                "--settings",
                "/j/settings.json",
                "--mcp-config",
                "/j/mcp.json",
                "--add-dir",
                "/a",
                "--add-dir",
                "/b",
                "--plugin-dir",
                "/p",
                "--permission-mode",
                "dontAsk",
                "--fallback-model",
                "sonnet"
            ]
        );
        assert!(LaunchFlags::default().args().is_empty());
    }
}
