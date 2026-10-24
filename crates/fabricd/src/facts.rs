//! What a node measures about itself: the exact strings `fabric doctor`
//! compares and the carry keys a snapshot records (ADR 0008, ADR 0012).
//!
//! Invariant: every fact is a tool's own output, trimmed and otherwise kept
//! whole, and a fact that could not be measured is absent, never empty or
//! guessed. A login is measured by exit code alone and recorded as `yes` or
//! `no`; no account name is ever read. A carry key is built only from
//! measured facts, and a cache whose facts are unknown gets
//! `CarryKey::Unmeasured`, which never matches. What makes a cache again is
//! recorded beside its key, and a cache detection found no lockfile or step
//! for is `Remake::Nothing`: the user's own files, which the carry decision
//! never leaves out (`crate::carry`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use fabric_client::api::Roots;
use fabric_core::env::{OVERRIDE_FILE, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{CacheClass, CarryKey, Platform, Remake, SnapshotManifest};
use fabric_env::detect::{CacheOrigin, Completion, Detection, Ecosystem, NodeManager};
use fabric_env::materialize::CacheHealth;
use fabric_net::wire::{FactName, FactValue, Facts, MAX_FACT_VALUE};
use fabric_store::capture::CarryKeys;

use crate::harnesses::HarnessEnv;
use crate::tool_env::ToolEnv;

/// How long one measuring command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Fact names, one constant each so a typo cannot make a fact disappear.
pub mod name {
    /// `id -un`.
    pub const USER: &str = "user";
    /// `$HOME`.
    pub const HOME: &str = "home";
    /// `git --version`, found on the daemon's own `PATH`, where restore runs it.
    pub const GIT: &str = "git.version";
    /// Where Codex capture and restore find `lsof`, which they ask whether
    /// a Codex process holds a thread's rollout open.
    pub const LSOF: &str = "lsof";
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
    /// `cargo --version`: rustup installs cargo and rustc as separate
    /// components, so a node can have one without the other.
    pub const CARGO: &str = "cargo.version";
    /// `${CARGO_HOME:-$HOME/.cargo}`.
    pub const CARGO_HOME: &str = "cargo.home";
    /// `python3 --version`.
    pub const PYTHON: &str = "python.version";
    /// `sys.base_prefix` of `python3`.
    pub const PYTHON_HOME: &str = "python.home";
    /// `uv --version`, the installer a Python setup step runs.
    pub const UV: &str = "uv.version";
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
    /// Where the `PATH` of the pulling command finds the job's ACP agent;
    /// measured by the target of a pull only, never sent to a peer.
    pub const ACP_AGENT: &str = "acp.agent";
    /// The `state_dir` the target's `harness.toml` gives the job's ACP
    /// agent; measured by the target of a pull only, never sent to a peer.
    pub const ACP_STATE_DIR: &str = "acp.state_dir";
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
    /// git is measured on the daemon's own `PATH`, where restore runs it,
    /// and `lsof` where Codex capture looks for it.
    pub async fn measure(home: &Path, env: &ToolEnv) -> Self {
        let inherited: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        Self::measure_from(home, env, &inherited).await
    }

    /// Measures as [`NodeFacts::measure`] does, with `inherited` as the
    /// daemon's own environment: each tool starts from it, and `env`'s
    /// `PATH` and roots then replace or remove what they name; git runs in
    /// `inherited` alone.
    async fn measure_from(home: &Path, env: &ToolEnv, inherited: &[(OsString, OsString)]) -> Self {
        let own = ToolEnv::from_vars(|name| {
            inherited
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        });
        let (mut facts, git) = tokio::join!(
            Box::pin(Self::measure_tools(home, env, inherited)),
            probe_command("git", &["--version"], Probe::Output, &own, home, inherited),
        );
        if let Some(git) = git {
            facts.0.insert(name::GIT.to_owned(), git);
        }
        if let Some(lsof) = fabric_harness::codex::Codex::lsof() {
            facts
                .0
                .insert(name::LSOF.to_owned(), lsof.display().to_string());
        }
        facts
    }

    /// Measures everything but git with the tools `env` names.
    async fn measure_tools(home: &Path, env: &ToolEnv, inherited: &[(OsString, OsString)]) -> Self {
        // Each probe is boxed, so the joined future holds 21 pointers, not 21 child processes.
        let run = |program: &'static str, args: &'static [&'static str], probe: Probe| {
            Box::pin(probe_command(program, args, probe, env, home, inherited))
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
            cargo,
            python,
            python_home,
            uv,
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
            run("cargo", &["--version"], Probe::Output),
            run("python3", &["--version"], Probe::Output),
            run(
                "python3",
                &["-c", "import sys; print(sys.base_prefix)"],
                Probe::Output
            ),
            run("uv", &["--version"], Probe::Output),
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
        put(name::CARGO, cargo);
        let cargo_home = env.root(Roots::CARGO_HOME);
        let cargo_home = cargo_home.map_or_else(|| home.join(".cargo"), PathBuf::from);
        put(name::CARGO_HOME, Some(cargo_home.display().to_string()));
        put(name::PYTHON, python);
        put(name::PYTHON_HOME, python_home);
        put(name::UV, uv);
        put(name::CLAUDE, claude);
        put(name::CLAUDE_LOGIN, claude_login);
        put(name::CODEX, codex);
        put(name::CODEX_LOGIN, codex_login);
        put(name::ICLOUD_DESKTOP, desktop);
        put(name::ICLOUD_DOCUMENTS, documents);
        Self(facts)
    }

    /// Adds what this node has for the ACP agent `program`: where the
    /// `PATH` of `env` finds it, and the `state_dir` its configuration gives
    /// it. Either is absent when the node has none, and the state directory
    /// is absent too when capture and restore would refuse it: outside the
    /// home, the home itself or under `~/.fabric`, or reached through a link.
    pub fn measure_acp(&mut self, program: &ProgramName, env: &HarnessEnv) {
        if let Some(found) = find_on_path(program.as_str(), &env.path_var) {
            self.0
                .insert(name::ACP_AGENT.to_owned(), found.display().to_string());
        }
        if let Some(entry) = env.config.acp.get(program.as_str())
            && crate::harnesses::acp_adapter(program, &[], env)
                .is_ok_and(|adapter| adapter.check_state_dir().is_ok())
        {
            self.0.insert(
                name::ACP_STATE_DIR.to_owned(),
                entry.state_dir.display().to_string(),
            );
        }
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
/// A relative or empty entry is skipped, as the harness's own lookup
/// skips it: it names a directory relative to the daemon's working
/// directory, which is wherever `fabric serve` started, not one the client
/// chose, and relative to the workspace at launch, so the binary measured
/// and the binary launched could differ. A client with no `PATH` sends an
/// empty list, whose `PATH` value is the empty string, which a shell reads
/// as the working directory (review finding on PR #13).
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

/// What a node knows about one program a setup step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program<'a> {
    /// Not a program the node measures, such as a declared `make`.
    Unmeasured,
    /// Measured and not installed.
    Missing,
    /// Installed; the version it reports.
    Installed(&'a str),
}

/// The `tool` of the `Other` key whose version is the node's `{os}-{arch}`.
pub const PLATFORM_TOOL: &str = "platform";

/// The `tool` of the `Other` key whose version is the node's native addon ABI.
pub const NODE_ABI_TOOL: &str = "node.abi";

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
            tool: PLATFORM_TOOL.to_owned(),
            version: format!("{}-{}", self.get(name::OS)?, self.get(name::ARCH)?),
        })
    }

    /// The ABI native Node addons are built for here: the key of a
    /// `node_modules` no lockfile describes, which no package manager names.
    fn node_abi_key(&self) -> Option<CarryKey> {
        Some(CarryKey::Other {
            tool: NODE_ABI_TOOL.to_owned(),
            version: self.get(name::NODE_ABI)?.to_owned(),
        })
    }

    /// This node's operating system and architecture, named as a snapshot names them.
    #[must_use]
    pub fn platform(&self) -> Platform {
        Platform {
            os: self.get(name::OS).unwrap_or_default().to_owned(),
            arch: self.get(name::ARCH).unwrap_or_default().to_owned(),
        }
    }

    /// The fact a node measures `program` by, for a program setup steps run;
    /// `None` for one no node measures, such as a declared `make`.
    #[must_use]
    pub fn program_fact(program: &str) -> Option<&'static str> {
        Some(match program {
            "node" => name::NODE,
            "npm" => name::NPM,
            "pnpm" => name::PNPM,
            "yarn" => name::YARN,
            "bun" => name::BUN,
            "rustc" => name::RUSTC,
            "cargo" => name::CARGO,
            "python3" => name::PYTHON,
            "uv" => name::UV,
            _ => return None,
        })
    }

    /// What this node knows about `program`.
    #[must_use]
    pub fn program(&self, program: &str) -> Program<'_> {
        match Self::program_fact(program) {
            Some(fact) => self.get(fact).map_or(Program::Missing, Program::Installed),
            None => Program::Unmeasured,
        }
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
            CarryKey::Other { tool, .. } if tool == PLATFORM_TOOL => self.platform_key(),
            CarryKey::Other { tool, .. } if tool == NODE_ABI_TOOL => self.node_abi_key(),
            CarryKey::Other { .. } | CarryKey::Unmeasured => None,
        };
        key.unwrap_or(CarryKey::Unmeasured)
    }
}

/// Says which field of two carry keys differs, in the words a decision line prints.
#[must_use]
pub fn key_difference(source: &CarryKey, target: &CarryKey) -> String {
    match (source, target) {
        (CarryKey::Unmeasured, _) => "the source measured no key".to_owned(),
        (_, CarryKey::Unmeasured) => "the target lacks a tool the key needs".to_owned(),
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
                format!("node major {source_node} on the source, {target_node} on the target")
            } else if source_pm != target_pm {
                format!("{manager} major {source_pm} on the source, {target_pm} on the target")
            } else if source_store != target_store {
                format!("pnpm store {source_store:?} on the source, {target_store:?} on the target")
            } else {
                "keys are equal".to_owned()
            }
        }
        (CarryKey::Rust { cargo_home: a, .. }, CarryKey::Rust { cargo_home: b, .. }) if a != b => {
            format!("CARGO_HOME {a} on the source, {b} on the target")
        }
        (CarryKey::Rust { .. }, CarryKey::Rust { .. }) if source != target => {
            "rustc -vV differs".to_owned()
        }
        (
            CarryKey::Python {
                python_version: a,
                python_home: home_a,
            },
            CarryKey::Python {
                python_version: b,
                python_home: home_b,
            },
        ) if source != target => {
            format!("{a} at {home_a} on the source, {b} at {home_b} on the target")
        }
        (
            CarryKey::Other { tool, version: a },
            CarryKey::Other {
                tool: target_tool,
                version: b,
            },
        ) if tool == target_tool && a != b => {
            let what = if tool == PLATFORM_TOOL {
                String::new()
            } else {
                format!("{tool} ")
            };
            format!("{what}{a} on the source, {b} on the target")
        }
        _ if source == target => "keys are equal".to_owned(),
        _ => "the keys differ".to_owned(),
    }
}

/// What a capture records per cache: detection says each cache's
/// ecosystem and what fills it, the facts give the strings.
pub struct MeasuredKeys {
    facts: NodeFacts,
    detected: BTreeMap<RelPath, DetectedCache>,
}

/// What detection says about one cache.
struct DetectedCache {
    ecosystem: Ecosystem,
    remake: Remake,
    /// The files at the root its steps read (`Detection::input_files`).
    inputs: Vec<&'static str>,
}

impl MeasuredKeys {
    /// Keys for the workspace `detection` describes, measured as `facts`.
    #[must_use]
    pub fn new(facts: NodeFacts, detection: Option<&Detection>) -> Self {
        let detected = detection
            .map(|d| {
                d.caches()
                    .iter()
                    .map(|cache| {
                        let remake = match &cache.completion {
                            Completion::Sentinel { filled_by } => {
                                let steps: Vec<SetupStep> = filled_by
                                    .iter()
                                    .filter_map(|index| d.spec().setup.get(*index).cloned())
                                    .collect();
                                if steps.is_empty() {
                                    Remake::Nothing
                                } else {
                                    // A cache whose health cannot be read
                                    // proves no finished install.
                                    let installed =
                                        fabric_env::materialize::cache_health(d, &cache.path)
                                            .is_ok_and(|health| {
                                                health == Some(CacheHealth::Healthy)
                                            });
                                    Remake::Setup { steps, installed }
                                }
                            }
                            // A declared cache no step fills holds whatever
                            // the user put there (ADR 0012, 2026-10-20).
                            Completion::Content => match cache.origin {
                                CacheOrigin::Declared => Remake::Nothing,
                                CacheOrigin::Detected => Remake::NextBuild,
                            },
                        };
                        let detected = DetectedCache {
                            ecosystem: cache.ecosystem,
                            remake,
                            inputs: d.input_files(&cache.path).to_vec(),
                        };
                        (cache.path.clone(), detected)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { facts, detected }
    }

    /// Turns each cache row that setup steps remake into one nothing
    /// remakes when `.fabric/env.toml`, or a file that cache's steps read
    /// (`Detection::input_files`), stayed on this node, and returns the
    /// paths it turned.
    ///
    /// The target learns the steps from its own detection over the
    /// restored tree. With the lockfile, the package manifest or the
    /// manager's configuration of a cache, or the override file, left
    /// behind by `.fabricignore` or a secret pattern, that detection does
    /// not see what this node saw and may plan no step for the path, so a
    /// rebuild the row promised would never run and the bytes, left out for
    /// it, would be lost. Nothing on the target is then known to make the
    /// path again, and it travels as the user's files. A file only another
    /// cache's steps read changes nothing here: a `requirements.txt` left
    /// behind turns `.venv` and leaves `node_modules` to its install, which
    /// alone fills the holes of a partial tree (issue #24). A row detection
    /// did not report counts every file detection reads.
    pub fn forget_lost_remakes(&self, manifest: &mut SnapshotManifest) -> Vec<RelPath> {
        let left = &manifest.left_behind;
        let stayed = |file: &str| {
            RelPath::parse(file).is_ok_and(|file| {
                left.iter()
                    .any(|left| file.starts_with_ignore_ascii_case(&left.path))
            })
        };
        let mut turned = Vec::new();
        for cache in &mut manifest.caches {
            if !matches!(cache.remake, Remake::Setup { .. }) {
                continue;
            }
            let inputs = self
                .detected
                .get(&cache.path)
                .map_or(fabric_env::detect::READ_FILES, |detected| {
                    detected.inputs.as_slice()
                });
            if stayed(OVERRIDE_FILE) || inputs.iter().any(|file| stayed(file)) {
                cache.remake = Remake::Nothing;
                turned.push(cache.path.clone());
            }
        }
        turned
    }

    /// The key of a cache the store's classes found and detection did not:
    /// a `node_modules` without a lockfile keys on the addon ABI, a `.venv`
    /// on the interpreter, and anything else on the platform.
    fn undetected_key(&self, class: CacheClass, path: &RelPath) -> Option<CarryKey> {
        let name = path.as_str().rsplit('/').next().unwrap_or_default();
        match class {
            CacheClass::Deps if name.eq_ignore_ascii_case("node_modules") => {
                self.facts.node_abi_key()
            }
            CacheClass::Deps if name.eq_ignore_ascii_case(".venv") => self.facts.python_key(),
            CacheClass::Deps | CacheClass::Build | CacheClass::ToolCache => {
                self.facts.platform_key()
            }
        }
    }
}

impl CarryKeys for MeasuredKeys {
    fn key(&self, class: CacheClass, path: &RelPath) -> CarryKey {
        let key = match self.detected.get(path).map(|detected| detected.ecosystem) {
            Some(Ecosystem::Node(manager)) => self.facts.node_key(manager_name(manager)),
            Some(Ecosystem::Rust) => self.facts.rust_key(),
            Some(Ecosystem::Python) => self.facts.python_key(),
            Some(Ecosystem::Other) => self.facts.platform_key(),
            None => self.undetected_key(class, path),
        };
        key.unwrap_or(CarryKey::Unmeasured)
    }

    /// A cache detection does not know is made again by nothing when it
    /// is a dependency tree, which then has no lockfile, and by the next
    /// build or tool run otherwise.
    fn remake(&self, class: CacheClass, path: &RelPath) -> Remake {
        match self.detected.get(path) {
            Some(detected) => detected.remake.clone(),
            None => match class {
                CacheClass::Deps => Remake::Nothing,
                CacheClass::Build | CacheClass::ToolCache => Remake::NextBuild,
            },
        }
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
            "node major 22 on the source, 26 on the target"
        );
    }

    #[test]
    fn a_pnpm_tree_needs_pnpm_on_the_target() {
        let source = pro().node_key("pnpm").unwrap();
        let air = NodeFacts::from_pairs([(name::NODE, "v22.1.0"), (name::NPM, "10.0.0")]);
        assert_eq!(air.key_like(&source), CarryKey::Unmeasured);
        assert!(!source.matches(&air.key_like(&source)));
    }

    /// A `node_modules` detection does not know has no lockfile, so
    /// nothing makes it again: the key is the addon ABI, read only for a
    /// warning, and the remake is `Nothing`, which the carry decision
    /// always carries (gate finding F6).
    #[test]
    fn a_dependency_tree_detection_does_not_know_is_the_users_files() {
        let facts = NodeFacts::from_pairs([(name::NODE_ABI, "127"), (name::OS, "macos")]);
        let keys = MeasuredKeys::new(facts, None);
        let node_modules = RelPath::parse("node_modules").unwrap();
        assert_eq!(
            keys.key(CacheClass::Deps, &node_modules),
            CarryKey::Other {
                tool: NODE_ABI_TOOL.to_owned(),
                version: "127".to_owned()
            }
        );
        assert_eq!(
            keys.remake(CacheClass::Deps, &node_modules),
            Remake::Nothing
        );
    }

    /// Build output detection does not know, such as `dist`, keys on the
    /// platform and the next build makes it again (gate finding F7).
    #[test]
    fn build_output_detection_does_not_know_keys_on_the_platform() {
        let keys = MeasuredKeys::new(pro(), None);
        let dist = RelPath::parse("dist").unwrap();
        assert_eq!(
            keys.key(CacheClass::Build, &dist),
            CarryKey::Other {
                tool: PLATFORM_TOOL.to_owned(),
                version: "macos-aarch64".to_owned()
            }
        );
        assert_eq!(keys.remake(CacheClass::Build, &dist), Remake::NextBuild);
    }

    /// The keys detection gives a workspace with an npm lockfile beside a
    /// `requirements.txt`, and a snapshot of it whose rows are `.venv` and
    /// `node_modules`, each filled by its own steps, and a `dist` the next
    /// build makes, with `left` stayed behind.
    fn two_ecosystems(
        left: &[(&str, fabric_core::snapshot::LeaveReason)],
    ) -> (MeasuredKeys, SnapshotManifest) {
        use fabric_core::snapshot::{CacheEntry, CarryState, HarnessState, LeftBehind};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "six\n").unwrap();
        let detection = fabric_env::detect::detect(dir.path(), &[]).unwrap();
        let keys = MeasuredKeys::new(pro(), Some(&detection));
        let row = |path: &str, class| {
            let path = RelPath::parse(path).unwrap();
            CacheEntry {
                class,
                carry: CarryState::Carried,
                key: keys.key(class, &path),
                remake: keys.remake(class, &path),
                path,
                native_files: 0,
            }
        };
        let manifest = SnapshotManifest {
            job_id: fabric_core::id::JobId::from_bytes([1; 16]),
            root: AbsPath::parse("/w").unwrap(),
            parent: None,
            created_at: 0,
            created_by: fabric_core::id::NodeId::from_bytes([2; 32]),
            platform: pro().platform(),
            git: None,
            files: Vec::new(),
            harness_state: HarnessState::default(),
            env_spec: None,
            caches: vec![
                row(".venv", CacheClass::Deps),
                row("dist", CacheClass::Build),
                row("node_modules", CacheClass::Deps),
            ],
            left_behind: left
                .iter()
                .map(|(path, reason)| LeftBehind {
                    path: RelPath::parse(path).unwrap(),
                    reason: *reason,
                })
                .collect(),
        };
        for cache in [0, 2] {
            assert!(
                matches!(manifest.caches[cache].remake, Remake::Setup { .. }),
                "{:?}",
                manifest.caches[cache]
            );
        }
        (keys, manifest)
    }

    /// A row setup steps remake keeps its steps while what its steps read
    /// travels, and becomes the user's files once its lockfile, the
    /// override file, or a directory that holds one of those files, stayed
    /// behind: the target's detection would plan no step for it (review of
    /// PR #11).
    #[test]
    fn a_rebuild_whose_inputs_stayed_behind_becomes_the_users_files() {
        use fabric_core::snapshot::LeaveReason;
        let (keys, mut kept) = two_ecosystems(&[(".env", LeaveReason::Secret)]);
        assert!(keys.forget_lost_remakes(&mut kept).is_empty());
        for (left, turned) in [
            ("package-lock.json", &["node_modules"][..]),
            (".fabric", &[".venv", "node_modules"][..]),
        ] {
            let (keys, mut lost) = two_ecosystems(&[(left, LeaveReason::Excluded)]);
            let turned: Vec<RelPath> = turned.iter().map(|p| RelPath::parse(p).unwrap()).collect();
            assert_eq!(keys.forget_lost_remakes(&mut lost), turned, "{left}");
            for cache in &lost.caches {
                if turned.contains(&cache.path) {
                    assert_eq!(cache.remake, Remake::Nothing, "{left}");
                }
            }
            assert_eq!(lost.caches[1].remake, Remake::NextBuild, "{left}");
        }
    }

    /// A file only one cache's steps read forgets only that cache's
    /// rebuild. With every rebuild forgotten, a `requirements.txt` left
    /// behind turned a partial `node_modules`, which holds its sentinel,
    /// into the user's files: it was written with its holes, and the
    /// target's detection, which still saw the lockfile, trusted the
    /// sentinel and never filled them (issue #24).
    #[test]
    fn a_file_of_another_ecosystem_left_behind_keeps_the_rebuild() {
        use fabric_core::snapshot::{CarryState, LeaveReason};
        let (keys, mut manifest) = two_ecosystems(&[("requirements.txt", LeaveReason::Excluded)]);
        let node_modules = &mut manifest.caches[2];
        node_modules.carry = CarryState::Partial;
        if let Remake::Setup { installed, .. } = &mut node_modules.remake {
            *installed = true;
        }
        assert_eq!(
            keys.forget_lost_remakes(&mut manifest),
            [RelPath::parse(".venv").unwrap()]
        );
        let decision = crate::carry::decide(&manifest.caches[2], &manifest.platform, &pro());
        assert!(
            matches!(decision, crate::carry::CarryDecision::Rebuild { .. }),
            "{decision:?}"
        );
    }

    /// A tree with a lockfile records the install step that fills it.
    #[test]
    fn a_detected_tree_records_the_step_that_fills_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        let detection = fabric_env::detect::detect(dir.path(), &[]).unwrap();
        let keys = MeasuredKeys::new(pro(), Some(&detection));
        let node_modules = RelPath::parse("node_modules").unwrap();
        let Remake::Setup { steps, installed } = keys.remake(CacheClass::Deps, &node_modules)
        else {
            panic!("no setup step recorded");
        };
        assert!(!installed, "no install ran");
        // The sentinel materialize writes after the install proves it.
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "1;\n").unwrap();
        let sentinel = fabric_env::materialize::sentinel_text(&detection, &node_modules).unwrap();
        std::fs::write(
            dir.path()
                .join("node_modules")
                .join(fabric_env::materialize::SENTINEL),
            sentinel,
        )
        .unwrap();
        let keys = MeasuredKeys::new(pro(), Some(&detection));
        assert!(matches!(
            keys.remake(CacheClass::Deps, &node_modules),
            Remake::Setup {
                installed: true,
                ..
            }
        ));
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].program.to_string(), "npm");
        assert!(matches!(
            keys.key(CacheClass::Deps, &node_modules),
            CarryKey::Node { .. }
        ));
    }

    #[test]
    fn a_major_is_the_leading_digits_of_a_version() {
        assert_eq!(major("v22.23.2").as_deref(), Some("22"));
        assert_eq!(major("11.23.0").as_deref(), Some("11"));
        assert_eq!(major("nightly"), None);
    }

    /// A relative `PATH` entry names a directory relative to the daemon's
    /// working directory here, and to the workspace once the harness
    /// starts, so the measured binary and the launched one could differ.
    /// It is skipped, as the harness's own lookup skips it.
    #[test]
    fn a_relative_path_entry_is_never_searched() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("fabric-test-tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let up = "../".repeat(cwd.components().count());
        let relative = format!(
            "{up}{}",
            dir.path().display().to_string().trim_start_matches('/')
        );
        assert!(Path::new(&relative).join("fabric-test-tool").is_file());
        assert_eq!(
            find_on_path("fabric-test-tool", &OsString::from(&relative)),
            None
        );
        assert_eq!(
            find_on_path("fabric-test-tool", dir.path().as_os_str()),
            Some(tool)
        );
    }
}
