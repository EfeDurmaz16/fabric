//! Putting a restored tree in place of an existing one.
//!
//! A pull onto a tree with content restores beside it, into the hidden
//! `.<tree>.<NAME>-incoming`, which the store's restore makes through its
//! own staging rename, so it exists only whole. Then the old tree is
//! renamed to `<tree>.<NAME>-previous` and the incoming tree onto the tree
//! path. A pull cut between the two renames leaves no tree at the path, the
//! old one aside and the new one whole; the next pull, or any sync of the
//! tree, finishes the second rename first. The tree path never holds part
//! of a tree. The snapshot the pull writes is recorded as pending before
//! it writes, so the sync that finishes a cut pull also knows what the tree
//! holds.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::{NAME, SyncError, faults, io_err};

/// The paths a replacing pull uses beside `tree`.
pub(crate) struct Beside {
    /// Where the new tree is restored.
    pub(crate) incoming: PathBuf,
    /// Where the tree it replaced is kept.
    pub(crate) previous: PathBuf,
}

impl Beside {
    pub(crate) fn of(tree: &Path) -> Result<Beside, SyncError> {
        let (Some(parent), Some(name)) = (tree.parent(), tree.file_name()) else {
            return Err(SyncError::BadTree {
                path: tree.to_owned(),
                why: "it has no parent directory".to_owned(),
            });
        };
        let name = name.to_string_lossy();
        Ok(Beside {
            incoming: parent.join(format!(".{name}.{NAME}-incoming")),
            previous: parent.join(format!("{name}.{NAME}-previous")),
        })
    }
}

/// True when `path` is absent or an empty directory.
pub(crate) fn is_empty(path: &Path) -> Result<bool, SyncError> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => Ok(false),
        Err(e) => Err(io_err(path)(e)),
    }
}

fn exists(path: &Path) -> Result<bool, SyncError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_err(path)(e)),
    }
}

/// Finishes or undoes what a cut pull left beside `tree`, and says
/// whether the tree path now holds the snapshot the pull was writing.
///
/// `pending` is true when the pull recorded that snapshot before it
/// started to write. With the tree path empty and the old tree aside, the
/// incoming tree is whole and goes in place. Otherwise an incoming tree is
/// one the pull never put in place, and is removed; the tree at the path
/// is untouched. With no incoming tree, a pending pull whose tree path has
/// content finished: its last rename, or its restore onto an absent tree,
/// had happened.
pub(crate) fn recover(tree: &Path, pending: bool) -> Result<bool, SyncError> {
    let beside = Beside::of(tree)?;
    if exists(&beside.incoming)? {
        if !exists(tree)? && exists(&beside.previous)? {
            fs::rename(&beside.incoming, tree).map_err(io_err(tree))?;
            sync_parent(tree)?;
            return Ok(pending);
        }
        remove_tree(&beside.incoming)?;
        return Ok(false);
    }
    Ok(pending && !is_empty(tree)?)
}

/// Puts the restored tree at `beside.incoming` in place of `tree`, keeping
/// the old tree at `beside.previous`.
pub(crate) fn swap_in(tree: &Path, beside: &Beside) -> Result<(), SyncError> {
    faults::hit("pull.before_swap");
    fs::rename(tree, &beside.previous).map_err(io_err(tree))?;
    faults::hit("pull.mid_swap");
    fs::rename(&beside.incoming, tree).map_err(io_err(tree))?;
    sync_parent(tree)
}

fn sync_parent(path: &Path) -> Result<(), SyncError> {
    let parent = path.parent().unwrap_or(Path::new("/"));
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(io_err(parent))
}

/// Removes the tree at `path`, giving its owner write and search on every
/// directory first, since a restored tree keeps read-only directories.
/// Links are removed, never followed.
pub(crate) fn remove_tree(path: &Path) -> Result<(), SyncError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_err(path)(e)),
    };
    if !meta.is_dir() {
        return fs::remove_file(path).map_err(io_err(path));
    }
    open_up(path)?;
    fs::remove_dir_all(path).map_err(io_err(path))
}

fn open_up(dir: &Path) -> Result<(), SyncError> {
    let meta = fs::symlink_metadata(dir).map_err(io_err(dir))?;
    let mode = meta.permissions().mode();
    if mode & 0o700 != 0o700 {
        fs::set_permissions(dir, fs::Permissions::from_mode(mode | 0o700)).map_err(io_err(dir))?;
    }
    for entry in fs::read_dir(dir).map_err(io_err(dir))? {
        let entry = entry.map_err(io_err(dir))?;
        if entry.file_type().map_err(io_err(entry.path()))?.is_dir() {
            open_up(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_cut_between_the_renames_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("app");
        let beside = Beside::of(&tree).unwrap();
        fs::create_dir(&beside.previous).unwrap();
        fs::create_dir(&beside.incoming).unwrap();
        fs::write(beside.incoming.join("new"), "new").unwrap();
        assert!(recover(&tree, true).unwrap());
        assert_eq!(fs::read_to_string(tree.join("new")).unwrap(), "new");
        assert!(!beside.incoming.exists());
    }

    #[test]
    fn an_incoming_tree_never_put_in_place_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("app");
        let beside = Beside::of(&tree).unwrap();
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("old"), "old").unwrap();
        fs::create_dir_all(beside.incoming.join("ro")).unwrap();
        fs::set_permissions(
            beside.incoming.join("ro"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        assert!(!recover(&tree, true).unwrap());
        assert_eq!(fs::read_to_string(tree.join("old")).unwrap(), "old");
        assert!(!beside.incoming.exists());
    }
}
