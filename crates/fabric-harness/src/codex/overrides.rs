//! The `-c` overrides a Codex launch passes on every run.
//!
//! Invariant: only the settings ADR 0006 names (sandbox, approval policy,
//! model, reasoning effort) can be expressed, each from a closed set or a
//! parsed name, so a job record cannot set another project's trust or any
//! other key. `danger-full-access` is left out on purpose: adding it is a
//! decision, not a missing case. Values checked against `codex --help` of
//! 0.155.1 on 2026-10-20.

use crate::harness::ModelName;

/// The `sandbox_mode` a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// `read-only`.
    ReadOnly,
    /// `workspace-write`.
    WorkspaceWrite,
}

/// The `approval_policy` a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// `untrusted`: only trusted commands run without asking.
    Untrusted,
    /// `on-request`: the model decides when to ask.
    OnRequest,
    /// `never`: never ask; a failure goes back to the model.
    Never,
}

/// The `model_reasoning_effort` a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    XHigh,
}

/// The settings a cold resume does not restore (H2, experiment E3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOverrides {
    /// The sandbox.
    pub sandbox: SandboxMode,
    /// When to ask before running a command.
    pub approval: ApprovalPolicy,
    /// The model, when the job pins one.
    pub model: Option<ModelName>,
    /// The reasoning effort, when the job pins one.
    pub effort: Option<Effort>,
}

impl CodexOverrides {
    /// The `key="value"` pairs, each passed after a `-c`.
    pub(crate) fn pairs(&self) -> Vec<String> {
        let sandbox = match self.sandbox {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
        };
        let approval = match self.approval {
            ApprovalPolicy::Untrusted => "untrusted",
            ApprovalPolicy::OnRequest => "on-request",
            ApprovalPolicy::Never => "never",
        };
        let mut pairs = vec![
            format!("sandbox_mode=\"{sandbox}\""),
            format!("approval_policy=\"{approval}\""),
        ];
        if let Some(model) = &self.model {
            pairs.push(format!("model=\"{}\"", model.as_str()));
        }
        if let Some(effort) = self.effort {
            let effort = match effort {
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High => "high",
                Effort::XHigh => "xhigh",
            };
            pairs.push(format!("model_reasoning_effort=\"{effort}\""));
        }
        pairs
    }
}
