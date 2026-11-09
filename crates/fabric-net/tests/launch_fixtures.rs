//! The launch path fixtures in `protocol/net/v4` (docs/net.md, "Launch
//! path fixtures") are the contract between the authority and the node
//! that runs a grant. This test checks that every fixture matches
//! `manifest.sha256`, that no file is missing from or extra to it, and
//! that the manifest's own digest is the one pinned below; then it builds
//! each case's message with the Rust constructors, encodes it as a request
//! in a control frame and decodes it back, and checks that each case
//! marked `reject` is refused by the check its `reason` names: the list
//! bounds of the wire, `LaunchRequest::check` or `Report::check`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use fabric_core::harness::{HarnessKind, ProgramName};
use fabric_core::id::{JobId, NodeId, SnapshotId};
use fabric_core::managed::launch::{
    AfterCut, ContinuationAsk, GrantToken, Heartbeat, Instruction, LaunchRefusal, LaunchRequest,
    Program, Report, ReportRefusal, TaskLaunch,
};
use fabric_core::managed::lifecycle::{
    CheckpointCut, EvidenceKind, EvidenceRef, HandoffBlocker, ReceiptStatus, TaskClass,
    TaskReceipt, WorkspaceRevision,
};
use fabric_core::managed::package::{
    AllowedFidelity, ArtifactRef, Decision, Fidelity, NextAction, PackageDigest, Question,
    TaskSummary, WorkPackage,
};
use fabric_core::managed::secret_ref::{
    AttemptScope, ResourceKind, SecretName, SecretRef, SecretScope,
};
use fabric_core::managed::source::{CommitId, GitUri};
use fabric_core::managed::{AttemptId, EventDigest, Generation, TaskId, WorkRevision};
use fabric_core::managed::{
    ExitOutcome, Intent, JobPolicy, Observation, RestartPolicy, SourceBinding, TaskState,
};
use fabric_core::path::{AbsPath, RelPath};
use fabric_net::wire::{Request, RequestBody, RequestId, WireError};
use serde_json::Value;

#[path = "../../fabric-client/tests/common/mod.rs"]
mod common;
use common::sha256;

/// The sha256 of `protocol/net/v4/manifest.sha256`. It moves only with the
/// fixtures, in the same commit, and docs/net.md records the same value.
const MANIFEST_SHA256: &str = "0aae368ea37e5082c791a970b3a829800a85668cb16bb8f0e10d1293e427c6b1";

type Fallible<T> = Result<T, Box<dyn Error>>;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/net/v4")
}

fn cases() -> Fallible<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root().join("cases"))? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("a case name is not text")?
            .to_owned();
        out.push((name, serde_json::from_slice(&fs::read(&path)?)?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

// The text forms of docs/net.md: ids as text, counters as decimal
// strings, enums by their snake-case names.

fn field<'a>(value: &'a Value, name: &str) -> Fallible<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| format!("no field {name} in {value}").into())
}

fn text<'a>(value: &'a Value, name: &str) -> Fallible<&'a str> {
    field(value, name)?
        .as_str()
        .ok_or_else(|| format!("{name} is not text").into())
}

fn parsed<T: FromStr>(value: &Value, name: &str) -> Fallible<T>
where
    T::Err: Error + 'static,
{
    Ok(text(value, name)?.parse()?)
}

fn optional<T>(
    value: &Value,
    name: &str,
    read: impl Fn(&Value) -> Fallible<T>,
) -> Fallible<Option<T>> {
    match field(value, name)? {
        Value::Null => Ok(None),
        v => read(v).map(Some),
    }
}

fn list<T: Clone>(value: &Value, read: impl Fn(&Value) -> Fallible<T>) -> Fallible<Vec<T>> {
    if let Some(items) = value.as_array() {
        return items.iter().map(read).collect();
    }
    // The `{ "repeat", "count" }` form of a list past its bound.
    let one = read(field(value, "repeat")?)?;
    let count = field(value, "count")?
        .as_u64()
        .ok_or("count is not a number")?;
    Ok(vec![one; usize::try_from(count)?])
}

fn hex32(text: &str) -> Fallible<[u8; 32]> {
    if text.len() != 64 {
        return Err(format!("{text} is not 32 bytes of hex").into());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)?;
    }
    Ok(out)
}

fn u32_of(value: &Value, name: &str) -> Fallible<u32> {
    Ok(u32::try_from(
        field(value, name)?.as_u64().ok_or("not a number")?,
    )?)
}

fn token(value: &Value) -> Fallible<GrantToken> {
    Ok(GrantToken {
        job: parsed(value, "job")?,
        attempt: parsed(value, "attempt")?,
        generation: parsed(value, "generation")?,
    })
}

fn class(name: &str) -> Fallible<TaskClass> {
    Ok(match name {
        "build" => TaskClass::Build,
        "test" => TaskClass::Test,
        "harness" => TaskClass::Harness,
        "opaque" => TaskClass::Opaque,
        other => return Err(format!("no class {other}").into()),
    })
}

fn program(value: &Value) -> Fallible<Program> {
    Ok(Program {
        argv: list(field(value, "argv")?, |v| {
            Ok(v.as_str().ok_or("an argument is not text")?.to_owned())
        })?,
        cwd: optional(value, "cwd", |v| {
            Ok(RelPath::parse(v.as_str().ok_or("cwd is not text")?)?)
        })?,
        env_allowlist: list(field(value, "env_allowlist")?, |v| {
            Ok(v.as_str().ok_or("a name is not text")?.parse()?)
        })?,
    })
}

fn task_launch(value: &Value) -> Fallible<TaskLaunch> {
    Ok(TaskLaunch {
        task: parsed(value, "task")?,
        class: class(text(value, "class")?)?,
        program: program(field(value, "program")?)?,
    })
}

fn harness(value: &Value) -> Fallible<HarnessKind> {
    Ok(match text(value, "kind")? {
        "claude_code" => HarnessKind::ClaudeCode,
        "codex" => HarnessKind::Codex,
        "acp" => HarnessKind::Acp {
            program: ProgramName::parse(text(value, "program")?)?,
            args: list(field(value, "args")?, |v| {
                Ok(v.as_str().ok_or("an argument is not text")?.to_owned())
            })?,
        },
        other => return Err(format!("no harness {other}").into()),
    })
}

fn strings(value: &Value) -> Fallible<Vec<String>> {
    list(value, |v| Ok(v.as_str().ok_or("not text")?.to_owned()))
}

fn intent(value: &Value) -> Fallible<Intent> {
    Ok(Intent {
        goal: text(value, "goal")?.to_owned(),
        harness: harness(field(value, "harness")?)?,
        acceptance: strings(field(value, "acceptance")?)?,
    })
}

fn source(value: &Value) -> Fallible<SourceBinding> {
    Ok(match text(value, "kind")? {
        "empty" => SourceBinding::Empty,
        "git" => SourceBinding::Git {
            uri: GitUri::parse(text(value, "uri")?)?,
            commit: CommitId::parse(text(value, "commit")?)?,
        },
        "repository" => SourceBinding::Repository {
            path: AbsPath::parse(text(value, "path")?)?,
        },
        "snapshot" => SourceBinding::Snapshot {
            snapshot: parsed(value, "snapshot")?,
        },
        other => return Err(format!("no source {other}").into()),
    })
}

fn allowed(value: &Value) -> Fallible<AllowedFidelity> {
    let mut out = AllowedFidelity {
        native_session: false,
        new_native_session_with_package: false,
        workspace_only: false,
    };
    for name in strings(value)? {
        match Fidelity::from_name(&name) {
            Some(Fidelity::NativeSession) => out.native_session = true,
            Some(Fidelity::NewNativeSessionWithPackage) => {
                out.new_native_session_with_package = true;
            }
            Some(Fidelity::WorkspaceOnly) => out.workspace_only = true,
            _ => return Err(format!("no allowed fidelity {name}").into()),
        }
    }
    Ok(out)
}

fn policy(value: &Value) -> Fallible<JobPolicy> {
    let mut policy = JobPolicy::m2_defaults(RestartPolicy::Block);
    policy.drain_budget_ms = u32_of(value, "drain_budget_ms")?;
    policy.allowed_fidelity = allowed(field(value, "allowed_fidelity")?)?;
    policy.secrets = list(field(value, "secrets")?, |v| {
        Ok(SecretRef {
            name: SecretName::parse(text(v, "name")?)?,
            scope: SecretScope {
                attempt: match text(v, "attempt")? {
                    "any" => AttemptScope::Any,
                    "current" => AttemptScope::Current,
                    other => return Err(format!("no attempt scope {other}").into()),
                },
                resource_kinds: list(field(v, "resource_kinds")?, |k| match k.as_str() {
                    Some("node") => Ok(ResourceKind::Node),
                    _ => Err("no resource kind".into()),
                })?,
            },
        })
    })?;
    policy.export_allowlist = list(field(value, "export_allowlist")?, |v| {
        Ok(RelPath::parse(v.as_str().ok_or("not text")?)?)
    })?;
    policy.placement.disk_bytes = field(value, "disk_bytes")?
        .as_u64()
        .ok_or("disk_bytes is not a number")?;
    policy.placement.pin = optional(value, "pin", |v| {
        Ok(v.as_str().ok_or("pin is not text")?.parse::<NodeId>()?)
    })?;
    Ok(policy)
}

fn workspace(value: &Value) -> Fallible<WorkspaceRevision> {
    Ok(WorkspaceRevision {
        bundle_head: parsed(value, "bundle_head")?,
        ledger_head: EventDigest::from_bytes(hex32(text(value, "ledger_head")?)?),
    })
}

fn receipt_status(value: &Value) -> Fallible<ReceiptStatus> {
    if let Some(name) = value.as_str() {
        return Ok(match name {
            "running" => ReceiptStatus::Running,
            "exited" => ReceiptStatus::Exited,
            "interrupted" => ReceiptStatus::Interrupted,
            other => return Err(format!("no receipt status {other}").into()),
        });
    }
    if let Some(inner) = value.get("checkpointed") {
        return Ok(ReceiptStatus::Checkpointed {
            snapshot: parsed(inner, "snapshot")?,
        });
    }
    if let Some(inner) = value.get("blocked") {
        return Ok(ReceiptStatus::Blocked {
            reason: text(inner, "reason")?.to_owned(),
        });
    }
    Err(format!("no receipt status {value}").into())
}

fn receipt(value: &Value) -> Fallible<TaskReceipt> {
    Ok(TaskReceipt {
        task: parsed(value, "task")?,
        class: class(text(value, "class")?)?,
        status: receipt_status(field(value, "status")?)?,
        exit: optional(value, "exit", |v| {
            Ok(i32::try_from(v.as_i64().ok_or("exit is not a number")?)?)
        })?,
        interrupted: field(value, "interrupted")?
            .as_bool()
            .ok_or("interrupted is not a bool")?,
        workspace: optional(value, "workspace", workspace)?,
    })
}

fn evidence(value: &Value) -> Fallible<EvidenceRef> {
    Ok(EvidenceRef {
        digest: hex32(text(value, "digest")?)?,
        attempt: parsed(value, "attempt")?,
        kind: match text(value, "kind")? {
            "checkpoint" => EvidenceKind::Checkpoint,
            "exit" => EvidenceKind::Exit,
            "receipt" => EvidenceKind::Receipt,
            other => return Err(format!("no evidence kind {other}").into()),
        },
    })
}

fn task_state(name: &str) -> Fallible<TaskState> {
    Ok(match name {
        "open" => TaskState::Open,
        "succeeded" => TaskState::Succeeded,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        other => return Err(format!("no task state {other}").into()),
    })
}

fn next_action(value: &Value) -> Fallible<NextAction> {
    if let Some(name) = value.as_str() {
        return Ok(match name {
            "continue" => NextAction::Continue,
            "checkpoint" => NextAction::Checkpoint,
            "cancel" => NextAction::Cancel,
            other => return Err(format!("no next action {other}").into()),
        });
    }
    if let Some(inner) = value.get("finish_task") {
        return Ok(NextAction::FinishTask {
            task: parsed(inner, "task")?,
        });
    }
    if let Some(inner) = value.get("register_task") {
        return Ok(NextAction::RegisterTask {
            parent: parsed(inner, "parent")?,
        });
    }
    Err(format!("no next action {value}").into())
}

fn package(value: &Value) -> Fallible<WorkPackage> {
    Ok(WorkPackage {
        version: u32_of(value, "version")?,
        job: parsed(value, "job")?,
        revision: parsed(value, "revision")?,
        intent: intent(field(value, "intent")?)?,
        decisions: list(field(value, "decisions")?, |v| {
            Ok(Decision {
                what: text(v, "what")?.to_owned(),
                reason: text(v, "reason")?.to_owned(),
                revision: parsed(v, "revision")?,
            })
        })?,
        tasks: list(field(value, "tasks")?, |v| {
            Ok(TaskSummary {
                task: parsed(v, "task")?,
                class: class(text(v, "class")?)?,
                state: task_state(text(v, "state")?)?,
                receipt: optional(v, "receipt", receipt)?,
            })
        })?,
        workspace: workspace(field(value, "workspace")?)?,
        evidence: list(field(value, "evidence")?, evidence)?,
        questions: list(field(value, "questions")?, |v| {
            Ok(Question {
                text: text(v, "text")?.to_owned(),
                revision: parsed::<WorkRevision>(v, "revision")?,
            })
        })?,
        artifacts: list(field(value, "artifacts")?, |v| {
            Ok(ArtifactRef {
                name: text(v, "name")?.to_owned(),
                digest: hex32(text(v, "digest")?)?,
                bytes: field(v, "bytes")?.as_u64().ok_or("bytes is not a number")?,
            })
        })?,
        next_actions: list(field(value, "next_actions")?, next_action)?,
    })
}

fn blocker(value: &Value) -> Fallible<HandoffBlocker> {
    if let Some(inner) = value.get("escaped_child") {
        return Ok(HandoffBlocker::EscapedChild {
            pid: u32_of(inner, "pid")?,
            group: u32_of(inner, "group")?,
        });
    }
    if let Some(inner) = value.get("uncontrolled_writer") {
        return Ok(HandoffBlocker::UncontrolledWriter {
            pid: u32_of(inner, "pid")?,
            path: RelPath::parse(text(inner, "path")?)?,
        });
    }
    if let Some(inner) = value.get("checkpoint_failed") {
        return Ok(HandoffBlocker::CheckpointFailed {
            task: parsed(inner, "task")?,
            reason: text(inner, "reason")?.to_owned(),
        });
    }
    if let Some(inner) = value.get("opaque") {
        return Ok(HandoffBlocker::Opaque {
            task: parsed(inner, "task")?,
        });
    }
    Err(format!("no blocker {value}").into())
}

fn outcome(value: &Value) -> Fallible<ExitOutcome> {
    match value.as_str() {
        Some("succeeded") => return Ok(ExitOutcome::Succeeded),
        Some("cancelled") => return Ok(ExitOutcome::Cancelled),
        _ => {}
    }
    let failed = field(value, "failed")?;
    Ok(ExitOutcome::Failed {
        code: optional(failed, "code", |v| {
            Ok(i32::try_from(v.as_i64().ok_or("code is not a number")?)?)
        })?,
    })
}

fn observation(value: &Value) -> Fallible<Observation> {
    let attempt: AttemptId = parsed(value, "attempt")?;
    let generation: Generation = parsed(value, "generation")?;
    Ok(match text(value, "kind")? {
        "started" => Observation::Started {
            attempt,
            generation,
        },
        "receipts" => Observation::Receipts {
            attempt,
            generation,
            receipts: list(field(value, "receipts")?, receipt)?,
        },
        "checkpointed" => {
            let cut = field(value, "cut")?;
            Observation::Checkpointed {
                attempt,
                generation,
                snapshot: parsed(value, "snapshot")?,
                cut: CheckpointCut {
                    snapshot: parsed(cut, "snapshot")?,
                    receipts: list(field(cut, "receipts")?, receipt)?,
                    facts: list(field(cut, "facts")?, |pair| {
                        let pair = pair.as_array().ok_or("a fact is not a pair")?;
                        match pair.as_slice() {
                            [Value::String(k), Value::String(v)] => Ok((k.clone(), v.clone())),
                            _ => Err("a fact is not two texts".into()),
                        }
                    })?,
                    evidence: list(field(cut, "evidence")?, evidence)?,
                },
            }
        }
        "handoff_blocked" => Observation::HandoffBlocked {
            attempt,
            generation,
            blockers: list(field(value, "blockers")?, blocker)?,
        },
        "continued" => Observation::Continued {
            attempt,
            generation,
            from: parsed(value, "from")?,
            achieved: Fidelity::from_name(text(value, "achieved")?).ok_or("no fidelity")?,
            package: optional(value, "package", |v| {
                Ok(PackageDigest(hex32(v.as_str().ok_or("not text")?)?))
            })?,
        },
        "exited" => Observation::Exited {
            attempt,
            generation,
            outcome: outcome(field(value, "outcome")?)?,
        },
        "lost" => Observation::Lost {
            attempt,
            generation,
        },
        other => return Err(format!("no observation {other}").into()),
    })
}

fn instruction(value: &Value) -> Fallible<Instruction> {
    if let Some(inner) = value.get("spawn") {
        return Ok(Instruction::Spawn(task_launch(inner)?));
    }
    if let Some(inner) = value.get("checkpoint") {
        return Ok(Instruction::Checkpoint {
            then: match text(inner, "then")? {
                "resume" => AfterCut::Resume,
                "end" => AfterCut::End,
                other => return Err(format!("no after-cut {other}").into()),
            },
        });
    }
    if let Some(inner) = value.get("stop") {
        return Ok(Instruction::Stop {
            reason: text(inner, "reason")?.to_owned(),
        });
    }
    Err(format!("no instruction {value}").into())
}

fn launch(m: &Value) -> Fallible<LaunchRequest> {
    Ok(LaunchRequest {
        token: token(field(m, "token")?)?,
        task: parsed::<TaskId>(m, "task")?,
        intent: intent(field(m, "intent")?)?,
        source: source(field(m, "source")?)?,
        policy: policy(field(m, "policy")?)?,
        initial_checkpoint: optional(m, "initial_checkpoint", |v| {
            Ok(v.as_str().ok_or("not text")?.parse::<SnapshotId>()?)
        })?,
        snapshot_at: optional(m, "snapshot_at", |v| {
            Ok(v.as_str().ok_or("not text")?.parse::<NodeId>()?)
        })?,
        tasks: list(field(m, "tasks")?, task_launch)?,
        continuation: optional(m, "continuation", |v| {
            Ok(ContinuationAsk {
                from: parsed(v, "from")?,
                allowed: allowed(field(v, "allowed")?)?,
            })
        })?,
        package: optional(m, "package", package)?,
    })
}

/// The request body a case's message builds, with the Rust constructors.
fn build(message: &Value) -> Fallible<RequestBody> {
    Ok(match text(message, "type")? {
        "launch" => RequestBody::Launch(Box::new(launch(message)?)),
        "instruct" => RequestBody::Instruct {
            token: token(field(message, "token")?)?,
            instruction: instruction(field(message, "instruction")?)?,
        },
        "report" => RequestBody::Report(Box::new(Report {
            token: token(field(message, "token")?)?,
            observation: observation(field(message, "observation")?)?,
        })),
        "heartbeat" => RequestBody::Heartbeat(Heartbeat {
            attempts: list(field(message, "attempts")?, token)?,
            running: field(message, "running")?
                .as_bool()
                .ok_or("running is not a boolean")?,
        }),
        other => return Err(format!("no message type {other}").into()),
    })
}

fn request(body: RequestBody) -> Request {
    Request {
        id: RequestId(7),
        budget_ms: 5_000,
        body,
    }
}

/// The receiver's check of a message that passed the wire: the reason
/// it would refuse, or `None`.
fn refusal(body: &RequestBody) -> Option<&'static str> {
    match body {
        RequestBody::Launch(launch) => match launch.check() {
            Ok(()) => None,
            Err(LaunchRefusal::BadProgram { .. }) => Some("bad_program"),
            Err(LaunchRefusal::Inconsistent { .. }) => Some("inconsistent"),
            Err(_) => Some("other"),
        },
        RequestBody::Report(report) => match report.check() {
            Ok(()) => None,
            Err(ReportRefusal::TokenMismatch) => Some("token_mismatch"),
            Err(ReportRefusal::NotReportable) => Some("not_reportable"),
            Err(_) => Some("other"),
        },
        RequestBody::Instruct {
            instruction: Instruction::Spawn(launch),
            ..
        } => launch.program.check().err().map(|_| "bad_program"),
        _ => None,
    }
}

#[test]
fn the_manifest_is_the_pinned_one() -> Fallible<()> {
    let manifest = fs::read(root().join("manifest.sha256"))?;
    assert_eq!(
        sha256(&manifest),
        MANIFEST_SHA256,
        "protocol/net/v4/manifest.sha256 changed; move the pin here and in docs/net.md together"
    );
    let mut listed = Vec::new();
    for line in std::str::from_utf8(&manifest)?.lines() {
        let (digest, path) = line.split_once("  ").ok_or("a manifest line has no path")?;
        assert_eq!(
            sha256(&fs::read(root().join(path))?),
            digest,
            "{path} does not match the manifest"
        );
        listed.push(path.to_owned());
    }
    let found: Vec<String> = cases()?
        .into_iter()
        .map(|(name, _)| format!("cases/{name}"))
        .collect();
    assert_eq!(
        listed, found,
        "a case is missing from or extra to the manifest"
    );
    Ok(())
}

#[test]
fn every_case_builds_and_round_trips_in_a_control_frame() -> Fallible<()> {
    let mut accepted = 0;
    for (name, case) in cases()? {
        assert_eq!(
            text(&case, "case")?,
            name.trim_end_matches(".json"),
            "{name} names another case"
        );
        let direction = text(&case, "direction")?;
        let kind = text(field(&case, "message")?, "type")?;
        let expected = match kind {
            "launch" | "instruct" => "authority_to_node",
            _ => "node_to_authority",
        };
        assert_eq!(direction, expected, "{name} travels the wrong way");
        if text(field(&case, "expect")?, "outcome")? != "accept" {
            continue;
        }
        let built = request(build(field(&case, "message")?)?);
        let payload = built.to_control_payload()?;
        assert_eq!(Request::from_control_payload(&payload)?, built, "{name}");
        assert_eq!(
            refusal(&built.body),
            None,
            "{name} is refused by its receiver"
        );
        accepted += 1;
    }
    assert!(accepted >= 10, "only {accepted} accepted cases");
    Ok(())
}

#[test]
fn a_case_marked_reject_is_refused_where_it_says() -> Fallible<()> {
    let mut rejected = 0;
    for (name, case) in cases()? {
        let expect = field(&case, "expect")?;
        if text(expect, "outcome")? != "reject" {
            continue;
        }
        let reason = text(expect, "reason")?;
        let built = request(build(field(&case, "message")?)?);
        match reason {
            "too_many_tasks" | "too_many_attempts" => {
                assert!(
                    matches!(built.to_control_payload(), Err(WireError::TooMany { .. })),
                    "{name} is sent past its bound"
                );
                // A sender that skips the bound is refused when parsed.
                let raw = postcard::to_stdvec(&built)?;
                assert!(
                    matches!(
                        Request::from_control_payload(&raw),
                        Err(WireError::TooMany { .. })
                    ),
                    "{name} is parsed past its bound"
                );
            }
            _ => {
                let payload = built.to_control_payload()?;
                let parsed = Request::from_control_payload(&payload)?;
                assert_eq!(refusal(&parsed.body), Some(reason), "{name}");
            }
        }
        rejected += 1;
    }
    assert!(rejected >= 7, "only {rejected} rejected cases");
    Ok(())
}

#[test]
fn the_shared_ids_parse_as_the_fixtures_write_them() -> Fallible<()> {
    let _: JobId = "01K8W3A00000000000000000J1".parse()?;
    let _: NodeId = "fab1aibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaibaeaqcaiba".parse()?;
    Ok(())
}
