//! Paths that have been checked.
//!
//! Invariant: a `RelPath` or `AbsPath` value always satisfies the path rules
//! of ADR 0004, so code that writes files can trust the type instead of
//! checking strings. Parsing happens once, at the boundary, including inside
//! `Deserialize`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use unicode_normalization::is_nfc;

/// Why a path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The path has no characters.
    #[error("path is empty")]
    Empty,
    /// A relative path started with a slash.
    #[error("relative path starts with a slash")]
    NotRelative,
    /// An absolute path did not start with a slash.
    #[error("absolute path does not start with a slash")]
    NotAbsolute,
    /// The path holds a NUL byte.
    #[error("path contains a NUL byte")]
    Nul,
    /// Two slashes in a row, or a trailing slash.
    #[error("path has an empty component")]
    EmptyComponent,
    /// A `.` or `..` component.
    #[error("path has a `.` or `..` component")]
    DotComponent,
    /// A `.git` component in any letter case, which restore must never write into.
    #[error("path has a `.git` component")]
    GitComponent,
    /// The path is not in Unicode normalization form C.
    #[error("path is not NFC-normalized")]
    NotNfc,
}

fn check_components(path: &str) -> Result<(), PathError> {
    if path.contains('\0') {
        return Err(PathError::Nul);
    }
    for component in path.split('/') {
        match component {
            "" => return Err(PathError::EmptyComponent),
            "." | ".." => return Err(PathError::DotComponent),
            // APFS and casefolded ext4 fold `.Git` onto `.git`; g, i and t
            // have no case-fold sources outside ASCII, so this is the whole set.
            c if c.eq_ignore_ascii_case(".git") => return Err(PathError::GitComponent),
            _ => {}
        }
    }
    if !is_nfc(path) {
        return Err(PathError::NotNfc);
    }
    Ok(())
}

/// A path relative to a workspace root or a harness state root.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RelPath(String);

impl RelPath {
    /// Checks `path` against the rules and wraps it.
    pub fn parse(path: &str) -> Result<Self, PathError> {
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path.starts_with('/') {
            return Err(PathError::NotRelative);
        }
        check_components(path)?;
        Ok(Self(path.to_owned()))
    }

    /// Returns the path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true when `self` is `prefix` or lies below it.
    #[must_use]
    pub fn starts_with(&self, prefix: &RelPath) -> bool {
        self.0 == prefix.0
            || self
                .0
                .strip_prefix(prefix.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// Returns true when `self` is `prefix` or lies below it on a volume that folds ASCII case.
    ///
    /// The owner's APFS volumes fold case, so `.FABRIC/x` is under `.fabric`
    /// and `Node_Modules` is `node_modules` there; a guard that compares
    /// bytes would let one directory pass as two.
    #[must_use]
    pub fn starts_with_ignore_ascii_case(&self, prefix: &RelPath) -> bool {
        let (path, prefix) = (self.0.as_bytes(), prefix.0.as_bytes());
        path.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            && matches!(path.get(prefix.len()), None | Some(b'/'))
    }

    /// Returns true when the two paths name one entry on a volume that folds ASCII case.
    #[must_use]
    pub fn eq_ignore_ascii_case(&self, other: &RelPath) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl FromStr for RelPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// An absolute path on a node; the one kind of host path a manifest records.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct AbsPath(String);

impl AbsPath {
    /// Checks `path` against the rules and wraps it.
    pub fn parse(path: &str) -> Result<Self, PathError> {
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        let Some(rest) = path.strip_prefix('/') else {
            return Err(PathError::NotAbsolute);
        };
        if !rest.is_empty() {
            check_components(rest)?;
        }
        Ok(Self(path.to_owned()))
    }

    /// Returns the path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the normalized text of `self` with `rel` appended.
    #[must_use]
    pub fn join(&self, rel: &RelPath) -> AbsPath {
        if self.0 == "/" {
            AbsPath(format!("/{}", rel.0))
        } else {
            AbsPath(format!("{}/{}", self.0, rel.0))
        }
    }
}

impl fmt::Display for AbsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AbsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AbsPath({:?})", self.0)
    }
}

impl FromStr for AbsPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for AbsPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_rules() {
        assert!(RelPath::parse("src/lib.rs").is_ok());
        assert_eq!(RelPath::parse(""), Err(PathError::Empty));
        assert_eq!(RelPath::parse("/etc"), Err(PathError::NotRelative));
        assert_eq!(RelPath::parse("a\0b"), Err(PathError::Nul));
        assert_eq!(RelPath::parse("a//b"), Err(PathError::EmptyComponent));
        assert_eq!(RelPath::parse("a/"), Err(PathError::EmptyComponent));
        assert_eq!(RelPath::parse("./a"), Err(PathError::DotComponent));
        assert_eq!(RelPath::parse("a/../b"), Err(PathError::DotComponent));
        assert_eq!(
            RelPath::parse("a/.git/config"),
            Err(PathError::GitComponent)
        );
        assert_eq!(
            RelPath::parse(".Git/hooks/pre-commit"),
            Err(PathError::GitComponent)
        );
        assert_eq!(RelPath::parse(".GIT"), Err(PathError::GitComponent));
        assert!(RelPath::parse(".gitignore").is_ok());
        assert_eq!(RelPath::parse("caf\u{65}\u{301}"), Err(PathError::NotNfc));
        assert!(RelPath::parse("caf\u{e9}").is_ok());
    }

    #[test]
    fn folded_comparisons_ignore_ascii_case_and_respect_components() {
        let p = |text: &str| RelPath::parse(text).unwrap();
        assert!(p(".FABRIC/jobs").starts_with_ignore_ascii_case(&p(".fabric")));
        assert!(p("Node_Modules").starts_with_ignore_ascii_case(&p("node_modules")));
        assert!(!p("node_modules2").starts_with_ignore_ascii_case(&p("node_modules")));
        assert!(!p("node").starts_with_ignore_ascii_case(&p("node_modules")));
        assert!(p("Node_Modules").eq_ignore_ascii_case(&p("node_modules")));
        assert!(!p("node_modules/a").eq_ignore_ascii_case(&p("node_modules")));
    }

    #[test]
    fn abs_path_rules() {
        assert!(AbsPath::parse("/").is_ok());
        assert!(AbsPath::parse("/Users/x/oss/fabric").is_ok());
        assert_eq!(AbsPath::parse("relative"), Err(PathError::NotAbsolute));
        assert_eq!(AbsPath::parse("/a/"), Err(PathError::EmptyComponent));
        assert_eq!(AbsPath::parse("/a/./b"), Err(PathError::DotComponent));
        assert_eq!(AbsPath::parse("/a/.git"), Err(PathError::GitComponent));
    }

    #[test]
    fn join_and_prefix() {
        let root = AbsPath::parse("/w").unwrap();
        let rel = RelPath::parse("node_modules/x").unwrap();
        assert_eq!(root.join(&rel).as_str(), "/w/node_modules/x");
        assert_eq!(
            AbsPath::parse("/").unwrap().join(&rel).as_str(),
            "/node_modules/x"
        );
        let deps = RelPath::parse("node_modules").unwrap();
        assert!(rel.starts_with(&deps));
        assert!(deps.starts_with(&deps));
        assert!(
            !RelPath::parse("node_modules2/x")
                .unwrap()
                .starts_with(&deps)
        );
    }

    #[test]
    fn deserialize_refuses_a_bad_path() {
        let bytes = postcard::to_stdvec("a/../b").unwrap();
        assert!(postcard::from_bytes::<RelPath>(&bytes).is_err());
        let bytes = postcard::to_stdvec("ok/path").unwrap();
        assert_eq!(
            postcard::from_bytes::<RelPath>(&bytes).unwrap(),
            RelPath::parse("ok/path").unwrap()
        );
    }
}
