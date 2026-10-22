//! Codex rollout files: names, lookup and lineage.
//!
//! Invariant: a rollout is found by its file name alone,
//! `rollout-<YYYY-MM-DDTHH-MM-SS>-<thread_id>[_<rollout_id>].jsonl[.zst]`,
//! anywhere under `sessions/` or `archived_sessions/`, which is how Codex
//! itself finds it when its `SQLite` row is missing (`codex-rs/rollout/src/
//! list.rs`, `visit_rollout_filenames`, at `e4568a3`, 2026-10-19). Neither
//! walk follows a symlink, but Codex looks a thread up in its `SQLite`
//! database first and opens the path it holds, through any link
//! (`find_thread_path_by_id_str_in_subdir` at `rust-v0.155.1`), so a
//! rollout of the thread reached through a symlink under either folder is
//! refused (`link_reaching`), never skipped. A
//! paginated thread owns only its suffix; the prefix lives in the rollout
//! named by `session_meta.history_base.thread_id`, which is a rollout id,
//! and must travel byte for byte because the reference is a byte offset.
//! Because the bytes travel verbatim, a rollout that holds a secret-shaped
//! string is refused rather than carried.
//!
//! A session's rollouts are also the record of how it ran, so a resume
//! reads its settings from them instead of from a second copy in the job
//! record (gate finding F10a). Record shapes, read from the owner's
//! rollouts on 2026-10-21 (key names and setting values only): every turn
//! writes `{"type":"turn_context","payload":{"model","effort",
//! "approval_policy","sandbox_policy":{"type",...},...}}`, and every file
//! starts with `{"type":"session_meta","payload":{"cli_version",
//! "model_provider",...}}`; a `turn_context` names no provider. A
//! thread a parent spawned holds the parent's `session_meta` as a second
//! line, and a desktop build writes a pre-release such as
//! `0.163.0-alpha.10.2`. A setting changed between turns, such as with
//! `/model` or `/permissions`, is recorded as `{"type":"event_msg",
//! "payload":{"type":"thread_settings_applied","thread_settings":{"model",
//! "model_provider_id","reasoning_effort","approval_policy",
//! "approvals_reviewer","permission_profile",...}}}` (5,036 of them in
//! the owner's plain rollouts, read on 2026-10-21), and Codex 0.155.1
//! restores the newer of the two records on resume (`latest_persisted_resume_settings` in
//! `codex-rs/app-server` and `apply_rollout_item` in
//! `codex-rs/state/src/extract.rs`, at `rust-v0.155.1`). So the settings
//! are those of the last of either record, in lineage order: in 7 of the
//! owner's 12,238 rollouts a change after the last turn names another
//! model or effort, one of them from gpt-5.5, the model gate finding
//! F10a got a 400 for, to gpt-6-astra. A snapshot holds a permission
//! profile rather than a sandbox mode, and `take_snapshot` says which
//! mode it reads from one. The writer is the highest `cli_version` of
//! any `session_meta` line in any file of the lineage. That is the newest
//! Codex that created a file of the lineage, not always the newest that
//! wrote to it: a resume appends without a `session_meta` and records no
//! version (a rollout on the owner's Mac holds one `session_meta`, by
//! 0.151.0-alpha.7.2 on 2026-09-02, and turns to 2026-10-20 whose
//! `turn_context` keys that build did not write), so the resume guard
//! also takes the known writer the job carries (`Codex::with_known_writer`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

use crate::error::{HarnessError, Input, IoOp};
use crate::harness::ModelName;
pub(crate) use crate::harness::Recorded;
use crate::jsontext;
use crate::redact::Redactor;
use crate::session::SessionId;
use crate::version::HarnessVersion;

use super::overrides::{ApprovalPolicy, ApprovalsReviewer, Effort, ProviderId, SandboxMode};

/// What a rollout file name says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutName {
    /// The `YYYY-MM-DDTHH-MM-SS` text from the name; sorts in time order.
    pub timestamp: String,
    /// The stable thread id.
    pub thread_id: SessionId,
    /// The rollout id; equals the thread id except after a revert.
    pub rollout_id: SessionId,
    /// True when the file is `.jsonl.zst`.
    pub compressed: bool,
}

/// One rollout file as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutFile {
    /// The file, plain or compressed.
    pub path: PathBuf,
    /// What its name says.
    pub name: RolloutName,
}

fn is_uuid_like(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Parses a rollout file name; `None` for any other name.
#[must_use]
pub fn parse_name(name: &str) -> Option<RolloutName> {
    let (core, compressed) = match name.strip_suffix(".jsonl.zst") {
        Some(core) => (core, true),
        None => (name.strip_suffix(".jsonl")?, false),
    };
    let core = core.strip_prefix("rollout-")?;
    if core.len() < 21 || core.as_bytes()[19] != b'-' {
        return None;
    }
    let timestamp = &core[..19];
    let ts_ok = timestamp.char_indices().all(|(i, c)| match i {
        10 => c == 'T',
        4 | 7 | 13 | 16 => c == '-',
        _ => c.is_ascii_digit(),
    });
    if !ts_ok {
        return None;
    }
    let ids = &core[20..];
    let (thread, rollout) = ids.split_once('_').unwrap_or((ids, ids));
    if !is_uuid_like(thread) || !is_uuid_like(rollout) {
        return None;
    }
    Some(RolloutName {
        timestamp: timestamp.to_owned(),
        thread_id: SessionId::parse(thread).ok()?,
        rollout_id: SessionId::parse(rollout).ok()?,
        compressed,
    })
}

/// Lists every rollout file under `root`, walking all subdirectories,
/// newest first. When a plain and a compressed file share a directory and
/// a name, the plain one is listed, as Codex does. A symlink is not
/// followed and not listed.
pub fn list(root: &Path) -> Result<Vec<RolloutFile>, HarnessError> {
    Ok(scan(root)?.0)
}

/// The rollout files under `root`, as `list` gives them, and every
/// symlink the walk met and did not follow.
pub(crate) fn scan(root: &Path) -> Result<(Vec<RolloutFile>, Vec<PathBuf>), HarnessError> {
    let mut found: Vec<RolloutFile> = Vec::new();
    let mut links = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(HarnessError::io(IoOp::Walk, &dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &dir, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if kind.is_symlink() {
                links.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(name) = parse_name(file_name) else {
                continue;
            };
            if name.compressed {
                let plain = path.with_file_name(file_name.trim_end_matches(".zst"));
                if plain.is_file() {
                    continue;
                }
            }
            found.push(RolloutFile { path, name });
        }
    }
    found.sort_by(|a, b| {
        b.name
            .timestamp
            .cmp(&a.name.timestamp)
            .then_with(|| b.name.rollout_id.cmp(&a.name.rollout_id))
            .then_with(|| a.path.cmp(&b.path))
    });
    links.sort();
    Ok((found, links))
}

/// The first of `links`, as `scan` met them, through which a rollout of
/// `session` is reached: a link whose own name is a rollout of the
/// thread, or a link to a directory that holds one at any depth, the
/// links inside it followed in turn, each directory once. A link that
/// leads nowhere reaches nothing. Codex opens a rollout through such a
/// link, so capture and the resume guard refuse it rather than skip it.
pub(crate) fn link_reaching(
    links: &[PathBuf],
    session: &SessionId,
) -> Result<Option<PathBuf>, HarnessError> {
    let of_session = |name: &str| parse_name(name).is_some_and(|n| n.thread_id == *session);
    let mut seen = std::collections::HashSet::new();
    for link in links {
        let mut stack = vec![link.clone()];
        while let Some(path) = stack.pop() {
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(HarnessError::io(IoOp::Walk, &path, e)),
            };
            if meta.is_file() {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(of_session)
                {
                    return Ok(Some(link.clone()));
                }
                continue;
            }
            if !meta.is_dir() {
                continue;
            }
            let real =
                std::fs::canonicalize(&path).map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
            if !seen.insert(real) {
                continue;
            }
            let (files, inner) = scan(&path)?;
            if files.iter().any(|f| f.name.thread_id == *session) {
                return Ok(Some(link.clone()));
            }
            stack.extend(inner);
        }
    }
    Ok(None)
}

#[derive(Deserialize)]
struct FirstLine {
    #[serde(rename = "type")]
    kind: String,
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    history_base: Option<HistoryBase>,
}

#[derive(Deserialize)]
struct HistoryBase {
    thread_id: String,
}

/// What the first line of a rollout says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    /// `session_meta.cwd`.
    pub cwd: Option<String>,
    /// The rollout id of the prefix file, for a paginated suffix.
    pub history_base: Option<SessionId>,
}

fn open_lines(file: &RolloutFile) -> Result<Box<dyn BufRead>, HarnessError> {
    let handle =
        std::fs::File::open(&file.path).map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
    if file.name.compressed {
        let decoder = zstd::stream::read::Decoder::new(handle)
            .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
        Ok(Box::new(BufReader::new(decoder)))
    } else {
        Ok(Box::new(BufReader::new(handle)))
    }
}

/// Reads and parses the first line of a plain or compressed rollout.
pub fn read_meta(file: &RolloutFile) -> Result<Meta, HarnessError> {
    let mut line = String::new();
    open_lines(file)?
        .read_line(&mut line)
        .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?;
    let malformed = |reason: String| HarnessError::Malformed {
        what: Input::RolloutLine,
        path: file.path.clone(),
        reason,
    };
    let parsed: FirstLine =
        serde_json::from_str(line.trim_end()).map_err(|e| malformed(e.to_string()))?;
    if parsed.kind != "session_meta" {
        return Err(malformed(format!(
            "first line is {:?}, not session_meta",
            parsed.kind
        )));
    }
    let history_base = match parsed.payload.history_base {
        Some(base) => Some(
            SessionId::parse(&base.thread_id)
                .map_err(|e| malformed(format!("history_base.thread_id: {e}")))?,
        ),
        None => None,
    };
    Ok(Meta {
        cwd: parsed.payload.cwd,
        history_base,
    })
}

/// Refuses a rollout, plain or compressed, that holds a secret-shaped
/// string, raw or inside a decoded JSON string.
pub(crate) fn refuse_secrets(file: &RolloutFile) -> Result<(), HarnessError> {
    let redactor = Redactor::standard()?;
    let mut lines = open_lines(file)?;
    let mut line = Vec::new();
    let mut number = 0usize;
    loop {
        line.clear();
        if lines
            .read_until(b'\n', &mut line)
            .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?
            == 0
        {
            return Ok(());
        }
        number += 1;
        if jsontext::holds_secret(&line, redactor) {
            return Err(HarnessError::SecretInState {
                path: file.path.clone(),
                line: number,
            });
        }
    }
}

/// What a Codex session's rollouts record about how it ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecordedSettings {
    /// The model of the last turn, or of a change after it.
    pub(crate) model: Recorded<ModelName>,
    /// The provider that served it: the `model_provider` of the newest
    /// `session_meta`, or the `model_provider_id` of a change after it.
    pub(crate) provider: Recorded<ProviderId>,
    /// The reasoning effort of the last turn, or of a change after it.
    pub(crate) effort: Recorded<Effort>,
    /// The approval policy of the last turn, or of a change after it.
    pub(crate) approval: Recorded<ApprovalPolicy>,
    /// The approvals reviewer of the last turn, or of a change after it.
    pub(crate) reviewer: Recorded<ApprovalsReviewer>,
    /// The sandbox of the last turn, or of a change after it.
    pub(crate) sandbox: Recorded<SandboxMode>,
    /// The highest `cli_version` any `session_meta` line records: the
    /// newest Codex that created a file of the lineage.
    pub(crate) writer: Option<HarnessVersion>,
}

/// A JSON value as a report shows it: a string as itself, anything else as JSON.
fn shown(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// A recorded string setting: carried when `parse` takes it, else not
/// carried with `reason`; absent when missing or `null`.
fn setting<T>(
    value: Option<&serde_json::Value>,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Recorded<T> {
    match value {
        None | Some(serde_json::Value::Null) => Recorded::Absent,
        Some(serde_json::Value::String(text)) => match parse(text) {
            Ok(parsed) => Recorded::Carried(parsed),
            Err(reason) => Recorded::NotCarried {
                value: text.clone(),
                reason,
            },
        },
        Some(other) => Recorded::NotCarried {
            value: shown(other),
            reason: "the record does not hold it as text".to_owned(),
        },
    }
}

fn model(value: Option<&serde_json::Value>) -> Recorded<ModelName> {
    setting(value, |text| {
        ModelName::parse(text).map_err(|e| format!("not a model name Fabric passes: {e}"))
    })
}

fn provider(value: Option<&serde_json::Value>) -> Recorded<ProviderId> {
    setting(value, |text| {
        ProviderId::parse(text).map_err(|e| format!("not a provider id Fabric passes: {e}"))
    })
}

fn effort(value: Option<&serde_json::Value>) -> Recorded<Effort> {
    setting(value, |text| {
        Effort::from_record(text).ok_or_else(|| "not an effort codex 0.155.1 names".to_owned())
    })
}

fn approval(value: Option<&serde_json::Value>) -> Recorded<ApprovalPolicy> {
    setting(value, |text| {
        ApprovalPolicy::from_record(text).ok_or_else(|| {
            if text == "untrusted" {
                "codex 0.155.1 refuses to start with it set".to_owned()
            } else {
                "not an approval policy Fabric passes, which are on-request and never".to_owned()
            }
        })
    })
}

fn reviewer(value: Option<&serde_json::Value>) -> Recorded<ApprovalsReviewer> {
    setting(value, |text| {
        ApprovalsReviewer::from_record(text).ok_or_else(|| {
            "not an approvals reviewer Fabric passes, which are user and auto_review".to_owned()
        })
    })
}

/// Why full access is never carried.
const FULL_ACCESS: &str = "Fabric never passes full access, ADR 0006";

/// Why a permission profile set between turns is not carried.
const PROFILE_UNNAMED: &str =
    "set between turns as a permission profile Fabric cannot name as a sandbox mode";

fn sandbox(value: Option<&serde_json::Value>) -> Recorded<SandboxMode> {
    setting(value, |text| {
        SandboxMode::from_record(text).ok_or_else(|| {
            if text == "danger-full-access" {
                FULL_ACCESS.to_owned()
            } else {
                "not a sandbox Fabric passes, which are read-only and workspace-write".to_owned()
            }
        })
    })
}

/// Whether a managed permission profile with a restricted file system
/// lets Codex write anywhere; `None` for any other profile, such as one
/// without a sandbox (`disabled`) or with an unrestricted file system.
fn profile_writes(profile: &serde_json::Value) -> Option<bool> {
    let file_system = &profile["file_system"];
    if profile["type"] != "managed" || file_system["type"] != "restricted" {
        return None;
    }
    let entries = file_system["entries"].as_array()?;
    Some(entries.iter().any(|entry| entry["access"] == "write"))
}

/// Whether a permission profile lets Codex write the workspace `cwd`: the
/// most specific entry for `cwd` or a directory above it, or a
/// `project_roots` entry with no subpath, which stands for the workspace
/// itself, grants `write`. That is the test Codex 0.155.1 applies before
/// it names a profile `workspace-write` (`to_legacy_sandbox_policy` in
/// `codex-rs/protocol/src/permissions.rs`), widened to a directory above
/// the workspace, which the owner's rollouts record after a
/// workspace-write turn as well. A tie goes to the entry that does not
/// write, so a profile that writes only elsewhere, such as `/tmp`, or
/// keeps the workspace read-only under a writable parent, is not one.
fn profile_writes_workspace(profile: &serde_json::Value, cwd: Option<&str>) -> bool {
    let Some(cwd) = cwd.map(Path::new).filter(|cwd| cwd.is_absolute()) else {
        return false;
    };
    let Some(entries) = profile["file_system"]["entries"].as_array() else {
        return false;
    };
    let mut nearest: Option<(usize, bool)> = None;
    for entry in entries {
        let path = &entry["path"];
        let depth = match path["type"].as_str() {
            Some("path") => match path["path"].as_str().map(Path::new) {
                Some(dir) if dir.is_absolute() && cwd.starts_with(dir) => dir.components().count(),
                _ => continue,
            },
            Some("special")
                if path["value"]["kind"] == "project_roots"
                    && path["value"]["subpath"].is_null() =>
            {
                cwd.components().count()
            }
            _ => continue,
        };
        let writes = entry["access"] == "write";
        nearest = match nearest {
            Some((best, best_writes)) if best > depth || (best == depth && !best_writes) => {
                Some((best, best_writes))
            }
            _ => Some((depth, writes)),
        };
    }
    nearest.is_some_and(|(_, writes)| writes)
}

/// The last `turn_context` read so far: its sandbox, the permission
/// profile it recorded beside it, and its working directory.
#[derive(Default)]
struct LastTurn {
    sandbox: Recorded<SandboxMode>,
    profile: Option<serde_json::Value>,
    cwd: Option<String>,
}

impl RecordedSettings {
    /// Takes the settings of one `turn_context` payload, replacing all five.
    fn take_turn(&mut self, payload: &serde_json::Value, turn: &mut LastTurn) {
        self.model = model(payload.get("model"));
        self.effort = effort(payload.get("effort"));
        self.approval = approval(payload.get("approval_policy"));
        self.reviewer = reviewer(payload.get("approvals_reviewer"));
        let policy = payload.get("sandbox_policy");
        self.sandbox = sandbox(policy.and_then(|policy| policy.get("type")).or(policy));
        *turn = LastTurn {
            sandbox: self.sandbox.clone(),
            profile: payload.get("permission_profile").cloned(),
            cwd: payload["cwd"].as_str().map(str::to_owned),
        };
    }

    /// Takes the settings of one `thread_settings_applied` snapshot, the
    /// record of a change between turns. It holds a permission profile,
    /// not a sandbox mode: the sandbox is the last turn's when the profile
    /// is the one that turn recorded, or still writes the workspace (its
    /// `cwd`, else the last turn's) after a workspace-write turn;
    /// read-only when it writes nowhere, so a sandbox narrowed between
    /// turns is not widened back; otherwise not carried, a profile that
    /// writes only outside the workspace included, so it is never widened
    /// to the workspace. A snapshot with no profile leaves the sandbox as
    /// it was.
    fn take_snapshot(&mut self, settings: &serde_json::Value, turn: &LastTurn) {
        self.model = model(settings.get("model"));
        self.provider = provider(settings.get("model_provider_id"));
        self.effort = effort(settings.get("reasoning_effort"));
        self.approval = approval(settings.get("approval_policy"));
        self.reviewer = reviewer(settings.get("approvals_reviewer"));
        let profile = match settings.get("permission_profile") {
            None | Some(serde_json::Value::Null) => return,
            Some(profile) => profile,
        };
        let cwd = settings["cwd"].as_str().or(turn.cwd.as_deref());
        self.sandbox = if turn.profile.as_ref() == Some(profile) {
            turn.sandbox.clone()
        } else {
            match profile_writes(profile) {
                Some(false) => Recorded::Carried(SandboxMode::ReadOnly),
                Some(true)
                    if turn.sandbox == Recorded::Carried(SandboxMode::WorkspaceWrite)
                        && profile_writes_workspace(profile, cwd) =>
                {
                    Recorded::Carried(SandboxMode::WorkspaceWrite)
                }
                _ if profile["type"] == "disabled" => Recorded::NotCarried {
                    value: "danger-full-access".to_owned(),
                    reason: FULL_ACCESS.to_owned(),
                },
                _ => Recorded::NotCarried {
                    value: shown(&profile["type"]),
                    reason: PROFILE_UNNAMED.to_owned(),
                },
            }
        };
    }
}

/// Reads what the rollouts of a session record, given its lineage newest
/// suffix first, as `lineage` returns it. A line that does not parse is
/// skipped, as a cut last line is; a `cli_version` that is there but is
/// not a version, `null` and any other value that is not text included,
/// is an error, because the guard cannot compare it.
pub(crate) fn recorded(lineage: &[RolloutFile]) -> Result<RecordedSettings, HarnessError> {
    let mut settings = RecordedSettings::default();
    let mut turn = LastTurn::default();
    for file in lineage.iter().rev() {
        let mut lines = open_lines(file)?;
        let mut line = Vec::new();
        loop {
            line.clear();
            if lines
                .read_until(b'\n', &mut line)
                .map_err(|e| HarnessError::io(IoOp::Read, &file.path, e))?
                == 0
            {
                break;
            }
            if ![
                &b"\"turn_context\""[..],
                b"\"session_meta\"",
                b"\"thread_settings_applied\"",
            ]
            .iter()
            .any(|kind| contains(&line, kind))
            {
                continue;
            }
            let record: serde_json::Value = match serde_json::from_slice(&line) {
                Ok(record) => record,
                Err(e) => {
                    warn!(path = %file.path.display(), error = %e, "codex: rollout line skipped");
                    continue;
                }
            };
            let payload = &record["payload"];
            match record["type"].as_str() {
                Some("turn_context") => settings.take_turn(payload, &mut turn),
                Some("event_msg") if payload["type"] == "thread_settings_applied" => {
                    settings.take_snapshot(&payload["thread_settings"], &turn);
                }
                Some("session_meta") => {
                    // A meta that names no provider, from a Codex that did
                    // not record one, leaves the one read before.
                    match provider(payload.get("model_provider")) {
                        Recorded::Absent => {}
                        named => settings.provider = named,
                    }
                    let malformed = |reason: String| HarnessError::Malformed {
                        what: Input::RolloutLine,
                        path: file.path.clone(),
                        reason: format!("session_meta.cli_version: {reason}"),
                    };
                    let text = match payload.get("cli_version") {
                        None => continue,
                        Some(serde_json::Value::String(text)) => text,
                        Some(_) => return Err(malformed("not text".to_owned())),
                    };
                    let version =
                        HarnessVersion::parse(text).map_err(|e| malformed(e.to_string()))?;
                    if settings.writer.as_ref().is_none_or(|w| version > *w) {
                        settings.writer = Some(version);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(settings)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_observed_names() {
        let name = parse_name(
            "rollout-2026-07-02T17-32-21-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl.zst",
        )
        .unwrap();
        assert_eq!(name.timestamp, "2026-07-02T17-32-21");
        assert_eq!(
            name.thread_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368a"
        );
        assert_eq!(name.rollout_id, name.thread_id);
        assert!(name.compressed);
        let name = parse_name(
            "rollout-2026-10-19T10-00-00-019f233e-bb33-7342-85fa-f2d33b14368a_019f233e-bb33-7342-85fa-f2d33b14368b.jsonl",
        )
        .unwrap();
        assert_eq!(
            name.thread_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368a"
        );
        assert_eq!(
            name.rollout_id.as_str(),
            "019f233e-bb33-7342-85fa-f2d33b14368b"
        );
        assert!(!name.compressed);
        assert!(parse_name("rollout-2026-10-19T10-00-00-not-a-uuid.jsonl").is_none());
        assert!(parse_name("notes.jsonl").is_none());
        assert!(
            parse_name("rollout-2026-10-19-019f233e-bb33-7342-85fa-f2d33b14368a.jsonl").is_none()
        );
    }

    #[test]
    fn plain_wins_over_its_compressed_sibling_and_newest_sorts_first() {
        let dir = tempfile::tempdir().unwrap();
        let a = "rollout-2026-10-19T10-00-00-019f233e-bb33-7342-85fa-f2d33b14368a";
        let b = "rollout-2026-10-19T11-00-00-019f233e-bb33-7342-85fa-f2d33b14368b";
        std::fs::create_dir_all(dir.path().join("2026/10/19")).unwrap();
        std::fs::write(dir.path().join(format!("2026/10/19/{a}.jsonl")), "").unwrap();
        std::fs::write(dir.path().join(format!("2026/10/19/{a}.jsonl.zst")), "").unwrap();
        std::fs::write(dir.path().join(format!("{b}.jsonl.zst")), "").unwrap();
        let listed = list(dir.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name.timestamp, "2026-10-19T11-00-00");
        assert!(listed[0].name.compressed);
        assert!(!listed[1].name.compressed);
        assert!(listed[1].path.ends_with(format!("{a}.jsonl")));
    }

    const THREAD: &str = "019f233e-bb33-7342-85fa-f2d33b14368a";
    const SUFFIX: &str = "019f233e-bb33-7342-85fa-f2d33b14368b";

    /// Only a link that reaches a rollout of the thread counts, so a link
    /// to another thread's folder blocks nothing: a link that leads
    /// nowhere, or only to another thread's rollout, reaches nothing; one
    /// that reaches the thread through a link inside it is named by the
    /// outer link `scan` met; and a link back to a folder already walked
    /// ends the walk.
    #[test]
    fn only_a_link_that_reaches_the_thread_counts() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let other = dir.path().join("other");
        let inner = dir.path().join("inner");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(
            other.join(format!("rollout-2026-10-21T10-00-00-{SUFFIX}.jsonl")),
            "",
        )
        .unwrap();
        symlink(&other, root.join("2025")).unwrap();
        symlink(dir.path().join("gone"), root.join("2024")).unwrap();
        symlink(&root, other.join("back")).unwrap();
        let session = SessionId::parse(THREAD).unwrap();
        let (files, links) = scan(&root).unwrap();
        assert!(files.is_empty());
        assert_eq!(link_reaching(&links, &session).unwrap(), None);

        std::fs::write(
            inner.join(format!("rollout-2026-10-21T10-00-00-{THREAD}.jsonl")),
            "",
        )
        .unwrap();
        symlink(&inner, other.join("10")).unwrap();
        let (_, links) = scan(&root).unwrap();
        assert_eq!(
            link_reaching(&links, &session).unwrap(),
            Some(root.join("2025"))
        );
    }

    fn meta(version: &str, base: Option<&str>) -> String {
        let base = base.map_or_else(String::new, |b| {
            format!(",\"history_base\":{{\"thread_id\":\"{b}\"}}")
        });
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{THREAD}\",\"cwd\":\"/w\",\"cli_version\":\"{version}\"{base}}}}}\n"
        )
    }

    fn turn(model: &str, effort: &str, approval: &str, sandbox: &str) -> String {
        turn_reviewed(model, effort, approval, "user", sandbox)
    }

    fn turn_reviewed(
        model: &str,
        effort: &str,
        approval: &str,
        reviewer: &str,
        sandbox: &str,
    ) -> String {
        format!(
            "{{\"type\":\"turn_context\",\"payload\":{{\"model\":\"{model}\",\"effort\":\"{effort}\",\"approval_policy\":\"{approval}\",\"approvals_reviewer\":\"{reviewer}\",\"sandbox_policy\":{{\"type\":\"{sandbox}\",\"network_access\":false}}}}}}\n"
        )
    }

    /// The lineage of a thread whose prefix is `prefix` (plain) and whose
    /// newest suffix is `suffix` (compressed), as `lineage_in` follows it.
    fn lineage(dir: &Path, prefix: &str, suffix: &str) -> Vec<RolloutFile> {
        std::fs::write(
            dir.join(format!("rollout-2026-10-19T10-00-00-{THREAD}.jsonl")),
            prefix,
        )
        .unwrap();
        std::fs::write(
            dir.join(format!(
                "rollout-2026-10-21T10-00-00-{THREAD}_{SUFFIX}.jsonl.zst"
            )),
            zstd::encode_all(suffix.as_bytes(), 0).unwrap(),
        )
        .unwrap();
        let session = SessionId::parse(THREAD).unwrap();
        super::super::lineage_in(&list(dir).unwrap(), &session, dir).unwrap()
    }

    /// The settings are the newest turn's, read through a compressed
    /// suffix; the writer is the highest `cli_version` of any
    /// `session_meta` line in any file, a pre-release and a spawned
    /// thread's copy of its parent's line included.
    #[test]
    fn the_last_turn_and_the_newest_writer_across_the_lineage_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = format!(
            "{}{}{{\"type\":\"response_item\",\"payload\":{{}}}}\n{}",
            meta("0.163.0-alpha.10.2", None),
            turn("gpt-5.6-sol", "low", "never", "read-only"),
            "{\"type\":\"turn_context\",\"payl",
        );
        let suffix = format!(
            "{}{}{}{}",
            meta("0.155.1", Some(THREAD)),
            meta("0.142.4", None),
            turn_reviewed("gpt-6-astra", "high", "never", "user", "read-only"),
            turn_reviewed(
                "gpt-6-sol",
                "xhigh",
                "on-request",
                "auto_review",
                "workspace-write"
            ),
        );
        let chain = lineage(dir.path(), &prefix, &suffix);
        assert_eq!(chain.len(), 2);
        let read = recorded(&chain).unwrap();
        assert_eq!(
            read,
            RecordedSettings {
                model: Recorded::Carried(ModelName::parse("gpt-6-sol").unwrap()),
                provider: Recorded::Absent,
                effort: Recorded::Carried(Effort::XHigh),
                approval: Recorded::Carried(ApprovalPolicy::OnRequest),
                reviewer: Recorded::Carried(ApprovalsReviewer::AutoReview),
                sandbox: Recorded::Carried(SandboxMode::WorkspaceWrite),
                writer: Some(HarnessVersion::parse("0.163.0-alpha.10.2").unwrap()),
            }
        );
        assert_eq!(read.writer.unwrap().as_str(), "0.163.0-alpha.10.2");
    }

    /// A permission profile as Codex 0.155.1 records one: managed, with
    /// a read entry for the root and a write entry for each of `writes`.
    fn profile(writes: &[&str]) -> String {
        let mut entries = vec![
            "{\"access\":\"read\",\"path\":{\"type\":\"special\",\"value\":{\"kind\":\"root\"}}}"
                .to_owned(),
        ];
        entries.extend(writes.iter().map(|path| {
            format!("{{\"access\":\"write\",\"path\":{{\"type\":\"path\",\"path\":\"{path}\"}}}}")
        }));
        format!(
            "{{\"type\":\"managed\",\"file_system\":{{\"type\":\"restricted\",\"entries\":[{}]}},\"network\":\"restricted\"}}",
            entries.join(",")
        )
    }

    /// A `turn_context` that records `permission_profile` beside its sandbox.
    fn turn_in(model: &str, effort: &str, sandbox: &str, profile: &str) -> String {
        format!(
            "{{\"type\":\"turn_context\",\"payload\":{{\"model\":\"{model}\",\"effort\":\"{effort}\",\"approval_policy\":\"on-request\",\"approvals_reviewer\":\"user\",\"sandbox_policy\":{{\"type\":\"{sandbox}\"}},\"permission_profile\":{profile}}}}}\n"
        )
    }

    /// The `event_msg` Codex writes when a setting changes between turns,
    /// such as `/model` or `/permissions` in the terminal.
    fn applied(model: &str, effort: &str, approval: &str, reviewer: &str, profile: &str) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"thread_settings_applied\",\"thread_id\":\"{THREAD}\",\"thread_settings\":{{\"model\":\"{model}\",\"model_provider_id\":\"openai\",\"approval_policy\":\"{approval}\",\"approvals_reviewer\":\"{reviewer}\",\"permission_profile\":{profile},\"cwd\":\"/w\",\"reasoning_effort\":\"{effort}\"}}}}}}\n"
        )
    }

    fn read_suffix(records: &[String]) -> RecordedSettings {
        let dir = tempfile::tempdir().unwrap();
        let suffix = format!("{}{}", meta("0.155.1", Some(THREAD)), records.concat());
        recorded(&lineage(dir.path(), &meta("0.155.1", None), &suffix)).unwrap()
    }

    /// The provider that served the model is the newest `session_meta`'s
    /// `model_provider`, through a compressed suffix whose meta, from a
    /// Codex that recorded none, names none; a settings change after it,
    /// which Codex 0.155.1 also writes on every resume, names the one
    /// that now serves the model, and a turn, which names no provider,
    /// leaves it.
    #[test]
    fn the_provider_is_the_newest_meta_s_or_a_later_change_s() {
        let on = |provider: &str| {
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{THREAD}\",\"cwd\":\"/w\",\"cli_version\":\"0.155.1\",\"model_provider\":\"{provider}\"}}}}\n"
            )
        };
        let shown = |read: RecordedSettings| match read.provider {
            Recorded::Carried(id) => id.as_str().to_owned(),
            other => format!("{other:?}"),
        };
        let dir = tempfile::tempdir().unwrap();
        let prefix = format!(
            "{}{}",
            on("azure"),
            turn("gpt-5-deploy", "high", "on-request", "read-only")
        );
        let suffix = format!(
            "{}{}",
            meta("0.155.1", Some(THREAD)),
            turn("gpt-5-deploy", "high", "on-request", "read-only")
        );
        let read = recorded(&lineage(dir.path(), &prefix, &suffix)).unwrap();
        assert_eq!(shown(read), "azure");

        let ww = profile(&["/w"]);
        let read = read_suffix(&[
            on("azure"),
            turn_in("gpt-5-deploy", "high", "workspace-write", &ww),
            applied("gpt-6-sol", "xhigh", "on-request", "user", &ww),
            turn_in("gpt-6-sol", "xhigh", "workspace-write", &ww),
        ]);
        assert_eq!(shown(read), "openai");
    }

    /// A setting changed between turns, which Codex records as
    /// `thread_settings_applied` and itself restores on resume when it is
    /// the newer record, wins over the last turn; a turn after it wins
    /// again. Seen on the owner's Mac: a last turn on gpt-5.5 at xhigh,
    /// then `/model` to gpt-6-astra at medium.
    #[test]
    fn the_newer_of_the_last_turn_and_a_settings_change_is_read() {
        let ww = profile(&["/w", "/w/.git", "/tmp"]);
        let read = read_suffix(&[
            turn_in("gpt-5.5", "xhigh", "workspace-write", &ww),
            applied(
                "gpt-6-astra",
                "medium",
                "never",
                "auto_review",
                &profile(&["/w"]),
            ),
        ]);
        assert_eq!(
            (
                read.model,
                read.effort,
                read.approval,
                read.reviewer,
                read.sandbox
            ),
            (
                Recorded::Carried(ModelName::parse("gpt-6-astra").unwrap()),
                Recorded::Carried(Effort::Medium),
                Recorded::Carried(ApprovalPolicy::Never),
                Recorded::Carried(ApprovalsReviewer::AutoReview),
                Recorded::Carried(SandboxMode::WorkspaceWrite),
            )
        );
        let read = read_suffix(&[
            applied("gpt-6-astra", "medium", "never", "auto_review", &ww),
            turn_in("gpt-5.5", "xhigh", "read-only", &profile(&[])),
        ]);
        assert_eq!(
            (read.model, read.effort, read.reviewer, read.sandbox),
            (
                Recorded::Carried(ModelName::parse("gpt-5.5").unwrap()),
                Recorded::Carried(Effort::XHigh),
                Recorded::Carried(ApprovalsReviewer::User),
                Recorded::Carried(SandboxMode::ReadOnly),
            )
        );
    }

    /// A settings change records a permission profile, not a sandbox
    /// mode. One that writes nowhere is read-only, so a sandbox narrowed
    /// between turns is not widened back; one equal to the last turn's,
    /// or that still writes after a workspace-write turn, keeps the turn's
    /// mode; full access and any other profile are not carried.
    #[test]
    fn a_permission_profile_set_between_turns_gives_the_sandbox_it_names() {
        let ww = profile(&["/w", "/tmp"]);
        let after = |sandbox: &str, turn_profile: &str, set: &str| {
            read_suffix(&[
                turn_in("gpt-6-sol", "high", sandbox, turn_profile),
                applied("gpt-6-sol", "high", "on-request", "user", set),
            ])
            .sandbox
        };
        assert_eq!(
            after("workspace-write", &ww, &profile(&[])),
            Recorded::Carried(SandboxMode::ReadOnly)
        );
        assert_eq!(
            after("workspace-write", &ww, &profile(&["/w"])),
            Recorded::Carried(SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            after("read-only", &ww, &ww),
            Recorded::Carried(SandboxMode::ReadOnly)
        );
        assert_eq!(
            reason(&after("read-only", &profile(&[]), &ww)),
            "managed: set between turns as a permission profile Fabric cannot name as a sandbox mode"
        );
        assert_eq!(
            reason(&after("workspace-write", &ww, "{\"type\":\"disabled\"}")),
            "danger-full-access: Fabric never passes full access, ADR 0006"
        );
        let unrestricted = "{\"type\":\"managed\",\"file_system\":{\"type\":\"unrestricted\"},\"network\":\"enabled\"}";
        assert_eq!(
            reason(&after("workspace-write", &ww, unrestricted)),
            "managed: set between turns as a permission profile Fabric cannot name as a sandbox mode"
        );
    }

    /// A profile set after a workspace-write turn keeps that mode only
    /// when it writes the workspace: an entry for its `cwd`, or for a
    /// directory above it with nothing nearer that keeps it read-only, or
    /// `project_roots`. One that still writes, but only elsewhere, is not
    /// carried, so `/tmp` alone never becomes workspace writes.
    #[test]
    fn a_profile_that_does_not_write_the_workspace_is_not_widened_to_it() {
        let ww = profile(&["/w/repo", "/tmp"]);
        let after = |cwd: &str, set: &str| {
            let set = applied("gpt-6-sol", "high", "on-request", "user", set)
                .replace("\"cwd\":\"/w\"", &format!("\"cwd\":\"{cwd}\""));
            read_suffix(&[turn_in("gpt-6-sol", "high", "workspace-write", &ww), set]).sandbox
        };
        let entry =
            |access: &str, path: &str| format!("{{\"access\":\"{access}\",\"path\":{path}}}");
        let entries = |entries: &[String]| {
            format!(
                "{{\"type\":\"managed\",\"file_system\":{{\"type\":\"restricted\",\"entries\":[{}]}},\"network\":\"restricted\"}}",
                entries.join(",")
            )
        };
        let unnamed = "managed: set between turns as a permission profile Fabric cannot name as a sandbox mode";
        let ww_carried = Recorded::Carried(SandboxMode::WorkspaceWrite);

        assert_eq!(reason(&after("/w/repo", &profile(&["/tmp"]))), unnamed);
        assert_eq!(reason(&after("/w/repo", &profile(&["/w/other"]))), unnamed);
        assert_eq!(after("/w/repo", &profile(&["/w/repo", "/tmp"])), ww_carried);
        assert_eq!(after("/w/repo", &profile(&["/w"])), ww_carried);
        let read_only_under_a_writable_parent = entries(&[
            entry("write", "{\"type\":\"path\",\"path\":\"/w\"}"),
            entry("read", "{\"type\":\"path\",\"path\":\"/w/repo\"}"),
        ]);
        assert_eq!(
            reason(&after("/w/repo", &read_only_under_a_writable_parent)),
            unnamed
        );
        // Two entries for the workspace itself: the one that does not
        // write wins, in either order.
        let write = entry("write", "{\"type\":\"path\",\"path\":\"/w/repo\"}");
        let deny = entry("deny", "{\"type\":\"path\",\"path\":\"/w/repo\"}");
        for pair in [[write.clone(), deny.clone()], [deny, write]] {
            assert_eq!(reason(&after("/w/repo", &entries(&pair))), unnamed);
        }
        let project_roots = |subpath: &str| {
            entries(&[entry(
                "write",
                &format!(
                    "{{\"type\":\"special\",\"value\":{{\"kind\":\"project_roots\"{subpath}}}}}"
                ),
            )])
        };
        assert_eq!(after("/w/repo", &project_roots("")), ww_carried);
        assert_eq!(
            reason(&after("/w/repo", &project_roots(",\"subpath\":\"docs\""))),
            unnamed
        );
        // With no `cwd` in the snapshot, the last turn's says where the
        // workspace is.
        let set = applied(
            "gpt-6-sol",
            "high",
            "on-request",
            "user",
            &profile(&["/w/repo"]),
        )
        .replace(",\"cwd\":\"/w\"", "");
        let turn = turn_in("gpt-6-sol", "high", "workspace-write", &ww).replace(
            "\"sandbox_policy\"",
            "\"cwd\":\"/w/repo\",\"sandbox_policy\"",
        );
        assert_eq!(
            read_suffix(&[turn.clone(), set.clone()]).sandbox,
            ww_carried
        );
        let elsewhere = turn.replace("\"cwd\":\"/w/repo\"", "\"cwd\":\"/v\"");
        assert_eq!(reason(&read_suffix(&[elsewhere, set]).sandbox), unnamed);
    }

    fn reason<T>(recorded: &Recorded<T>) -> String {
        match recorded {
            Recorded::NotCarried { value, reason } => format!("{value}: {reason}"),
            Recorded::Carried(_) => "carried".to_owned(),
            Recorded::Absent => "absent".to_owned(),
        }
    }

    /// A value outside the closed sets is reported with its reason, never
    /// carried, and a `cli_version` that is not a version fails the read.
    #[test]
    fn values_outside_the_closed_sets_are_not_carried_and_a_bad_writer_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let suffix = format!(
            "{}{}",
            meta("0.155.1", Some(THREAD)),
            turn_reviewed(
                "gpt 6",
                "turbo",
                "on-failure",
                "bogus",
                "danger-full-access"
            ),
        );
        let chain = lineage(dir.path(), &meta("0.155.1", None), &suffix);
        let read = recorded(&chain).unwrap();
        assert_eq!(
            reason(&read.model),
            "gpt 6: not a model name Fabric passes: model name has an invalid character at index 3"
        );
        assert_eq!(
            reason(&read.effort),
            "turbo: not an effort codex 0.155.1 names"
        );
        assert_eq!(
            reason(&read.approval),
            "on-failure: not an approval policy Fabric passes, which are on-request and never"
        );
        assert_eq!(
            reason(&read.reviewer),
            "bogus: not an approvals reviewer Fabric passes, which are user and auto_review"
        );
        assert_eq!(
            reason(&read.sandbox),
            "danger-full-access: Fabric never passes full access, ADR 0006"
        );

        // A project Codex marks untrusted records `untrusted`, which Codex
        // 0.155.1 refuses to start with when it is set.
        let dir = tempfile::tempdir().unwrap();
        let suffix = format!(
            "{}{}",
            meta("0.155.1", Some(THREAD)),
            turn("gpt-6-sol", "high", "untrusted", "workspace-write"),
        );
        let chain = lineage(dir.path(), &meta("0.155.1", None), &suffix);
        assert_eq!(
            reason(&recorded(&chain).unwrap().approval),
            "untrusted: codex 0.155.1 refuses to start with it set"
        );

        let dir = tempfile::tempdir().unwrap();
        let chain = lineage(
            dir.path(),
            &meta("0.155.1", None),
            &meta("not-a-version", Some(THREAD)),
        );
        assert!(matches!(
            recorded(&chain),
            Err(HarnessError::Malformed {
                what: Input::RolloutLine,
                ..
            })
        ));
    }

    /// A `cli_version` that is there but is not text fails the read as a
    /// bad version does: skipped, it would leave the guard without the
    /// newest writer. Only a `session_meta` with no `cli_version` is
    /// read past.
    #[test]
    fn a_writer_recorded_as_anything_but_text_is_refused() {
        let quoted = meta("0.160.0", Some(THREAD));
        for value in ["160", "null", "[\"0.160.0\"]", "{\"v\":\"0.160.0\"}"] {
            let dir = tempfile::tempdir().unwrap();
            let suffix = quoted.replace("\"0.160.0\"", value);
            assert_ne!(suffix, quoted);
            let chain = lineage(dir.path(), &meta("0.155.1", None), &suffix);
            assert!(
                matches!(
                    recorded(&chain),
                    Err(HarnessError::Malformed {
                        what: Input::RolloutLine,
                        ref reason,
                        ..
                    }) if reason == "session_meta.cli_version: not text"
                ),
                "{value}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let chain = lineage(
            dir.path(),
            &meta("0.155.1", None),
            &quoted.replace(",\"cli_version\":\"0.160.0\"", ""),
        );
        assert_eq!(
            recorded(&chain).unwrap().writer.map(|w| w.to_string()),
            Some("0.155.1".to_owned())
        );
    }
}
