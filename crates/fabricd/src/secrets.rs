//! The local secret resolver (ADR 0025): the one resolver of M5, on the
//! node that runs the attempt.
//!
//! Invariant: a value comes from the daemon's environment
//! (`FABRIC_SECRET_<name>`) or from `<config>/secrets/<name>`, a file that
//! is mode 0600 and owned by the daemon's user, read anew at every call
//! and cached never, so a rotated or removed secret excludes the node at
//! the next placement. A value leaves this module only as a
//! [`SecretValue`], which does not serialize, clone or print.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fabric_core::id::JobId;
use fabric_core::managed::secret_ref::ResourceKind;
use fabric_core::managed::{AttemptId, SecretName, SecretRef, SecretValue};

/// Why a secret did not resolve on this node.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// Neither the environment nor a file holds it.
    #[error("secret {name} is not set on this node")]
    Missing {
        /// The secret.
        name: SecretName,
    },
    /// The file is readable by someone else.
    #[error("the file of secret {name} has mode {mode:o}; it must be 0600")]
    FileMode {
        /// The secret.
        name: SecretName,
        /// The mode found.
        mode: u32,
    },
    /// The file belongs to another user.
    #[error("the file of secret {name} is not owned by the daemon's user")]
    FileOwner {
        /// The secret.
        name: SecretName,
    },
    /// The value is not UTF-8, so it cannot be an environment value.
    #[error("the value of secret {name} is not UTF-8")]
    NotUtf8 {
        /// The secret.
        name: SecretName,
    },
    /// The ref's scope does not admit a node.
    #[error("secret {name} is not scoped to a node")]
    OutOfScope {
        /// The secret.
        name: SecretName,
    },
}

/// The daemon's environment, as the resolver reads it.
type Environment = Box<dyn Fn(&str) -> Option<OsString> + Send + Sync>;

/// The local resolver: the daemon's environment and `<config>/secrets`.
pub struct LocalResolver {
    secrets_dir: PathBuf,
    env: Environment,
}

impl std::fmt::Debug for LocalResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LocalResolver({})", self.secrets_dir.display())
    }
}

impl LocalResolver {
    /// The resolver for the node whose configuration directory is `config_dir`.
    #[must_use]
    pub fn open(config_dir: &Path) -> LocalResolver {
        LocalResolver {
            secrets_dir: config_dir.join("secrets"),
            env: Box::new(|key| std::env::var_os(key)),
        }
    }

    /// A resolver over a given environment instead of the process's; the
    /// tests use it because the process environment is shared and setting
    /// it is unsound under Rust 2024.
    #[cfg(test)]
    pub(crate) fn with_environment(config_dir: &Path, vars: Vec<(String, String)>) -> Self {
        LocalResolver {
            secrets_dir: config_dir.join("secrets"),
            env: Box::new(move |key| {
                vars.iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| OsString::from(v))
            }),
        }
    }

    /// Which refs this node cannot resolve right now, in the order given;
    /// read anew every call, cached never.
    #[must_use]
    pub fn unresolved(&self, refs: &[SecretRef]) -> Vec<SecretName> {
        refs.iter()
            .filter(|r| self.lookup(r).is_err())
            .map(|r| r.name.clone())
            .collect()
    }

    /// The value of `secret` for `job`'s `attempt`, from the environment,
    /// else the file, else `Missing`. The job and attempt name whose
    /// resolution this is; the scope's attempt rule is the caller's, who
    /// knows the live grant.
    pub fn resolve(
        &self,
        job: JobId,
        attempt: AttemptId,
        secret: &SecretRef,
    ) -> Result<SecretValue, SecretError> {
        let value = self.lookup(secret)?;
        tracing::debug!(%job, %attempt, name = %secret.name, "resolved a secret for a task");
        Ok(value)
    }

    fn lookup(&self, secret: &SecretRef) -> Result<SecretValue, SecretError> {
        let name = &secret.name;
        if !secret.scope.resource_kinds.contains(&ResourceKind::Node) {
            return Err(SecretError::OutOfScope { name: name.clone() });
        }
        if let Some(value) = (self.env)(&name.env_var()) {
            let text = value
                .into_string()
                .map_err(|_| SecretError::NotUtf8 { name: name.clone() })?;
            return Ok(SecretValue::new(text.into_bytes()));
        }
        let path = self.secrets_dir.join(name.as_str());
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SecretError::Missing { name: name.clone() });
            }
            Err(_) => return Err(SecretError::Missing { name: name.clone() }),
        };
        if !meta.is_file() {
            return Err(SecretError::Missing { name: name.clone() });
        }
        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            return Err(SecretError::FileMode {
                name: name.clone(),
                mode,
            });
        }
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(SecretError::FileOwner { name: name.clone() });
        }
        let bytes =
            std::fs::read(&path).map_err(|_| SecretError::Missing { name: name.clone() })?;
        let text =
            String::from_utf8(bytes).map_err(|_| SecretError::NotUtf8 { name: name.clone() })?;
        // A trailing newline is the editor's, not the secret's.
        let text = text.strip_suffix('\n').unwrap_or(&text).to_owned();
        Ok(SecretValue::new(text.into_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use fabric_core::managed::SecretScope;

    use super::*;

    fn secret(name: &str) -> SecretRef {
        SecretRef {
            name: SecretName::parse(name).unwrap(),
            scope: SecretScope::node_any(),
        }
    }

    fn write_secret(dir: &Path, name: &str, value: &str, mode: u32) {
        let secrets = dir.join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        let path = secrets.join(name);
        std::fs::write(&path, value).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn env_then_file_then_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "TOKEN", "from-file\n", 0o600);
        write_secret(dir.path(), "ONLY_FILE", "file-only", 0o600);
        let resolver = LocalResolver::with_environment(
            dir.path(),
            vec![("FABRIC_SECRET_TOKEN".into(), "from-env".into())],
        );
        let job = JobId::from_bytes([1; 16]);
        let attempt = AttemptId::from_bytes([1; 16]);
        assert_eq!(
            resolver
                .resolve(job, attempt, &secret("TOKEN"))
                .unwrap()
                .expose(),
            b"from-env"
        );
        assert_eq!(
            resolver
                .resolve(job, attempt, &secret("ONLY_FILE"))
                .unwrap()
                .expose(),
            b"file-only"
        );
        assert_eq!(
            resolver
                .resolve(job, attempt, &secret("NOWHERE"))
                .unwrap_err(),
            SecretError::Missing {
                name: SecretName::parse("NOWHERE").unwrap()
            }
        );
        let out_of_scope = SecretRef {
            scope: SecretScope {
                attempt: fabric_core::managed::secret_ref::AttemptScope::Any,
                resource_kinds: Vec::new(),
            },
            ..secret("TOKEN")
        };
        assert!(matches!(
            resolver.resolve(job, attempt, &out_of_scope),
            Err(SecretError::OutOfScope { .. })
        ));
        assert_eq!(
            resolver.unresolved(&[secret("TOKEN"), secret("NOWHERE"), secret("ONLY_FILE")]),
            vec![SecretName::parse("NOWHERE").unwrap()]
        );
    }

    #[test]
    fn a_file_not_0600_or_not_owned_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "LOOSE", "x", 0o644);
        write_secret(dir.path(), "GROUP", "x", 0o640);
        write_secret(dir.path(), "TIGHT", "x", 0o600);
        let resolver = LocalResolver::open(dir.path());
        let job = JobId::from_bytes([1; 16]);
        let attempt = AttemptId::from_bytes([1; 16]);
        assert_eq!(
            resolver
                .resolve(job, attempt, &secret("LOOSE"))
                .unwrap_err(),
            SecretError::FileMode {
                name: SecretName::parse("LOOSE").unwrap(),
                mode: 0o644
            }
        );
        assert!(matches!(
            resolver.resolve(job, attempt, &secret("GROUP")),
            Err(SecretError::FileMode { mode: 0o640, .. })
        ));
        assert!(resolver.resolve(job, attempt, &secret("TIGHT")).is_ok());
        // Ownership: the file is this user's, so the owner check passes;
        // a file of another user cannot be made without privileges, so
        // the rule is asserted on the comparison the resolver makes.
        let meta = std::fs::metadata(dir.path().join("secrets/TIGHT")).unwrap();
        assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
        // The names the resolver cannot resolve are in the order given.
        assert_eq!(
            resolver.unresolved(&[secret("LOOSE"), secret("TIGHT"), secret("GROUP")]),
            vec![
                SecretName::parse("LOOSE").unwrap(),
                SecretName::parse("GROUP").unwrap()
            ]
        );
    }

    #[test]
    fn eligibility_is_read_anew_every_call() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = LocalResolver::open(dir.path());
        let refs = [secret("ROTATED")];
        assert_eq!(resolver.unresolved(&refs).len(), 1, "absent at first");
        write_secret(dir.path(), "ROTATED", "v1", 0o600);
        assert!(
            resolver.unresolved(&refs).is_empty(),
            "present once written"
        );
        std::fs::remove_file(dir.path().join("secrets/ROTATED")).unwrap();
        assert_eq!(resolver.unresolved(&refs).len(), 1, "absent once removed");
        write_secret(dir.path(), "ROTATED", "v2", 0o644);
        assert_eq!(
            resolver.unresolved(&refs).len(),
            1,
            "a loosened file counts as unresolved"
        );
    }

    #[test]
    fn a_value_prints_redacted_and_a_resolver_shows_only_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "TOKEN", "hunter2", 0o600);
        let resolver = LocalResolver::open(dir.path());
        let value = resolver
            .resolve(
                JobId::from_bytes([1; 16]),
                AttemptId::from_bytes([1; 16]),
                &secret("TOKEN"),
            )
            .unwrap();
        assert!(!format!("{value:?}").contains("hunter2"));
        assert!(!format!("{resolver:?}").contains("hunter2"));
    }
}
