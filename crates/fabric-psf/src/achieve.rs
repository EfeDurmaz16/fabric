//! What a reader can achieve on a target (section 6).

use crate::manifest::{Fidelity, Manifest};

/// The format Fabric's harness adapters read natively.
pub const HARNESS_STATE_FORMAT: &str = "fabric.harness_state.v1";

/// Where a document would be continued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The workspace root on the target.
    pub root: String,
    /// The home directory on the target.
    pub home: String,
    /// The harnesses installed there by kind, with the version each prints, if known.
    pub harnesses: Vec<(String, Option<String>)>,
}

/// The offered level, the one achieved on a target, and why it fell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Achieved {
    /// What the document offers.
    pub offered: Fidelity,
    /// What the target can do with it.
    pub achieved: Fidelity,
    /// One sentence per reason it fell, in the order they were found.
    pub why: Vec<String>,
}

/// One version as section 6 orders it: `core[-pre][+build]`.
#[derive(PartialEq, Eq)]
struct Version {
    core: Vec<u64>,
    pre: Vec<Ident>,
}

/// A pre-release identifier: digits before letters, as semver orders them.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Number(u64),
    Word(String),
}

impl Version {
    /// `None` when `text` is not of the form section 6 compares.
    fn parse(text: &str) -> Option<Version> {
        let text = text.split_once('+').map_or(text, |(version, _)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let number = |p: &str| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        };
        let core = core.split('.').map(number).collect::<Option<Vec<u64>>>()?;
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|p| {
                    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                        None
                    } else if p.bytes().all(|b| b.is_ascii_digit()) {
                        number(p).map(Ident::Number)
                    } else {
                        Some(Ident::Word(p.to_owned()))
                    }
                })
                .collect::<Option<Vec<Ident>>>()?,
        };
        Some(Version { core, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let len = self.core.len().max(other.core.len());
        let at = |v: &Version, i: usize| v.core.get(i).copied().unwrap_or(0);
        (0..len)
            .map(|i| at(self, i).cmp(&at(other, i)))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

/// Whether `have` is not older than `want` (section 6); `None` when either
/// is not a version section 6 compares.
fn not_older(have: &str, want: &str) -> Option<bool> {
    Some(Version::parse(have)? >= Version::parse(want)?)
}

impl Manifest {
    /// What `target` achieves with every attachment present.
    pub fn achievable(&self, target: &Target) -> Achieved {
        achievable(self, target, &[])
    }
}

/// What `target` achieves when the attachments named in `detached` are absent.
pub fn achievable(manifest: &Manifest, target: &Target, detached: &[String]) -> Achieved {
    let parts = manifest.parts();
    let offered = parts.fidelity;
    let mut why = Vec::new();
    if offered == Fidelity::Unsupported {
        why.push(format!(
            "the writer offers nothing: {}",
            parts.reason.as_deref().unwrap_or_default()
        ));
        return Achieved {
            offered,
            achieved: offered,
            why,
        };
    }
    let installed = |kind: &str| target.harnesses.iter().find(|(k, _)| k == kind);
    for a in &parts.attachments {
        if a.fidelity != Fidelity::NativeSession {
            continue;
        }
        let have = installed(&a.harness);
        let fell = if detached.contains(&a.name) {
            Some(format!("attachment {} is detached", a.name))
        } else if have.is_none() {
            Some(format!("{} is not installed here", a.harness))
        } else if let (Some(want), Some((_, have))) = (&parts.harness.version, have)
            && !have
                .as_deref()
                .and_then(|have| not_older(have, want))
                .unwrap_or(false)
        {
            Some(format!(
                "{} here is {}, older than {want} or not comparable",
                a.harness,
                have.as_deref().unwrap_or("of unknown version")
            ))
        } else if a.format != HARNESS_STATE_FORMAT {
            Some(format!(
                "attachment {} is {}, which is not read natively here",
                a.name, a.format
            ))
        } else if let Some(bound) = &a.bound
            && (bound.root != target.root || bound.home != target.home)
        {
            Some(format!(
                "attachment {} is bound to root {} and home {}, and here they are {} and {}",
                a.name, bound.root, bound.home, target.root, target.home
            ))
        } else {
            None
        };
        match fell {
            None => {
                return Achieved {
                    offered,
                    achieved: Fidelity::NativeSession,
                    why,
                };
            }
            Some(reason) => why.push(reason),
        }
    }
    if parts.package.is_some() {
        if let Some(a) = parts
            .attachments
            .iter()
            .find(|a| !detached.contains(&a.name) && installed(&a.harness).is_some())
        {
            if offered == Fidelity::NativeSession {
                why.push(format!(
                    "a new {} session starts from {} and the package",
                    a.harness, a.name
                ));
            }
            return Achieved {
                offered,
                achieved: Fidelity::NewNativeSessionWithPackage,
                why,
            };
        }
        if !parts.attachments.is_empty() {
            why.push("no present attachment is for a harness installed here".to_owned());
        }
    } else if !parts.attachments.is_empty() {
        why.push("the document has no work package to start a new session from".to_owned());
    }
    Achieved {
        offered,
        achieved: Fidelity::WorkspaceOnly,
        why,
    }
}
