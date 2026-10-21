//! What a Claude session's transcripts record about how it ran.
//!
//! Invariant: a resume continues with the model the session last ran
//! with and is refused by a Claude older than the newest one that wrote
//! it, both read from the transcripts themselves, never from a second
//! copy in the job record (gate finding F10). Record shape, read from the
//! owner's transcripts on 2026-10-21 (key names and setting values only):
//! every line of a turn carries `version` (`2.1.294`), and an `assistant`
//! line carries `message.model`, the model's full name
//! (`claude-opus-5-5`). A line Claude writes itself, such as an API error
//! shown as a message, carries `<synthetic>`, which names no model and is
//! skipped, as are the lines of a side chain. The model is the last one
//! of the transcript under the workspace's project key, the one `claude
//! --resume` reads from the workspace, or of the other keys' transcripts
//! when that key holds none; the writer is the highest `version` of any
//! line of any of them. A line that does not parse is skipped, as a cut
//! last line is; a `version` that is there and is not a version is an
//! error, because the guard cannot compare it.
//!
//! `message.model` names the model that served a reply, which is not
//! always the one the session asked for. When Claude falls back, it first
//! writes a `system` line, subtype `model_fallback` (overload or usage
//! limit) or `model_refusal_fallback`, with `originalModel` and
//! `fallbackModel` (2.1.294 bundle; 126 refusal fallbacks in the owner's
//! transcripts on 2026-10-21, after each of which later replies went back
//! to the original). A reply the fallback served therefore counts as the
//! original model, so a resume does not pin the fallback for good. An
//! alias such as `opusplan` is recorded nowhere, so a session run under
//! one resumes on the full model of its last reply.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::debug;

use crate::error::{HarnessError, Input, IoOp};
use crate::harness::{ModelName, Recorded};
use crate::version::HarnessVersion;

/// What a Claude session's transcripts record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecordedClaude {
    /// The model of the last assistant line.
    pub(crate) model: Recorded<ModelName>,
    /// The highest `version` any line records.
    pub(crate) writer: Option<HarnessVersion>,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    #[serde(default)]
    message: Option<Message>,
    #[serde(rename = "originalModel", default)]
    original_model: Option<String>,
    #[serde(rename = "fallbackModel", default)]
    fallback_model: Option<String>,
}

/// A fallback Claude recorded: replies served by `to` stand for `from`.
struct Fallback {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    model: Option<String>,
}

/// True for a value Claude writes in place of a model, such as `<synthetic>`.
fn placeholder(model: &str) -> bool {
    model.starts_with('<') && model.ends_with('>')
}

/// Reads `transcripts` in order, the last one's model winning.
pub(crate) fn read(transcripts: &[PathBuf]) -> Result<RecordedClaude, HarnessError> {
    let mut recorded = RecordedClaude::default();
    for path in transcripts {
        read_one(path, &mut recorded)?;
    }
    Ok(recorded)
}

fn read_one(path: &Path, recorded: &mut RecordedClaude) -> Result<(), HarnessError> {
    let file = File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut fallback: Option<Fallback> = None;
    loop {
        bytes.clear();
        if reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| HarnessError::io(IoOp::Read, path, e))?
            == 0
        {
            return Ok(());
        }
        let line: Line = match serde_json::from_slice(&bytes) {
            Ok(line) => line,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "claude: transcript line skipped");
                continue;
            }
        };
        if let Some(text) = &line.version {
            let version = HarnessVersion::parse(text).map_err(|e| HarnessError::Malformed {
                what: Input::TranscriptLine,
                path: path.to_path_buf(),
                reason: format!("version: {e}"),
            })?;
            if recorded.writer.as_ref().is_none_or(|w| version > *w) {
                recorded.writer = Some(version);
            }
        }
        if line.sidechain {
            continue;
        }
        if line.kind.as_deref() == Some("system")
            && matches!(
                line.subtype.as_deref(),
                Some("model_fallback" | "model_refusal_fallback")
            )
            && let (Some(from), Some(to)) = (line.original_model, line.fallback_model)
        {
            fallback = Some(Fallback { from, to });
            continue;
        }
        if line.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(served) = line.message.and_then(|m| m.model) else {
            continue;
        };
        if placeholder(&served) {
            continue;
        }
        let model = match &fallback {
            Some(f) if f.to == served => f.from.clone(),
            _ => {
                fallback = None;
                served
            }
        };
        recorded.model = match ModelName::parse(&model) {
            Ok(name) => Recorded::Carried(name),
            Err(e) => Recorded::NotCarried {
                value: model,
                reason: format!("not a model name Fabric passes: {e}"),
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(version: &str, model: &str, sidechain: bool) -> String {
        format!(
            "{{\"type\":\"assistant\",\"version\":\"{version}\",\"isSidechain\":{sidechain},\"message\":{{\"model\":\"{model}\",\"role\":\"assistant\"}}}}\n"
        )
    }

    /// The model is the last real main-chain one of the last transcript
    /// read, skipping `<synthetic>` and side chains, and the one asked for
    /// when a fallback served the reply; the writer is the highest version
    /// of any line of any transcript.
    #[test]
    fn the_last_real_model_and_the_newest_writer_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.jsonl");
        let own = dir.path().join("own.jsonl");
        std::fs::write(
            &other,
            format!(
                "{}{}",
                assistant("2.1.300", "claude-fable-5", false),
                "{\"type\":\"user\",\"version\":\"2.1.300\",\"message\":\"cut"
            ),
        )
        .unwrap();
        std::fs::write(
            &own,
            format!(
                "{}{}{}{}",
                "{\"type\":\"user\",\"version\":\"2.1.199\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
                assistant("2.1.294", "claude-opus-5-5", false),
                assistant("2.1.294", "claude-haiku-4-5", true),
                assistant("2.1.294", "<synthetic>", false),
            ),
        )
        .unwrap();
        let read = read(&[other.clone(), own.clone()]).unwrap();
        assert_eq!(
            read.model,
            Recorded::Carried(ModelName::parse("claude-opus-5-5").unwrap())
        );
        assert_eq!(read.writer.unwrap().as_str(), "2.1.300");

        // A reply a fallback served stands for the model the session asked
        // for, until a reply comes from another model.
        let fallback = "{\"type\":\"system\",\"subtype\":\"model_refusal_fallback\",\"version\":\"2.1.294\",\"isSidechain\":false,\"originalModel\":\"claude-fable-5\",\"fallbackModel\":\"claude-opus-4-8\"}\n";
        let served = [
            assistant("2.1.294", "claude-fable-5", false),
            fallback.to_owned(),
            assistant("2.1.294", "claude-opus-4-8", false),
            assistant("2.1.294", "claude-opus-4-8", false),
        ]
        .concat();
        std::fs::write(&own, &served).unwrap();
        assert_eq!(
            super::read(std::slice::from_ref(&own)).unwrap().model,
            Recorded::Carried(ModelName::parse("claude-fable-5").unwrap())
        );
        let switched = format!("{served}{}", assistant("2.1.294", "claude-opus-5", false));
        std::fs::write(&own, switched).unwrap();
        assert_eq!(
            super::read(std::slice::from_ref(&own)).unwrap().model,
            Recorded::Carried(ModelName::parse("claude-opus-5").unwrap())
        );

        std::fs::write(&own, assistant("2.1.x", "claude-opus-5-5", false)).unwrap();
        assert!(matches!(
            super::read(&[own]),
            Err(HarnessError::Malformed {
                what: Input::TranscriptLine,
                ..
            })
        ));
    }
}
