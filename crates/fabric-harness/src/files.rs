//! Staging files into a state tree and placing them back.
//!
//! Invariant: every file that enters a state tree passes the denylist, is
//! redacted or refused as its `Carry` says, and is written through a
//! temporary sibling and a rename, with its length and `BLAKE3` digest
//! recorded for the manifest. A restore writes nothing until every
//! destination has passed the rule of ADR 0006 (state outside the root is
//! protected like the root): the bytes already there are equal to, or a
//! prefix of, the incoming bytes, or are this node's own copy, whose
//! redacted form the incoming bytes extend; then only the incoming suffix
//! is added and the local bytes stay. A file the harness rewrites in place
//! (`Mutability::Replaceable`) may also be replaced outright, but only
//! when it still holds the bytes this node recorded when the job last left
//! it (`departure`). Every file written keeps the mode of
//! the file it replaces, 0600 when there is none. Nothing here is safe
//! against a process that appends to a destination at the same time; the
//! adapter's running-harness guard protects that case, not this module.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fabric_core::path::RelPath;
use tracing::warn;

use crate::denylist;
use crate::departure::Departure;
use crate::error::HarnessError;
use crate::jsontext::{self, TransformError};
use crate::manifest::{Carried, Content, Digest, MANIFEST_FILE, Mutability};
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
    fn content(self) -> Content {
        match self {
            Carry::Opaque | Carry::OpaqueRefuseSecrets => Content::Opaque,
            Carry::Text => Content::Text,
            Carry::JsonLines => Content::JsonLines,
            Carry::JsonDocument => Content::JsonDocument,
        }
    }

    fn of(content: Content) -> Self {
        match content {
            Content::Opaque => Carry::Opaque,
            Content::Text => Carry::Text,
            Content::JsonLines => Carry::JsonLines,
            Content::JsonDocument => Carry::JsonDocument,
        }
    }
}

/// Ends the process at a named step when `FABRIC_CRASHPOINT` names it, as
/// `<step>:<index>`. Built only with the `crashpoints` feature, which the
/// crash tests turn on, so they can stop a capture or restore at an exact
/// step instead of after a guessed delay.
#[cfg(feature = "crashpoints")]
fn crashpoint(step: &str, index: usize) {
    if std::env::var("FABRIC_CRASHPOINT").is_ok_and(|named| named == format!("{step}:{index}")) {
        std::process::abort();
    }
}

#[cfg(not(feature = "crashpoints"))]
fn crashpoint(_step: &str, _index: usize) {}

const TEMP_MARK: &str = ".fabric-tmp-";
const COMPARE_BUFFER: usize = 64 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// True for the name of a temporary file this module writes; a killed run
/// can leave one behind and it is never state.
#[must_use]
pub fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.contains(TEMP_MARK)
}

fn temp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    parent.join(format!(
        ".{name}{TEMP_MARK}{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Lists the regular files under `dir`, relative paths in sorted order.
/// A symlink is an error: a harness state tree has none, and following one
/// would carry or overwrite something outside the tree.
pub(crate) fn walk(dir: &Path) -> Result<Vec<PathBuf>, HarnessError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = fs::read_dir(&d).map_err(|e| HarnessError::io("walk", &d, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io("walk", &d, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io("walk", &path, e))?;
            if kind.is_symlink() {
                return Err(HarnessError::SymlinkInState { path });
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_temp_name)
                {
                    warn!(path = %path.display(), "skipping a temporary file left by a killed run");
                    continue;
                }
                let rel = path
                    .strip_prefix(dir)
                    .map(Path::to_path_buf)
                    .unwrap_or(path);
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Joins path components into a `RelPath`, refusing names the tree rules forbid.
pub(crate) fn rel_path(parts: &[&str]) -> Result<RelPath, HarnessError> {
    let joined = parts.join("/");
    RelPath::parse(&joined).map_err(|e| HarnessError::Malformed {
        what: "state path",
        path: PathBuf::from(&joined),
        reason: e.to_string(),
    })
}

/// Converts a file system path relative to some root into tree components.
pub(crate) fn rel_components(rel: &Path) -> Result<Vec<String>, HarnessError> {
    rel.components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| HarnessError::Malformed {
                    what: "state path",
                    path: rel.to_path_buf(),
                    reason: "not UTF-8".to_owned(),
                })
        })
        .collect()
}

/// What one write produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Written {
    /// Bytes written.
    pub size: u64,
    /// Digest of those bytes.
    pub digest: Digest,
}

struct Hashing<W> {
    inner: W,
    hasher: blake3::Hasher,
    size: u64,
}

impl<W: Write> Hashing<W> {
    fn new(inner: W) -> Self {
        Hashing {
            inner,
            hasher: blake3::Hasher::new(),
            size: 0,
        }
    }

    fn finish(self) -> (W, Written) {
        let written = Written {
            size: self.size,
            digest: Digest::from_hash(self.hasher.finalize()),
        };
        (self.inner, written)
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn open_temp(tmp: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)?;
    // `mode` applies only when the file is created; a temporary file left
    // by a killed run keeps its old mode until this resets it.
    file.set_permissions(Permissions::from_mode(0o600))?;
    Ok(file)
}

fn fsync_dir(dir: &Path) -> Result<(), HarnessError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| HarnessError::io("fsync", dir, e))
}

/// Moves `tmp` over `dest`, giving it the mode of the file it replaces, and
/// makes the rename durable.
fn commit(tmp: &Path, dest: &Path) -> Result<(), HarnessError> {
    match fs::metadata(dest) {
        Ok(meta) => {
            fs::set_permissions(tmp, meta.permissions())
                .map_err(|e| HarnessError::io("write", tmp, e))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(HarnessError::io("read", dest, e)),
    }
    fs::rename(tmp, dest).map_err(|e| HarnessError::io("rename", dest, e))?;
    fsync_dir(dest.parent().unwrap_or(Path::new(".")))
}

/// Writes a file through a temporary sibling and a rename, letting
/// `produce` write the bytes. The file gets mode 0600, or the mode of the file it
/// replaces, and the parent directory is fsynced after the rename.
pub(crate) fn write_atomic_with<F>(path: &Path, produce: F) -> Result<Written, HarnessError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), HarnessError>,
{
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| HarnessError::io("create dir", parent, e))?;
    let tmp = temp_path(path);
    let result = (|| {
        let file = open_temp(&tmp).map_err(|e| HarnessError::io("write", &tmp, e))?;
        let mut writer = BufWriter::new(Hashing::new(file));
        produce(&mut writer)?;
        let hashing = writer
            .into_inner()
            .map_err(|e| HarnessError::io("write", &tmp, e.into_error()))?;
        let (file, written) = hashing.finish();
        file.sync_all()
            .map_err(|e| HarnessError::io("write", &tmp, e))?;
        commit(&tmp, path)?;
        Ok(written)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Writes `bytes` to `path` through a temporary sibling and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), HarnessError> {
    write_atomic_with(path, |w| {
        w.write_all(bytes)
            .map_err(|e| HarnessError::io("write", path, e))
    })
    .map(|_| ())
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
        HarnessError::io("read", self.src, e)
    }

    fn emit(&mut self, bytes: &[u8]) -> Result<(), HarnessError> {
        self.out
            .write_all(bytes)
            .map_err(|e| HarnessError::io("write", self.src, e))
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
        io::copy(reader, self.out).map_err(|e| HarnessError::io("write", self.src, e))?;
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
fn transfer(
    src: &Path,
    out: &mut dyn Write,
    carry: Carry,
    redactor: &Redactor,
    counts: &mut RedactionCounts,
) -> Result<(), HarnessError> {
    let input = File::open(src).map_err(|e| HarnessError::io("read", src, e))?;
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
        Ok(()) => fsync_dir(out),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(HarnessError::io("remove", &path, e)),
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
        let written = write_atomic_with(&dst, |w| {
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
    let mut reader = File::open(path).map_err(|e| HarnessError::io("read", path, e))?;
    let mut sink = Hashing::new(io::sink());
    io::copy(&mut reader, &mut sink).map_err(|e| HarnessError::io("read", path, e))?;
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

/// How the bytes of an existing file relate to an incoming one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    /// There is no existing file.
    Missing,
    /// Both hold the same bytes.
    Equal,
    /// The existing bytes are a strict prefix of the incoming bytes.
    ExistingIsPrefix,
    /// The existing file is longer, or differs somewhere.
    Diverged,
}

/// Compares two files by streaming them; neither is loaded whole.
pub(crate) fn compare(existing: &Path, incoming: &Path) -> Result<Relation, HarnessError> {
    let a = match File::open(existing) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Relation::Missing),
        Err(e) => return Err(HarnessError::io("read", existing, e)),
    };
    let b = File::open(incoming).map_err(|e| HarnessError::io("read", incoming, e))?;
    let mut ra = BufReader::with_capacity(COMPARE_BUFFER, a);
    let mut rb = BufReader::with_capacity(COMPARE_BUFFER, b);
    loop {
        let ba = ra
            .fill_buf()
            .map_err(|e| HarnessError::io("read", existing, e))?;
        let bb = rb
            .fill_buf()
            .map_err(|e| HarnessError::io("read", incoming, e))?;
        if ba.is_empty() {
            return Ok(if bb.is_empty() {
                Relation::Equal
            } else {
                Relation::ExistingIsPrefix
            });
        }
        if bb.is_empty() {
            return Ok(Relation::Diverged);
        }
        let n = ba.len().min(bb.len());
        if ba[..n] != bb[..n] {
            return Ok(Relation::Diverged);
        }
        ra.consume(n);
        rb.consume(n);
    }
}

/// A sink that checks the bytes written to it against an incoming file,
/// so a redacted view of a local file is compared without being stored.
struct PrefixCheck {
    incoming: BufReader<File>,
    buffer: Vec<u8>,
    seen: u64,
    diverged: bool,
}

impl Write for PrefixCheck {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.diverged {
            return Ok(data.len());
        }
        self.buffer.resize(data.len(), 0);
        match io::Read::read_exact(&mut self.incoming, &mut self.buffer) {
            Ok(()) if self.buffer == data => self.seen += data.len() as u64,
            Ok(()) => self.diverged = true,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => self.diverged = true,
            Err(e) => return Err(e),
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The length of `existing` after the redaction capture applies, when
/// that redacted view is a prefix of `incoming`. This is how a node
/// recognises its own departed copy when the job comes back: the only
/// bytes the incoming file lacks are the ones capture removed on purpose.
fn redacted_view_prefix(
    existing: &Path,
    incoming: &Path,
    content: Content,
) -> Result<Option<u64>, HarnessError> {
    let reader = File::open(incoming).map_err(|e| HarnessError::io("read", incoming, e))?;
    let mut check = PrefixCheck {
        incoming: BufReader::with_capacity(COMPARE_BUFFER, reader),
        buffer: Vec::new(),
        seen: 0,
        diverged: false,
    };
    let produced = transfer(
        existing,
        &mut check,
        Carry::of(content),
        Redactor::standard()?,
        &mut RedactionCounts::new(),
    );
    match produced {
        Ok(()) => Ok((!check.diverged).then_some(check.seen)),
        Err(HarnessError::SecretInSignedBlock { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// One staged file and the place a restore puts it.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    /// The staged copy, already checked against its digest.
    pub staged: PathBuf,
    /// Where it lands on this node.
    pub dest: PathBuf,
    /// What its bytes are.
    pub content: Content,
    /// How the harness changes it.
    pub mutability: Mutability,
}

/// What restore does at one destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The destination already holds these bytes.
    Keep,
    /// The destination is missing, a prefix, or a rewritable file this node
    /// has not changed since the job left: it gets the staged bytes.
    Replace,
    /// The destination is this node's own copy, which the incoming file
    /// extends after its first `view_len` bytes: the local bytes are kept
    /// and only the incoming suffix is added.
    Extend { view_len: u64 },
}

fn plan(placement: &Placement, departure: &Departure) -> Result<Step, HarnessError> {
    match compare(&placement.dest, &placement.staged)? {
        Relation::Equal => return Ok(Step::Keep),
        Relation::Missing | Relation::ExistingIsPrefix => return Ok(Step::Replace),
        Relation::Diverged => {}
    }
    if placement.content != Content::Opaque {
        let staged_len = fs::metadata(&placement.staged)
            .map_err(|e| HarnessError::io("read", &placement.staged, e))?
            .len();
        match redacted_view_prefix(&placement.dest, &placement.staged, placement.content)? {
            Some(view_len) if view_len == staged_len => return Ok(Step::Keep),
            Some(view_len) => return Ok(Step::Extend { view_len }),
            None => {}
        }
    }
    if placement.mutability == Mutability::Replaceable && departure.unchanged(&placement.dest)? {
        return Ok(Step::Replace);
    }
    Err(HarnessError::StateDiverged {
        path: placement.dest.clone(),
    })
}

fn copy_into(path: &Path, out: &mut dyn Write, skip: u64) -> Result<(), HarnessError> {
    let mut reader = File::open(path).map_err(|e| HarnessError::io("read", path, e))?;
    if skip > 0 {
        io::Seek::seek(&mut reader, io::SeekFrom::Start(skip))
            .map_err(|e| HarnessError::io("read", path, e))?;
    }
    io::copy(&mut reader, out).map_err(|e| HarnessError::io("write", path, e))?;
    Ok(())
}

/// Places every file or none. Each destination is first checked against
/// the module rule, with `departure` saying which rewritable files this
/// node has not touched since the job left; only when all pass is anything
/// written, so a `StateDiverged` leaves this node as it was. A write that
/// then fails leaves the files before it placed, each one equal to, an
/// extension of, or an allowed replacement for what was there, and running
/// the restore again converges. Returns every destination, placed or found
/// equal, in order.
pub(crate) fn place_all(
    placements: &[Placement],
    departure: &Departure,
) -> Result<Vec<PathBuf>, HarnessError> {
    let mut dests: Vec<&Path> = placements.iter().map(|p| p.dest.as_path()).collect();
    dests.sort();
    if let Some(pair) = dests.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(HarnessError::DuplicateTreePath {
            path: pair[0].display().to_string(),
        });
    }
    let steps = placements
        .iter()
        .map(|placement| plan(placement, departure))
        .collect::<Result<Vec<Step>, HarnessError>>()?;
    for (index, (placement, step)) in placements.iter().zip(steps).enumerate() {
        match step {
            Step::Keep => {}
            Step::Replace => {
                write_atomic_with(&placement.dest, |w| copy_into(&placement.staged, w, 0))?;
            }
            Step::Extend { view_len } => {
                write_atomic_with(&placement.dest, |w| {
                    copy_into(&placement.dest, w, 0)?;
                    copy_into(&placement.staged, w, view_len)
                })?;
            }
        }
        crashpoint("place-after-file", index);
    }
    Ok(placements.iter().map(|p| p.dest.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn placement(dir: &Path, name: &str, dest: &Path, bytes: &[u8], content: Content) -> Placement {
        let staged = dir.join(format!("staged-{name}"));
        fs::write(&staged, bytes).unwrap();
        Placement {
            staged,
            dest: dest.to_path_buf(),
            content,
            mutability: Mutability::AppendOnly,
        }
    }

    fn place_bytes(dest: &Path, dir: &Path, bytes: &[u8]) -> Result<Vec<PathBuf>, HarnessError> {
        place_all(
            &[placement(dir, "one", dest, bytes, Content::Opaque)],
            &Departure::default(),
        )
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn place_accepts_equal_and_prefix_and_refuses_divergence() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a/b.jsonl");
        place_bytes(&dest, dir.path(), b"one\n").unwrap();
        place_bytes(&dest, dir.path(), b"one\n").unwrap();
        place_bytes(&dest, dir.path(), b"one\ntwo\n").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"one\ntwo\n");
        for older_or_other in [&b"one\n"[..], b"one\nTWO\n"] {
            assert!(matches!(
                place_bytes(&dest, dir.path(), older_or_other),
                Err(HarnessError::StateDiverged { .. })
            ));
        }
        assert_eq!(fs::read(&dest).unwrap(), b"one\ntwo\n");
        assert_eq!(
            fs::read_dir(dir.path().join("a")).unwrap().count(),
            1,
            "no temporary file is left behind"
        );
    }

    #[test]
    fn a_divergence_in_the_last_file_writes_nothing_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.jsonl");
        let last = dir.path().join("last.jsonl");
        fs::write(&last, b"mine\n").unwrap();
        let placements = [
            placement(dir.path(), "a", &first, b"new\n", Content::JsonLines),
            placement(dir.path(), "b", &last, b"theirs\n", Content::JsonLines),
        ];
        let err = place_all(&placements, &Departure::default()).unwrap_err();
        assert!(
            matches!(err, HarnessError::StateDiverged { ref path } if path == &last),
            "{err}"
        );
        assert!(!first.exists(), "nothing is placed when one file diverges");
        assert_eq!(fs::read(&last).unwrap(), b"mine\n");
    }

    #[test]
    fn written_files_keep_the_mode_they_replace_and_default_to_0600() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.jsonl");
        place_bytes(&fresh, dir.path(), b"x\n").unwrap();
        assert_eq!(mode(&fresh), 0o600);
        let private = dir.path().join("private.jsonl");
        fs::write(&private, b"x\n").unwrap();
        fs::set_permissions(&private, Permissions::from_mode(0o600)).unwrap();
        place_bytes(&private, dir.path(), b"x\ny\n").unwrap();
        assert_eq!(mode(&private), 0o600);
        let shared = dir.path().join("shared.json");
        fs::write(&shared, b"x\n").unwrap();
        fs::set_permissions(&shared, Permissions::from_mode(0o644)).unwrap();
        write_atomic(&shared, b"x\ny\n").unwrap();
        assert_eq!(mode(&shared), 0o644);
    }

    #[test]
    fn a_departed_copy_keeps_its_own_bytes_and_gains_only_the_new_suffix() {
        // The node's own transcript still holds the secret; the incoming
        // copy was redacted at capture and then grew elsewhere.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("t.jsonl");
        let original = format!("{{\"t\":\"{GITHUB}\"}}\n");
        fs::write(&dest, &original).unwrap();
        let incoming = b"{\"t\":\"[REDACTED:github-token]\"}\n{\"more\":1}\n";
        place_all(
            &[placement(
                dir.path(),
                "a",
                &dest,
                incoming,
                Content::JsonLines,
            )],
            &Departure::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            format!("{original}{{\"more\":1}}\n")
        );
        // The same incoming copy again changes nothing.
        let before = fs::read(&dest).unwrap();
        place_all(
            &[placement(
                dir.path(),
                "b",
                &dest,
                incoming,
                Content::JsonLines,
            )],
            &Departure::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), before);
        // A different edit is still a divergence.
        fs::write(&dest, format!("{{\"t\":\"{GITHUB}\",\"edited\":true}}\n")).unwrap();
        assert!(matches!(
            place_all(
                &[placement(
                    dir.path(),
                    "c",
                    &dest,
                    incoming,
                    Content::JsonLines
                )],
                &Departure::default(),
            ),
            Err(HarnessError::StateDiverged { .. })
        ));
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

    #[test]
    fn walk_skips_temp_files_and_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), b"a").unwrap();
        fs::write(dir.path().join(".a.fabric-tmp-1-0"), b"partial").unwrap();
        assert_eq!(walk(dir.path()).unwrap(), vec![PathBuf::from("a")]);
        std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("link")).unwrap();
        assert!(matches!(
            walk(dir.path()),
            Err(HarnessError::SymlinkInState { .. })
        ));
    }
}
