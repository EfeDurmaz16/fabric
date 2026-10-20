//! The secret policy: which paths never leave a node (ADR 0004, Ignored
//! files by class; ADR 0007, Secrets).
//!
//! Invariant: a path that matches a secret pattern never leaves the node,
//! and the pattern list can grow per job but never shrink. The built-in
//! patterns are always on and `SecretPolicy` has no way to remove one. A
//! job's own patterns live in its `EnvSpec`, and detection keeps every one
//! the previous spec held, so dropping a line from `.fabric/env.toml`
//! does not drop the pattern.
//! Matching ignores ASCII case, because the owner's volumes fold case and
//! `.ENV` there is the same file as `.env`.
//!
//! A pattern is one or more path components separated by `/`. Each
//! component is a glob in which `*` matches any run of characters and every
//! other character matches itself. A pattern matches a path when its
//! components equal a run of the path's components, anywhere in the path, so
//! `.aws` denies `config/.aws/credentials` and `.docker/config.json` denies
//! `home/.docker/config.json`. A matching directory denies everything under it.
//!
//! Capture never hands git a pattern. It lists the paths that changed,
//! sorts them with `SecretPolicy::staging` and stages literal paths, so git
//! and this matcher cannot disagree, and a tracked file that matches a
//! pattern is staged with consent or the capture stops; it is never left
//! at its index version, which restore would write back over the user's edit.

use crate::env::SecretPattern;
use crate::path::RelPath;

/// Patterns that apply everywhere, inside caches included.
///
/// Exact names only. A glob here makes a dependency tree uncarriable:
/// measured on real `node_modules` trees, `credentials*` matched
/// `@auth/core/providers/credentials.js`, `@aws-sdk/types/dist-types/credentials.d.ts`
/// and `google-auth-library/build/src/auth/credentials.js`, and `id_*`
/// matched the `ssh2` test fixtures. Those globs apply outside caches.
const EVERYWHERE: &[&str] = &[
    ".env",
    ".envrc",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".aws",
    ".ssh",
    ".gnupg",
];

/// Patterns that apply outside caches only.
///
/// Many npm packages ship `*.pem` and `*.key` test fixtures and `ssh2`
/// ships `id_rsa` ones, and a cache with holes looks warm and is broken,
/// so inside a cache these do not apply. Key and credential files are
/// listed by their exact names, not as `id_*` or `credentials*`: those
/// globs matched source files such as `src/id_test.go` and
/// `src/credentials.ts`, which are ordinary work.
const OUTSIDE_CACHES: &[&str] = &[
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

/// Names the built-in patterns never deny: committed templates that list
/// variable names with empty or sample values, which agents edit as
/// ordinary work. A pattern a job adds still denies them.
const TEMPLATES: &[&str] = &[".env.example", ".env.sample", ".env.template", ".env.dist"];

/// Where a path lies, which decides how many patterns apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Under a dependency, build or tool-cache path: only the high-confidence patterns apply.
    Cache,
    /// Anywhere else, the working tree and precious files included: every pattern applies.
    Work,
}

/// The built-in secret patterns plus any a job added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretPolicy {
    added: Vec<SecretPattern>,
}

impl SecretPolicy {
    /// Returns the built-in policy widened by `patterns`, such as the `secret_patterns` of a job's `EnvSpec`.
    ///
    /// This is how a node applies a job's list to a capture, whichever node
    /// recorded it.
    pub fn from_patterns(patterns: &[SecretPattern]) -> Self {
        let mut policy = Self::default();
        for pattern in patterns {
            policy.widen(pattern.clone());
        }
        policy
    }

    /// Adds a pattern that applies in every scope. Adding one twice changes nothing.
    pub fn widen(&mut self, pattern: SecretPattern) {
        if !self.added.contains(&pattern) {
            self.added.push(pattern);
        }
    }

    /// Returns the first pattern that denies `path` in `scope`, or `None` when the path may travel.
    #[must_use]
    pub fn denies(&self, path: &RelPath, scope: Scope) -> Option<&str> {
        let outside = match scope {
            Scope::Cache => &[][..],
            Scope::Work => OUTSIDE_CACHES,
        };
        let names: Vec<&str> = path.as_str().split('/').collect();
        EVERYWHERE
            .iter()
            .chain(outside)
            .copied()
            .find(|pattern| path_matches(pattern, &names, TEMPLATES))
            .or_else(|| {
                self.added
                    .iter()
                    .find(|pattern| pattern_matches(pattern, path.as_str()))
                    .map(SecretPattern::as_str)
            })
    }

    /// Sorts the paths git reports as changed into what capture stages, what it leaves out, and what needs consent.
    ///
    /// `changes` is every path that differs between the index `I` capture
    /// starts from and the working tree: tracked paths the working tree
    /// modified or deleted, and untracked paths that are not ignored. An
    /// untracked path the policy denies is left out of `W`. A tracked path
    /// the policy denies is never left out silently, because leaving it out
    /// would put the `I` version in `W` and restore would revert the user's
    /// edit or bring back a file the user deleted; it waits for consent
    /// instead. Every other path is staged.
    #[must_use]
    pub fn staging(&self, changes: impl IntoIterator<Item = (RelPath, Tracking)>) -> Staging {
        let mut staging = Staging::default();
        for (path, tracking) in changes {
            match (self.denies(&path, Scope::Work), tracking) {
                (None, _) => staging.stage.push(path),
                (Some(pattern), Tracking::Untracked) => staging.left_out.push(Denied {
                    pattern: pattern.to_owned(),
                    path,
                }),
                (Some(pattern), Tracking::Tracked) => staging.tracked.push(Denied {
                    pattern: pattern.to_owned(),
                    path,
                }),
            }
        }
        staging
    }
}

/// Whether git tracks a changed path in the index capture starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    /// The path is in the index `I` and the working tree modified, deleted or replaced it.
    Tracked,
    /// The path is not in `I` and not ignored, so `git add -A` would add it.
    Untracked,
}

/// A path the secret policy denied, with the pattern that denied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    /// The path, relative to the workspace root.
    pub path: RelPath,
    /// The pattern that denied it.
    pub pattern: String,
}

/// What capture stages into the working-tree commit `W`.
///
/// The paths to hand git come only from `Staging::paths`, which refuses
/// while a tracked path that matches a secret pattern has changed and the
/// user has not agreed to carry the change.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Staging {
    stage: Vec<RelPath>,
    left_out: Vec<Denied>,
    tracked: Vec<Denied>,
}

impl Staging {
    /// Returns what capture stages and what it keeps out, or refuses when a tracked secret changed without consent.
    ///
    /// Capture passes each staged path to `git add -A` as a `:(literal)`
    /// pathspec, so git applies no pattern of its own, and lists the
    /// paths kept out in its output.
    pub fn paths(self, consent: TrackedSecrets) -> Result<Staged, TrackedSecretChanged> {
        let mut paths = match consent {
            TrackedSecrets::Refuse if !self.tracked.is_empty() => {
                return Err(TrackedSecretChanged {
                    changed: self.tracked,
                });
            }
            TrackedSecrets::Refuse => self.stage.into_iter(),
            TrackedSecrets::Carry => {
                let mut paths = self.stage;
                paths.extend(self.tracked.into_iter().map(|denied| denied.path));
                paths.into_iter()
            }
        };
        let stage = match paths.next() {
            None => Stage::Nothing,
            Some(first) => Stage::Paths {
                first,
                rest: paths.collect(),
            },
        };
        Ok(Staged {
            stage,
            left_out: self.left_out,
        })
    }
}

/// The result of sorting a capture's changes: what git stages and the untracked paths the policy kept out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// What capture hands `git add`.
    pub stage: Stage,
    /// The untracked paths the policy denied, each with its pattern; capture lists them so nothing stays behind silently.
    pub left_out: Vec<Denied>,
}

/// What capture hands `git add`: nothing, or at least one literal path.
///
/// `git add -A` with no pathspec at all stages everything, secrets
/// included, so "nothing to stage" is its own variant and the list of
/// paths cannot be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Nothing may be staged; capture runs no `git add`.
    Nothing,
    /// At least one path to stage.
    Paths {
        /// The first path to stage.
        first: RelPath,
        /// The paths after it, possibly none.
        rest: Vec<RelPath>,
    },
}

/// Whether the user agreed that changes to tracked files matching a secret pattern may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedSecrets {
    /// Refuse the capture and name the files; the default.
    Refuse,
    /// Stage the changes like any other tracked change.
    Carry,
}

/// Tracked files that match a secret pattern changed, and the user has not agreed to carry them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "tracked files that match a secret pattern changed: {}; commit or revert them, or agree to carry the change",
    changed_list(changed)
)]
pub struct TrackedSecretChanged {
    /// Each changed path and the pattern it matches.
    pub changed: Vec<Denied>,
}

fn changed_list(changed: &[Denied]) -> String {
    changed
        .iter()
        .map(|denied| format!("{} ({})", denied.path, denied.pattern))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Returns true when a job's `pattern` denies `path`, a relative path whose components are separated by `/`.
pub fn pattern_matches(pattern: &SecretPattern, path: &str) -> bool {
    let names: Vec<&str> = path.split('/').collect();
    path_matches(pattern.as_str(), &names, &[])
}

/// Returns true when the components of `pattern` match a run of `names`, the components of a path.
///
/// A component equal to one of `exempt`, ignoring ASCII case, matches no glob.
fn path_matches(pattern: &str, names: &[&str], exempt: &[&str]) -> bool {
    let len = pattern.split('/').count();
    names.windows(len).any(|window| {
        window.iter().zip(pattern.split('/')).all(|(name, glob)| {
            glob_matches(glob.as_bytes(), name.as_bytes())
                && !exempt.iter().any(|e| e.eq_ignore_ascii_case(name))
        })
    })
}

/// Matches one component: `*` is any run of bytes, every other byte matches itself ignoring ASCII case.
fn glob_matches(glob: &[u8], name: &[u8]) -> bool {
    let (mut g, mut n) = (0, 0);
    // The last `*` seen and the name position it was tried at, for backtracking.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if glob.get(g) == Some(&b'*') {
            star = Some((g, n));
            g += 1;
        } else if glob
            .get(g)
            .is_some_and(|b| b.eq_ignore_ascii_case(&name[n]))
        {
            g += 1;
            n += 1;
        } else if let Some((star_g, star_n)) = star {
            g = star_g + 1;
            n = star_n + 1;
            star = Some((star_g, star_n + 1));
        } else {
            return false;
        }
    }
    glob[g..].iter().all(|&b| b == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> RelPath {
        RelPath::parse(text).unwrap()
    }

    /// One sample per built-in pattern, each first matched by that pattern
    /// and by no earlier one. Removing a pattern, or moving one between the
    /// two lists, fails this test.
    #[test]
    fn every_builtin_pattern_denies_its_sample_at_the_top_and_nested() {
        let everywhere = [
            (".env", ".env"),
            (".envrc", ".envrc"),
            (".netrc", ".netrc"),
            (".npmrc", ".npmrc"),
            (".pypirc", ".pypirc"),
            (".git-credentials", ".git-credentials"),
            (".aws/config", ".aws"),
            (".ssh/known_hosts", ".ssh"),
            (".gnupg/pubring.kbx", ".gnupg"),
        ];
        let outside_caches = [
            (".env.local", ".env.*"),
            ("id_rsa", "id_rsa"),
            ("id_dsa", "id_dsa"),
            ("id_ecdsa", "id_ecdsa"),
            ("id_ed25519", "id_ed25519"),
            ("id_ecdsa_sk", "id_ecdsa_sk"),
            ("id_ed25519_sk", "id_ed25519_sk"),
            ("credentials", "credentials"),
            ("credentials.json", "credentials.json"),
            ("credentials.yml", "credentials.yml"),
            ("credentials.yaml", "credentials.yaml"),
            ("credentials.toml", "credentials.toml"),
            (".credentials.json", ".credentials*"),
            ("prod.env", "*.env"),
            ("server.pem", "*.pem"),
            ("server.key", "*.key"),
            ("cert.p12", "*.p12"),
            ("cert.pfx", "*.pfx"),
            ("keystore.jks", "*.jks"),
            ("terraform.tfstate", "*.tfstate"),
            ("terraform.tfstate.backup", "*.tfstate.*"),
            (".docker/config.json", ".docker/config.json"),
        ];
        let policy = SecretPolicy::default();
        for (sample, pattern) in everywhere {
            for prefix in ["", "precious/out/", "node_modules/pkg/"] {
                let p = path(&format!("{prefix}{sample}"));
                assert_eq!(policy.denies(&p, Scope::Work), Some(pattern), "{p}");
                assert_eq!(policy.denies(&p, Scope::Cache), Some(pattern), "{p}");
            }
        }
        for (sample, pattern) in outside_caches {
            for prefix in ["", "precious/out/"] {
                let p = path(&format!("{prefix}{sample}"));
                assert_eq!(policy.denies(&p, Scope::Work), Some(pattern), "{p}");
                assert_eq!(policy.denies(&p, Scope::Cache), None, "{p}");
            }
        }
        assert_eq!(everywhere.map(|(_, pattern)| pattern), EVERYWHERE);
        assert_eq!(outside_caches.map(|(_, pattern)| pattern), OUTSIDE_CACHES);
    }

    #[test]
    fn matching_ignores_ascii_case() {
        let policy = SecretPolicy::default();
        assert_eq!(policy.denies(&path(".ENV"), Scope::Work), Some(".env"));
        assert_eq!(
            policy.denies(&path("certs/Server.PEM"), Scope::Work),
            Some("*.pem")
        );
        assert_eq!(
            policy.denies(&path("Config/.AWS/x"), Scope::Cache),
            Some(".aws")
        );
    }

    #[test]
    fn near_misses_travel() {
        let policy = SecretPolicy::default();
        for sample in [
            "environment.ts",
            "src/env.rs",
            "monkey",
            "docs/keys.md",
            "my.env.d/readme",
            ".docker/daemon.json",
            "docker/config.json",
            "envrc",
            "src/identity.rs",
            "src/id_test.go",
            "src/id_allocator.rs",
            "id_rsa.pub",
            "src/credentials.ts",
            "lib/credentials_provider.py",
        ] {
            assert_eq!(policy.denies(&path(sample), Scope::Work), None, "{sample}");
        }
    }

    /// Env templates are committed work; `.env.*` must not deny them, in any
    /// case or depth, while a job can still add one by name.
    #[test]
    fn env_templates_travel_unless_a_job_names_them() {
        let mut policy = SecretPolicy::default();
        for sample in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.dist",
            "web/.ENV.Example",
        ] {
            assert_eq!(policy.denies(&path(sample), Scope::Work), None, "{sample}");
        }
        assert_eq!(
            policy.denies(&path(".env.production"), Scope::Work),
            Some(".env.*")
        );
        policy.widen(SecretPattern::parse(".env.example").unwrap());
        assert_eq!(
            policy.denies(&path(".env.example"), Scope::Work),
            Some(".env.example")
        );
    }

    /// A job can add a pattern and it applies inside caches too; there is no
    /// call that removes a built-in one.
    #[test]
    fn a_job_widens_the_list_in_every_scope() {
        let mut policy = SecretPolicy::default();
        let local_db = path("data/local.sqlite");
        assert_eq!(policy.denies(&local_db, Scope::Work), None);
        policy.widen(SecretPattern::parse("*.sqlite").unwrap());
        policy.widen(SecretPattern::parse("*.sqlite").unwrap());
        assert_eq!(policy.denies(&local_db, Scope::Work), Some("*.sqlite"));
        assert_eq!(
            policy.denies(&path("node_modules/x/local.sqlite"), Scope::Cache),
            Some("*.sqlite")
        );
        assert_eq!(policy.added.len(), 1);
    }

    /// The built-in list obeys the rules a job's patterns are held to, so
    /// one matcher serves both.
    #[test]
    fn every_builtin_is_a_valid_pattern() {
        for builtin in EVERYWHERE.iter().chain(OUTSIDE_CACHES) {
            assert!(SecretPattern::parse(builtin).is_ok(), "{builtin}");
        }
    }

    /// The plain recursive definition of the glob: `*` is any run, every other byte matches itself ignoring ASCII case.
    fn reference(glob: &[u8], name: &[u8]) -> bool {
        match glob.split_first() {
            None => name.is_empty(),
            Some((b'*', rest)) => (0..=name.len()).any(|skip| reference(rest, &name[skip..])),
            Some((g, rest)) => name
                .split_first()
                .is_some_and(|(n, name)| g.eq_ignore_ascii_case(n) && reference(rest, name)),
        }
    }

    proptest::proptest! {
        /// The backtracking matcher agrees with the recursive definition on
        /// short globs over a small alphabet, where repeated text and
        /// several stars make backtracking matter.
        #[test]
        fn the_matcher_agrees_with_the_definition(
            glob in "[ab.*]{0,8}",
            name in "[abAB.]{0,10}",
        ) {
            proptest::prop_assert_eq!(
                glob_matches(glob.as_bytes(), name.as_bytes()),
                reference(glob.as_bytes(), name.as_bytes())
            );
        }
    }
}
