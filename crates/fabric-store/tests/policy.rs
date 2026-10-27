//! The store's capture policy against the core's `SecretPolicy` (M2-03):
//! a path the core carries, the store carries too. Ignored until the store
//! uses the core's policy.

use fabric_core::path::RelPath;
use fabric_store::policy::IgnorePolicy;

/// Ordinary source files whose names start like key or credential files,
/// a dependency's `credentials.js` and committed environment templates are
/// work, as `fabric_core::secret` says; the exact key and credential names
/// stay secrets.
#[test]
#[ignore = "M2-03: expected failure until fixed"]
fn the_store_classifies_secrets_as_the_core_does() {
    let policy = IgnorePolicy::default();
    for path in [
        "src/id_test.go",
        "src/credentials.ts",
        "node_modules/pkg/credentials.js",
        ".env.example",
    ] {
        assert!(!policy.is_secret(&RelPath::parse(path).unwrap()), "{path}");
    }
    for path in ["id_rsa", "credentials.json", ".env", ".env.local"] {
        assert!(policy.is_secret(&RelPath::parse(path).unwrap()), "{path}");
    }
}
