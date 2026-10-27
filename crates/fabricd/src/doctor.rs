//! The preflight: compare two nodes' facts as exact strings (ADR 0008).
//!
//! Invariant: every row compares strings for equality and never parses a
//! version into a range, because every coupling the research found was an
//! exact-string one, except the version row of the job's harness, which
//! orders versions as the resume guard does. A row that refuses stops a
//! move unless the user passes `--force`, and some refuse even then: a
//! different user or home, because the root would not exist on the target
//! (ADR 0011), and a target with no login for the job's harness, or for an
//! ACP job no agent or no `state_dir` for it, because the harness would
//! fail inside with a worse message once the target owns the job. The
//! comparison is pure; measuring is `facts`' job.
//!
//! The version row of the job's harness, Claude's as Codex's, refuses,
//! and `--force` waives, only a target whose harness is older than the
//! source's, or than the newest known to have written the job's session
//! (`Context::floor`, the known writer the guard refuses below), read
//! through `fabric_core::version::HarnessVersion`, the parse and the order
//! the resume guard uses (issue #28, gate finding F13). A newer target
//! warns, since the guard lets a newer harness resume the session, and two
//! versions that differ where either does not parse refuse as any
//! difference did, because they cannot be ordered. Both sides are the
//! versions each daemon finds from its own directory; a version manager's
//! pin in the workspace can pick another, and the rollout's own
//! `cli_version` is not read here, so the row is a warning ahead of the
//! guard, not the guard. A target whose version matches the source's but
//! is older than the known writer only warns: a move only copies bytes,
//! and the guard refuses the older harness at resume, before it can touch
//! the session, whatever the move was told (gate finding F10).

use std::path::Path;

use fabric_client::api::{Outcome as WireOutcome, PreflightRow};
use fabric_core::env::EnvSpec;
use fabric_core::harness::HarnessKind;
use fabric_core::version::HarnessVersion;
use fabric_env::detect::every_resume;

use crate::carry::command;
use crate::facts::{NodeFacts, Probes, Tool, major, name};
use crate::paths::{SyncedFolders, check_root};

/// The clock difference above which mtime-keyed caches are suspect (ADR 0008).
pub const CLOCK_SKEW_LIMIT_MS: i64 = 5_000;

/// What a row means for a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Both nodes agree, or the fact does not matter for this job.
    Equal,
    /// The move goes ahead and the dependent caches are rebuilt instead of carried.
    Fallback,
    /// The move goes ahead; the user should know.
    Warn,
    /// The move stops, unless the waiver lets `--force` turn this into a warning.
    Refuse(Waiver),
}

/// Whether `--force` can turn a refusal into a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiver {
    /// It can.
    Force,
    /// It cannot: the move would land on a root that does not exist, or
    /// on a node that cannot run the job's harness (ADR 0008).
    Never,
}

/// One line of the preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The fact or facts compared.
    pub fact: &'static str,
    /// This node's value.
    pub here: Option<String>,
    /// The peer's value.
    pub peer: Option<String>,
    /// What it means.
    pub outcome: Outcome,
    /// Why, in one sentence.
    pub note: String,
}

impl Row {
    /// The row as the client API carries it (ADR 0018).
    #[must_use]
    pub fn to_wire(&self) -> PreflightRow {
        PreflightRow {
            fact: self.fact.to_owned(),
            here: self.here.clone(),
            peer: self.peer.clone(),
            outcome: match self.outcome {
                Outcome::Equal => WireOutcome::Equal,
                Outcome::Fallback => WireOutcome::Fallback,
                Outcome::Warn => WireOutcome::Warn,
                Outcome::Refuse(Waiver::Force) => WireOutcome::Refuse,
                Outcome::Refuse(Waiver::Never) => WireOutcome::RefuseAlways,
            },
            note: self.note.clone(),
        }
    }
}

/// Which node a move would land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The peer: a push from here.
    Peer,
    /// This node: a pull from the peer.
    Here,
}

/// What the comparison needs beyond the two fact sets.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    /// The harness of the job being moved, if a job is named.
    pub harness: Option<&'a HarnessKind>,
    /// The job root the target would restore into, if a job is named.
    pub root: Option<&'a Path>,
    /// Which node is the target.
    pub target: Target,
    /// The peer's clock minus this node's, from the session hello.
    pub clock_skew_ms: Option<i64>,
    /// The wire the session runs on, and how QUIC failed if it fell back.
    pub wire: Option<String>,
    /// The newest version of the job's harness known to have written its
    /// session (`SessionRecord::known_writer`), as the job's owner records
    /// it: this node's record when it owns the job, else the owner's,
    /// asked over the wire, since a node the job left keeps the record of
    /// its own tenure and one that never ran it holds none. The resume
    /// guard refuses a harness older than it, and a capture on the source
    /// records at least it.
    pub floor: Option<&'a HarnessVersion>,
}

fn value(facts: &NodeFacts, fact: &str) -> Option<String> {
    facts.get(fact).map(str::to_owned)
}

fn row(
    fact: &'static str,
    here: &NodeFacts,
    peer: &NodeFacts,
    on_differ: Outcome,
    note: &str,
) -> Row {
    let (h, p) = (value(here, fact), value(peer, fact));
    let outcome = if h == p { Outcome::Equal } else { on_differ };
    Row {
        fact,
        here: h,
        peer: p,
        outcome,
        note: if outcome == Outcome::Equal {
            String::new()
        } else {
            note.to_owned()
        },
    }
}

/// What the os and arch rows say: a path something on the target can
/// make again does not cross a platform, and a path nothing can make
/// again does, since leaving it out would lose it (`crate::carry`).
const PLATFORM_NOTE: &str = "only a path nothing on the target can make again crosses; the carry lines say what happens to each path, ADR 0012";

/// The tools a move or resume preflight measures on this node for a job
/// under `harness` (issue #26): every row's, but a login only where it
/// gates the move, which is the target's login for the job's harness. On a
/// push the target is the peer, which measures its own; `fabric doctor`
/// measures them all.
#[must_use]
pub fn probes(harness: &HarnessKind, target: Target) -> Probes {
    let rows = Probes::ALL
        .without(Tool::ClaudeLogin)
        .without(Tool::CodexLogin);
    match (target, harness) {
        (Target::Here, HarnessKind::ClaudeCode) => rows.with(Tool::ClaudeLogin),
        (Target::Here, HarnessKind::Codex) => rows.with(Tool::CodexLogin),
        (Target::Here, HarnessKind::Acp { .. }) | (Target::Peer, _) => rows,
    }
}

/// Compares this node's facts with a peer's, row by row (ADR 0008, the table).
#[must_use]
pub fn compare(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Vec<Row> {
    let mut rows = Vec::new();
    for fact in [name::USER, name::HOME] {
        let mut row = row(
            fact,
            here,
            peer,
            Outcome::Refuse(Waiver::Never),
            "the root would differ and nothing rewrites paths in this phase (ADR 0011)",
        );
        // Two absences compare equal, and prove nothing about the user.
        if row.here.is_none() || row.peer.is_none() {
            row.outcome = Outcome::Refuse(Waiver::Never);
            "not measured, and the move needs it measured and equal on both nodes (ADR 0011)"
                .clone_into(&mut row.note);
        }
        rows.push(row);
    }
    rows.push(git_row(here, peer, ctx));
    rows.push(lsof_row(here, peer, ctx));
    rows.push(row(name::OS, here, peer, Outcome::Warn, PLATFORM_NOTE));
    rows.push(row(name::ARCH, here, peer, Outcome::Warn, PLATFORM_NOTE));
    for fact in [name::PNPM, name::PNPM_STORE] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Fallback,
            "a pnpm node_modules is rebuilt only where the same pnpm major runs; the carry lines say what happens to each path, ADR 0012",
        ));
    }
    let mut node = row(
        name::NODE,
        here,
        peer,
        Outcome::Warn,
        "same major, other release",
    );
    let node_majors = (
        here.get(name::NODE).and_then(major),
        peer.get(name::NODE).and_then(major),
    );
    if node.outcome != Outcome::Equal && node_majors.0 != node_majors.1 {
        node.outcome = Outcome::Fallback;
        "a node_modules with a lockfile is rebuilt, one without travels as it is; the carry lines say which, ADR 0012"
            .clone_into(&mut node.note);
    }
    rows.push(node);
    rows.push(row(
        name::NODE_ABI,
        here,
        peer,
        Outcome::Fallback,
        "native addons built for one ABI do not load under another; the carry lines name a tree that holds one",
    ));
    for fact in [name::RUSTC, name::CARGO_HOME] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Fallback,
            "target/ is left for the next cargo build to make again (ADR 0012)",
        ));
    }
    for fact in [name::MACOS, name::MACOS_SDK] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Warn,
            "native build output is keyed on rustc and Node already",
        ));
    }
    rows.extend(harness_rows(here, peer, ctx));
    rows.push(synced_row(here, peer, ctx));
    rows.push(clock_row(ctx));
    rows.push(Row {
        fact: "network",
        here: Some("dialed".to_owned()),
        peer: ctx.wire.clone(),
        outcome: Outcome::Equal,
        note: "the wire this session runs on".to_owned(),
    });
    rows
}

fn harness_rows(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Vec<Row> {
    let mut rows = Vec::new();
    let pairs = [
        (
            "claude",
            name::CLAUDE,
            name::CLAUDE_LOGIN,
            matches!(ctx.harness, Some(HarnessKind::ClaudeCode)),
        ),
        (
            "codex",
            name::CODEX,
            name::CODEX_LOGIN,
            matches!(ctx.harness, Some(HarnessKind::Codex)),
        ),
    ];
    for (program, version, login, used) in pairs {
        let r = if used {
            version_row(version, program, here, peer, ctx)
        } else {
            let mut r = row(version, here, peer, Outcome::Warn, "not the job's harness");
            if r.outcome == Outcome::Warn && ctx.harness.is_some() {
                r.outcome = Outcome::Equal;
                r.note.clear();
            }
            r
        };
        rows.push(r);
        let target_login = match ctx.target {
            Target::Peer => peer.get(login),
            Target::Here => here.get(login),
        };
        let refused = used && target_login != Some("yes");
        rows.push(Row {
            fact: login,
            here: value(here, login),
            peer: value(peer, login),
            outcome: if refused {
                Outcome::Refuse(Waiver::Never)
            } else {
                Outcome::Equal
            },
            note: if refused {
                "the target has no login for the job's harness; log in there first".to_owned()
            } else {
                String::new()
            },
        });
    }
    if let Some(HarnessKind::Acp { program, .. }) = ctx.harness {
        rows.extend(acp_rows(program.as_str(), here, peer, ctx));
    }
    rows
}

/// The row of `fact`, the `--version` output of `program`, the job's
/// harness, ordered as the resume guard orders versions (ADR 0006,
/// issue #28 F13). It refuses, and `--force` waives, a target older than
/// the source, or older than the newest version known to have written the
/// session (`Context::floor`) when that is above the source's, naming it.
/// It warns for a newer target, which the guard lets resume the session,
/// and for a target on the source's own version below that floor, which
/// the guard refuses at resume (F10). Two texts that differ and do not
/// both parse cannot be ordered, so they refuse as any difference did.
fn version_row(
    fact: &'static str,
    program: &str,
    here: &NodeFacts,
    peer: &NodeFacts,
    ctx: &Context<'_>,
) -> Row {
    let read = |facts: &NodeFacts| {
        facts
            .get(fact)
            .and_then(|text| HarnessVersion::from_output(text).ok())
    };
    let (source, target) = match ctx.target {
        Target::Peer => (read(here), read(peer)),
        Target::Here => (read(peer), read(here)),
    };
    let same = here.get(fact) == peer.get(fact);
    let floor = ctx
        .floor
        .filter(|floor| source.as_ref().is_none_or(|source| *floor > source));
    let below_floor = target
        .as_ref()
        .and_then(|target| floor.filter(|floor| target < *floor));
    let (outcome, note) = if let Some(floor) = below_floor {
        (
            if same {
                Outcome::Warn
            } else {
                Outcome::Refuse(Waiver::Force)
            },
            format!(
                "the target's {program} is older than {program} {floor}, the newest known to have written the session; a resume there refuses it, so update {program} there first (ADR 0006)"
            ),
        )
    } else if same {
        (Outcome::Equal, String::new())
    } else if let (Some(source), Some(target)) = (source, target) {
        match target.cmp(&source) {
            std::cmp::Ordering::Less => (
                Outcome::Refuse(Waiver::Force),
                format!(
                    "the target's {program} is older; a resume there refuses a session a newer {program} wrote, so update {program} there first (ADR 0006)"
                ),
            ),
            std::cmp::Ordering::Equal => (Outcome::Equal, String::new()),
            std::cmp::Ordering::Greater => (
                Outcome::Warn,
                format!(
                    "the target's {program} is newer, which a resume there accepts; once it writes the session, a node with an older {program} refuses it until updated (ADR 0006)"
                ),
            ),
        }
    } else {
        (
            Outcome::Refuse(Waiver::Force),
            format!(
                "a side reports no {program} version the resume guard reads, so the row cannot tell which is older and wants the same version on both nodes (ADR 0008)"
            ),
        )
    };
    Row {
        fact,
        here: value(here, fact),
        peer: value(peer, fact),
        outcome,
        note,
    }
}

/// The rows for an ACP job: the target must find the agent and give it a
/// `state_dir`, or the job would be owned by a node that cannot launch it,
/// restore its session or capture it. The target measures them: this
/// node for a pull, and the peer for a push, whose question for its facts
/// names the job's harness (issue #16). A fact the target did not report
/// refuses as one it does not have.
fn acp_rows(program: &str, here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> [Row; 2] {
    let (target, whose, there) = match ctx.target {
        Target::Peer => (peer, "the target's", "there"),
        Target::Here => (here, "this node's", "here"),
    };
    let missing = [
        (
            name::ACP_AGENT,
            format!("{whose} PATH has no {program}, so the job could not launch {there}"),
        ),
        (
            name::ACP_STATE_DIR,
            format!(
                "harness.toml {there} gives {program} no state_dir capture and restore accept (none, one outside the home or under ~/.fabric, or one reached through a link), so its session could not be restored or captured {there}"
            ),
        ),
    ];
    missing.map(|(fact, absent)| {
        let (outcome, note) = if target.get(fact).is_some() {
            (Outcome::Equal, String::new())
        } else {
            (Outcome::Refuse(Waiver::Never), absent)
        };
        Row {
            fact,
            here: value(here, fact),
            peer: value(peer, fact),
            outcome,
            note,
        }
    })
}

/// Every job's workspace is a repository and restore runs git, so a job
/// moves only to a node whose daemon finds git; the versions may differ.
fn git_row(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Row {
    let target = match ctx.target {
        Target::Peer => peer,
        Target::Here => here,
    };
    let missing = target.get(name::GIT).is_none();
    let (outcome, note) = match (missing, ctx.harness.is_some()) {
        (false, _) => (Outcome::Equal, ""),
        (true, true) => (
            Outcome::Refuse(Waiver::Never),
            "the target's daemon finds no git, and restore runs it for every job",
        ),
        (true, false) => (
            Outcome::Warn,
            "the target's daemon finds no git, so no job can move there",
        ),
    };
    Row {
        fact: name::GIT,
        here: value(here, name::GIT),
        peer: value(peer, name::GIT),
        outcome,
        note: note.to_owned(),
    }
}

/// Every suspend or move of a Codex job asks `lsof` whether a Codex
/// process holds the thread's rollout open, and fails on a node that has
/// none, so a Codex job moves only to a node where it is found.
fn lsof_row(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Row {
    let target = match ctx.target {
        Target::Peer => peer,
        Target::Here => here,
    };
    let missing = target.get(name::LSOF).is_none();
    let (outcome, note) = match (missing, ctx.harness) {
        (true, Some(HarnessKind::Codex)) => (
            Outcome::Refuse(Waiver::Never),
            "the target has no lsof, which every suspend or move of a Codex job runs, so the job could leave it only by a steal",
        ),
        (true, None) => (
            Outcome::Warn,
            "the target has no lsof, so a Codex job could not be suspended or moved there",
        ),
        (false, _) | (true, Some(_)) => (Outcome::Equal, ""),
    };
    Row {
        fact: name::LSOF,
        here: value(here, name::LSOF),
        peer: value(peer, name::LSOF),
        outcome,
        note: note.to_owned(),
    }
}

fn synced_row(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Row {
    let target = match ctx.target {
        Target::Peer => peer,
        Target::Here => here,
    };
    let synced = SyncedFolders {
        desktop: target.syncs(name::ICLOUD_DESKTOP),
        documents: target.syncs(name::ICLOUD_DOCUMENTS),
    };
    let show = |f: &NodeFacts| {
        Some(format!(
            "desktop={} documents={}",
            f.syncs(name::ICLOUD_DESKTOP),
            f.syncs(name::ICLOUD_DOCUMENTS)
        ))
    };
    let refusal = match (ctx.root, target.get(name::HOME)) {
        (Some(root), Some(home)) => check_root(root, Path::new(home), synced).err(),
        _ => None,
    };
    Row {
        fact: "icloud",
        here: show(here),
        peer: show(peer),
        outcome: if refusal.is_some() {
            Outcome::Refuse(Waiver::Force)
        } else {
            Outcome::Equal
        },
        note: refusal.map(|e| e.to_string()).unwrap_or_default(),
    }
}

fn clock_row(ctx: &Context<'_>) -> Row {
    let over = ctx
        .clock_skew_ms
        .is_some_and(|skew| skew.abs() > CLOCK_SKEW_LIMIT_MS);
    Row {
        fact: "clock",
        here: Some("the reference".to_owned()),
        peer: ctx.clock_skew_ms.map(|ms| format!("{ms} ms from here")),
        outcome: if over { Outcome::Warn } else { Outcome::Equal },
        note: if over {
            "above 5 s; Cargo and other mtime-keyed caches may rebuild".to_owned()
        } else {
            String::new()
        },
    }
}

/// The rows for the setup steps of the job's spec that run on every
/// resume (`fabric_env::detect::every_resume`), one per program this node
/// measures.
///
/// Materialize on the target runs those steps whatever the carry decision
/// did with each cache, and refuses before anything runs when a program
/// is missing, so a target without one would own a job it cannot resume.
/// The row refuses that move, for a push and a pull alike, and `--force`
/// waives it: the user may install the program there before resuming. A
/// program no node measures, such as a declared `make`, gets no row;
/// resume names it when it is missing (ADR 0007). A step that runs only
/// when its cache is not whole is the carry decision's, per path
/// (`crate::carry`).
#[must_use]
pub fn setup_rows(spec: &EnvSpec, here: &NodeFacts, peer: &NodeFacts, target: Target) -> Vec<Row> {
    let on_target = match target {
        Target::Peer => peer,
        Target::Here => here,
    };
    let mut rows: Vec<Row> = Vec::new();
    for step in every_resume(spec) {
        let program = step.program.as_str();
        let Some(fact) = NodeFacts::program_fact(program) else {
            continue;
        };
        if rows.iter().any(|row| row.fact == fact) {
            continue;
        }
        let runs = format!("{} runs on every resume", command(step));
        let (outcome, note) = if on_target.get(fact).is_some() {
            (Outcome::Equal, runs)
        } else {
            (
                Outcome::Refuse(Waiver::Force),
                format!(
                    "{runs}, and the target has no {program}; install it there, or pass --force and install it before resuming"
                ),
            )
        };
        rows.push(Row {
            fact,
            here: value(here, fact),
            peer: value(peer, fact),
            outcome,
            note,
        });
    }
    rows
}

/// The rows that stop a move: refusals, minus those `--force` waives.
#[must_use]
pub fn blocking(rows: &[Row], force: bool) -> Vec<&Row> {
    rows.iter()
        .filter(|r| match r.outcome {
            Outcome::Refuse(Waiver::Never) => true,
            Outcome::Refuse(Waiver::Force) => !force,
            Outcome::Equal | Outcome::Fallback | Outcome::Warn => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pro() -> NodeFacts {
        NodeFacts::from_pairs([
            (name::USER, "me"),
            (name::HOME, "/Users/me"),
            (name::GIT, "git version 2.54.0"),
            (name::LSOF, "/usr/sbin/lsof"),
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PNPM, "11.23.0"),
            (name::PNPM_STORE, "/Users/me/Library/pnpm/store/v11"),
            (name::NODE, "v22.23.2"),
            (name::NODE_ABI, "127"),
            (name::RUSTC, "rustc 1.97.1"),
            (name::CARGO_HOME, "/Users/me/.cargo"),
            (name::MACOS, "26.6.2"),
            (name::MACOS_SDK, "27.0"),
            (name::CLAUDE, "2.1.294 (Claude Code)"),
            (name::CLAUDE_LOGIN, "yes"),
            (name::CODEX, "codex-cli 0.155.1"),
            (name::CODEX_LOGIN, "yes"),
            (name::ICLOUD_DESKTOP, "0"),
        ])
    }

    fn with(base: &NodeFacts, changes: &[(&str, Option<&str>)]) -> NodeFacts {
        let mut pairs: BTreeMapPairs = base
            .iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        for (k, v) in changes {
            match v {
                Some(v) => {
                    pairs.insert((*k).to_owned(), (*v).to_owned());
                }
                None => {
                    pairs.remove(*k);
                }
            }
        }
        NodeFacts::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    type BTreeMapPairs = std::collections::BTreeMap<String, String>;

    /// The fact that should move, the changes on the peer, the job's harness, the outcome.
    type Case<'a> = (
        &'a str,
        &'a [(&'a str, Option<&'a str>)],
        Option<&'a HarnessKind>,
        Outcome,
    );

    fn outcome_of(rows: &[Row], fact: &str) -> Outcome {
        rows.iter()
            .find(|r| r.fact == fact)
            .map(|r| r.outcome)
            .unwrap()
    }

    fn ctx(harness: Option<&HarnessKind>) -> Context<'_> {
        Context {
            harness,
            root: None,
            target: Target::Peer,
            clock_skew_ms: Some(9),
            wire: Some("quic".to_owned()),
            floor: None,
        }
    }

    /// Each row of the ADR 0008 table, one case per row: the fact that
    /// differs, the job's harness, and the outcome the table names.
    #[test]
    fn each_row_gives_the_outcome_the_table_names() {
        let claude = HarnessKind::ClaudeCode;
        let cases: [Case<'_>; 12] = [
            (
                "user",
                &[(name::USER, Some("other"))],
                None,
                Outcome::Refuse(Waiver::Never),
            ),
            (
                "home",
                &[(name::HOME, Some("/home/x"))],
                None,
                Outcome::Refuse(Waiver::Never),
            ),
            (
                "pnpm.version",
                &[(name::PNPM, None)],
                None,
                Outcome::Fallback,
            ),
            (
                "pnpm.store",
                &[(name::PNPM_STORE, Some("/s/v10"))],
                None,
                Outcome::Fallback,
            ),
            (
                "node.version",
                &[(name::NODE, Some("v26.0.0"))],
                None,
                Outcome::Fallback,
            ),
            (
                "node.version",
                &[(name::NODE, Some("v22.1.0"))],
                None,
                Outcome::Warn,
            ),
            (
                "node.abi",
                &[(name::NODE_ABI, Some("147"))],
                None,
                Outcome::Fallback,
            ),
            (
                "rustc",
                &[(name::RUSTC, Some("rustc 1.96.0"))],
                None,
                Outcome::Fallback,
            ),
            (
                "macos.version",
                &[(name::MACOS, Some("26.5.2"))],
                None,
                Outcome::Warn,
            ),
            (
                "claude.version",
                &[(name::CLAUDE, Some("2.1.199"))],
                Some(&claude),
                Outcome::Refuse(Waiver::Force),
            ),
            (
                "claude.login",
                &[(name::CLAUDE_LOGIN, Some("no"))],
                Some(&claude),
                Outcome::Refuse(Waiver::Never),
            ),
            (
                "codex.version",
                &[(name::CODEX, Some("codex-cli 0.142.4"))],
                Some(&claude),
                Outcome::Equal,
            ),
        ];
        for (fact, changes, harness, expected) in cases {
            let peer = with(&pro(), changes);
            let rows = compare(&pro(), &peer, &ctx(harness));
            assert_eq!(outcome_of(&rows, fact), expected, "{fact} with {changes:?}");
            let others: Vec<_> = rows
                .iter()
                .filter(|r| r.fact != fact && r.outcome != Outcome::Equal)
                .collect();
            assert!(
                others.is_empty(),
                "{fact}: only that row moves, got {others:?}"
            );
        }
    }

    #[test]
    fn force_waives_every_refusal_but_the_home_and_the_login() {
        let claude = HarnessKind::ClaudeCode;
        let peer = with(
            &pro(),
            &[
                (name::HOME, Some("/home/x")),
                (name::CLAUDE, Some("2.1.199")),
                (name::CLAUDE_LOGIN, Some("no")),
            ],
        );
        let rows = compare(&pro(), &peer, &ctx(Some(&claude)));
        let facts: Vec<&str> = blocking(&rows, true).iter().map(|r| r.fact).collect();
        assert_eq!(facts, vec![name::HOME, name::CLAUDE_LOGIN]);
        assert_eq!(blocking(&rows, false).len(), 3);
    }

    /// The second gate run moved a Codex job with `--force` onto a node
    /// with an older Codex: the row names the older side, in the order the
    /// resume guard uses, for a push and for a pull onto this node alike,
    /// and `--force` still waives it, because the guard refuses the older
    /// Codex at resume.
    #[test]
    fn an_older_harness_on_the_target_is_named_and_force_still_waives_it() {
        let codex = HarnessKind::Codex;
        let older = with(&pro(), &[(name::CODEX, Some("codex-cli 0.142.4"))]);
        let row_of = |rows: Vec<Row>| rows.into_iter().find(|r| r.fact == name::CODEX).unwrap();
        let pushed = row_of(compare(&pro(), &older, &ctx(Some(&codex))));
        assert_eq!(pushed.outcome, Outcome::Refuse(Waiver::Force));
        assert!(
            pushed.note.starts_with("the target's codex is older"),
            "{}",
            pushed.note
        );
        let mut pull = ctx(Some(&codex));
        pull.target = Target::Here;
        let pulled = row_of(compare(&pro(), &older, &pull));
        assert_eq!(pulled.outcome, Outcome::Warn);
        assert!(
            pulled.note.starts_with("the target's codex is newer"),
            "{}",
            pulled.note
        );
        let pulled_onto_older = row_of(compare(&older, &pro(), &pull));
        assert_eq!(pulled_onto_older.outcome, Outcome::Refuse(Waiver::Force));
        assert!(
            pulled_onto_older
                .note
                .starts_with("the target's codex is older"),
            "{}",
            pulled_onto_older.note
        );
    }

    /// The note orders versions as the resume guard does, not as text:
    /// `0.155.9` is older than `0.155.10`, and a release is newer than its
    /// own pre-release, where a string compare says the opposite of both.
    #[test]
    fn the_older_target_is_found_by_the_guards_order_not_by_text() {
        let codex = HarnessKind::Codex;
        let older = "the target's codex is older";
        let note = |here: &str, target: &str| {
            let source = with(&pro(), &[(name::CODEX, Some(here))]);
            let peer = with(&pro(), &[(name::CODEX, Some(target))]);
            compare(&source, &peer, &ctx(Some(&codex)))
                .into_iter()
                .find(|r| r.fact == name::CODEX)
                .unwrap()
                .note
        };
        let n = note("codex-cli 0.155.10", "codex-cli 0.155.9");
        assert!(n.starts_with(older), "{n}");
        let n = note("codex-cli 0.163.0-alpha.10.2", "codex-cli 0.163.0");
        assert!(!n.starts_with(older), "{n}");
        let n = note("codex-cli 0.163.0", "codex-cli 0.163.0-alpha.10.2");
        assert!(n.starts_with(older), "{n}");
    }

    /// The session's floor is the newest Codex known to have written it,
    /// which a version manager's pin in the workspace can put above what
    /// the source's daemon finds: here 0.160.0 ran the thread while the
    /// source's daemon finds 0.155.1. A target on 0.158.0 looks newer than
    /// the source, yet the guard there refuses it, so the note names the
    /// floor; a target on the source's own version is not refused by the
    /// row and only warns; a target at the floor gets no such note.
    #[test]
    fn the_older_target_is_named_against_the_newest_known_writer() {
        let codex = HarnessKind::Codex;
        let floor = HarnessVersion::parse("0.160.0").unwrap();
        let older_than_floor = "the target's codex is older than codex 0.160.0, the newest known to have written the session";
        let row = |target: &str| {
            let peer = with(&pro(), &[(name::CODEX, Some(target))]);
            let mut context = ctx(Some(&codex));
            context.floor = Some(&floor);
            compare(&pro(), &peer, &context)
                .into_iter()
                .find(|r| r.fact == name::CODEX)
                .unwrap()
        };
        let between = row("codex-cli 0.158.0");
        assert_eq!(between.outcome, Outcome::Refuse(Waiver::Force));
        assert!(
            between.note.starts_with(older_than_floor),
            "{}",
            between.note
        );
        let same = row("codex-cli 0.155.1");
        assert_eq!(same.outcome, Outcome::Warn);
        assert!(same.note.starts_with(older_than_floor), "{}", same.note);
        let at_floor = row("codex-cli 0.160.0");
        assert_eq!(at_floor.outcome, Outcome::Warn);
        assert!(
            at_floor.note.starts_with("the target's codex is newer"),
            "{}",
            at_floor.note
        );
    }

    /// Gate finding F13 (issue #28): the Claude row refused a target with
    /// a newer Claude, which the resume guard lets resume the session,
    /// while the two Macs of the gate ran different releases. The job's
    /// harness row refuses only an older target, ordered as the guard
    /// orders versions, for Claude as for Codex and for a push as for a
    /// pull, and a newer target only warns, so no `--force` is needed.
    /// Versions that differ and cannot be ordered still refuse.
    #[test]
    fn only_an_older_harness_on_the_target_is_refused() {
        let cases = [
            (
                HarnessKind::ClaudeCode,
                name::CLAUDE,
                "2.1.294 (Claude Code)",
                "2.1.300 (Claude Code)",
            ),
            (
                HarnessKind::Codex,
                name::CODEX,
                "codex-cli 0.155.1",
                "codex-cli 0.160.0",
            ),
        ];
        for (harness, fact, older, newer) in &cases {
            let older = with(&pro(), &[(fact, Some(older))]);
            let newer = with(&pro(), &[(fact, Some(newer))]);
            let unreadable = with(&pro(), &[(fact, Some("a build of today"))]);
            for target in [Target::Peer, Target::Here] {
                let context = Context {
                    target,
                    ..ctx(Some(harness))
                };
                // `onto` is the target of the move, `from` the node it leaves.
                let compared = |onto: &NodeFacts, from: &NodeFacts| {
                    let rows = match target {
                        Target::Peer => compare(from, onto, &context),
                        Target::Here => compare(onto, from, &context),
                    };
                    let row = rows.iter().find(|r| r.fact == *fact).unwrap().clone();
                    (row.outcome, blocking(&rows, false).is_empty())
                };
                assert_eq!(
                    compared(&newer, &older),
                    (Outcome::Warn, true),
                    "{harness} {target:?} onto a newer one"
                );
                assert_eq!(
                    compared(&older, &newer),
                    (Outcome::Refuse(Waiver::Force), false),
                    "{harness} {target:?} onto an older one"
                );
                assert_eq!(
                    compared(&newer, &newer),
                    (Outcome::Equal, true),
                    "{harness} {target:?} onto the same one"
                );
                assert_eq!(
                    compared(&unreadable, &older),
                    (Outcome::Refuse(Waiver::Force), false),
                    "{harness} {target:?} onto one that does not parse"
                );
            }
        }
    }

    #[test]
    fn a_root_in_a_folder_the_target_syncs_is_refused() {
        let peer = with(&pro(), &[(name::ICLOUD_DESKTOP, Some("1"))]);
        let root = Path::new("/Users/me/Desktop/job");
        let mut context = ctx(None);
        context.root = Some(root);
        assert_eq!(
            outcome_of(&compare(&pro(), &peer, &context), "icloud"),
            Outcome::Refuse(Waiver::Force)
        );
        context.target = Target::Here;
        assert_eq!(
            outcome_of(&compare(&pro(), &peer, &context), "icloud"),
            Outcome::Equal
        );
    }

    #[test]
    fn a_clock_off_by_more_than_five_seconds_warns() {
        let mut context = ctx(None);
        context.clock_skew_ms = Some(-5_001);
        assert_eq!(
            outcome_of(&compare(&pro(), &pro(), &context), "clock"),
            Outcome::Warn
        );
    }

    /// The user and the home must be measured and equal on both nodes. When
    /// neither node could read its user name, the two absences are equal,
    /// and two users with the same `HOME` string must still be refused.
    #[test]
    fn an_unmeasured_user_or_home_is_refused_even_on_both_sides() {
        for fact in [name::USER, name::HOME] {
            let here = with(&pro(), &[(fact, None)]);
            for peer in [with(&pro(), &[(fact, None)]), pro()] {
                let rows = compare(&here, &peer, &ctx(None));
                assert_eq!(
                    outcome_of(&rows, fact),
                    Outcome::Refuse(Waiver::Never),
                    "{fact}"
                );
                assert!(!blocking(&rows, true).is_empty(), "{fact}");
            }
        }
    }

    /// Every suspend or move of a Codex job asks `lsof` whether Codex holds
    /// the thread's rollout open, and fails on a node that has none. So a
    /// Codex job is refused a target without it, with no waiver: it could
    /// leave that node only by a steal.
    #[test]
    fn a_target_without_lsof_is_refused_a_codex_job() {
        let codex = HarnessKind::Codex;
        let claude = HarnessKind::ClaudeCode;
        let without = with(&pro(), &[(name::LSOF, None)]);
        let mut pull = ctx(Some(&codex));
        pull.target = Target::Here;
        for (here, peer, context, refused) in [
            (&pro(), &without, ctx(Some(&codex)), true),
            (&without, &pro(), pull.clone(), true),
            (&without, &pro(), ctx(Some(&codex)), false),
            (&pro(), &without, ctx(Some(&claude)), false),
        ] {
            let rows = compare(here, peer, &context);
            assert_eq!(
                outcome_of(&rows, name::LSOF) == Outcome::Refuse(Waiver::Never),
                refused,
                "{:?} {:?}",
                context.target,
                context.harness
            );
        }
    }

    /// Every job's workspace is a repository, and restore runs git on the
    /// target. A target whose daemon finds no git would take the job and
    /// fail to restore it, so a move to it is refused, with no waiver.
    #[test]
    fn a_target_without_git_is_refused() {
        let codex = HarnessKind::Codex;
        let without = with(&pro(), &[(name::GIT, None)]);
        let mut pull = ctx(Some(&codex));
        pull.target = Target::Here;
        for (here, peer, context, refused) in [
            (&pro(), &without, ctx(Some(&codex)), true),
            (&without, &pro(), pull.clone(), true),
            (&without, &pro(), ctx(Some(&codex)), false),
            (&pro(), &without, ctx(None), false),
        ] {
            let rows = compare(here, peer, &context);
            assert_eq!(
                outcome_of(&rows, name::GIT) == Outcome::Refuse(Waiver::Never),
                refused,
                "{:?}",
                context.target
            );
        }
    }

    /// The os and arch rows used to say no cache crosses, while the carry
    /// decision carries a tree nothing on the target can make again to
    /// any platform. The rows now say what the decision does.
    #[test]
    fn the_os_and_arch_rows_say_what_the_carry_decision_does() {
        use crate::carry::{CarryDecision, decide};
        use fabric_core::snapshot::{CacheClass, CacheEntry, CarryKey, CarryState, Remake};
        let tree = CacheEntry {
            path: fabric_core::path::RelPath::parse("node_modules").unwrap(),
            class: CacheClass::Deps,
            carry: CarryState::Carried,
            key: CarryKey::Unmeasured,
            remake: Remake::Nothing,
            native_files: 0,
        };
        for (fact, change) in [(name::OS, "linux"), (name::ARCH, "x86_64")] {
            let peer = with(&pro(), &[(fact, Some(change))]);
            let rows = compare(&pro(), &peer, &ctx(None));
            let row = rows.iter().find(|r| r.fact == fact).unwrap();
            assert_eq!(row.note, PLATFORM_NOTE, "{fact}");
            assert!(
                matches!(
                    decide(&tree, &pro().platform(), &peer),
                    CarryDecision::Carry { .. }
                ),
                "{fact}"
            );
        }
    }

    /// Issue #16: a push judges the target, the peer, by the ACP facts it
    /// measured for the job's agent, as a pull judges this node, and
    /// `--force` waives neither. A fact the peer did not report, as one
    /// that kept the default `facts_for` leaves out, refuses.
    #[test]
    fn a_push_refuses_a_target_without_the_acp_agent_or_its_state_dir() {
        let acp = HarnessKind::Acp {
            program: fabric_core::harness::ProgramName::parse("agent").unwrap(),
            args: Vec::new(),
        };
        let agent = [
            (name::ACP_AGENT, Some("/usr/local/bin/agent")),
            (name::ACP_STATE_DIR, Some(".agent")),
        ];
        let has = with(&pro(), &agent);
        for target in [Target::Peer, Target::Here] {
            let context = Context {
                target,
                ..ctx(Some(&acp))
            };
            // `onto` is the target of the move, `from` the node it leaves.
            let outcomes = |onto: &NodeFacts, from: &NodeFacts| -> Vec<(&str, Outcome)> {
                let rows = match target {
                    Target::Peer => compare(from, onto, &context),
                    Target::Here => compare(onto, from, &context),
                };
                rows.iter()
                    .filter(|row| row.fact.starts_with("acp."))
                    .map(|row| (row.fact, row.outcome))
                    .collect()
            };
            assert_eq!(
                outcomes(&has, &pro()),
                [
                    (name::ACP_AGENT, Outcome::Equal),
                    (name::ACP_STATE_DIR, Outcome::Equal)
                ],
                "{target:?}"
            );
            assert_eq!(
                outcomes(&pro(), &has),
                [
                    (name::ACP_AGENT, Outcome::Refuse(Waiver::Never)),
                    (name::ACP_STATE_DIR, Outcome::Refuse(Waiver::Never))
                ],
                "{target:?}"
            );
            let no_dir = with(&has, &[(name::ACP_STATE_DIR, None)]);
            assert_eq!(
                outcomes(&no_dir, &has)[1],
                (name::ACP_STATE_DIR, Outcome::Refuse(Waiver::Never)),
                "{target:?}"
            );
        }
    }

    /// A preflight measures here every fact its rows read but a login,
    /// which it measures only where the login gates the move: on this
    /// node as the target, for the job's harness (issue #26). Every row
    /// then has the outcome it has with every fact measured, a target's
    /// missing login included.
    #[test]
    fn a_preflight_measures_a_login_only_where_it_gates_the_move() {
        use crate::facts::Tool;
        let acp = HarnessKind::Acp {
            program: fabric_core::harness::ProgramName::parse("agent").unwrap(),
            args: Vec::new(),
        };
        let logins = [
            (
                HarnessKind::ClaudeCode,
                Tool::ClaudeLogin,
                name::CLAUDE_LOGIN,
            ),
            (HarnessKind::Codex, Tool::CodexLogin, name::CODEX_LOGIN),
            (acp, Tool::ClaudeLogin, name::CLAUDE_LOGIN),
        ];
        for (harness, login, fact) in &logins {
            for target in [Target::Peer, Target::Here] {
                let probes = probes(harness, target);
                let measured: Vec<Tool> = [Tool::ClaudeLogin, Tool::CodexLogin]
                    .into_iter()
                    .filter(|tool| probes.contains(*tool))
                    .collect();
                let gates = target == Target::Here && !matches!(harness, HarnessKind::Acp { .. });
                assert_eq!(
                    measured,
                    if gates { vec![*login] } else { vec![] },
                    "{harness} {target:?}"
                );
                for here_login in ["yes", "no"] {
                    let here = with(&pro(), &[(fact, Some(here_login))]);
                    let ctx = Context {
                        target,
                        ..ctx(Some(harness))
                    };
                    let outcomes = |here: &NodeFacts| -> Vec<(&str, Outcome)> {
                        compare(here, &pro(), &ctx)
                            .iter()
                            .map(|row| (row.fact, row.outcome))
                            .collect()
                    };
                    assert_eq!(
                        outcomes(&here.only(probes)),
                        outcomes(&here),
                        "{harness} {target:?} {here_login}"
                    );
                }
            }
        }
    }

    /// A pull checks the programs of the steps its job runs on every
    /// resume with the facts its preflight measured here, so those facts
    /// must hold every program a setup step can name, cargo by its own
    /// fact. A program left out would read as missing and refuse a pull
    /// to a node that has it (issue #26).
    #[test]
    fn a_pull_preflight_measures_every_program_a_setup_row_reads() {
        use fabric_core::env::{RunWhen, SetupStep};
        use fabric_core::harness::ProgramName;
        let programs = [
            "node", "npm", "pnpm", "yarn", "bun", "rustc", "cargo", "python3", "uv",
        ];
        let spec = EnvSpec {
            tools: Vec::new(),
            setup: programs
                .iter()
                .map(|program| SetupStep {
                    program: ProgramName::parse(program).unwrap(),
                    args: Vec::new(),
                    cwd: None,
                    when: RunWhen::Always,
                })
                .collect(),
            env_var_names: Vec::new(),
            cache_paths: Vec::new(),
            secret_patterns: Vec::new(),
        };
        let here = with(
            &pro(),
            &[
                (name::NPM, Some("10.9.2")),
                (name::YARN, Some("1.22.22")),
                (name::BUN, Some("1.3.14")),
                (name::CARGO, Some("cargo 1.97.1 (a1b2c3d4e 2026-09-01)")),
                (name::PYTHON, Some("Python 3.12.1")),
                (name::UV, Some("uv 0.9.5")),
            ],
        );
        let outcomes = |here: &NodeFacts| -> Vec<(&str, Outcome)> {
            setup_rows(&spec, here, &NodeFacts::default(), Target::Here)
                .iter()
                .map(|row| (row.fact, row.outcome))
                .collect()
        };
        let all = outcomes(&here);
        assert_eq!(all.len(), programs.len(), "{all:?}");
        assert!(
            all.iter().all(|(_, outcome)| *outcome == Outcome::Equal),
            "{all:?}"
        );
        for harness in [HarnessKind::ClaudeCode, HarnessKind::Codex] {
            let probes = probes(&harness, Target::Here);
            assert_eq!(outcomes(&here.only(probes)), all, "{harness}");
        }
    }
}
