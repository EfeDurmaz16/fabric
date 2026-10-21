//! The `-c` overrides a Codex launch passes.
//!
//! Invariant: only the settings ADR 0006 names (sandbox, approval policy,
//! model, reasoning effort) can be expressed, each from a closed set or a
//! parsed name, so neither a job record nor a session record can set
//! another project's trust or any other key. `danger-full-access` is left
//! out on purpose: adding it is a decision, not a missing case. Values
//! checked against `codex --help` of 0.155.1 on 2026-10-20, and the
//! spellings each enum reads against the `turn_context` records of the
//! owner's rollouts on 2026-10-21 (values only).
//!
//! A resume takes these settings from the session (`rollout::recorded`),
//! and any field the adapter was configured with wins over the session's
//! (`effective`). A launch with no session passes only what was configured.

use crate::harness::{LaunchSettings, ModelName, NotCarried, Origin, PassedSetting, SettingName};

use super::rollout::{Recorded, RecordedSettings};

/// The `sandbox_mode` a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// `read-only`.
    ReadOnly,
    /// `workspace-write`.
    WorkspaceWrite,
}

impl SandboxMode {
    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
        }
    }

    /// The mode a record spells; `None` for any other, `danger-full-access` included.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite]
            .into_iter()
            .find(|mode| mode.as_str() == text)
    }
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

impl ApprovalPolicy {
    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalPolicy::Untrusted => "untrusted",
            ApprovalPolicy::OnRequest => "on-request",
            ApprovalPolicy::Never => "never",
        }
    }

    /// The policy a record spells; `None` for any other.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        [
            ApprovalPolicy::Untrusted,
            ApprovalPolicy::OnRequest,
            ApprovalPolicy::Never,
        ]
        .into_iter()
        .find(|policy| policy.as_str() == text)
    }
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

impl Effort {
    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
        }
    }

    /// The effort a record spells; `None` for any other.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        [Effort::Low, Effort::Medium, Effort::High, Effort::XHigh]
            .into_iter()
            .find(|effort| effort.as_str() == text)
    }
}

/// Settings passed as `-c` overrides; a cold resume restores none of them
/// (H2, experiment E3; gate finding F10a). A field left `None` is taken
/// from the session on a resume and not passed on a new thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexOverrides {
    /// The sandbox.
    pub sandbox: Option<SandboxMode>,
    /// When to ask before running a command.
    pub approval: Option<ApprovalPolicy>,
    /// The model.
    pub model: Option<ModelName>,
    /// The reasoning effort.
    pub effort: Option<Effort>,
}

impl CodexOverrides {
    /// The `key="value"` pairs, each passed after a `-c`, in a fixed order.
    pub(crate) fn pairs(&self) -> Vec<String> {
        let mut pairs = Vec::new();
        if let Some(model) = &self.model {
            pairs.push(format!("model=\"{}\"", model.as_str()));
        }
        if let Some(effort) = self.effort {
            pairs.push(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
        }
        if let Some(sandbox) = self.sandbox {
            pairs.push(format!("sandbox_mode=\"{}\"", sandbox.as_str()));
        }
        if let Some(approval) = self.approval {
            pairs.push(format!("approval_policy=\"{}\"", approval.as_str()));
        }
        pairs
    }
}

/// One field of `effective`: the pin when there is one, else the
/// session's value when Fabric carries it, else nothing, with the
/// report's entry for each case.
fn pick<T: Clone>(
    name: SettingName,
    pinned: Option<&T>,
    recorded: Option<&Recorded<T>>,
    spell: impl Fn(&T) -> String,
    report: &mut LaunchSettings,
) -> Option<T> {
    let (value, origin) = match (pinned, recorded) {
        (Some(value), _) => (value.clone(), Origin::Pinned),
        (None, Some(Recorded::Carried(value))) => (value.clone(), Origin::Session),
        (None, Some(Recorded::NotCarried { value, reason })) => {
            report
                .not_carried
                .push(NotCarried::new(name, value, reason.clone()));
            return None;
        }
        (None, Some(Recorded::Absent) | None) => return None,
    };
    report.passed.push(PassedSetting {
        name,
        value: spell(&value),
        origin,
    });
    Some(value)
}

/// The overrides a launch passes, and what to say about them: each field
/// the adapter was configured with, else the session's own when the
/// launch resumes one and Fabric carries its value.
pub(crate) fn effective(
    pinned: Option<&CodexOverrides>,
    recorded: Option<&RecordedSettings>,
) -> (CodexOverrides, LaunchSettings) {
    let mut report = LaunchSettings::default();
    let model = pick(
        SettingName::Model,
        pinned.and_then(|p| p.model.as_ref()),
        recorded.map(|r| &r.model),
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let effort = pick(
        SettingName::Effort,
        pinned.and_then(|p| p.effort.as_ref()),
        recorded.map(|r| &r.effort),
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let sandbox = pick(
        SettingName::Sandbox,
        pinned.and_then(|p| p.sandbox.as_ref()),
        recorded.map(|r| &r.sandbox),
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let approval = pick(
        SettingName::Approval,
        pinned.and_then(|p| p.approval.as_ref()),
        recorded.map(|r| &r.approval),
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let overrides = CodexOverrides {
        sandbox,
        approval,
        model,
        effort,
    };
    (overrides, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> RecordedSettings {
        RecordedSettings {
            model: Recorded::Carried(ModelName::parse("gpt-6-sol").unwrap()),
            effort: Recorded::NotCarried {
                value: "ultra".to_owned(),
                reason: "not an effort Fabric passes".to_owned(),
            },
            approval: Recorded::Carried(ApprovalPolicy::OnRequest),
            sandbox: Recorded::Carried(SandboxMode::WorkspaceWrite),
        }
    }

    /// A pin wins over the session field by field; a field with no pin
    /// takes the session's carried value; a value Fabric does not carry
    /// is reported, not passed.
    #[test]
    fn a_pin_wins_field_by_field_and_the_rest_comes_from_the_session() {
        let pinned = CodexOverrides {
            sandbox: Some(SandboxMode::ReadOnly),
            ..CodexOverrides::default()
        };
        let (overrides, report) = effective(Some(&pinned), Some(&recorded()));
        assert_eq!(
            overrides,
            CodexOverrides {
                sandbox: Some(SandboxMode::ReadOnly),
                approval: Some(ApprovalPolicy::OnRequest),
                model: Some(ModelName::parse("gpt-6-sol").unwrap()),
                effort: None,
            }
        );
        let passed: Vec<(SettingName, &str, Origin)> = report
            .passed
            .iter()
            .map(|p| (p.name, p.value.as_str(), p.origin))
            .collect();
        assert_eq!(
            passed,
            [
                (SettingName::Model, "gpt-6-sol", Origin::Session),
                (SettingName::Sandbox, "read-only", Origin::Pinned),
                (SettingName::Approval, "on-request", Origin::Session),
            ]
        );
        assert_eq!(
            report.not_carried,
            [NotCarried::new(
                SettingName::Effort,
                "ultra",
                "not an effort Fabric passes"
            )]
        );
        let (none, report) = effective(None, None);
        assert_eq!(none, CodexOverrides::default());
        assert_eq!(report, LaunchSettings::default());
    }
}
