//! The preflight: compare two nodes' facts as exact strings (ADR 0008).
//!
//! Invariant: every row compares strings for equality and never parses a
//! version into a range, because every coupling the research found was an
//! exact-string one. A row that refuses stops a move unless the user passes
//! `--force`, and some refuse even then: a different user or home, because
//! the root would not exist on the target (ADR 0011), and a target with no
//! login for the job's harness, or for an ACP job no agent or no
//! `state_dir` for it, because the harness would fail inside with a worse
//! message once the target owns the job. The comparison is pure; measuring
//! is `facts`' job.

use std::fmt;
use std::path::Path;

use fabric_core::harness::HarnessKind;

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
            name::CLAUDE,
            name::CLAUDE_LOGIN,
            matches!(ctx.harness, Some(HarnessKind::ClaudeCode)),
        ),
        (
            name::CODEX,
            name::CODEX_LOGIN,
            matches!(ctx.harness, Some(HarnessKind::Codex)),
        ),
    ];
    for (version, login, used) in pairs {
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

/// The rows for an ACP job: the target must find the agent and give it a
/// `state_dir`, or the job would be owned by a node that cannot launch it,
/// restore its session or capture it. Only the target of a pull measures
/// them; a peer does not report its agents, so a push can only warn.
fn acp_rows(program: &str, here: &NodeFacts, peer: &NodeFacts, ctx: &Context<'_>) -> [Row; 2] {
    let missing = [
        (
            name::ACP_AGENT,
            format!("this node's PATH has no {program}, so the job could not launch here"),
        ),
        (
            name::ACP_STATE_DIR,
            format!(
                "harness.toml here gives {program} no state_dir, so its session could not be restored or captured here"
            ),
        ),
    ];
    missing.map(|(fact, absent)| {
        let (outcome, note) = match ctx.target {
            Target::Here if here.get(fact).is_some() => (Outcome::Equal, String::new()),
            Target::Here => (Outcome::Refuse(Waiver::Never), absent),
            Target::Peer => (
                Outcome::Warn,
                format!(
                    "a peer does not report its ACP agents, so whether it has {program} and a state_dir for it is not checked"
                ),
            ),
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
            (name::USER, "me"),
            (name::HOME, "/Users/me"),
            (name::GIT, "git version 2.54.0"),
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
}
