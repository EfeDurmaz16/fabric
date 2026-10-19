//! The override file, `.fabric/env.toml`: what a user declares about a
//! job's environment on top of what detection finds.
//!
//! Invariant: the file names things and never carries a value. An
//! environment variable is a parsed name, a setup argument or tool pin
//! shaped like a credential is refused, and no error from this module
//! quotes text from the file: errors carry key paths, indexes and line
//! numbers only, so a value the user wrote by mistake does not reach a
//! terminal or a log.
//!
//! ```toml
//! package_manager = "pnpm"         # npm, pnpm, yarn or bun; decides between lockfiles
//! env = ["DATABASE_URL"]           # names only, resolved on each node
//! secret_patterns = ["*.sqlite"]   # added to the built-in secret list
//!
//! [tools]                          # a declared pin replaces a detected one
//! node = "22"
//!
//! [[cache]]                        # added to the detected cache paths; when one
//! path = ".output"                 # is missing on resume the setup steps run, so
//! class = "build"                  # name what they rebuild; deps, build or tool-cache
//!                                  # with no cache path at all the steps run every resume
//!
//! [[setup]]                        # any setup entry replaces every detected step
//! program = "pnpm"
//! args = ["install", "--frozen-lockfile"]
//! cwd = "web"                      # optional; the root when absent
//! when = "caches-missing"          # or "always"; caches-missing when absent
//! ```

use fabric_core::env::{EnvVarName, EnvVarNameError, RunWhen, SetupStep};
use fabric_core::harness::{ProgramName, ProgramNameError};
use fabric_core::path::{PathError, RelPath};
use fabric_core::snapshot::CacheClass;
use toml::{Table, Value};

use crate::detect::{PackageManagerName, PinError, check_pin};
use crate::secret::{self, Pattern, PatternError};

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
        reason: PatternError,
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
    /// Two `cache` entries name the same path.
    #[error("cache path `{path}` is declared twice")]
    DuplicateCache {
        /// The repeated path.
        path: RelPath,
    },
    /// A tool pin that is not a short printable version, or that is shaped like a credential.
    #[error("`{key}` is not a version pin")]
    Pin {
        /// The dotted path of the key.
        key: String,
        /// What is wrong with it.
        #[source]
        reason: PinError,
    },
}

/// A cache path the user declares, with the class the store gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredCache {
    /// The path, relative to the workspace root.
    pub path: RelPath,
    /// Its class for the snapshot policy.
    pub class: CacheClass,
}

/// The parsed override file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverride {
    /// The package manager to use when the workspace alone does not decide.
    pub package_manager: Option<PackageManagerName>,
    /// Names of environment variables the job needs, sorted, without repeats.
    pub env_var_names: Vec<EnvVarName>,
    /// Patterns added to the built-in secret list.
    pub secret_patterns: Vec<Pattern>,
    /// Declared tool pins as `(tool, version)`, sorted by tool.
    pub tools: Vec<(String, String)>,
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
                _ => return Err(OverrideError::UnknownKey { key: key.clone() }),
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
    names.sort();
    names.dedup();
    Ok(names)
}

fn patterns(value: &Value) -> Result<Vec<Pattern>, OverrideError> {
    strings(value, "secret_patterns")?
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            Pattern::parse(text).map_err(|reason| OverrideError::Pattern { index, reason })
        })
        .collect()
}

fn tools(value: &Value) -> Result<Vec<(String, String)>, OverrideError> {
    // A TOML table iterates in key order, so the result is sorted by tool.
    table(value, "tools")?
        .iter()
        .map(|(tool, version)| {
            let key = format!("tools.{tool}");
            let version = string(version, &key)?;
            check_pin(version).map_err(|reason| OverrideError::Pin { key, reason })?;
            Ok((tool.clone(), version.to_owned()))
        })
        .collect()
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
        reject_unknown(entry, &at, &["path", "class"])?;
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
        if caches.iter().any(|c| c.path == path) {
            return Err(OverrideError::DuplicateCache { path });
        }
        caches.push(DeclaredCache { path, class });
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
        if let Some(arg) = args.iter().position(|a| secret::looks_like_credential(a)) {
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

fn reject_unknown(entry: &Table, at: &str, known: &[&str]) -> Result<(), OverrideError> {
    match entry.keys().find(|k| !known.contains(&k.as_str())) {
        Some(key) => Err(OverrideError::UnknownKey {
            key: format!("{at}.{key}"),
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
env = ["OPENAI_API_KEY", "DATABASE_URL", "DATABASE_URL"]
secret_patterns = ["*.sqlite"]

[tools]
pnpm = "11.23.0"
node = "22"

[[cache]]
path = ".output"
class = "build"

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
        assert_eq!(
            parsed.tools,
            [
                ("node".to_owned(), "22".to_owned()),
                ("pnpm".to_owned(), "11.23.0".to_owned())
            ]
        );
        assert_eq!(
            parsed.caches,
            [DeclaredCache {
                path: RelPath::parse(".output").unwrap(),
                class: CacheClass::Build
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
        let text =
            "[[cache]]\npath = \"a\"\nclass = \"deps\"\n[[cache]]\npath = \"a\"\nclass = \"build\"";
        assert!(matches!(
            EnvOverride::parse(text),
            Err(OverrideError::DuplicateCache { .. })
        ));
    }

    #[test]
    fn a_syntax_error_reports_its_line() {
        match EnvOverride::parse("env = []\n\nnode = \n") {
            Err(OverrideError::Syntax { line, .. }) => assert_eq!(line, 3),
            other => panic!("{other:?}"),
        }
    }
}
