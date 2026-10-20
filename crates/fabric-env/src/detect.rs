//! Detection of an `EnvSpec` from the files a workspace already has (ADR 0007).
//!
//! Invariant: detection reads a fixed list of files at the workspace root
//! and nothing else. It reads regular files only: a symbolic link in that
//! list is refused, because a link can point at a file the secret policy
//! denies or at one outside the workspace, and either would break the next
//! rule. It never opens a file the secret policy denies, never reads an
//! environment variable and never runs a program, so the same files give
//! the same spec on every node, and the spec it returns has passed
//! `EnvSpec::validate`. Every tool name and pin it keeps is short,
//! printable and not shaped like a credential, because the spec travels to
//! other nodes; `ToolName` and `ToolVersion` hold that rule by type.
//!
//! What it reads, and what each file gives:
//!
//! | File | Gives |
//! |---|---|
//! | `package.json` with a lockfile | the package manager's install step, `node_modules` as a cache |
//! | `Cargo.toml` | `cargo fetch`, with `--locked` when `Cargo.lock` exists; `target` as a cache |
//! | `uv.lock`, else `requirements.txt` | `uv sync --frozen`, or a `uv` venv and `uv pip install -r`; `.venv` as a cache |
//! | `go.mod` | `go mod download`; no cache inside the workspace |
//! | `mise.toml`, `.tool-versions`, `rust-toolchain.toml`, `.node-version`, `.nvmrc`, `.python-version`, `go.mod`, `package.json` | tool pins, in that order of precedence |
//! | `.devcontainer/devcontainer.json`, `.devcontainer.json` | recorded, not interpreted |
//! | `.fabric/env.toml` | the user's overrides, which win over everything above |

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fabric_core::env::{
    EnvSpec, EnvSpecError, PinError, RunWhen, SecretPattern, SetupStep, Tool, ToolName, ToolSource,
    ToolVersion,
};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use fabric_core::snapshot::CacheClass;
use toml::Table;

use crate::overrides::{
    DeclaredCache, DeclaredEcosystem, EnvOverride, OverrideError, PackageManagerName,
    canonical_tool, line_and_column,
};
use crate::secret::{Scope, SecretPolicy};

/// Where the override file lives, relative to the workspace root.
pub const OVERRIDE_FILE: &str = ".fabric/env.toml";

/// The largest file detection reads; a bigger one is refused rather than loaded.
const MAX_FILE_LEN: u64 = 4 << 20;

/// Why a workspace's environment could not be detected.
#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    /// The workspace root does not resolve.
    #[error("the workspace root does not resolve")]
    Root {
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A file exists and could not be read.
    #[error("could not read `{file}`")]
    Read {
        /// The file, relative to the root.
        file: &'static str,
        /// The IO error.
        #[source]
        source: io::Error,
    },
    /// A file in the fixed list is a symbolic link or another non-regular file.
    #[error(
        "`{file}` is not a regular file; detection does not follow links, replace it with the file itself"
    )]
    NotARegularFile {
        /// The file, relative to the root.
        file: &'static str,
    },
    /// A tool name in a detected file was refused; the name itself is never quoted.
    #[error("the tool name of pin {entry} in `{file}` {reason}")]
    ToolName {
        /// The file, relative to the root.
        file: &'static str,
        /// Zero-based index of the pin among the pins the file gives, in file order.
        entry: usize,
        /// What is wrong with it.
        reason: PinError,
    },
    /// A version pin in a detected file was refused; the pin itself is never quoted.
    #[error("the version of pin {entry} in `{file}` {reason}")]
    Pin {
        /// The file, relative to the root.
        file: &'static str,
        /// Zero-based index of the pin among the pins the file gives, in file order.
        entry: usize,
        /// What is wrong with it.
        reason: PinError,
    },
    /// A file is larger than detection reads.
    #[error("`{file}` is larger than {MAX_FILE_LEN} bytes")]
    TooLarge {
        /// The file, relative to the root.
        file: &'static str,
    },
    /// A file is not UTF-8.
    #[error("`{file}` is not UTF-8")]
    NotUtf8 {
        /// The file, relative to the root.
        file: &'static str,
    },
    /// A JSON file does not parse.
    #[error("`{file}` is not valid JSON at line {line}, column {column}")]
    Json {
        /// The file, relative to the root.
        file: &'static str,
        /// One-based line of the error.
        line: usize,
        /// One-based column of the error.
        column: usize,
    },
    /// A TOML file other than the override file does not parse.
    #[error("`{file}` is not valid TOML at line {line}, column {column}")]
    Toml {
        /// The file, relative to the root.
        file: &'static str,
        /// One-based line of the error.
        line: usize,
        /// One-based column of the error.
        column: usize,
    },
    /// `packageManager` in `package.json` is not `<npm|pnpm|yarn|bun>@<version>`.
    #[error(
        "`packageManager` in package.json must be npm, pnpm, yarn or bun followed by `@<version>`"
    )]
    BadPackageManagerField,
    /// More than one lockfile and nothing that says which manager owns the tree.
    #[error(
        "found {}; set `packageManager` in package.json or `package_manager` in {OVERRIDE_FILE}",
        lockfiles.join(", ")
    )]
    AmbiguousLockfiles {
        /// The lockfiles found at the root.
        lockfiles: Vec<&'static str>,
    },
    /// The chosen manager's lockfile is absent while another manager's is present.
    #[error(
        "{manager} was chosen by {because} but only {} exist; add its lockfile or set `package_manager` in {OVERRIDE_FILE}",
        found.join(", ")
    )]
    LockfileMissing {
        /// The manager that was chosen.
        manager: NodeManager,
        /// What chose it.
        because: ManagerSource,
        /// The lockfiles that do exist.
        found: Vec<&'static str>,
    },
    /// A declared cache path lies inside, or contains, a cache detection found.
    #[error(
        "declared cache `{declared}` nests with detected cache `{detected}`; a cache may not contain another"
    )]
    NestedCache {
        /// The declared path.
        declared: RelPath,
        /// The detected path it nests with.
        detected: RelPath,
    },
    /// A `[[cache]]` entry without `ecosystem` in a workspace where more than one ecosystem was detected.
    #[error(
        "`cache[{index}]` could be made by more than one detected ecosystem; set its `ecosystem` to node, rust, python or other"
    )]
    AmbiguousCacheEcosystem {
        /// Zero-based index of the entry in `[[cache]]`.
        index: usize,
    },
    /// A `[[cache]]` entry names `ecosystem = "node"` and no package manager was detected.
    #[error(
        "`cache[{index}].ecosystem` is node, but no package manager was detected to measure its carry key"
    )]
    CacheEcosystemNotDetected {
        /// Zero-based index of the entry in `[[cache]]`.
        index: usize,
    },
    /// A `[[cache]]` entry for a detected path names another ecosystem than detection found.
    #[error("`cache[{index}].ecosystem` differs from the ecosystem detection found for that path")]
    CacheEcosystemConflict {
        /// Zero-based index of the entry in `[[cache]]`.
        index: usize,
    },
    /// `package_manager` is set in `.fabric/env.toml` but the root has no `package.json` for it to apply to.
    #[error("`package_manager` is set in {OVERRIDE_FILE} but there is no package.json at the root")]
    PackageManagerWithoutPackageJson,
    /// `.fabric/env.toml` was refused; the source says where and why.
    #[error("{OVERRIDE_FILE} was refused")]
    Override(#[from] OverrideError),
    /// The spec built from the workspace broke one of its own rules.
    #[error("detected environment spec is invalid")]
    Spec(#[from] EnvSpecError),
    /// A program or path name built into this module failed its own type's rules; a bug here.
    #[error("built-in name `{name}` is not valid")]
    BuiltIn {
        /// The name.
        name: &'static str,
    },
}

/// A JavaScript package manager, resolved to the line whose install command differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeManager {
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn 1.
    YarnClassic,
    /// Yarn 2 and later.
    YarnBerry,
    /// Bun.
    Bun,
}

impl fmt::Display for NodeManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NodeManager::Npm => "npm",
            NodeManager::Pnpm => "pnpm",
            NodeManager::YarnClassic => "yarn (classic)",
            NodeManager::YarnBerry => "yarn (berry)",
            NodeManager::Bun => "bun",
        })
    }
}

impl NodeManager {
    /// Every manager.
    pub(crate) const ALL: [NodeManager; 5] = [
        NodeManager::Pnpm,
        NodeManager::Npm,
        NodeManager::YarnClassic,
        NodeManager::YarnBerry,
        NodeManager::Bun,
    ];

    /// Returns the name package.json and the override file use for the manager.
    pub fn name(self) -> PackageManagerName {
        match self {
            NodeManager::Npm => PackageManagerName::Npm,
            NodeManager::Pnpm => PackageManagerName::Pnpm,
            NodeManager::YarnClassic | NodeManager::YarnBerry => PackageManagerName::Yarn,
            NodeManager::Bun => PackageManagerName::Bun,
        }
    }

    /// Returns the lockfile names the manager writes.
    pub(crate) fn lockfiles(self) -> &'static [&'static str] {
        match self {
            NodeManager::Npm => &["package-lock.json"],
            NodeManager::Pnpm => &["pnpm-lock.yaml"],
            NodeManager::YarnClassic | NodeManager::YarnBerry => &["yarn.lock"],
            NodeManager::Bun => &["bun.lock", "bun.lockb"],
        }
    }

    /// Returns the files the manager writes inside `node_modules` as an install finishes.
    ///
    /// A first install killed halfway leaves `node_modules` without them,
    /// which is how materialize tells a finished tree from a torn one. Bun
    /// writes none, so a Bun tree is judged by materialize's own sentinel.
    pub(crate) fn install_markers(self) -> &'static [&'static str] {
        match self {
            NodeManager::Npm => &[".package-lock.json"],
            NodeManager::Pnpm => &[".modules.yaml"],
            NodeManager::YarnClassic => &[".yarn-integrity"],
            NodeManager::YarnBerry => &[".yarn-state.yml"],
            NodeManager::Bun => &[],
        }
    }

    /// Returns the frozen-lockfile install step, which never rewrites the lockfile.
    fn install_step(self) -> Result<SetupStep, DetectError> {
        let args: &[&str] = match self {
            NodeManager::Npm => &["ci"],
            NodeManager::Pnpm | NodeManager::YarnClassic | NodeManager::Bun => {
                &["install", "--frozen-lockfile"]
            }
            NodeManager::YarnBerry => &["install", "--immutable"],
        };
        step(self.name().as_str(), args)
    }
}

/// What decided the package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerSource {
    /// `package_manager` in `.fabric/env.toml`.
    OverrideFile,
    /// `packageManager` in `package.json`.
    PackageManagerField,
    /// The only lockfile at the root.
    Lockfile,
}

impl fmt::Display for ManagerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ManagerSource::OverrideFile => "`package_manager` in .fabric/env.toml",
            ManagerSource::PackageManagerField => "`packageManager` in package.json",
            ManagerSource::Lockfile => "the only lockfile",
        })
    }
}

/// The package manager detection chose, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerChoice {
    /// The manager.
    pub manager: NodeManager,
    /// What decided it.
    pub because: ManagerSource,
}

/// The ecosystem that produces a cache path; it decides which carry key the store measures (ADR 0012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    /// Made with this JavaScript package manager and its Node: the `Node` key.
    Node(NodeManager),
    /// Made by Cargo: the `Rust` key.
    Rust,
    /// Made with the workspace's Python: the `Python` key.
    Python,
    /// Made by nothing an ecosystem rule knows, as the user declared: carried when the platform matches.
    Other,
}

/// Whether an ecosystem rule found a cache path or the user declared it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOrigin {
    /// Found by a detection rule: `node_modules`, `target` or `.venv`.
    Detected,
    /// Declared in `.fabric/env.toml` and not also detected.
    Declared,
}

impl Ecosystem {
    /// Returns the files one of which the installer itself writes as it finishes; empty when it writes none.
    ///
    /// The markers follow the manager detection chose, not the lockfiles on
    /// disk: a Bun tree next to a leftover `package-lock.json` needs no
    /// `.package-lock.json`. `.venv` has none: `pyvenv.cfg` is the first
    /// file `uv venv` writes, before any package, so it proves nothing.
    pub fn install_markers(self) -> &'static [&'static str] {
        match self {
            Ecosystem::Node(manager) => manager.install_markers(),
            Ecosystem::Rust | Ecosystem::Python | Ecosystem::Other => &[],
        }
    }
}

/// How materialize tells a finished cache from a torn one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// One of these files, which the installer itself writes as it finishes.
    Marker(&'static [&'static str]),
    /// The sentinel materialize writes once the steps that fill the cache have exited 0.
    Sentinel,
    /// Content alone: no setup step fills the cache, so no step can leave it torn.
    Content,
}

/// One cache path, its class for the snapshot policy, and its ecosystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePath {
    /// The path, relative to the root.
    pub path: RelPath,
    /// Its class for the snapshot policy (ADR 0004).
    pub class: CacheClass,
    /// The ecosystem whose carry key applies to it.
    pub ecosystem: Ecosystem,
    /// Whether detection found it or the user declared it.
    pub origin: CacheOrigin,
}

impl CachePath {
    /// Returns the secret scope capture applies under this path.
    ///
    /// Only a cache an ecosystem rule found gets the narrowed list: the
    /// measurement that justified it was made on `node_modules` trees, and
    /// a path the user declared can be a mistake that holds the user's own
    /// `.env.local` or keys, such as `web` written for `web/node_modules`.
    pub fn secret_scope(&self) -> Scope {
        match self.origin {
            CacheOrigin::Declared => Scope::Work,
            CacheOrigin::Detected => Scope::Cache,
        }
    }
}

/// What a setup step fills, which decides when a `CachesMissing` step runs.
///
/// This lives in the detection report, not in the spec: the report is
/// rebuilt from the same files on every node, and linking a step to its
/// cache in the spec would change the spec format for something every
/// node can derive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepScope {
    /// Fills this cache in the workspace; runs when that cache is not whole.
    Fills(RelPath),
    /// Fills a per-node cache outside the workspace that a build turns into this cache; runs when this cache is not whole.
    Warms(RelPath),
    /// Fills a per-node cache outside the workspace and nothing in it; runs when any cache is not whole, or when there is none.
    Outside,
    /// Declared by the user, who did not say what it fills; runs when any cache is not whole, or when there is none.
    Declared,
}

/// A version pin found in one file, kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The tool, with `nodejs` read as `node` and `golang` as `go`, from every file including the override one.
    pub tool: ToolName,
    /// The version text as the file wrote it.
    pub version: ToolVersion,
    /// The file it came from, relative to the root.
    pub file: &'static str,
    /// Whether the user declared it in the override file or detection found it.
    pub source: ToolSource,
}

/// Everything detection found in a workspace.
///
/// The fields are read-only so that the report stays consistent: the spec's
/// `cache_paths` are the paths of `caches`, in the same order, and nothing
/// can change one without the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    root: PathBuf,
    spec: EnvSpec,
    step_scopes: Vec<StepScope>,
    caches: Vec<CachePath>,
    pins: Vec<Pin>,
    node_manager: Option<ManagerChoice>,
    devcontainer: Option<RelPath>,
    secrets: SecretPolicy,
}

impl Detection {
    /// Returns the workspace root the report describes, resolved when detection ran.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the spec: one pin per tool, the setup steps, the variable names and the cache paths.
    pub fn spec(&self) -> &EnvSpec {
        &self.spec
    }

    /// Returns what each setup step fills, one per step of the spec and in the same order.
    pub fn step_scopes(&self) -> &[StepScope] {
        &self.step_scopes
    }

    /// Returns how materialize proves `cache` whole.
    ///
    /// The installer's own marker when its ecosystem has one. Otherwise the
    /// sentinel when a setup step fills the cache: a detected step that
    /// fills that path, or a step the user declared, which stands in for
    /// the detected steps and does not say what it fills. A detected
    /// `target` is made by a build, never by setup, so no declared step
    /// counts as filling it. Otherwise content alone.
    pub fn completion(&self, cache: &CachePath) -> Completion {
        let markers = cache.ecosystem.install_markers();
        if !markers.is_empty() {
            return Completion::Marker(markers);
        }
        let built = cache.origin == CacheOrigin::Detected && cache.ecosystem == Ecosystem::Rust;
        let filled = self.step_scopes.iter().any(|scope| match scope {
            StepScope::Fills(path) => path == &cache.path,
            StepScope::Declared => !built,
            StepScope::Warms(_) | StepScope::Outside => false,
        });
        if filled {
            Completion::Sentinel
        } else {
            Completion::Content
        }
    }

    /// Returns the spec's cache paths with their class and ecosystem, sorted by path.
    pub fn caches(&self) -> &[CachePath] {
        &self.caches
    }

    /// Returns every pin found, in order of precedence; the spec holds the first one per tool.
    pub fn pins(&self) -> &[Pin] {
        &self.pins
    }

    /// Returns the package manager and what chose it, when the root has a `package.json` and a lockfile.
    pub fn node_manager(&self) -> Option<ManagerChoice> {
        self.node_manager
    }

    /// Returns the devcontainer file, recorded for a later import and not interpreted.
    pub fn devcontainer(&self) -> Option<&RelPath> {
        self.devcontainer.as_ref()
    }

    /// Returns the secret policy, widened by the override file.
    pub fn secrets(&self) -> &SecretPolicy {
        &self.secrets
    }
}

/// Detects the environment of the workspace at `root`, applying `.fabric/env.toml` when present.
///
/// `previous` is the job's last spec, or `None` for a job that has none
/// yet. Every secret pattern it holds stays in force and in the new spec,
/// even when the override file no longer lists it: a job's secret list
/// only grows, and the override file is a file in the working tree that
/// the agent can rewrite.
pub fn detect(root: &Path, previous: Option<&EnvSpec>) -> Result<Detection, DetectError> {
    let root = fs::canonicalize(root).map_err(|source| DetectError::Root { source })?;
    let root = root.as_path();
    let overrides = match read_text(root, OVERRIDE_FILE)? {
        Some(text) => EnvOverride::parse(&text)?,
        None => EnvOverride::default(),
    };
    let package = read_package_json(root)?;
    let pins = collect_pins(root, &overrides, package.as_ref())?;

    let mut setup = Vec::new();
    let mut caches = Vec::new();
    let node_manager = match (&package, overrides.package_manager) {
        (Some(package), declared) => choose_node_manager(root, package, declared)?,
        (None, Some(_)) => return Err(DetectError::PackageManagerWithoutPackageJson),
        (None, None) => None,
    };
    if let Some(choice) = node_manager {
        let node_modules = rel("node_modules")?;
        setup.push((
            choice.manager.install_step()?,
            StepScope::Fills(node_modules.clone()),
        ));
        caches.push(CachePath {
            path: node_modules,
            class: CacheClass::Deps,
            ecosystem: Ecosystem::Node(choice.manager),
            origin: CacheOrigin::Detected,
        });
    }
    detect_rust(root, &mut setup, &mut caches)?;
    detect_python(root, &mut setup, &mut caches)?;
    if exists(root, "go.mod")? {
        setup.push((step("go", &["mod", "download"])?, StepScope::Outside));
    }

    merge_declared_caches(&mut caches, &overrides.caches, node_manager)?;
    caches.sort_by(|a, b| a.path.cmp(&b.path));

    if let Some(declared) = &overrides.setup {
        setup = declared
            .iter()
            .map(|step| (step.clone(), StepScope::Declared))
            .collect();
    }
    let (setup, step_scopes): (Vec<SetupStep>, Vec<StepScope>) = setup.into_iter().unzip();

    let secret_patterns = job_secret_patterns(previous, &overrides.secret_patterns);
    let mut secrets = SecretPolicy::default();
    for pattern in &secret_patterns {
        secrets.widen(pattern.clone());
    }

    let spec = EnvSpec {
        tools: chosen_tools(&pins),
        setup,
        env_var_names: overrides.env_var_names.clone(),
        cache_paths: caches.iter().map(|c| c.path.clone()).collect(),
        secret_patterns,
    };
    spec.validate()?;

    Ok(Detection {
        root: root.to_owned(),
        spec,
        step_scopes,
        caches,
        pins,
        node_manager,
        devcontainer: find_devcontainer(root)?,
        secrets,
    })
}

/// Adds the declared caches to the detected ones, giving each the ecosystem that makes it.
fn merge_declared_caches(
    caches: &mut Vec<CachePath>,
    declared_caches: &[DeclaredCache],
    node_manager: Option<ManagerChoice>,
) -> Result<(), DetectError> {
    let detected_ecosystems: Vec<Ecosystem> = caches.iter().map(|c| c.ecosystem).collect();
    for (index, declared) in declared_caches.iter().enumerate() {
        if let Some(found) = caches
            .iter_mut()
            .find(|c| c.path.eq_ignore_ascii_case(&declared.path))
        {
            // The ecosystem stays: it decides the carry key, and a declared
            // class must not let a `node_modules` travel to a node with
            // another Node major.
            if declared.ecosystem.is_some_and(|named| {
                resolve_ecosystem(named, node_manager) != Some(found.ecosystem)
            }) {
                return Err(DetectError::CacheEcosystemConflict { index });
            }
            found.class = declared.class;
            continue;
        }
        if let Some(found) = caches.iter().find(|c| {
            c.origin == CacheOrigin::Detected
                && (declared.path.starts_with_ignore_ascii_case(&c.path)
                    || c.path.starts_with_ignore_ascii_case(&declared.path))
        }) {
            return Err(DetectError::NestedCache {
                declared: declared.path.clone(),
                detected: found.path.clone(),
            });
        }
        let ecosystem = match (declared.ecosystem, detected_ecosystems.as_slice()) {
            (Some(named), _) => resolve_ecosystem(named, node_manager)
                .ok_or(DetectError::CacheEcosystemNotDetected { index })?,
            (None, []) => Ecosystem::Other,
            (None, [only]) => *only,
            (None, _) => return Err(DetectError::AmbiguousCacheEcosystem { index }),
        };
        caches.push(CachePath {
            path: declared.path.clone(),
            class: declared.class,
            ecosystem,
            origin: CacheOrigin::Declared,
        });
    }
    Ok(())
}

/// Returns the job's secret patterns: every one its previous spec held, plus the override file's, sorted.
fn job_secret_patterns(
    previous: Option<&EnvSpec>,
    declared: &[SecretPattern],
) -> Vec<SecretPattern> {
    let recorded = previous.map_or(&[][..], |spec| spec.secret_patterns.as_slice());
    let dropped = recorded.iter().filter(|p| !declared.contains(p)).count();
    if dropped > 0 {
        tracing::warn!(
            dropped,
            "secret patterns this job recorded are missing from {OVERRIDE_FILE}; they stay in force"
        );
    }
    let mut secret_patterns: Vec<SecretPattern> =
        recorded.iter().chain(declared).cloned().collect();
    secret_patterns.sort();
    secret_patterns.dedup();
    secret_patterns
}

/// Returns the ecosystem a `[[cache]]` entry names, or `None` for `node` when no package manager was detected.
fn resolve_ecosystem(named: DeclaredEcosystem, node: Option<ManagerChoice>) -> Option<Ecosystem> {
    match named {
        DeclaredEcosystem::Node => node.map(|choice| Ecosystem::Node(choice.manager)),
        DeclaredEcosystem::Rust => Some(Ecosystem::Rust),
        DeclaredEcosystem::Python => Some(Ecosystem::Python),
        DeclaredEcosystem::Other => Some(Ecosystem::Other),
    }
}

/// Collects every pin, highest precedence first.
fn collect_pins(
    root: &Path,
    overrides: &EnvOverride,
    package: Option<&PackageJson>,
) -> Result<Vec<Pin>, DetectError> {
    let mut pins: Vec<Pin> = overrides
        .tools
        .iter()
        .enumerate()
        .map(|(entry, (tool, version))| {
            let mut declared = pin(tool.as_str(), version.as_str(), OVERRIDE_FILE, entry)?;
            declared.source = ToolSource::Declared;
            Ok(declared)
        })
        .collect::<Result<_, DetectError>>()?;
    pins.extend(mise_pins(root)?);
    pins.extend(tool_versions_pins(root)?);
    pins.extend(rust_toolchain_pin(root)?);
    for (file, tool) in [
        (".node-version", "node"),
        (".nvmrc", "node"),
        (".python-version", "python"),
    ] {
        pins.extend(first_line_pin(root, file, tool)?);
    }
    pins.extend(go_mod_pin(root)?);
    if let Some(package) = package {
        let mut entry = 0;
        if let Some((name, version)) = &package.manager {
            pins.push(pin(name.as_str(), version, "package.json", entry)?);
            entry += 1;
        }
        if let Some(engine) = &package.engines_node {
            pins.push(pin("node", engine, "package.json", entry)?);
        }
    }
    Ok(pins)
}

/// Builds a detected pin after checking its tool name, read through its alias, and its version.
fn pin(tool: &str, version: &str, file: &'static str, entry: usize) -> Result<Pin, DetectError> {
    let tool = ToolName::parse(canonical_tool(tool)).map_err(|reason| DetectError::ToolName {
        file,
        entry,
        reason,
    })?;
    let version = ToolVersion::parse(version).map_err(|reason| DetectError::Pin {
        file,
        entry,
        reason,
    })?;
    Ok(Pin {
        tool,
        version,
        file,
        source: ToolSource::Detected,
    })
}

fn detect_rust(
    root: &Path,
    setup: &mut Vec<(SetupStep, StepScope)>,
    caches: &mut Vec<CachePath>,
) -> Result<(), DetectError> {
    if !exists(root, "Cargo.toml")? {
        return Ok(());
    }
    // `--locked` keeps a resume from rewriting Cargo.lock, as the frozen
    // installs do for JavaScript; without a lockfile there is nothing to keep.
    let args: &[&str] = if exists(root, "Cargo.lock")? {
        &["fetch", "--locked"]
    } else {
        &["fetch"]
    };
    let target = rel("target")?;
    // `cargo fetch` fills ~/.cargo, not `target`, but `target` is what a
    // build makes from it: while `target` is whole the fetch has nothing
    // to add, and a resume on a warm workspace runs no command.
    setup.push((step("cargo", args)?, StepScope::Warms(target.clone())));
    caches.push(CachePath {
        path: target,
        class: CacheClass::Build,
        ecosystem: Ecosystem::Rust,
        origin: CacheOrigin::Detected,
    });
    Ok(())
}

fn detect_python(
    root: &Path,
    setup: &mut Vec<(SetupStep, StepScope)>,
    caches: &mut Vec<CachePath>,
) -> Result<(), DetectError> {
    let venv = rel(".venv")?;
    let fills = || StepScope::Fills(venv.clone());
    if exists(root, "uv.lock")? {
        setup.push((step("uv", &["sync", "--frozen"])?, fills()));
    } else if exists(root, "requirements.txt")? {
        // A bare `pip install` would write into whichever Python is first
        // on PATH; uv puts the packages in the workspace's own `.venv`.
        setup.push((step("uv", &["venv", "--allow-existing"])?, fills()));
        setup.push((
            step("uv", &["pip", "install", "-r", "requirements.txt"])?,
            fills(),
        ));
    } else {
        return Ok(());
    }
    caches.push(CachePath {
        path: venv,
        class: CacheClass::Deps,
        ecosystem: Ecosystem::Python,
        origin: CacheOrigin::Detected,
    });
    Ok(())
}

fn find_devcontainer(root: &Path) -> Result<Option<RelPath>, DetectError> {
    for file in [".devcontainer/devcontainer.json", ".devcontainer.json"] {
        if exists(root, file)? {
            tracing::info!(file, "devcontainer found; recorded, not interpreted");
            return Ok(Some(rel(file)?));
        }
    }
    Ok(None)
}

/// Takes the first pin per tool, which is the declared one when the override file has it.
fn chosen_tools(pins: &[Pin]) -> Vec<Tool> {
    let mut chosen: BTreeMap<&ToolName, &Pin> = BTreeMap::new();
    for pin in pins {
        chosen.entry(&pin.tool).or_insert(pin);
    }
    chosen
        .into_values()
        .map(|pin| Tool {
            name: pin.tool.clone(),
            version: pin.version.clone(),
            source: pin.source,
        })
        .collect()
}

/// What detection takes from `package.json`.
struct PackageJson {
    manager: Option<(PackageManagerName, String)>,
    engines_node: Option<String>,
    yarn_major: Option<u64>,
}

fn read_package_json(root: &Path) -> Result<Option<PackageJson>, DetectError> {
    let Some(text) = read_text(root, "package.json")? else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| DetectError::Json {
            file: "package.json",
            line: err.line(),
            column: err.column(),
        })?;
    let mut manager = None;
    let mut yarn_major = None;
    if let Some(field) = value.get("packageManager") {
        let text = field.as_str().ok_or(DetectError::BadPackageManagerField)?;
        let (name, rest) = text
            .split_once('@')
            .ok_or(DetectError::BadPackageManagerField)?;
        let name = PackageManagerName::parse(name).ok_or(DetectError::BadPackageManagerField)?;
        // Corepack appends `+<algorithm>.<hash>`; that is integrity, not version.
        let version = rest.split_once('+').map_or(rest, |(v, _)| v);
        let major: String = version.chars().take_while(char::is_ascii_digit).collect();
        let major: u64 = major
            .parse()
            .map_err(|_| DetectError::BadPackageManagerField)?;
        if name == PackageManagerName::Yarn {
            yarn_major = Some(major);
        }
        manager = Some((name, version.to_owned()));
    }
    let engines_node = value
        .get("engines")
        .and_then(|engines| engines.get("node"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(Some(PackageJson {
        manager,
        engines_node,
        yarn_major,
    }))
}

/// Picks the manager: the override file, then `packageManager`, then the only lockfile.
///
/// Returns `None` when there is no lockfile at all: without one no install
/// can rebuild `node_modules`, so it is not a cache and travels as the
/// user's own files.
fn choose_node_manager(
    root: &Path,
    package: &PackageJson,
    declared: Option<PackageManagerName>,
) -> Result<Option<ManagerChoice>, DetectError> {
    // Ambiguity is decided on the managers that own a present lockfile, not
    // on the file names: Bun's `bun.lock` next to its older `bun.lockb` is
    // one manager, not two.
    let mut lockfiles = Vec::new();
    let mut owners = Vec::new();
    for manager in NodeManager::ALL {
        for &file in manager.lockfiles() {
            if !lockfiles.contains(&file) && exists(root, file)? {
                lockfiles.push(file);
                if !owners.contains(&manager.name()) {
                    owners.push(manager.name());
                }
            }
        }
    }
    if lockfiles.is_empty() {
        tracing::warn!("package.json has no lockfile; node_modules travels as the user's files");
        return Ok(None);
    }
    let berry = match package.yarn_major {
        Some(major) => major >= 2,
        None => exists(root, ".yarnrc.yml")?,
    };
    let resolve = |name| match name {
        PackageManagerName::Npm => NodeManager::Npm,
        PackageManagerName::Pnpm => NodeManager::Pnpm,
        PackageManagerName::Yarn if berry => NodeManager::YarnBerry,
        PackageManagerName::Yarn => NodeManager::YarnClassic,
        PackageManagerName::Bun => NodeManager::Bun,
    };
    let (name, because) = if let Some(name) = declared {
        (name, ManagerSource::OverrideFile)
    } else if let Some((name, _)) = &package.manager {
        (*name, ManagerSource::PackageManagerField)
    } else {
        match owners.as_slice() {
            [only] => (*only, ManagerSource::Lockfile),
            _ => return Err(DetectError::AmbiguousLockfiles { lockfiles }),
        }
    };
    let manager = resolve(name);
    if !owners.contains(&name) {
        return Err(DetectError::LockfileMissing {
            manager,
            because,
            found: lockfiles,
        });
    }
    Ok(Some(ManagerChoice { manager, because }))
}

/// Reads `[tools]` from `mise.toml`: a string, the first string of an array, or a table's `version`.
fn mise_pins(root: &Path) -> Result<Vec<Pin>, DetectError> {
    let Some(table) = read_toml(root, "mise.toml")? else {
        return Ok(Vec::new());
    };
    let Some(tools) = table.get("tools").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    tools
        .iter()
        .filter_map(|(tool, value)| match value {
            toml::Value::String(v) => Some((tool, v.as_str())),
            toml::Value::Array(items) => items
                .first()
                .and_then(toml::Value::as_str)
                .map(|v| (tool, v)),
            toml::Value::Table(t) => t
                .get("version")
                .and_then(toml::Value::as_str)
                .map(|v| (tool, v)),
            _ => None,
        })
        .enumerate()
        .map(|(entry, (tool, version))| pin(tool, version, "mise.toml", entry))
        .collect()
}

/// Reads `.tool-versions`: `<tool> <version> [<fallback>...]` per line, `#` starts a comment.
fn tool_versions_pins(root: &Path) -> Result<Vec<Pin>, DetectError> {
    let Some(text) = read_text(root, ".tool-versions")? else {
        return Ok(Vec::new());
    };
    text.lines()
        .filter_map(|line| {
            let line = line.split_once('#').map_or(line, |(before, _)| before);
            let mut words = line.split_whitespace();
            Some((words.next()?, words.next()?))
        })
        .enumerate()
        .map(|(entry, (tool, version))| pin(tool, version, ".tool-versions", entry))
        .collect()
}

/// Reads `toolchain.channel` from `rust-toolchain.toml`.
fn rust_toolchain_pin(root: &Path) -> Result<Option<Pin>, DetectError> {
    let Some(table) = read_toml(root, "rust-toolchain.toml")? else {
        return Ok(None);
    };
    table
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(toml::Value::as_str)
        .map(|channel| pin("rust", channel, "rust-toolchain.toml", 0))
        .transpose()
}

/// Reads the first line that is not empty and not a `#` comment.
fn first_line_pin(root: &Path, file: &'static str, tool: &str) -> Result<Option<Pin>, DetectError> {
    let Some(text) = read_text(root, file)? else {
        return Ok(None);
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|version| pin(tool, version, file, 0))
        .transpose()
}

/// Reads the `go` directive of `go.mod`.
fn go_mod_pin(root: &Path) -> Result<Option<Pin>, DetectError> {
    let Some(text) = read_text(root, "go.mod")? else {
        return Ok(None);
    };
    text.lines()
        .find_map(
            |line| match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["go", version, ..] => Some(pin("go", version, "go.mod", 0)),
                _ => None,
            },
        )
        .transpose()
}

fn step(program: &'static str, args: &[&str]) -> Result<SetupStep, DetectError> {
    let program =
        ProgramName::parse(program).map_err(|_| DetectError::BuiltIn { name: program })?;
    Ok(SetupStep {
        program,
        args: args.iter().map(|&a| a.to_owned()).collect(),
        cwd: None,
        when: RunWhen::CachesMissing,
    })
}

fn rel(path: &'static str) -> Result<RelPath, DetectError> {
    RelPath::parse(path).map_err(|_| DetectError::BuiltIn { name: path })
}

/// Returns whether something named `file` exists under `root`, a link included; an error other than "not found" is an error.
///
/// A link counts as present because the snapshot carries it as a link, so
/// every node answers the same; what it points at is never read here. A
/// directory on the way that is a link, such as `.devcontainer -> ..`,
/// means `file` is not in the workspace, so it counts as absent.
fn exists(root: &Path, file: &'static str) -> Result<bool, DetectError> {
    let Some(full) = inside(root, file)? else {
        return Ok(false);
    };
    match fs::symlink_metadata(full) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DetectError::Read { file, source }),
    }
}

/// Returns `root/file` when every directory on the way is a real directory, or `None` when one is missing or a link.
fn inside(root: &Path, file: &'static str) -> Result<Option<PathBuf>, DetectError> {
    let mut full = root.to_path_buf();
    let mut components = file.split('/').peekable();
    while let Some(component) = components.next() {
        full.push(component);
        if components.peek().is_none() {
            break;
        }
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Ok(None),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(DetectError::Read { file, source }),
        }
    }
    Ok(Some(full))
}

/// Reads the regular file `file` under `root` as UTF-8, or `None` when nothing is there.
///
/// No component may be a link: `.nvmrc -> .env` would put a secret into
/// the spec, `.nvmrc -> ../../.nvmrc` would give two nodes two specs from
/// one workspace, and `.fabric -> ../other/.fabric` would read another
/// workspace's override file. The file is opened by path and then checked
/// on the open handle: it must be the same regular file the link check
/// saw, by device and inode, and the directories on the way must still be
/// real ones. That closes the window between the check and the open
/// without `O_NOFOLLOW`, which std does not name and which would cover the
/// last component only.
fn read_text(root: &Path, file: &'static str) -> Result<Option<String>, DetectError> {
    let read_err = |source| DetectError::Read { file, source };
    let not_regular = || DetectError::NotARegularFile { file };
    let mut parents = file.split('/').collect::<Vec<_>>();
    parents.pop();
    let mut dir = root.to_path_buf();
    for parent in &parents {
        dir.push(parent);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(not_regular()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(read_err(err)),
        }
    }
    let full = root.join(file);
    let seen = match fs::symlink_metadata(&full) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Err(not_regular()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(read_err(err)),
    };
    let handle = fs::File::open(&full).map_err(read_err)?;
    let opened = handle.metadata().map_err(read_err)?;
    if !opened.is_file() || opened.dev() != seen.dev() || opened.ino() != seen.ino() {
        return Err(not_regular());
    }
    if inside(root, file)?.is_none() {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    handle
        .take(MAX_FILE_LEN + 1)
        .read_to_end(&mut bytes)
        .map_err(read_err)?;
    if bytes.len() as u64 > MAX_FILE_LEN {
        return Err(DetectError::TooLarge { file });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DetectError::NotUtf8 { file })
}

fn read_toml(root: &Path, file: &'static str) -> Result<Option<Table>, DetectError> {
    let Some(text) = read_text(root, file)? else {
        return Ok(None);
    };
    text.parse::<Table>().map(Some).map_err(|err| {
        let (line, column) = line_and_column(&text, err.span().map_or(0, |span| span.start));
        DetectError::Toml { file, line, column }
    })
}
