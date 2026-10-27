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
//! it, which the caller answers. Each destination is checked again just
//! before its rename: when its length, times or inode moved since it was
//! planned, as a harness that started after the adapter's running-harness
//! guard and wrote to it would move them, the restore stops there
//! (`StateChanged`) and the bytes that harness wrote stay. A write that
//! lands between that check and the rename is not caught; only a lock the
//! harness itself honours could close that window.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
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
        Ok(_) => Ok((!check.diverged).then_some(check.seen)),
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

/// What a destination was when restore planned it: its device, inode,
/// length, modification and change times, or `None` when it was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen(Option<[i64; 7]>);

impl Seen {
    /// Reads what `path` is now, without following a link.
    pub(crate) fn of(path: &Path) -> Result<Self, HarnessError> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Seen(None)),
            Err(e) => return Err(HarnessError::io(IoOp::Read, path, e)),
        };
        let wide = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        Ok(Seen(Some([
            wide(meta.dev()),
            wide(meta.ino()),
            wide(meta.size()),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
        ])))
    }

    /// Refuses `path` when it is no longer what was seen.
    pub(crate) fn still(self, path: &Path) -> Result<(), HarnessError> {
        if Seen::of(path)? == self {
            Ok(())
        } else {
            Err(HarnessError::StateChanged {
                path: path.to_path_buf(),
            })
        }
    }
}

fn plan(placement: &Placement, unchanged: Unchanged<'_>) -> Result<(Step, Seen), HarnessError> {
    // Read before the bytes are compared, so a write during the compare
    // shows at the check before the rename.
    let seen = Seen::of(&placement.dest)?;
    let step = plan_step(placement, unchanged)?;
    Ok((step, seen))
}

fn plan_step(placement: &Placement, unchanged: Unchanged<'_>) -> Result<Step, HarnessError> {
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
/// then fails, or a destination that changed since it was checked
/// (`StateChanged`), leaves the files before it placed, each one equal to,
/// an extension of, or an allowed replacement for what was there, and
/// running the restore again converges. Returns every destination, placed
/// or found equal, in order.
///
/// Each file is atomic across a power loss: a barrier orders its bytes
/// before its rename, so a destination never holds a torn mix that would
/// stop the next run as a divergence, or lose the local bytes an extension
/// keeps. The files share the directory flushes: once the last is placed,
/// or a write has stopped the run, each directory written into is flushed
/// once, and one per device through the drive's cache, so what was placed
/// is durable when this returns, with one full flush instead of one per
/// file.
pub(crate) fn place_all(
    placements: &[Placement],
    unchanged: Unchanged<'_>,
) -> Result<Vec<PathBuf>, HarnessError> {
    // APFS and casefolded ext4 open `A/x` and `a/x` as one file, so two
    // destinations that differ only in letter case would overwrite each
    // other; they are refused whatever this file system does.
    let mut dests: Vec<String> = placements
        .iter()
        .map(|p| p.dest.to_string_lossy().to_lowercase())
        .collect();
    dests.sort();
    if let Some(pair) = dests.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(HarnessError::DuplicateTreePath {
            path: pair[0].clone(),
        });
    }
    let steps = placements
        .iter()
        .map(|placement| plan(placement, unchanged))
        .collect::<Result<Vec<(Step, Seen)>, HarnessError>>()?;
    let mut renamed = durable::Renamed::default();
    let placed = (|| {
        for (index, (placement, (step, seen))) in placements.iter().zip(steps).enumerate() {
            let dest = &placement.dest;
            match step {
                Step::Keep => {}
                Step::Replace => {
                    durable::write_ordered_checked(
                        dest,
                        |w| copy_into(&placement.staged, w, 0),
                        || seen.still(dest),
                        &mut renamed,
                    )?;
                }
                Step::Extend { view_len } => {
                    durable::write_ordered_checked(
                        dest,
                        |w| {
                            copy_into(dest, w, 0)?;
                            copy_into(&placement.staged, w, view_len)
                        },
                        || seen.still(dest),
                        &mut renamed,
                    )?;
                }
            }
            crashpoint("place-after-file", index);
        }
        Ok::<(), HarnessError>(())
    })();
    // What was placed before a write stopped the run is made durable too.
    let flushed = renamed.flush();
    placed?;
    flushed?;
    Ok(placements.iter().map(|p| p.dest.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::Flush;
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
    fn destinations_that_differ_only_in_case_are_refused_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let upper = dir.path().join("out/Memory.md");
        let lower = dir.path().join("out/memory.md");
        let err = place_all(
            &[
                placement(dir.path(), "a", &upper, b"a", Content::Opaque),
                placement(dir.path(), "b", &lower, b"b", Content::Opaque),
            ],
            &never_left,
        )
        .unwrap_err();
        assert!(
            matches!(err, HarnessError::DuplicateTreePath { .. }),
            "{err}"
        );
        assert!(!dir.path().join("out").exists());
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

    /// A harness that starts after the running-harness guard and appends
    /// a turn once restore has checked the transcript must not have that
    /// turn replaced by the staged copy.
    #[test]
    fn a_destination_written_after_it_was_planned_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("t.jsonl");
        let memory = dir.path().join("MEMORY.md");
        fs::write(&transcript, b"one\n").unwrap();
        fs::write(&memory, b"old\n").unwrap();
        let mut rewritable = placement(dir.path(), "m", &memory, b"new\n", Content::Opaque);
        rewritable.mutability = Mutability::Replaceable;
        let placements = [
            placement(dir.path(), "t", &transcript, b"one\ntwo\n", Content::Opaque),
            rewritable,
        ];
        // Asked while the second file is planned, after the first was: the
        // live turn lands between the transcript's plan and its write.
        let live_turn = |_: &Path| -> Result<bool, HarnessError> {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .unwrap();
            file.write_all(b"live\n").unwrap();
            Ok(true)
        };
        let err = place_all(&placements, &live_turn).unwrap_err();
        assert!(
            matches!(err, HarnessError::StateChanged { ref path } if path == &transcript),
            "{err}"
        );
        assert_eq!(fs::read(&transcript).unwrap(), b"one\nlive\n");
        assert_eq!(fs::read(&memory).unwrap(), b"old\n");
    }

    /// Each placed file keeps its barrier before its rename, and the restore
    /// ends with one flush of each directory it wrote into, the last
    /// through the drive's cache. Before, each file was a barrier and a full
    /// flush of about 5.5 ms on APFS.
    #[test]
    fn a_restore_orders_each_file_and_flushes_the_drive_once() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(home.join("p")).unwrap();
        let placements = [
            placement(
                dir.path(),
                "a",
                &home.join("p/a.jsonl"),
                b"a\n",
                Content::Opaque,
            ),
            placement(
                dir.path(),
                "b",
                &home.join("p/b.jsonl"),
                b"b\n",
                Content::Opaque,
            ),
            placement(
                dir.path(),
                "c",
                &home.join("p/q/c.txt"),
                b"c\n",
                Content::Opaque,
            ),
        ];
        let (placed, log) = durable::record::flushes(|| place_all(&placements, &never_left));
        assert_eq!(placed.unwrap().len(), 3);
        let at = |how, place: &str| (how, place.to_owned());
        assert_eq!(
            durable::record::shape(&log, dir.path()),
            vec![
                at(Flush::Barrier, "tmp:home/p"),
                at(Flush::Barrier, "tmp:home/p"),
                at(Flush::Plain, "home/p"),
                at(Flush::Barrier, "tmp:home/p/q"),
                at(Flush::Plain, "home/p"),
                at(Flush::Full, "home/p/q"),
            ]
        );
        assert_eq!(fs::read(home.join("p/q/c.txt")).unwrap(), b"c\n");
    }

    /// A restore of one file costs what an atomic write does: its barrier
    /// and one full flush of its directory, nothing more.
    #[test]
    fn a_restore_of_one_file_flushes_as_an_atomic_write_does() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("t.jsonl");
        let (placed, log) = durable::record::flushes(|| place_bytes(&dest, dir.path(), b"one\n"));
        placed.unwrap();
        assert_eq!(
            durable::record::shape(&log, dir.path()),
            vec![
                (Flush::Barrier, "tmp:".to_owned()),
                (Flush::Full, String::new())
            ]
        );
    }

    /// A destination that changed after it was planned stops the restore
    /// after the files before it were placed, and those are flushed through
    /// the drive's cache before the error returns, as they were before.
    #[test]
    fn a_restore_stopped_part_way_flushes_what_it_placed() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.jsonl");
        let second = dir.path().join("second.jsonl");
        let memory = dir.path().join("MEMORY.md");
        fs::write(&second, b"one\n").unwrap();
        fs::write(&memory, b"old\n").unwrap();
        let mut rewritable = placement(dir.path(), "m", &memory, b"new\n", Content::Opaque);
        rewritable.mutability = Mutability::Replaceable;
        let placements = [
            placement(dir.path(), "f", &first, b"new\n", Content::Opaque),
            placement(dir.path(), "s", &second, b"one\ntwo\n", Content::Opaque),
            rewritable,
        ];
        // Asked while the last file is planned: a live turn lands in the
        // second file after its plan and before its write.
        let live_turn = |_: &Path| -> Result<bool, HarnessError> {
            let mut file = fs::OpenOptions::new().append(true).open(&second).unwrap();
            file.write_all(b"live\n").unwrap();
            Ok(true)
        };
        let (placed, log) = durable::record::flushes(|| place_all(&placements, &live_turn));
        let err = placed.unwrap_err();
        assert!(
            matches!(err, HarnessError::StateChanged { ref path } if path == &second),
            "{err}"
        );
        assert_eq!(fs::read(&first).unwrap(), b"new\n");
        assert_eq!(fs::read(&memory).unwrap(), b"old\n");
        let at = |how, place: &str| (how, place.to_owned());
        assert_eq!(
            durable::record::shape(&log, dir.path()),
            vec![
                at(Flush::Barrier, "tmp:"),
                at(Flush::Barrier, "tmp:"),
                at(Flush::Full, ""),
            ]
        );
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
