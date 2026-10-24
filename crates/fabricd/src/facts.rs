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
//!
//! Each operation measures only the tools whose facts it reads
//! ([`Probes`]), so a fact it did not ask for is absent too; each set of
//! tools is tested against the function that reads the facts. A login is
//! measured only where it gates a move or a resume target, never by a
//! suspend. An answer is reused for a minute while the tool's binary stays
//! the same ([`FactCache`]); `fabric doctor` always measures anew.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use fabric_client::api::Roots;
use fabric_core::env::{OVERRIDE_FILE, SetupStep};
use fabric_core::harness::ProgramName;
use fabric_core::path::{AbsPath, RelPath};
use fabric_core::snapshot::{CacheClass, CarryKey, LeftBehind, Platform, Remake, SnapshotManifest};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// The trimmed standard output of a command that exited 0.
    Output,
    /// `yes` when the command exited 0, `no` when it ran and failed.
    ExitCode,
}

/// One measuring command: the fact it gives, its arguments, and how its answer reads.
type Command = (&'static str, &'static [&'static str], Probe);

/// A program a node measures, with the commands it runs for its facts.
///
/// Facts that need no process, the home, the platform, this build's
/// version, `CARGO_HOME` and where `lsof` is, belong to no tool and are
/// in every measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    /// `id -un`.
    User,
    /// `git --version`, on the daemon's own `PATH`.
    Git,
    /// `sw_vers -productVersion`.
    Macos,
    /// `xcrun --show-sdk-version`.
    MacosSdk,
    /// `node --version` and the ABI of its native addons.
    Node,
    /// `npm --version`.
    Npm,
    /// `pnpm --version` and `pnpm store path`.
    Pnpm,
    /// `yarn --version`.
    Yarn,
    /// `bun --version`.
    Bun,
    /// `rustc -vV`.
    Rustc,
    /// `cargo --version`: rustup installs cargo and rustc as separate
    /// components, so a node can have one without the other.
    Cargo,
    /// `python3 --version` and its `sys.base_prefix`.
    Python,
    /// `uv --version`.
    Uv,
    /// `claude --version`.
    Claude,
    /// `claude auth status`, which only a move or resume target needs.
    ClaudeLogin,
    /// `codex --version`.
    Codex,
    /// `codex login status`, which only a move or resume target needs.
    CodexLogin,
    /// Whether iCloud Drive syncs `~/Desktop` and `~/Documents`.
    ICloud,
}

impl Tool {
    /// Every tool.
    pub const ALL: [Tool; 18] = [
        Tool::User,
        Tool::Git,
        Tool::Macos,
        Tool::MacosSdk,
        Tool::Node,
        Tool::Npm,
        Tool::Pnpm,
        Tool::Yarn,
        Tool::Bun,
        Tool::Rustc,
        Tool::Cargo,
        Tool::Python,
        Tool::Uv,
        Tool::Claude,
        Tool::ClaudeLogin,
        Tool::Codex,
        Tool::CodexLogin,
        Tool::ICloud,
    ];

    const fn bit(self) -> u32 {
        1 << self as u32
    }

    /// The program its commands run.
    fn program(self) -> &'static str {
        match self {
            Tool::User => "id",
            Tool::Git => "git",
            Tool::Macos => "sw_vers",
            Tool::MacosSdk => "xcrun",
            Tool::Node => "node",
            Tool::Npm => "npm",
            Tool::Pnpm => "pnpm",
            Tool::Yarn => "yarn",
            Tool::Bun => "bun",
            Tool::Rustc => "rustc",
            Tool::Cargo => "cargo",
            Tool::Python => "python3",
            Tool::Uv => "uv",
            Tool::Claude | Tool::ClaudeLogin => "claude",
            Tool::Codex | Tool::CodexLogin => "codex",
            Tool::ICloud => "defaults",
        }
    }

    /// The commands it runs, each giving one fact.
    fn commands(self) -> &'static [Command] {
        use Probe::{ExitCode, Output};
        match self {
            Tool::User => &[(name::USER, &["-un"], Output)],
            Tool::Git => &[(name::GIT, &["--version"], Output)],
            Tool::Macos => &[(name::MACOS, &["-productVersion"], Output)],
            Tool::MacosSdk => &[(name::MACOS_SDK, &["--show-sdk-version"], Output)],
            Tool::Node => &[
                (name::NODE, &["--version"], Output),
                (name::NODE_ABI, &["-p", "process.versions.modules"], Output),
            ],
            Tool::Npm => &[(name::NPM, &["--version"], Output)],
            Tool::Pnpm => &[
                (name::PNPM, &["--version"], Output),
                (name::PNPM_STORE, &["store", "path"], Output),
            ],
            Tool::Yarn => &[(name::YARN, &["--version"], Output)],
            Tool::Bun => &[(name::BUN, &["--version"], Output)],
            Tool::Rustc => &[(name::RUSTC, &["-vV"], Output)],
            Tool::Cargo => &[(name::CARGO, &["--version"], Output)],
            Tool::Python => &[
                (name::PYTHON, &["--version"], Output),
                (
                    name::PYTHON_HOME,
                    &["-c", "import sys; print(sys.base_prefix)"],
                    Output,
                ),
            ],
            Tool::Uv => &[(name::UV, &["--version"], Output)],
            Tool::Claude => &[(name::CLAUDE, &["--version"], Output)],
            Tool::ClaudeLogin => &[(name::CLAUDE_LOGIN, &["auth", "status"], ExitCode)],
            Tool::Codex => &[(name::CODEX, &["--version"], Output)],
            Tool::CodexLogin => &[(name::CODEX_LOGIN, &["login", "status"], ExitCode)],
            Tool::ICloud => &[
                (
                    name::ICLOUD_DESKTOP,
                    &["read", "com.apple.finder", "FXICloudDriveDesktop"],
                    Output,
                ),
                (
                    name::ICLOUD_DOCUMENTS,
                    &["read", "com.apple.finder", "FXICloudDriveDocuments"],
                    Output,
                ),
            ],
        }
    }
}

/// The tools one measure runs (issue #26): each operation asks for the
/// facts it reads, and `fabric doctor` for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probes(u32);

impl Probes {
    /// No tool: only the facts that need no process.
    pub const NONE: Probes = Probes(0);

    /// Every tool, as `fabric doctor` and a peer's facts question measure.
    pub const ALL: Probes = Probes::of(&Tool::ALL);

    /// The tools in `tools`.
    #[must_use]
    pub const fn of(tools: &[Tool]) -> Probes {
        let mut bits = 0;
        let mut i = 0;
        while i < tools.len() {
            bits |= tools[i].bit();
            i += 1;
        }
        Probes(bits)
    }

    /// These and `tool`.
    #[must_use]
    pub const fn with(self, tool: Tool) -> Probes {
        Probes(self.0 | tool.bit())
    }

    /// These without `tool`.
    #[must_use]
    pub const fn without(self, tool: Tool) -> Probes {
        Probes(self.0 & !tool.bit())
    }

    /// These and `other`'s.
    #[must_use]
    pub const fn union(self, other: Probes) -> Probes {
        Probes(self.0 | other.0)
    }

    /// Whether `tool` is one of these.
    #[must_use]
    pub const fn contains(self, tool: Tool) -> bool {
        self.0 & tool.bit() != 0
    }

    /// Each of these tools.
    pub fn tools(self) -> impl Iterator<Item = Tool> {
        Tool::ALL
            .into_iter()
            .filter(move |tool| self.contains(*tool))
    }

    /// What `NodeFacts::program` reads for `program`, as a setup step names it.
    #[must_use]
    pub fn for_program(program: &str) -> Probes {
        program_tool(program).map_or(Probes::NONE, |(tool, _)| Probes::NONE.with(tool))
    }

    /// What `NodeFacts::key_like` reads for a key shaped as `key`.
    #[must_use]
    pub fn for_key(key: &CarryKey) -> Probes {
        let node = Probes::NONE.with(Tool::Node);
        match key {
            CarryKey::Node {
                package_manager, ..
            } => node.union(Probes::for_program(package_manager)),
            CarryKey::Rust { .. } => Probes::NONE.with(Tool::Rustc),
            CarryKey::Python { .. } => Probes::NONE.with(Tool::Python),
            CarryKey::Other { tool, .. } if tool == NODE_ABI_TOOL => node,
            CarryKey::Other { .. } | CarryKey::Unmeasured => Probes::NONE,
        }
    }
}

/// The tool that measures `program`, as a setup step names it, and the
/// fact that holds its version.
fn program_tool(program: &str) -> Option<(Tool, &'static str)> {
    Some(match program {
        "node" => (Tool::Node, name::NODE),
        "npm" => (Tool::Npm, name::NPM),
        "pnpm" => (Tool::Pnpm, name::PNPM),
        "yarn" => (Tool::Yarn, name::YARN),
        "bun" => (Tool::Bun, name::BUN),
        "rustc" => (Tool::Rustc, name::RUSTC),
        "cargo" => (Tool::Cargo, name::CARGO),
        "python3" => (Tool::Python, name::PYTHON),
        "uv" => (Tool::Uv, name::UV),
        _ => return None,
    })
}

/// How long measured facts are reused while each tool that gave them is
/// the same binary (issue #26): a tool updated in place, or one a `PATH`
/// change now finds elsewhere, is measured again at once.
pub const FACTS_TTL: Duration = Duration::from_mins(1);

/// Whether a measure may answer from what was measured lately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reuse {
    /// Facts measured within the time to live, for the same environment,
    /// home and binary.
    Cached,
    /// Only what each tool answers now, as `fabric doctor` shows; the
    /// answers still replace what the cache held.
    Fresh,
}

/// The facts this node measured lately, kept per environment, home and
/// tool for [`FACTS_TTL`] (issue #26).
///
/// Invariant: a kept answer is reused only while the tool's binary is the
/// same: found at the same path on the same `PATH`, resolving to the same
/// file, with the same modification time. A login that answered `no` is
/// never kept, so a user who logs in after a refusal is seen at once, and
/// neither is a tool with a command that did not answer in time.
#[derive(Debug)]
pub struct FactCache {
    state: Mutex<CacheState>,
}

#[derive(Debug)]
struct CacheState {
    ttl: Duration,
    entries: HashMap<CacheKey, Kept>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    env: ToolEnv,
    home: PathBuf,
    tool: Tool,
}

#[derive(Debug)]
struct Kept {
    binary: Binary,
    at: Instant,
    facts: Vec<(&'static str, String)>,
}

/// A tool's binary as a measure found it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Binary {
    /// Where the `PATH` finds it.
    found: PathBuf,
    /// The file that resolves to, links followed.
    resolved: PathBuf,
    /// When that file was last written.
    modified: SystemTime,
}

impl Binary {
    fn of(found: &Path) -> Option<Binary> {
        let resolved = std::fs::canonicalize(found).ok()?;
        let modified = std::fs::metadata(&resolved).ok()?.modified().ok()?;
        Some(Binary {
            found: found.to_owned(),
            resolved,
            modified,
        })
    }
}

/// What one command answered.
#[derive(Debug)]
enum Answer {
    /// The fact.
    Fact(String),
    /// It ran to its end and gave no fact, which is an answer too.
    Nothing,
    /// It did not start, or did not end within [`COMMAND_TIMEOUT`].
    Silent,
}

/// One tool a measure runs.
struct Miss {
    tool: Tool,
    found: PathBuf,
    binary: Option<Binary>,
    key: CacheKey,
}

impl FactCache {
    /// An empty cache whose answers live for `ttl`.
    #[must_use]
    pub fn new(ttl: Duration) -> FactCache {
        FactCache {
            state: Mutex::new(CacheState {
                ttl,
                entries: HashMap::new(),
            }),
        }
    }

    /// Measures `probes` with the tools `env` names: programs are looked
    /// up on its `PATH` and run with its roots, each set to its value or
    /// removed, never the daemon's own (ADR 0018, The client's environment).
    /// git is measured on the daemon's own `PATH`, where restore runs it,
    /// and `lsof` where Codex capture looks for it. A tool `probes` leaves
    /// out gives no fact.
    pub async fn measure(
        &self,
        home: &Path,
        env: &ToolEnv,
        probes: Probes,
        reuse: Reuse,
    ) -> NodeFacts {
        let inherited: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        self.measure_from(home, env, &inherited, (probes, reuse), Instant::now())
            .await
    }

    /// Measures as [`FactCache::measure`] does at `now`, with `inherited`
    /// as the daemon's own environment: each tool starts from it, and
    /// `env`'s `PATH` and roots then replace or remove what they name; git
    /// runs in `inherited` alone.
    async fn measure_from(
        &self,
        home: &Path,
        env: &ToolEnv,
        inherited: &[(OsString, OsString)],
        (probes, reuse): (Probes, Reuse),
        now: Instant,
    ) -> NodeFacts {
        let own = ToolEnv::from_vars(|name| {
            inherited
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        });
        let mut facts = NodeFacts::without_tools(home, env);
        let mut misses = Vec::new();
        for tool in probes.tools() {
            let path_var = if tool == Tool::Git { &own } else { env }.path_var();
            let Some(found) = find_on_path(tool.program(), path_var) else {
                continue;
            };
            let binary = Binary::of(&found);
            let key = CacheKey {
                env: env.clone(),
                home: home.to_owned(),
                tool,
            };
            let kept = match (reuse, &binary) {
                (Reuse::Cached, Some(binary)) => self.lookup(&key, binary, now),
                _ => None,
            };
            match kept {
                Some(kept) => facts.put(kept),
                None => misses.push(Miss {
                    tool,
                    found,
                    binary,
                    key,
                }),
            }
        }
        let answers = run_all(&misses, (env, &own), home, inherited).await;
        let mut keep = Vec::new();
        for (miss, answers) in misses.into_iter().zip(answers) {
            let mut measured = Vec::new();
            let mut whole = true;
            for ((fact, _, _), answer) in miss.tool.commands().iter().zip(answers) {
                match answer {
                    Answer::Fact(value) => measured.push((*fact, value)),
                    Answer::Nothing => {}
                    Answer::Silent => whole = false,
                }
            }
            let refused_login = matches!(miss.tool, Tool::ClaudeLogin | Tool::CodexLogin)
                && measured.iter().any(|(_, value)| value != "yes");
            if whole
                && !refused_login
                && let Some(binary) = miss.binary
            {
                keep.push((
                    miss.key,
                    Kept {
                        binary,
                        at: now,
                        facts: measured.clone(),
                    },
                ));
            }
            facts.put(measured);
        }
        if !keep.is_empty() {
            let mut state = crate::node::lock(&self.state);
            let ttl = state.ttl;
            state
                .entries
                .retain(|_, kept| now.saturating_duration_since(kept.at) < ttl);
            state.entries.extend(keep);
        }
        facts
    }

    /// The facts kept for `key`, while `binary` is the one that gave them
    /// and they are younger than the time to live.
    fn lookup(
        &self,
        key: &CacheKey,
        binary: &Binary,
        now: Instant,
    ) -> Option<Vec<(&'static str, String)>> {
        let state = crate::node::lock(&self.state);
        let kept = state.entries.get(key)?;
        (kept.binary == *binary && now.saturating_duration_since(kept.at) < state.ttl)
            .then(|| kept.facts.clone())
    }
}

/// Runs every command of every tool in `misses` at once, and returns each
/// tool's answers in the order of its commands. git runs in `own`, every
/// other tool in `env`.
async fn run_all(
    misses: &[Miss],
    (env, own): (&ToolEnv, &ToolEnv),
    home: &Path,
    inherited: &[(OsString, OsString)],
) -> Vec<Vec<Answer>> {
    let env = Arc::new(env.clone());
    let own = Arc::new(own.clone());
    let inherited: Arc<[(OsString, OsString)]> = inherited.into();
    let mut set = tokio::task::JoinSet::new();
    for (index, miss) in misses.iter().enumerate() {
        for (command, &(_, args, probe)) in miss.tool.commands().iter().enumerate() {
            let env = Arc::clone(if miss.tool == Tool::Git { &own } else { &env });
            let (binary, home, inherited) =
                (miss.found.clone(), home.to_owned(), Arc::clone(&inherited));
            set.spawn(async move {
                let answer = probe_command(&binary, args, probe, &env, &home, &inherited).await;
                (index, command, answer)
            });
        }
    }
    let mut answers: Vec<Vec<Answer>> = misses
        .iter()
        .map(|miss| {
            miss.tool
                .commands()
                .iter()
                .map(|_| Answer::Silent)
                .collect()
        })
        .collect();
    while let Some(done) = set.join_next().await {
        // A task that panicked answered nothing; its slot stays silent.
        if let Ok((index, command, answer)) = done
            && let Some(slot) = answers
                .get_mut(index)
                .and_then(|tool| tool.get_mut(command))
        {
            *slot = answer;
        }
    }
    answers
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

    /// The facts that need no process: the home, the platform, this
    /// build's version, `CARGO_HOME` as `env` sets it or its default, and
    /// where Codex capture finds `lsof`.
    fn without_tools(home: &Path, env: &ToolEnv) -> Self {
        let mut facts = BTreeMap::new();
        facts.insert(name::HOME.to_owned(), home.display().to_string());
        facts.insert(name::OS.to_owned(), std::env::consts::OS.to_owned());
        facts.insert(name::ARCH.to_owned(), std::env::consts::ARCH.to_owned());
        facts.insert(
            name::FABRIC.to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        );
        facts.insert(
            name::CARGO_HOME.to_owned(),
            env.root(Roots::CARGO_HOME)
                .map_or_else(|| home.join(".cargo"), PathBuf::from)
                .display()
                .to_string(),
        );
        if let Some(lsof) = fabric_harness::codex::Codex::lsof() {
            facts.insert(name::LSOF.to_owned(), lsof.display().to_string());
        }
        Self(facts)
    }

    fn put(&mut self, facts: Vec<(&'static str, String)>) {
        for (name, value) in facts {
            self.0.insert(name.to_owned(), value);
        }
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

    /// Only the facts `probes` measures, and those no tool gives: what a
    /// measure of `probes` would have found on the node these describe.
    #[cfg(test)]
    pub(crate) fn only(&self, probes: Probes) -> Self {
        let dropped: Vec<&str> = Tool::ALL
            .into_iter()
            .filter(|tool| !probes.contains(*tool))
            .flat_map(|tool| tool.commands().iter().map(|(fact, _, _)| *fact))
            .collect();
        Self(
            self.0
                .iter()
                .filter(|(name, _)| !dropped.contains(&name.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }
}

async fn probe_command(
    binary: &Path,
    args: &[&str],
    probe: Probe,
    env: &ToolEnv,
    home: &Path,
    inherited: &[(OsString, OsString)],
) -> Answer {
    let path_var = env.path_var();
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
    tracing::debug!(?binary, ?args, "measuring");
    let output = tokio::time::timeout(COMMAND_TIMEOUT, child).await;
    tracing::debug!(?binary, ?args, done = output.is_ok(), "measured");
    let Ok(Ok(output)) = output else {
        return Answer::Silent;
    };
    match probe {
        Probe::ExitCode => {
            Answer::Fact(if output.status.success() { "yes" } else { "no" }.to_owned())
        }
        Probe::Output => {
            let text = String::from_utf8(output.stdout).unwrap_or_default();
            let text = text.trim();
            if output.status.success() && !text.is_empty() && text.len() <= MAX_FACT_VALUE {
                Answer::Fact(text.to_owned())
            } else {
                Answer::Nothing
            }
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
        program_tool(program).map(|(_, fact)| fact)
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
        let stayed = |file: &str| stayed_behind(left, file);
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

    /// The tools a capture of the workspace `detection` describes measures
    /// for its keys: the ecosystem of each cache detection found, and node
    /// and python3, whose facts key a `node_modules` or a `.venv` the
    /// store's rules find where detection found none (`undetected_key`).
    /// The platform key needs no tool, and no login is among them.
    #[must_use]
    pub fn probes(detection: &Detection) -> Probes {
        detection
            .caches()
            .iter()
            .fold(Probes::of(&[Tool::Node, Tool::Python]), |probes, cache| {
                probes.union(match cache.ecosystem {
                    Ecosystem::Node(manager) => Probes::for_program(manager_name(manager)),
                    Ecosystem::Rust => Probes::of(&[Tool::Rustc]),
                    Ecosystem::Python => Probes::of(&[Tool::Python]),
                    Ecosystem::Other => Probes::NONE,
                })
            })
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

/// Whether `file`, a path from the workspace root, stayed on the
/// capturing node: it, or a directory above it, is among the snapshot's
/// `left` paths, which `.fabricignore` or a secret pattern kept there.
pub(crate) fn stayed_behind(left: &[LeftBehind], file: &str) -> bool {
    RelPath::parse(file).is_ok_and(|file| {
        left.iter()
            .any(|left| file.starts_with_ignore_ascii_case(&left.path))
    })
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
pub(crate) mod tests {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use fabric_client::api::{ClientEnv, PathDir, SystemPath};

    use super::*;

    /// Fake tools for the programs a measure runs, git aside, in one
    /// directory, each appending its name and arguments to a log before it
    /// answers as the tool would: the log counts the probe processes a
    /// measure starts. Both logins answer yes until `log_out`.
    ///
    /// macOS checks a new executable the first time it runs, which takes
    /// a tenth of a second or more here, so a test installs the tools it
    /// reads unless it must see a tool it does not expect run.
    pub(crate) struct CountingTools {
        dir: PathBuf,
        /// `PATH` of the tools, `/usr/bin` and `/bin`, with no tool root.
        pub(crate) env: ToolEnv,
    }

    impl CountingTools {
        /// Every tool a measure runs but git.
        pub(crate) fn new(dir: &Path) -> Self {
            Self::only(
                dir,
                &[
                    "id", "sw_vers", "xcrun", "node", "npm", "pnpm", "yarn", "bun", "rustc",
                    "cargo", "python3", "uv", "claude", "codex", "defaults",
                ],
            )
        }

        /// The tools named `programs`.
        pub(crate) fn only(dir: &Path, programs: &[&str]) -> Self {
            let dir = dir.join("counting");
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let path = std::env::join_paths([
                dir.join("bin").as_path(),
                Path::new("/usr/bin"),
                Path::new("/bin"),
            ])
            .unwrap();
            let tools = Self {
                env: ToolEnv::from_vars(|name| (name == "PATH").then(|| path.clone())),
                dir,
            };
            let login = format!("[ ! -f '{}' ]", tools.dir.join("logged-out").display());
            for (program, answer) in [
                ("id", "echo tester".to_owned()),
                ("sw_vers", "echo 15.6".to_owned()),
                ("xcrun", "echo 15.5".to_owned()),
                (
                    "node",
                    "case \"$1\" in --version) echo v22.23.2 ;; -p) echo 127 ;; esac".to_owned(),
                ),
                ("npm", "echo 10.9.2".to_owned()),
                (
                    "pnpm",
                    "case \"$1\" in --version) echo 9.9.9 ;; store) echo /pnpm/store ;; esac"
                        .to_owned(),
                ),
                ("yarn", "echo 1.22.22".to_owned()),
                ("bun", "echo 1.3.14".to_owned()),
                ("rustc", "echo 'rustc 1.97.0'".to_owned()),
                ("cargo", "echo 'cargo 1.97.0'".to_owned()),
                (
                    "python3",
                    "case \"$1\" in --version) echo 'Python 3.12.1' ;; -c) echo /opt/python ;; esac"
                        .to_owned(),
                ),
                ("uv", "echo 'uv 0.9.5'".to_owned()),
                (
                    "claude",
                    format!(
                        "case \"$1\" in --version) echo '2.1.294 (Claude Code)' ;; auth) {login} ;; esac"
                    ),
                ),
                (
                    "codex",
                    format!(
                        "case \"$1\" in --version) echo 'codex-cli 0.155.1' ;; login) {login} ;; esac"
                    ),
                ),
                ("defaults", "echo 0".to_owned()),
            ] {
                if programs.contains(&program) {
                    tools.install(program, &answer);
                }
            }
            tools
        }

        /// Writes `program`, which logs its run and then runs `answer`,
        /// through a short `sh`, so this process never holds it open for
        /// writing: Linux refuses to run a file another test's child
        /// inherited open for writing (`ETXTBSY`).
        pub(crate) fn install(&self, program: &str, answer: &str) {
            let log = self.dir.join("probes.log");
            let script = format!(
                "#!/bin/sh\necho \"{program} $*\" >> '{}'\n{answer}\n",
                log.display()
            );
            let mut writer = std::process::Command::new("/bin/sh")
                .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
                .arg(self.dir.join("bin").join(program))
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            writer
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            assert!(writer.wait().unwrap().success());
        }

        /// Gives `program` another modification time, as an update in place does.
        pub(crate) fn touch(&self, program: &str) {
            let status = std::process::Command::new("touch")
                .args(["-m", "-t", "202001011200"])
                .arg(self.dir.join("bin").join(program))
                .status()
                .unwrap();
            assert!(status.success());
        }

        /// Makes both logins answer no.
        pub(crate) fn log_out(&self) {
            std::fs::write(self.dir.join("logged-out"), "").unwrap();
        }

        /// The probes run since the last call, sorted, as `program args...`.
        pub(crate) fn ran(&self) -> Vec<String> {
            let log = self.dir.join("probes.log");
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            std::fs::write(&log, "").unwrap();
            let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
            lines.sort();
            lines
        }
    }

    const NODE_PROBES: [&str; 2] = ["node --version", "node -p process.versions.modules"];
    const PNPM_PROBES: [&str; 2] = ["pnpm --version", "pnpm store path"];

    /// An answer is reused while the tool's binary is the same and younger
    /// than the time to live: a second measure starts no process, a tool
    /// written since is probed again alone, another home probes again, and
    /// every tool is probed again once its answer is too old (issue #26).
    #[tokio::test]
    async fn a_tool_is_probed_again_only_once_it_changed_or_its_answer_is_old() {
        let dir = tempfile::tempdir().unwrap();
        let tools = CountingTools::only(dir.path(), &["node", "pnpm"]);
        let home = dir.path().join("home");
        let cache = FactCache::new(FACTS_TTL);
        let probes = (Probes::of(&[Tool::Node, Tool::Pnpm]), Reuse::Cached);
        let measure = |home: PathBuf, at: Duration| {
            let (cache, env) = (&cache, &tools.env);
            let now = start() + at;
            async move { cache.measure_from(&home, env, &[], probes, now).await }
        };
        let first = measure(home.clone(), Duration::ZERO).await;
        assert_eq!(first.get(name::PNPM_STORE), Some("/pnpm/store"));
        assert_eq!(tools.ran(), [NODE_PROBES, PNPM_PROBES].concat());

        let half = FACTS_TTL / 2;
        assert_eq!(measure(home.clone(), half).await, first);
        assert!(tools.ran().is_empty(), "a kept answer was probed again");

        tools.touch("pnpm");
        assert_eq!(measure(home.clone(), half).await, first);
        assert_eq!(tools.ran(), PNPM_PROBES, "only the changed tool");

        measure(dir.path().join("other"), half).await;
        assert_eq!(
            tools.ran(),
            [NODE_PROBES, PNPM_PROBES].concat(),
            "another home"
        );

        measure(home, half + FACTS_TTL).await;
        assert_eq!(tools.ran(), [NODE_PROBES, PNPM_PROBES].concat(), "too old");
    }

    /// One instant every test measures from, so `start() + at` is the
    /// same clock for every measure of a test.
    fn start() -> Instant {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *START.get_or_init(Instant::now)
    }

    /// A login that answered no is asked again at the next measure, so a
    /// user who logs in after a refused move is seen at once, while a yes
    /// is kept as any answer is.
    #[tokio::test]
    async fn a_refused_login_is_asked_again_and_a_login_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let tools = CountingTools::only(dir.path(), &["codex"]);
        let home = dir.path().join("home");
        let cache = FactCache::new(FACTS_TTL);
        let login = (Probes::of(&[Tool::CodexLogin]), Reuse::Cached);
        tools.log_out();
        for _ in 0..2 {
            let facts = cache
                .measure_from(&home, &tools.env, &[], login, start())
                .await;
            assert_eq!(facts.get(name::CODEX_LOGIN), Some("no"));
            assert_eq!(tools.ran(), ["codex login status"]);
        }
        std::fs::remove_file(dir.path().join("counting/logged-out")).unwrap();
        for ran in [&["codex login status"][..], &[]] {
            let facts = cache
                .measure_from(&home, &tools.env, &[], login, start())
                .await;
            assert_eq!(facts.get(name::CODEX_LOGIN), Some("yes"));
            assert_eq!(tools.ran(), ran);
        }
    }

    /// `fabric doctor` shows what each tool answers now: a fresh measure
    /// runs every probe again, and its answer, not the older one, serves
    /// the next measure that may reuse one.
    #[tokio::test]
    async fn a_fresh_measure_probes_again_and_its_answer_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let tools = CountingTools::only(dir.path(), &[]);
        let home = dir.path().join("home");
        let cache = FactCache::new(FACTS_TTL);
        let node = Probes::of(&[Tool::Node]);
        let version = dir.path().join("node-version");
        std::fs::write(&version, "v22.23.2\n").unwrap();
        tools.install(
            "node",
            &format!(
                "case \"$1\" in --version) cat '{}' ;; -p) echo 127 ;; esac",
                version.display()
            ),
        );
        let measure = |reuse| {
            let (cache, env, home) = (&cache, &tools.env, &home);
            async move {
                cache
                    .measure_from(home, env, &[], (node, reuse), start())
                    .await
            }
        };
        assert_eq!(
            measure(Reuse::Cached).await.get(name::NODE),
            Some("v22.23.2")
        );
        assert_eq!(tools.ran(), NODE_PROBES);
        // A version manager switched the version behind the same binary.
        std::fs::write(&version, "v24.1.0\n").unwrap();
        assert_eq!(measure(Reuse::Fresh).await.get(name::NODE), Some("v24.1.0"));
        assert_eq!(tools.ran(), NODE_PROBES);
        assert_eq!(
            measure(Reuse::Cached).await.get(name::NODE),
            Some("v24.1.0")
        );
        assert!(tools.ran().is_empty());
    }

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
    /// when the test process had no `PNPM_HOME`). Both measures share one
    /// cache, so an answer kept for one environment must not serve the
    /// other (issue #26).
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
        let cache = FactCache::new(FACTS_TTL);
        let all = (Probes::ALL, Reuse::Cached);
        let now = Instant::now();
        let facts = cache
            .measure_from(&home, &ToolEnv::from(&client), &daemon, all, now)
            .await;
        assert_eq!(facts.get(name::CARGO_HOME), Some("/client/cargo"));
        assert_eq!(facts.get(name::PNPM), Some("9.9.9"));
        assert_eq!(facts.get(name::PNPM_STORE), Some("/client/pnpm/store"));

        let bare = ClientEnv {
            path,
            roots: Roots::default(),
        };
        let facts = cache
            .measure_from(&home, &ToolEnv::from(&bare), &daemon, all, now)
            .await;
        let default_cargo = home.join(".cargo");
        assert_eq!(facts.get(name::CARGO_HOME), default_cargo.to_str());
        assert_eq!(facts.get(name::PNPM_STORE), Some("unset/store"));
    }

    /// Every fact a node can measure, as a rich node would answer.
    fn everything() -> NodeFacts {
        NodeFacts::from_pairs([
            (name::USER, "me"),
            (name::HOME, "/Users/me"),
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v22.23.2"),
            (name::NODE_ABI, "127"),
            (name::NPM, "10.9.2"),
            (name::PNPM, "11.23.0"),
            (name::PNPM_STORE, "/Users/me/Library/pnpm/store/v11"),
            (name::YARN, "1.22.22"),
            (name::BUN, "1.3.14"),
            (name::RUSTC, "rustc 1.97.1"),
            (name::CARGO_HOME, "/Users/me/.cargo"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
            (name::UV, "uv 0.9.5"),
            (name::CLAUDE, "2.1.294 (Claude Code)"),
            (name::CLAUDE_LOGIN, "yes"),
            (name::CODEX, "codex-cli 0.155.1"),
            (name::CODEX_LOGIN, "yes"),
        ])
    }

    /// A capture measures only what its keys read: for every cache
    /// detection finds, and for a `node_modules`, a `.venv` and build
    /// output the store's rules find without it, the key recorded from
    /// those facts is the key every fact gives, and no login is measured
    /// (issue #26).
    #[test]
    fn a_capture_measures_what_its_keys_read_and_no_login() {
        let workspaces: [&[(&str, &str)]; 7] = [
            &[],
            &[("package.json", "{}"), ("package-lock.json", "{}")],
            &[
                ("package.json", "{}"),
                ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            ],
            &[("package.json", "{}"), ("yarn.lock", "")],
            &[("package.json", "{}"), ("bun.lock", "{}")],
            &[(
                "Cargo.toml",
                "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
            )],
            &[("requirements.txt", "")],
        ];
        let undetected = [
            (CacheClass::Deps, "node_modules"),
            (CacheClass::Deps, "web/node_modules"),
            (CacheClass::Deps, ".venv"),
            (CacheClass::Build, "dist"),
            (CacheClass::ToolCache, ".cache"),
        ];
        for files in workspaces {
            let dir = tempfile::tempdir().unwrap();
            for (file, text) in files {
                std::fs::write(dir.path().join(file), text).unwrap();
            }
            let detection = fabric_env::detect::detect(dir.path(), &[]).unwrap();
            let probes = MeasuredKeys::probes(&detection);
            assert!(
                !probes.contains(Tool::ClaudeLogin) && !probes.contains(Tool::CodexLogin),
                "{files:?}"
            );
            let narrow = MeasuredKeys::new(everything().only(probes), Some(&detection));
            let full = MeasuredKeys::new(everything(), Some(&detection));
            let caches = detection
                .caches()
                .iter()
                .map(|cache| (cache.class, cache.path.clone()))
                .chain(
                    undetected
                        .iter()
                        .map(|(class, path)| (*class, RelPath::parse(path).unwrap())),
                );
            for (class, path) in caches {
                let key = full.key(class, &path);
                assert_ne!(key, CarryKey::Unmeasured, "{files:?} {path}");
                assert_eq!(narrow.key(class, &path), key, "{files:?} {path}");
            }
        }
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
