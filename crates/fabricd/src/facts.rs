//! What a node measures about itself: the exact strings `fabric doctor`
//! compares and the carry keys a snapshot records (ADR 0008, ADR 0012).
//!
//! Invariant: every fact is a tool's own output, trimmed and otherwise kept
//! whole, and a fact that could not be measured is absent, never empty or
//! guessed. A login is measured by exit code alone and recorded as `yes` or
//! `no`; no account name is ever read. A carry key is built only from
//! measured facts, and a cache whose ecosystem or facts are unknown gets
//! `CarryKey::Unmeasured`, which never matches, so it is rebuilt rather than
//! trusted.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use fabric_client::api::Roots;
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{CacheClass, CarryKey};
use fabric_env::detect::{Detection, Ecosystem, NodeManager};
use fabric_net::wire::{FactName, FactValue, Facts, MAX_FACT_VALUE};
use fabric_store::capture::CarryKeys;

use crate::tool_env::ToolEnv;

/// How long one measuring command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Fact names, one constant each so a typo cannot make a fact disappear.
pub mod name {
    /// `id -un`.
    pub const USER: &str = "user";
    /// `$HOME`.
    pub const HOME: &str = "home";
    /// The operating system, as Rust names it.
    pub const OS: &str = "os";
    /// The CPU architecture, as Rust names it.
    pub const ARCH: &str = "arch";
    /// This build's version.
    pub const FABRIC: &str = "fabric.version";
    /// `sw_vers -productVersion`.
    pub const MACOS: &str = "macos.version";
    /// `xcrun --show-sdk-version`.
    pub const MACOS_SDK: &str = "macos.sdk";
    /// `node --version`.
    pub const NODE: &str = "node.version";
    /// `node -p process.versions.modules`, the ABI of native addons.
    pub const NODE_ABI: &str = "node.abi";
    /// `npm --version`.
    pub const NPM: &str = "npm.version";
    /// `pnpm --version`.
    pub const PNPM: &str = "pnpm.version";
    /// `pnpm store path`.
    pub const PNPM_STORE: &str = "pnpm.store";
    /// `yarn --version`.
    pub const YARN: &str = "yarn.version";
    /// `bun --version`.
    pub const BUN: &str = "bun.version";
    /// `rustc -vV`, whole.
    pub const RUSTC: &str = "rustc";
    /// `${CARGO_HOME:-$HOME/.cargo}`.
    pub const CARGO_HOME: &str = "cargo.home";
    /// `python3 --version`.
    pub const PYTHON: &str = "python.version";
    /// `sys.base_prefix` of `python3`.
    pub const PYTHON_HOME: &str = "python.home";
    /// `claude --version`.
    pub const CLAUDE: &str = "claude.version";
    /// `claude auth status`, exit code only.
    pub const CLAUDE_LOGIN: &str = "claude.login";
    /// `codex --version`.
    pub const CODEX: &str = "codex.version";
    /// `codex login status`, exit code only.
    pub const CODEX_LOGIN: &str = "codex.login";
    /// Whether iCloud Drive syncs `~/Desktop`.
    pub const ICLOUD_DESKTOP: &str = "icloud.desktop";
    /// Whether iCloud Drive syncs `~/Documents`.
    pub const ICLOUD_DOCUMENTS: &str = "icloud.documents";
}

/// One node's measured facts, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeFacts(BTreeMap<String, String>);

/// What one measuring command reports.
enum Probe {
    /// The trimmed standard output of a command that exited 0.
    Output,
    /// `yes` when the command exited 0, `no` when it ran and failed.
    ExitCode,
}

impl NodeFacts {
    /// Builds facts from name and value pairs; for tests and for a peer's answer.
    #[must_use]
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        )
    }

    /// Measures this node with the tools `env` names: programs are looked
    /// up on its `PATH` and run with its roots, each set to its value or
    /// removed, never the daemon's own (ADR 0018, The client's environment).
    pub async fn measure(home: &Path, env: &ToolEnv) -> Self {
        let inherited: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        Self::measure_from(home, env, &inherited).await
    }

    /// Measures as [`NodeFacts::measure`] does, with `inherited` as the
    /// daemon's own environment: each tool starts from it, and `env`'s
    /// `PATH` and roots then replace or remove what they name.
    async fn measure_from(home: &Path, env: &ToolEnv, inherited: &[(OsString, OsString)]) -> Self {
        // Each probe is boxed, so the joined future holds 19 pointers, not 19 child processes.
        let run = |program: &'static str, args: &'static [&'static str], probe: Probe| {
            let (env, home) = (env.clone(), home.to_owned());
            Box::pin(
                async move { probe_command(program, args, probe, &env, &home, inherited).await },
            )
        };
        let (
            user,
            macos,
            sdk,
            node,
            node_abi,
            npm,
            pnpm,
            pnpm_store,
            yarn,
            bun,
            rustc,
            python,
            python_home,
            claude,
            claude_login,
            codex,
            codex_login,
            desktop,
            documents,
        ) = tokio::join!(
            run("id", &["-un"], Probe::Output),
            run("sw_vers", &["-productVersion"], Probe::Output),
            run("xcrun", &["--show-sdk-version"], Probe::Output),
            run("node", &["--version"], Probe::Output),
            run("node", &["-p", "process.versions.modules"], Probe::Output),
            run("npm", &["--version"], Probe::Output),
            run("pnpm", &["--version"], Probe::Output),
            run("pnpm", &["store", "path"], Probe::Output),
            run("yarn", &["--version"], Probe::Output),
            run("bun", &["--version"], Probe::Output),
            run("rustc", &["-vV"], Probe::Output),
            run("python3", &["--version"], Probe::Output),
            run(
                "python3",
                &["-c", "import sys; print(sys.base_prefix)"],
                Probe::Output
            ),
            run("claude", &["--version"], Probe::Output),
            run("claude", &["auth", "status"], Probe::ExitCode),
            run("codex", &["--version"], Probe::Output),
            run("codex", &["login", "status"], Probe::ExitCode),
            run(
                "defaults",
                &["read", "com.apple.finder", "FXICloudDriveDesktop"],
                Probe::Output
            ),
            run(
                "defaults",
                &["read", "com.apple.finder", "FXICloudDriveDocuments"],
                Probe::Output
            ),
        );
        let mut facts = BTreeMap::new();
        let mut put = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                facts.insert(name.to_owned(), value);
            }
        };
        put(name::USER, user);
        put(name::HOME, Some(home.display().to_string()));
        put(name::OS, Some(std::env::consts::OS.to_owned()));
        put(name::ARCH, Some(std::env::consts::ARCH.to_owned()));
        put(name::FABRIC, Some(env!("CARGO_PKG_VERSION").to_owned()));
        put(name::MACOS, macos);
        put(name::MACOS_SDK, sdk);
        put(name::NODE, node);
        put(name::NODE_ABI, node_abi);
        put(name::NPM, npm);
        put(name::PNPM, pnpm);
        put(name::PNPM_STORE, pnpm_store);
        put(name::YARN, yarn);
        put(name::BUN, bun);
        put(name::RUSTC, rustc);
        put(
            name::CARGO_HOME,
            Some(
                env.root(Roots::CARGO_HOME)
                    .map_or_else(|| home.join(".cargo"), PathBuf::from)
                    .display()
                    .to_string(),
            ),
        );
        put(name::PYTHON, python);
        put(name::PYTHON_HOME, python_home);
        put(name::CLAUDE, claude);
        put(name::CLAUDE_LOGIN, claude_login);
        put(name::CODEX, codex);
        put(name::CODEX_LOGIN, codex_login);
        put(name::ICLOUD_DESKTOP, desktop);
        put(name::ICLOUD_DOCUMENTS, documents);
        Self(facts)
    }

    /// The value of a fact, if it was measured.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Every fact, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// True when iCloud Drive syncs the named folder on this node.
    #[must_use]
    pub fn syncs(&self, name: &str) -> bool {
        self.get(name) == Some("1")
    }

    /// The facts as the wire carries them; a value too long for a fact is cut.
    #[must_use]
    pub fn to_wire(&self) -> Facts {
        let pairs = self
            .0
            .iter()
            .map(|(k, v)| (FactName::truncated(k), FactValue::truncated(v)))
            .collect();
        Facts::new(pairs).unwrap_or_default()
    }

    /// Reads a peer's facts from the wire.
    #[must_use]
    pub fn from_wire(facts: &Facts) -> Self {
        Self::from_pairs(facts.iter())
    }
}

async fn probe_command(
    program: &str,
    args: &[&str],
    probe: Probe,
    env: &ToolEnv,
    home: &Path,
    inherited: &[(OsString, OsString)],
) -> Option<String> {
    let path_var = env.path_var();
    let binary = find_on_path(program, path_var)?;
    let mut command = tokio::process::Command::new(binary);
    command
        .env_clear()
        .envs(inherited.iter().map(|(k, v)| (k, v)));
    for (name, value) in env.roots() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let child = command
        .args(args)
        .env("PATH", path_var)
        .env("HOME", home)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    tracing::debug!(program, ?args, "measuring");
    let output = tokio::time::timeout(COMMAND_TIMEOUT, child).await;
    tracing::debug!(program, ?args, done = output.is_ok(), "measured");
    let output = output.ok()?.ok()?;
    match probe {
        Probe::ExitCode => Some(if output.status.success() { "yes" } else { "no" }.to_owned()),
        Probe::Output => {
            let text = String::from_utf8(output.stdout).ok()?;
            let text = text.trim();
            (output.status.success() && !text.is_empty() && text.len() <= MAX_FACT_VALUE)
                .then(|| text.to_owned())
        }
    }
}

/// The first executable named `program` on `path_var`, always an absolute path.
///
/// A relative or empty entry is skipped, as the harness adapters skip it:
/// it names a directory relative to the daemon's working directory, which
/// is wherever `fabric serve` started, not one the client chose. A client
/// with no `PATH` sends an empty list, whose `PATH` value is the empty
/// string, which a shell reads as the working directory (review finding
/// on PR #13).
pub fn find_on_path(program: &str, path_var: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// The major version in a version string: `v22.23.2` gives `22`, `11.23.0` gives `11`.
#[must_use]
pub fn major(version: &str) -> Option<String> {
    let digits: String = version
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then_some(digits)
}

impl NodeFacts {
    /// This node's `Node` key for `manager`, when every string it needs was measured.
    fn node_key(&self, manager: &str) -> Option<CarryKey> {
        let version_fact = match manager {
            "npm" => name::NPM,
            "pnpm" => name::PNPM,
            "yarn" => name::YARN,
            "bun" => name::BUN,
            _ => return None,
        };
        let pnpm_store_path = if manager == "pnpm" {
            Some(self.get(name::PNPM_STORE)?.to_owned())
        } else {
            None
        };
        Some(CarryKey::Node {
            node_major: major(self.get(name::NODE)?)?,
            package_manager: manager.to_owned(),
            pm_major: major(self.get(version_fact)?)?,
            pnpm_store_path,
        })
    }

    fn rust_key(&self) -> Option<CarryKey> {
        Some(CarryKey::Rust {
            rustc_vv: self.get(name::RUSTC)?.to_owned(),
            cargo_home: AbsPath::parse(self.get(name::CARGO_HOME)?).ok()?,
        })
    }

    fn python_key(&self) -> Option<CarryKey> {
        Some(CarryKey::Python {
            python_version: self.get(name::PYTHON)?.to_owned(),
            python_home: AbsPath::parse(self.get(name::PYTHON_HOME)?).ok()?,
        })
    }

    fn platform_key(&self) -> Option<CarryKey> {
        Some(CarryKey::Other {
            tool: "platform".to_owned(),
            version: format!("{}-{}", self.get(name::OS)?, self.get(name::ARCH)?),
        })
    }

    /// The key this node measures for the ecosystem `source` names (ADR 0012).
    ///
    /// The target of a restore cannot run detection before the workspace
    /// exists, so it measures the key of the same shape as the source's:
    /// the same package manager, its own versions and paths.
    #[must_use]
    pub fn key_like(&self, source: &CarryKey) -> CarryKey {
        let key = match source {
            CarryKey::Node {
                package_manager, ..
            } => self.node_key(package_manager),
            CarryKey::Rust { .. } => self.rust_key(),
            CarryKey::Python { .. } => self.python_key(),
            CarryKey::Other { tool, .. } if tool == "platform" => self.platform_key(),
            CarryKey::Other { .. } | CarryKey::Unmeasured => None,
        };
        key.unwrap_or(CarryKey::Unmeasured)
    }
}

/// Says which field of two carry keys differs, for the one line a skipped cache prints.
#[must_use]
pub fn key_difference(source: &CarryKey, target: &CarryKey) -> String {
    match (source, target) {
        (CarryKey::Unmeasured, _) => "the source measured no key".to_owned(),
        (_, CarryKey::Unmeasured) => "this node lacks a tool the key needs".to_owned(),
        (
            CarryKey::Node {
                node_major: source_node,
                pm_major: source_pm,
                pnpm_store_path: source_store,
                package_manager: manager,
            },
            CarryKey::Node {
                node_major: target_node,
                pm_major: target_pm,
                pnpm_store_path: target_store,
                ..
            },
        ) => {
            if source_node != target_node {
                format!("node major {source_node} on the source, {target_node} here")
            } else if source_pm != target_pm {
                format!("{manager} major {source_pm} on the source, {target_pm} here")
            } else if source_store != target_store {
                format!("pnpm store {source_store:?} on the source, {target_store:?} here")
            } else {
                "keys are equal".to_owned()
            }
        }
        (CarryKey::Rust { cargo_home: a, .. }, CarryKey::Rust { cargo_home: b, .. }) if a != b => {
            format!("CARGO_HOME {a} on the source, {b} here")
        }
        (CarryKey::Rust { .. }, CarryKey::Rust { .. }) => "rustc -vV differs".to_owned(),
        _ if source == target => "keys are equal".to_owned(),
        _ => "the keys differ".to_owned(),
    }
}

/// Carry keys for a capture: detection says each cache's ecosystem, the facts give the strings.
pub struct MeasuredKeys {
    facts: NodeFacts,
    ecosystems: BTreeMap<RelPath, Ecosystem>,
}

impl MeasuredKeys {
    /// Keys for the workspace `detection` describes, measured as `facts`.
    #[must_use]
    pub fn new(facts: NodeFacts, detection: Option<&Detection>) -> Self {
        let ecosystems = detection
            .map(|d| {
                d.caches()
                    .iter()
                    .map(|cache| (cache.path.clone(), cache.ecosystem))
                    .collect()
            })
            .unwrap_or_default();
        Self { facts, ecosystems }
    }
}

impl CarryKeys for MeasuredKeys {
    fn key(&self, _class: CacheClass, path: &RelPath) -> CarryKey {
        let key = match self.ecosystems.get(path) {
            Some(Ecosystem::Node(manager)) => self.facts.node_key(manager_name(*manager)),
            Some(Ecosystem::Rust) => self.facts.rust_key(),
            Some(Ecosystem::Python) => self.facts.python_key(),
            Some(Ecosystem::Other) => self.facts.platform_key(),
            None => None,
        };
        key.unwrap_or(CarryKey::Unmeasured)
    }
}

fn manager_name(manager: NodeManager) -> &'static str {
    match manager {
        NodeManager::Npm => "npm",
        NodeManager::Pnpm => "pnpm",
        NodeManager::YarnClassic | NodeManager::YarnBerry => "yarn",
        NodeManager::Bun => "bun",
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use fabric_client::api::{ClientEnv, PathDir, SystemPath};

    use super::*;

    /// A tool in a directory the client's `PATH` names only relatively is
    /// not found, and an empty entry, which an empty client `PATH` gives,
    /// goes through the same filter: the node would resolve either against
    /// its own working directory (review finding on PR #13).
    #[test]
    fn find_on_path_skips_relative_and_empty_entries() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The test runs from the crate directory; a relative entry that
        // reaches the tool from there must not count.
        let cwd = std::env::current_dir().unwrap();
        let up: PathBuf = cwd.components().skip(1).map(|_| "..").collect();
        let relative = up.join(dir.path().strip_prefix("/").unwrap());
        assert!(
            relative.join("tool").exists(),
            "the relative entry reaches the tool"
        );
        let var = std::env::join_paths([relative.as_path(), Path::new("")]).unwrap();
        assert_eq!(find_on_path("tool", &var), None);
        let var = std::env::join_paths([relative.as_path(), dir.path()]).unwrap();
        assert_eq!(find_on_path("tool", &var), Some(tool));
    }

    /// Gate finding on PR #8: capture measured carry keys with the daemon's
    /// `CARGO_HOME`, not the requesting CLI's. A request's roots reach the
    /// facts and the tools measured, and a root it leaves unset is removed,
    /// not taken from the daemon. The daemon here sets every root itself,
    /// whatever the environment the test runs in, so a root taken from it
    /// shows (review finding on PR #13: the check of removal passed only
    /// when the test process had no `PNPM_HOME`).
    #[tokio::test]
    async fn facts_are_measured_in_the_environment_the_request_names() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let pnpm = bin.join("pnpm");
        std::fs::write(
            &pnpm,
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 9.9.9 ;;\nstore) echo \"${PNPM_HOME:-unset}/store\" ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&pnpm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = vec![
            PathDir::parse(bin.to_str().unwrap()).unwrap(),
            PathDir::parse("/usr/bin").unwrap(),
            PathDir::parse("/bin").unwrap(),
        ];
        let home = dir.path().join("home");
        let daemon: Vec<(OsString, OsString)> = Roots::default()
            .vars()
            .iter()
            .map(|(name, _)| ((*name).into(), format!("/daemon/{name}").into()))
            .collect();
        let client = ClientEnv {
            path: path.clone(),
            roots: Roots {
                cargo_home: Some(SystemPath::parse("/client/cargo").unwrap()),
                pnpm_home: Some(SystemPath::parse("/client/pnpm").unwrap()),
                ..Roots::default()
            },
        };
        let facts = NodeFacts::measure_from(&home, &ToolEnv::from(&client), &daemon).await;
        assert_eq!(facts.get(name::CARGO_HOME), Some("/client/cargo"));
        assert_eq!(facts.get(name::PNPM), Some("9.9.9"));
        assert_eq!(facts.get(name::PNPM_STORE), Some("/client/pnpm/store"));

        let bare = ClientEnv {
            path,
            roots: Roots::default(),
        };
        let facts = NodeFacts::measure_from(&home, &ToolEnv::from(&bare), &daemon).await;
        let default_cargo = home.join(".cargo");
        assert_eq!(facts.get(name::CARGO_HOME), default_cargo.to_str());
        assert_eq!(facts.get(name::PNPM_STORE), Some("unset/store"));
    }

    fn pro() -> NodeFacts {
        NodeFacts::from_pairs([
            (name::NODE, "v22.23.2"),
            (name::NPM, "10.9.2"),
            (name::PNPM, "11.23.0"),
            (name::PNPM_STORE, "/Users/x/Library/pnpm/store/v11"),
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
        ])
    }

    #[test]
    fn an_npm_tree_carries_only_where_the_node_and_npm_majors_match() {
        let source = pro().node_key("npm").unwrap();
        assert!(source.matches(&pro().key_like(&source)));
        let air = NodeFacts::from_pairs([(name::NODE, "v26.0.0"), (name::NPM, "11.1.0")]);
        let target = air.key_like(&source);
        assert!(!source.matches(&target));
        assert_eq!(
            key_difference(&source, &target),
            "node major 22 on the source, 26 here"
        );
    }

    #[test]
    fn a_pnpm_tree_needs_pnpm_on_the_target() {
        let source = pro().node_key("pnpm").unwrap();
        let air = NodeFacts::from_pairs([(name::NODE, "v22.1.0"), (name::NPM, "10.0.0")]);
        assert_eq!(air.key_like(&source), CarryKey::Unmeasured);
        assert!(!source.matches(&air.key_like(&source)));
    }

    #[test]
    fn a_cache_detection_does_not_know_is_never_carried() {
        let keys = MeasuredKeys::new(pro(), None);
        let key = keys.key(CacheClass::Deps, &RelPath::parse("node_modules").unwrap());
        assert_eq!(key, CarryKey::Unmeasured);
    }

    #[test]
    fn a_major_is_the_leading_digits_of_a_version() {
        assert_eq!(major("v22.23.2").as_deref(), Some("22"));
        assert_eq!(major("11.23.0").as_deref(), Some("11"));
        assert_eq!(major("nightly"), None);
    }
}
