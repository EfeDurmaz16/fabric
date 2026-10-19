//! Durable file writes shared by the node key and the peer book.
//!
//! Invariant: a writer builds new content in a sibling temporary file that
//! only it names, so two writers, in one process or in two, never touch the
//! same temporary file, and a crash leaves at most a stray temporary file
//! next to a whole final file. The directory is synced after the final name
//! changes, so the name itself survives a power loss.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The suffix of every temporary file this module names.
pub(crate) const TEMP_SUFFIX: &str = ".tmp";

/// Names a temporary sibling of `path` that no other writer names: process id, clock and a counter.
pub(crate) fn temp_sibling(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map_or_else(Default::default, OsStr::to_os_string);
    name.push(format!(
        ".{}.{nanos:x}.{sequence}{TEMP_SUFFIX}",
        std::process::id()
    ));
    path.with_file_name(name)
}

/// Syncs the directory that holds `path`, so a rename or link into it is durable.
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_temporary_names_for_one_path_differ_and_sit_beside_it() {
        let path = Path::new("/data/node.key");
        let (a, b) = (temp_sibling(path), temp_sibling(path));
        assert_ne!(a, b);
        for tmp in [&a, &b] {
            assert_eq!(tmp.parent(), path.parent());
            let name = tmp.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("node.key.") && name.ends_with(TEMP_SUFFIX));
        }
    }
}
