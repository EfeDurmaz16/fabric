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

/// Compares dotted versions part by part as integers; `None` when either
/// has a part that is not a number.
fn not_older(have: &str, want: &str) -> Option<bool> {
    let parts = |text: &str| -> Option<Vec<u64>> {
        text.split('.')
            .map(|p| {
                p.bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| p.parse().ok())
                    .flatten()
            })
            .collect()
    };
    let (mut have, mut want) = (parts(have)?, parts(want)?);
    let len = have.len().max(want.len());
    have.resize(len, 0);
    want.resize(len, 0);
    Some(have >= want)
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
