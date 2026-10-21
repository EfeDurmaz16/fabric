//! Harness versions, as `--version` prints them and as sessions record
//! them, and the guard that keeps an older harness off a newer session.
//!
//! Invariant: a version is `MAJOR.MINOR.PATCH`, with an optional
//! pre-release after `-` and build metadata after `+`, each a list of
//! dot-separated identifiers of ASCII letters, digits and `-`, and at most
//! `MAX_LEN` bytes in all; two versions are ordered by semver precedence
//! (semver.org 2.0.0, section 11): the three numbers, then a release
//! above any of its pre-releases, then the pre-release identifiers one by
//! one, numbers as numbers and below words, a longer list above its own
//! prefix. Build metadata is checked and then ignored. Anything else does
//! not parse, and a caller that cannot read a version refuses rather than
//! guesses. A version comes from a file that travelled and is printed in
//! a refusal, so the text a parsed version keeps holds no character a
//! terminal acts on, and the text a parse error keeps is cut and has its
//! control characters replaced. One parse and one order serve the resume
//! guard of every adapter and the harness rows of `fabric doctor`, so the
//! two never disagree on which of two versions is older. Formats seen on
//! 2026-10-21: `codex-cli 0.155.1` and `2.1.294 (Claude Code)` from
//! `--version`, and `0.163.0-alpha.10.2` in the `session_meta` of a
//! rollout the Codex desktop app wrote.
//!
//! An older harness must not run a session a newer one wrote (gate
//! finding F10b): Codex 0.142.4 appended records without the `ordinal`
//! field 0.155.1 writes, and wrote a `legacy` thread row into its own
//! database, after which 0.155.1 refused the session. `refuse_older`
//! therefore asks the binary a launch would run for its version before
//! anything starts, and refuses when it is older than the newest version
//! the session records, or when its version cannot be read.

use std::cmp::Ordering;
use std::fmt;

/// The longest version text that parses. The longest seen, a desktop
/// Codex pre-release such as `0.163.0-alpha.10.2`, is 18 bytes.
const MAX_LEN: usize = 64;

/// Why a version did not parse. The text it holds is cut and has its
/// control characters replaced (`harness::shown`), because it can come
/// from a file that travelled and is printed to a terminal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// The text holds no token that starts like a version.
    #[error("{0:?} holds no version")]
    NoVersion(String),
    /// The token is not `MAJOR.MINOR.PATCH` with an optional pre-release
    /// and build metadata, or is longer than `MAX_LEN` bytes.
    #[error("{0:?} is not MAJOR.MINOR.PATCH with an optional pre-release")]
    Malformed(String),
}

/// True for a non-empty list of dot-separated, non-empty identifiers of
/// ASCII letters, digits and `-`, as semver's pre-release and build
/// metadata are.
fn identifiers(text: &str) -> bool {
    text.split('.').all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// One dot-separated identifier of a pre-release. The variant order is
/// the semver order: a number ranks below a word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    /// Digits only, compared as a number.
    Number(u64),
    /// Letters, digits and `-`, compared in ASCII order.
    Word(String),
}

/// A harness version, ordered by semver precedence.
#[derive(Debug, Clone)]
pub struct HarnessVersion {
    core: [u64; 3],
    pre: Vec<Identifier>,
    text: String,
}

impl HarnessVersion {
    /// Parses a version such as `0.155.1` or `0.163.0-alpha.10.2`.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let malformed = || VersionError::Malformed(crate::harness::shown(text));
        if text.len() > MAX_LEN {
            return Err(malformed());
        }
        let without_build = match text.split_once('+') {
            Some((version, build)) if identifiers(build) => version,
            Some(_) => return Err(malformed()),
            None => text,
        };
        let (core_text, pre_text) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let mut parts = core_text.split('.');
        let mut core = [0_u64; 3];
        for slot in &mut core {
            *slot = parts.next().and_then(number).ok_or_else(malformed)?;
        }
        if parts.next().is_some() {
            return Err(malformed());
        }
        if pre_text.is_some_and(|p| !identifiers(p)) {
            return Err(malformed());
        }
        let pre = pre_text
            .into_iter()
            .flat_map(|p| p.split('.'))
            .map(|part| {
                number(part).map_or_else(|| Identifier::Word(part.to_owned()), Identifier::Number)
            })
            .collect();
        Ok(HarnessVersion {
            core,
            pre,
            text: text.to_owned(),
        })
    }

    /// The version in what `--version` printed: the first token that
    /// starts like one, as `codex-cli 0.155.1` or `2.1.294 (Claude Code)`.
    pub fn from_output(output: &str) -> Result<Self, VersionError> {
        let token = crate::process::version_token(output)
            .ok_or_else(|| VersionError::NoVersion(crate::harness::shown(output)))?;
        Self::parse(token)
    }

    /// The version as it was written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl Ord for HarnessVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for HarnessVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HarnessVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HarnessVersion {}

impl fmt::Display for HarnessVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Refuses to start `binary` on `session` when the version it reports is
/// older than `recorded`, the newest version the session records, or
/// cannot be read. Runs `binary --version` with the launch environment
/// minus the parent-session markers, as the launch itself would run.
#[cfg(any(feature = "claude", feature = "codex"))]
pub(crate) async fn refuse_older(
    program: &str,
    binary: &std::path::Path,
    env: &[(fabric_core::env::EnvVarName, String)],
    session: &crate::session::SessionId,
    recorded: &HarnessVersion,
) -> Result<(), crate::error::HarnessError> {
    use crate::error::HarnessError;
    let unknown = |reason: String| HarnessError::HarnessVersionUnknown {
        program: program.to_owned(),
        recorded: recorded.to_string(),
        session: session.clone(),
        reason,
    };
    let output = crate::process::reported_version(binary, env)
        .await
        .map_err(|e| unknown(e.to_string()))?;
    let local = HarnessVersion::from_output(&output).map_err(|e| unknown(e.to_string()))?;
    if local < *recorded {
        return Err(HarnessError::HarnessOlderThanSession {
            program: program.to_owned(),
            local: local.to_string(),
            recorded: recorded.to_string(),
            session: session.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> HarnessVersion {
        HarnessVersion::parse(text).unwrap()
    }

    #[test]
    fn versions_order_by_semver_precedence() {
        let ascending = [
            "0.142.4",
            "0.155.0-alpha.16",
            "0.155.1",
            "0.155.10",
            "0.163.0-alpha.2",
            "0.163.0-alpha.10",
            "0.163.0-alpha.10.2",
            "0.163.0-beta",
            "0.163.0",
            "1.0.0",
        ];
        for pair in ascending.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
            assert!(v(pair[1]) > v(pair[0]), "{} > {}", pair[1], pair[0]);
        }
        assert_eq!(v("0.155.1+build.7"), v("0.155.1"));
    }

    #[test]
    fn only_three_numbers_and_a_pre_release_parse() {
        for bad in [
            "",
            "0.155",
            "0.155.1.2",
            "0.155.x",
            "v0.155.1",
            "0.155.1-",
            "0.155.1-a..b",
        ] {
            assert!(HarnessVersion::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            HarnessVersion::from_output("codex-cli 0.155.1").unwrap(),
            v("0.155.1")
        );
        assert_eq!(
            HarnessVersion::from_output("2.1.199 (Claude Code)").unwrap(),
            v("2.1.199")
        );
        assert!(matches!(
            HarnessVersion::from_output("codex-cli"),
            Err(VersionError::NoVersion(_))
        ));
        assert!(matches!(
            HarnessVersion::from_output("fake 1.2"),
            Err(VersionError::Malformed(_))
        ));
    }

    /// A recorded version comes from a file that travelled and is printed
    /// in a refusal, so its build metadata is held to semver's identifiers
    /// and its length to `MAX_LEN`; an escape sequence after `+` does not
    /// parse, and the error that names it shows no control character and
    /// stays short.
    #[test]
    fn build_metadata_is_checked_and_a_bad_version_is_shown_safely() {
        assert_eq!(v("0.155.1+build-7.x"), v("0.155.1"));
        let escape = "99.0.0+\u{1b}]52;c;aGk=\u{7}";
        let long = format!("1.0.0+{}", "a".repeat(MAX_LEN));
        for bad in ["0.155.1+", "0.155.1+a b", "0.155.1+a..b", escape, &long] {
            assert!(HarnessVersion::parse(bad).is_err(), "{bad:?}");
        }
        let flood = format!("1.0.0+\u{1b}{}", "x".repeat(1 << 20));
        for text in [escape, flood.as_str()] {
            let shown = HarnessVersion::parse(text).unwrap_err().to_string();
            assert!(!shown.contains('\u{1b}') && shown.len() < 200, "{shown:?}");
        }
    }
}
