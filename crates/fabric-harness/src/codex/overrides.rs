//! The `-c` overrides a Codex launch passes.
//!
//! Invariant: only the settings ADR 0006 names (sandbox, with what a
//! session's `workspace-write` sandbox allowed beyond the workspace,
//! approval policy and approvals reviewer, model, reasoning effort, and
//! the model provider the session's model ran on) can be expressed, each
//! from a closed set or a parsed name, so neither a job record nor a
//! session record can set another project's trust or any other key. A provider
//! id only names an entry of `model_providers` this node already
//! defines, or a built-in one; it cannot define one. `danger-full-access` is left out on purpose: adding it is
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
//! travels with the policy, and only with it: when a terminal launch
//! does not pass the session's policy, because it is a recorded `never`,
//! one Fabric does not carry, or a pin replaced it, the session's
//! reviewer is reported as left out too, so the node's policy keeps the
//! node's reviewer. A pinned reviewer still passes. `codex exec resume` sends none unless `-c`
//! sets it, and Codex then restores the thread's own from the rollout
//! (`latest_persisted_resume_settings`, `codex-rs/app-server` at the
//! same tag), so a headless launch neither passes nor reports the
//! recorded one; it passes only a pinned one.
//!
//! The effort set is every effort Codex 0.155.1 names, `max` and
//! `ultra` included, not the four `codex --help` lists: with `-c model`
//! set, a terminal `codex resume` 0.155.1 sends this node's effort, and
//! the app server then skips its own restore of the thread's model and
//! effort (`resume_model_settings_for_overrides` in
//! `codex-rs/tui/src/app/config_persistence.rs` and
//! `merge_persisted_resume_metadata` in `codex-rs/app-server`, at
//! `rust-v0.155.1`). So a recorded effort Fabric left out while it
//! passed the model was not kept but replaced by the node's. A record
//! that names no effort still resumes on the node's effort when the
//! model is passed, where Codex alone would clear it to the model's
//! default; nothing in `-c` can say "no effort".
//!
//! A model is served by a provider, and the session records the pair:
//! `session_meta.model_provider` when a rollout is created, and
//! `thread_settings_applied.thread_settings.model_provider_id`, which
//! Codex 0.155.1 writes on every resume and settings change
//! (`checkpoint_thread_settings` and `applied_event` in
//! `codex-rs/core/src/session` at `rust-v0.155.1`). Codex does not pair
//! a `-c model` with the provider the thread ran on: `codex exec resume`
//! always sends this node's `model_provider_id` with the model
//! (`thread_resume_params_from_config` in `codex-rs/exec/src/lib.rs`),
//! and a terminal `codex resume` with `-c model` set does too
//! (`resume_model_settings_for_overrides` in `codex-rs/tui`), so the
//! app server's own restore of model and provider together
//! (`merge_persisted_resume_metadata`) is skipped. A model carried alone
//! would be sent to this node's default provider, which may not serve
//! it. So the session's provider is passed with the session's model, and
//! only with it: when the model is pinned or not carried, the recorded
//! provider is reported as left out, and the node's provider goes with
//! the node's model. A provider this node does not define makes Codex
//! refuse to start (`Model provider ... not found`,
//! `codex-rs/core/src/config/mod.rs`), which says plainly what a model
//! sent to the wrong provider would not. A provider cannot be pinned.
//!
//! A sandbox is more than its mode (M2-21). A `workspace-write` turn
//! records what it allowed beyond the workspace in its `sandbox_policy`:
//! `writable_roots`, `network_access`, `exclude_tmpdir_env_var` and
//! `exclude_slash_tmp`, the fields of `[sandbox_workspace_write]`, which
//! Codex 0.155.1 reads only with `sandbox_mode = "workspace-write"`
//! (`derive_permission_profile` in `codex-rs/config/src/config_toml.rs`
//! at `rust-v0.155.1`). They travel with the session's own mode and only
//! with it, as `-c sandbox_workspace_write.*` right after it, so this
//! node's own table neither widens nor narrows the session's sandbox; a
//! record that omits the list of roots, as 0.155.1 does when it is empty,
//! passes an empty one. Each root is an absolute path with no control
//! character, passed as a quoted TOML string. A pinned mode takes this
//! node's table, and the session's are reported as left out. A read-only
//! sandbox's network access is reported, never passed: Codex takes none
//! for read-only from its configuration.
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

use crate::harness::{
    LaunchSettings, ModelName, ModelNameError, NotCarried, Origin, PassedSetting, SettingName,
};

use super::rollout::{Recorded, RecordedSettings};

/// The `model_provider` a run uses: the id of a `model_providers` entry
/// or a built-in one, such as `openai`, held to the rules of a
/// `ModelName` so it is safe on a command line and inside a quoted TOML
/// string. It only names a provider; it cannot define one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderId(ModelName);

impl ProviderId {
    /// Checks `text` against the rules of a model name and wraps it.
    pub(crate) fn parse(text: &str) -> Result<Self, ModelNameError> {
        ModelName::parse(text).map(ProviderId)
    }

    /// The id as given.
    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// The `sandbox_mode` a run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxMode {
    /// `read-only`.
    ReadOnly,
    /// `workspace-write`, with what this node's own
    /// `[sandbox_workspace_write]` allows beyond the workspace.
    WorkspaceWrite,
    /// `workspace-write`, with what a session's own sandbox allowed beyond
    /// the workspace, passed in place of this node's (M2-21).
    WorkspaceWriteWith(SandboxAccess),
}

impl SandboxMode {
    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite | SandboxMode::WorkspaceWriteWith(_) => "workspace-write",
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

/// A folder a `workspace-write` sandbox may write beyond the workspace: an
/// absolute path with no control character, which travels inside a quoted
/// TOML string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot(String);

impl WritableRoot {
    /// Checks `text` and wraps it; says why when it is no such path.
    pub fn parse(text: &str) -> Result<Self, String> {
        if !text.starts_with('/') {
            return Err("not an absolute path".to_owned());
        }
        if text.chars().any(char::is_control) {
            return Err("holds a control character".to_owned());
        }
        Ok(WritableRoot(text.to_owned()))
    }

    /// The path as given.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a `workspace-write` sandbox may write beyond the workspace, as
/// Codex 0.155.1 reads it from `[sandbox_workspace_write]`
/// (`SandboxWorkspaceWrite` in `codex-rs/config/src/types.rs` at
/// `rust-v0.155.1`) and records it in a `turn_context`'s
/// `sandbox_policy`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WritableRoots {
    /// `writable_roots`: folders beyond the workspace. A record omits the
    /// list when it is empty.
    pub roots: Vec<WritableRoot>,
    /// `exclude_tmpdir_env_var`: `$TMPDIR` is left out; `None` when not
    /// recorded, so this node's own applies.
    pub exclude_tmpdir_env_var: Option<bool>,
    /// `exclude_slash_tmp`: `/tmp` is left out; `None` when not recorded.
    pub exclude_slash_tmp: Option<bool>,
}

impl WritableRoots {
    /// How the CLI says it: `[/a, /b]` or `none`, and what it leaves out.
    #[must_use]
    pub fn spelled(&self) -> String {
        let mut text = if self.roots.is_empty() {
            "none".to_owned()
        } else {
            let roots: Vec<&str> = self.roots.iter().map(WritableRoot::as_str).collect();
            format!("[{}]", roots.join(", "))
        };
        match (
            self.exclude_slash_tmp == Some(true),
            self.exclude_tmpdir_env_var == Some(true),
        ) {
            (true, true) => text.push_str(" without /tmp or TMPDIR"),
            (true, false) => text.push_str(" without /tmp"),
            (false, true) => text.push_str(" without TMPDIR"),
            (false, false) => {}
        }
        text
    }
}

/// What a session's `workspace-write` sandbox allowed beyond the
/// workspace: its writable roots and its network access, each `None`
/// when the record does not hold it, so this node's own applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxAccess {
    /// The writable roots and the temporary folders left out.
    pub writable: Option<WritableRoots>,
    /// `network_access`: commands may reach the network.
    pub network_access: Option<bool>,
}

impl SandboxAccess {
    /// The `sandbox_workspace_write.*` pairs, each passed after a `-c`.
    fn pairs(&self) -> Vec<String> {
        const TABLE: &str = "sandbox_workspace_write";
        let mut pairs = Vec::new();
        if let Some(writable) = &self.writable {
            let roots: Vec<String> = writable
                .roots
                .iter()
                .map(|root| toml_string(root.as_str()))
                .collect();
            pairs.push(format!("{TABLE}.writable_roots=[{}]", roots.join(",")));
        }
        if let Some(network) = self.network_access {
            pairs.push(format!("{TABLE}.network_access={network}"));
        }
        if let Some(writable) = &self.writable {
            if let Some(exclude) = writable.exclude_tmpdir_env_var {
                pairs.push(format!("{TABLE}.exclude_tmpdir_env_var={exclude}"));
            }
            if let Some(exclude) = writable.exclude_slash_tmp {
                pairs.push(format!("{TABLE}.exclude_slash_tmp={exclude}"));
            }
        }
        pairs
    }
}

/// `text` as a TOML basic string. A `WritableRoot` holds no control
/// character, so only the quote and the backslash need escaping.
fn toml_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// How a network access is said: `on` or `off`.
fn network_spelled(on: bool) -> &'static str {
    if on { "on" } else { "off" }
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

/// The `model_reasoning_effort` a run uses: every effort Codex 0.155.1
/// names (`ReasoningEffort` in `codex-rs/protocol/src/openai_models.rs`
/// at `rust-v0.155.1`). Codex also takes any other text as an effort a
/// model defines; Fabric does not pass one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// `none`.
    None,
    /// `minimal`.
    Minimal,
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    XHigh,
    /// `max`.
    Max,
    /// `ultra`.
    Ultra,
    /// `persistent`.
    Persistent,
}

impl Effort {
    /// Every effort, in Codex's order.
    const ALL: [Effort; 9] = [
        Effort::None,
        Effort::Minimal,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
        Effort::Ultra,
        Effort::Persistent,
    ];

    /// The spelling Codex uses, in `-c` and in its records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::None => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
            Effort::Ultra => "ultra",
            Effort::Persistent => "persistent",
        }
    }

    /// The effort a record spells; `None` for any other.
    #[must_use]
    pub fn from_record(text: &str) -> Option<Self> {
        Effort::ALL
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
        if let Some(sandbox) = &self.sandbox {
            pairs.push(format!("sandbox_mode=\"{}\"", sandbox.as_str()));
            if let SandboxMode::WorkspaceWriteWith(access) = sandbox {
                pairs.extend(access.pairs());
            }
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

/// What one launch passes as `-c` overrides: the settings, each the pin
/// or else the session's, and the provider the session's model ran on,
/// which travels with that model and only with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaunchOverrides {
    /// The settings.
    pub(crate) settings: CodexOverrides,
    /// The session's model provider, set only when the session's model is passed.
    pub(crate) provider: Option<ProviderId>,
}

impl LaunchOverrides {
    /// The `key="value"` pairs, each passed after a `-c`, in a fixed
    /// order: the provider right after the model it goes with.
    pub(crate) fn pairs(&self) -> Vec<String> {
        let mut pairs = self.settings.pairs();
        if let Some(provider) = &self.provider {
            let after_model = usize::from(self.settings.model.is_some());
            pairs.insert(
                after_model,
                format!("model_provider=\"{}\"", provider.as_str()),
            );
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

/// Why a launch that does not pass the session's model does not pass the
/// provider it ran on either.
const PROVIDER_GOES_WITH_MODEL: &str =
    "it goes with the session's model, which this launch does not pass";

/// Why a terminal launch that does not pass the session's approval policy
/// does not pass the session's reviewer either.
const REVIEWER_GOES_WITH_APPROVAL: &str =
    "it goes with the session's approval policy, which this launch does not pass";

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

/// The model a launch passes, the pin or else the session's, and the
/// provider that goes with it: the one the session's model ran on, only
/// when the session's model is passed. Otherwise a recorded provider is
/// reported as left out, and the node's provider goes with the model.
fn model_and_provider(
    pinned: Option<&ModelName>,
    recorded: Option<&RecordedSettings>,
    report: &mut LaunchSettings,
) -> (Option<ModelName>, Option<ProviderId>) {
    let recorded_model = recorded.map(|r| &r.model);
    let session_model = pinned.is_none() && matches!(recorded_model, Some(Recorded::Carried(_)));
    let model = pick(
        SettingName::Model,
        pinned,
        recorded_model,
        |v| v.as_str().to_owned(),
        report,
    );
    let recorded_provider = recorded.map(|r| &r.provider);
    let spell = |v: &ProviderId| v.as_str().to_owned();
    if session_model {
        return (
            model,
            pick(
                SettingName::Provider,
                None,
                recorded_provider,
                spell,
                report,
            ),
        );
    }
    leave_out(
        SettingName::Provider,
        None,
        recorded_provider,
        spell,
        PROVIDER_GOES_WITH_MODEL,
        report,
    );
    (model, None)
}

/// Why a launch that does not pass the session's sandbox does not pass
/// what it allowed beyond the workspace either.
const ACCESS_GOES_WITH_SANDBOX: &str =
    "it goes with the session's sandbox, which this launch does not pass";

/// The sandbox a launch passes, the pin or else the session's, and what
/// the session's `workspace-write` sandbox allowed beyond the workspace,
/// its writable roots and network access, which travel with the session's
/// own mode and only with it (M2-21): passed with it, this node's
/// `[sandbox_workspace_write]` neither widens nor narrows the session's
/// sandbox. When the mode passed is a pin, the session's are reported as
/// left out, and a pin's own are passed.
fn sandbox_and_access(
    pinned: Option<&SandboxMode>,
    recorded: Option<&RecordedSettings>,
    report: &mut LaunchSettings,
) -> Option<SandboxMode> {
    let recorded_sandbox = recorded.map(|r| &r.sandbox);
    let session_sandbox =
        pinned.is_none() && matches!(recorded_sandbox, Some(Recorded::Carried(_)));
    let sandbox = pick(
        SettingName::Sandbox,
        pinned,
        recorded_sandbox,
        |v| v.as_str().to_owned(),
        report,
    );
    let writable = recorded.map(|r| &r.writable);
    let network = recorded.map(|r| &r.network);
    let spell_network = |v: &bool| network_spelled(*v).to_owned();
    if session_sandbox && sandbox == Some(SandboxMode::WorkspaceWrite) {
        let writable = pick(
            SettingName::WritableRoots,
            None,
            writable,
            WritableRoots::spelled,
            report,
        );
        let network_access = pick(
            SettingName::NetworkAccess,
            None,
            network,
            spell_network,
            report,
        );
        if writable.is_none() && network_access.is_none() {
            return sandbox;
        }
        return Some(SandboxMode::WorkspaceWriteWith(SandboxAccess {
            writable,
            network_access,
        }));
    }
    if let Some(SandboxMode::WorkspaceWriteWith(access)) = &sandbox {
        if let Some(pin) = &access.writable {
            pick(
                SettingName::WritableRoots,
                Some(pin),
                None,
                WritableRoots::spelled,
                report,
            );
        }
        if let Some(pin) = &access.network_access {
            pick(
                SettingName::NetworkAccess,
                Some(pin),
                None,
                spell_network,
                report,
            );
        }
    }
    leave_out(
        SettingName::WritableRoots,
        None,
        writable,
        WritableRoots::spelled,
        ACCESS_GOES_WITH_SANDBOX,
        report,
    );
    leave_out(
        SettingName::NetworkAccess,
        None,
        network,
        spell_network,
        ACCESS_GOES_WITH_SANDBOX,
        report,
    );
    sandbox
}

/// The overrides a launch passes, and what to say about them: each field
/// the adapter was configured with, else the session's own when the
/// launch resumes one and Fabric carries its value. A `headless` launch
/// passes no approval policy, because `codex exec` sets its own, and a
/// terminal launch does not carry a recorded `never`, because `codex
/// exec` records the one it forced. A terminal launch carries the
/// session's reviewer only when it carries the session's policy, and any
/// launch carries the session's model provider only with the session's
/// model.
pub(crate) fn effective(
    pinned: Option<&CodexOverrides>,
    recorded: Option<&RecordedSettings>,
    headless: bool,
) -> (LaunchOverrides, LaunchSettings) {
    let mut report = LaunchSettings::default();
    let (model, provider) =
        model_and_provider(pinned.and_then(|p| p.model.as_ref()), recorded, &mut report);
    let effort = pick(
        SettingName::Effort,
        pinned.and_then(|p| p.effort.as_ref()),
        recorded.map(|r| &r.effort),
        |v| v.as_str().to_owned(),
        &mut report,
    );
    let sandbox = sandbox_and_access(
        pinned.and_then(|p| p.sandbox.as_ref()),
        recorded,
        &mut report,
    );
    let pinned_approval = pinned.and_then(|p| p.approval.as_ref());
    let recorded_approval = recorded.map(|r| &r.approval);
    let spell_approval = |v: &ApprovalPolicy| v.as_str().to_owned();
    let mut session_approval = false;
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
        session_approval =
            pinned_approval.is_none() && matches!(recorded_approval, Some(Recorded::Carried(_)));
        pick(
            SettingName::Approval,
            pinned_approval,
            recorded_approval.as_ref(),
            spell_approval,
            &mut report,
        )
    };
    // Under `codex exec resume`, Codex restores the thread's own reviewer
    // when none is passed, so only a pin is. A terminal launch passes the
    // session's reviewer only with the session's policy.
    let pinned_reviewer = pinned.and_then(|p| p.reviewer.as_ref());
    let recorded_reviewer = recorded.map(|r| &r.reviewer);
    let spell_reviewer = |v: &ApprovalsReviewer| v.as_str().to_owned();
    let reviewer = if headless {
        pick(
            SettingName::Reviewer,
            pinned_reviewer,
            None,
            spell_reviewer,
            &mut report,
        )
    } else if pinned_reviewer.is_some() || session_approval {
        pick(
            SettingName::Reviewer,
            pinned_reviewer,
            recorded_reviewer,
            spell_reviewer,
            &mut report,
        )
    } else {
        leave_out(
            SettingName::Reviewer,
            None,
            recorded_reviewer,
            spell_reviewer,
            REVIEWER_GOES_WITH_APPROVAL,
            &mut report,
        );
        None
    };
    let overrides = LaunchOverrides {
        settings: CodexOverrides {
            sandbox,
            approval,
            reviewer,
            model,
            effort,
        },
        provider,
    };
    (overrides, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> RecordedSettings {
        RecordedSettings {
            model: Recorded::Carried(ModelName::parse("gpt-6-sol").unwrap()),
            provider: Recorded::Absent,
            effort: Recorded::NotCarried {
                value: "turbo".to_owned(),
                reason: "not an effort codex 0.155.1 names".to_owned(),
            },
            approval: Recorded::Carried(ApprovalPolicy::OnRequest),
            reviewer: Recorded::Carried(ApprovalsReviewer::User),
            sandbox: Recorded::Carried(SandboxMode::WorkspaceWrite),
            writable: Recorded::Absent,
            network: Recorded::Absent,
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
            overrides.settings,
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
                "turbo",
                "not an effort codex 0.155.1 names"
            )]
        );
        let (none, report) = effective(None, None, false);
        assert_eq!(none, LaunchOverrides::default());
        assert_eq!(report, LaunchSettings::default());
    }

    /// `codex exec` sets its own approval policy over any `-c`, so a
    /// headless launch passes none and says which one it left out, the
    /// pin's or else the session's. It passes no reviewer either and says
    /// nothing of it: `codex exec resume` restores the thread's own.
    #[test]
    fn a_headless_launch_leaves_the_approval_policy_to_codex_exec() {
        let (overrides, report) = effective(None, Some(&recorded()), true);
        assert_eq!(
            (overrides.settings.approval, overrides.settings.reviewer),
            (None, None)
        );
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
        assert_eq!(overrides, LaunchOverrides::default());
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
        assert_eq!(overrides.settings.approval, None);
        assert!(
            report
                .passed
                .iter()
                .all(|p| p.name != SettingName::Approval)
        );
        assert!(report.not_carried.contains(&NotCarried::new(
            SettingName::Approval,
            "never",
            EXEC_RECORDS_NEVER
        )));
        let pinned = CodexOverrides {
            approval: Some(ApprovalPolicy::Never),
            ..CodexOverrides::default()
        };
        let (overrides, _) = effective(Some(&pinned), Some(&after_exec), false);
        assert_eq!(overrides.settings.approval, Some(ApprovalPolicy::Never));
    }

    /// The reviewer says who answers what the approval policy asks, so a
    /// terminal launch passes the session's reviewer only with the
    /// session's policy. When the policy it passes is the node's, a
    /// recorded `never` or one Fabric does not carry having been left out,
    /// or a pin's, the session's reviewer is left out too and reported, so
    /// a node that sends approvals to auto review is not made to ask the
    /// terminal. A pinned reviewer still passes.
    #[test]
    fn a_terminal_launch_carries_the_reviewer_only_with_the_policy() {
        let reviewer_left_out =
            NotCarried::new(SettingName::Reviewer, "user", REVIEWER_GOES_WITH_APPROVAL);
        let after_exec = RecordedSettings {
            approval: Recorded::Carried(ApprovalPolicy::Never),
            ..recorded()
        };
        let not_carried_policy = RecordedSettings {
            approval: Recorded::NotCarried {
                value: "on-failure".to_owned(),
                reason: "not an approval policy Fabric passes".to_owned(),
            },
            ..recorded()
        };
        let pinned_policy = CodexOverrides {
            approval: Some(ApprovalPolicy::OnRequest),
            ..CodexOverrides::default()
        };
        for (pinned, session) in [
            (None, &after_exec),
            (None, &not_carried_policy),
            (Some(&pinned_policy), &recorded()),
        ] {
            let (overrides, report) = effective(pinned, Some(session), false);
            assert_eq!(overrides.settings.reviewer, None, "{pinned:?} {session:?}");
            assert!(
                report
                    .passed
                    .iter()
                    .all(|p| p.name != SettingName::Reviewer),
                "{report:?}"
            );
            assert_eq!(report.not_carried.last(), Some(&reviewer_left_out));
        }

        let (overrides, _) = effective(None, Some(&recorded()), false);
        assert_eq!(overrides.settings.reviewer, Some(ApprovalsReviewer::User));

        let pinned_reviewer = CodexOverrides {
            reviewer: Some(ApprovalsReviewer::AutoReview),
            ..CodexOverrides::default()
        };
        let (overrides, report) = effective(Some(&pinned_reviewer), Some(&after_exec), false);
        assert_eq!(
            overrides.settings.reviewer,
            Some(ApprovalsReviewer::AutoReview)
        );
        assert!(
            report
                .not_carried
                .iter()
                .all(|n| n.name != SettingName::Reviewer)
        );
    }

    /// The writable roots and network access of the session's own
    /// sandbox.
    fn access() -> (WritableRoots, bool) {
        (
            WritableRoots {
                roots: vec![
                    WritableRoot::parse("/w/extra").unwrap(),
                    WritableRoot::parse("/tmp/a \"b\\c").unwrap(),
                ],
                exclude_tmpdir_env_var: Some(false),
                exclude_slash_tmp: Some(true),
            },
            true,
        )
    }

    fn with_access() -> RecordedSettings {
        let (writable, network) = access();
        RecordedSettings {
            writable: Recorded::Carried(writable),
            network: Recorded::Carried(network),
            ..recorded()
        }
    }

    /// M2-21: what the session's `workspace-write` sandbox allowed beyond
    /// the workspace travels with the session's mode, right after it, as
    /// `sandbox_workspace_write.*`, each root a quoted TOML string, and the
    /// launch says so.
    #[test]
    fn the_session_s_sandbox_travels_whole() {
        for headless in [true, false] {
            let (overrides, report) = effective(None, Some(&with_access()), headless);
            let pairs = overrides.pairs();
            let at = pairs
                .iter()
                .position(|p| p == "sandbox_mode=\"workspace-write\"")
                .unwrap();
            assert_eq!(
                pairs[at..at + 5],
                [
                    "sandbox_mode=\"workspace-write\"",
                    r#"sandbox_workspace_write.writable_roots=["/w/extra","/tmp/a \"b\\c"]"#,
                    "sandbox_workspace_write.network_access=true",
                    "sandbox_workspace_write.exclude_tmpdir_env_var=false",
                    "sandbox_workspace_write.exclude_slash_tmp=true",
                ]
            );
            let passed: Vec<(SettingName, &str, Origin)> = report
                .passed
                .iter()
                .map(|p| (p.name, p.value.as_str(), p.origin))
                .filter(|(name, _, _)| {
                    matches!(
                        name,
                        SettingName::Sandbox
                            | SettingName::WritableRoots
                            | SettingName::NetworkAccess
                    )
                })
                .collect();
            assert_eq!(
                passed,
                [
                    (SettingName::Sandbox, "workspace-write", Origin::Session),
                    (
                        SettingName::WritableRoots,
                        "[/w/extra, /tmp/a \"b\\c] without /tmp",
                        Origin::Session
                    ),
                    (SettingName::NetworkAccess, "on", Origin::Session),
                ]
            );
        }
    }

    /// A pinned sandbox is this node's choice: the session's writable
    /// roots and network access do not go with it, and are said to be
    /// left out; a pin that names its own are passed as pinned.
    #[test]
    fn a_pinned_sandbox_leaves_the_session_s_access_out() {
        let left_out = [
            NotCarried::new(
                SettingName::WritableRoots,
                "[/w/extra, /tmp/a \"b\\c] without /tmp",
                ACCESS_GOES_WITH_SANDBOX,
            ),
            NotCarried::new(SettingName::NetworkAccess, "on", ACCESS_GOES_WITH_SANDBOX),
        ];
        for mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
            let pinned = CodexOverrides {
                sandbox: Some(mode.clone()),
                ..CodexOverrides::default()
            };
            let (overrides, report) = effective(Some(&pinned), Some(&with_access()), false);
            assert_eq!(overrides.settings.sandbox, Some(mode));
            assert!(
                overrides
                    .pairs()
                    .iter()
                    .all(|p| !p.starts_with("sandbox_workspace_write.")),
                "{overrides:?}"
            );
            for left in &left_out {
                assert!(report.not_carried.contains(left), "{report:?}");
            }
        }

        let (writable, _) = access();
        let pinned = CodexOverrides {
            sandbox: Some(SandboxMode::WorkspaceWriteWith(SandboxAccess {
                writable: Some(WritableRoots {
                    roots: writable.roots[..1].to_vec(),
                    ..WritableRoots::default()
                }),
                network_access: Some(false),
            })),
            ..CodexOverrides::default()
        };
        let (overrides, report) = effective(Some(&pinned), Some(&with_access()), true);
        let pairs = overrides.pairs();
        assert!(
            pairs.contains(&r#"sandbox_workspace_write.writable_roots=["/w/extra"]"#.to_owned())
                && pairs.contains(&"sandbox_workspace_write.network_access=false".to_owned()),
            "{pairs:?}"
        );
        assert!(report.passed.contains(&PassedSetting {
            name: SettingName::NetworkAccess,
            value: "off".to_owned(),
            origin: Origin::Pinned,
        }));
        assert!(report.not_carried.contains(&left_out[1]), "{report:?}");
    }

    /// A read-only session that reached the network passes its mode and
    /// says the network access was left out, with why.
    #[test]
    fn a_read_only_session_s_network_access_is_reported_not_passed() {
        let session = RecordedSettings {
            sandbox: Recorded::Carried(SandboxMode::ReadOnly),
            network: Recorded::NotCarried {
                value: "on".to_owned(),
                reason: "codex takes none for read-only".to_owned(),
            },
            ..recorded()
        };
        let (overrides, report) = effective(None, Some(&session), false);
        assert_eq!(overrides.settings.sandbox, Some(SandboxMode::ReadOnly));
        assert!(
            report.not_carried.contains(&NotCarried::new(
                SettingName::NetworkAccess,
                "on",
                "codex takes none for read-only"
            )),
            "{report:?}"
        );
    }

    #[test]
    fn a_writable_root_is_an_absolute_path_without_control_characters() {
        assert!(WritableRoot::parse("/a/b c").is_ok());
        assert!(WritableRoot::parse("a/b").is_err());
        assert!(WritableRoot::parse("").is_err());
        assert!(WritableRoot::parse("/a\nb").is_err());
        assert!(WritableRoot::parse("/a\u{7f}").is_err());
    }

    /// A model is served by a provider, and Codex sends this node's
    /// provider with any `-c model`, so the session's provider travels
    /// with the session's model, right after it, and only with it: a
    /// pinned model, or a recorded one Fabric does not carry, leaves the
    /// session's provider out and says so, and the node's provider goes
    /// with the node's model. A later settings change names the provider
    /// that serves its model.
    #[test]
    fn the_model_provider_travels_only_with_the_session_s_model() {
        let azure = RecordedSettings {
            provider: Recorded::Carried(ProviderId::parse("azure").unwrap()),
            ..recorded()
        };
        for headless in [true, false] {
            let (overrides, report) = effective(None, Some(&azure), headless);
            assert_eq!(
                overrides.provider.as_ref().map(ProviderId::as_str),
                Some("azure")
            );
            assert_eq!(
                overrides.pairs()[..2],
                ["model=\"gpt-6-sol\"", "model_provider=\"azure\""]
            );
            assert_eq!(
                report.passed[..2],
                [
                    PassedSetting {
                        name: SettingName::Model,
                        value: "gpt-6-sol".to_owned(),
                        origin: Origin::Session,
                    },
                    PassedSetting {
                        name: SettingName::Provider,
                        value: "azure".to_owned(),
                        origin: Origin::Session,
                    },
                ]
            );
        }

        let provider_left_out =
            NotCarried::new(SettingName::Provider, "azure", PROVIDER_GOES_WITH_MODEL);
        let pinned_model = CodexOverrides {
            model: Some(ModelName::parse("o3").unwrap()),
            ..CodexOverrides::default()
        };
        let model_not_carried = RecordedSettings {
            model: Recorded::NotCarried {
                value: "gpt 6".to_owned(),
                reason: "not a model name Fabric passes".to_owned(),
            },
            ..azure.clone()
        };
        for (pinned, session) in [(Some(&pinned_model), &azure), (None, &model_not_carried)] {
            let (overrides, report) = effective(pinned, Some(session), false);
            assert_eq!(overrides.provider, None, "{pinned:?} {session:?}");
            assert!(
                overrides
                    .pairs()
                    .iter()
                    .all(|p| !p.starts_with("model_provider=")),
                "{overrides:?}"
            );
            assert!(
                report
                    .passed
                    .iter()
                    .all(|p| p.name != SettingName::Provider),
                "{report:?}"
            );
            assert!(
                report.not_carried.contains(&provider_left_out),
                "{report:?}"
            );
        }
    }
}
