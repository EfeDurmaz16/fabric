//! Placing a staged tree back on a node.
//!
//! Invariant: a restore writes nothing until every destination has passed
//! the rule of ADR 0006 (state outside the root is protected like the
//! root): the bytes already there are equal to, or a prefix of, the
//! incoming bytes, or are this node's own copy, whose redacted form the
//! incoming bytes extend; then only the incoming suffix is added and the
//! local bytes stay. A file the harness rewrites in place
//! (`Mutability::Replaceable`) may also be replaced outright, but only
//! when it still holds the bytes this node recorded when the job last left
//! it, which the caller answers. Nothing here is safe against a process
//! that appends to a destination at the same time; the adapter's
//! running-harness guard protects that case, not this module.

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

use crate::crashpoint::crashpoint;
use crate::durable;
use crate::error::{HarnessError, IoOp};
use crate::manifest::{Content, Mutability};
use crate::redact::{RedactionCounts, Redactor};
use crate::stage::{self, Carry};

const COMPARE_BUFFER: usize = 64 * 1024;

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
        Err(e) => return Err(HarnessError::io(IoOp::Read, existing, e)),
    };
    let b = File::open(incoming).map_err(|e| HarnessError::io(IoOp::Read, incoming, e))?;
    let mut ra = BufReader::with_capacity(COMPARE_BUFFER, a);
    let mut rb = BufReader::with_capacity(COMPARE_BUFFER, b);
    loop {
        let ba = ra
            .fill_buf()
            .map_err(|e| HarnessError::io(IoOp::Read, existing, e))?;
        let bb = rb
            .fill_buf()
            .map_err(|e| HarnessError::io(IoOp::Read, incoming, e))?;
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
    let reader = File::open(incoming).map_err(|e| HarnessError::io(IoOp::Read, incoming, e))?;
    let mut check = PrefixCheck {
        incoming: BufReader::with_capacity(COMPARE_BUFFER, reader),
        buffer: Vec::new(),
        seen: 0,
        diverged: false,
    };
    let produced = stage::transfer(
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

/// Says whether a rewritable file at a destination still holds the
/// bytes this node recorded when the job last left it.
pub(crate) type Unchanged<'a> = &'a dyn Fn(&Path) -> Result<bool, HarnessError>;

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

fn plan(placement: &Placement, unchanged: Unchanged<'_>) -> Result<Step, HarnessError> {
    match compare(&placement.dest, &placement.staged)? {
        Relation::Equal => return Ok(Step::Keep),
        Relation::Missing | Relation::ExistingIsPrefix => return Ok(Step::Replace),
        Relation::Diverged => {}
    }
    if placement.content != Content::Opaque {
        let staged_len = fs::metadata(&placement.staged)
            .map_err(|e| HarnessError::io(IoOp::Read, &placement.staged, e))?
            .len();
        match redacted_view_prefix(&placement.dest, &placement.staged, placement.content)? {
            Some(view_len) if view_len == staged_len => return Ok(Step::Keep),
            Some(view_len) => return Ok(Step::Extend { view_len }),
            None => {}
        }
    }
    if placement.mutability == Mutability::Replaceable && unchanged(&placement.dest)? {
        return Ok(Step::Replace);
    }
    Err(HarnessError::StateDiverged {
        path: placement.dest.clone(),
    })
}

fn copy_into(path: &Path, out: &mut dyn Write, skip: u64) -> Result<(), HarnessError> {
    let mut reader = File::open(path).map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    if skip > 0 {
        io::Seek::seek(&mut reader, io::SeekFrom::Start(skip))
            .map_err(|e| HarnessError::io(IoOp::Read, path, e))?;
    }
    io::copy(&mut reader, out).map_err(|e| HarnessError::io(IoOp::Write, path, e))?;
    Ok(())
}

/// Places every file or none. Each destination is first checked against
/// the module rule, with `unchanged` saying which rewritable files this
/// node has not touched since the job left; only when all pass is anything
/// written, so a `StateDiverged` leaves this node as it was. A write that
/// then fails leaves the files before it placed, each one equal to, an
/// extension of, or an allowed replacement for what was there, and running
/// the restore again converges. Returns every destination, placed or found
/// equal, in order.
pub(crate) fn place_all(
    placements: &[Placement],
    unchanged: Unchanged<'_>,
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
        .map(|placement| plan(placement, unchanged))
        .collect::<Result<Vec<Step>, HarnessError>>()?;
    for (index, (placement, step)) in placements.iter().zip(steps).enumerate() {
        match step {
            Step::Keep => {}
            Step::Replace => {
                durable::write_atomic_with(&placement.dest, |w| {
                    copy_into(&placement.staged, w, 0)
                })?;
            }
            Step::Extend { view_len } => {
                durable::write_atomic_with(&placement.dest, |w| {
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
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    const GITHUB: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";

    /// A node that recorded no departure: no rewritable file may be replaced.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "it stands in for the fallible lookup of a departure record"
    )]
    fn never_left(_: &Path) -> Result<bool, HarnessError> {
        Ok(false)
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
            &never_left,
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
        let err = place_all(&placements, &never_left).unwrap_err();
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
        durable::write_atomic(&shared, b"x\ny\n").unwrap();
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
            &never_left,
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
            &never_left,
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
                &never_left,
            ),
            Err(HarnessError::StateDiverged { .. })
        ));
    }
}
