//! Capture builds the working-tree commit `W` from the paths git reports as
//! changed, sorted by `SecretPolicy::staging`, and hands git only literal
//! paths. This runs that sequence with real git, in a repository whose
//! `.gitignore` negates built-in patterns, and checks the tree git wrote:
//! an untracked secret never enters it, and a tracked file's edit or
//! deletion is never replaced by the version in the index.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use fabric_core::env::{SecretPattern, SecretPatternError};
use fabric_core::path::RelPath;
use fabric_env::secret::{Scope, SecretPolicy, TrackedSecretChanged, TrackedSecrets, Tracking};

/// Committed before the working tree changes. The first five are the
/// failure from the review: source and template files whose names an
/// over-broad pattern matched, so their edits never reached `W`.
const COMMITTED: &[(&str, &str)] = &[
    (".env.example", "A=\n"),
    ("src/id_test.go", "package src\n"),
    ("src/credentials.ts", "export {}\n"),
    ("src/main.rs", "fn main() {}\n"),
    ("docs/notes.md", "old\n"),
    (".env.production", "PUBLIC_URL=x\n"),
    ("config/server.key", "old key\n"),
    (
        ".gitignore",
        ".env*\n!.env.local\n!.env.example\n!.env.sample\n!server.pem\n",
    ),
];

/// Edits made to committed files after the commit; `None` deletes the file.
const EDITS: &[(&str, Option<&str>)] = &[
    (".env.example", Some("A=\nNEW_VAR=\n")),
    ("src/id_test.go", Some("package src // edited\n")),
    ("src/credentials.ts", None),
    ("docs/notes.md", Some("new\n")),
    (".env.production", None),
    ("config/server.key", Some("new key\n")),
];

/// Files created after the commit and never added. One or more per
/// built-in pattern, case variants, a job pattern, and near misses that
/// must reach `W`. No two differ only in case, so the tree is the same on
/// a case-folding volume.
const CREATED: &[&str] = &[
    "src/id_allocator.rs",
    ".env",
    ".env.local",
    ".env.sample",
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
    "upper/.SSH/known_hosts",
    "data/local.sqlite",
    "environment.ts",
    "docs/keys.md",
    "monkey",
    "docker/config.json",
    ".docker/daemon.json",
    "my.env.d/readme",
    "src/identity.rs",
];

fn git(dir: &Path, index: &Path, args: &[&str], stdin: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_INDEX_FILE", index)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.unwrap_or_default())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}

fn nul_paths(bytes: &[u8]) -> io::Result<Vec<RelPath>> {
    bytes
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| {
            let text = std::str::from_utf8(name).map_err(io::Error::other)?;
            RelPath::parse(text).map_err(io::Error::other)
        })
        .collect()
}

fn write(root: &Path, path: &str, text: &str) -> io::Result<()> {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(full, text)
}

/// A repository after `COMMITTED`, `EDITS` and `CREATED`, and the index capture starts from.
struct Repo {
    dir: tempfile::TempDir,
    index: std::path::PathBuf,
}

fn repo() -> io::Result<Repo> {
    let dir = tempfile::tempdir()?;
    let index = dir.path().join(".git/index");
    let root = dir.path();
    git(root, &index, &["init", "-q"], None)?;
    for (path, text) in COMMITTED {
        write(root, path, text)?;
    }
    git(root, &index, &["add", "-A", "-f", "--", "."], None)?;
    git(root, &index, &["commit", "-q", "-m", "base"], None)?;
    for (path, edit) in EDITS {
        match edit {
            Some(text) => write(root, path, text)?,
            None => fs::remove_file(root.join(path))?,
        }
    }
    for path in CREATED {
        write(root, path, "new\n")?;
    }
    Ok(Repo { dir, index })
}

/// Plays capture's part: lists the changes against `I`, sorts them, stages
/// the literal paths into a temporary index and returns the tree as path to contents.
fn capture_w(
    repo: &Repo,
    policy: &SecretPolicy,
    consent: TrackedSecrets,
) -> io::Result<Result<BTreeMap<String, String>, TrackedSecretChanged>> {
    let root = repo.dir.path();
    let temp = root.join(".git/fabric-capture-index");
    fs::copy(&repo.index, &temp)?;
    let tracked = git(
        root,
        &temp,
        &["diff", "--name-only", "-z", "--no-renames"],
        None,
    )?;
    let untracked = git(
        root,
        &temp,
        &["ls-files", "-z", "--others", "--exclude-standard"],
        None,
    )?;
    let changes = nul_paths(&tracked)?
        .into_iter()
        .map(|path| (path, Tracking::Tracked))
        .chain(
            nul_paths(&untracked)?
                .into_iter()
                .map(|path| (path, Tracking::Untracked)),
        );
    let paths = match policy.staging(changes).paths(consent) {
        Ok(paths) => paths,
        Err(refused) => return Ok(Err(refused)),
    };
    if !paths.is_empty() {
        let mut list = Vec::new();
        for path in &paths {
            list.extend_from_slice(b":(literal)");
            list.extend_from_slice(path.as_str().as_bytes());
            list.push(0);
        }
        git(
            root,
            &temp,
            &["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"],
            Some(&list),
        )?;
    }
    let tree =
        String::from_utf8(git(root, &temp, &["write-tree"], None)?).map_err(io::Error::other)?;
    let names = git(
        root,
        &temp,
        &["ls-tree", "-r", "-z", "--name-only", tree.trim()],
        None,
    )?;
    let mut files = BTreeMap::new();
    for path in nul_paths(&names)? {
        let spec = format!("{}:{}", tree.trim(), path.as_str());
        let blob = git(root, &temp, &["cat-file", "blob", &spec], None)?;
        files.insert(
            path.as_str().to_owned(),
            String::from_utf8(blob).map_err(io::Error::other)?,
        );
    }
    Ok(Ok(files))
}

fn policy() -> Result<SecretPolicy, SecretPatternError> {
    let mut policy = SecretPolicy::default();
    policy.widen(SecretPattern::parse("*.sqlite")?);
    Ok(policy)
}

/// Without consent a changed tracked secret stops the capture and is named.
#[test]
fn a_changed_tracked_secret_refuses_the_capture_and_names_it() {
    let repo = repo().unwrap();
    let refused = capture_w(&repo, &policy().unwrap(), TrackedSecrets::Refuse)
        .unwrap()
        .unwrap_err();
    let named: Vec<(&str, &str)> = refused
        .changed
        .iter()
        .map(|d| (d.path.as_str(), d.pattern.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            (".env.production", ".env.*"),
            ("config/server.key", "*.key")
        ]
    );
}

/// With consent every tracked change reaches `W` as the working tree has
/// it, deletions included, and no untracked path the policy denies does.
#[test]
fn w_holds_every_tracked_change_and_no_untracked_secret() {
    let repo = repo().unwrap();
    let policy = policy().unwrap();
    let w = capture_w(&repo, &policy, TrackedSecrets::Carry)
        .unwrap()
        .unwrap();
    for (path, edit) in EDITS {
        assert_eq!(w.get(*path).map(String::as_str), *edit, "{path}");
    }
    for path in CREATED {
        let denied = policy
            .denies(&RelPath::parse(path).unwrap(), Scope::Work)
            .is_some();
        assert_eq!(w.contains_key(*path), !denied, "{path}");
    }
    for kept in ["src/id_allocator.rs", ".env.sample", "src/main.rs"] {
        assert!(w.contains_key(kept), "{kept}");
    }
    for out in [
        ".env",
        ".env.local",
        "server.pem",
        "upper/.ENV",
        "data/local.sqlite",
    ] {
        assert!(!w.contains_key(out), "{out}");
    }
}
