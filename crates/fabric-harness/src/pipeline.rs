//! The capture and restore steps every adapter shares.
//!
//! Invariant: every adapter captures and restores through these functions,
//! so the rules that keep secrets home and local bytes safe are written
//! once. Capture stages the sources (denylist, then redaction or refusal),
//! builds the manifest, records the departure of the rewritable files and
//! writes the manifest last. Restore refuses a manifest of another harness
//! kind, workspace path or home (ADR 0011); then, for every entry, a path
//! on the denylist, a path the adapter never captures, and a content kind
//! or mutability other than the adapter's own rule for that path, and a
//! destination that is a symlink or lies below one under the home, since a
//! rename or a new directory there would write through it; then any staged
//! file whose digest does not match. Only after all of that does it place
//! files, all or nothing. An adapter supplies only what differs: its
//! sources, and where a tree path lands on this node.

#![cfg_attr(
    not(all(feature = "acp", feature = "claude", feature = "codex")),
    allow(
        dead_code,
        reason = "parts serve only some adapters; the default build checks all"
    )
)]

use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use tracing::warn;

use fabric_core::harness::HarnessKind;
use fabric_core::path::AbsPath;
use fabric_core::session::SessionId;

use crate::crashpoint::crashpoint;
use crate::denylist;
use crate::departure::{Departure, Records};
use crate::error::{HarnessError, Input, IoOp};
use crate::harness::Restored;
use crate::manifest::{Carried, Content, Mutability, StateManifest};
use crate::place::{self, Placement};
use crate::redact::RedactionCounts;
use crate::stage::{self, Source};

/// Checks that `workspace` is an absolute directory and returns its
/// physical path, parsed once: the manifest, the Claude project key, the
/// Codex trust key and the ACP `cwd` all use this one value.
pub(crate) fn physical_workspace(workspace: &Path) -> Result<AbsPath, HarnessError> {
    let bad = || HarnessError::BadWorkspace {
        path: workspace.to_path_buf(),
    };
    if !workspace.is_absolute() {
        return Err(bad());
    }
    let physical = std::fs::canonicalize(workspace)
        .map_err(|e| HarnessError::io(IoOp::Canonicalize, workspace, e))?;
    if !physical.is_dir() {
        return Err(bad());
    }
    let text = physical.to_str().ok_or_else(|| HarnessError::NotUtf8 {
        path: physical.clone(),
    })?;
    AbsPath::parse(text).map_err(|e| HarnessError::Malformed {
        what: Input::WorkspacePath,
        path: physical.clone(),
        reason: e.to_string(),
    })
}

/// The home or workspace path as a file system path.
pub(crate) fn fs_path(path: &AbsPath) -> &Path {
    Path::new(path.as_str())
}

/// Lists the regular files under `dir`, relative paths in sorted order.
/// A symlink is an error: a harness state tree has none, and following one
/// would carry or overwrite something outside the tree.
pub(crate) fn walk(dir: &Path) -> Result<Vec<PathBuf>, HarnessError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| HarnessError::io(IoOp::Walk, &d, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| HarnessError::io(IoOp::Walk, &d, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| HarnessError::io(IoOp::Walk, &path, e))?;
            if kind.is_symlink() {
                return Err(HarnessError::SymlinkInState { path });
            }
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(crate::durable::is_temp_name)
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
        what: Input::StatePath,
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
                .ok_or_else(|| HarnessError::NotUtf8 {
                    path: rel.to_path_buf(),
                })
        })
        .collect()
}

/// Runs a blocking capture or restore body off the async runtime.
pub(crate) async fn blocking<T, F>(body: F) -> Result<T, HarnessError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HarnessError> + Send + 'static,
{
    tokio::task::spawn_blocking(body)
        .await
        .map_err(|source| HarnessError::BlockingTask { source })?
}

/// Who captures: the values every manifest records.
pub(crate) struct Capturer<'a> {
    /// The harness that captures.
    pub kind: HarnessKind,
    /// The session captured.
    pub session: &'a SessionId,
    /// The physical workspace path.
    pub root: &'a AbsPath,
    /// The home the adapter serves.
    pub home: &'a AbsPath,
    /// The installed version, when known.
    pub version: Option<String>,
    /// Where this node records its rewritable files; `None` for an adapter
    /// whose files only grow.
    pub departure: Option<&'a Records>,
}

/// Stages `sources` into `out`, records the rewritable ones as a pending
/// departure and writes the manifest last, so a tree with a manifest is
/// whole and its pending record exists.
pub(crate) fn capture(
    capturer: Capturer<'_>,
    sources: &[Source],
    mut counts: RedactionCounts,
    out: &Path,
) -> Result<StateManifest, HarnessError> {
    let carried = stage::stage(sources, out, &mut counts)?;
    let manifest = StateManifest::new(
        capturer.kind,
        capturer.session.clone(),
        capturer.root.clone(),
        capturer.home.clone(),
        capturer.version,
        carried,
        &counts,
    )?;
    if let Some(records) = capturer.departure {
        records.write_pending(sources, manifest.digest()?)?;
        crashpoint("capture-after-departure", 0);
    }
    manifest.write(out)?;
    Ok(manifest)
}

/// Reads the manifest under `state` and refuses one captured by another
/// kind of harness, under another workspace path or under another home:
/// under ADR 0011 the target is the captured place, and the harness keys
/// its state to those exact strings.
pub(crate) fn open(
    state: &Path,
    kind: &HarnessKind,
    root: &AbsPath,
    home: &AbsPath,
) -> Result<StateManifest, HarnessError> {
    let manifest = StateManifest::read(state)?;
    if manifest.kind() != kind {
        return Err(HarnessError::WrongHarness {
            captured: manifest.kind().clone(),
            restoring: kind.clone(),
        });
    }
    if manifest.root() != root {
        return Err(HarnessError::RootMismatch {
            from: manifest.root().clone(),
            to: root.clone(),
        });
    }
    if manifest.home() != home {
        return Err(HarnessError::HomeMismatch {
            from: manifest.home().clone(),
            to: home.clone(),
        });
    }
    Ok(manifest)
}

/// Confirms that the capture which produced `manifest` left this node, so
/// restore may replace the rewritable files it recorded. `records` is
/// `None` for an adapter that keeps none.
pub(crate) fn confirm_departure(
    manifest: &StateManifest,
    kind: &HarnessKind,
    records: Option<&Records>,
) -> Result<(), HarnessError> {
    if manifest.kind() != kind {
        return Err(HarnessError::WrongHarness {
            captured: manifest.kind().clone(),
            restoring: kind.clone(),
        });
    }
    match records {
        Some(records) => records.confirm(manifest.session(), manifest.digest()?),
        None => Ok(()),
    }
}

/// Refuses `dest` when it, or any directory between `home` and it, is a
/// symlink. `home` itself may be one; what the adapter owns starts below it.
fn refuse_symlinks(home: &Path, dest: &Path) -> Result<(), HarnessError> {
    let below = dest
        .strip_prefix(home)
        .map_err(|_| HarnessError::NotUnderHome {
            path: dest.to_path_buf(),
            home: home.to_path_buf(),
        })?;
    let mut at = home.to_path_buf();
    for part in below.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(HarnessError::SymlinkInState { path: at });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(HarnessError::io(IoOp::Read, &at, e)),
        }
    }
    Ok(())
}

/// Where a tree path lands on this node and how the adapter treats it.
pub(crate) struct Destination {
    /// The file on this node.
    pub path: PathBuf,
    /// The content kind the adapter captures that path as.
    pub content: Content,
    /// How the harness changes that file.
    pub mutability: Mutability,
}

/// Places the files of `manifest`, staged under `state`, where
/// `destination` says. `destination` answers `None` for a path the adapter
/// never captures. Nothing is written until every entry has passed every
/// check; `departure` holds the confirmed record that says which
/// rewritable files this node left untouched.
pub(crate) fn restore<F>(
    manifest: StateManifest,
    state: &Path,
    departure: Option<&Records>,
    mut destination: F,
) -> Result<Restored, HarnessError>
where
    F: FnMut(&str, &Carried) -> Result<Option<Destination>, HarnessError>,
{
    let home = fs_path(manifest.home()).to_path_buf();
    let mut placements = Vec::with_capacity(manifest.files().len());
    for file in manifest.files() {
        let rel = file.rel_path.as_str();
        if let Some(rule) = denylist::check(rel) {
            return Err(HarnessError::Denied {
                path: rel.to_owned(),
                rule,
            });
        }
        let Some(dest) = destination(rel, file)? else {
            return Err(HarnessError::UnexpectedFile {
                kind: manifest.kind().clone(),
                path: rel.to_owned(),
            });
        };
        if dest.content != file.content || dest.mutability != file.mutability {
            return Err(HarnessError::EntryMismatch {
                kind: manifest.kind().clone(),
                path: rel.to_owned(),
            });
        }
        refuse_symlinks(&home, &dest.path)?;
        placements.push(Placement {
            staged: state.join(rel),
            dest: dest.path,
            content: dest.content,
            mutability: dest.mutability,
        });
    }
    stage::verify(state, manifest.files())?;
    let departure = match departure {
        Some(records) => records.active()?,
        None => Departure::default(),
    };
    let placed = place::place_all(&placements, &|dest| departure.unchanged(dest))?;
    Ok(Restored { manifest, placed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_temp_files_and_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"a").unwrap();
        std::fs::write(dir.path().join(".a.fabric-tmp-1-0"), b"partial").unwrap();
        assert_eq!(walk(dir.path()).unwrap(), vec![PathBuf::from("a")]);
        std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("link")).unwrap();
        assert!(matches!(
            walk(dir.path()),
            Err(HarnessError::SymlinkInState { .. })
        ));
    }
}
