//! Codex rollout files: names, lookup and lineage.
//!
//! Invariant: a rollout is found by its file name alone,
//! `rollout-<YYYY-MM-DDTHH-MM-SS>-<thread_id>[_<rollout_id>].jsonl[.zst]`,
//! anywhere under `sessions/` or `archived_sessions/`, which is how Codex
//! itself finds it when its `SQLite` row is missing (`codex-rs/rollout/src/
//! list.rs`, `visit_rollout_filenames`, at `e4568a3`, 2026-10-19). A
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
//! starts with `{"type":"session_meta","payload":{"cli_version",...}}`. A
//! thread a parent spawned holds the parent's `session_meta` as a second
//! line, and a desktop build writes a pre-release such as
//! `0.163.0-alpha.10.2`. The settings are the last `turn_context`'s, in
//! lineage order; the writer is the highest `cli_version` of any
//! `session_meta` line in any file of the lineage. That is the newest
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

use super::overrides::{ApprovalPolicy, ApprovalsReviewer, Effort, SandboxMode};

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
/// a name, the plain one is listed, as Codex does.
pub fn list(root: &Path) -> Result<Vec<RolloutFile>, HarnessError> {
    let mut found: Vec<RolloutFile> = Vec::new();
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
    Ok(found)
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
    /// The model of the last turn.
    pub(crate) model: Recorded<ModelName>,
    /// The reasoning effort of the last turn.
    pub(crate) effort: Recorded<Effort>,
    /// The approval policy of the last turn.
    pub(crate) approval: Recorded<ApprovalPolicy>,
    /// The approvals reviewer of the last turn.
    pub(crate) reviewer: Recorded<ApprovalsReviewer>,
    /// The sandbox of the last turn.
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

impl RecordedSettings {
    /// Takes the settings of one `turn_context` payload, replacing all five.
    fn take_turn(&mut self, payload: &serde_json::Value) {
        self.model = setting(payload.get("model"), |text| {
            ModelName::parse(text).map_err(|e| format!("not a model name Fabric passes: {e}"))
        });
        self.effort = setting(payload.get("effort"), |text| {
            Effort::from_record(text).ok_or_else(|| "not an effort codex 0.155.1 names".to_owned())
        });
        self.approval = setting(payload.get("approval_policy"), |text| {
            ApprovalPolicy::from_record(text).ok_or_else(|| {
                if text == "untrusted" {
                    "codex 0.155.1 refuses to start with it set".to_owned()
                } else {
                    "not an approval policy Fabric passes, which are on-request and never"
                        .to_owned()
                }
            })
        });
        self.reviewer = setting(payload.get("approvals_reviewer"), |text| {
            ApprovalsReviewer::from_record(text).ok_or_else(|| {
                "not an approvals reviewer Fabric passes, which are user and auto_review".to_owned()
            })
        });
        let sandbox = payload.get("sandbox_policy");
        let mode = sandbox.and_then(|policy| policy.get("type")).or(sandbox);
        self.sandbox = setting(mode, |text| {
            SandboxMode::from_record(text).ok_or_else(|| {
                if text == "danger-full-access" {
                    "Fabric never passes full access, ADR 0006".to_owned()
                } else {
                    "not a sandbox Fabric passes, which are read-only and workspace-write"
                        .to_owned()
                }
            })
        });
    }
}

/// Reads what the rollouts of a session record, given its lineage newest
/// suffix first, as `lineage` returns it. A line that does not parse is
/// skipped, as a cut last line is; a `cli_version` that is there but is
/// not a version is an error, because the guard cannot compare it.
pub(crate) fn recorded(lineage: &[RolloutFile]) -> Result<RecordedSettings, HarnessError> {
    let mut settings = RecordedSettings::default();
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
            let turn = contains(&line, b"\"turn_context\"");
            if !turn && !contains(&line, b"\"session_meta\"") {
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
                Some("turn_context") => settings.take_turn(payload),
                Some("session_meta") => {
                    let Some(text) = payload.get("cli_version").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let version =
                        HarnessVersion::parse(text).map_err(|e| HarnessError::Malformed {
                            what: Input::RolloutLine,
                            path: file.path.clone(),
                            reason: format!("session_meta.cli_version: {e}"),
                        })?;
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
                effort: Recorded::Carried(Effort::XHigh),
                approval: Recorded::Carried(ApprovalPolicy::OnRequest),
                reviewer: Recorded::Carried(ApprovalsReviewer::AutoReview),
                sandbox: Recorded::Carried(SandboxMode::WorkspaceWrite),
                writer: Some(HarnessVersion::parse("0.163.0-alpha.10.2").unwrap()),
            }
        );
        assert_eq!(read.writer.unwrap().as_str(), "0.163.0-alpha.10.2");
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
}
