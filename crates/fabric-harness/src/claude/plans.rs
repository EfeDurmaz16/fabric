//! Which plan files travel with a Claude session.
//!
//! Invariant: a file under `~/.claude/plans/` is carried, and placed back,
//! only when it belongs to the session. Claude names a session's plan
//! `<slug>.md`, a subagent's plan `<slug>-agent-<agent id>.md` and the
//! plan's workshop document `<slug>.workshop.md`, where the slug is the
//! first `slug` a record of the session's transcript holds; on `--resume`
//! Claude reads that slug back from the transcript and opens the same
//! names (2.1.294 bundle, `copyPlanForResume`, `getPlanFilePath` and
//! `getSlugFromLog`, read on 2026-10-22). A slug that is not
//! `[a-z0-9][a-z0-9-]{0,119}` names no plan, as Claude refuses it too. A
//! plans directory moved by the `plansDirectory` setting lies inside the
//! project root and travels with the workspace snapshot (ADR 0004).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::Deserialize;

use crate::error::{HarnessError, IoOp};

/// The longest slug Claude accepts from a transcript.
const SLUG_MAX: usize = 120;

#[derive(Deserialize)]
struct Line {
    slug: Option<String>,
}

/// True for a slug Claude accepts from a transcript.
fn valid(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    bytes.len() <= SLUG_MAX
        && bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The plan slug of each transcript: the first non-empty `slug` of its
/// records, as Claude takes it, when it is valid. A line that does not
/// parse is skipped, as a cut last line is.
pub(crate) fn slugs(transcripts: &[&Path]) -> Result<BTreeSet<String>, HarnessError> {
    let mut found = BTreeSet::new();
    for path in transcripts {
        let file = File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
            if n == 0 {
                break;
            }
            if !line.windows(6).any(|w| w == b"\"slug\"") {
                continue;
            }
            let Ok(Line { slug: Some(slug) }) = serde_json::from_slice::<Line>(&line) else {
                continue;
            };
            if slug.is_empty() {
                continue;
            }
            if valid(&slug) {
                found.insert(slug);
            }
            break;
        }
    }
    Ok(found)
}

/// True when the file `name` directly under the plans directory is the
/// plan, a subagent's plan or the workshop document of one of `slugs`.
pub(crate) fn belongs(name: &str, slugs: &BTreeSet<String>) -> bool {
    slugs.iter().any(|slug| {
        let Some(rest) = name.strip_prefix(slug.as_str()) else {
            return false;
        };
        let agent = rest
            .strip_prefix("-agent-")
            .and_then(|rest| rest.strip_suffix(".md"))
            .is_some_and(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            });
        rest == ".md" || rest == ".workshop.md" || agent
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_slug_of_each_transcript_names_its_plans() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        std::fs::write(
            &a,
            concat!(
                "{\"type\":\"user\",\"slug\":\"\"}\n",
                "{\"type\":\"user\",\"slug\":\"dreamy-popping-cerf\"}\n",
                "{\"type\":\"user\",\"slug\":\"later-other-slug\"}\n",
            ),
        )
        .unwrap();
        std::fs::write(&b, "{\"type\":\"user\",\"slug\":\"../escape\"}\n").unwrap();
        let slugs = slugs(&[a.as_path(), b.as_path()]).unwrap();
        assert_eq!(
            slugs.iter().map(String::as_str).collect::<Vec<_>>(),
            ["dreamy-popping-cerf"]
        );
        for name in [
            "dreamy-popping-cerf.md",
            "dreamy-popping-cerf.workshop.md",
            "dreamy-popping-cerf-agent-a97a8637863f59591.md",
        ] {
            assert!(belongs(name, &slugs), "{name} belongs");
        }
        for name in [
            "later-other-slug.md",
            "dreamy-popping-cerf.txt",
            "dreamy-popping-cerf-agent-.md",
            "dreamy-popping-cerf-agent-x/y.md",
            "dreamy-popping-cerfs.md",
        ] {
            assert!(!belongs(name, &slugs), "{name} does not belong");
        }
    }
}
