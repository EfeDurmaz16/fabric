//! The container of section 3: a directory with `manifest.json` and
//! `blobs/<sha256 hex>`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::manifest::{Manifest, read_manifest};
use crate::refusal::{ContainerCode, ContainerRefusal};
use crate::sha256::{hex, sha256};

/// The manifest's file name inside a container.
pub const MANIFEST_FILE: &str = "manifest.json";
/// The directory of blobs inside a container.
pub const BLOBS_DIR: &str = "blobs";

/// A container that passed every rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// The manifest.
    pub manifest: Manifest,
    /// The attachments whose blob is present and matches, in manifest order.
    pub present: Vec<String>,
    /// The attachments whose blob is absent, in manifest order.
    pub detached: Vec<String>,
}

fn refusal(code: ContainerCode, path: impl Into<String>) -> ContainerRefusal {
    ContainerRefusal {
        code,
        path: path.into(),
        field: String::new(),
    }
}

fn unreadable(path: &str) -> impl FnOnce(io::Error) -> ContainerRefusal + '_ {
    move |_| refusal(ContainerCode::Unreadable, path)
}

/// Every entry under `dir` as a `/`-separated relative path, sorted by bytes.
/// Symbolic links are listed and not followed.
fn entries(dir: &Path) -> Result<Vec<(String, fs::FileType)>, ContainerRefusal> {
    let mut out = Vec::new();
    let mut pending: Vec<(PathBuf, String)> = vec![(dir.to_owned(), String::new())];
    while let Some((path, rel)) = pending.pop() {
        let listing = fs::read_dir(&path).map_err(unreadable(&rel))?;
        for entry in listing {
            let entry = entry.map_err(unreadable(&rel))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let kind = entry.file_type().map_err(unreadable(&child))?;
            if kind.is_dir() {
                pending.push((entry.path(), child.clone()));
            }
            out.push((child, kind));
        }
    }
    out.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(out)
}

/// Reads the container at `dir` (section 3 and the container rules of section 10).
pub fn read_container(dir: &Path) -> Result<Container, ContainerRefusal> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let is_file = fs::symlink_metadata(&manifest_path).is_ok_and(|m| m.file_type().is_file());
    if !is_file {
        return Err(refusal(ContainerCode::ManifestMissing, MANIFEST_FILE));
    }
    let bytes = read_capped(&manifest_path).map_err(unreadable(MANIFEST_FILE))?;
    let manifest = read_manifest(&bytes).map_err(|r| ContainerRefusal {
        code: ContainerCode::Manifest(r.code),
        path: MANIFEST_FILE.to_owned(),
        field: r.field,
    })?;
    let attachments = &manifest.parts().attachments;
    let mut found = Vec::new();
    for (rel, kind) in entries(dir)? {
        if rel == MANIFEST_FILE || (rel == BLOBS_DIR && kind.is_dir()) {
            continue;
        }
        let blob = rel
            .strip_prefix("blobs/")
            .filter(|name| !name.contains('/') && kind.is_file());
        let named: Vec<_> = blob
            .map(|name| attachments.iter().filter(|a| a.digest == name).collect())
            .unwrap_or_default();
        let Some(name) = blob.filter(|_| !named.is_empty()) else {
            return Err(refusal(ContainerCode::BlobUnlisted, rel));
        };
        let data = fs::read(dir.join(&rel)).map_err(unreadable(&rel))?;
        let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if hex(&sha256(&data)) != name || named.iter().any(|a| a.bytes != len) {
            return Err(refusal(ContainerCode::BlobMismatch, rel));
        }
        found.push(name.to_owned());
    }
    let (present, detached) = attachments
        .iter()
        .partition::<Vec<_>, _>(|a| found.contains(&a.digest));
    Ok(Container {
        present: present.into_iter().map(|a| a.name.clone()).collect(),
        detached: detached.into_iter().map(|a| a.name.clone()).collect(),
        manifest,
    })
}

/// Reads at most one byte over the manifest cap, so a huge file is refused unparsed.
fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let cap = crate::check::MANIFEST_MAX_BYTES as u64 + 1;
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(cap).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Writes `manifest` and `blobs` as a container at `dir`, which must not
/// exist, and reads it back. A blob the manifest does not name is refused
/// by the read, so nothing unnamed is ever left in a container.
pub fn write_container(
    dir: &Path,
    manifest: &Manifest,
    blobs: &[Vec<u8>],
) -> Result<Container, WriteError> {
    fs::create_dir(dir).map_err(WriteError::Io)?;
    let blob_dir = dir.join(BLOBS_DIR);
    fs::create_dir(&blob_dir).map_err(WriteError::Io)?;
    for blob in blobs {
        fs::write(blob_dir.join(hex(&sha256(blob))), blob).map_err(WriteError::Io)?;
    }
    fs::write(dir.join(MANIFEST_FILE), manifest.bytes()).map_err(WriteError::Io)?;
    read_container(dir).map_err(WriteError::Refused)
}

/// Why [`write_container`] failed.
#[derive(Debug)]
pub enum WriteError {
    /// The file system refused a write.
    Io(io::Error),
    /// The written container does not read back.
    Refused(ContainerRefusal),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Io(e) => write!(f, "writing the container: {e}"),
            WriteError::Refused(r) => write!(f, "the written container reads back refused: {r}"),
        }
    }
}

impl std::error::Error for WriteError {}
