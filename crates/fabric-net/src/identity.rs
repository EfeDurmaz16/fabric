//! The node key (ADR 0005).
//!
//! Invariant: the private key lives in one file that only its owner can
//! read, it is written once and never overwritten, and it leaves this module
//! only as the PKCS#8 bytes the TLS layer signs with. The node id is the
//! public key, so two machines never share an id. A crash while the key is
//! being written leaves no key or a whole key, never a torn one, and the
//! next start works either way. It can also leave a temporary copy of the
//! key beside it, mode 0600 like the key, which `save` describes.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use fabric_core::id::NodeId;
use rcgen::{KeyPair, PKCS_ED25519};
use rustls::pki_types::PrivatePkcs8KeyDer;

use crate::checkpoint;
use crate::durable::{sync_parent, temp_sibling};

/// Why a node key could not be made, read or written.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// The key file could not be read or written.
    #[error("key file {path}: {source}")]
    Io {
        /// The key file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The key file is readable by other users; the key may have leaked.
    #[error("key file {path} has mode {mode:o}; it must be 0600")]
    Permissions {
        /// The key file.
        path: PathBuf,
        /// The mode bits found.
        mode: u32,
    },
    /// The key file does not hold an ed25519 key in PKCS#8 form.
    #[error("key file {path} does not hold an ed25519 PKCS#8 key")]
    Malformed {
        /// The key file.
        path: PathBuf,
    },
    /// No key could be generated.
    #[error("key generation: {0}")]
    Generate(#[source] rcgen::Error),
    /// The key's public half is not the 32 bytes of an ed25519 key.
    #[error("the key is not ed25519")]
    NotEd25519,
}

/// This node's ed25519 keypair and the id derived from it.
pub struct NodeIdentity {
    key: KeyPair,
    id: NodeId,
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeIdentity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl NodeIdentity {
    /// Makes a new keypair from the system random source.
    pub fn generate() -> Result<Self, IdentityError> {
        let key = KeyPair::generate_for(&PKCS_ED25519).map_err(IdentityError::Generate)?;
        Self::from_key(key)
    }

    /// Reads the key at `path`, refusing a file other users can read.
    ///
    /// The mode is checked on the handle the key is read from, so it is the
    /// mode of the file read, even if the path changes in between.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let io = |source| IdentityError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut file = fs::File::open(path).map_err(io)?;
        #[cfg(unix)]
        {
            let mode = file.metadata().map_err(io)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(IdentityError::Permissions {
                    path: path.to_path_buf(),
                    mode,
                });
            }
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        let der = PrivatePkcs8KeyDer::from(bytes);
        let key = KeyPair::from_pkcs8_der_and_sign_algo(&der, &PKCS_ED25519).map_err(|_| {
            IdentityError::Malformed {
                path: path.to_path_buf(),
            }
        })?;
        Self::from_key(key)
    }

    /// Reads the key at `path`, or makes one and writes it there if none exists.
    ///
    /// Two processes that race here end with the same key: each writes its
    /// own temporary file, the first hard link to `path` wins, and the other
    /// reads the winner.
    pub fn load_or_generate(path: &Path) -> Result<Self, IdentityError> {
        match Self::load(path) {
            Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            other => return other,
        }
        let fresh = Self::generate()?;
        match fresh.save(path) {
            Ok(()) => Ok(fresh),
            Err(IdentityError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                Self::load(path)
            }
            Err(err) => Err(err),
        }
    }

    /// Writes the key to `path` with mode 0600, refusing to replace an existing file.
    ///
    /// The bytes go to a temporary file that only this call names and are
    /// synced; the final name is then taken with a hard link, which fails
    /// if the file exists; the directory is synced last. A crash can leave
    /// a stray `node.key.<pid>.<clock>.<n>.tmp` behind. It holds a copy of
    /// the secret key, mode 0600 like the key itself, and no start reads
    /// it; delete it, as the key would be deleted, before the key is
    /// replaced or the directory leaves this machine.
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let io = |source| IdentityError::Io {
            path: path.to_path_buf(),
            source,
        };
        let der = self.pkcs8_der();
        let tmp = temp_sibling(path);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let written = (|| {
            let mut file = options.open(&tmp)?;
            file.write_all(der.secret_pkcs8_der())?;
            file.sync_all()?;
            Ok::<(), io::Error>(())
        })();
        checkpoint::reached("identity.after_tmp");
        let linked = written.and_then(|()| fs::hard_link(&tmp, path));
        checkpoint::reached("identity.after_link");
        let _ = fs::remove_file(&tmp);
        linked.map_err(io)?;
        sync_parent(path).map_err(io)
    }

    /// Returns the node id: the public key.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.id
    }

    /// Returns the private key as PKCS#8 for the TLS layer.
    pub(crate) fn pkcs8_der(&self) -> PrivatePkcs8KeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.key.serialize_der())
    }

    /// Returns the keypair for the certificate builder.
    pub(crate) fn key_pair(&self) -> &KeyPair {
        &self.key
    }

    fn from_key(key: KeyPair) -> Result<Self, IdentityError> {
        let public: [u8; 32] = key
            .public_key_raw()
            .try_into()
            .map_err(|_| IdentityError::NotEd25519)?;
        Ok(Self {
            key,
            id: NodeId::from_bytes(public),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_loads_back_with_the_same_id_and_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        let first = NodeIdentity::load_or_generate(&path).unwrap();
        let second = NodeIdentity::load_or_generate(&path).unwrap();
        assert_eq!(first.node_id(), second.node_id());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "node.key"),
            "no temporary file is left behind"
        );
    }

    #[test]
    fn racing_writers_end_with_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let ids: Vec<NodeId> = (0..8)
            .map(|_| {
                let (path, barrier) = (path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    NodeIdentity::load_or_generate(&path).unwrap().node_id()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(ids.iter().all(|id| *id == ids[0]));
        assert_eq!(NodeIdentity::load(&path).unwrap().node_id(), ids[0]);
    }

    #[test]
    fn save_never_replaces_an_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        let kept = NodeIdentity::generate().unwrap();
        kept.save(&path).unwrap();
        let err = NodeIdentity::generate().unwrap().save(&path).unwrap_err();
        assert!(
            matches!(err, IdentityError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(NodeIdentity::load(&path).unwrap().node_id(), kept.node_id());
    }

    #[cfg(unix)]
    #[test]
    fn a_key_other_users_can_read_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        NodeIdentity::generate().unwrap().save(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            NodeIdentity::load(&path),
            Err(IdentityError::Permissions { mode: 0o644, .. })
        ));
    }

    #[test]
    fn bytes_that_are_not_a_key_are_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        options
            .open(&path)
            .unwrap()
            .write_all(b"not a key")
            .unwrap();
        assert!(matches!(
            NodeIdentity::load(&path),
            Err(IdentityError::Malformed { .. })
        ));
    }
}
