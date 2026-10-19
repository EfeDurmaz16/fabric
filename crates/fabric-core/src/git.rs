//! Git names that cross the wire (ADR 0004).
//!
//! Invariant: a `GitOid`, `RefName`, `BranchName`, `RemoteName`,
//! `GitConfigKey` or `GitConfigValue` value always passes the rule its type
//! states, so restore can put it on a git command line without looking at it
//! again, and a manifest can carry only the config keys ADR 0004 allows.
//! Parsing happens once, at the boundary, including inside `Deserialize`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why a git name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitNameError {
    /// An object id is not 40 or 64 lowercase hex characters.
    #[error("object id is not 40 or 64 lowercase hex characters")]
    BadOid,
    /// A ref name does not start with `refs/` or breaks a `check-ref-format` rule.
    #[error("ref name breaks the check-ref-format rules")]
    BadRefName,
    /// A branch or remote name is empty, starts with `-`, or breaks a `check-ref-format` rule.
    #[error("branch or remote name is not valid")]
    BadShortName,
    /// A config key outside the allowlist of ADR 0004.
    #[error("config key {0} is not in the carried allowlist")]
    KeyNotAllowed(String),
    /// A config value with a newline or NUL, or one that starts with `-`.
    #[error("config value is not valid")]
    BadValue,
}

macro_rules! parsed_string {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Returns the text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = GitNameError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::parse(&text).map_err(serde::de::Error::custom)
            }
        }
    };
}

parsed_string!(
    GitOid,
    "A git object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex characters."
);

impl GitOid {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        let hex = text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !(text.len() == 40 || text.len() == 64) || !hex {
            return Err(GitNameError::BadOid);
        }
        Ok(Self(text.to_owned()))
    }
}

/// Applies the rules of `git check-ref-format` to a full ref name.
fn check_ref_format(name: &str) -> bool {
    if !name.starts_with("refs/") || name.ends_with('.') || name.contains("@{") {
        return false;
    }
    if name
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
    {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    name.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && c.strip_suffix(".lock").is_none())
}

parsed_string!(
    RefName,
    "A full ref name under `refs/` that passes `git check-ref-format`."
);

impl RefName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        if !check_ref_format(text) {
            return Err(GitNameError::BadRefName);
        }
        Ok(Self(text.to_owned()))
    }
}

/// Checks a branch or remote name: what git accepts between `refs/x/` and the next slash.
fn check_short_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "HEAD"
        && check_ref_format(&format!("refs/heads/{name}"))
}

parsed_string!(
    BranchName,
    "A branch name as `git branch` accepts it: valid under `refs/heads/`, not `HEAD`, no leading `-`."
);

impl BranchName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        if !check_short_name(text) {
            return Err(GitNameError::BadShortName);
        }
        Ok(Self(text.to_owned()))
    }
}

parsed_string!(
    RemoteName,
    "A remote name as `git remote add` accepts it: valid under `refs/remotes/`, no leading `-`."
);

impl RemoteName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        if !check_short_name(text) {
            return Err(GitNameError::BadShortName);
        }
        Ok(Self(text.to_owned()))
    }
}

parsed_string!(
    GitConfigValue,
    "A config value restore may pass to `git config`: no newline, no NUL, no leading `-`."
);

impl GitConfigValue {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        if text.starts_with('-') || text.contains('\n') || text.contains('\0') {
            return Err(GitNameError::BadValue);
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns true when the value is a `scheme://` URL whose authority holds
    /// userinfo, which is where a token lives in a remote URL.
    ///
    /// The scp-like form `git@host:path` has no scheme and no password slot,
    /// so it is not userinfo by this rule.
    #[must_use]
    pub fn has_userinfo(&self) -> bool {
        let Some((_, rest)) = self.0.split_once("://") else {
            return false;
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        rest[..end].contains('@')
    }
}

/// A repository config key that a snapshot may carry; exactly the allowlist of ADR 0004.
///
/// It travels as its text form, such as `remote.origin.url`, and an unknown
/// key fails to decode.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitConfigKey {
    /// `remote.<name>.url`; userinfo is stripped at capture and refused on decode.
    RemoteUrl(RemoteName),
    /// `remote.<name>.pushurl`; userinfo is stripped at capture and refused on decode.
    RemotePushUrl(RemoteName),
    /// `remote.<name>.fetch`.
    RemoteFetch(RemoteName),
    /// `branch.<name>.remote`.
    BranchRemote(BranchName),
    /// `branch.<name>.merge`.
    BranchMerge(BranchName),
    /// `branch.<name>.rebase`.
    BranchRebase(BranchName),
    /// `push.default`.
    PushDefault,
    /// `core.ignorecase`.
    CoreIgnoreCase,
    /// `core.autocrlf`.
    CoreAutoCrlf,
    /// `core.symlinks`.
    CoreSymlinks,
}

impl GitConfigKey {
    /// Returns true for the keys whose value is a remote URL, which must carry no userinfo.
    #[must_use]
    pub const fn is_remote_url(&self) -> bool {
        matches!(
            self,
            GitConfigKey::RemoteUrl(_) | GitConfigKey::RemotePushUrl(_)
        )
    }

    /// Parses the text form of a key, refusing every key outside the allowlist.
    pub fn parse(text: &str) -> Result<Self, GitNameError> {
        let denied = || GitNameError::KeyNotAllowed(text.to_owned());
        match text {
            "push.default" => return Ok(GitConfigKey::PushDefault),
            "core.ignorecase" => return Ok(GitConfigKey::CoreIgnoreCase),
            "core.autocrlf" => return Ok(GitConfigKey::CoreAutoCrlf),
            "core.symlinks" => return Ok(GitConfigKey::CoreSymlinks),
            _ => {}
        }
        let (section, rest) = text.split_once('.').ok_or_else(denied)?;
        let (subsection, key) = rest.rsplit_once('.').ok_or_else(denied)?;
        match (section, key) {
            ("remote", "url") => RemoteName::parse(subsection).map(GitConfigKey::RemoteUrl),
            ("remote", "pushurl") => RemoteName::parse(subsection).map(GitConfigKey::RemotePushUrl),
            ("remote", "fetch") => RemoteName::parse(subsection).map(GitConfigKey::RemoteFetch),
            ("branch", "remote") => BranchName::parse(subsection).map(GitConfigKey::BranchRemote),
            ("branch", "merge") => BranchName::parse(subsection).map(GitConfigKey::BranchMerge),
            ("branch", "rebase") => BranchName::parse(subsection).map(GitConfigKey::BranchRebase),
            _ => Err(denied()),
        }
        .map_err(|_| denied())
    }
}

impl fmt::Display for GitConfigKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitConfigKey::RemoteUrl(n) => write!(f, "remote.{n}.url"),
            GitConfigKey::RemotePushUrl(n) => write!(f, "remote.{n}.pushurl"),
            GitConfigKey::RemoteFetch(n) => write!(f, "remote.{n}.fetch"),
            GitConfigKey::BranchRemote(n) => write!(f, "branch.{n}.remote"),
            GitConfigKey::BranchMerge(n) => write!(f, "branch.{n}.merge"),
            GitConfigKey::BranchRebase(n) => write!(f, "branch.{n}.rebase"),
            GitConfigKey::PushDefault => f.write_str("push.default"),
            GitConfigKey::CoreIgnoreCase => f.write_str("core.ignorecase"),
            GitConfigKey::CoreAutoCrlf => f.write_str("core.autocrlf"),
            GitConfigKey::CoreSymlinks => f.write_str("core.symlinks"),
        }
    }
}

impl FromStr for GitConfigKey {
    type Err = GitNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for GitConfigKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for GitConfigKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_is_40_or_64_lowercase_hex() {
        assert!(GitOid::parse(&"a".repeat(40)).is_ok());
        assert!(GitOid::parse(&"0".repeat(64)).is_ok());
        assert_eq!(GitOid::parse(&"A".repeat(40)), Err(GitNameError::BadOid));
        assert_eq!(GitOid::parse(&"a".repeat(39)), Err(GitNameError::BadOid));
        assert_eq!(GitOid::parse("-"), Err(GitNameError::BadOid));
    }

    #[test]
    fn ref_name_follows_check_ref_format() {
        assert!(RefName::parse("refs/heads/main").is_ok());
        assert!(RefName::parse("refs/fabric/01J/snap/01K").is_ok());
        for bad in [
            "main",
            "refs/heads/",
            "refs//heads",
            "refs/heads/.hidden",
            "refs/heads/x.lock",
            "refs/heads/a..b",
            "refs/heads/a b",
            "refs/heads/a~1",
            "refs/heads/a^",
            "refs/heads/a:b",
            "refs/heads/a?",
            "refs/heads/a*",
            "refs/heads/a[0]",
            "refs/heads/a\\b",
            "refs/heads/a.",
            "refs/heads/a@{1}",
            "refs/heads/a\u{7}",
        ] {
            assert_eq!(RefName::parse(bad), Err(GitNameError::BadRefName), "{bad}");
        }
    }

    #[test]
    fn branch_and_remote_names_refuse_options_and_head() {
        assert!(BranchName::parse("feat/core").is_ok());
        assert!(RemoteName::parse("origin").is_ok());
        for bad in ["", "-x", "HEAD", "a..b", ".hidden"] {
            assert_eq!(
                BranchName::parse(bad),
                Err(GitNameError::BadShortName),
                "{bad}"
            );
            assert_eq!(
                RemoteName::parse(bad),
                Err(GitNameError::BadShortName),
                "{bad}"
            );
        }
    }

    #[test]
    fn config_keys_are_exactly_the_allowlist() {
        for text in [
            "remote.origin.url",
            "remote.origin.pushurl",
            "remote.origin.fetch",
            "remote.my.fork.url",
            "branch.main.remote",
            "branch.main.merge",
            "branch.main.rebase",
            "push.default",
            "core.ignorecase",
            "core.autocrlf",
            "core.symlinks",
        ] {
            let key = GitConfigKey::parse(text).unwrap();
            assert_eq!(key.to_string(), text);
        }
        assert_eq!(
            GitConfigKey::parse("remote.my.fork.url"),
            Ok(GitConfigKey::RemoteUrl(
                RemoteName::parse("my.fork").unwrap()
            ))
        );
        for bad in [
            "credential.helper",
            "core.fsmonitor",
            "core.hooksPath",
            "http.extraheader",
            "url.x.insteadof",
            "remote..url",
            "remote.-x.url",
            "remote.origin.proxy",
            "branch.HEAD.merge",
            "push",
            "",
        ] {
            assert!(
                matches!(GitConfigKey::parse(bad), Err(GitNameError::KeyNotAllowed(k)) if k == bad),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_url_value_knows_when_it_carries_userinfo() {
        let with = |v: &str| GitConfigValue::parse(v).unwrap().has_userinfo();
        assert!(with("https://efe:ghp_secret123@github.com/x/y.git"));
        assert!(with("https://token@github.com/x/y.git"));
        assert!(with("ssh://git@github.com/x/y.git"));
        assert!(!with("https://github.com/x/y.git"));
        assert!(!with("https://github.com/x/y.git?u=a@b"));
        assert!(!with("git@github.com:x/y.git"));
        assert!(!with("/srv/git/y.git"));
        assert!(
            GitConfigKey::parse("remote.origin.url")
                .unwrap()
                .is_remote_url()
        );
        assert!(
            GitConfigKey::parse("remote.origin.pushurl")
                .unwrap()
                .is_remote_url()
        );
        assert!(
            !GitConfigKey::parse("remote.origin.fetch")
                .unwrap()
                .is_remote_url()
        );
    }

    #[test]
    fn config_values_cannot_look_like_options_or_span_lines() {
        assert!(GitConfigValue::parse("https://github.com/x/y.git").is_ok());
        assert!(GitConfigValue::parse("+refs/heads/*:refs/remotes/origin/*").is_ok());
        assert_eq!(
            GitConfigValue::parse("--global"),
            Err(GitNameError::BadValue)
        );
        assert_eq!(GitConfigValue::parse("a\nb"), Err(GitNameError::BadValue));
        assert_eq!(GitConfigValue::parse("a\0b"), Err(GitNameError::BadValue));
    }

    #[test]
    fn names_travel_as_text() {
        let key = GitConfigKey::parse("branch.main.merge").unwrap();
        let bytes = postcard::to_stdvec(&key).unwrap();
        assert_eq!(bytes, postcard::to_stdvec("branch.main.merge").unwrap());
        assert_eq!(postcard::from_bytes::<GitConfigKey>(&bytes).unwrap(), key);
        let denied = postcard::to_stdvec("credential.helper").unwrap();
        assert!(postcard::from_bytes::<GitConfigKey>(&denied).is_err());
    }
}
