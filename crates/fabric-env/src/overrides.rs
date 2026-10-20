//! The override file, `.fabric/env.toml`: what a user declares about a
//! job's environment on top of what detection finds.
//!
//! Invariant: the file names things and never carries a value. An
//! environment variable is a parsed name, a setup argument or tool pin
//! shaped like a credential is refused, and no error from this module
//! quotes a value from the file: errors carry key paths, indexes and line
//! numbers. The one text from the file an error may quote is an unknown
//! key's name, and only when it is short, printable and not shaped like a
//! credential, so a value the user wrote by mistake does not reach a
//! terminal or a log.
//!
//! ```toml
//! package_manager = "pnpm"         # npm, pnpm, yarn or bun; decides between lockfiles
//! env = ["DATABASE_URL"]           # names only, resolved on each node
//! secret_patterns = ["*.sqlite"]   # added to the built-in secret list
//!
//! [tools]                          # a declared pin replaces a detected one;
//! node = "22"                      # nodejs reads as node and golang as go, as in .tool-versions
//!
//! [[cache]]                        # added to the detected cache paths; when one
//! path = ".output"                 # is missing on resume the setup steps run, so
//! class = "build"                  # name what they rebuild; deps, build or tool-cache
//! ecosystem = "node"               # optional: node, rust, python or other; picks the
//!                                  # carry key; the single detected ecosystem when absent
//!                                  # with no cache path at all the steps run every resume
//!
//! [[setup]]                        # any setup entry replaces every detected step
//! program = "pnpm"
//! args = ["install", "--frozen-lockfile"]
//! cwd = "web"                      # optional; the root when absent
//! when = "caches-missing"          # or "always"; caches-missing when absent
//! ```

use std::collections::BTreeMap;

use fabric_core::credential::looks_like_credential;
use fabric_core::env::{
    EnvVarName, EnvVarNameError, PinError, RunWhen, SecretPattern, SecretPatternError, SetupStep,
    ToolName, ToolVersion,
};
use fabric_core::harness::{ProgramName, ProgramNameError};
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::CacheClass;
use toml::{Table, Value};

use crate::secret::pattern_matches;

/// Where the override file lives, relative to the workspace root.
pub const OVERRIDE_FILE: &str = ".fabric/env.toml";

/// The directory that holds Fabric's own files in a workspace; never a cache.
const FABRIC_DIR: &str = ".fabric";

/// Why the override file was refused.
#[derive(Debug, thiserror::Error)]
pub enum OverrideError {
    /// The text is not TOML.
    #[error("not valid TOML at line {line}, column {column}")]
    Syntax {
        /// One-based line of the error.
        line: usize,
        /// One-based column of the error, in characters.
        column: usize,
    },
    /// A key this file does not define.
    #[error("unknown key `{key}`")]
    UnknownKey {
        /// The dotted path of the key.
        key: String,
    },
    /// A key whose value has the wrong TOML type.
    #[error("`{key}` must be {expected}")]
    WrongType {
        /// The dotted path of the key.
        key: String,
        /// What the key takes.
        expected: &'static str,
    },
    /// A string that is not one of the values a key takes.
    #[error("`{key}` must be one of: {expected}")]
    UnknownValue {
        /// The dotted path of the key.
        key: String,
        /// The values the key takes.
        expected: &'static str,
    },
    /// A required key is absent.
    #[error("`{key}` is required")]
    MissingKey {
        /// The dotted path of the key.
        key: String,
    },
    /// An entry of `env` is not a variable name; a `NAME=value` pair lands here.
    #[error("`env[{index}]` is not an environment variable name")]
    EnvVarName {
        /// Zero-based index in `env`.
        index: usize,
        /// What is wrong with it.
        #[source]
        reason: EnvVarNameError,
    },
    /// An entry of `env` repeats an earlier one.
    #[error("`env[{index}]` repeats an earlier name")]
    DuplicateEnv {
        /// Zero-based index of the repeat.
        index: usize,
    },
    /// A path that is not a relative workspace path.
    #[error("`{key}` is not a relative path inside the workspace")]
    Path {
        /// The dotted path of the key.
        key: String,
        /// What is wrong with it.
        #[source]
        reason: PathError,
    },
    /// A setup program that is not a bare name to look up on `PATH`.
    #[error("`setup[{index}].program` must be a program name, not a path or an option")]
    Program {
        /// Zero-based index in `setup`.
        index: usize,
        /// What is wrong with it.
        #[source]
        reason: ProgramNameError,
    },
    /// An entry of `secret_patterns` that is not a pattern.
    #[error("`secret_patterns[{index}]` is not a secret pattern")]
    Pattern {
        /// Zero-based index in `secret_patterns`.
        index: usize,
        /// What is wrong with it.
        #[source]
        reason: SecretPatternError,
    },
    /// An entry of `secret_patterns` that repeats an earlier one, ignoring ASCII case as matching does.
    #[error("`secret_patterns[{index}]` repeats an earlier pattern")]
    DuplicatePattern {
        /// Zero-based index of the repeat.
        index: usize,
    },
    /// An entry of `secret_patterns` that denies the override file itself.
    ///
    /// Detection reads the override file through the job's patterns, so
    /// such a pattern would drop every override from the next detection on.
    #[error(
        "`secret_patterns[{index}]` matches {OVERRIDE_FILE}, which detection must be able to read"
    )]
    PatternDeniesOverrideFile {
        /// Zero-based index in `secret_patterns`.
        index: usize,
    },
    /// A setup argument shaped like a credential; the spec travels to other nodes.
    #[error(
        "`setup[{step}].args[{arg}]` looks like a credential; pass it as an `env` name and let the program read it"
    )]
    CredentialInArgs {
        /// Zero-based index in `setup`.
        step: usize,
        /// Zero-based index in that step's `args`.
        arg: usize,
    },
    /// Two `cache` entries name the same path, ignoring ASCII case.
    #[error("`cache[{index}].path` repeats an earlier cache path")]
    DuplicateCache {
        /// Zero-based index of the later entry in `cache`.
        index: usize,
    },
    /// A `cache` entry under `.fabric`, which holds Fabric's own files and is never a cache.
    #[error("`{key}` is under `.fabric`, which is never a cache")]
    ReservedCache {
        /// The dotted path of the key.
        key: String,
    },
    /// A `cache` entry that lies inside, or contains, another declared cache.
    #[error(
        "`cache[{index}].path` nests with `cache[{other}].path`; a cache may not contain another"
    )]
    NestedCache {
        /// Zero-based index of the later entry in `cache`.
        index: usize,
        /// Zero-based index of the entry it nests with.
        other: usize,
    },
    /// A `[tools]` key that is not a tool name; the name itself is never quoted.
    #[error("the name of `tools[{index}]` {reason}")]
    ToolName {
        /// Zero-based index of the entry in `[tools]`, in file order.
        index: usize,
        /// What is wrong with it.
        reason: PinError,
    },
    /// A `[tools]` key that names the same tool as an earlier one, such as `nodejs` next to `node`.
    #[error("`tools[{index}]` names a tool an earlier entry already pins")]
    DuplicateTool {
        /// Zero-based index of the later entry in `[tools]`, in file order.
        index: usize,
    },
    /// A `[tools]` value that is not a version pin; the pin itself is never quoted.
    #[error("the version of `tools[{index}]` {reason}")]
    Pin {
        /// Zero-based index of the entry in `[tools]`, in file order.
        index: usize,
        /// What is wrong with it.
        reason: PinError,
    },
}

/// A JavaScript package manager as package.json and the override file name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManagerName {
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn, either line; the workspace decides which.
    Yarn,
    /// Bun.
    Bun,
}

impl PackageManagerName {
    /// Parses `npm`, `pnpm`, `yarn` or `bun`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "yarn" => Some(Self::Yarn),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    /// Returns the name as package.json writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
}

/// A cache path the user declares, with the class the store gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredCache {
    /// The path, relative to the workspace root.
    pub path: RelPath,
    /// Its class for the snapshot policy.
    pub class: CacheClass,
    /// The ecosystem that produces it, which picks its carry key; `None` lets detection choose.
    pub ecosystem: Option<DeclaredEcosystem>,
}

/// The ecosystem a `[[cache]]` entry names in `ecosystem`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredEcosystem {
    /// Made by the detected JavaScript package manager's toolchain.
    Node,
    /// Made by Cargo.
    Rust,
    /// Made by the workspace's Python.
    Python,
    /// Made by nothing an ecosystem rule knows; carried when the platform matches.
    Other,
}

/// The parsed override file.
///
/// Crate-private: `detect` is its only reader, and its sorted, repeat-free
/// lists are kept so by `parse`, not by the type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EnvOverride {
    /// The package manager to use when the workspace alone does not decide.
    pub package_manager: Option<PackageManagerName>,
    /// Names of environment variables the job needs, sorted; a repeat is refused.
    pub env_var_names: Vec<EnvVarName>,
    /// Patterns added to the built-in secret list.
    pub secret_patterns: Vec<SecretPattern>,
    /// Declared tool pins, by tool name.
    pub tools: BTreeMap<ToolName, ToolVersion>,
    /// Declared cache paths, in file order.
    pub caches: Vec<DeclaredCache>,
    /// Declared setup steps; `Some` replaces every detected step, even when empty.
    pub setup: Option<Vec<SetupStep>>,
}

impl EnvOverride {
    /// Parses the text of an override file.
    pub fn parse(text: &str) -> Result<Self, OverrideError> {
        let table: Table = text.parse().map_err(|err: toml::de::Error| {
            let offset = err.span().map_or(0, |span| span.start);
            let (line, column) = line_and_column(text, offset);
            OverrideError::Syntax { line, column }
        })?;
        let mut parsed = EnvOverride::default();
        for (key, value) in &table {
            match key.as_str() {
                "package_manager" => {
                    let name = string(value, key)?;
                    let manager = PackageManagerName::parse(name).ok_or_else(|| {
                        OverrideError::UnknownValue {
                            key: key.clone(),
                            expected: "npm, pnpm, yarn, bun",
                        }
                    })?;
                    parsed.package_manager = Some(manager);
                }
                "env" => parsed.env_var_names = env_names(value)?,
                "secret_patterns" => parsed.secret_patterns = patterns(value)?,
                "tools" => parsed.tools = tools(value)?,
                "cache" => parsed.caches = caches(value)?,
                "setup" => parsed.setup = Some(steps(value)?),
                _ => {
                    return Err(OverrideError::UnknownKey {
                        key: shown(key).to_owned(),
                    });
                }
            }
        }
        Ok(parsed)
    }
}

/// Returns the one-based line and character column of a byte offset in `text`.
pub(crate) fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, OverrideError> {
    value.as_str().ok_or_else(|| OverrideError::WrongType {
        key: key.to_owned(),
        expected: "a string",
    })
}

fn array<'a>(
    value: &'a Value,
    key: &str,
    expected: &'static str,
) -> Result<&'a [Value], OverrideError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| OverrideError::WrongType {
            key: key.to_owned(),
            expected,
        })
}

fn table<'a>(value: &'a Value, key: &str) -> Result<&'a Table, OverrideError> {
    value.as_table().ok_or_else(|| OverrideError::WrongType {
        key: key.to_owned(),
        expected: "a table",
    })
}

fn strings<'a>(value: &'a Value, key: &str) -> Result<Vec<&'a str>, OverrideError> {
    array(value, key, "an array of strings")?
        .iter()
        .enumerate()
        .map(|(index, item)| string(item, &format!("{key}[{index}]")))
        .collect()
}

fn env_names(value: &Value) -> Result<Vec<EnvVarName>, OverrideError> {
    let mut names = strings(value, "env")?
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            EnvVarName::parse(text).map_err(|reason| OverrideError::EnvVarName { index, reason })
        })
        .collect::<Result<Vec<_>, _>>()?;
    // A repeat is refused, as in every list of this file, so no entry is dropped unseen.
    let mut seen: Vec<&EnvVarName> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if seen.contains(&name) {
            return Err(OverrideError::DuplicateEnv { index });
        }
        seen.push(name);
    }
    names.sort();
    Ok(names)
}

fn patterns(value: &Value) -> Result<Vec<SecretPattern>, OverrideError> {
    let mut patterns: Vec<SecretPattern> = Vec::new();
    for (index, text) in strings(value, "secret_patterns")?.into_iter().enumerate() {
        let pattern = SecretPattern::parse(text)
            .map_err(|reason| OverrideError::Pattern { index, reason })?;
        if pattern_matches(&pattern, OVERRIDE_FILE) {
            return Err(OverrideError::PatternDeniesOverrideFile { index });
        }
        if patterns
            .iter()
            .any(|p| p.as_str().eq_ignore_ascii_case(pattern.as_str()))
        {
            return Err(OverrideError::DuplicatePattern { index });
        }
        patterns.push(pattern);
    }
    Ok(patterns)
}

/// Parses `[tools]`, reading each name through its alias; two keys for one tool are refused.
fn tools(value: &Value) -> Result<BTreeMap<ToolName, ToolVersion>, OverrideError> {
    let mut tools = BTreeMap::new();
    for (index, (tool, version)) in table(value, "tools")?.iter().enumerate() {
        let tool = ToolName::parse(canonical_tool(tool))
            .map_err(|reason| OverrideError::ToolName { index, reason })?;
        let version = string(version, &format!("tools[{index}]"))?;
        let version =
            ToolVersion::parse(version).map_err(|reason| OverrideError::Pin { index, reason })?;
        if tools.insert(tool, version).is_some() {
            return Err(OverrideError::DuplicateTool { index });
        }
    }
    Ok(tools)
}

/// Reads `nodejs` as `node` and `golang` as `go`, as asdf and mise do, in every file that names tools.
pub(crate) fn canonical_tool(name: &str) -> &str {
    match name {
        "nodejs" => "node",
        "golang" => "go",
        other => other,
    }
}

fn rel_path(value: &Value, key: &str) -> Result<RelPath, OverrideError> {
    RelPath::parse(string(value, key)?).map_err(|reason| OverrideError::Path {
        key: key.to_owned(),
        reason,
    })
}

fn caches(value: &Value) -> Result<Vec<DeclaredCache>, OverrideError> {
    let mut caches: Vec<DeclaredCache> = Vec::new();
    for (index, item) in array(value, "cache", "an array of tables")?
        .iter()
        .enumerate()
    {
        let at = format!("cache[{index}]");
        let entry = table(item, &at)?;
        reject_unknown(entry, &at, &["path", "class", "ecosystem"])?;
        let path_key = format!("{at}.path");
        let path = rel_path(required(entry, "path", &path_key)?, &path_key)?;
        let class_key = format!("{at}.class");
        let class = match string(required(entry, "class", &class_key)?, &class_key)? {
            "deps" => CacheClass::Deps,
            "build" => CacheClass::Build,
            "tool-cache" => CacheClass::ToolCache,
            _ => {
                return Err(OverrideError::UnknownValue {
                    key: class_key,
                    expected: "deps, build, tool-cache",
                });
            }
        };
        let ecosystem = match entry.get("ecosystem") {
            None => None,
            Some(value) => {
                let key = format!("{at}.ecosystem");
                Some(match string(value, &key)? {
                    "node" => DeclaredEcosystem::Node,
                    "rust" => DeclaredEcosystem::Rust,
                    "python" => DeclaredEcosystem::Python,
                    "other" => DeclaredEcosystem::Other,
                    _ => {
                        return Err(OverrideError::UnknownValue {
                            key,
                            expected: "node, rust, python, other",
                        });
                    }
                })
            }
        };
        if caches.iter().any(|c| c.path.eq_ignore_ascii_case(&path)) {
            return Err(OverrideError::DuplicateCache { index });
        }
        // A cache is dropped on a carry mismatch, so a path that holds
        // Fabric's own files or the user's work must not become one by accident.
        if path
            .as_str()
            .split('/')
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(FABRIC_DIR))
        {
            return Err(OverrideError::ReservedCache { key: path_key });
        }
        if let Some(other) = caches.iter().position(|c| {
            path.starts_with_ignore_ascii_case(&c.path)
                || c.path.starts_with_ignore_ascii_case(&path)
        }) {
            return Err(OverrideError::NestedCache { index, other });
        }
        caches.push(DeclaredCache {
            path,
            class,
            ecosystem,
        });
    }
    Ok(caches)
}

fn steps(value: &Value) -> Result<Vec<SetupStep>, OverrideError> {
    let mut steps = Vec::new();
    for (index, item) in array(value, "setup", "an array of tables")?
        .iter()
        .enumerate()
    {
        let at = format!("setup[{index}]");
        let entry = table(item, &at)?;
        reject_unknown(entry, &at, &["program", "args", "cwd", "when"])?;
        let program_key = format!("{at}.program");
        let program = string(required(entry, "program", &program_key)?, &program_key)?;
        let program = ProgramName::parse(program)
            .map_err(|reason| OverrideError::Program { index, reason })?;
        let args = match entry.get("args") {
            Some(args) => strings(args, &format!("{at}.args"))?,
            None => Vec::new(),
        };
        if let Some(arg) = args.iter().position(|a| looks_like_credential(a)) {
            return Err(OverrideError::CredentialInArgs { step: index, arg });
        }
        let cwd = match entry.get("cwd") {
            Some(cwd) => Some(rel_path(cwd, &format!("{at}.cwd"))?),
            None => None,
        };
        let when = match entry.get("when") {
            None => RunWhen::CachesMissing,
            Some(when) => {
                let when_key = format!("{at}.when");
                match string(when, &when_key)? {
                    "caches-missing" => RunWhen::CachesMissing,
                    "always" => RunWhen::Always,
                    _ => {
                        return Err(OverrideError::UnknownValue {
                            key: when_key,
                            expected: "caches-missing, always",
                        });
                    }
                }
            }
        };
        steps.push(SetupStep {
            program,
            args: args.into_iter().map(str::to_owned).collect(),
            cwd,
            when,
        });
    }
    Ok(steps)
}

fn required<'a>(entry: &'a Table, name: &str, key: &str) -> Result<&'a Value, OverrideError> {
    entry.get(name).ok_or_else(|| OverrideError::MissingKey {
        key: key.to_owned(),
    })
}

/// Returns `key` when it is safe to quote in an error, or a placeholder.
///
/// A key the file wrote is quoted only when it passes the rule a tool name
/// does: at most `MAX_PIN_LEN` printable ASCII characters with no space
/// and not shaped like a credential, so a token typed where a key belongs
/// never reaches a terminal or a log.
fn shown(key: &str) -> &str {
    if ToolName::parse(key).is_ok() {
        key
    } else {
        "<key not shown>"
    }
}

fn reject_unknown(entry: &Table, at: &str, known: &[&str]) -> Result<(), OverrideError> {
    match entry.keys().find(|k| !known.contains(&k.as_str())) {
        Some(key) => Err(OverrideError::UnknownKey {
            key: format!("{at}.{}", shown(key)),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_file_parses() {
        let parsed = EnvOverride::parse(
            r#"
package_manager = "pnpm"
env = ["OPENAI_API_KEY", "DATABASE_URL"]
secret_patterns = ["*.sqlite"]

[tools]
pnpm = "11.23.0"
node = "22"

[[cache]]
path = ".output"
class = "build"
ecosystem = "node"

[[setup]]
program = "pnpm"
args = ["install", "--frozen-lockfile"]
cwd = "web"

[[setup]]
program = "pnpm"
args = ["run", "codegen"]
when = "always"
"#,
        )
        .unwrap();
        assert_eq!(parsed.package_manager, Some(PackageManagerName::Pnpm));
        let names: Vec<&str> = parsed
            .env_var_names
            .iter()
            .map(EnvVarName::as_str)
            .collect();
        assert_eq!(names, ["DATABASE_URL", "OPENAI_API_KEY"]);
        assert_eq!(parsed.secret_patterns[0].as_str(), "*.sqlite");
        let tools: Vec<(&str, &str)> = parsed
            .tools
            .iter()
            .map(|(name, version)| (name.as_str(), version.as_str()))
            .collect();
        assert_eq!(tools, [("node", "22"), ("pnpm", "11.23.0")]);
        assert_eq!(
            parsed.caches,
            [DeclaredCache {
                path: RelPath::parse(".output").unwrap(),
                class: CacheClass::Build,
                ecosystem: Some(DeclaredEcosystem::Node),
            }]
        );
        let setup = parsed.setup.unwrap();
        assert_eq!(setup.len(), 2);
        assert_eq!(setup[0].cwd, Some(RelPath::parse("web").unwrap()));
        assert_eq!(setup[0].when, RunWhen::CachesMissing);
        assert_eq!(setup[1].when, RunWhen::Always);
        assert_eq!(setup[1].args, ["run", "codegen"]);
    }

    /// `setup = []` declares that nothing runs, which differs from no
    /// `setup` key, where detection's steps stay.
    #[test]
    fn an_empty_setup_list_is_a_declaration() {
        assert_eq!(
            EnvOverride::parse("setup = []").unwrap().setup,
            Some(vec![])
        );
        assert_eq!(EnvOverride::parse("").unwrap().setup, None);
    }

    /// The error names where the mistake is and never what was written there.
    #[test]
    fn a_value_written_where_a_name_belongs_is_refused_without_echoing_it() {
        let secret = "postgres://admin:hunter2@db.internal/prod";
        let cases = [
            format!("env = [\"DATABASE_URL={secret}\"]"),
            format!("env = \"{secret}\""),
            format!("[env]\nDATABASE_URL = \"{secret}\""),
            format!("package_manager = \"{secret}\""),
            format!("[[setup]]\nprogram = \"pnpm\"\nargs = [\"--registry={secret}\"]"),
            format!("[[setup]]\nprogram = \"{secret}\""),
            format!("[[cache]]\npath = \"/{secret}\"\nclass = \"deps\""),
            format!("[[cache]]\npath = \"x\"\nclass = \"{secret}\""),
            format!("tools = {{ node = [\"{secret}\"] }}"),
            format!("env = [\"{secret}"),
            format!("\"{secret}\" = 1"),
            format!("[[cache]]\npath = \"x\"\nclass = \"deps\"\n\"{secret}\" = 1"),
            format!(
                "[[cache]]\npath = \"{secret}\"\nclass = \"deps\"\n[[cache]]\npath = \"{secret}\"\nclass = \"deps\""
            ),
        ];
        for text in cases {
            let err = EnvOverride::parse(&text).unwrap_err();
            let shown = format!("{err} {err:?}");
            assert!(!shown.contains("hunter2"), "{shown}");
        }
        assert!(matches!(
            EnvOverride::parse("env = [\"A\", \"B=c\"]"),
            Err(OverrideError::EnvVarName { index: 1, .. })
        ));
    }

    /// `nodejs` reads as `node`, so pinning both would drop one pin unseen.
    #[test]
    fn a_tool_pinned_twice_under_an_alias_is_refused() {
        assert!(matches!(
            EnvOverride::parse("[tools]\nnode = \"22\"\nnodejs = \"20\"\n"),
            Err(OverrideError::DuplicateTool { index: 1 })
        ));
    }

    /// The index is the user's only pointer to a refused entry, since the
    /// text is never quoted, so it counts entries in the order the file has them.
    #[test]
    fn a_tools_entry_is_named_by_its_place_in_the_file() {
        let token = format!("ghp_{}", "x".repeat(36));
        let text = format!("[tools]\nzeta = \"1\"\nalpha = \"{token}\"\n");
        assert!(matches!(
            EnvOverride::parse(&text),
            Err(OverrideError::Pin {
                index: 1,
                reason: PinError::Credential
            })
        ));
    }

    #[test]
    fn a_repeated_env_name_is_refused() {
        assert!(matches!(
            EnvOverride::parse("env = [\"A\", \"B\", \"A\"]"),
            Err(OverrideError::DuplicateEnv { index: 2 })
        ));
    }

    /// Matching ignores ASCII case, so `*.DB` repeats `*.db`.
    #[test]
    fn a_repeated_secret_pattern_is_refused() {
        for text in [
            "secret_patterns = [\"*.db\", \"*.sqlite\", \"*.db\"]",
            "secret_patterns = [\"*.db\", \"*.sqlite\", \"*.DB\"]",
        ] {
            assert!(
                matches!(
                    EnvOverride::parse(text),
                    Err(OverrideError::DuplicatePattern { index: 2 })
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn a_credential_in_setup_arguments_is_refused() {
        let token = format!("ghp_{}", "x".repeat(36));
        let text = format!(
            "[[setup]]\nprogram = \"npm\"\nargs = [\"ci\"]\n[[setup]]\nprogram = \"npm\"\nargs = [\"run\", \"--token={token}\"]"
        );
        assert!(matches!(
            EnvOverride::parse(&text),
            Err(OverrideError::CredentialInArgs { step: 1, arg: 1 })
        ));
    }

    #[test]
    fn unknown_keys_are_refused_at_every_level() {
        for (text, key) in [
            ("packagemanager = \"pnpm\"", "packagemanager"),
            (
                "[[setup]]\nprogram = \"x\"\nshell = \"sh\"",
                "setup[0].shell",
            ),
            (
                "[[cache]]\npath = \"x\"\nclass = \"deps\"\nsize = 1",
                "cache[0].size",
            ),
        ] {
            match EnvOverride::parse(text) {
                Err(OverrideError::UnknownKey { key: found }) => assert_eq!(found, key),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_setup_program_must_be_a_bare_name() {
        assert!(matches!(
            EnvOverride::parse("[[setup]]\nprogram = \"./install.sh\""),
            Err(OverrideError::Program {
                index: 0,
                reason: ProgramNameError::Slash
            })
        ));
        assert!(matches!(
            EnvOverride::parse("[[setup]]\nargs = [\"x\"]"),
            Err(OverrideError::MissingKey { .. })
        ));
    }

    #[test]
    fn a_cache_path_declared_twice_is_refused() {
        for second in ["a", "A"] {
            let text = format!(
                "[[cache]]\npath = \"a\"\nclass = \"deps\"\n[[cache]]\npath = \"{second}\"\nclass = \"build\""
            );
            assert!(matches!(
                EnvOverride::parse(&text),
                Err(OverrideError::DuplicateCache { .. })
            ));
        }
    }

    /// A cache is dropped on a carry mismatch, so Fabric's own directory
    /// and a path that nests with another cache are refused; a sibling is fine.
    #[test]
    fn a_cache_under_fabric_or_nested_in_another_cache_is_refused() {
        for path in [".fabric", ".fabric/jobs", ".FABRIC", ".Fabric/jobs"] {
            let text = format!("[[cache]]\npath = \"{path}\"\nclass = \"build\"");
            match EnvOverride::parse(&text) {
                Err(OverrideError::ReservedCache { key }) => assert_eq!(key, "cache[0].path"),
                other => panic!("{path}: {other:?}"),
            }
        }
        for (first, second) in [
            ("out", "out/cache"),
            ("out/cache", "out"),
            ("Out", "out/cache"),
        ] {
            let text = format!(
                "[[cache]]\npath = \"{first}\"\nclass = \"build\"\n[[cache]]\npath = \"{second}\"\nclass = \"build\""
            );
            match EnvOverride::parse(&text) {
                Err(OverrideError::NestedCache { index, other }) => {
                    assert_eq!((index, other), (1, 0), "{first} {second}");
                }
                other => panic!("{first} {second}: {other:?}"),
            }
        }
        let siblings = "[[cache]]\npath = \"out\"\nclass = \"build\"\n[[cache]]\npath = \"outer\"\nclass = \"build\"";
        assert_eq!(EnvOverride::parse(siblings).unwrap().caches.len(), 2);
    }

    /// Detection reads the override file through the job's patterns, so a
    /// pattern that denies it would drop every override from then on.
    #[test]
    fn a_pattern_that_denies_the_override_file_is_refused() {
        for pattern in ["*.toml", "env.toml", ".fabric", "*", ".FABRIC/ENV.TOML"] {
            let text = format!("secret_patterns = [\"*.sqlite\", \"{pattern}\"]");
            assert!(
                matches!(
                    EnvOverride::parse(&text),
                    Err(OverrideError::PatternDeniesOverrideFile { index: 1 })
                ),
                "{pattern}"
            );
        }
    }

    #[test]
    fn a_syntax_error_reports_its_line() {
        match EnvOverride::parse("env = []\n\nnode = \n") {
            Err(OverrideError::Syntax { line, .. }) => assert_eq!(line, 3),
            other => panic!("{other:?}"),
        }
    }
}
