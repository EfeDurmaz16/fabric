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
    /// The move stops unless `--force` turns this into a warning.
    Refuse,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Outcome::Equal => "ok",
            Outcome::Fallback => "rebuild",
            Outcome::Warn => "warn",
            Outcome::Refuse => "refuse",
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
    /// Whether `--force` can turn a refusal into a warning.
    pub forceable: bool,
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
        forceable: true,
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
        let mut r = row(
            fact,
            here,
            peer,
            Outcome::Refuse,
            "the root would differ and nothing rewrites paths in this phase (ADR 0011)",
        );
        r.forceable = false;
        rows.push(r);
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
        forceable: true,
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
            if used { Outcome::Refuse } else { Outcome::Warn },
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
                Outcome::Refuse
            } else {
                Outcome::Equal
            },
            forceable: false,
            note: if refused {
                "the target has no login for the job's harness; log in there first".to_owned()
            } else {
                String::new()
            },
        });
    }
    rows
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
            Outcome::Refuse
        } else {
            Outcome::Equal
        },
        forceable: true,
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
        forceable: true,
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
        .filter(|r| r.outcome == Outcome::Refuse && !(force && r.forceable))
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
                Outcome::Refuse,
            ),
            (
                "home",
                &[(name::HOME, Some("/home/x"))],
                None,
                Outcome::Refuse,
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
                Outcome::Refuse,
            ),
            (
                "claude.login",
                &[(name::CLAUDE_LOGIN, Some("no"))],
                Some(&claude),
                Outcome::Refuse,
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
        let root = Path::new("/Users/efebarandurmaz/Desktop/job");
        let mut context = ctx(None);
        context.root = Some(root);
        assert_eq!(
            outcome_of(&compare(&pro(), &peer, &context), "icloud"),
            Outcome::Refuse
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
