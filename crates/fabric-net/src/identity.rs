//! The node key (ADR 0005).
//!
//! Invariant: the private key lives in one file that only its owner can
//! read, it is written once and never overwritten, and it leaves this module
//! only as the PKCS#8 bytes the TLS layer signs with. The node id is the
//! public key, so two machines never share an id.

use std::fmt;
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use fabric_core::id::NodeId;
use rustls::pki_types::PrivatePkcs8KeyDer;

use crate::tls::provider;

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
    /// The system random source failed, so no key could be made.
    #[error("the system random source failed")]
    Random,
    /// The key could not be encoded as PKCS#8.
    #[error("the key could not be encoded as PKCS#8")]
    Encode,
}

/// This node's ed25519 keypair and the id derived from it.
pub struct NodeIdentity {
    key: SigningKey,
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
        let mut seed = [0u8; 32];
        provider()
            .secure_random
            .fill(&mut seed)
            .map_err(|_| IdentityError::Random)?;
        Ok(Self::from_key(SigningKey::from_bytes(&seed)))
    }

    /// Reads the key at `path`, refusing a file other users can read.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        let io = |source| IdentityError::Io {
            path: path.to_path_buf(),
            source,
        };
        let bytes = fs::read(path).map_err(io)?;
        #[cfg(unix)]
        {
            let mode = fs::metadata(path).map_err(io)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(IdentityError::Permissions {
                    path: path.to_path_buf(),
                    mode,
                });
            }
        }
        let key = SigningKey::from_pkcs8_der(&bytes).map_err(|_| IdentityError::Malformed {
            path: path.to_path_buf(),
        })?;
        Ok(Self::from_key(key))
    }

    /// Reads the key at `path`, or makes one and writes it there if none exists.
    ///
    /// Two processes that race here end with the same key: the first write
    /// wins and the second reads it.
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
    /// The bytes go to a sibling temporary file first and are synced; the
    /// final name is then taken with a hard link, which fails if the file
    /// exists, so a crash leaves either no key or a whole key.
    pub fn save(&self, path: &Path) -> Result<(), IdentityError> {
        let io = |source| IdentityError::Io {
            path: path.to_path_buf(),
            source,
        };
        let der = self.pkcs8_der()?;
        let tmp = temp_path(path);
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
        let linked = written.and_then(|()| fs::hard_link(&tmp, path));
        let _ = fs::remove_file(&tmp);
        linked.map_err(io)
    }

    /// Returns the node id: the public key.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.id
    }

    /// Returns the private key as PKCS#8 for the TLS layer.
    pub(crate) fn pkcs8_der(&self) -> Result<PrivatePkcs8KeyDer<'static>, IdentityError> {
        let document = self.key.to_pkcs8_der().map_err(|_| IdentityError::Encode)?;
        Ok(PrivatePkcs8KeyDer::from(document.as_bytes().to_vec()))
    }

    fn from_key(key: SigningKey) -> Self {
        let id = NodeId::from_bytes(key.verifying_key().to_bytes());
        Self { key, id }
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
    name.push(".tmp");
    path.with_file_name(name)
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
        assert!(!temp_path(&path).exists());
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
