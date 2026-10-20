//! Staging files into a state tree.
//!
//! Invariant: every file that enters a state tree passes the denylist, is
//! redacted or refused as its `Carry` says, and is written durably, with
//! its length and `BLAKE3` digest recorded for the manifest. A staged tree
//! is checked against those digests before restore places a byte of it.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use tracing::warn;

use crate::crashpoint::crashpoint;
use crate::denylist;
use crate::durable::{self, Hashing, Written};
use crate::error::{HarnessError, IoOp};
use crate::jsontext::{self, TransformError};
use crate::manifest::{Carried, Content, MANIFEST_FILE, Mutability};
use crate::redact::{RedactionCounts, Redactor};

/// The redaction rule name under which dropped signed blocks are counted.
pub const SIGNED_BLOCK_DROPPED: &str = "signed-block-dropped";

/// One file an adapter wants to carry.
#[derive(Debug, Clone)]
pub(crate) struct Source {
    /// Where the file is on this node.
    pub abs: PathBuf,
    /// Where it goes in the state tree.
    pub rel: RelPath,
    /// How capture treats its bytes.
    pub carry: Carry,
    /// How the harness changes the file.
    pub mutability: Mutability,
}

/// How capture treats a file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Carry {
    /// Copied byte for byte; the adapter scanned it for secrets another way.
    Opaque,
    /// Copied byte for byte; capture is refused when a line holds a secret
    /// shape, raw or inside a decoded JSON string.
    OpaqueRefuseSecrets,
    /// Plain text, redacted line by line.
    Text,
    /// One JSON value per line.
    JsonLines,
    /// One JSON document.
    JsonDocument,
}

impl Carry {
    /// The content kind the manifest records for this treatment.
    pub(crate) fn content(self) -> Content {
        match self {
            Carry::Opaque | Carry::OpaqueRefuseSecrets => Content::Opaque,
            Carry::Text => Content::Text,
            Carry::JsonLines => Content::JsonLines,
            Carry::JsonDocument => Content::JsonDocument,
        }
    }

    pub(crate) fn of(content: Content) -> Self {
        match content {
            Content::Opaque => Carry::Opaque,
            Content::Text => Carry::Text,
            Content::JsonLines => Carry::JsonLines,
            Content::JsonDocument => Carry::JsonDocument,
        }
    }
}

fn split_newline(line: &[u8]) -> (&[u8], &'static [u8]) {
    match line.strip_suffix(b"\n") {
        Some(body) => (body, b"\n"),
        None => (line, b""),
    }
}

/// What the JSON transform made of one text.
enum Parsed {
    /// Valid JSON with nothing to redact.
    Same,
    /// Valid JSON, redacted.
    Redacted(String),
    /// Not JSON; the caller redacts it as plain text.
    NotJson,
}

/// What every capture step needs: where the bytes come from and go, and
/// the rules that apply to them.
struct Transfer<'a> {
    src: &'a Path,
    out: &'a mut dyn Write,
    redactor: &'a Redactor,
    counts: &'a mut RedactionCounts,
    /// True while the text being read is inside a PEM private key block
    /// whose BEGIN line was on an earlier line: such lines are dropped up
    /// to and including the END line.
    in_private_key: bool,
}

impl Transfer<'_> {
    fn read_err(&self, e: io::Error) -> HarnessError {
        HarnessError::io(IoOp::Read, self.src, e)
    }

    fn emit(&mut self, bytes: &[u8]) -> Result<(), HarnessError> {
        self.out
            .write_all(bytes)
            .map_err(|e| HarnessError::io(IoOp::Write, self.src, e))
    }

    fn next_line(
        &self,
        reader: &mut BufReader<File>,
        line: &mut Vec<u8>,
    ) -> Result<bool, HarnessError> {
        line.clear();
        let n = reader
            .read_until(b'\n', line)
            .map_err(|e| self.read_err(e))?;
        Ok(n > 0)
    }

    fn opaque(&mut self, reader: &mut BufReader<File>) -> Result<(), HarnessError> {
        io::copy(reader, self.out).map_err(|e| HarnessError::io(IoOp::Write, self.src, e))?;
        Ok(())
    }

    fn opaque_refusing_secrets(
        &mut self,
        reader: &mut BufReader<File>,
    ) -> Result<(), HarnessError> {
        let mut line = Vec::new();
        let mut number = 0usize;
        while self.next_line(reader, &mut line)? {
            number += 1;
            if jsontext::holds_secret(&line, self.redactor) {
                return Err(HarnessError::SecretInState {
                    path: self.src.to_path_buf(),
                    line: number,
                });
            }
            self.emit(&line)?;
        }
        Ok(())
    }

    /// Redacts one line as text, carrying a PEM private key block across
    /// lines: the BEGIN line is redacted from the header to its end, every
    /// later line up to the END line is dropped, and what follows the END
    /// header on its line is redacted like any other text. A block with no
    /// END line is dropped to the end of the file.
    fn text_line(&mut self, line: &[u8]) -> Result<(), HarnessError> {
        if self.in_private_key {
            let Some(end) = self.redactor.private_key_end(line) else {
                return Ok(());
            };
            self.in_private_key = false;
            let rest = &line[end..];
            return if rest.trim_ascii().is_empty() {
                Ok(())
            } else {
                self.text_line(rest)
            };
        }
        self.in_private_key = self.redactor.opens_private_key(line);
        self.plain_line(line)
    }

    /// Redacts one line on its own: as UTF-8 when it is, as bytes when it
    /// is not, so a line cut inside a multi-byte character keeps its bytes.
    fn plain_line(&mut self, line: &[u8]) -> Result<(), HarnessError> {
        let (body, newline) = split_newline(line);
        let redacted: Option<Vec<u8>> = match std::str::from_utf8(body) {
            Ok(text) => self
                .redactor
                .apply(text, self.counts)
                .map(String::into_bytes),
            Err(_) => self.redactor.apply_bytes(body, self.counts),
        };
        match redacted {
            Some(redacted) => {
                self.emit(&redacted)?;
                self.emit(newline)
            }
            None => self.emit(line),
        }
    }

    fn text(&mut self, reader: &mut BufReader<File>) -> Result<(), HarnessError> {
        let mut line = Vec::new();
        while self.next_line(reader, &mut line)? {
            self.text_line(&line)?;
        }
        Ok(())
    }

    /// Redacts one JSON text.
    fn json(&mut self, text: &str, line: usize) -> Result<Parsed, HarnessError> {
        match jsontext::transform(text, self.redactor) {
            Ok(outcome) => {
                fold_counts(self.counts, &outcome);
                Ok(outcome.text.map_or(Parsed::Same, Parsed::Redacted))
            }
            Err(TransformError::NotJson { .. }) => {
                warn!(path = %self.src.display(), line, "not JSON; redacted as text");
                Ok(Parsed::NotJson)
            }
            Err(TransformError::SecretInUndroppableSignedBlock) => {
                Err(HarnessError::SecretInSignedBlock {
                    path: self.src.to_path_buf(),
                    line,
                })
            }
        }
    }

    fn json_lines(&mut self, reader: &mut BufReader<File>) -> Result<(), HarnessError> {
        let mut line = Vec::new();
        let mut number = 0usize;
        while self.next_line(reader, &mut line)? {
            number += 1;
            if self.in_private_key {
                self.text_line(&line)?;
                continue;
            }
            let (body, newline) = split_newline(&line);
            let Ok(text) = std::str::from_utf8(body) else {
                warn!(path = %self.src.display(), line = number, "not UTF-8; redacted as bytes");
                self.text_line(&line)?;
                continue;
            };
            if text.trim().is_empty() {
                self.emit(&line)?;
                continue;
            }
            match self.json(text, number)? {
                Parsed::Same => self.emit(&line)?,
                Parsed::Redacted(new) => {
                    self.emit(new.as_bytes())?;
                    self.emit(newline)?;
                }
                Parsed::NotJson => self.text_line(&line)?,
            }
        }
        Ok(())
    }

    fn json_document(&mut self, reader: &mut BufReader<File>) -> Result<(), HarnessError> {
        let mut bytes = Vec::new();
        io::Read::read_to_end(reader, &mut bytes).map_err(|e| self.read_err(e))?;
        if let Ok(text) = std::str::from_utf8(&bytes) {
            match self.json(text, 1)? {
                Parsed::Same => return self.emit(&bytes),
                Parsed::Redacted(new) => return self.emit(new.as_bytes()),
                Parsed::NotJson => {}
            }
        }
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            self.text_line(line)?;
        }
        Ok(())
    }
}

/// Copies `src` into `out` under the rules of `carry`, redacting with
/// `redactor`. Every kind but a JSON document streams line by line;
/// documents are the small sidecars (`settings.json`, `*.meta.json`),
/// never a transcript.
pub(crate) fn transfer(
    src: &Path,
    out: &mut dyn Write,
    carry: Carry,
    redactor: &Redactor,
    counts: &mut RedactionCounts,
) -> Result<(), HarnessError> {
    let input = File::open(src).map_err(|e| HarnessError::io(IoOp::Read, src, e))?;
    let mut reader = BufReader::new(input);
    let mut step = Transfer {
        src,
        out,
        redactor,
        counts,
        in_private_key: false,
    };
    match carry {
        Carry::Opaque => step.opaque(&mut reader),
        Carry::OpaqueRefuseSecrets => step.opaque_refusing_secrets(&mut reader),
        Carry::Text => step.text(&mut reader),
        Carry::JsonLines => step.json_lines(&mut reader),
        Carry::JsonDocument => step.json_document(&mut reader),
    }
}

fn fold_counts(counts: &mut RedactionCounts, outcome: &jsontext::Outcome) {
    for (rule, n) in &outcome.redactions {
        *counts.entry(rule).or_insert(0) += n;
    }
    if outcome.dropped_signed_blocks > 0 {
        *counts.entry(SIGNED_BLOCK_DROPPED).or_insert(0) += outcome.dropped_signed_blocks;
    }
}

/// Removes an earlier capture's manifest from `out` and makes the removal
/// durable, so a capture killed part way through a directory it reuses
/// never leaves an old manifest naming files it has partly overwritten.
fn clear_manifest(out: &Path) -> Result<(), HarnessError> {
    let path = out.join(MANIFEST_FILE);
    match fs::remove_file(&path) {
        Ok(()) => durable::fsync_dir(out),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(HarnessError::io(IoOp::Remove, &path, e)),
    }
}

/// Stages `sources` under `out`, checking the denylist and redacting as
/// each file's `Carry` says. Returns the carried entries sorted by path and
/// adds to the redaction counts. An earlier manifest in `out` is removed
/// first; the caller writes the new one last.
pub(crate) fn stage(
    sources: &[Source],
    out: &Path,
    counts: &mut RedactionCounts,
) -> Result<Vec<Carried>, HarnessError> {
    let mut sorted: Vec<&Source> = sources.iter().collect();
    sorted.sort_by(|a, b| a.rel.cmp(&b.rel));
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0].rel == pair[1].rel) {
        return Err(HarnessError::DuplicateTreePath {
            path: pair[0].rel.as_str().to_owned(),
        });
    }
    for source in &sorted {
        if let Some(rule) = denylist::check(source.rel.as_str()) {
            return Err(HarnessError::Denied {
                path: source.rel.as_str().to_owned(),
                rule,
            });
        }
    }
    let redactor = Redactor::standard()?;
    clear_manifest(out)?;
    let mut carried = Vec::with_capacity(sorted.len());
    for (index, source) in sorted.into_iter().enumerate() {
        let dst = out.join(source.rel.as_str());
        let written = durable::write_atomic_with(&dst, |w| {
            transfer(&source.abs, w, source.carry, redactor, counts)
        })?;
        carried.push(Carried {
            rel_path: source.rel.clone(),
            content: source.carry.content(),
            mutability: source.mutability,
            size: written.size,
            blake3: written.digest,
        });
        crashpoint("stage-after-file", index);
    }
    Ok(carried)
}

/// The length and digest of the file at `path`.
pub(crate) fn digest_file(path: &Path) -> Result<Written, HarnessError> {
    let mut reader = File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    let mut sink = Hashing::new(io::sink());
    io::copy(&mut reader, &mut sink).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    Ok(sink.finish().1)
}

/// Checks every staged file's length and digest against the manifest, so
/// a tree cut by a crash is refused before anything is placed.
pub(crate) fn verify(state: &Path, files: &[Carried]) -> Result<(), HarnessError> {
    for file in files {
        let path = state.join(file.rel_path.as_str());
        let found = digest_file(&path)?;
        if found.size != file.size || found.digest != file.blake3 {
            return Err(HarnessError::StateCorrupt {
                path,
                reason: format!(
                    "manifest says {} bytes, {}; found {} bytes, {}",
                    file.size, file.blake3, found.size, found.digest
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Digest;
    use std::os::unix::fs::PermissionsExt;

    const GITHUB: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";

    fn source(abs: PathBuf, rel: &str, carry: Carry) -> Source {
        Source {
            abs,
            rel: RelPath::parse(rel).unwrap(),
            carry,
            mutability: Mutability::AppendOnly,
        }
    }

    fn stage_one(dir: &Path, name: &str, bytes: &[u8], carry: Carry) -> (PathBuf, Vec<Carried>) {
        let src = dir.join(name);
        fs::write(&src, bytes).unwrap();
        let out = dir.join("out");
        let sources = vec![source(src, &format!("home/{name}"), carry)];
        let carried = stage(&sources, &out, &mut RedactionCounts::new()).unwrap();
        (out.join(format!("home/{name}")), carried)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn stage_refuses_a_denied_path_before_copying() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok");
        let secret = dir.path().join("src");
        fs::write(&ok, b"x").unwrap();
        fs::write(&secret, b"x").unwrap();
        let out = dir.path().join("out");
        let sources = vec![
            source(ok, "home/.claude/a", Carry::Opaque),
            source(secret, "home/.claude/.credentials.json", Carry::Opaque),
        ];
        let err = stage(&sources, &out, &mut RedactionCounts::new()).unwrap_err();
        assert!(matches!(err, HarnessError::Denied { .. }), "{err}");
        assert!(!out.exists());
    }

    #[test]
    fn stage_refuses_two_sources_on_one_tree_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let sources = vec![
            source(a, "home/x", Carry::Opaque),
            source(b, "home/x", Carry::Opaque),
        ];
        let err = stage(
            &sources,
            &dir.path().join("out"),
            &mut RedactionCounts::new(),
        )
        .unwrap_err();
        assert!(
            matches!(err, HarnessError::DuplicateTreePath { .. }),
            "{err}"
        );
    }

    #[test]
    fn stage_redacts_json_lines_and_keeps_a_cut_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let bytes =
            format!("{{\"cwd\":\"/w\",\"t\":\"{GITHUB}\"}}\n{{\"cwd\":\"/w\",\"cut\":\"{GITHUB}");
        let mut counts = RedactionCounts::new();
        let src = dir.path().join("t.jsonl");
        fs::write(&src, bytes).unwrap();
        let out = dir.path().join("out");
        let carried = stage(
            &[source(src, "home/t.jsonl", Carry::JsonLines)],
            &out,
            &mut counts,
        )
        .unwrap();
        let staged = fs::read_to_string(out.join("home/t.jsonl")).unwrap();
        assert!(!staged.contains(GITHUB));
        assert_eq!(counts.get("github-token"), Some(&2));
        assert!(staged.ends_with("\"cut\":\"[REDACTED:github-token]"));
        assert_eq!(carried[0].size, staged.len() as u64);
        assert_eq!(
            carried[0].blake3,
            Digest::from_hash(blake3::hash(staged.as_bytes()))
        );
        assert_eq!(mode(&out.join("home/t.jsonl")), 0o600);
    }

    #[test]
    fn a_line_cut_inside_a_two_byte_character_does_not_stop_capture() {
        // `kill -9` cut "café" after the first byte of `é`; Claude then
        // appended a whole line after the cut.
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = b"{\"a\":\"ok\"}\n{\"a\":\"caf\xC3".to_vec();
        bytes.extend_from_slice(format!(" {GITHUB}\n{{\"a\":\"{GITHUB}\"}}\n").as_bytes());
        let (staged, _) = stage_one(dir.path(), "t.jsonl", &bytes, Carry::JsonLines);
        let mut expected = b"{\"a\":\"ok\"}\n{\"a\":\"caf\xC3 [REDACTED:github-token]\n".to_vec();
        expected.extend_from_slice(b"{\"a\":\"[REDACTED:github-token]\"}\n");
        assert_eq!(fs::read(&staged).unwrap(), expected);
    }

    #[test]
    fn stage_counts_a_dropped_signed_block() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!(
            "{{\"content\":[{{\"type\":\"text\",\"text\":\"a\"}},{{\"type\":\"thinking\",\"thinking\":\"{GITHUB}\",\"signature\":\"s\"}}]}}\n"
        );
        let src = dir.path().join("t.jsonl");
        fs::write(&src, line).unwrap();
        let mut counts = RedactionCounts::new();
        stage(
            &[source(src, "home/t.jsonl", Carry::JsonLines)],
            &dir.path().join("out"),
            &mut counts,
        )
        .unwrap();
        assert_eq!(counts.get(SIGNED_BLOCK_DROPPED), Some(&1));
        assert!(!counts.contains_key("github-token"));
    }

    #[test]
    fn opaque_with_a_secret_is_refused_and_text_is_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "AKIAABCDEFGHIJKLMNOP";
        let src = dir.path().join("r.jsonl");
        fs::write(&src, format!("one\n{{\"k\":\"{secret}\"}}\n")).unwrap();
        let err = stage(
            &[source(src, "home/r.jsonl", Carry::OpaqueRefuseSecrets)],
            &dir.path().join("out"),
            &mut RedactionCounts::new(),
        )
        .unwrap_err();
        assert!(
            matches!(err, HarnessError::SecretInState { line: 2, .. }),
            "{err}"
        );
        assert!(!dir.path().join("out/home/r.jsonl").exists());
        let mut raw = format!("key {secret} here\n").into_bytes();
        raw.extend_from_slice(b"\xffraw\n");
        let (staged, _) = stage_one(dir.path(), "notes.txt", &raw, Carry::Text);
        assert_eq!(
            fs::read(&staged).unwrap(),
            b"key [REDACTED:aws-access-key] here\n\xffraw\n",
            "a line that is not UTF-8 is kept byte for byte"
        );
    }

    const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACBkZXZpY2Uta2V5\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn a_private_key_over_several_lines_is_redacted_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, format!("before\nkey: {PEM}after\n")).unwrap();
        let mut counts = RedactionCounts::new();
        stage(
            &[source(src, "home/a.txt", Carry::Text)],
            &dir.path().join("out"),
            &mut counts,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("out/home/a.txt")).unwrap(),
            "before\nkey: [REDACTED:private-key-block]\nafter\n"
        );
        assert_eq!(counts.get("private-key-block"), Some(&1));

        // A file cut inside the key loses everything after the BEGIN line.
        let cut = &PEM[..PEM.find("QyNT").unwrap()];
        let (staged, _) = stage_one(
            dir.path(),
            "cut.txt",
            format!("x\n{cut}").as_bytes(),
            Carry::Text,
        );
        assert_eq!(
            fs::read_to_string(staged).unwrap(),
            "x\n[REDACTED:private-key-block]\n"
        );
    }

    #[test]
    fn a_private_key_over_several_lines_refuses_an_opaque_capture() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s1.json");
        fs::write(&src, format!("{{\n\"k\": \"x\"\n}}\n{PEM}")).unwrap();
        let err = stage(
            &[source(src, "home/s1.json", Carry::OpaqueRefuseSecrets)],
            &dir.path().join("out"),
            &mut RedactionCounts::new(),
        )
        .unwrap_err();
        assert!(
            matches!(err, HarnessError::SecretInState { line: 4, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_json_document_is_redacted_whole_and_falls_back_to_text() {
        let dir = tempfile::tempdir().unwrap();
        let pretty = format!("{{\n  \"token\": \"{GITHUB}\"\n}}\n");
        let (staged, _) = stage_one(dir.path(), "d.json", pretty.as_bytes(), Carry::JsonDocument);
        assert_eq!(
            fs::read_to_string(&staged).unwrap(),
            "{\n  \"token\": \"[REDACTED:github-token]\"\n}\n"
        );
        let broken = format!("{{\n  \"token\": \"{GITHUB}\"\n");
        let (staged, _) = stage_one(dir.path(), "e.json", broken.as_bytes(), Carry::JsonDocument);
        assert_eq!(
            fs::read_to_string(&staged).unwrap(),
            "{\n  \"token\": \"[REDACTED:github-token]\"\n"
        );
    }

    #[test]
    fn verify_catches_a_truncated_or_edited_staged_file() {
        let dir = tempfile::tempdir().unwrap();
        let (staged, carried) = stage_one(dir.path(), "t.txt", b"hello\nworld\n", Carry::Text);
        verify(&dir.path().join("out"), &carried).unwrap();
        fs::write(&staged, b"hello\n").unwrap();
        let err = verify(&dir.path().join("out"), &carried).unwrap_err();
        assert!(matches!(err, HarnessError::StateCorrupt { .. }), "{err}");
        fs::write(&staged, b"hello\nworlD\n").unwrap();
        assert!(verify(&dir.path().join("out"), &carried).is_err());
    }
}
