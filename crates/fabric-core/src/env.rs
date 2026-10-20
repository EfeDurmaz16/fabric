//! The environment spec, version 0 (ADR 0007).
//!
//! Invariant: a spec names things and never holds a secret value: an
//! environment variable appears as a parsed name that cannot hold a `=` or a
//! value. A setup command is a program and its arguments, never a shell
//! string. Every list whose order carries no meaning is strictly sorted, so
//! the spec's identity, the hash of its canonical bytes, is the same for the
//! same content on every node, and an edit is a new spec.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::codec::{self, CodecError, VersionedError};
use crate::credential::looks_like_credential;
use crate::harness::ProgramName;
use crate::id::EnvSpecId;
use crate::path::RelPath;
use crate::secret::pattern_matches;
use crate::snapshot::first_out_of_order;

/// The format version this crate writes; the first varint of every encoded spec.
pub const ENV_SPEC_VERSION: u16 = 0;

/// Where a workspace's override file lives, relative to its root (ADR 0007).
pub const OVERRIDE_FILE: &str = ".fabric/env.toml";

/// Why an environment spec was refused.
#[derive(Debug, thiserror::Error)]
pub enum EnvSpecError {
    /// A list is not strictly sorted by its key, which also catches a repeated key.
    #[error("{list} entry {index} is not strictly after the one before it")]
    OutOfOrder {
        /// Which list: `tools`, `env_var_names`, `cache_paths` or `secret_patterns`.
        list: &'static str,
        /// Zero-based index of the entry, within its list.
        index: usize,
    },
    /// The bytes carry another version or do not decode, or the value does
    /// not encode. A bad program, variable or tool name, or a bad pin, on the wire surfaces here too.
    #[error("environment spec codec: {0}")]
    Codec(#[from] VersionedError),
}

impl From<CodecError> for EnvSpecError {
    fn from(err: CodecError) -> Self {
        EnvSpecError::Codec(VersionedError::Codec(err))
    }
}

/// Why an environment variable name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvVarNameError {
    /// The name has no characters.
    #[error("environment variable name is empty")]
    Empty,
    /// A character outside `[A-Za-z0-9_]`, or a leading digit.
    #[error("environment variable name has an invalid character at index {index}")]
    BadChar {
        /// Zero-based index of the offending character.
        index: usize,
    },
}

/// The name of an environment variable: `[A-Za-z_][A-Za-z0-9_]*`.
///
/// A name cannot hold `=`, so a value cannot ride along in it. Checked once,
/// at the boundary, including inside `Deserialize`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EnvVarName(String);

impl EnvVarName {
    /// Checks `text` against the rule and wraps it.
    pub fn parse(text: &str) -> Result<Self, EnvVarNameError> {
        if text.is_empty() {
            return Err(EnvVarNameError::Empty);
        }
        let bad = text
            .bytes()
            .position(|b| !(b == b'_' || b.is_ascii_alphanumeric()));
        if let Some(index) = bad {
            return Err(EnvVarNameError::BadChar { index });
        }
        if text.as_bytes()[0].is_ascii_digit() {
            return Err(EnvVarNameError::BadChar { index: 0 });
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for EnvVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvVarName({:?})", self.0)
    }
}

impl FromStr for EnvVarName {
    type Err = EnvVarNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for EnvVarName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// The most characters a tool name or a version pin may have.
pub const MAX_PIN_LEN: usize = 64;

/// Why a tool name or a version pin was refused; the text itself is never quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    /// The text has no characters.
    #[error("is empty")]
    Empty,
    /// The text is longer than `MAX_PIN_LEN` characters.
    #[error("is longer than {MAX_PIN_LEN} characters")]
    TooLong,
    /// A character outside printable ASCII, or a space in a tool name.
    #[error("has a character outside printable ASCII, or a space in a tool name")]
    BadChar,
    /// The text is shaped like a token, a key or a URL with a password.
    #[error("looks like a credential; the spec travels to other nodes")]
    Credential,
}

/// Whether pin text may hold a space: a version range may, a tool name may not.
#[derive(Clone, Copy)]
enum Spaces {
    Allowed,
    Refused,
}

/// Checks the rules a tool name and a version pin share.
fn check_pin_text(text: &str, spaces: Spaces) -> Result<(), PinError> {
    if text.is_empty() {
        return Err(PinError::Empty);
    }
    if text.len() > MAX_PIN_LEN {
        return Err(PinError::TooLong);
    }
    let first = match spaces {
        Spaces::Allowed => b' ',
        Spaces::Refused => b'!',
    };
    if !text.bytes().all(|b| (first..=b'~').contains(&b)) {
        return Err(PinError::BadChar);
    }
    if looks_like_credential(text) {
        return Err(PinError::Credential);
    }
    Ok(())
}

/// The name of a pinned tool as pin files write it, such as `node`, `rust` or `npm:@angular/cli`.
///
/// One to 64 printable ASCII characters with no space, and not shaped like
/// a credential. Checked once, at the boundary, including inside
/// `Deserialize`, so a name from a peer cannot carry a terminal escape
/// sequence or a token to the node that prints it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, PinError> {
        check_pin_text(text, Spaces::Refused)?;
        Ok(Self(text.to_owned()))
    }

    /// Returns the name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A version pin as its file wrote it, such as `22`, `v20`, `>=18 <21` or `1.97.1`.
///
/// One to 64 printable ASCII characters, spaces allowed, and not shaped
/// like a credential. Kept whole and never parsed into parts. Checked once,
/// at the boundary, including inside `Deserialize`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolVersion(String);

impl ToolVersion {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, PinError> {
        check_pin_text(text, Spaces::Allowed)?;
        Ok(Self(text.to_owned()))
    }

    /// Returns the pin.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! pin_text_impls {
    ($name:ident) => {
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
            type Err = PinError;

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

pin_text_impls!(ToolName);
pin_text_impls!(ToolVersion);

/// Why a secret pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretPatternError {
    /// The pattern has no characters.
    #[error("secret pattern is empty")]
    Empty,
    /// A leading, trailing or doubled slash.
    #[error("secret pattern has an empty component")]
    EmptyComponent,
    /// A `.` or `..` component, which no path holds.
    #[error("secret pattern has a `.` or `..` component")]
    DotComponent,
    /// A `**` component; a pattern already matches at any depth.
    #[error("secret pattern has a `**` component; a pattern already matches at any depth")]
    DoubleStar,
    /// A character with a meaning in gitignore that the matcher does not share.
    #[error("secret pattern has an unsupported character at index {index}")]
    BadChar {
        /// Zero-based byte index of the character.
        index: usize,
    },
    /// The pattern denies the override file, which detection reads through the job's patterns.
    ///
    /// Such a pattern, `*.toml`, `env.toml` or `.fabric`, would drop every
    /// override from the next detection on, so it is refused wherever a
    /// pattern comes from: the override file or a peer's spec.
    #[error("secret pattern matches {OVERRIDE_FILE}, which detection must be able to read")]
    DeniesOverrideFile,
}

/// A secret pattern a job adds to the built-in list: path components separated by `/`, where `*` matches any run of characters.
///
/// Refused: `\`, `[`, `]`, `?`, `!`, `#`, whitespace and NUL, because
/// gitignore gives them meanings the matcher does not; empty, `.`, `..`
/// and `**` components; and any pattern that denies `OVERRIDE_FILE`.
/// Checked once, at the boundary, including inside `Deserialize`. The
/// matcher itself is `fabric_core::secret`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretPattern(String);

impl SecretPattern {
    /// Checks `text` against the pattern rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, SecretPatternError> {
        if text.is_empty() {
            return Err(SecretPatternError::Empty);
        }
        let bad = text.char_indices().find(|&(_, c)| {
            matches!(c, '\\' | '[' | ']' | '?' | '!' | '#' | '\0') || c.is_whitespace()
        });
        if let Some((index, _)) = bad {
            return Err(SecretPatternError::BadChar { index });
        }
        for component in text.split('/') {
            match component {
                "" => return Err(SecretPatternError::EmptyComponent),
                "." | ".." => return Err(SecretPatternError::DotComponent),
                "**" => return Err(SecretPatternError::DoubleStar),
                _ => {}
            }
        }
        let pattern = Self(text.to_owned());
        if pattern_matches(&pattern, OVERRIDE_FILE) {
            return Err(SecretPatternError::DeniesOverrideFile);
        }
        Ok(pattern)
    }

    /// Returns the pattern text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SecretPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretPattern({:?})", self.0)
    }
}

impl<'de> Deserialize<'de> for SecretPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Why a setup argument was refused; the argument itself is never quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SetupArgError {
    /// A control character, such as a terminal escape, a newline or NUL.
    #[error("has a control character")]
    Control,
    /// The text is shaped like a token, a key or a URL with a password.
    #[error("looks like a credential; pass it as an `env` name and let the program read it")]
    Credential,
}

/// One argument of a setup step: any text with no control character that is not shaped like a credential.
///
/// The spec travels to other nodes, and a node prints a peer's steps
/// before it runs them (ADR 0007), so an argument can carry neither a
/// token nor a terminal escape. Checked once, at the boundary, including
/// inside `Deserialize`. The empty argument is allowed; programs take it.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SetupArg(String);

impl SetupArg {
    /// Checks `text` against the rules and wraps it.
    pub fn parse(text: &str) -> Result<Self, SetupArgError> {
        if text.chars().any(char::is_control) {
            return Err(SetupArgError::Control);
        }
        if looks_like_credential(text) {
            return Err(SetupArgError::Credential);
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns the argument.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SetupArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SetupArg({:?})", self.0)
    }
}

impl<'de> Deserialize<'de> for SetupArg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// What a workspace needs before a harness can run in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSpec {
    /// Tools the workspace pins. Informational: printed beside the measured
    /// versions, never compared for equality and never a reason to fail.
    /// Sorted by name.
    pub tools: Vec<Tool>,
    /// Commands that make the workspace runnable, in the order they run.
    pub setup: Vec<SetupStep>,
    /// Names of environment variables the job needs; values never travel. Sorted.
    pub env_var_names: Vec<EnvVarName>,
    /// Paths that count as caches for snapshot policy and warmth checks. Sorted.
    pub cache_paths: Vec<RelPath>,
    /// Secret patterns the job added to the built-in list. The list only
    /// grows: detection keeps every pattern the job's previous spec held,
    /// even after the override file drops it. Sorted.
    pub secret_patterns: Vec<SecretPattern>,
}

/// One tool and the version its workspace pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// The tool's name as pin files write it, such as `node`, `rust` or `pnpm`.
    pub name: ToolName,
    /// The pin as its file wrote it (`22`, `v20`, `>=18`, `1.97.1`), kept whole and never parsed into parts.
    pub version: ToolVersion,
    /// Where the entry came from.
    pub source: ToolSource,
}

/// How a tool entry got into the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolSource {
    /// Found by detection on the workspace.
    Detected,
    /// Written by the user.
    Declared,
}

/// One command that materialize runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// The program to run, looked up on `PATH`, never through a shell.
    pub program: ProgramName,
    /// Its arguments, passed as they are.
    pub args: Vec<SetupArg>,
    /// The directory to run in, relative to the root; `None` is the root itself.
    pub cwd: Option<RelPath>,
    /// When the step runs.
    pub when: RunWhen,
}

/// When a setup step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunWhen {
    /// When a cache path is missing, empty, partial or keyed to another
    /// node, and on every resume when the spec has no cache paths, since
    /// nothing then proves the workspace warm.
    CachesMissing,
    /// On every resume.
    Always,
}

impl EnvSpec {
    /// Checks the spec's own rules: every unordered list is strictly sorted.
    pub fn validate(&self) -> Result<(), EnvSpecError> {
        let lists = [
            (
                "tools",
                first_out_of_order(self.tools.iter().map(|t| &t.name)),
            ),
            (
                "env_var_names",
                first_out_of_order(self.env_var_names.iter()),
            ),
            ("cache_paths", first_out_of_order(self.cache_paths.iter())),
            (
                "secret_patterns",
                first_out_of_order(self.secret_patterns.iter()),
            ),
        ];
        for (list, index) in lists {
            if let Some(index) = index {
                return Err(EnvSpecError::OutOfOrder { list, index });
            }
        }
        Ok(())
    }

    /// Returns the canonical bytes of a valid spec: the version, then the fields.
    pub fn encode(&self) -> Result<Vec<u8>, EnvSpecError> {
        self.validate()?;
        Ok(codec::encode_versioned(ENV_SPEC_VERSION, self)?)
    }

    /// Decodes bytes written by `encode`, refusing other versions, and checks the result.
    pub fn decode(bytes: &[u8]) -> Result<Self, EnvSpecError> {
        let spec: EnvSpec = codec::decode_versioned(bytes, ENV_SPEC_VERSION)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Returns the spec's identity: blake3 of its canonical bytes.
    pub fn id(&self) -> Result<EnvSpecId, EnvSpecError> {
        Ok(EnvSpecId::of(&self.encode()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> EnvVarName {
        EnvVarName::parse(name).unwrap()
    }

    fn spec() -> EnvSpec {
        EnvSpec {
            tools: vec![Tool {
                name: ToolName::parse("node").unwrap(),
                version: ToolVersion::parse("v22.23.2").unwrap(),
                source: ToolSource::Detected,
            }],
            setup: vec![SetupStep {
                program: ProgramName::parse("npm").unwrap(),
                args: vec![SetupArg::parse("ci").unwrap()],
                cwd: None,
                when: RunWhen::CachesMissing,
            }],
            env_var_names: vec![var("DATABASE_URL")],
            cache_paths: vec![RelPath::parse("node_modules").unwrap()],
            secret_patterns: vec![SecretPattern::parse("*.sqlite").unwrap()],
        }
    }

    #[test]
    fn id_changes_when_the_spec_changes() {
        let a = spec();
        let mut b = spec();
        b.env_var_names.push(var("OTHER"));
        assert_ne!(a.id().unwrap(), b.id().unwrap());
        assert_eq!(a.id().unwrap(), spec().id().unwrap());
    }

    /// A name cannot carry a value: `DATABASE_URL=postgres://u:p@h/db` is
    /// not a name, on the wire included.
    #[test]
    fn an_env_var_name_is_a_name_and_never_a_value() {
        assert!(EnvVarName::parse("DATABASE_URL").is_ok());
        assert!(EnvVarName::parse("_x9").is_ok());
        assert_eq!(EnvVarName::parse(""), Err(EnvVarNameError::Empty));
        assert_eq!(
            EnvVarName::parse("DATABASE_URL=postgres://u:p@h/db"),
            Err(EnvVarNameError::BadChar { index: 12 })
        );
        assert_eq!(
            EnvVarName::parse("9LIVES"),
            Err(EnvVarNameError::BadChar { index: 0 })
        );
        assert_eq!(
            EnvVarName::parse("A B"),
            Err(EnvVarNameError::BadChar { index: 1 })
        );
        let bytes = postcard::to_stdvec("A=b").unwrap();
        assert!(postcard::from_bytes::<EnvVarName>(&bytes).is_err());
    }

    /// One sample per refusal, and the shapes real pin files hold pass.
    #[test]
    fn a_pin_is_short_printable_and_not_a_credential() {
        for ok in [
            "22",
            "v20",
            ">=18 <21",
            "^18 || ^20",
            "1.97.1",
            "lts/*",
            "3.12",
        ] {
            assert!(ToolVersion::parse(ok).is_ok(), "{ok}");
        }
        for ok in ["node", "npm:@angular/cli", "aqua:cli/cli"] {
            assert!(ToolName::parse(ok).is_ok(), "{ok}");
        }
        assert_eq!(ToolVersion::parse(""), Err(PinError::Empty));
        assert_eq!(
            ToolVersion::parse(&"1".repeat(MAX_PIN_LEN + 1)),
            Err(PinError::TooLong)
        );
        assert_eq!(ToolVersion::parse("22\u{7}"), Err(PinError::BadChar));
        assert_eq!(ToolVersion::parse("2\u{e9}"), Err(PinError::BadChar));
        assert_eq!(ToolName::parse("my tool"), Err(PinError::BadChar));
        assert_eq!(ToolName::parse("\u{1b}[31mnode"), Err(PinError::BadChar));
        assert_eq!(
            ToolVersion::parse("https://u:p@registry.example.com/x"),
            Err(PinError::Credential)
        );
        let token = format!("ghp_{}", "q7".repeat(18));
        assert_eq!(ToolName::parse(&token), Err(PinError::Credential));
    }

    /// A peer's spec is checked like a local one: a name that carries a
    /// terminal escape or a token does not decode.
    #[test]
    fn a_bad_tool_name_or_pin_does_not_decode() {
        let token = format!("ghp_{}", "q7".repeat(18));
        for bad in ["\u{1b}[31mnode\n", token.as_str(), ""] {
            let bytes = postcard::to_stdvec(bad).unwrap();
            assert!(postcard::from_bytes::<ToolName>(&bytes).is_err(), "{bad:?}");
            assert!(
                postcard::from_bytes::<ToolVersion>(&bytes).is_err(),
                "{bad:?}"
            );
        }
    }

    /// A peer's step cannot carry a token or a terminal escape to the prompt that prints it.
    #[test]
    fn a_setup_arg_is_printable_and_not_a_credential() {
        for ok in ["", "--frozen-lockfile", "-c", "echo x > out/f", "caf\u{e9}"] {
            assert!(SetupArg::parse(ok).is_ok(), "{ok:?}");
        }
        assert_eq!(SetupArg::parse("\u{1b}[2Jci"), Err(SetupArgError::Control));
        assert_eq!(SetupArg::parse("a\nb"), Err(SetupArgError::Control));
        let token = format!("--token=ghp_{}", "q7".repeat(18));
        assert_eq!(SetupArg::parse(&token), Err(SetupArgError::Credential));
        for bad in ["\u{1b}[31mci", token.as_str()] {
            let bytes = postcard::to_stdvec(bad).unwrap();
            assert!(postcard::from_bytes::<SetupArg>(&bytes).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_secret_pattern_that_gitignore_would_read_differently_is_refused() {
        assert_eq!(SecretPattern::parse(""), Err(SecretPatternError::Empty));
        assert_eq!(
            SecretPattern::parse("/x"),
            Err(SecretPatternError::EmptyComponent)
        );
        assert_eq!(
            SecretPattern::parse("x/"),
            Err(SecretPatternError::EmptyComponent)
        );
        assert_eq!(
            SecretPattern::parse("a/../b"),
            Err(SecretPatternError::DotComponent)
        );
        assert_eq!(
            SecretPattern::parse("**/x"),
            Err(SecretPatternError::DoubleStar)
        );
        assert_eq!(
            SecretPattern::parse("!x"),
            Err(SecretPatternError::BadChar { index: 0 })
        );
        assert_eq!(
            SecretPattern::parse("#x"),
            Err(SecretPatternError::BadChar { index: 0 })
        );
        assert_eq!(
            SecretPattern::parse("x?"),
            Err(SecretPatternError::BadChar { index: 1 })
        );
        assert_eq!(
            SecretPattern::parse("[ab]"),
            Err(SecretPatternError::BadChar { index: 0 })
        );
        assert_eq!(
            SecretPattern::parse("a b"),
            Err(SecretPatternError::BadChar { index: 1 })
        );
        assert_eq!(
            SecretPattern::parse("x\\"),
            Err(SecretPatternError::BadChar { index: 1 })
        );
    }

    #[test]
    fn a_bad_secret_pattern_does_not_decode() {
        let bytes = postcard::to_stdvec("**/x").unwrap();
        assert!(postcard::from_bytes::<SecretPattern>(&bytes).is_err());
    }

    /// Detection reads the override file through the job's patterns, so a
    /// pattern that denies it would drop every override from then on; a
    /// peer's spec that holds one does not decode.
    #[test]
    fn a_pattern_that_denies_the_override_file_is_refused() {
        for pattern in ["*.toml", "env.toml", ".fabric", "*", ".FABRIC/ENV.TOML"] {
            assert_eq!(
                SecretPattern::parse(pattern),
                Err(SecretPatternError::DeniesOverrideFile),
                "{pattern}"
            );
        }
        assert!(SecretPattern::parse("*.sqlite").is_ok());
        let mut s = spec();
        s.secret_patterns = vec![SecretPattern(String::from("*.toml"))];
        let bytes = codec::encode_versioned(ENV_SPEC_VERSION, &s).unwrap();
        assert!(EnvSpec::decode(&bytes).is_err());
    }

    /// Two specs with the same content in another order would have two ids.
    #[test]
    fn unordered_lists_must_be_strictly_sorted() {
        let mut s = spec();
        s.env_var_names = vec![var("B"), var("A")];
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "env_var_names",
                index: 1
            })
        ));
        let mut s = spec();
        s.cache_paths.push(s.cache_paths[0].clone());
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "cache_paths",
                index: 1
            })
        ));
        let mut s = spec();
        s.tools.push(s.tools[0].clone());
        assert!(matches!(
            s.encode(),
            Err(EnvSpecError::OutOfOrder {
                list: "tools",
                index: 1
            })
        ));
        // The same rule holds on the wire: bytes of an unsorted spec, made
        // without validate, do not decode.
        let unsorted = EnvSpec {
            env_var_names: vec![var("B"), var("A")],
            ..spec()
        };
        let bytes = codec::encode_versioned(ENV_SPEC_VERSION, &unsorted).unwrap();
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::OutOfOrder {
                list: "env_var_names",
                index: 1
            })
        ));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes.push(0);
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::Codec(VersionedError::Codec(
                CodecError::TrailingBytes(1)
            )))
        ));
    }

    /// Version 0 is one byte, `0x00`; `0x80 0x00` reads as the same number
    /// and would give the spec a second id.
    #[test]
    fn an_overlong_version_prefix_is_refused() {
        let bytes = spec().encode().unwrap();
        let mut over = vec![0x80, 0x00];
        over.extend_from_slice(&bytes[1..]);
        assert!(matches!(
            EnvSpec::decode(&over),
            Err(EnvSpecError::Codec(VersionedError::Codec(
                CodecError::NonCanonical
            )))
        ));
    }

    #[test]
    fn unknown_version_is_refused() {
        let mut bytes = spec().encode().unwrap();
        bytes[0] = 7;
        assert!(matches!(
            EnvSpec::decode(&bytes),
            Err(EnvSpecError::Codec(VersionedError::UnknownVersion(7)))
        ));
    }
}
