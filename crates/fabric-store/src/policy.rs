//! The ignored-file policy (ADR 0004, Ignored files by class).
//!
//! Invariant: every path gets exactly one class, a path that matches a
//! secret pattern is `Secret` whatever else it matches, and the secret list
//! can be widened per job but never narrowed. Under a dependency, build or
//! tool-cache directory only the high-confidence patterns apply, so a
//! `node_modules` is carried whole or left behind, never with holes it does
//! not know about. A generic directory name such as `out` or `build` is a
//! cache only next to the file of the tool that makes it, so an ignored
//! `experiments/out/` with no `package.json` beside it stays precious. A
//! cache the job declares by its path is a cache whatever its name, and
//! keeps the full secret list (ADR 0007, The override file). Names are
//! compared with ASCII case folded, as APFS and git on a Mac compare them,
//! so `Server.PEM` is as secret as `server.pem`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use fabric_core::secret::{Scope, SecretPattern, SecretPolicy};
use fabric_core::snapshot::CacheClass;

/// The built-in secret patterns of `fabric_core::secret`, as text, for the
/// exclude file capture hands `git add -A` (`IgnorePolicy::gitignore_lines`).
///
/// Which path is a secret is decided by `fabric_core::secret::SecretPolicy`
/// alone (M2-03): this list only keeps most secrets out of the user's
/// object store before that decision, and a path it names that the core
/// carries, such as `.env.example`, is added back. It must name at least
/// what the core denies; a test pins it to the core's samples.
pub const SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".envrc",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".aws",
    ".ssh",
    ".gnupg",
    ".env.*",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ecdsa_sk",
    "id_ed25519_sk",
    "credentials",
    "credentials.json",
    "credentials.yml",
    "credentials.yaml",
    "credentials.toml",
    ".credentials*",
    "*.env",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.tfstate",
    "*.tfstate.*",
    ".docker/config.json",
];

/// A directory name that holds a cache, and the files that make it one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CacheDir {
    /// The directory name.
    pub name: &'static str,
    /// The class of everything under it.
    pub class: CacheClass,
    /// Files of which one must sit beside the directory; empty when only a tool makes the name.
    pub markers: &'static [&'static str],
}

const fn cache_dir(
    name: &'static str,
    class: CacheClass,
    markers: &'static [&'static str],
) -> CacheDir {
    CacheDir {
        name,
        class,
        markers,
    }
}

const NODE: &[&str] = &["package.json"];
const PYTHON_OR_NODE: &[&str] = &["package.json", "pyproject.toml", "setup.py"];

/// The cache directories every policy starts with.
pub(crate) const CACHE_DIRS: &[CacheDir] = &[
    cache_dir("node_modules", CacheClass::Deps, &[]),
    cache_dir(".pnpm-store", CacheClass::Deps, &[]),
    cache_dir(".venv", CacheClass::Deps, &[]),
    cache_dir(
        "vendor",
        CacheClass::Deps,
        &["go.mod", "composer.json", "Gemfile"],
    ),
    cache_dir("target", CacheClass::Build, &["Cargo.toml", "pom.xml"]),
    cache_dir("dist", CacheClass::Build, PYTHON_OR_NODE),
    cache_dir(
        "build",
        CacheClass::Build,
        &[
            "package.json",
            "pyproject.toml",
            "setup.py",
            "build.gradle",
            "build.gradle.kts",
            "CMakeLists.txt",
        ],
    ),
    cache_dir("out", CacheClass::Build, NODE),
    cache_dir(".next", CacheClass::Build, &[]),
    cache_dir(".turbo", CacheClass::Build, &[]),
    cache_dir("__pycache__", CacheClass::ToolCache, &[]),
    cache_dir(".mypy_cache", CacheClass::ToolCache, &[]),
    cache_dir(".pytest_cache", CacheClass::ToolCache, &[]),
    cache_dir(".ruff_cache", CacheClass::ToolCache, &[]),
    cache_dir(".cache", CacheClass::ToolCache, NODE),
];

/// Answers whether a workspace directory holds an entry of a given name.
pub trait Markers {
    /// True when the directory at `dir` (root-relative, empty for the root) holds `name`.
    fn has(&mut self, dir: &str, name: &str) -> bool;
}

impl<F: FnMut(&str, &str) -> bool> Markers for F {
    fn has(&mut self, dir: &str, name: &str) -> bool {
        self(dir, name)
    }
}

/// The file in a workspace root that widens the policy for one job.
pub const FABRICIGNORE: &str = ".fabricignore";

/// Why the policy could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// The `.fabricignore` file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A pattern in `.fabricignore` is empty, absolute, or uses syntax the matcher does not model.
    #[error("{path}:{line}: pattern {text:?} is not valid")]
    BadPattern {
        /// The file the pattern came from.
        path: PathBuf,
        /// The one-based line.
        line: usize,
        /// The pattern text.
        text: String,
    },
    /// A pattern a caller added is empty, absolute, or uses syntax the matcher does not model.
    #[error("pattern {text:?} is not valid")]
    BadPolicyPattern {
        /// The pattern text.
        text: String,
    },
}

/// A cache directory and the class of everything under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRoot {
    /// The directory, relative to the workspace root.
    pub path: RelPath,
    /// Its class.
    pub class: CacheClass,
}

/// The class a path gets, with the cache directory it lies under when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// Matches a secret pattern; never carried.
    Secret {
        /// The cache directory the path lies under.
        under: Option<CacheRoot>,
    },
    /// Matches a pattern in `.fabricignore`; not carried for this job.
    Excluded {
        /// The cache directory the path lies under.
        under: Option<CacheRoot>,
    },
    /// Below a dependency, build output or tool cache directory; carried.
    Cache(CacheRoot),
    /// Ignored by git and nobody's cache: the user's work.
    Precious,
}

impl Class {
    /// True when bytes of this class travel in a snapshot.
    #[must_use]
    pub const fn carried(&self) -> bool {
        !matches!(self, Class::Secret { .. } | Class::Excluded { .. })
    }

    /// The cache directory the path lies under, whatever its class.
    #[must_use]
    pub const fn cache_root(&self) -> Option<&CacheRoot> {
        match self {
            Class::Secret { under } | Class::Excluded { under } => under.as_ref(),
            Class::Cache(root) => Some(root),
            Class::Precious => None,
        }
    }
}

/// A glob over one or more path components; `*` and `?` never cross a slash.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    components: Vec<String>,
}

impl Pattern {
    /// Parses a pattern; gitignore syntax the in-process matcher does not
    /// model (negation, comments, escapes, classes and a `**` past the
    /// leading one, which git matches across any number of directories) is
    /// refused, so a pattern can never mean one thing to git and another to
    /// the policy.
    fn parse(text: &str) -> Option<Pattern> {
        let text = text.strip_prefix("**/").unwrap_or(text);
        if text.is_empty()
            || text.starts_with(['/', '!', '#'])
            || text.contains(['\\', '[', ']'])
            || text.split('/').any(|c| c.is_empty() || c == "**")
        {
            return None;
        }
        Some(Pattern {
            components: text.split('/').map(str::to_owned).collect(),
        })
    }

    /// True when the pattern's components match `comps` starting at `at`.
    fn matches_at(&self, comps: &[&str], at: usize) -> bool {
        let end = at + self.components.len();
        end <= comps.len()
            && self
                .components
                .iter()
                .zip(&comps[at..end])
                .all(|(p, c)| glob(p, c))
    }

    fn gitignore_line(&self) -> String {
        if self.components.len() == 1 {
            self.components[0].clone()
        } else {
            format!("**/{}", self.components.join("/"))
        }
    }
}

/// Matches `text` against a glob with `*` and `?`, folding ASCII case.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The policy for one capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnorePolicy {
    cache_dirs: Vec<CacheRule>,
    declared: Vec<CacheRoot>,
    /// The one secret policy, the core's, widened by the job's patterns.
    secrets: SecretPolicy,
    /// The built-in list and the job's patterns as gitignore patterns, for the exclude file only.
    secret_lines: Vec<Pattern>,
    excludes: Vec<Pattern>,
}

/// A directory name the policy treats as a cache, with the files that make it one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheRule {
    name: String,
    class: CacheClass,
    markers: &'static [&'static str],
}

fn patterns(list: &[&str]) -> Vec<Pattern> {
    list.iter().filter_map(|p| Pattern::parse(p)).collect()
}

impl Default for IgnorePolicy {
    fn default() -> Self {
        IgnorePolicy {
            cache_dirs: CACHE_DIRS
                .iter()
                .map(|d| CacheRule {
                    name: d.name.to_owned(),
                    class: d.class,
                    markers: d.markers,
                })
                .collect(),
            declared: Vec::new(),
            secrets: SecretPolicy::default(),
            secret_lines: patterns(SECRET_PATTERNS),
            excludes: Vec::new(),
        }
    }
}

impl IgnorePolicy {
    /// Makes every directory named `name` a cache of `class`, with no marker needed.
    pub fn add_cache_dir(&mut self, class: CacheClass, name: &str) {
        self.cache_dirs
            .retain(|rule| !rule.name.eq_ignore_ascii_case(name));
        self.cache_dirs.push(CacheRule {
            name: name.to_owned(),
            class,
            markers: &[],
        });
    }

    /// Makes `path` from the root, and everything under it, a cache of
    /// `class`, as `.fabric/env.toml` declares one. The full secret list
    /// applies under it: a path the user declared can be a mistake that
    /// holds the user's own keys, and a declared cache found inside it, or
    /// a cache directory name under it, does not narrow that.
    pub fn add_declared_cache(&mut self, class: CacheClass, path: &RelPath) {
        self.declared.retain(|root| root.path != *path);
        self.declared.push(CacheRoot {
            path: path.clone(),
            class,
        });
    }

    /// The declared cache `comps` lies in, as the index of its last
    /// component and its class.
    fn declared_at(&self, comps: &[&str]) -> Option<(usize, CacheClass)> {
        self.declared
            .iter()
            .filter_map(|root| {
                let declared: Vec<&str> = root.path.as_str().split('/').collect();
                (declared.len() <= comps.len()
                    && declared
                        .iter()
                        .zip(comps)
                        .all(|(d, c)| d.eq_ignore_ascii_case(c)))
                .then(|| (declared.len() - 1, root.class))
            })
            .min_by_key(|(at, _)| *at)
    }

    /// Widens the secret list with a job's pattern, which applies in every
    /// scope, caches included.
    ///
    /// The text must be a `fabric_core::secret::SecretPattern`, as a job's
    /// `secret_patterns` are, so the store and the core read it alike.
    pub fn add_secret_pattern(&mut self, text: &str) -> Result<(), PolicyError> {
        let bad = || PolicyError::BadPolicyPattern {
            text: text.to_owned(),
        };
        let pattern = SecretPattern::parse(text).map_err(|_| bad())?;
        let line = Pattern::parse(pattern.as_str()).ok_or_else(bad)?;
        self.secrets.widen(pattern);
        self.secret_lines.push(line);
        Ok(())
    }

    /// True when the core's policy denies `path`. Under a cache directory
    /// the policy finds by its name, the full list applies to the path up
    /// to that directory and the narrow list to the whole path, so a
    /// dependency's `credentials.js` travels and its `.env` does not.
    fn denies_secret(&self, path: &RelPath, comps: &[&str], narrow_from: Option<usize>) -> bool {
        let Some(at) = narrow_from else {
            return self.secrets.denies(path, Scope::Work).is_some();
        };
        let above = RelPath::parse(&comps[..=at].join("/"));
        above.is_ok_and(|above| self.secrets.denies(&above, Scope::Work).is_some())
            || self.secrets.denies(path, Scope::Cache).is_some()
    }

    /// Adds a pattern whose matches are not carried for this job.
    pub fn add_exclude(&mut self, text: &str) -> Result<(), PolicyError> {
        let pattern = Pattern::parse(text).ok_or_else(|| PolicyError::BadPolicyPattern {
            text: text.to_owned(),
        })?;
        self.excludes.push(pattern);
        Ok(())
    }

    /// Reads `root/.fabricignore` as excludes; returns how many patterns it added.
    ///
    /// One pattern per line; blank lines and lines starting with `#` are skipped.
    pub fn load_fabricignore(&mut self, root: &Path) -> Result<usize, PolicyError> {
        let path = root.join(FABRICIGNORE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(source) => return Err(PolicyError::Io { path, source }),
        };
        let mut added = 0;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let pattern = Pattern::parse(line).ok_or_else(|| PolicyError::BadPattern {
                path: path.clone(),
                line: index + 1,
                text: line.to_owned(),
            })?;
            self.excludes.push(pattern);
            added += 1;
        }
        Ok(added)
    }

    /// The class of the directory `comps[i]`, when its name and a marker beside it make it a cache.
    fn cache_class_at(
        &self,
        comps: &[&str],
        i: usize,
        markers: &mut dyn Markers,
    ) -> Option<CacheClass> {
        let rule = self
            .cache_dirs
            .iter()
            .find(|rule| rule.name.eq_ignore_ascii_case(comps[i]))?;
        let parent = comps[..i].join("/");
        (rule.markers.is_empty() || rule.markers.iter().any(|m| markers.has(&parent, m)))
            .then_some(rule.class)
    }

    /// Classifies one ignored path; `markers` says which tool files sit where.
    pub fn classify(&self, path: &RelPath, markers: &mut dyn Markers) -> Class {
        let comps: Vec<&str> = path.as_str().split('/').collect();
        let declared = self.declared_at(&comps);
        let cache = declared.or_else(|| {
            (0..comps.len()).find_map(|i| {
                self.cache_class_at(&comps, i, markers)
                    .map(|class| (i, class))
            })
        });
        let cache_root = cache.and_then(|(i, class)| {
            RelPath::parse(&comps[..=i].join("/"))
                .ok()
                .map(|path| CacheRoot { path, class })
        });
        // A declared cache keeps the full list (`add_declared_cache`).
        let narrow_from = cache.filter(|_| declared.is_none()).map(|(i, _)| i);
        let secret = self.denies_secret(path, &comps, narrow_from);
        if secret {
            Class::Secret { under: cache_root }
        } else if (0..comps.len()).any(|j| self.excludes.iter().any(|p| p.matches_at(&comps, j))) {
            Class::Excluded { under: cache_root }
        } else {
            cache_root.map_or(Class::Precious, Class::Cache)
        }
    }

    /// Returns the full secret list and the excludes as gitignore lines, for `core.excludesFile`.
    #[must_use]
    pub fn gitignore_lines(&self) -> String {
        let mut text = String::new();
        for pattern in self.secret_lines.iter().chain(&self.excludes) {
            text.push_str(&pattern.gitignore_line());
            text.push('\n');
        }
        text
    }

    /// True when the full secret list denies the path, as the core's
    /// `SecretPolicy::denies` does in `Scope::Work`.
    #[must_use]
    pub fn is_secret(&self, path: &RelPath) -> bool {
        self.secrets.denies(path, Scope::Work).is_some()
    }

    /// True when a path matches the full secret list or an exclude, whatever lies above it.
    #[must_use]
    pub fn denies_anywhere(&self, path: &RelPath) -> bool {
        if self.is_secret(path) {
            return true;
        }
        let comps: Vec<&str> = path.as_str().split('/').collect();
        (0..comps.len()).any(|j| self.excludes.iter().any(|p| p.matches_at(&comps, j)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_star_past_the_leading_one_is_refused() {
        for text in ["foo/**/token", "foo/**", "**/a/**/b", "**/**"] {
            assert_eq!(Pattern::parse(text), None, "{text}");
        }
        assert!(Pattern::parse("**/token").is_some());
        assert!(Pattern::parse("foo/*/token").is_some());
        let mut policy = IgnorePolicy::default();
        assert!(matches!(
            policy.add_secret_pattern("foo/**/token"),
            Err(PolicyError::BadPolicyPattern { .. })
        ));
    }

    fn rel(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    fn cache(path: &str, class: CacheClass) -> Class {
        Class::Cache(CacheRoot {
            path: rel(path),
            class,
        })
    }

    fn is_secret(class: &Class) -> bool {
        matches!(class, Class::Secret { .. })
    }

    /// Classifies in a workspace whose only tool files are `pkg/a/Cargo.toml` and `web/package.json`.
    fn classify(p: &IgnorePolicy, path: &str) -> Class {
        let mut markers = |dir: &str, name: &str| {
            matches!(
                (dir, name),
                ("pkg/a", "Cargo.toml") | ("web", "package.json")
            )
        };
        p.classify(&rel(path), &mut markers)
    }

    /// A cache `.fabric/env.toml` declares, such as `.output`, has no name
    /// the policy knows. It must still be a cache, so a snapshot records it
    /// with a carry key rather than as the user's own files, and the full
    /// secret list applies under it, a key file included.
    #[test]
    fn a_declared_cache_is_a_cache_with_the_full_secret_list() {
        let mut p = IgnorePolicy::default();
        assert_eq!(classify(&p, ".output/server/index.mjs"), Class::Precious);
        p.add_declared_cache(CacheClass::Build, &rel(".output"));
        assert_eq!(
            classify(&p, ".output/server/index.mjs"),
            cache(".output", CacheClass::Build)
        );
        assert_eq!(
            classify(&p, ".output/node_modules/x/index.js"),
            cache(".output", CacheClass::Build)
        );
        for secret in [".output/server.pem", ".output/node_modules/x/tls.key"] {
            assert!(is_secret(&classify(&p, secret)), "{secret}");
        }
        assert_eq!(classify(&p, "web/.output/index.mjs"), Class::Precious);
    }

    /// Every line of the exclude file is a pattern the core denies, at the
    /// top and nested, so the prefilter never keeps out a path the core
    /// carries without capture adding it back by name.
    #[test]
    fn every_exclude_line_is_a_pattern_the_core_denies() {
        let p = IgnorePolicy::default();
        for pattern in SECRET_PATTERNS {
            for base in ["", "precious/"] {
                let sample = rel(&format!("{base}{}", pattern.replace('*', "x")));
                assert!(p.is_secret(&sample), "{pattern}: {sample}");
                let comps: Vec<&str> = sample.as_str().split('/').collect();
                assert!(
                    p.secret_lines
                        .iter()
                        .any(|line| (0..comps.len()).any(|j| line.matches_at(&comps, j))),
                    "{pattern}: {sample}"
                );
            }
        }
    }

    #[test]
    fn glob_rules() {
        assert!(glob("*.pem", "server.pem"));
        assert!(glob("*.pem", ".pem"));
        assert!(!glob("*.pem", "pem"));
        assert!(glob(".env.*", ".env.local"));
        assert!(!glob(".env.*", ".env"));
        assert!(glob("id_*", "id_ed25519"));
        assert!(glob("credentials*", "credentials.json"));
        assert!(glob("a?c", "abc"));
        assert!(!glob("a?c", "ac"));
        assert!(glob("*", ""));
    }

    #[test]
    fn secret_names_match_in_any_case() {
        let p = IgnorePolicy::default();
        for path in [
            "secrets/Server.PEM",
            "secrets/.ENV",
            "ID_RSA",
            "Credentials.json",
            "home/.SSH/config",
        ] {
            assert!(is_secret(&classify(&p, path)), "{path}");
            assert!(p.denies_anywhere(&rel(path)), "{path}");
        }
    }

    #[test]
    fn classes_in_order() {
        let p = IgnorePolicy::default();
        for secret in [
            ".env",
            "config/.aws/credentials",
            ".docker/config.json",
            "home/.docker/config.json",
        ] {
            assert!(is_secret(&classify(&p, secret)), "{secret}");
        }
        assert_eq!(classify(&p, "home/.docker/other.json"), Class::Precious);
        assert_eq!(
            classify(&p, "node_modules/x/index.js"),
            cache("node_modules", CacheClass::Deps)
        );
        assert_eq!(
            classify(&p, "pkg/a/target/debug/x"),
            cache("pkg/a/target", CacheClass::Build)
        );
        assert_eq!(
            classify(&p, "src/__pycache__/m.pyc"),
            cache("src/__pycache__", CacheClass::ToolCache)
        );
        assert_eq!(
            classify(&p, "web/out/index.html"),
            cache("web/out", CacheClass::Build)
        );
        assert_eq!(
            classify(&p, "web/OUT/index.html"),
            cache("web/OUT", CacheClass::Build)
        );
        assert_eq!(classify(&p, "docs/assessment/notes.md"), Class::Precious);
    }

    #[test]
    fn a_generic_name_is_a_cache_only_beside_its_tool_file() {
        let p = IgnorePolicy::default();
        for path in [
            "experiments/out/result.csv",
            "target/notes.md",
            "docs/build/report.pdf",
            "data/.cache/model.bin",
            "vendor/blob.bin",
        ] {
            assert_eq!(classify(&p, path), Class::Precious, "{path}");
        }
        let mut widened = p.clone();
        widened.add_cache_dir(CacheClass::Build, "out");
        assert_eq!(
            classify(&widened, "experiments/out/result.csv"),
            cache("experiments/out", CacheClass::Build)
        );
    }

    #[test]
    fn narrow_list_under_a_cache_and_full_list_under_precious() {
        let p = IgnorePolicy::default();
        assert_eq!(
            classify(&p, "node_modules/pkg/test/fixtures/cert.pem"),
            cache("node_modules", CacheClass::Deps)
        );
        assert!(is_secret(&classify(&p, "node_modules/pkg/.env")));
        assert!(is_secret(&classify(&p, "data/cert.pem")));
        assert_eq!(
            classify(&p, "pkg/node_modules/x/.npmrc"),
            Class::Secret {
                under: Some(CacheRoot {
                    path: rel("pkg/node_modules"),
                    class: CacheClass::Deps,
                }),
            }
        );
    }

    #[test]
    fn fabricignore_excludes_and_secrets_only_widen() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(FABRICIGNORE),
            "# comment\n\nexperiments/out\n*.bak\n",
        )
        .unwrap();
        let mut p = IgnorePolicy::default();
        assert_eq!(p.load_fabricignore(dir.path()).unwrap(), 2);
        assert_eq!(
            classify(&p, "experiments/out/a.csv"),
            Class::Excluded { under: None }
        );
        assert_eq!(classify(&p, "x/notes.bak"), Class::Excluded { under: None });
        assert_eq!(classify(&p, "x/notes.txt"), Class::Precious);
        p.add_secret_pattern("*.secret").unwrap();
        assert!(is_secret(&classify(&p, "node_modules/a/b.secret")));
        assert!(p.gitignore_lines().contains("**/.docker/config.json\n"));
        assert!(p.gitignore_lines().contains("*.bak\n"));
        assert!(p.add_exclude("/abs").is_err());
        assert!(p.add_exclude("a//b").is_err());
        assert!(p.add_exclude("!.env").is_err());
        assert!(p.add_exclude("#x").is_err());
        assert!(p.add_exclude("a[0-9]").is_err());
        assert!(p.add_secret_pattern("\\!x").is_err());
    }
}
