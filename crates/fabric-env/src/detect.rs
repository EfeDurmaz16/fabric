//! Detection of an `EnvSpec` from the files a workspace already has (ADR 0007).
//!
//! Invariant: detection reads a fixed list of files at the workspace root
//! and nothing else. It reads regular files only: a symbolic link in that
//! list is refused, because a link can point at a file the secret policy
//! denies or at one outside the workspace, and either would break the next
//! rule. It never opens a file the secret policy denies, never reads an
//! environment variable and never runs a program, so the same files give
//! the same spec on every node, and the spec it returns has passed
//! `EnvSpec::validate`. Every pin it keeps is short, printable and not
//! shaped like a credential, because the spec travels to other nodes.
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
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use fabric_core::env::{EnvSpec, EnvSpecError, RunWhen, SetupStep, Tool, ToolSource};
use fabric_core::harness::ProgramName;
use fabric_core::path::RelPath;
use fabric_core::snapshot::CacheClass;
use toml::Table;

use crate::overrides::{EnvOverride, OverrideError, line_and_column};
use crate::secret::{self, SecretPolicy};

/// Where the override file lives, relative to the workspace root.
pub const OVERRIDE_FILE: &str = ".fabric/env.toml";

/// The largest file detection reads; a bigger one is refused rather than loaded.
const MAX_FILE_LEN: u64 = 4 << 20;

/// The most characters a version pin may have.
pub const MAX_PIN_LEN: usize = 64;

/// Why a version pin was refused; the pin itself is never quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    /// The pin has no characters.
    #[error("version pin is empty")]
    Empty,
    /// The pin is longer than `MAX_PIN_LEN` characters.
    #[error("version pin is longer than {MAX_PIN_LEN} characters")]
    TooLong,
    /// A character outside printable ASCII, space included.
    #[error("version pin has a character outside printable ASCII")]
    BadChar,
    /// The pin is shaped like a token, key or password URL.
    #[error("version pin looks like a credential; the spec travels to other nodes")]
    Credential,
}

/// Checks that `version` is a pin and not a value that must stay on this node.
pub(crate) fn check_pin(version: &str) -> Result<(), PinError> {
    if version.is_empty() {
        return Err(PinError::Empty);
    }
    if version.len() > MAX_PIN_LEN {
        return Err(PinError::TooLong);
    }
    if !version.bytes().all(|b| (b' '..=b'~').contains(&b)) {
        return Err(PinError::BadChar);
    }
    if secret::looks_like_credential(version) {
        return Err(PinError::Credential);
    }
    Ok(())
}

/// Why a workspace's environment could not be detected.
#[derive(Debug, thiserror::Error)]
pub enum DetectError {
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
    /// A version pin in a detected file was refused.
    #[error("the `{tool}` pin in `{file}` was refused")]
    Pin {
        /// The file, relative to the root.
        file: &'static str,
        /// The tool the pin is for.
        tool: String,
        /// What is wrong with it.
        #[source]
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
        "found {lockfiles:?}; set `packageManager` in package.json or `package_manager` in {OVERRIDE_FILE}"
    )]
    AmbiguousLockfiles {
        /// The lockfiles found at the root.
        lockfiles: Vec<&'static str>,
    },
    /// The chosen manager's lockfile is absent while another manager's is present.
    #[error(
        "{manager:?} was chosen by {because:?} but only {found:?} exist; add its lockfile or set `package_manager` in {OVERRIDE_FILE}"
    )]
    LockfileMissing {
        /// The manager that was chosen.
        manager: NodeManager,
        /// What chose it.
        because: ManagerSource,
        /// The lockfiles that do exist.
        found: Vec<&'static str>,
    },
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

/// A JavaScript package manager as package.json and the override file name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManagerName {
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn, either line; the workspace decides which.
    Yarn,
    /// Bun.
    Bun,
}

impl PackageManagerName {
    /// Parses `npm`, `pnpm`, `yarn` or `bun`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "yarn" => Some(Self::Yarn),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    /// Returns the name as package.json writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
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
    /// writes none, so a Bun tree is judged by being non-empty.
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

/// The package manager detection chose, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerChoice {
    /// The manager.
    pub manager: NodeManager,
    /// What decided it.
    pub because: ManagerSource,
}

/// The ecosystem a cache path belongs to; it decides which carry key the store measures (ADR 0012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    /// A JavaScript dependency tree installed by this manager.
    Node(NodeManager),
    /// Cargo build output.
    Rust,
    /// A Python virtual environment.
    Python,
    /// A path the user declared; no ecosystem rule applies.
    Declared,
}

impl Ecosystem {
    /// Returns the files one of which a finished install leaves in the cache; empty when content alone proves it.
    ///
    /// The markers follow the manager detection chose, not the lockfiles on
    /// disk: a Bun tree next to a leftover `package-lock.json` needs no
    /// `.package-lock.json`.
    pub fn install_markers(self) -> &'static [&'static str] {
        match self {
            Ecosystem::Node(manager) => manager.install_markers(),
            Ecosystem::Python => &["pyvenv.cfg"],
            Ecosystem::Rust | Ecosystem::Declared => &[],
        }
    }
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
}

/// A version pin found in one file, kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The tool, with `nodejs` read as `node` and `golang` as `go`, from every file including the override one.
    pub tool: String,
    /// The version text as the file wrote it.
    pub version: String,
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
    spec: EnvSpec,
    caches: Vec<CachePath>,
    pins: Vec<Pin>,
    node_manager: Option<ManagerChoice>,
    devcontainer: Option<RelPath>,
    secrets: SecretPolicy,
}

impl Detection {
    /// Returns the spec: one pin per tool, the setup steps, the variable names and the cache paths.
    pub fn spec(&self) -> &EnvSpec {
        &self.spec
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
pub fn detect(root: &Path) -> Result<Detection, DetectError> {
    let overrides = match read_text(root, OVERRIDE_FILE)? {
        Some(text) => EnvOverride::parse(&text)?,
        None => EnvOverride::default(),
    };
    let package = read_package_json(root)?;
    let pins = collect_pins(root, &overrides, package.as_ref())?;

    let mut setup = Vec::new();
    let mut caches = Vec::new();
    let node_manager = match &package {
        Some(package) => choose_node_manager(root, package, overrides.package_manager)?,
        None => None,
    };
    if let Some(choice) = node_manager {
        setup.push(choice.manager.install_step()?);
        caches.push(CachePath {
            path: rel("node_modules")?,
            class: CacheClass::Deps,
            ecosystem: Ecosystem::Node(choice.manager),
        });
    }
    detect_rust(root, &mut setup, &mut caches)?;
    detect_python(root, &mut setup, &mut caches)?;
    if exists(root, "go.mod")? {
        setup.push(step("go", &["mod", "download"])?);
    }

    for declared in &overrides.caches {
        match caches.iter_mut().find(|c| c.path == declared.path) {
            // The ecosystem stays: it decides the carry key, and a declared
            // class must not let a `node_modules` travel to a node with
            // another Node major.
            Some(found) => found.class = declared.class,
            None => caches.push(CachePath {
                path: declared.path.clone(),
                class: declared.class,
                ecosystem: Ecosystem::Declared,
            }),
        }
    }
    caches.sort_by(|a, b| a.path.cmp(&b.path));

    if let Some(declared) = &overrides.setup {
        setup.clone_from(declared);
    }

    let mut secrets = SecretPolicy::default();
    for pattern in &overrides.secret_patterns {
        secrets.widen(pattern.clone());
    }

    let spec = EnvSpec {
        tools: chosen_tools(&pins),
        setup,
        env_var_names: overrides.env_var_names.clone(),
        cache_paths: caches.iter().map(|c| c.path.clone()).collect(),
    };
    spec.validate()?;

    Ok(Detection {
        spec,
        caches,
        pins,
        node_manager,
        devcontainer: find_devcontainer(root)?,
        secrets,
    })
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
        .map(|(tool, version)| Pin {
            tool: canonical_tool(tool),
            version: version.clone(),
            file: OVERRIDE_FILE,
            source: ToolSource::Declared,
        })
        .collect();
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
        if let Some((name, version)) = &package.manager {
            pins.push(pin(name.as_str(), version, "package.json")?);
        }
        if let Some(engine) = &package.engines_node {
            pins.push(pin("node", engine, "package.json")?);
        }
    }
    Ok(pins)
}

/// Builds a detected pin after checking its version.
fn pin(tool: &str, version: &str, file: &'static str) -> Result<Pin, DetectError> {
    check_pin(version).map_err(|reason| DetectError::Pin {
        file,
        tool: tool.to_owned(),
        reason,
    })?;
    Ok(Pin {
        tool: tool.to_owned(),
        version: version.to_owned(),
        file,
        source: ToolSource::Detected,
    })
}

fn detect_rust(
    root: &Path,
    setup: &mut Vec<SetupStep>,
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
    setup.push(step("cargo", args)?);
    caches.push(CachePath {
        path: rel("target")?,
        class: CacheClass::Build,
        ecosystem: Ecosystem::Rust,
    });
    Ok(())
}

fn detect_python(
    root: &Path,
    setup: &mut Vec<SetupStep>,
    caches: &mut Vec<CachePath>,
) -> Result<(), DetectError> {
    if exists(root, "uv.lock")? {
        setup.push(step("uv", &["sync", "--frozen"])?);
    } else if exists(root, "requirements.txt")? {
        // A bare `pip install` would write into whichever Python is first
        // on PATH; uv puts the packages in the workspace's own `.venv`.
        setup.push(step("uv", &["venv", "--allow-existing"])?);
        setup.push(step("uv", &["pip", "install", "-r", "requirements.txt"])?);
    } else {
        return Ok(());
    }
    caches.push(CachePath {
        path: rel(".venv")?,
        class: CacheClass::Deps,
        ecosystem: Ecosystem::Python,
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
    let mut chosen: BTreeMap<&str, &Pin> = BTreeMap::new();
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
        .filter_map(|(tool, value)| {
            let version = match value {
                toml::Value::String(v) => Some(v.as_str()),
                toml::Value::Array(items) => items.first().and_then(toml::Value::as_str),
                toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str),
                _ => None,
            }?;
            Some(pin(&canonical_tool(tool), version, "mise.toml"))
        })
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
            let tool = words.next()?;
            let version = words.next()?;
            Some(pin(&canonical_tool(tool), version, ".tool-versions"))
        })
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
        .map(|channel| pin("rust", channel, "rust-toolchain.toml"))
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
        .map(|version| pin(tool, version, file))
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
                ["go", version, ..] => Some(pin("go", version, "go.mod")),
                _ => None,
            },
        )
        .transpose()
}

fn canonical_tool(name: &str) -> String {
    match name {
        "nodejs" => "node".to_owned(),
        "golang" => "go".to_owned(),
        other => other.to_owned(),
    }
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
/// every node answers the same; what it points at is never read here.
fn exists(root: &Path, file: &'static str) -> Result<bool, DetectError> {
    match fs::symlink_metadata(root.join(file)) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DetectError::Read { file, source }),
    }
}

/// Reads the regular file `file` under `root` as UTF-8, or `None` when nothing is there.
///
/// A link is refused rather than followed: `.nvmrc -> .env` would put a
/// secret into the spec, and `.nvmrc -> ../../.nvmrc` would give two
/// nodes two specs from one workspace.
fn read_text(root: &Path, file: &'static str) -> Result<Option<String>, DetectError> {
    let read_err = |source| DetectError::Read { file, source };
    let full = root.join(file);
    match fs::symlink_metadata(&full) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(DetectError::NotARegularFile { file }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(read_err(err)),
    }
    let handle = fs::File::open(&full).map_err(read_err)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One sample per refusal, and the pin shapes real files hold pass.
    #[test]
    fn a_pin_is_short_printable_and_not_a_credential() {
        for ok in [
            "22",
            "v20",
            ">=18 <21",
            "^18 || ^20",
            "1.97.1",
            "lts/*",
            "3.12",
        ] {
            assert_eq!(check_pin(ok), Ok(()), "{ok}");
        }
        assert_eq!(check_pin(""), Err(PinError::Empty));
        assert_eq!(
            check_pin(&"1".repeat(MAX_PIN_LEN + 1)),
            Err(PinError::TooLong)
        );
        assert_eq!(check_pin("22\u{7}"), Err(PinError::BadChar));
        assert_eq!(check_pin("2\u{e9}"), Err(PinError::BadChar));
        assert_eq!(
            check_pin("https://u:p@registry.example.com/x"),
            Err(PinError::Credential)
        );
    }
}
