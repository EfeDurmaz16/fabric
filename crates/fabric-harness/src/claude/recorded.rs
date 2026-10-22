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
//! line of any of them. A line that is not JSON is skipped, as a cut last
//! line is; a `version` that is there and is not a version, `null` and
//! any other value that is not text included, is an error, because the
//! guard cannot compare it. The version is read before the rest of the
//! line, so a line whose other fields have an unexpected shape still
//! counts toward the writer.
//!
//! `message.model` names the model that served a reply, which is not
//! always the one the session asked for. When Claude falls back, it writes
//! a `system` line, subtype `model_fallback` (overload or usage limit) or
//! `model_refusal_fallback`, with `originalModel` and `fallbackModel`
//! (2.1.294 bundle), and that line can come before or after the reply the
//! fallback served. Of the 126 refusal fallbacks in the owner's
//! transcripts on 2026-10-21 (written by 2.1.197 to 2.1.278), 83 name as
//! their parent an earlier reply the fallback model served, 27 of which
//! ended the turn, and in 118 the next reply came from the fallback model
//! too: Claude stays on it for the rest of the process and goes back to
//! the original on a session transition (`unlatchRefusalFallbackModel`,
//! 2.1.294 bundle), which a resume is. So a reply the fallback model
//! served counts as the original model, whether it comes right before the
//! fallback line or after it until a reply comes from another model, and
//! a resume does not pin the fallback for good. An alias such as
//! `opusplan` is recorded nowhere, so a session run under one resumes on
//! the full model of its last reply.

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
    /// The model each transcript ends on, as recorded, in the order read
    /// and each once; what a resume can name when it cannot tell which
    /// transcript Claude continues.
    pub(crate) models: Vec<String>,
    /// True when some transcript records no model, so the one Claude
    /// continues may end on none.
    pub(crate) without_model: bool,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
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

/// Reads `transcripts` in order, the model of the last one that records
/// one winning.
pub(crate) fn read(transcripts: &[PathBuf]) -> Result<RecordedClaude, HarnessError> {
    let mut recorded = RecordedClaude::default();
    for path in transcripts {
        let mut one = RecordedClaude::default();
        read_one(path, &mut one)?;
        recorded.writer = recorded.writer.max(one.writer);
        let ends_on = match &one.model {
            Recorded::Carried(model) => model.as_str(),
            Recorded::NotCarried { value, .. } => value.as_str(),
            Recorded::Absent => {
                recorded.without_model = true;
                continue;
            }
        };
        if !recorded.models.iter().any(|seen| seen == ends_on) {
            recorded.models.push(ends_on.to_owned());
        }
        recorded.model = one.model;
    }
    Ok(recorded)
}

/// The model a resume asks for in place of `model`, or why it cannot.
fn recorded_model(model: String) -> Recorded<ModelName> {
    match ModelName::parse(&model) {
        Ok(name) => Recorded::Carried(name),
        Err(e) => Recorded::NotCarried {
            value: model,
            reason: format!("not a model name Fabric passes: {e}"),
        },
    }
}

fn read_one(path: &Path, recorded: &mut RecordedClaude) -> Result<(), HarnessError> {
    let file = File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut fallback: Option<Fallback> = None;
    // The model that served the last reply of this file.
    let mut last_served: Option<String> = None;
    loop {
        bytes.clear();
        if reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| HarnessError::io(IoOp::Read, path, e))?
            == 0
        {
            return Ok(());
        }
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "claude: transcript line skipped");
                continue;
            }
        };
        if let Some(version) = value.get("version") {
            let malformed = |reason: String| HarnessError::Malformed {
                what: Input::TranscriptLine,
                path: path.to_path_buf(),
                reason: format!("version: {reason}"),
            };
            let text = version
                .as_str()
                .ok_or_else(|| malformed("not text".to_owned()))?;
            let version = HarnessVersion::parse(text).map_err(|e| malformed(e.to_string()))?;
            if recorded.writer.as_ref().is_none_or(|w| version > *w) {
                recorded.writer = Some(version);
            }
        }
        let line = match Line::deserialize(&value) {
            Ok(line) => line,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "claude: transcript line skipped");
                continue;
            }
        };
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
            // The line often follows the reply the fallback served.
            if last_served.as_deref() == Some(to.as_str()) {
                recorded.model = recorded_model(from.clone());
            }
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
                served.clone()
            }
        };
        last_served = Some(served);
        recorded.model = recorded_model(model);
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

    /// A refusal fallback from claude-fable-5 to claude-opus-4-8, as
    /// Claude 2.1.294 writes it.
    const FALLBACK: &str = "{\"type\":\"system\",\"subtype\":\"model_refusal_fallback\",\"version\":\"2.1.294\",\"isSidechain\":false,\"originalModel\":\"claude-fable-5\",\"fallbackModel\":\"claude-opus-4-8\"}\n";

    /// The model `lines`, as one transcript, gives a resume.
    fn model_of(lines: &[String]) -> Recorded<ModelName> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("own.jsonl");
        std::fs::write(&path, lines.concat()).unwrap();
        read(&[path]).unwrap().model
    }

    fn model(name: &str) -> Recorded<ModelName> {
        Recorded::Carried(ModelName::parse(name).unwrap())
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
        let served = [
            assistant("2.1.294", "claude-fable-5", false),
            FALLBACK.to_owned(),
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

    /// Claude often writes the fallback line after the reply the fallback
    /// served, and a turn can end on that reply (83 of the 126 real refusal
    /// fallbacks, 27 of them at the end of a turn): that reply still stands
    /// for the model the session asked for, and so do the replies the
    /// fallback model serves after the line.
    #[test]
    fn a_fallback_line_after_the_reply_it_served_still_stands_for_the_original() {
        let ended = [
            assistant("2.1.294", "claude-fable-5", false),
            assistant("2.1.294", "claude-opus-4-8", false),
            FALLBACK.to_owned(),
        ];
        assert_eq!(model_of(&ended), model("claude-fable-5"));
        let latched = [
            ended.as_slice(),
            &[assistant("2.1.294", "claude-opus-4-8", false)],
        ]
        .concat();
        assert_eq!(model_of(&latched), model("claude-fable-5"));
    }

    /// A `version` that is there but is not text fails the read as a bad
    /// version does, and a line whose other fields have an unexpected
    /// shape still names its writer: skipping either would leave the guard
    /// without the newest writer. A line with no `version` is read past.
    #[test]
    fn a_writer_is_never_skipped_with_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("own.jsonl");
        let writer_of = |lines: &[String]| {
            std::fs::write(&path, lines.concat()).unwrap();
            read(std::slice::from_ref(&path))
        };
        let newer = assistant("2.1.300", "claude-opus-5-5", false);
        for value in ["2100", "null", "[\"2.1.300\"]", "{\"v\":\"2.1.300\"}"] {
            let line = newer.replace("\"2.1.300\"", value);
            assert_ne!(line, newer);
            let read = writer_of(&[assistant("2.1.294", "claude-opus-5-5", false), line]);
            assert!(
                matches!(
                    read,
                    Err(HarnessError::Malformed {
                        what: Input::TranscriptLine,
                        ref reason,
                        ..
                    }) if reason == "version: not text"
                ),
                "{value}: {read:?}"
            );
        }
        let odd_shape = newer.replace("\"isSidechain\":false", "\"isSidechain\":\"no\"");
        assert_ne!(odd_shape, newer);
        let read = writer_of(&[
            assistant("2.1.294", "claude-opus-5-5", false),
            odd_shape,
            "{\"type\":\"summary\",\"summary\":\"no version here\"}\n".to_owned(),
        ])
        .unwrap();
        assert_eq!(read.writer.unwrap().as_str(), "2.1.300");
        assert_eq!(read.model, model("claude-opus-5-5"));
    }

    /// A fallback stands for the original model only until a reply comes
    /// from another model: a user who then switches to the fallback's model
    /// with `/model` keeps it on resume.
    #[test]
    fn a_reply_from_another_model_ends_the_fallback() {
        let switched = [
            assistant("2.1.294", "claude-fable-5", false),
            FALLBACK.to_owned(),
            assistant("2.1.294", "claude-opus-4-8", false),
            assistant("2.1.294", "claude-fable-5", false),
            assistant("2.1.294", "claude-opus-4-8", false),
        ];
        assert_eq!(model_of(&switched), model("claude-opus-4-8"));
    }
}
