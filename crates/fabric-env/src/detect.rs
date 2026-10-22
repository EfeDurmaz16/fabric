//! Detection of an `EnvSpec` from the files a workspace already has (ADR 0007).
//!
//! Invariant: detection reads a fixed list of files at the workspace root
//! and nothing else. It reads regular files only: a symbolic link in that
//! list is refused, because a link can point at a file the secret policy
//! denies or at one outside the workspace, and either would break the next
//! rule. A file it only checks for and hashes, such as a lockfile, may be
//! a link to a regular file inside the workspace that the policy does not
//! deny, since that resolves the same on every node; any other link there
//! is refused, since the step that file selects reads it. It never opens a file
//! the job's secret policy denies, and counts a denied file it only checks
//! for as absent, since capture leaves it behind and the next node will
//! not have it. It never reads an environment variable and never runs a
//! program, so the same files give the same spec on every node, and the
//! spec it returns has passed `EnvSpec::validate`. Every tool name and pin
//! it keeps is short,
//! printable and not shaped like a credential, because the spec travels to
//! other nodes; `ToolName` and `ToolVersion` hold that rule by type. A pin
//! a detected file gives that breaks the rule is dropped with a warning
//! that names the file and the entry, never the text: pins are
//! informational, so one that nothing reads must not stop a resume. A pin
//! the override file declares is refused instead, since the user wrote it
//! on purpose.
//!
//! What it reads, and what each file gives:
//!
//! | File | Gives |
//! |---|---|
//! | `package.json` with a lockfile | the package manager's install step, `node_modules` as a cache |
//! | `.yarnrc.yml`, for Yarn 2 and later | `nodeLinker`, which must be `node-modules` |
//! | `Cargo.toml` | `cargo fetch`, with `--locked` when `Cargo.lock` exists; `target` as a cache |
//! | `uv.lock`, else `requirements.txt` | `uv sync --frozen`, or a `uv` venv and `uv pip install --python .venv -r`; `.venv` as a cache |
//! | `go.mod` | `go mod download`; no cache inside the workspace |
//! | `mise.toml`, `.tool-versions`, `rust-toolchain.toml`, `.node-version`, `.nvmrc`, `.python-version`, `go.mod`, `package.json` | tool pins, in that order of precedence |
//! | `.devcontainer/devcontainer.json`, `.devcontainer.json` | recorded, not interpreted |
//! | `.fabric/env.toml` | the user's overrides, which win over everything above |
//!
//! It also hashes, without keeping them, the files each ecosystem's steps
//! read: `package.json` with the manager's lockfiles and configuration,
//! `Cargo.toml` and `Cargo.lock`, `pyproject.toml` and `uv.lock` or
//! `requirements.txt`, `go.mod` and `go.sum`. The report keeps one digest
//! per cache, which materialize writes into the cache's sentinel, so a
//! lockfile changed since the install makes the cache stale.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fabric_core::env::{
    EnvSpec, EnvSpecError, OVERRIDE_FILE, RunWhen, SetupArg, SetupStep, Tool, ToolName, ToolSource,
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
use fabric_core::secret::{Scope, SecretPattern, SecretPolicy};

/// Every file detection opens, relative to the root. `Reader` refuses any
/// other name, so a new reader must add its file here, where a test checks
/// that no built-in secret pattern denies it. The lockfiles, manifests and
/// manager configuration a setup step reads are hashed whole, and most of
/// them are never parsed: their digest goes into the sentinel of the cache
/// the step fills (`Inputs`).
const READ_FILES: &[&str] = &[
    OVERRIDE_FILE,
    "package.json",
    "mise.toml",
    ".tool-versions",
    "rust-toolchain.toml",
    ".node-version",
    ".nvmrc",
    ".python-version",
    "go.mod",
    ".yarnrc.yml",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    ".pnpmfile.cjs",
    "yarn.lock",
    ".yarnrc",
    "bun.lock",
    "bun.lockb",
    "bunfig.toml",
    "Cargo.toml",
    "Cargo.lock",
    "pyproject.toml",
    "uv.lock",
    "requirements.txt",
    "go.sum",
];

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
        "{manager} was chosen by {because}, but the lockfiles found are: {}; add its lockfile or set `package_manager` in {OVERRIDE_FILE}",
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
        "`cache[{index}].path` nests with detected cache `{detected}`; a cache may not contain another"
    )]
    NestedCache {
        /// Zero-based index of the entry in `[[cache]]`.
        index: usize,
        /// The detected path it nests with, a name detection itself chose.
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
    /// Yarn 2 or later installs with Plug'n'Play here, which writes no `node_modules`.
    ///
    /// The `.yarnrc.yml` value is never quoted. A Plug'n'Play install leaves
    /// no `node_modules` for the detected cache to find, and its `.yarn/unplugged`
    /// holds native builds for one Node ABI with no carry key, so it is
    /// refused rather than reinstalled on every resume.
    #[error(
        "Yarn 2 or later installs with Plug'n'Play here, which writes no node_modules for Fabric to check or carry; set `nodeLinker: node-modules` in .yarnrc.yml"
    )]
    YarnPlugAndPlay,
    /// `package_manager` is set in `.fabric/env.toml` but the root has no `package.json` for it to apply to.
    #[error("`package_manager` is set in {OVERRIDE_FILE} but there is no package.json at the root")]
    PackageManagerWithoutPackageJson,
    /// `.fabric/env.toml` was refused; the source says where and why.
    #[error("{OVERRIDE_FILE} was refused")]
    Override(#[from] OverrideError),
    /// The spec built from the workspace broke one of its own rules.
    #[error("detected environment spec is invalid")]
    Spec(#[from] EnvSpecError),
    /// A program or path name built into this module failed its own type's rules.
    ///
    /// Nothing in the workspace causes it, so a caller can treat it as a
    /// bug report against this crate. Every built-in name is parsed by the
    /// detection tests, which cover each rule that uses one.
    #[error("built-in name `{name}` is not valid; this is a bug in fabric-env")]
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
            // `npm ci` installs from either; npm reads the shrinkwrap when both exist.
            NodeManager::Npm => &["npm-shrinkwrap.json", "package-lock.json"],
            NodeManager::Pnpm => &["pnpm-lock.yaml"],
            NodeManager::YarnClassic | NodeManager::YarnBerry => &["yarn.lock"],
            NodeManager::Bun => &["bun.lock", "bun.lockb"],
        }
    }

    /// Returns the files at the root its install reads besides `package.json`: its lockfiles and its own configuration, which can name code the install runs.
    ///
    /// `.npmrc` is not among them: the secret policy denies it, so it never
    /// travels and is never read.
    fn inputs(self) -> &'static [&'static str] {
        match self {
            NodeManager::Npm => &["npm-shrinkwrap.json", "package-lock.json"],
            NodeManager::Pnpm => &["pnpm-lock.yaml", "pnpm-workspace.yaml", ".pnpmfile.cjs"],
            NodeManager::YarnClassic => &["yarn.lock", ".yarnrc"],
            NodeManager::YarnBerry => &["yarn.lock", ".yarnrc.yml"],
            NodeManager::Bun => &["bun.lock", "bun.lockb", "bunfig.toml"],
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
        match self {
            ManagerSource::OverrideFile => write!(f, "`package_manager` in {OVERRIDE_FILE}"),
            ManagerSource::PackageManagerField => f.write_str("`packageManager` in package.json"),
            ManagerSource::Lockfile => f.write_str("the only lockfile"),
        }
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

/// How materialize tells a finished cache from a torn one, and which setup steps fill the cache.
///
/// Detection decides this once per cache, so materialize never works out
/// again which step fills which cache. No installer's own file is proof:
/// npm writes `node_modules/.package-lock.json`, pnpm `.modules.yaml` and
/// Yarn 1 `.yarn-integrity` before the root package's lifecycle scripts
/// run, so a failed or interrupted `postinstall` leaves the marker over a
/// tree that is not whole. Only a step that exited 0 proves its cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// The sentinel materialize writes once the steps that fill the cache have exited 0.
    Sentinel {
        /// The setup steps that fill the cache, by index in the spec.
        filled_by: Vec<usize>,
    },
    /// Content alone: no setup step fills the cache, so no step can leave it torn.
    Content,
}

impl Completion {
    /// Returns the setup steps that fill the cache, by index in the spec; empty for `Content`.
    pub fn filled_by(&self) -> &[usize] {
        match self {
            Completion::Sentinel { filled_by } => filled_by,
            Completion::Content => &[],
        }
    }
}

/// One cache path, its class for the snapshot policy, its ecosystem, and how materialize proves it whole.
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
    /// How materialize proves it whole, and which steps fill it.
    pub completion: Completion,
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

/// Which caches decide whether a `CachesMissing` step runs.
///
/// This lives in the detection report, not in the spec: the report is
/// rebuilt from the same files on every node, and linking a step to its
/// cache in the spec would change the spec format for something every
/// node can derive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepGate {
    /// Runs when one of these caches is not whole.
    Caches(Vec<RelPath>),
    /// Runs when any cache is not whole, or when the report has none.
    AnyCache,
    /// Runs on every resume: the step fills a per-node cache outside the
    /// workspace, such as Go's module cache, which is never carried and
    /// which no cache in the workspace proves warm.
    EveryResume,
}

/// What a setup step fills, as detection knows it before the caches are final.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StepScope {
    /// Fills this cache in the workspace.
    Fills(RelPath),
    /// Fills a per-node cache outside the workspace that a build turns into this cache.
    Warms(RelPath),
    /// Fills a per-node cache outside the workspace and nothing in it.
    Outside,
    /// Declared by the user, who did not say what it fills.
    Declared,
}

impl StepScope {
    /// Returns true when the step fills `cache`.
    ///
    /// A declared step stands in for the detected steps and does not say
    /// what it fills, so it counts as filling every cache but a detected
    /// `target`, which a build makes and setup never does.
    fn fills(&self, cache: &FoundCache) -> bool {
        match self {
            StepScope::Fills(path) => path == &cache.path,
            StepScope::Declared => {
                !(cache.origin == CacheOrigin::Detected && cache.ecosystem == Ecosystem::Rust)
            }
            StepScope::Warms(_) | StepScope::Outside => false,
        }
    }

    /// Returns the caches whose health decides whether the step at `index` runs.
    ///
    /// The caches the step fills, so a declared `npm ci` never runs because
    /// a `target` is missing; for `cargo fetch`, the `target` a build makes
    /// from what it fetches. A step that fills only a cache outside the
    /// workspace, such as `go mod download`, runs on every resume: a healthy
    /// `node_modules` beside it says nothing about this node's module cache.
    /// A declared step that fills no cache waits on every cache.
    fn gate(&self, index: usize, caches: &[CachePath]) -> StepGate {
        match self {
            StepScope::Warms(path) => return StepGate::Caches(vec![path.clone()]),
            StepScope::Outside => return StepGate::EveryResume,
            StepScope::Fills(_) | StepScope::Declared => {}
        }
        let filled: Vec<RelPath> = caches
            .iter()
            .filter(|cache| cache.completion.filled_by().contains(&index))
            .map(|cache| cache.path.clone())
            .collect();
        if filled.is_empty() {
            StepGate::AnyCache
        } else {
            StepGate::Caches(filled)
        }
    }
}

/// Variables that move where uv works: its directory, the project it reads and the environment it writes.
///
/// A detected `uv` step runs without them. Inherited from the shell that
/// runs resume, `UV_PROJECT_ENVIRONMENT` would make `uv sync` fill another
/// environment than `.venv`, `UV_PROJECT` would sync another project and
/// `UV_WORKING_DIR` would resolve `.venv` and `requirements.txt` somewhere
/// else: the step would succeed, change something outside the workspace
/// and leave the cache missing for every later resume. A declared step is
/// the user's and keeps the environment it inherits.
const UV_REDIRECTS: &[&str] = &["UV_PROJECT_ENVIRONMENT", "UV_PROJECT", "UV_WORKING_DIR"];

/// A setup step as detection builds it: the step, what it fills, and the variables it runs without.
struct FoundStep {
    step: SetupStep,
    scope: StepScope,
    cleared: &'static [&'static str],
}

impl FoundStep {
    /// A step that runs with every variable it inherits.
    fn new(step: SetupStep, scope: StepScope) -> Self {
        Self {
            step,
            scope,
            cleared: &[],
        }
    }
}

/// A cache path before the setup steps are final: what `CachePath` holds but its completion.
#[derive(Debug, Clone)]
struct FoundCache {
    path: RelPath,
    class: CacheClass,
    ecosystem: Ecosystem,
    origin: CacheOrigin,
}

impl FoundCache {
    /// Gives the cache its completion, now that the steps that fill it are known.
    fn finish(self, scopes: &[StepScope]) -> CachePath {
        let filled_by: Vec<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, scope)| scope.fills(&self))
            .map(|(index, _)| index)
            .collect();
        let completion = if filled_by.is_empty() {
            Completion::Content
        } else {
            Completion::Sentinel { filled_by }
        };
        CachePath {
            path: self.path,
            class: self.class,
            ecosystem: self.ecosystem,
            origin: self.origin,
            completion,
        }
    }
}

/// The files at the root that each ecosystem's steps read, as the rules that fired found them.
///
/// The steps run code these files name: a lockfile picks the packages
/// whose install scripts run, and `package.json` holds the root package's
/// own. A cache's sentinel records the digest of its ecosystem's files, so
/// a changed lockfile makes its cache stale even though the spec, which
/// holds the commands and not the files, has not changed.
#[derive(Debug, Default)]
struct Inputs {
    node: Vec<&'static str>,
    rust: &'static [&'static str],
    python: &'static [&'static str],
    go: &'static [&'static str],
}

impl Inputs {
    /// Returns every file of every ecosystem, sorted, each once.
    fn all(&self) -> Vec<&'static str> {
        let mut files: Vec<&'static str> = self
            .node
            .iter()
            .chain(self.rust)
            .chain(self.python)
            .chain(self.go)
            .copied()
            .collect();
        files.sort_unstable();
        files.dedup();
        files
    }

    /// Returns the files whose digest proves a cache of `ecosystem` current, sorted.
    ///
    /// Nothing says what an `other` cache is made from, so every input counts for it.
    fn of(&self, ecosystem: Ecosystem) -> Vec<&'static str> {
        let mut files = match ecosystem {
            Ecosystem::Node(_) => self.node.clone(),
            Ecosystem::Rust => self.rust.to_vec(),
            Ecosystem::Python => self.python.to_vec(),
            Ecosystem::Other => return self.all(),
        };
        files.sort_unstable();
        files.dedup();
        files
    }

    /// Hashes every input file once and returns, per cache, the digest of the files of its ecosystem.
    ///
    /// Each present file adds its name, a zero byte and the blake3 of its
    /// bytes, in name order; an absent or denied file adds nothing, so
    /// adding or removing a lockfile changes the digest too.
    fn digests(
        &self,
        reader: &Reader<'_>,
        caches: &[FoundCache],
    ) -> Result<Vec<blake3::Hash>, DetectError> {
        let mut files = BTreeMap::new();
        for file in self.all() {
            if let Some(hash) = reader.digest(file)? {
                files.insert(file, hash);
            }
        }
        Ok(caches
            .iter()
            .map(|cache| {
                let mut hasher = blake3::Hasher::new();
                for file in self.of(cache.ecosystem) {
                    if let Some(hash) = files.get(file) {
                        hasher.update(file.as_bytes());
                        hasher.update(&[0]);
                        hasher.update(hash.as_bytes());
                    }
                }
                hasher.finalize()
            })
            .collect())
    }
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
    /// One gate per step of `spec.setup`, in the same order; built with it in `detect` and never changed.
    step_gates: Vec<StepGate>,
    /// Per step of `spec.setup`, in the same order: the variables it runs without (`UV_REDIRECTS`).
    step_cleared: Vec<&'static [&'static str]>,
    caches: Vec<CachePath>,
    /// One digest per cache, in the same order: the files the steps of its ecosystem read (`Inputs`).
    inputs: Vec<blake3::Hash>,
    pins: Vec<Pin>,
    node_manager: Option<ManagerChoice>,
    devcontainer: Option<RelPath>,
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

    /// Returns each setup step of the spec, in order, with the caches that decide whether it runs.
    pub fn steps(&self) -> impl Iterator<Item = (&SetupStep, &StepGate)> {
        self.spec.setup.iter().zip(&self.step_gates)
    }

    /// Returns the variables of the node's environment that the step at `index` runs without; empty for a step the user declared.
    pub(crate) fn cleared(&self, index: usize) -> &'static [&'static str] {
        self.step_cleared.get(index).copied().unwrap_or_default()
    }

    /// Returns the spec's cache paths with their class, ecosystem and completion, sorted by path.
    pub fn caches(&self) -> &[CachePath] {
        &self.caches
    }

    /// Returns each cache with the digest of the files its setup steps read, which its sentinel records.
    pub(crate) fn caches_with_inputs(&self) -> impl Iterator<Item = (&CachePath, &blake3::Hash)> {
        self.caches.iter().zip(&self.inputs)
    }

    /// Returns one digest of every cache's path and the digest of the files
    /// its setup steps read, in path order: what an approval of the spec's
    /// steps covers besides the spec's id (ADR 0007, step 4).
    ///
    /// A lockfile, `package.json` or a manager's own configuration can name
    /// code an install runs, and can change while the spec, which holds the
    /// commands, keeps its id.
    #[must_use]
    pub fn inputs_digest(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        for (cache, inputs) in self.caches_with_inputs() {
            hasher.update(cache.path.as_str().as_bytes());
            hasher.update(&[0]);
            hasher.update(inputs.as_bytes());
        }
        hasher.finalize()
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
}

/// Detects the environment of the workspace at `root`, applying `.fabric/env.toml` when present.
///
/// `recorded` is the `secret_patterns` of the job's last spec, empty for a
/// job that has none yet. Every one stays in force and in the new spec,
/// even when the override file no longer lists it: a job's secret list
/// only grows, and the override file is a file in the working tree that
/// the agent can rewrite. The capture policy is built from the returned
/// spec with `SecretPolicy::from_patterns` (ADR 0004).
pub fn detect(root: &Path, recorded: &[SecretPattern]) -> Result<Detection, DetectError> {
    let root = fs::canonicalize(root).map_err(|source| DetectError::Root { source })?;
    let root = root.as_path();
    // No pattern can deny the override file: `SecretPattern` refuses one
    // that would (`DeniesOverrideFile`), and a test checks the built-ins.
    let overrides = match read_text(root, OVERRIDE_FILE)? {
        Some(text) => EnvOverride::parse(&text)?,
        None => EnvOverride::default(),
    };
    let secret_patterns = job_secret_patterns(recorded, &overrides.secret_patterns);
    let secrets = SecretPolicy::from_patterns(&secret_patterns);
    let reader = Reader {
        root,
        secrets: &secrets,
    };
    let package = read_package_json(&reader)?;
    let pins = collect_pins(&reader, &overrides, package.as_ref())?;

    let mut setup = Vec::new();
    let mut caches = Vec::new();
    let mut inputs = Inputs::default();
    let node_manager = match (&package, overrides.package_manager) {
        (Some(package), declared) => choose_node_manager(&reader, package, declared)?,
        (None, Some(_)) => return Err(DetectError::PackageManagerWithoutPackageJson),
        (None, None) => None,
    };
    if let Some(choice) = node_manager {
        inputs.node = std::iter::once("package.json")
            .chain(choice.manager.inputs().iter().copied())
            .collect();
        let node_modules = rel("node_modules")?;
        setup.push(FoundStep::new(
            choice.manager.install_step()?,
            StepScope::Fills(node_modules.clone()),
        ));
        caches.push(FoundCache {
            path: node_modules,
            class: CacheClass::Deps,
            ecosystem: Ecosystem::Node(choice.manager),
            origin: CacheOrigin::Detected,
        });
    }
    inputs.rust = detect_rust(&reader, &mut setup, &mut caches)?;
    inputs.python = detect_python(&reader, &mut setup, &mut caches)?;
    if reader.exists("go.mod")? {
        inputs.go = &["go.mod", "go.sum"];
        setup.push(FoundStep::new(
            step("go", &["mod", "download"])?,
            StepScope::Outside,
        ));
    }

    merge_declared_caches(&mut caches, &overrides.caches, node_manager)?;
    caches.sort_by(|a, b| a.path.cmp(&b.path));
    let cache_inputs = inputs.digests(&reader, &caches)?;

    if let Some(declared) = &overrides.setup {
        setup = declared
            .iter()
            .map(|step| FoundStep::new(step.clone(), StepScope::Declared))
            .collect();
    }
    let step_cleared = setup.iter().map(|found| found.cleared).collect();
    let (setup, step_scopes): (Vec<SetupStep>, Vec<StepScope>) = setup
        .into_iter()
        .map(|found| (found.step, found.scope))
        .unzip();
    let caches: Vec<CachePath> = caches
        .into_iter()
        .map(|cache| cache.finish(&step_scopes))
        .collect();
    let step_gates = step_scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| scope.gate(index, &caches))
        .collect();

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
        step_gates,
        step_cleared,
        caches,
        inputs: cache_inputs,
        pins,
        node_manager,
        devcontainer: find_devcontainer(&reader)?,
    })
}

/// Adds the declared caches to the detected ones, giving each the ecosystem that makes it.
fn merge_declared_caches(
    caches: &mut Vec<FoundCache>,
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
                index,
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
        caches.push(FoundCache {
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
    recorded: &[SecretPattern],
    declared: &[SecretPattern],
) -> Vec<SecretPattern> {
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
    reader: &Reader<'_>,
    overrides: &EnvOverride,
    package: Option<&PackageJson>,
) -> Result<Vec<Pin>, DetectError> {
    let mut pins: Vec<Pin> = overrides
        .tools
        .iter()
        .map(|(tool, version)| Pin {
            tool: tool.clone(),
            version: version.clone(),
            file: OVERRIDE_FILE,
            source: ToolSource::Declared,
        })
        .collect();
    pins.extend(mise_pins(reader)?);
    pins.extend(tool_versions_pins(reader)?);
    pins.extend(rust_toolchain_pin(reader)?);
    for (file, tool) in [
        (".node-version", "node"),
        (".nvmrc", "node"),
        (".python-version", "python"),
    ] {
        pins.extend(first_line_pin(reader, file, tool)?);
    }
    pins.extend(go_mod_pin(reader)?);
    if let Some(package) = package {
        if let Some((name, version)) = &package.manager {
            pins.extend(pin(
                name.as_str(),
                version,
                "package.json",
                PinAt::Key("packageManager"),
            ));
        }
        if let Some(engine) = &package.engines_node {
            pins.extend(pin(
                "node",
                engine,
                "package.json",
                PinAt::Key("engines.node"),
            ));
        }
    }
    Ok(pins)
}

/// Where in its file a detected pin sits, so a warning can point at it without quoting it.
#[derive(Debug, Clone, Copy)]
enum PinAt {
    /// A fixed key, such as `engines.node`.
    Key(&'static str),
    /// The zero-based index of the entry under `[tools]`, counting every entry.
    ToolsEntry(usize),
    /// The one-based line number.
    Line(usize),
}

impl fmt::Display for PinAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PinAt::Key(key) => write!(f, "`{key}`"),
            PinAt::ToolsEntry(index) => write!(f, "[tools] entry {index}"),
            PinAt::Line(line) => write!(f, "line {line}"),
        }
    }
}

/// Builds a detected pin, reading its tool name through its alias, or drops it when the name or the version breaks the pin rules.
///
/// The warning names the file, where in it the pin sits and the rule,
/// never the text, so a token in a pin file does not reach the log.
/// Dropping is as leak-safe as refusing: a dropped pin never enters the
/// spec.
fn pin(tool: &str, version: &str, file: &'static str, at: PinAt) -> Option<Pin> {
    let parsed = ToolName::parse(canonical_tool(tool))
        .map_err(|reason| ("tool name", reason))
        .and_then(|tool| {
            ToolVersion::parse(version)
                .map(|version| (tool, version))
                .map_err(|reason| ("version", reason))
        });
    match parsed {
        Ok((tool, version)) => Some(Pin {
            tool,
            version,
            file,
            source: ToolSource::Detected,
        }),
        Err((part, reason)) => {
            tracing::warn!(
                file,
                %at,
                part,
                %reason,
                "pin dropped; pins are informational, so detection goes on without it"
            );
            None
        }
    }
}

/// Adds Cargo's step and cache, and returns the files they read.
fn detect_rust(
    reader: &Reader<'_>,
    setup: &mut Vec<FoundStep>,
    caches: &mut Vec<FoundCache>,
) -> Result<&'static [&'static str], DetectError> {
    if !reader.exists("Cargo.toml")? {
        return Ok(&[]);
    }
    // `--locked` keeps a resume from rewriting Cargo.lock, as the frozen
    // installs do for JavaScript; without a lockfile there is nothing to keep.
    let args: &[&str] = if reader.exists("Cargo.lock")? {
        &["fetch", "--locked"]
    } else {
        &["fetch"]
    };
    let target = rel("target")?;
    // `cargo fetch` fills ~/.cargo, not `target`, but `target` is what a
    // build makes from it: while `target` is whole the fetch has nothing
    // to add, and a resume on a warm workspace runs no command.
    setup.push(FoundStep::new(
        step("cargo", args)?,
        StepScope::Warms(target.clone()),
    ));
    caches.push(FoundCache {
        path: target,
        class: CacheClass::Build,
        ecosystem: Ecosystem::Rust,
        origin: CacheOrigin::Detected,
    });
    Ok(&["Cargo.toml", "Cargo.lock"])
}

/// Adds uv's steps and `.venv`, and returns the files the steps read.
fn detect_python(
    reader: &Reader<'_>,
    setup: &mut Vec<FoundStep>,
    caches: &mut Vec<FoundCache>,
) -> Result<&'static [&'static str], DetectError> {
    let venv = rel(".venv")?;
    let uv = |args| -> Result<FoundStep, DetectError> {
        Ok(FoundStep {
            step: step("uv", args)?,
            scope: StepScope::Fills(venv.clone()),
            cleared: UV_REDIRECTS,
        })
    };
    let inputs: &'static [&'static str] = if reader.exists("uv.lock")? {
        setup.push(uv(&["sync", "--frozen"])?);
        &["pyproject.toml", "uv.lock"]
    } else if reader.exists("requirements.txt")? {
        // A bare `pip install` would write into whichever Python is first
        // on PATH. Both steps name `.venv`: without `--python`, `uv pip
        // install` prefers an activated `VIRTUAL_ENV` or `CONDA_PREFIX`
        // inherited from the shell that runs resume over the workspace's
        // own `.venv`, and would fill that one instead.
        setup.push(uv(&["venv", "--allow-existing", ".venv"])?);
        setup.push(uv(&[
            "pip",
            "install",
            "--python",
            ".venv",
            "-r",
            "requirements.txt",
        ])?);
        &["requirements.txt"]
    } else {
        return Ok(&[]);
    };
    caches.push(FoundCache {
        path: venv,
        class: CacheClass::Deps,
        ecosystem: Ecosystem::Python,
        origin: CacheOrigin::Detected,
    });
    Ok(inputs)
}

fn find_devcontainer(reader: &Reader<'_>) -> Result<Option<RelPath>, DetectError> {
    for file in [".devcontainer/devcontainer.json", ".devcontainer.json"] {
        if reader.exists(file)? {
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

fn read_package_json(reader: &Reader<'_>) -> Result<Option<PackageJson>, DetectError> {
    let Some(text) = reader.text("package.json")? else {
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
    reader: &Reader<'_>,
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
            if !lockfiles.contains(&file) && reader.exists(file)? {
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
        None => reader.exists(".yarnrc.yml")?,
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
    if manager == NodeManager::YarnBerry
        && yarn_node_linker(reader)?.as_deref() != Some("node-modules")
    {
        return Err(DetectError::YarnPlugAndPlay);
    }
    Ok(Some(ManagerChoice { manager, because }))
}

/// Returns the top-level `nodeLinker` of `.yarnrc.yml` without quotes, or `None` when the file or the key is absent.
///
/// Only the one key is read, line by line: `nodeLinker: <value>` at the
/// start of a line, the value plain or quoted and optionally followed by a
/// comment. Absent means Yarn's default, Plug'n'Play.
fn yarn_node_linker(reader: &Reader<'_>) -> Result<Option<String>, DetectError> {
    let Some(text) = reader.text(".yarnrc.yml")? else {
        return Ok(None);
    };
    Ok(text.lines().find_map(|line| {
        let value = line.strip_prefix("nodeLinker:")?;
        let value = value.split_once(" #").map_or(value, |(before, _)| before);
        Some(value.trim().trim_matches(['"', '\'']).to_owned())
    }))
}

/// Reads `[tools]` from `mise.toml`: a string, the first string of an array, or a table's `version`.
fn mise_pins(reader: &Reader<'_>) -> Result<Vec<Pin>, DetectError> {
    let Some(table) = reader.toml("mise.toml")? else {
        return Ok(Vec::new());
    };
    let Some(tools) = table.get("tools").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    Ok(tools
        .iter()
        .enumerate()
        .filter_map(|(entry, (tool, value))| {
            let at = PinAt::ToolsEntry(entry);
            let version = match value {
                toml::Value::String(v) => Some(v.as_str()),
                toml::Value::Array(items) => items.first().and_then(toml::Value::as_str),
                toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str),
                _ => None,
            };
            let Some(version) = version else {
                tracing::warn!(
                    file = "mise.toml",
                    %at,
                    "pin dropped: detection reads a string, an array whose first item is a string, or a table with a string `version`"
                );
                return None;
            };
            pin(tool, version, "mise.toml", at)
        })
        .collect())
}

/// Reads `.tool-versions`: `<tool> <version> [<fallback>...]` per line, `#` starts a comment.
fn tool_versions_pins(reader: &Reader<'_>) -> Result<Vec<Pin>, DetectError> {
    let Some(text) = reader.text(".tool-versions")? else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.split_once('#').map_or(line, |(before, _)| before);
            let mut words = line.split_whitespace();
            let (tool, version) = (words.next()?, words.next()?);
            pin(tool, version, ".tool-versions", PinAt::Line(index + 1))
        })
        .collect())
}

/// Reads `toolchain.channel` from `rust-toolchain.toml`.
fn rust_toolchain_pin(reader: &Reader<'_>) -> Result<Option<Pin>, DetectError> {
    let Some(table) = reader.toml("rust-toolchain.toml")? else {
        return Ok(None);
    };
    Ok(table
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(toml::Value::as_str)
        .and_then(|channel| {
            pin(
                "rust",
                channel,
                "rust-toolchain.toml",
                PinAt::Key("toolchain.channel"),
            )
        }))
}

/// Reads the first line that is not empty and not a `#` comment.
fn first_line_pin(
    reader: &Reader<'_>,
    file: &'static str,
    tool: &str,
) -> Result<Option<Pin>, DetectError> {
    let Some(text) = reader.text(file)? else {
        return Ok(None);
    };
    Ok(text
        .lines()
        .map(str::trim)
        .enumerate()
        .find(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .and_then(|(index, version)| pin(tool, version, file, PinAt::Line(index + 1))))
}

/// Reads the `go` directive of `go.mod`.
fn go_mod_pin(reader: &Reader<'_>) -> Result<Option<Pin>, DetectError> {
    let Some(text) = reader.text("go.mod")? else {
        return Ok(None);
    };
    Ok(text
        .lines()
        .find_map(
            |line| match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["go", version, ..] => Some(*version),
                _ => None,
            },
        )
        .and_then(|version| pin("go", version, "go.mod", PinAt::Key("go"))))
}

fn step(program: &'static str, args: &[&'static str]) -> Result<SetupStep, DetectError> {
    let program =
        ProgramName::parse(program).map_err(|_| DetectError::BuiltIn { name: program })?;
    let args = args
        .iter()
        .map(|&arg| SetupArg::parse(arg).map_err(|_| DetectError::BuiltIn { name: arg }))
        .collect::<Result<_, _>>()?;
    Ok(SetupStep {
        program,
        args,
        cwd: None,
        when: RunWhen::CachesMissing,
    })
}

fn rel(path: &'static str) -> Result<RelPath, DetectError> {
    RelPath::parse(path).map_err(|_| DetectError::BuiltIn { name: path })
}

/// Opens the regular file the link at `full` names, for `file`, a file
/// detection checks for or hashes, when it lies inside `root` and
/// `secrets` does not deny it; any other link is refused.
///
/// Such a link travels as a link and resolves the same on every node, as
/// Django's `requirements.txt -> requirements/base.txt`. One that leaves
/// the workspace, as `package-lock.json -> ../lock.json`, names a file
/// that can differ from node to node; one to a denied file names a file
/// capture leaves behind; one that names nothing or a directory selects
/// an install that cannot run. The file is checked on the open handle to
/// be the one the resolved path named, by device and inode.
fn linked_file(
    root: &Path,
    secrets: &SecretPolicy,
    file: &'static str,
    full: &Path,
) -> Result<fs::File, DetectError> {
    let not_regular = || DetectError::NotARegularFile { file };
    let read_err = |source| DetectError::Read { file, source };
    let root = fs::canonicalize(root).map_err(read_err)?;
    let target = fs::canonicalize(full).map_err(|_| not_regular())?;
    let rel = target
        .strip_prefix(&root)
        .ok()
        .and_then(Path::to_str)
        .and_then(|text| RelPath::parse(text).ok())
        .ok_or_else(not_regular)?;
    if secrets.denies(&rel, Scope::Work).is_some() {
        return Err(not_regular());
    }
    let seen = fs::metadata(&target).map_err(read_err)?;
    let handle = fs::File::open(&target).map_err(read_err)?;
    let opened = handle.metadata().map_err(read_err)?;
    if !seen.is_file()
        || !opened.is_file()
        || opened.dev() != seen.dev()
        || opened.ino() != seen.ino()
    {
        return Err(not_regular());
    }
    Ok(handle)
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

/// Opens the regular file `file` under `root`, or returns `None` when nothing is there.
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
fn open_regular(root: &Path, file: &'static str) -> Result<Option<fs::File>, DetectError> {
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
    Ok(Some(handle))
}

/// Reads the regular file `file` under `root` as UTF-8, or `None` when nothing is there.
fn read_text(root: &Path, file: &'static str) -> Result<Option<String>, DetectError> {
    let Some(handle) = open_regular(root, file)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    handle
        .take(MAX_FILE_LEN + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DetectError::Read { file, source })?;
    if bytes.len() as u64 > MAX_FILE_LEN {
        return Err(DetectError::TooLarge { file });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DetectError::NotUtf8 { file })
}

/// Reads the files of the fixed list, skipping any the secret policy denies.
struct Reader<'a> {
    root: &'a Path,
    secrets: &'a SecretPolicy,
}

impl Reader<'_> {
    /// Returns whether `file` is present for detection: absent when the secret policy denies it, else as `open_checked` finds it.
    ///
    /// Capture leaves a denied file out of the snapshot, so on the next
    /// node it is not there. Counting it here would give the source a spec
    /// the target cannot derive, such as an `npm ci` for a lockfile that
    /// never arrives.
    fn exists(&self, file: &'static str) -> Result<bool, DetectError> {
        if self.denied(file)? {
            return Ok(false);
        }
        Ok(self.open_checked(file)?.is_some())
    }

    /// Opens `file`, which detection checks for or hashes: the regular file
    /// at its name, or the one a link there names inside the workspace
    /// (`linked_file`). `None` when nothing is there, or when a directory
    /// on the way is a link, as `.devcontainer -> ..`, since then `file` is
    /// not in the workspace. A file whose text detection reads still
    /// refuses a link (`open_regular`).
    ///
    /// A file detection only checks for still selects the step that reads
    /// it, so a directory at a lockfile's name, which selects an install
    /// that cannot run, is refused.
    fn open_checked(&self, file: &'static str) -> Result<Option<fs::File>, DetectError> {
        let Some(full) = inside(self.root, file)? else {
            return Ok(None);
        };
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_symlink() => {
                linked_file(self.root, self.secrets, file, &full).map(Some)
            }
            Ok(meta) if meta.is_file() => open_regular(self.root, file),
            Ok(_) => Err(DetectError::NotARegularFile { file }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(DetectError::Read { file, source }),
        }
    }

    /// Returns true when the secret policy denies `file`, and logs which pattern did.
    fn denied(&self, file: &'static str) -> Result<bool, DetectError> {
        let Some(pattern) = self.secrets.denies(&rel(file)?, Scope::Work) else {
            return Ok(false);
        };
        tracing::warn!(
            file,
            pattern,
            "not read: the secret policy denies this file"
        );
        Ok(true)
    }

    /// Reads `file` as text, or `None` when it is absent or the secret policy denies it.
    ///
    /// Refuses a name missing from `READ_FILES` as a bug in this module.
    fn text(&self, file: &'static str) -> Result<Option<String>, DetectError> {
        if !READ_FILES.contains(&file) {
            return Err(DetectError::BuiltIn { name: file });
        }
        if self.denied(file)? {
            return Ok(None);
        }
        read_text(self.root, file)
    }

    /// Returns the blake3 of the bytes of `file`, or `None` when it is absent or the secret policy denies it.
    ///
    /// The file goes through the hasher in pieces and none of it is kept,
    /// so a lockfile larger than `MAX_FILE_LEN` is hashed, not refused.
    /// Refuses a name missing from `READ_FILES` as a bug in this module.
    fn digest(&self, file: &'static str) -> Result<Option<blake3::Hash>, DetectError> {
        if !READ_FILES.contains(&file) {
            return Err(DetectError::BuiltIn { name: file });
        }
        if self.denied(file)? {
            return Ok(None);
        }
        let Some(handle) = self.open_checked(file)? else {
            return Ok(None);
        };
        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(handle)
            .map_err(|source| DetectError::Read { file, source })?;
        Ok(Some(hasher.finalize()))
    }

    /// Reads `file` as a TOML table, or `None` when it is absent or the secret policy denies it.
    fn toml(&self, file: &'static str) -> Result<Option<Table>, DetectError> {
        let Some(text) = self.text(file)? else {
            return Ok(None);
        };
        text.parse::<Table>().map(Some).map_err(|err| {
            let (line, column) = line_and_column(&text, err.span().map_or(0, |span| span.start));
            DetectError::Toml { file, line, column }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A buffer the log writes into, shared with the test.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut bytes = self
                .0
                .lock()
                .map_err(|_| io::Error::other("log buffer poisoned"))?;
            bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A dropped pin's warning is the user's only pointer to it, so it
    /// names the place as the file numbers it: blank and comment lines
    /// count, and so does a `[tools]` entry whose value detection cannot read.
    #[test]
    fn a_dropped_pin_is_named_by_its_place_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("mise.toml"),
            "[tools]\nnode = 22\npython = \"3.12\"\nruby = \"\"\n",
        )
        .unwrap();
        fs::write(
            root.join(".tool-versions"),
            "# pins\n\nnode 22\nruby \u{1b}[31m3\n",
        )
        .unwrap();
        let secrets = SecretPolicy::default();
        let reader = Reader {
            root,
            secrets: &secrets,
        };
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let (mise, tool_versions) = tracing::subscriber::with_default(subscriber, || {
            (
                mise_pins(&reader).unwrap(),
                tool_versions_pins(&reader).unwrap(),
            )
        });
        assert_eq!(mise.len(), 1);
        assert_eq!(tool_versions.len(), 1);
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 3, "{log}");
        assert!(lines[0].contains("[tools] entry 0"), "{log}");
        assert!(lines[1].contains("[tools] entry 2"), "{log}");
        assert!(lines[2].contains("line 4"), "{log}");
    }

    /// Detection never opens a denied file; for the built-in list that holds
    /// because no file it reads matches a built-in pattern, and a job
    /// pattern that names one makes `Reader` skip it.
    #[test]
    fn no_file_detection_reads_is_a_builtin_secret() {
        let policy = SecretPolicy::default();
        for &file in READ_FILES {
            assert_eq!(
                policy.denies(&RelPath::parse(file).unwrap(), Scope::Work),
                None,
                "{file}"
            );
        }
    }
}
