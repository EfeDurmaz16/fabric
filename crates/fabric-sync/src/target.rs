//! Where a tree is synced to: a directory on this machine or a path behind
//! SSH, each a store with one head per tree name.
//!
//! A target directory holds `store/`, a `fabric-store` store, and
//! `heads/<name>`, one line `<job> <snapshot>` per tree. A head is replaced
//! by a rename while the store's lock is held, so two pushes to one target
//! take turns and neither loses the other's head unseen.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fabric_core::id::{JobId, SnapshotId};
use fabric_store::objects::ChunkParams;
use fabric_store::store::{Store, StoreError};

use crate::objects::{self, Objects};
use crate::{SyncError, io_err};

/// A parsed `TARGET` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A directory on this machine.
    Dir(PathBuf),
    /// A path on another machine, reached with `ssh`.
    Ssh {
        /// `host` or `user@host`.
        host: String,
        /// The port, when given in an `ssh://` address.
        port: Option<u16>,
        /// The path there; a leading `~/` is the remote user's home.
        path: String,
    },
}

impl Target {
    /// Reads `ssh://[user@]host[:port]/path`, `[user@]host:path`, or a path.
    ///
    /// A word whose first `:` comes before any `/` is an SSH target, as for
    /// `scp`; `./host:dir` names a local directory with a colon in it.
    pub fn parse(text: &str) -> Result<Target, SyncError> {
        let bad = || SyncError::BadTarget {
            text: text.to_owned(),
        };
        if text.is_empty() {
            return Err(bad());
        }
        if let Some(rest) = text.strip_prefix("ssh://") {
            let (authority, path) = rest.split_once('/').ok_or_else(bad)?;
            let (host, port) = match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port.parse::<u16>().map_err(|_| bad())?)),
                None => (authority, None),
            };
            let path = if let Some(home) = path.strip_prefix('~') {
                format!("~{home}")
            } else {
                format!("/{path}")
            };
            return Self::ssh(host, port, path).ok_or_else(bad);
        }
        let colon = text.find(':');
        let slash = text.find('/');
        if let Some(colon) = colon
            && slash.is_none_or(|slash| colon < slash)
        {
            let (host, path) = (&text[..colon], &text[colon + 1..]);
            let path = if path.is_empty() { "." } else { path };
            return Self::ssh(host, None, path.to_owned()).ok_or_else(bad);
        }
        Ok(Target::Dir(PathBuf::from(text)))
    }

    /// An SSH target, when the host cannot be read as an option of `ssh`.
    fn ssh(host: &str, port: Option<u16>, path: String) -> Option<Target> {
        let plain = !host.is_empty()
            && !host.starts_with('-')
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-@[]:".contains(c));
        (plain && !path.is_empty()).then(|| Target::Ssh {
            host: host.to_owned(),
            port,
            path,
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Dir(path) => write!(f, "{}", path.display()),
            Target::Ssh {
                host,
                port: Some(port),
                path,
            } => write!(f, "ssh://{host}:{port}/{}", path.trim_start_matches('/')),
            Target::Ssh { host, path, .. } => write!(f, "{host}:{path}"),
        }
    }
}

/// The name a tree has on a target, one file name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TreeName(String);

impl TreeName {
    /// Letters, digits, `.`, `_` and `-`, at most 100, not starting with `.`.
    pub fn parse(text: &str) -> Result<TreeName, SyncError> {
        let ok = !text.is_empty()
            && text.len() <= 100
            && !text.starts_with('.')
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
        if ok {
            Ok(TreeName(text.to_owned()))
        } else {
            Err(SyncError::BadName {
                text: text.to_owned(),
            })
        }
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TreeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a target's head for a name says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Head {
    /// The job every snapshot of the tree belongs to.
    pub(crate) job: JobId,
    /// The last snapshot pushed.
    pub(crate) snapshot: SnapshotId,
}

impl Head {
    pub(crate) fn line(self) -> String {
        format!("{} {}\n", self.job, self.snapshot)
    }

    pub(crate) fn read(text: &str) -> Option<Head> {
        let (job, snapshot) = text.trim_end_matches('\n').split_once(' ')?;
        Some(Head {
            job: job.parse().ok()?,
            snapshot: snapshot.parse().ok()?,
        })
    }
}

/// The heads of a target.
pub(crate) trait Heads {
    /// The head for `name`, if any.
    fn head(&mut self, name: &TreeName) -> Result<Option<Head>, SyncError>;
    /// Moves the head for `name` to `new` if it is still `expected`.
    fn set_head(
        &mut self,
        name: &TreeName,
        expected: Option<SnapshotId>,
        new: Head,
    ) -> Result<(), SyncError>;
}

/// A target: its objects and its heads.
pub(crate) trait Remote: Objects + Heads {
    /// Lets the other side go; an SSH side is asked to exit and waited for.
    fn close(self: Box<Self>) -> Result<(), SyncError>;
}

/// A target directory, opened with its store's lock held.
pub(crate) struct DirTarget {
    store: Store,
    heads: PathBuf,
}

impl DirTarget {
    /// Opens the target at `dir`; with `create`, makes it when absent.
    pub(crate) fn open(dir: &Path, create: bool) -> Result<DirTarget, SyncError> {
        let root = dir.join("store");
        let store = match Store::open(&root) {
            Ok(store) => store,
            Err(StoreError::NotAStore { .. }) if create => {
                Store::create(&root, ChunkParams::DEFAULT)?
            }
            Err(StoreError::NotAStore { .. }) => {
                return Err(SyncError::BadTree {
                    path: dir.to_owned(),
                    why: "it holds no synced tree".to_owned(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        let heads = dir.join("heads");
        fs::create_dir_all(&heads).map_err(io_err(&heads))?;
        Ok(DirTarget { store, heads })
    }
}

impl Objects for DirTarget {
    fn has_snapshot(&mut self, id: SnapshotId) -> Result<bool, SyncError> {
        objects::has_snapshot(&self.store, id)
    }

    fn manifest_chunks(
        &mut self,
        id: SnapshotId,
    ) -> Result<(u64, Vec<fabric_core::id::ChunkId>), SyncError> {
        Ok(self.store.manifest_chunks(id)?)
    }

    fn missing(
        &mut self,
        ids: &[fabric_core::id::ChunkId],
    ) -> Result<Vec<fabric_core::id::ChunkId>, SyncError> {
        objects::missing(&self.store, ids)
    }

    fn get(&mut self, ids: &[fabric_core::id::ChunkId]) -> Result<Vec<Vec<u8>>, SyncError> {
        objects::get(&self.store, ids)
    }

    fn put(&mut self, chunks: &[(fabric_core::id::ChunkId, Vec<u8>)]) -> Result<(), SyncError> {
        objects::put(&self.store, chunks)
    }

    fn publish(&mut self, manifest: &[u8]) -> Result<SnapshotId, SyncError> {
        objects::publish(&mut self.store, manifest)
    }
}

impl Heads for DirTarget {
    fn head(&mut self, name: &TreeName) -> Result<Option<Head>, SyncError> {
        let path = self.heads.join(name.as_str());
        match fs::read_to_string(&path) {
            Ok(text) => Head::read(&text)
                .map(Some)
                .ok_or_else(|| SyncError::Protocol {
                    reason: format!("{} does not read as a head", path.display()),
                }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(&path)(e)),
        }
    }

    fn set_head(
        &mut self,
        name: &TreeName,
        expected: Option<SnapshotId>,
        new: Head,
    ) -> Result<(), SyncError> {
        let now = self.head(name)?;
        if now.map(|head| head.snapshot) != expected {
            return Err(match now {
                Some(head) => SyncError::TargetMoved {
                    name: name.to_string(),
                    head: head.snapshot,
                },
                None => SyncError::Refused {
                    reason: format!("the head of {name} was removed meanwhile"),
                },
            });
        }
        write_atomic(&self.heads.join(name.as_str()), new.line().as_bytes())
    }
}

impl Remote for DirTarget {
    fn close(self: Box<Self>) -> Result<(), SyncError> {
        Ok(())
    }
}

/// Writes `bytes` to `path` through a flushed temp file and a rename, then flushes the directory.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SyncError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut file = fs::File::create(&tmp).map_err(io_err(&tmp))?;
    file.write_all(bytes).map_err(io_err(&tmp))?;
    file.sync_all().map_err(io_err(&tmp))?;
    drop(file);
    fs::rename(&tmp, path).map_err(io_err(path))?;
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(io_err(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_reads_as_a_path_or_an_ssh_address() {
        let ssh = |host: &str, port, path: &str| Target::Ssh {
            host: host.to_owned(),
            port,
            path: path.to_owned(),
        };
        let cases = [
            ("/srv/sync", Target::Dir("/srv/sync".into())),
            ("./box:dir", Target::Dir("./box:dir".into())),
            ("box:sync", ssh("box", None, "sync")),
            ("me@box:~/sync", ssh("me@box", None, "~/sync")),
            ("box:", ssh("box", None, ".")),
            (
                "ssh://me@box:2222/srv/sync",
                ssh("me@box", Some(2222), "/srv/sync"),
            ),
            ("ssh://box/~/sync", ssh("box", None, "~/sync")),
        ];
        for (text, want) in cases {
            assert_eq!(Target::parse(text).unwrap(), want, "{text}");
        }
        for text in [
            "",
            "-oProxyCommand=x:y",
            "ssh://box",
            "ssh://box:port/x",
            "a b:c",
        ] {
            assert!(Target::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_tree_name_is_one_plain_file_name() {
        for ok in ["app", "my-app_2.0"] {
            assert!(TreeName::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".git", "a/b", "..", "a b", &"x".repeat(101)] {
            assert!(TreeName::parse(bad).is_err(), "{bad}");
        }
    }
}
