//! The carry decision: what a target does with each ignored path of a
//! snapshot (ADR 0012, as amended 2026-10-21 after gate findings F6 and F7).
//!
//! Invariant: one function decides, from the row the capturing node
//! recorded and the target's facts, and doctor, move and restore all call
//! it, so a path cannot be promised in one place and dropped in another. A
//! path that nothing can make again is never left out: it is carried, or
//! the move is refused and a forced one carries it. A path is rebuilt only
//! when the target can run what rebuilds it. A secret is never carried. Every
//! decision has one line that says why.

use std::fmt;

use fabric_core::env::SetupStep;
use fabric_core::path::RelPath;
use fabric_core::snapshot::{
    CacheEntry, CarryKey, CarryState, LeaveReason, Platform, Remake, SnapshotManifest,
};

use crate::facts::{
    NODE_ABI_TOOL, NodeFacts, PLATFORM_TOOL, Probes, Program, key_difference, major, name,
};

/// What a target does with one ignored path of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarryDecision {
    /// The target writes the bytes.
    Carry {
        /// Why the bytes are written.
        why: String,
        /// What the user should know before relying on them.
        warnings: Vec<String>,
    },
    /// The target leaves the bytes out and the path is made again there.
    Rebuild {
        /// What makes it.
        by: RebuildBy,
        /// Why the bytes are not used.
        because: String,
    },
    /// The target can neither use the bytes nor make the path again. A
    /// move, pushed or pulled, stops here unless forced; a forced one, and
    /// a restore that meets this without a move, writes the bytes, since
    /// leaving them out would lose them.
    Refuse {
        /// Why, precisely.
        reason: String,
    },
    /// The bytes do not reach the target's workspace.
    LeaveBehind {
        /// Why.
        reason: LeaveBehindReason,
    },
}

/// What makes a rebuilt path again on the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebuildBy {
    /// These setup steps, which resume runs when the path is missing (ADR 0007).
    Setup(Vec<SetupStep>),
    /// The next build, which the user or the agent runs.
    NextBuild,
}

/// Why a path's bytes do not reach the target's workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaveBehindReason {
    /// It matches a secret pattern; it never left the source.
    Secret,
    /// `.fabricignore` names it; it never left the source.
    Excluded,
    /// Every file under the cache matched a secret pattern or `.fabricignore`.
    NothingCaptured,
    /// Build output or a tool cache made for another operating system or architecture.
    OtherPlatform {
        /// The source's `os-arch`.
        source: String,
        /// The target's `os-arch`.
        target: String,
    },
}

impl CarryDecision {
    /// True when the target writes the bytes: carried, or refused, since a
    /// refused path reaches a restore only through `--force`, or on the
    /// node that already owns the job, and leaving it out there would lose
    /// it.
    #[must_use]
    pub const fn writes(&self) -> bool {
        matches!(
            self,
            CarryDecision::Carry { .. } | CarryDecision::Refuse { .. }
        )
    }

    /// True when the decision stops a move that is not forced.
    #[must_use]
    pub const fn refuses(&self) -> bool {
        matches!(self, CarryDecision::Refuse { .. })
    }

    /// The word a decision line starts with.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            CarryDecision::Carry { .. } => "carry",
            CarryDecision::Rebuild { .. } => "rebuild",
            CarryDecision::Refuse { .. } => "refuse",
            CarryDecision::LeaveBehind { .. } => "leave",
        }
    }
}

impl fmt::Display for CarryDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CarryDecision::Carry { why, warnings } => {
                f.write_str(why)?;
                for warning in warnings {
                    write!(f, "; warning: {warning}")?;
                }
                Ok(())
            }
            CarryDecision::Rebuild {
                by: RebuildBy::Setup(steps),
                because,
            } => {
                let commands: Vec<String> = steps.iter().map(command).collect();
                write!(f, "{} runs on the target: {because}", commands.join(", then "))
            }
            CarryDecision::Rebuild {
                by: RebuildBy::NextBuild,
                because,
            } => write!(f, "the next build on the target makes it: {because}"),
            CarryDecision::Refuse { reason } => f.write_str(reason),
            CarryDecision::LeaveBehind { reason } => match reason {
                LeaveBehindReason::Secret => f.write_str(
                    "it matches a secret pattern and never leaves the source; set it up on the target by hand",
                ),
                LeaveBehindReason::Excluded => {
                    f.write_str(".fabricignore excludes it; it never leaves the source")
                }
                LeaveBehindReason::NothingCaptured => f.write_str(
                    "every file under it matched a secret pattern or .fabricignore, so nothing of it was captured",
                ),
                LeaveBehindReason::OtherPlatform { source, target } => write!(
                    f,
                    "made for {source}, and the target is {target}; the next build there makes it again"
                ),
            },
        }
    }
}

/// One path and its decision, as doctor, move and resume print it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathDecision {
    /// The path, relative to the root.
    pub path: RelPath,
    /// What the target does with it.
    pub decision: CarryDecision,
}

impl fmt::Display for PathDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<7} {}  ({})",
            self.decision.word(),
            self.path,
            self.decision
        )
    }
}

/// `program args...`, in backticks, as a setup step runs.
pub(crate) fn command(step: &SetupStep) -> String {
    let mut text = format!("`{}", step.program);
    for arg in &step.args {
        text.push(' ');
        text.push_str(arg.as_str());
    }
    text.push('`');
    text
}

fn platform_name(platform: &Platform) -> String {
    format!("{}-{}", platform.os, platform.arch)
}

/// Whether native addons built under the source's key load on the target:
/// `None` when the key says nothing about the Node ABI.
fn same_node_abi(key: &CarryKey, target: &NodeFacts) -> Option<bool> {
    match key {
        CarryKey::Other { tool, version } if tool == NODE_ABI_TOOL => {
            Some(target.get(name::NODE_ABI) == Some(version.as_str()))
        }
        CarryKey::Node { node_major, .. } => Some(
            target
                .get(name::NODE)
                .and_then(major)
                .is_some_and(|m| m == *node_major),
        ),
        _ => None,
    }
}

/// Why the bytes under `entry` may not load on the target, if they may not:
/// native addons built for another ABI or another platform.
fn native_risk(entry: &CacheEntry, same_platform: bool, target: &NodeFacts) -> Option<String> {
    if entry.native_files == 0 {
        return None;
    }
    let (addons, were) = if entry.native_files == 1 {
        ("1 native addon".to_owned(), "was")
    } else {
        (format!("{} native addons", entry.native_files), "were")
    };
    if !same_platform {
        return Some(format!(
            "{addons} inside {were} built for another os or architecture"
        ));
    }
    match (same_node_abi(&entry.key, target), &entry.key) {
        (Some(true), _) => None,
        (Some(false), CarryKey::Other { version, .. }) => Some(format!(
            "{addons} inside {were} built for node ABI {version}, and the target's is {}",
            target.get(name::NODE_ABI).unwrap_or("unknown")
        )),
        (Some(false), CarryKey::Node { node_major, .. }) => Some(format!(
            "{addons} inside {were} built under node major {node_major}, and the target runs {}",
            target.get(name::NODE).unwrap_or("no node")
        )),
        _ => Some(format!(
            "{addons} inside {were} built for one node ABI and may not load under another"
        )),
    }
}

/// Why the target cannot run `steps` as the source would, if it cannot.
///
/// A program this node does not measure, such as a declared `make`, is
/// assumed present: resume checks every program on `PATH` before it runs
/// any step and says what is missing (ADR 0007).
fn cannot_run(steps: &[SetupStep], key: &CarryKey, target: &NodeFacts) -> Option<String> {
    for step in steps {
        let program = step.program.to_string();
        match target.program(&program) {
            Program::Unmeasured => {}
            Program::Missing => return Some(format!("{program} is not installed on the target")),
            Program::Installed(version) => {
                if let CarryKey::Node {
                    package_manager,
                    pm_major,
                    ..
                } = key
                    && *package_manager == program
                {
                    let here = major(version);
                    if here.as_deref() != Some(pm_major.as_str()) {
                        return Some(format!(
                            "{program} major {pm_major} wrote the lockfile on the source, and the target has {program} {version}"
                        ));
                    }
                }
            }
        }
    }
    None
}

/// Decides what a node that measured `target` does with one cache row of a
/// snapshot captured on `source` (ADR 0012, as amended 2026-10-21).
///
/// - Nothing makes the path again (a dependency tree with no lockfile, a
///   declared cache no step fills): carry, whatever the versions and the
///   platform, with a warning when native addons inside were built for
///   another ABI, or when it may hold binaries for another platform. A
///   restore writes it as it left, the record of a finished install
///   included even when it has holes, since no step was checked here.
/// - Setup steps make it from a lockfile: carry when the keys match; else
///   rebuild when the target can run those steps as the source would; else
///   carry when the platform matches, nothing inside is native, nothing
///   under it stayed behind and a finished install vouches for the bytes,
///   since without one resume would run the steps there anyway, and a
///   restore drops the record of a tree with holes, so resume would run
///   them over it too; else refuse.
/// - The next build makes it: carry when the keys match, or when the
///   platform matches and the target lacks the toolchain the key names;
///   leave it behind on another platform; else rebuild.
#[must_use]
pub fn decide(entry: &CacheEntry, source: &Platform, target: &NodeFacts) -> CarryDecision {
    if entry.carry == CarryState::NotCarried {
        return CarryDecision::LeaveBehind {
            reason: LeaveBehindReason::NothingCaptured,
        };
    }
    let seen = Seen::new(entry, source, target);
    match &entry.remake {
        Remake::Nothing => {
            let mut warnings = seen.warnings;
            // A tree keyed on the addon ABI with no addon inside is plain
            // JavaScript, which runs anywhere; anything else made for one
            // platform may hold binaries for it.
            let plain_js = entry.native_files == 0
                && matches!(&entry.key, CarryKey::Other { tool, .. } if tool == NODE_ABI_TOOL);
            if !seen.same_platform && seen.native.is_none() && !plain_js {
                warnings.push(format!(
                    "it was made for {}, and the target is {}; anything built for that platform inside may not run there",
                    platform_name(source),
                    platform_name(&seen.target_platform)
                ));
            }
            if let CarryKey::Python { .. } = entry.key
                && !entry.key.matches(&seen.theirs)
            {
                warnings.push(format!(
                    "its interpreter may be missing on the target: {}",
                    key_difference(&entry.key, &seen.theirs)
                ));
            }
            CarryDecision::Carry {
                why: "no lockfile or setup step makes it again, so it travels as the user's files"
                    .to_owned(),
                warnings,
            }
        }
        Remake::Setup { steps, installed } => {
            decide_setup(entry, (steps, *installed), seen, source, target)
        }
        Remake::NextBuild => decide_next_build(entry, seen, source),
    }
}

/// What `decide` reads about one row on the target, worked out once.
struct Seen {
    target_platform: Platform,
    same_platform: bool,
    /// The key the target measures for the row's ecosystem.
    theirs: CarryKey,
    /// The keys match and the secret policy left no hole.
    keys_match: bool,
    /// Why native addons inside may not load on the target.
    native: Option<String>,
    /// Why the bytes are not used as they are: the key field that differs, or the holes.
    difference: String,
    warnings: Vec<String>,
}

impl Seen {
    fn new(entry: &CacheEntry, source: &Platform, target: &NodeFacts) -> Seen {
        let target_platform = target.platform();
        let same_platform = target_platform == *source;
        let theirs = target.key_like(&entry.key);
        let partial = entry.carry == CarryState::Partial;
        let native = native_risk(entry, same_platform, target);
        let holes = "files under it that match a secret pattern stayed on the source";
        let mut warnings: Vec<String> = native.iter().cloned().collect();
        if partial {
            warnings.push(holes.to_owned());
        }
        // No key names the platform but the platform key itself, and bytes
        // built for one os or arch are not trusted on another (ADR 0004).
        let difference = if !same_platform {
            format!(
                "made for {}, and the target is {}",
                platform_name(source),
                platform_name(&target_platform)
            )
        } else if partial {
            holes.to_owned()
        } else {
            key_difference(&entry.key, &theirs)
        };
        Seen {
            keys_match: entry.key.matches(&theirs) && !partial && same_platform,
            difference,
            target_platform,
            same_platform,
            theirs,
            native,
            warnings,
        }
    }
}

/// A path setup steps make from a lockfile; `installed` says a finished
/// install of those steps vouches for its bytes.
fn decide_setup(
    entry: &CacheEntry,
    (steps, installed): (&[SetupStep], bool),
    seen: Seen,
    source: &Platform,
    target: &NodeFacts,
) -> CarryDecision {
    if seen.keys_match {
        return CarryDecision::Carry {
            why: "the keys match".to_owned(),
            warnings: seen.warnings,
        };
    }
    let Some(missing) = cannot_run(steps, &entry.key, target) else {
        return CarryDecision::Rebuild {
            by: RebuildBy::Setup(steps.to_vec()),
            because: seen.difference,
        };
    };
    let steps_text: Vec<String> = steps.iter().map(command).collect();
    let steps_text = steps_text.join(", then ");
    let advice = "install the tool on the target, or pass --force to carry it as it is";
    let partial = entry.carry == CarryState::Partial;
    match (seen.same_platform, seen.native) {
        (true, None) if installed && !partial => CarryDecision::Carry {
            why: format!(
                "{missing}, so {steps_text} cannot rebuild it there; it holds no native addon, the os and arch match and a finished install vouches for it, so it travels as it is"
            ),
            warnings: seen.warnings,
        },
        // A restore drops the install record of a tree with holes, which
        // would call it whole, so materialize runs the steps over it before
        // the job can resume.
        (true, None) if partial => CarryDecision::Refuse {
            reason: format!(
                "{missing}, so {steps_text} cannot rebuild it there, and files under it that match a secret pattern or .fabricignore stayed on the source, so resume there would run {steps_text} first to fill them; install the tool on the target, or pass --force to carry it and install the tool before resuming"
            ),
        },
        // Materialize runs the steps over a tree no finished install
        // vouches for, so the bytes alone would not let the job resume.
        (true, None) => CarryDecision::Refuse {
            reason: format!(
                "{missing}, so {steps_text} cannot rebuild it there, and no install Fabric finished on the source vouches for it, so resume there would run {steps_text} first; install the tool on the target, or pass --force to carry it and install the tool before resuming"
            ),
        },
        (true, Some(native)) => CarryDecision::Refuse {
            reason: format!(
                "{missing}, so {steps_text} cannot rebuild it there, and {native}; {advice}"
            ),
        },
        (false, _) => CarryDecision::Refuse {
            reason: format!(
                "{missing}, so {steps_text} cannot rebuild it there, and it was made for {}, not {}; {advice}",
                platform_name(source),
                platform_name(&seen.target_platform)
            ),
        },
    }
}

/// A path the next build or tool run makes.
fn decide_next_build(entry: &CacheEntry, seen: Seen, source: &Platform) -> CarryDecision {
    if seen.keys_match {
        let why = match &entry.key {
            CarryKey::Other { tool, version } if tool == PLATFORM_TOOL => {
                format!("same os and arch, {version}")
            }
            _ => "the keys match".to_owned(),
        };
        return CarryDecision::Carry {
            why,
            warnings: seen.warnings,
        };
    }
    if !seen.same_platform {
        return CarryDecision::LeaveBehind {
            reason: LeaveBehindReason::OtherPlatform {
                source: platform_name(source),
                target: platform_name(&seen.target_platform),
            },
        };
    }
    if matches!(seen.theirs, CarryKey::Unmeasured) {
        CarryDecision::Carry {
            why: format!(
                "{}; build output for the same os and arch travels as it is",
                seen.difference
            ),
            warnings: seen.warnings,
        }
    } else {
        CarryDecision::Rebuild {
            by: RebuildBy::NextBuild,
            because: seen.difference,
        }
    }
}

/// The tools whose facts `decide_all` reads for `manifest` (issue #26):
/// for each row it carries, what the target's key of the same shape needs
/// and the programs its setup steps run. The platform needs no tool.
#[must_use]
pub fn probes(manifest: &SnapshotManifest) -> Probes {
    manifest
        .caches
        .iter()
        .filter(|entry| entry.carry != CarryState::NotCarried)
        .fold(Probes::NONE, |probes, entry| {
            let steps = match &entry.remake {
                Remake::Setup { steps, .. } => steps.as_slice(),
                Remake::Nothing | Remake::NextBuild => &[],
            };
            steps
                .iter()
                .fold(probes.union(Probes::for_key(&entry.key)), |probes, step| {
                    probes.union(Probes::for_program(step.program.as_str()))
                })
        })
}

/// Decides every cache row of `manifest` for a node that measured `target`,
/// then lists every path that stayed on the source, sorted by path.
#[must_use]
pub fn decide_all(manifest: &SnapshotManifest, target: &NodeFacts) -> Vec<PathDecision> {
    let mut decisions: Vec<PathDecision> = manifest
        .caches
        .iter()
        .map(|entry| PathDecision {
            path: entry.path.clone(),
            decision: decide(entry, &manifest.platform, target),
        })
        .chain(manifest.left_behind.iter().map(|left| PathDecision {
            path: left.path.clone(),
            decision: CarryDecision::LeaveBehind {
                reason: match left.reason {
                    LeaveReason::Secret => LeaveBehindReason::Secret,
                    LeaveReason::Excluded => LeaveBehindReason::Excluded,
                },
            },
        }))
        .collect();
    decisions.sort_by(|a, b| a.path.cmp(&b.path));
    decisions
}

#[cfg(test)]
mod tests {
    use fabric_core::env::{RunWhen, SetupArg};
    use fabric_core::harness::ProgramName;
    use fabric_core::snapshot::CacheClass;

    use super::*;

    fn macos() -> Platform {
        Platform {
            os: "macos".to_owned(),
            arch: "aarch64".to_owned(),
        }
    }

    /// The Pro as the gate measured it: Node 22, npm 10, pnpm 11.
    fn pro() -> NodeFacts {
        NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v22.23.2"),
            (name::NODE_ABI, "127"),
            (name::NPM, "10.9.2"),
            (name::PNPM, "11.23.0"),
            (name::PNPM_STORE, "/Users/x/Library/pnpm/store/v11"),
            (name::RUSTC, "rustc 1.97.1"),
            (name::CARGO_HOME, "/Users/x/.cargo"),
        ])
    }

    /// The Air as the gate measured it: Node 26, npm 11, no pnpm, older rustc.
    fn air() -> NodeFacts {
        NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v26.0.0"),
            (name::NODE_ABI, "147"),
            (name::NPM, "11.1.0"),
            (name::RUSTC, "rustc 1.96.0"),
            (name::CARGO_HOME, "/Users/x/.cargo"),
        ])
    }

    fn linux() -> NodeFacts {
        let mut pairs: Vec<(String, String)> = air()
            .iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        for (k, v) in &mut pairs {
            if k == name::OS {
                "linux".clone_into(v);
            }
        }
        NodeFacts::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    fn step(program: &str, args: &[&str]) -> SetupStep {
        SetupStep {
            program: ProgramName::parse(program).unwrap(),
            args: args.iter().map(|a| SetupArg::parse(a).unwrap()).collect(),
            cwd: None,
            when: RunWhen::CachesMissing,
        }
    }

    fn entry(path: &str, class: CacheClass, key: CarryKey, remake: Remake) -> CacheEntry {
        CacheEntry {
            path: RelPath::parse(path).unwrap(),
            class,
            carry: CarryState::Carried,
            key,
            remake,
            native_files: 0,
        }
    }

    /// A `node_modules` with no lockfile, as the Pro records it.
    fn hand_made_tree() -> CacheEntry {
        entry(
            "node_modules",
            CacheClass::Deps,
            CarryKey::Other {
                tool: NODE_ABI_TOOL.to_owned(),
                version: "127".to_owned(),
            },
            Remake::Nothing,
        )
    }

    /// A tree a finished install filled from a lockfile, as the sentinel says.
    fn locked_tree(manager: &str, pm_major: &str, install: SetupStep) -> CacheEntry {
        entry(
            "node_modules",
            CacheClass::Deps,
            CarryKey::Node {
                node_major: "22".to_owned(),
                package_manager: manager.to_owned(),
                pm_major: pm_major.to_owned(),
                pnpm_store_path: (manager == "pnpm")
                    .then(|| "/Users/x/Library/pnpm/store/v11".to_owned()),
            },
            Remake::Setup {
                steps: vec![install],
                installed: true,
            },
        )
    }

    fn pnpm_tree() -> CacheEntry {
        locked_tree(
            "pnpm",
            "11",
            step("pnpm", &["install", "--frozen-lockfile"]),
        )
    }

    fn npm_tree() -> CacheEntry {
        locked_tree("npm", "10", step("npm", &["ci"]))
    }

    fn dist() -> CacheEntry {
        entry(
            "dist",
            CacheClass::Build,
            CarryKey::Other {
                tool: PLATFORM_TOOL.to_owned(),
                version: "macos-aarch64".to_owned(),
            },
            Remake::NextBuild,
        )
    }

    fn rust_target() -> CacheEntry {
        entry(
            "target",
            CacheClass::Build,
            pro().key_like(&CarryKey::Rust {
                rustc_vv: String::new(),
                cargo_home: fabric_core::path::AbsPath::parse("/x").unwrap(),
            }),
            Remake::NextBuild,
        )
    }

    /// F6 as the gate met it: a hand-made `node_modules` and two Node
    /// majors. It is carried, because nothing on the Air can make it again.
    #[test]
    fn a_tree_with_no_lockfile_is_carried_across_node_majors() {
        let decision = decide(&hand_made_tree(), &macos(), &air());
        assert!(
            matches!(&decision, CarryDecision::Carry { warnings, .. } if warnings.is_empty()),
            "{decision:?}"
        );
        assert!(decision.writes());
    }

    #[test]
    fn a_tree_with_no_lockfile_and_native_addons_is_carried_with_a_warning() {
        let mut tree = hand_made_tree();
        tree.native_files = 2;
        let decision = decide(&tree, &macos(), &air());
        let CarryDecision::Carry { warnings, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(
            warnings,
            &["2 native addons inside were built for node ABI 127, and the target's is 147"]
        );
        let same = decide(&tree, &macos(), &pro());
        assert!(
            matches!(&same, CarryDecision::Carry { warnings, .. } if warnings.is_empty()),
            "{same:?}"
        );
    }

    #[test]
    fn a_tree_with_no_lockfile_is_carried_even_to_another_platform() {
        let decision = decide(&hand_made_tree(), &macos(), &linux());
        assert!(decision.writes(), "{decision:?}");
        assert!(!decision.refuses());
    }

    /// A Node or Python key names no platform, so equal keys can match
    /// across platforms, while a `node_modules` or a venv holds binaries
    /// built for the platform it was installed on. A tree something on
    /// the target can make again never crosses; it is rebuilt there, and
    /// build output is left behind.
    #[test]
    fn a_cache_built_on_another_platform_is_not_carried() {
        let target = NodeFacts::from_pairs([
            (name::OS, "linux"),
            (name::ARCH, "x86_64"),
            (name::NODE, "v22.23.2"),
            (name::NPM, "10.9.2"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
            (name::UV, "uv 0.9.5"),
        ]);
        let venv = entry(
            ".venv",
            CacheClass::Deps,
            CarryKey::Python {
                python_version: "Python 3.12.1".to_owned(),
                python_home: fabric_core::path::AbsPath::parse("/opt/py312").unwrap(),
            },
            Remake::Setup {
                steps: vec![step("uv", &["sync", "--frozen"])],
                installed: true,
            },
        );
        for tree in [npm_tree(), venv] {
            assert!(tree.key.matches(&target.key_like(&tree.key)), "{tree:?}");
            let decision = decide(&tree, &macos(), &target);
            assert!(
                matches!(decision, CarryDecision::Rebuild { .. }),
                "{decision:?}"
            );
            assert!(!decision.writes());
        }
        assert!(!decide(&dist(), &macos(), &target).writes());
        assert!(decide(&npm_tree(), &macos(), &pro()).writes());
    }

    /// A venv nothing can make again still crosses, since leaving it out
    /// would lose it, but its interpreter link and compiled modules are
    /// for the source's platform, so the line says so even where the
    /// interpreter keys match. A tree of plain JavaScript needs no warning.
    #[test]
    fn a_path_nothing_remakes_says_when_it_crosses_a_platform() {
        let target = NodeFacts::from_pairs([
            (name::OS, "linux"),
            (name::ARCH, "aarch64"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
        ]);
        let venv = entry(
            ".venv",
            CacheClass::Deps,
            CarryKey::Python {
                python_version: "Python 3.12.1".to_owned(),
                python_home: fabric_core::path::AbsPath::parse("/opt/py312").unwrap(),
            },
            Remake::Nothing,
        );
        let decision = decide(&venv, &macos(), &target);
        let CarryDecision::Carry { warnings, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(
            warnings,
            &[
                "it was made for macos-aarch64, and the target is linux-aarch64; anything built for that platform inside may not run there"
            ]
        );
        let same = NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
        ]);
        assert!(
            matches!(decide(&venv, &macos(), &same), CarryDecision::Carry { warnings, .. } if warnings.is_empty())
        );
        assert!(matches!(
            decide(&hand_made_tree(), &macos(), &linux()),
            CarryDecision::Carry { warnings, .. } if warnings.is_empty()
        ));
    }

    #[test]
    fn a_locked_tree_whose_keys_match_is_carried() {
        let decision = decide(&npm_tree(), &macos(), &pro());
        assert_eq!(
            decision,
            CarryDecision::Carry {
                why: "the keys match".to_owned(),
                warnings: Vec::new()
            }
        );
    }

    #[test]
    fn a_locked_tree_is_rebuilt_where_the_same_manager_major_runs() {
        let target = NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v26.0.0"),
            (name::NPM, "10.1.0"),
        ]);
        let decision = decide(&npm_tree(), &macos(), &target);
        assert_eq!(
            decision,
            CarryDecision::Rebuild {
                by: RebuildBy::Setup(vec![step("npm", &["ci"])]),
                because: "node major 22 on the source, 26 on the target".to_owned(),
            }
        );
        assert!(!decision.writes());
        assert_eq!(
            PathDecision {
                path: RelPath::parse("node_modules").unwrap(),
                decision,
            }
            .to_string(),
            "rebuild node_modules  (`npm ci` runs on the target: node major 22 on the source, 26 on the target)"
        );
    }

    /// Equal keys do not carry a tree to another platform: no Node key
    /// names the os, and its addons and shims are built for one.
    #[test]
    fn a_locked_tree_with_equal_keys_is_rebuilt_on_another_platform() {
        let target = NodeFacts::from_pairs([
            (name::OS, "linux"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v22.1.0"),
            (name::NPM, "10.0.0"),
        ]);
        assert!(npm_tree().key.matches(&target.key_like(&npm_tree().key)));
        assert_eq!(
            decide(&npm_tree(), &macos(), &target),
            CarryDecision::Rebuild {
                by: RebuildBy::Setup(vec![step("npm", &["ci"])]),
                because: "made for macos-aarch64, and the target is linux-aarch64".to_owned(),
            }
        );
    }

    /// The gate's pnpm case: the Air has no pnpm, the tree is pure
    /// JavaScript and both Macs are macos-aarch64, so it travels as it is.
    #[test]
    fn a_pure_locked_tree_travels_where_the_manager_is_missing() {
        let decision = decide(&pnpm_tree(), &macos(), &air());
        let CarryDecision::Carry { why, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert!(
            why.starts_with("pnpm is not installed on the target"),
            "{why}"
        );
    }

    /// A locked tree installed outside Fabric holds no sentinel, so after
    /// a restore materialize runs the lockfile install over it, and a
    /// target without the package manager could not resume the job. Such
    /// a tree is refused where a finished install would have travelled
    /// (review of PR #11).
    #[test]
    fn a_locked_tree_no_finished_install_vouches_for_is_refused_where_it_cannot_be_rebuilt() {
        let mut tree = pnpm_tree();
        tree.remake = Remake::Setup {
            steps: vec![step("pnpm", &["install", "--frozen-lockfile"])],
            installed: false,
        };
        let decision = decide(&tree, &macos(), &air());
        let CarryDecision::Refuse { reason } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(
            reason,
            "pnpm is not installed on the target, so `pnpm install --frozen-lockfile` cannot rebuild it there, and no install Fabric finished on the source vouches for it, so resume there would run `pnpm install --frozen-lockfile` first; install the tool on the target, or pass --force to carry it and install the tool before resuming"
        );
        assert!(decision.writes(), "a forced move must not lose the tree");
        let mut npm = npm_tree();
        npm.remake = Remake::Setup {
            steps: vec![step("npm", &["ci"])],
            installed: false,
        };
        assert!(decide(&npm, &macos(), &air()).refuses());
        assert!(matches!(
            decide(&npm, &macos(), &pro()),
            CarryDecision::Carry { .. }
        ));
    }

    #[test]
    fn a_locked_tree_whose_manager_major_differs_is_not_rebuilt() {
        let decision = decide(&npm_tree(), &macos(), &air());
        let CarryDecision::Carry { why, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert!(
            why.starts_with("npm major 10 wrote the lockfile on the source"),
            "{why}"
        );
    }

    #[test]
    fn a_native_locked_tree_the_target_cannot_rebuild_is_refused() {
        let mut tree = pnpm_tree();
        tree.native_files = 1;
        let decision = decide(&tree, &macos(), &air());
        let CarryDecision::Refuse { reason } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(
            reason,
            "pnpm is not installed on the target, so `pnpm install --frozen-lockfile` cannot rebuild it there, and 1 native addon inside was built under node major 22, and the target runs v26.0.0; install the tool on the target, or pass --force to carry it as it is"
        );
        assert!(decision.refuses());
        assert!(decision.writes(), "a forced move must not lose the tree");
    }

    #[test]
    fn a_locked_tree_the_target_cannot_rebuild_on_another_platform_is_refused() {
        let decision = decide(&pnpm_tree(), &macos(), &linux());
        assert!(decision.refuses(), "{decision:?}");
    }

    /// A tree with holes is refused where it cannot be rebuilt: a restore
    /// writes it without the install record that would call it whole, so
    /// resume there would run the install first, and the target has no
    /// installer (review of PR #11, issue #24).
    #[test]
    fn a_partial_locked_tree_the_target_cannot_rebuild_is_refused() {
        let mut tree = pnpm_tree();
        tree.carry = CarryState::Partial;
        let decision = decide(&tree, &macos(), &air());
        let CarryDecision::Refuse { reason } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(
            reason,
            "pnpm is not installed on the target, so `pnpm install --frozen-lockfile` cannot rebuild it there, and files under it that match a secret pattern or .fabricignore stayed on the source, so resume there would run `pnpm install --frozen-lockfile` first to fill them; install the tool on the target, or pass --force to carry it and install the tool before resuming"
        );
        assert!(matches!(
            decide(&pnpm_tree(), &macos(), &air()),
            CarryDecision::Carry { .. }
        ));
    }

    /// A tree the secret policy punched a hole in is never trusted as warm
    /// where it can be rebuilt (ADR 0004, Partial).
    #[test]
    fn a_partial_locked_tree_is_rebuilt_even_when_the_keys_match() {
        let mut tree = npm_tree();
        tree.carry = CarryState::Partial;
        let decision = decide(&tree, &macos(), &pro());
        assert!(
            matches!(decision, CarryDecision::Rebuild { .. }),
            "{decision:?}"
        );
    }

    #[test]
    fn a_partial_tree_with_no_lockfile_is_carried_with_a_warning() {
        let mut tree = hand_made_tree();
        tree.carry = CarryState::Partial;
        let decision = decide(&tree, &macos(), &air());
        let CarryDecision::Carry { warnings, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(warnings.len(), 1);
    }

    /// F7: `dist` between the gate Macs, both macos-aarch64.
    #[test]
    fn build_output_is_carried_on_the_same_platform() {
        let decision = decide(&dist(), &macos(), &air());
        assert_eq!(
            decision,
            CarryDecision::Carry {
                why: "same os and arch, macos-aarch64".to_owned(),
                warnings: Vec::new()
            }
        );
    }

    #[test]
    fn build_output_is_left_behind_on_another_platform_with_the_reason() {
        let decision = decide(&dist(), &macos(), &linux());
        assert_eq!(
            decision,
            CarryDecision::LeaveBehind {
                reason: LeaveBehindReason::OtherPlatform {
                    source: "macos-aarch64".to_owned(),
                    target: "linux-aarch64".to_owned(),
                }
            }
        );
    }

    #[test]
    fn a_rust_target_is_rebuilt_under_another_rustc() {
        let decision = decide(&rust_target(), &macos(), &air());
        assert_eq!(
            decision,
            CarryDecision::Rebuild {
                by: RebuildBy::NextBuild,
                because: "rustc -vV differs".to_owned()
            }
        );
        assert!(matches!(
            decide(&rust_target(), &macos(), &pro()),
            CarryDecision::Carry { .. }
        ));
    }

    #[test]
    fn a_rust_target_travels_where_no_rustc_could_rebuild_it() {
        let target = NodeFacts::from_pairs([(name::OS, "macos"), (name::ARCH, "aarch64")]);
        let decision = decide(&rust_target(), &macos(), &target);
        assert!(
            matches!(decision, CarryDecision::Carry { .. }),
            "{decision:?}"
        );
    }

    /// rustup installs cargo and rustc as separate components, so a
    /// target can have rustc and no cargo. A declared cargo step is judged
    /// by the target's cargo: read through rustc, it was a rebuild that
    /// left the bytes out, and materialize then refused the missing cargo
    /// after the job had moved (issue #24).
    #[test]
    fn a_cargo_step_is_not_a_rebuild_where_only_rustc_is_installed() {
        let vendor = entry(
            "vendor",
            CacheClass::Deps,
            pro().key_like(&CarryKey::Rust {
                rustc_vv: String::new(),
                cargo_home: fabric_core::path::AbsPath::parse("/x").unwrap(),
            }),
            Remake::Setup {
                steps: vec![step("cargo", &["vendor", "--locked"])],
                installed: false,
            },
        );
        let rustc_only = [
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::RUSTC, "rustc 1.96.0"),
            (name::CARGO_HOME, "/Users/x/.cargo"),
        ];
        let decision = decide(&vendor, &macos(), &NodeFacts::from_pairs(rustc_only));
        assert_eq!(
            decision,
            CarryDecision::Refuse {
                reason: "cargo is not installed on the target, so `cargo vendor --locked` cannot rebuild it there, and no install Fabric finished on the source vouches for it, so resume there would run `cargo vendor --locked` first; install the tool on the target, or pass --force to carry it and install the tool before resuming".to_owned()
            }
        );
        let with_cargo = NodeFacts::from_pairs(
            rustc_only
                .into_iter()
                .chain([(name::CARGO, "cargo 1.96.0 (a1b2c3d4e 2026-09-01)")]),
        );
        assert!(
            matches!(
                decide(&vendor, &macos(), &with_cargo),
                CarryDecision::Rebuild { .. }
            ),
            "a target with cargo rebuilds it"
        );
    }

    #[test]
    fn a_venv_with_a_lockfile_is_rebuilt_where_uv_runs_and_travels_where_it_does_not() {
        let source = NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
        ]);
        let venv = entry(
            ".venv",
            CacheClass::Deps,
            source.key_like(&CarryKey::Python {
                python_version: String::new(),
                python_home: fabric_core::path::AbsPath::parse("/x").unwrap(),
            }),
            Remake::Setup {
                steps: vec![step("uv", &["sync", "--frozen"])],
                installed: true,
            },
        );
        let with_uv = NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PYTHON, "Python 3.13.0"),
            (name::PYTHON_HOME, "/opt/py313"),
            (name::UV, "uv 0.9.5"),
        ]);
        assert!(matches!(
            decide(&venv, &macos(), &with_uv),
            CarryDecision::Rebuild { .. }
        ));
        let without_uv = NodeFacts::from_pairs([
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PYTHON, "Python 3.13.0"),
            (name::PYTHON_HOME, "/opt/py313"),
        ]);
        let decision = decide(&venv, &macos(), &without_uv);
        assert!(
            matches!(&decision, CarryDecision::Carry { why, .. } if why.starts_with("uv is not installed")),
            "{decision:?}"
        );
    }

    #[test]
    fn a_venv_with_no_lockfile_is_carried_and_names_its_interpreter() {
        let venv = entry(
            ".venv",
            CacheClass::Deps,
            CarryKey::Python {
                python_version: "Python 3.12.1".to_owned(),
                python_home: fabric_core::path::AbsPath::parse("/opt/py312").unwrap(),
            },
            Remake::Nothing,
        );
        let decision = decide(&venv, &macos(), &air());
        let CarryDecision::Carry { warnings, .. } = &decision else {
            panic!("{decision:?}");
        };
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn a_cache_nothing_was_captured_of_is_left_behind() {
        let mut tree = npm_tree();
        tree.carry = CarryState::NotCarried;
        assert_eq!(
            decide(&tree, &macos(), &pro()),
            CarryDecision::LeaveBehind {
                reason: LeaveBehindReason::NothingCaptured
            }
        );
    }

    /// A declared step whose program this node does not measure is assumed
    /// to run; resume names a missing program before it runs anything.
    #[test]
    fn a_declared_step_with_an_unmeasured_program_is_a_rebuild() {
        let tree = entry(
            "web/node_modules",
            CacheClass::Deps,
            CarryKey::Unmeasured,
            Remake::Setup {
                steps: vec![step("make", &["deps"])],
                installed: true,
            },
        );
        assert!(matches!(
            decide(&tree, &macos(), &air()),
            CarryDecision::Rebuild { .. }
        ));
    }

    #[test]
    fn every_path_left_behind_is_listed_with_its_reason() {
        use fabric_core::id::{JobId, NodeId};
        use fabric_core::snapshot::{HarnessState, LeftBehind};
        let manifest = SnapshotManifest {
            job_id: JobId::from_bytes([1; 16]),
            root: fabric_core::path::AbsPath::parse("/w").unwrap(),
            parent: None,
            created_at: 0,
            created_by: NodeId::from_bytes([2; 32]),
            platform: macos(),
            git: None,
            files: Vec::new(),
            harness_state: HarnessState::default(),
            env_spec: None,
            caches: vec![dist(), hand_made_tree()],
            left_behind: vec![
                LeftBehind {
                    path: RelPath::parse(".env").unwrap(),
                    reason: LeaveReason::Secret,
                },
                LeftBehind {
                    path: RelPath::parse("scratch/big.bin").unwrap(),
                    reason: LeaveReason::Excluded,
                },
            ],
        };
        let lines: Vec<String> = decide_all(&manifest, &air())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            [
                "leave   .env  (it matches a secret pattern and never leaves the source; set it up on the target by hand)",
                "carry   dist  (same os and arch, macos-aarch64)",
                "carry   node_modules  (no lockfile or setup step makes it again, so it travels as the user's files)",
                "leave   scratch/big.bin  (.fabricignore excludes it; it never leaves the source)",
            ]
        );
    }

    /// A snapshot captured on the Pro holding `caches`.
    fn snapshot_of(caches: Vec<CacheEntry>) -> SnapshotManifest {
        SnapshotManifest {
            job_id: fabric_core::id::JobId::from_bytes([1; 16]),
            root: fabric_core::path::AbsPath::parse("/w").unwrap(),
            parent: None,
            created_at: 0,
            created_by: fabric_core::id::NodeId::from_bytes([2; 32]),
            platform: macos(),
            git: None,
            files: Vec::new(),
            harness_state: fabric_core::snapshot::HarnessState::default(),
            env_spec: None,
            caches,
            left_behind: Vec::new(),
        }
    }

    /// A restore measures only what the carry decision reads (issue #26):
    /// each decision taken with only those facts is the one every fact
    /// gives, for rows whose keys match, which reads the key's tools, and
    /// for rows whose keys differ, which reads the programs their setup
    /// steps run, cargo by its own fact. Build output keyed on the platform
    /// needs no process.
    #[test]
    fn a_restore_measures_what_the_carry_decision_reads() {
        use crate::facts::Tool;
        let everything = NodeFacts::from_pairs([
            (name::USER, "me"),
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::NODE, "v22.23.2"),
            (name::NODE_ABI, "127"),
            (name::NPM, "10.9.2"),
            (name::PNPM, "11.23.0"),
            (name::PNPM_STORE, "/Users/x/Library/pnpm/store/v11"),
            (name::YARN, "1.22.22"),
            (name::BUN, "1.3.14"),
            (name::RUSTC, "rustc 1.97.1"),
            (name::CARGO, "cargo 1.97.1 (a1b2c3d4e 2026-09-01)"),
            (name::CARGO_HOME, "/Users/x/.cargo"),
            (name::PYTHON, "Python 3.12.1"),
            (name::PYTHON_HOME, "/opt/py312"),
            (name::UV, "uv 0.9.5"),
            (name::CLAUDE, "2.1.294 (Claude Code)"),
            (name::CLAUDE_LOGIN, "yes"),
            (name::CODEX, "codex-cli 0.155.1"),
            (name::CODEX_LOGIN, "yes"),
        ]);
        let mut caches = Vec::new();
        for (manager, pm_major) in [("npm", "10"), ("pnpm", "11"), ("yarn", "1"), ("bun", "1")] {
            for node_major in ["22", "20"] {
                let mut tree = locked_tree(manager, pm_major, step(manager, &["install"]));
                tree.path = RelPath::parse(&format!("{manager}{node_major}/node_modules")).unwrap();
                if let CarryKey::Node {
                    node_major: major, ..
                } = &mut tree.key
                {
                    node_major.clone_into(major);
                }
                caches.push(tree);
            }
        }
        let python = |version: &str, path: &str, remake: Remake| {
            let key = CarryKey::Python {
                python_version: version.to_owned(),
                python_home: fabric_core::path::AbsPath::parse("/opt/py312").unwrap(),
            };
            entry(path, CacheClass::Deps, key, remake)
        };
        let setup = |program: &str| Remake::Setup {
            steps: vec![step(program, &["sync"])],
            installed: true,
        };
        let vendor = CarryKey::Rust {
            rustc_vv: "rustc 1.96.0".to_owned(),
            cargo_home: fabric_core::path::AbsPath::parse("/Users/x/.cargo").unwrap(),
        };
        caches.extend([
            rust_target(),
            entry("vendor", CacheClass::Deps, vendor, setup("cargo")),
            python("Python 3.12.1", ".venv", Remake::Nothing),
            python("Python 3.11.0", "uv/.venv", setup("uv")),
            python("Python 3.11.0", "pip/.venv", setup("python3")),
            CacheEntry {
                native_files: 2,
                ..hand_made_tree()
            },
            dist(),
        ]);
        let manifest = snapshot_of(caches);
        let probes = probes(&manifest);
        assert_eq!(
            probes,
            Probes::of(&[
                Tool::Node,
                Tool::Npm,
                Tool::Pnpm,
                Tool::Yarn,
                Tool::Bun,
                Tool::Rustc,
                Tool::Cargo,
                Tool::Python,
                Tool::Uv,
            ])
        );
        assert_eq!(
            decide_all(&manifest, &everything.only(probes)),
            decide_all(&manifest, &everything)
        );
        assert_eq!(super::probes(&snapshot_of(vec![dist()])), Probes::NONE);
    }
}
