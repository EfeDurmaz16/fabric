//! Harness versions, as `--version` prints them and as sessions record
//! them.
//!
//! Invariant: a version is `MAJOR.MINOR.PATCH`, with an optional
//! pre-release after `-` and build metadata after `+`, and two versions
//! are ordered by semver precedence (semver.org 2.0.0, section 11): the
//! three numbers, then a release above any of its pre-releases, then the
//! pre-release identifiers one by one, numbers as numbers and below
//! words, a longer list above its own prefix. Build metadata is ignored.
//! Anything else does not parse, and a caller that cannot read a version
//! refuses rather than guesses. Formats seen on
//! 2026-10-21: `codex-cli 0.155.1` and `2.1.294 (Claude Code)` from
//! `--version`, and `0.163.0-alpha.10.2` in the `session_meta` of a
//! rollout the Codex desktop app wrote.

use std::cmp::Ordering;
use std::fmt;

/// Why a version did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// The text holds no token that starts like a version.
    #[error("{0:?} holds no version")]
    NoVersion(String),
    /// The token is not `MAJOR.MINOR.PATCH` with an optional pre-release.
    #[error("{0:?} is not MAJOR.MINOR.PATCH with an optional pre-release")]
    Malformed(String),
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
        let malformed = || VersionError::Malformed(text.to_owned());
        let without_build = text.split_once('+').map_or(text, |(version, _)| version);
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
        let mut pre = Vec::new();
        for part in pre_text.into_iter().flat_map(|p| p.split('.')) {
            let identifier = if let Some(n) = number(part) {
                Identifier::Number(n)
            } else if !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                Identifier::Word(part.to_owned())
            } else {
                return Err(malformed());
            };
            pre.push(identifier);
        }
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
            .ok_or_else(|| VersionError::NoVersion(output.to_owned()))?;
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
}
