//! The ignored-file policy (ADR 0004, Ignored files by class).
//!
//! Invariant: every path gets exactly one class, a path that matches a
//! secret pattern is `Secret` whatever else it matches, and the secret list
//! can be widened per job but never narrowed. Under a dependency, build or
//! tool-cache directory only the high-confidence patterns apply, so a
//! `node_modules` is carried whole or left behind, never with holes it does
//! not know about. A generic directory name such as `out` or `build` is a
//! cache only next to the file of the tool that makes it, so an ignored
//! `experiments/out/` with no `package.json` beside it stays precious. Names
//! are compared with ASCII case folded, as APFS and git on a Mac compare
//! them, so `Server.PEM` is as secret as `server.pem`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fabric_core::path::RelPath;
use fabric_core::snapshot::CacheClass;

/// Patterns that are never carried, matched against every path component.
pub const SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    ".envrc",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.tfstate",
    "*.tfstate.*",
    "id_*",
    "credentials*",
    ".credentials*",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".aws",
    ".ssh",
    ".gnupg",
    ".docker/config.json",
];

/// The patterns that still apply below a dependency, build or tool-cache directory.
pub const CACHE_SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    ".envrc",
    "id_*",
    "credentials*",
    ".credentials*",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".aws",
    ".ssh",
    ".gnupg",
];

/// A directory name that holds a cache, and the files that make it one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheDir {
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
pub const CACHE_DIRS: &[CacheDir] = &[
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
    /// A pattern is empty, absolute, or has an empty component.
    #[error("{path}:{line}: pattern {text:?} is not valid")]
    BadPattern {
        /// The file the pattern came from, or the policy itself.
        path: PathBuf,
        /// The one-based line.
        line: usize,
        /// The pattern text.
        text: String,
    },
}

/// The class a path gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Matches a secret pattern; never carried.
    Secret,
    /// Matches a pattern in `.fabricignore`; not carried for this job.
    Excluded,
    /// Below a dependency directory.
    Deps,
    /// Below a build output directory.
    Build,
    /// Below a tool cache.
    ToolCache,
    /// Ignored by git and nobody's cache: the user's work.
    Precious,
}

impl Class {
    /// True when bytes of this class travel in a snapshot.
    #[must_use]
    pub const fn carried(self) -> bool {
        !matches!(self, Class::Secret | Class::Excluded)
    }
}

/// What the policy says about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The class.
    pub class: Class,
    /// The cache directory the path lies under, when it does.
    pub cache_root: Option<(RelPath, CacheClass)>,
}

/// A glob over one or more path components; `*` and `?` never cross a slash.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    components: Vec<String>,
}

impl Pattern {
    /// Parses a pattern; gitignore syntax the in-process matcher does not
    /// model (negation, comments, escapes and classes) is refused, so a
    /// pattern can never mean one thing to git and another to the policy.
    fn parse(text: &str) -> Option<Pattern> {
        let text = text.strip_prefix("**/").unwrap_or(text);
        if text.is_empty()
            || text.starts_with(['/', '!', '#'])
            || text.contains(['\\', '[', ']'])
            || text.split('/').any(str::is_empty)
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
    cache_dirs: Vec<(String, CacheClass, &'static [&'static str])>,
    secrets: Vec<Pattern>,
    cache_secrets: Vec<Pattern>,
    excludes: Vec<Pattern>,
}

fn patterns(list: &[&str]) -> Vec<Pattern> {
    list.iter().filter_map(|p| Pattern::parse(p)).collect()
}

impl Default for IgnorePolicy {
    fn default() -> Self {
        IgnorePolicy {
            cache_dirs: CACHE_DIRS
                .iter()
                .map(|d| (d.name.to_owned(), d.class, d.markers))
                .collect(),
            secrets: patterns(SECRET_PATTERNS),
            cache_secrets: patterns(CACHE_SECRET_PATTERNS),
            excludes: Vec::new(),
        }
    }
}

impl IgnorePolicy {
    /// Makes every directory named `name` a cache of `class`, with no marker needed.
    pub fn add_cache_dir(&mut self, class: CacheClass, name: &str) {
        self.cache_dirs
            .retain(|(n, _, _)| !n.eq_ignore_ascii_case(name));
        self.cache_dirs.push((name.to_owned(), class, &[]));
    }

    /// Widens the secret list, in both the full and the narrow form.
    pub fn add_secret_pattern(&mut self, text: &str) -> Result<(), PolicyError> {
        let pattern = Pattern::parse(text).ok_or_else(|| PolicyError::BadPattern {
            path: PathBuf::from("<policy>"),
            line: 0,
            text: text.to_owned(),
        })?;
        self.cache_secrets.push(pattern.clone());
        self.secrets.push(pattern);
        Ok(())
    }

    /// Adds a pattern whose matches are not carried for this job.
    pub fn add_exclude(&mut self, text: &str) -> Result<(), PolicyError> {
        let pattern = Pattern::parse(text).ok_or_else(|| PolicyError::BadPattern {
            path: PathBuf::from("<policy>"),
            line: 0,
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
        let (_, class, needs) = self
            .cache_dirs
            .iter()
            .find(|(name, _, _)| name.eq_ignore_ascii_case(comps[i]))?;
        let parent = comps[..i].join("/");
        (needs.is_empty() || needs.iter().any(|m| markers.has(&parent, m))).then_some(*class)
    }

    /// Classifies one ignored path; `markers` says which tool files sit where.
    pub fn classify(&self, path: &RelPath, markers: &mut dyn Markers) -> Classification {
        let comps: Vec<&str> = path.as_str().split('/').collect();
        let cache = (0..comps.len()).find_map(|i| {
            self.cache_class_at(&comps, i, markers)
                .map(|class| (i, class))
        });
        let cache_root = cache.and_then(|(i, class)| {
            RelPath::parse(&comps[..=i].join("/"))
                .ok()
                .map(|root| (root, class))
        });
        let secret = (0..comps.len()).any(|j| {
            let narrow = cache.is_some_and(|(i, _)| j > i);
            let list = if narrow {
                &self.cache_secrets
            } else {
                &self.secrets
            };
            list.iter().any(|p| p.matches_at(&comps, j))
        });
        let class = if secret {
            Class::Secret
        } else if (0..comps.len()).any(|j| self.excludes.iter().any(|p| p.matches_at(&comps, j))) {
            Class::Excluded
        } else {
            match cache {
                Some((_, CacheClass::Deps)) => Class::Deps,
                Some((_, CacheClass::Build)) => Class::Build,
                Some((_, CacheClass::ToolCache)) => Class::ToolCache,
                None => Class::Precious,
            }
        };
        Classification { class, cache_root }
    }

    /// Returns the full secret list and the excludes as gitignore lines, for `core.excludesFile`.
    #[must_use]
    pub fn gitignore_lines(&self) -> String {
        let mut text = String::new();
        for pattern in self.secrets.iter().chain(&self.excludes) {
            text.push_str(&pattern.gitignore_line());
            text.push('\n');
        }
        text
    }

    /// True when any component of a path matches the full secret list.
    #[must_use]
    pub fn is_secret(&self, path: &RelPath) -> bool {
        let comps: Vec<&str> = path.as_str().split('/').collect();
        (0..comps.len()).any(|j| self.secrets.iter().any(|p| p.matches_at(&comps, j)))
    }

    /// True when a path matches the full secret list or an exclude, whatever lies above it.
    #[must_use]
    pub fn denies_anywhere(&self, path: &RelPath) -> bool {
        let comps: Vec<&str> = path.as_str().split('/').collect();
        (0..comps.len()).any(|j| {
            self.secrets
                .iter()
                .chain(&self.excludes)
                .any(|p| p.matches_at(&comps, j))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    /// Classifies in a workspace whose only tool files are `pkg/a/Cargo.toml` and `web/package.json`.
    fn classify(p: &IgnorePolicy, path: &str) -> Classification {
        let mut markers = |dir: &str, name: &str| {
            matches!(
                (dir, name),
                ("pkg/a", "Cargo.toml") | ("web", "package.json")
            )
        };
        p.classify(&rel(path), &mut markers)
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
            assert_eq!(classify(&p, path).class, Class::Secret, "{path}");
            assert!(p.denies_anywhere(&rel(path)), "{path}");
        }
    }

    #[test]
    fn classes_in_order() {
        let p = IgnorePolicy::default();
        assert_eq!(classify(&p, ".env").class, Class::Secret);
        assert_eq!(classify(&p, "config/.aws/credentials").class, Class::Secret);
        assert_eq!(classify(&p, ".docker/config.json").class, Class::Secret);
        assert_eq!(
            classify(&p, "home/.docker/config.json").class,
            Class::Secret
        );
        assert_eq!(
            classify(&p, "home/.docker/other.json").class,
            Class::Precious
        );
        assert_eq!(classify(&p, "node_modules/x/index.js").class, Class::Deps);
        assert_eq!(classify(&p, "pkg/a/target/debug/x").class, Class::Build);
        assert_eq!(
            classify(&p, "src/__pycache__/m.pyc").class,
            Class::ToolCache
        );
        assert_eq!(classify(&p, "web/out/index.html").class, Class::Build);
        assert_eq!(classify(&p, "web/OUT/index.html").class, Class::Build);
        assert_eq!(
            classify(&p, "docs/assessment/notes.md").class,
            Class::Precious
        );
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
            let found = classify(&p, path);
            assert_eq!(found.class, Class::Precious, "{path}");
            assert_eq!(found.cache_root, None, "{path}");
        }
        assert_eq!(
            classify(&p, "pkg/a/target/debug/x").cache_root,
            Some((rel("pkg/a/target"), CacheClass::Build))
        );
        let mut widened = p.clone();
        widened.add_cache_dir(CacheClass::Build, "out");
        assert_eq!(
            classify(&widened, "experiments/out/result.csv").class,
            Class::Build
        );
    }

    #[test]
    fn narrow_list_under_a_cache_and_full_list_under_precious() {
        let p = IgnorePolicy::default();
        let under_cache = classify(&p, "node_modules/pkg/test/fixtures/cert.pem");
        assert_eq!(under_cache.class, Class::Deps);
        assert_eq!(
            under_cache.cache_root,
            Some((rel("node_modules"), CacheClass::Deps))
        );
        assert_eq!(classify(&p, "node_modules/pkg/.env").class, Class::Secret);
        assert_eq!(classify(&p, "data/cert.pem").class, Class::Secret);
        assert_eq!(
            classify(&p, "pkg/node_modules/x/.npmrc").cache_root,
            Some((rel("pkg/node_modules"), CacheClass::Deps))
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
        assert_eq!(classify(&p, "experiments/out/a.csv").class, Class::Excluded);
        assert_eq!(classify(&p, "x/notes.bak").class, Class::Excluded);
        assert_eq!(classify(&p, "x/notes.txt").class, Class::Precious);
        p.add_secret_pattern("*.secret").unwrap();
        assert_eq!(classify(&p, "node_modules/a/b.secret").class, Class::Secret);
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
