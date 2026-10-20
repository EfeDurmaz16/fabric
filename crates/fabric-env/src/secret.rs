//! The secret policy, shared by snapshot capture and the environment spec
//! (ADR 0004, Ignored files by class; ADR 0007, Secrets).
//!
//! Invariant: a path that matches a secret pattern never leaves the node,
//! and the pattern list can grow per job but never shrink. The built-in
//! patterns are always on and `SecretPolicy` has no way to remove one.
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

use fabric_core::path::RelPath;

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
/// Many npm packages ship `*.pem` and `*.key` test fixtures, source files
/// named `credentials.js` and `.env.example` samples, and a cache with
/// holes looks warm and is broken, so inside a cache these do not apply.
const OUTSIDE_CACHES: &[&str] = &[
    ".env.*",
    "id_*",
    "credentials*",
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

/// Where a path lies, which decides how many patterns apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Under a dependency, build or tool-cache path: only the high-confidence patterns apply.
    Cache,
    /// Anywhere else, the working tree and precious files included: every pattern applies.
    Work,
}

/// Why a secret pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
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
    /// A character with a meaning in gitignore that this matcher does not share.
    #[error("secret pattern has an unsupported character at index {index}")]
    BadChar {
        /// Zero-based byte index of the character.
        index: usize,
    },
}

/// A checked secret pattern a job adds to the built-in list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern(String);

impl Pattern {
    /// Checks `text` against the pattern rules and wraps it.
    ///
    /// Refused: `\`, `[`, `]`, `?`, `!`, `#`, whitespace and NUL, because
    /// gitignore gives them meanings this matcher does not, and the same
    /// pattern must deny the same paths in both.
    pub fn parse(text: &str) -> Result<Self, PatternError> {
        if text.is_empty() {
            return Err(PatternError::Empty);
        }
        let bad = text.char_indices().find(|&(_, c)| {
            matches!(c, '\\' | '[' | ']' | '?' | '!' | '#' | '\0') || c.is_whitespace()
        });
        if let Some((index, _)) = bad {
            return Err(PatternError::BadChar { index });
        }
        for component in text.split('/') {
            match component {
                "" => return Err(PatternError::EmptyComponent),
                "." | ".." => return Err(PatternError::DotComponent),
                "**" => return Err(PatternError::DoubleStar),
                _ => {}
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// Returns the pattern text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The built-in secret patterns plus any a job added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretPolicy {
    added: Vec<Pattern>,
}

impl SecretPolicy {
    /// Adds a pattern that applies in every scope. Adding one twice changes nothing.
    pub fn widen(&mut self, pattern: Pattern) {
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
            .chain(self.added.iter().map(Pattern::as_str))
            .find(|pattern| path_matches(pattern, &names))
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
    /// Returns the untracked paths the policy keeps out of `W`; capture lists them in its output.
    pub fn left_out(&self) -> &[Denied] {
        &self.left_out
    }

    /// Returns the tracked paths that match a secret pattern and changed in the working tree.
    pub fn tracked_secrets(&self) -> &[Denied] {
        &self.tracked
    }

    /// Returns the paths capture stages, or refuses when a tracked secret changed without consent.
    ///
    /// Capture passes each path to `git add -A` as a `:(literal)` pathspec,
    /// so git applies no pattern of its own. An empty list means there is
    /// nothing to stage: `git add -A` with no pathspec at all stages
    /// everything, so capture must not run it then.
    pub fn paths(self, consent: TrackedSecrets) -> Result<Vec<RelPath>, TrackedSecretChanged> {
        match consent {
            TrackedSecrets::Refuse if !self.tracked.is_empty() => Err(TrackedSecretChanged {
                changed: self.tracked,
            }),
            TrackedSecrets::Refuse => Ok(self.stage),
            TrackedSecrets::Carry => {
                let mut paths = self.stage;
                paths.extend(self.tracked.into_iter().map(|denied| denied.path));
                Ok(paths)
            }
        }
    }
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

/// Returns true when the components of `pattern` match a run of `names`, the components of a path.
fn path_matches(pattern: &str, names: &[&str]) -> bool {
    let len = pattern.split('/').count();
    names.windows(len).any(|window| {
        window
            .iter()
            .zip(pattern.split('/'))
            .all(|(name, glob)| glob_matches(glob.as_bytes(), name.as_bytes()))
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

/// A known token shape: a prefix and the characters and length that must follow it.
struct TokenShape {
    prefix: &'static str,
    min_len: usize,
    allowed: fn(u8) -> bool,
}

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn alnum_dash_underscore(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn upper_alnum(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

/// Token formats with a fixed prefix, each with the body its issuer documents.
const TOKEN_SHAPES: &[TokenShape] = &[
    TokenShape {
        prefix: "ghp_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "gho_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "ghu_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "ghs_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "ghr_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "github_pat_",
        min_len: 22,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "glpat-",
        min_len: 20,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "npm_",
        min_len: 36,
        allowed: alnum,
    },
    TokenShape {
        prefix: "xoxb-",
        min_len: 10,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "xoxp-",
        min_len: 10,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "sk-",
        min_len: 20,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "AKIA",
        min_len: 16,
        allowed: upper_alnum,
    },
];

/// Returns true when `text` holds something shaped like a credential value.
///
/// Checks the shapes that are certain: GitHub, GitLab, npm and Slack
/// tokens, `OpenAI` and Anthropic style API keys, AWS access key ids, a PEM
/// block header and a URL with a password before the host. It is a guard on
/// text that will travel, such as setup arguments, not a scanner for files.
#[must_use]
pub fn looks_like_credential(text: &str) -> bool {
    text.contains("-----BEGIN") || url_has_password(text) || has_token_shape(text)
}

fn has_token_shape(text: &str) -> bool {
    let bytes = text.as_bytes();
    TOKEN_SHAPES.iter().any(|shape| {
        text.match_indices(shape.prefix).any(|(start, _)| {
            let starts_a_word = start == 0 || !alnum_dash_underscore(bytes[start - 1]);
            let body = &bytes[start + shape.prefix.len()..];
            let body_len = body.iter().take_while(|&&b| (shape.allowed)(b)).count();
            starts_a_word && body_len >= shape.min_len
        })
    })
}

fn url_has_password(text: &str) -> bool {
    text.match_indices("://").any(|(start, sep)| {
        let rest = &text[start + sep.len()..];
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        authority
            .rsplit_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(':'))
    })
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
            ("id_ed25519", "id_*"),
            ("credentials.json", "credentials*"),
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
        ] {
            assert_eq!(policy.denies(&path(sample), Scope::Work), None, "{sample}");
        }
    }

    /// A job can add a pattern and it applies inside caches too; there is no
    /// call that removes a built-in one.
    #[test]
    fn a_job_widens_the_list_in_every_scope() {
        let mut policy = SecretPolicy::default();
        let local_db = path("data/local.sqlite");
        assert_eq!(policy.denies(&local_db, Scope::Work), None);
        policy.widen(Pattern::parse("*.sqlite").unwrap());
        policy.widen(Pattern::parse("*.sqlite").unwrap());
        assert_eq!(policy.denies(&local_db, Scope::Work), Some("*.sqlite"));
        assert_eq!(
            policy.denies(&path("node_modules/x/local.sqlite"), Scope::Cache),
            Some("*.sqlite")
        );
        assert_eq!(policy.added.len(), 1);
    }

    #[test]
    fn a_pattern_that_gitignore_would_read_differently_is_refused() {
        assert_eq!(Pattern::parse(""), Err(PatternError::Empty));
        assert_eq!(Pattern::parse("/x"), Err(PatternError::EmptyComponent));
        assert_eq!(Pattern::parse("x/"), Err(PatternError::EmptyComponent));
        assert_eq!(Pattern::parse("a/../b"), Err(PatternError::DotComponent));
        assert_eq!(Pattern::parse("**/x"), Err(PatternError::DoubleStar));
        assert_eq!(
            Pattern::parse("!x"),
            Err(PatternError::BadChar { index: 0 })
        );
        assert_eq!(
            Pattern::parse("#x"),
            Err(PatternError::BadChar { index: 0 })
        );
        assert_eq!(
            Pattern::parse("x?"),
            Err(PatternError::BadChar { index: 1 })
        );
        assert_eq!(
            Pattern::parse("[ab]"),
            Err(PatternError::BadChar { index: 0 })
        );
        assert_eq!(
            Pattern::parse("a b"),
            Err(PatternError::BadChar { index: 1 })
        );
        assert_eq!(
            Pattern::parse("x\\"),
            Err(PatternError::BadChar { index: 1 })
        );
        for builtin in EVERYWHERE.iter().chain(OUTSIDE_CACHES) {
            assert!(Pattern::parse(builtin).is_ok(), "{builtin}");
        }
    }

    #[test]
    fn star_backtracks_across_repeated_text() {
        assert!(glob_matches(b"*.tfstate.*", b"a.tfstate.tfstate.bak"));
        assert!(glob_matches(b"*.env", b".env"));
        assert!(glob_matches(b"id_*", b"id_"));
        assert!(!glob_matches(b"*.key", b"monkey"));
        assert!(!glob_matches(b"*.env", b".envrc"));
    }

    #[test]
    fn credential_shapes_are_recognized() {
        let github = format!("ghp_{}", "a1".repeat(18));
        let npm = format!("npm_{}", "Z9".repeat(18));
        for text in [
            github.as_str(),
            &format!("--token={github}"),
            &npm,
            "sk-ant-api03-abcdefghijklmnopqrstuvwx",
            "sk-proj-abcdefghijklmnopqrst",
            "AKIAIOSFODNN7EXAMPLE",
            "glpat-abcdefghij0123456789",
            "github_pat_11ABCDEFG0123456789abc",
            "xoxb-1234567890-abc",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "https://user:hunter2@registry.example.com/simple",
            "--index-url=https://u:p@h/x",
        ] {
            assert!(looks_like_credential(text), "{text}");
        }
    }

    #[test]
    fn ordinary_arguments_are_not_credentials() {
        for text in [
            "install",
            "--frozen-lockfile",
            "--disk-cache-size-is-a-long-option",
            "task-runner-with-a-very-long-name",
            "npm_config_registry_override_value_x",
            "https://user@github.com/org/repo.git",
            "https://registry.npmjs.org/",
            "git@github.com:org/repo.git",
            "ghp_short",
            "AKIAlowercase0000000",
        ] {
            assert!(!looks_like_credential(text), "{text}");
        }
    }
}
