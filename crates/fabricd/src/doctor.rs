//! The preflight: compare two nodes' facts as exact strings (ADR 0008).
//!
//! Invariant: every row compares strings for equality and never parses a
//! version into a range, because every coupling the research found was an
//! exact-string one. A row that refuses stops a move unless the user passes
//! `--force`, and two rows refuse even then: a different user or home,
//! because the root would not exist on the target (ADR 0011), and a target
//! with no login for the job's harness, because the harness would fail
//! inside with a worse message. The comparison is pure; measuring is
//! `facts`' job.
//!
//! One row orders versions, and only to word its note and warn: when the
//! job's harness on the target is older than the source's, or than the
//! newest known to have written the job's session (`Context::floor`, the
//! known writer the guard refuses below), the note says so, read through
//! `fabric_harness::HarnessVersion`, the parse and the order the resume
//! guard uses. Both sides are the versions each daemon finds from its own
//! directory; a version manager's pin in the workspace can pick another,
//! and the rollout's own `cli_version` is not read here, so the note is a
//! warning ahead of the guard, not the guard. The outcome stays the
//! equality refusal that `--force` waives, and a target whose version
//! matches the source's but is older than the known writer only warns: a
//! move only copies bytes, and the guard refuses the older harness at
//! resume, before it can touch the session, whatever the move was told
//! (gate finding F10).

use std::fmt;
use std::path::Path;

use fabric_core::harness::HarnessKind;
use fabric_harness::HarnessVersion;

use crate::facts::{NodeFacts, major, name};
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

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Outcome::Equal => "ok",
            Outcome::Fallback => "rebuild",
            Outcome::Warn => "warn",
            Outcome::Refuse(_) => "refuse",
        })
    }
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

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "absent".to_owned());
        write!(
            f,
            "{:<7} {:<16} here: {}  peer: {}",
            self.outcome,
            self.fact,
            show(&self.here).replace('\n', " / "),
            show(&self.peer).replace('\n', " / ")
        )?;
        if !self.note.is_empty() {
            write!(f, "  ({})", self.note)?;
        }
        Ok(())
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

/// Compares this node's facts with a peer's, row by row (ADR 0008, the table).
#[must_use]
pub fn compare(here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> Vec<Row> {
    let mut rows = Vec::new();
    for fact in [name::USER, name::HOME] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Refuse(Waiver::Never),
            "the root would differ and nothing rewrites paths in this phase (ADR 0011)",
        ));
    }
    rows.push(row(
        name::OS,
        here,
        peer,
        Outcome::Warn,
        "no cache is carried across operating systems",
    ));
    rows.push(row(
        name::ARCH,
        here,
        peer,
        Outcome::Warn,
        "no cache is carried across architectures",
    ));
    for fact in [name::PNPM, name::PNPM_STORE] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Fallback,
            "a pnpm node_modules is not carried; the lockfile install runs (ADR 0012)",
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
        "node_modules is not carried across Node majors; the lockfile install runs (ADR 0012)"
            .clone_into(&mut node.note);
    }
    rows.push(node);
    rows.push(row(
        name::NODE_ABI,
        here,
        peer,
        Outcome::Fallback,
        "native addons built for one ABI do not load under another",
    ));
    for fact in [name::RUSTC, name::CARGO_HOME] {
        rows.push(row(
            fact,
            here,
            peer,
            Outcome::Fallback,
            "target/ is not carried; cargo rebuilds (ADR 0012)",
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
        let mut r = row(
            version,
            here,
            peer,
            if used {
                Outcome::Refuse(Waiver::Force)
            } else {
                Outcome::Warn
            },
            if used {
                "the job's harness must be the same version on both nodes (ADR 0008)"
            } else {
                "not the job's harness"
            },
        );
        if !used && r.outcome == Outcome::Warn && ctx.harness.is_some() {
            r.outcome = Outcome::Equal;
            r.note.clear();
        }
        if used && let Some(note) = older_target(here, peer, version, program, ctx) {
            if r.outcome == Outcome::Equal {
                r.outcome = Outcome::Warn;
            }
            r.note = note;
        }
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
    rows
}

/// The note for a target whose `fact`, read as `--version` output, is
/// older than the newest known to have written the session
/// (`Context::floor`) when that is above the source's, naming it, or else
/// older than the source's; `None` when it is neither, or when the
/// target's does not parse.
fn older_target(
    here: &NodeFacts,
    peer: &NodeFacts,
    fact: &str,
    program: &str,
    ctx: &Context<'_>,
) -> Option<String> {
    let read = |facts: &NodeFacts| {
        facts
            .get(fact)
            .and_then(|text| HarnessVersion::from_output(text).ok())
    };
    let (source, target) = match ctx.target {
        Target::Peer => (read(here), read(peer)?),
        Target::Here => (read(peer), read(here)?),
    };
    let floor = ctx
        .floor
        .filter(|floor| source.as_ref().is_none_or(|source| *floor > source));
    match floor {
        Some(floor) if target < *floor => Some(format!(
            "the target's {program} is older than {program} {floor}, the newest known to have written the session; a resume there refuses it, so update {program} there first (ADR 0006)"
        )),
        _ if source.is_some_and(|source| target < source) => Some(format!(
            "the target's {program} is older; a resume there refuses a session a newer {program} wrote, so update {program} there first (ADR 0006)"
        )),
        _ => None,
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
            (name::USER, "efebarandurmaz"),
            (name::HOME, "/Users/efebarandurmaz"),
            (name::OS, "macos"),
            (name::ARCH, "aarch64"),
            (name::PNPM, "11.23.0"),
            (
                name::PNPM_STORE,
                "/Users/efebarandurmaz/Library/pnpm/store/v11",
            ),
            (name::NODE, "v22.23.2"),
            (name::NODE_ABI, "127"),
            (name::RUSTC, "rustc 1.97.1"),
            (name::CARGO_HOME, "/Users/efebarandurmaz/.cargo"),
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
        assert_eq!(
            pulled.note,
            "the job's harness must be the same version on both nodes (ADR 0008)"
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
        assert_eq!(
            at_floor.note,
            "the job's harness must be the same version on both nodes (ADR 0008)"
        );
    }

    #[test]
    fn a_root_in_a_folder_the_target_syncs_is_refused() {
        let peer = with(&pro(), &[(name::ICLOUD_DESKTOP, Some("1"))]);
        let root = Path::new("/Users/efebarandurmaz/Desktop/job");
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
}
