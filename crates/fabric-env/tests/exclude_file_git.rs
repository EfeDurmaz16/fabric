//! Capture hands the secret exclude file to `git add -A`. This proves git
//! keeps out exactly the paths `SecretPolicy::denies` denies, with
//! `core.ignorecase` on and off, so the two matchers cannot drift apart.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use fabric_core::path::RelPath;
use fabric_env::secret::{Pattern, Scope, SecretPolicy};

/// Every path the test creates: one or more per built-in pattern, case
/// variants, nested placements, a job pattern, and near misses that must
/// stay in the commit. No two differ only in case, so the tree is the same
/// on a case-folding volume.
const PATHS: &[&str] = &[
    ".env",
    ".env.local",
    ".envrc",
    "id_ed25519",
    "credentials.json",
    ".credentials.json",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".aws/config",
    ".ssh/known_hosts",
    ".gnupg/pubring.kbx",
    "prod.env",
    "server.pem",
    "server.key",
    "cert.p12",
    "cert.pfx",
    "keystore.jks",
    "terraform.tfstate",
    "terraform.tfstate.backup",
    ".docker/config.json",
    "precious/out/.env",
    "precious/out/id_rsa",
    "config/.aws/credentials",
    "home/.docker/config.json",
    "upper/.ENV",
    "upper/Server.PEM",
    "upper/.SSH/id_x",
    "data/local.sqlite",
    "src/main.rs",
    "environment.ts",
    "docs/keys.md",
    "monkey",
    "docker/config.json",
    ".docker/daemon.json",
    "my.env.d/readme",
    "src/identity.rs",
];

fn git(dir: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

/// Creates the tree, runs `git add -A` with the exclude file, and returns what git staged.
fn staged_with(policy: &SecretPolicy, ignorecase: &str) -> io::Result<BTreeSet<String>> {
    let repo = tempfile::tempdir()?;
    let scratch = tempfile::tempdir()?;
    for path in PATHS {
        let full = repo.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full, "x")?;
    }
    let exclude = scratch.path().join("exclude");
    fs::write(&exclude, policy.exclude_file())?;
    let exclude_arg = format!("core.excludesFile={}", exclude.display());
    let ignorecase_arg = format!("core.ignorecase={ignorecase}");
    git(repo.path(), &["init", "-q"])?;
    git(
        repo.path(),
        &["-c", &exclude_arg, "-c", &ignorecase_arg, "add", "-A"],
    )?;
    Ok(git(repo.path(), &["ls-files"])?
        .lines()
        .map(str::to_owned)
        .collect())
}

#[test]
fn git_keeps_out_exactly_what_the_policy_denies() {
    let mut policy = SecretPolicy::default();
    policy.widen(Pattern::parse("*.sqlite").unwrap());
    for ignorecase in ["true", "false"] {
        let staged = staged_with(&policy, ignorecase).unwrap();
        for path in PATHS {
            let denied = policy
                .denies(&RelPath::parse(path).unwrap(), Scope::Work)
                .is_some();
            assert_eq!(
                staged.contains(*path),
                !denied,
                "{path} with core.ignorecase={ignorecase}"
            );
        }
        assert!(staged.contains("src/main.rs"));
    }
}
