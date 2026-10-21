//! The `-c` overrides a Codex launch passes.
//!
//! Invariant: only the settings ADR 0006 names (sandbox, approval
//! policy and approvals reviewer, model, reasoning effort) can be
//! expressed, each from a closed set or a parsed name, so neither a job
//! record nor a session record can set another project's trust or any
//! other key. `danger-full-access` is left out on purpose: adding it is
//! a decision, not a missing case. Values checked against `codex
//! --help` of 0.155.1 on 2026-10-20, and the spellings each enum reads
//! against the `turn_context` records of the owner's rollouts on
//! 2026-10-21 (values only). The approval policy `untrusted` is not in
//! the set: Codex 0.155.1 refuses to load any configuration that sets
//! it (`approval_policy = "untrusted" is no longer supported; remove
//! this setting`, seen on 2026-10-21 with `codex mcp list -c
//! approval_policy="untrusted"`), yet a `turn_context` can still record
//! it for a project Codex marks untrusted.
//!
//! The approval policy says when Codex asks, and `approvals_reviewer`
//! says who answers: the user, or `auto_review`, a reviewer model
//! (`ApprovalsReviewer` in `codex-rs/protocol/src/config_types.rs` at
//! `rust-v0.155.1`, which reads the older `guardian_subagent` as
//! `auto_review` and writes `auto_review`). A terminal `codex resume`
//! sends the node's reviewer with the thread
//! (`thread_resume_params_from_config` in
//! `codex-rs/tui/src/app_server_session.rs` at the same tag), so a
//! policy carried alone would let the node's reviewer answer what the
//! session's user answered, or the other way round; the reviewer
//! travels with the policy. `codex exec resume` sends none unless `-c`
//! sets it, and Codex then restores the thread's own from the rollout
//! (`latest_persisted_resume_settings`, `codex-rs/app-server` at the
//! same tag), so a headless launch neither passes nor reports it.
//!
//! A resume takes these settings from the session (`rollout::recorded`),
//! and any field the adapter was configured with wins over the session's
//! (`effective`). A launch with no session passes only what was configured.
//! A headless launch passes no approval policy and reports the one it
//! would have passed as left out: `codex exec` sets its own, `never`,
//! as a typed override that wins over `-c`, and drops it only when the
//! node sends approvals to auto review, which then uses the node's own
//! (`codex-rs/exec/src/lib.rs` at `rust-v0.155.1`, `ConfigOverrides` and
//! `build_exec_config`). A turn `codex exec` ran therefore records the
//! `never` it forced (all 77 exec turns in the owner's rollouts whose
//! approvals go to the user, counted on 2026-10-21), and nothing in a
//! `turn_context` tells an exec turn from a terminal one, so a terminal
//! launch does not carry a recorded `never` either: it reports it as left
//! out, and Codex uses the node's own policy.

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
            ApprovalPolicy::OnRequest => "on-request",
            ApprovalPolicy::Never => "never",
        }
    }

    /// The policy a record spells; `None` for any other.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        [ApprovalPolicy::OnRequest, ApprovalPolicy::Never]
            .into_iter()
            .find(|policy| policy.as_str() == text)
    }
}

/// The `approvals_reviewer` a run uses: who answers an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalsReviewer {
    /// `user`: the user answers.
    User,
    /// `auto_review`: a reviewer model answers.
    AutoReview,
}

impl ApprovalsReviewer {
    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalsReviewer::User => "user",
            ApprovalsReviewer::AutoReview => "auto_review",
        }
    }

    /// The reviewer a record spells, the older `guardian_subagent` read
    /// as `auto_review` as Codex reads it; `None` for any other.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        if text == "guardian_subagent" {
            return Some(ApprovalsReviewer::AutoReview);
        }
        [ApprovalsReviewer::User, ApprovalsReviewer::AutoReview]
            .into_iter()
            .find(|reviewer| reviewer.as_str() == text)
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
    /// Who answers what Codex asks.
    pub reviewer: Option<ApprovalsReviewer>,
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
        if let Some(reviewer) = self.reviewer {
            pairs.push(format!("approvals_reviewer=\"{}\"", reviewer.as_str()));
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

/// Why a headless launch passes no approval policy.
const EXEC_SETS_APPROVAL: &str = "codex exec sets it: never, or this node's own under auto review";

/// Why a terminal launch does not carry a recorded `never`.
const EXEC_RECORDS_NEVER: &str =
    "codex exec records never for the turns it runs, so it may not be the session's own";

/// The approval a terminal launch may carry from the session: any but a
/// recorded `never`, which may be the one `codex exec` forced.
fn terminal_approval(recorded: &Recorded<ApprovalPolicy>) -> Recorded<ApprovalPolicy> {
    match recorded {
        Recorded::Carried(ApprovalPolicy::Never) => Recorded::NotCarried {
            value: ApprovalPolicy::Never.as_str().to_owned(),
            reason: EXEC_RECORDS_NEVER.to_owned(),
        },
        other => other.clone(),
    }
}

/// One field a launch cannot pass: the value it would have passed, the
/// pin or else the session's, is reported as left out for `reason`; a
/// value Fabric does not carry keeps its own reason.
fn leave_out<T>(
    name: SettingName,
    pinned: Option<&T>,
    recorded: Option<&Recorded<T>>,
    spell: impl Fn(&T) -> String,
    reason: &str,
    report: &mut LaunchSettings,
) {
    let left = match (pinned, recorded) {
        (Some(value), _) | (None, Some(Recorded::Carried(value))) => {
            NotCarried::new(name, &spell(value), reason)
        }
        (None, Some(Recorded::NotCarried { value, reason })) => {
            NotCarried::new(name, value, reason.clone())
        }
        (None, Some(Recorded::Absent) | None) => return,
    };
    report.not_carried.push(left);
}

/// The overrides a launch passes, and what to say about them: each field
/// the adapter was configured with, else the session's own when the
/// launch resumes one and Fabric carries its value. A `headless` launch
/// passes no approval policy, because `codex exec` sets its own, and a
/// terminal launch does not carry a recorded `never`, because `codex
/// exec` records the one it forced.
pub(crate) fn effective(
    pinned: Option<&CodexOverrides>,
    recorded: Option<&RecordedSettings>,
    headless: bool,
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
    let pinned_approval = pinned.and_then(|p| p.approval.as_ref());
    let recorded_approval = recorded.map(|r| &r.approval);
    let spell_approval = |v: &ApprovalPolicy| v.as_str().to_owned();
    let approval = if headless {
        leave_out(
            SettingName::Approval,
            pinned_approval,
            recorded_approval,
            spell_approval,
            EXEC_SETS_APPROVAL,
            &mut report,
        );
        None
    } else {
        let recorded_approval = recorded_approval.map(terminal_approval);
        pick(
            SettingName::Approval,
            pinned_approval,
            recorded_approval.as_ref(),
            spell_approval,
            &mut report,
        )
    };
    // Under `codex exec resume`, Codex restores the thread's own reviewer
    // when none is passed, so only a pin is.
    let reviewer = pick(
        SettingName::Reviewer,
        pinned.and_then(|p| p.reviewer.as_ref()),
        if headless {
            None
        } else {
            recorded.map(|r| &r.reviewer)
        },
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let overrides = CodexOverrides {
        sandbox,
        approval,
        reviewer,
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
            reviewer: Recorded::Carried(ApprovalsReviewer::User),
            sandbox: Recorded::Carried(SandboxMode::WorkspaceWrite),
            writer: None,
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
        let (overrides, report) = effective(Some(&pinned), Some(&recorded()), false);
        assert_eq!(
            overrides,
            CodexOverrides {
                sandbox: Some(SandboxMode::ReadOnly),
                approval: Some(ApprovalPolicy::OnRequest),
                reviewer: Some(ApprovalsReviewer::User),
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
                (SettingName::Reviewer, "user", Origin::Session),
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
        let (none, report) = effective(None, None, false);
        assert_eq!(none, CodexOverrides::default());
        assert_eq!(report, LaunchSettings::default());
    }

    /// `codex exec` sets its own approval policy over any `-c`, so a
    /// headless launch passes none and says which one it left out, the
    /// pin's or else the session's. It passes no reviewer either and says
    /// nothing of it: `codex exec resume` restores the thread's own.
    #[test]
    fn a_headless_launch_leaves_the_approval_policy_to_codex_exec() {
        let (overrides, report) = effective(None, Some(&recorded()), true);
        assert_eq!((overrides.approval, overrides.reviewer), (None, None));
        assert!(
            report
                .passed
                .iter()
                .all(|p| !matches!(p.name, SettingName::Approval | SettingName::Reviewer))
        );
        assert!(
            report
                .not_carried
                .iter()
                .all(|n| n.name != SettingName::Reviewer)
        );
        assert_eq!(
            report.not_carried.last(),
            Some(&NotCarried::new(
                SettingName::Approval,
                "on-request",
                EXEC_SETS_APPROVAL
            ))
        );
        let pinned = CodexOverrides {
            approval: Some(ApprovalPolicy::Never),
            ..CodexOverrides::default()
        };
        let (overrides, report) = effective(Some(&pinned), None, true);
        assert_eq!(overrides, CodexOverrides::default());
        assert_eq!(
            report.not_carried,
            [NotCarried::new(
                SettingName::Approval,
                "never",
                EXEC_SETS_APPROVAL
            )]
        );
    }

    /// A turn `codex exec` ran records the `never` it forced, and nothing
    /// in the record says which turns those were, so a terminal resume
    /// after a headless turn does not carry `never`: it says so, and Codex
    /// asks as the node's own policy says. A pinned `never` still passes.
    #[test]
    fn a_terminal_launch_does_not_carry_a_recorded_never() {
        let after_exec = RecordedSettings {
            approval: Recorded::Carried(ApprovalPolicy::Never),
            ..recorded()
        };
        let (overrides, report) = effective(None, Some(&after_exec), false);
        assert_eq!(overrides.approval, None);
        assert!(
            report
                .passed
                .iter()
                .all(|p| p.name != SettingName::Approval)
        );
        assert_eq!(
            report.not_carried.last(),
            Some(&NotCarried::new(
                SettingName::Approval,
                "never",
                EXEC_RECORDS_NEVER
            ))
        );
        let pinned = CodexOverrides {
            approval: Some(ApprovalPolicy::Never),
            ..CodexOverrides::default()
        };
        let (overrides, _) = effective(Some(&pinned), Some(&after_exec), false);
        assert_eq!(overrides.approval, Some(ApprovalPolicy::Never));
    }
}
