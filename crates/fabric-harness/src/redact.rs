//! Secret redaction.
//!
//! Invariant: a redaction rule never leaves part of a match behind, and a
//! text that is not UTF-8 is matched as bytes with the same rules, so a
//! line cut inside a multi-byte character is redacted like any other.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use regex::bytes::Regex as BytesRegex;

use crate::error::BadRule;

/// Per-rule match counts.
pub type RedactionCounts = BTreeMap<&'static str, u32>;

/// Secret-shaped text that must never leave the machine it was typed on.
pub struct Redactor {
    rules: Vec<Rule>,
    key_end: BytesRegex,
}

/// The line that closes a PEM private key block.
const KEY_END: &str = r"-----END [A-Z ]*PRIVATE KEY-----";

struct Rule {
    name: &'static str,
    text: Regex,
    bytes: BytesRegex,
    accept: fn(&[u8]) -> bool,
}

/// One rule as written: a name, a pattern, and a check every match must
/// also pass before it counts as a secret.
struct Pattern {
    name: &'static str,
    regex: &'static str,
    accept: fn(&[u8]) -> bool,
}

/// A pattern with a capture group named `keep` keeps that part and
/// replaces only the rest: `DB_PASSWORD=` stays, its value does not.
///
/// `(?-u:\b)` is an ASCII word boundary. Every key shape starts on one, so
/// a prefix such as `sk-` inside an ordinary word (`disk-usage`,
/// `task-list`) never starts a match, and the text and byte forms of a rule
/// agree on where a word ends.
const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "anthropic-key",
        regex: r"(?-u:\b)sk-ant-[A-Za-z0-9_\-]{20,}",
        accept: any,
    },
    Pattern {
        name: "openai-key",
        regex: r"(?-u:\b)sk-(?:proj|svcacct|admin)-[A-Za-z0-9_\-]{32,}|(?-u:\b)sk-[A-Za-z0-9]{32,}",
        accept: mixed_case,
    },
    Pattern {
        name: "github-token",
        regex: r"(?-u:\b)gh[pousr]_[A-Za-z0-9]{36,}",
        accept: any,
    },
    Pattern {
        name: "github-pat",
        regex: r"(?-u:\b)github_pat_[A-Za-z0-9_]{22,}",
        accept: any,
    },
    Pattern {
        name: "aws-access-key",
        regex: r"(?-u:\b)AKIA[0-9A-Z]{16}(?-u:\b)",
        accept: any,
    },
    Pattern {
        name: "slack-token",
        regex: r"(?-u:\b)xox[abprs]-[A-Za-z0-9\-]{10,}",
        accept: any,
    },
    Pattern {
        name: "google-api-key",
        regex: r"(?-u:\b)AIza[0-9A-Za-z_\-]{35}",
        accept: any,
    },
    Pattern {
        name: "stripe-key",
        regex: r"(?-u:\b)[sr]k_(?:live|test)_[A-Za-z0-9]{16,}|(?-u:\b)whsec_[A-Za-z0-9+/=]{24,}",
        accept: any,
    },
    Pattern {
        name: "jwt",
        regex: r"(?-u:\b)eyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        accept: any,
    },
    Pattern {
        name: "private-key-block",
        regex: r"-----BEGIN [A-Z ]*PRIVATE KEY-----(?:[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----|[\s\S]*)",
        accept: any,
    },
    Pattern {
        name: "authorization-header",
        regex: r"(?i)(?-u:\b)authorization:[ \t]*(?:bearer|basic|token)[ \t]+[A-Za-z0-9._~+/\-]{8,}=*",
        accept: any,
    },
    Pattern {
        name: "bearer-token",
        regex: r"(?i)(?-u:\b)bearer[ \t]+[A-Za-z0-9._~+/\-]{20,}=*",
        accept: bearer_value_is_token,
    },
    Pattern {
        name: "url-credentials",
        regex: r"(?-u:\b)(?P<keep>[A-Za-z][A-Za-z0-9+.\-]*://)[^\s/:@]+:[^\s/@]+@",
        accept: any,
    },
    Pattern {
        name: "secret-assignment",
        regex: r#"(?-u:\b)(?P<keep>[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_KEY|APIKEY|PRIVATE_KEY|ACCESS_KEY|CREDENTIALS|DATABASE_URL|DSN)[A-Z0-9_]*=['"]?)[^\s'"$\[][^\s'"]{3,}"#,
        accept: any,
    },
];

fn any(_: &[u8]) -> bool {
    true
}

/// True for a random-looking key: it has an upper-case letter and a lower-case
/// letter or a digit. A kebab-case name has no upper-case letter.
fn mixed_case(token: &[u8]) -> bool {
    token.iter().any(u8::is_ascii_uppercase)
        && token
            .iter()
            .any(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// True when the word after `bearer` looks like a token, not prose: it has
/// both letter cases, or a digit and either an upper-case letter or only hex
/// characters. `bearer authentication-mechanism-v2` is prose.
fn bearer_value_is_token(matched: &[u8]) -> bool {
    let value = matched
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(matched, |at| matched[at..].trim_ascii_start());
    let upper = value.iter().any(u8::is_ascii_uppercase);
    let lower = value.iter().any(u8::is_ascii_lowercase);
    let digit = value.iter().any(u8::is_ascii_digit);
    let hex = value
        .iter()
        .all(|b| b.is_ascii_hexdigit() || matches!(b, b'-' | b'='));
    (upper && lower) || (digit && (upper || hex))
}

/// Compiles every standard rule; one rule that does not compile fails the
/// whole set, so a pattern edit can never switch a rule off in silence.
fn build(patterns: &[Pattern]) -> Result<Redactor, BadRule> {
    let bad = |pattern: &Pattern, e: regex::Error| BadRule {
        rule: pattern.name,
        reason: e.to_string(),
    };
    let rules = patterns
        .iter()
        .map(|pattern| {
            Ok(Rule {
                name: pattern.name,
                text: Regex::new(pattern.regex).map_err(|e| bad(pattern, e))?,
                bytes: BytesRegex::new(pattern.regex).map_err(|e| bad(pattern, e))?,
                accept: pattern.accept,
            })
        })
        .collect::<Result<Vec<Rule>, BadRule>>()?;
    let key_end = BytesRegex::new(KEY_END).map_err(|e| BadRule {
        rule: "private-key-block",
        reason: e.to_string(),
    })?;
    Ok(Redactor { rules, key_end })
}

static STANDARD: LazyLock<Result<Redactor, BadRule>> = LazyLock::new(|| build(PATTERNS));

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Redactor {
    /// The standard rule set; an error when any of its patterns does not
    /// compile, and then nothing may be captured.
    pub fn standard() -> Result<&'static Redactor, BadRule> {
        STANDARD.as_ref().map_err(Clone::clone)
    }

    /// Where a PEM private key block opened on an earlier line ends in
    /// `bytes`: the offset just past its END line, or `None` when the block
    /// goes on. A block whose BEGIN line has no END after it is redacted to
    /// the end of that text, so a caller that reads line by line uses this
    /// to drop every line up to the END line.
    #[must_use]
    pub fn private_key_end(&self, bytes: &[u8]) -> Option<usize> {
        self.key_end.find(bytes).map(|m| m.end())
    }

    /// True when the last PEM private key BEGIN line in `bytes` has no END
    /// line after it, so the block continues on the next line.
    #[must_use]
    pub fn opens_private_key(&self, bytes: &[u8]) -> bool {
        let Some(rule) = self.rules.iter().find(|r| r.name == "private-key-block") else {
            return false;
        };
        rule.bytes
            .find_iter(bytes)
            .last()
            .is_some_and(|m| self.key_end.find(m.as_bytes()).is_none())
    }

    /// Returns true when any rule matches somewhere in `text`.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        self.rules.iter().any(|rule| {
            rule.text
                .find_iter(text)
                .any(|m| (rule.accept)(m.as_str().as_bytes()))
        })
    }

    /// Returns true when any rule matches somewhere in `bytes`.
    #[must_use]
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.rules.iter().any(|rule| {
            rule.bytes
                .find_iter(bytes)
                .any(|m| (rule.accept)(m.as_bytes()))
        })
    }

    /// Replaces every match with `[REDACTED:<rule>]`, counting per rule;
    /// `None` when nothing matched.
    #[must_use]
    pub fn apply(&self, text: &str, counts: &mut RedactionCounts) -> Option<String> {
        let mut current: Option<String> = None;
        for rule in &self.rules {
            let input = current.as_deref().unwrap_or(text);
            let marker = format!("[REDACTED:{}]", rule.name);
            let mut n = 0usize;
            let replaced = rule.text.replace_all(input, |caps: &regex::Captures<'_>| {
                let found = caps.get(0).map_or("", |m| m.as_str());
                if (rule.accept)(found.as_bytes()) {
                    n += 1;
                    let keep = caps.name("keep").map_or("", |m| m.as_str());
                    format!("{keep}{marker}")
                } else {
                    found.to_owned()
                }
            });
            if n == 0 {
                continue;
            }
            *counts.entry(rule.name).or_insert(0) += count(n);
            current = Some(replaced.into_owned());
        }
        current
    }

    /// `apply` for bytes that are not UTF-8; bytes outside a match are kept.
    #[must_use]
    pub fn apply_bytes(&self, bytes: &[u8], counts: &mut RedactionCounts) -> Option<Vec<u8>> {
        let mut current: Option<Vec<u8>> = None;
        for rule in &self.rules {
            let input = current.as_deref().unwrap_or(bytes);
            let marker = format!("[REDACTED:{}]", rule.name).into_bytes();
            let mut n = 0usize;
            let replaced = rule
                .bytes
                .replace_all(input, |caps: &regex::bytes::Captures<'_>| {
                    let found = caps.get(0).map_or(&b""[..], |m| m.as_bytes());
                    if (rule.accept)(found) {
                        n += 1;
                        let mut out = caps
                            .name("keep")
                            .map_or(&b""[..], |m| m.as_bytes())
                            .to_vec();
                        out.extend_from_slice(&marker);
                        out
                    } else {
                        found.to_vec()
                    }
                });
            if n == 0 {
                continue;
            }
            *counts.entry(rule.name).or_insert(0) += count(n);
            current = Some(replaced.into_owned());
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_pattern_compiles() {
        assert_eq!(Redactor::standard().unwrap().rules.len(), PATTERNS.len());
        let broken = [
            Pattern {
                name: "fine",
                regex: "x",
                accept: any,
            },
            Pattern {
                name: "broken",
                regex: "(",
                accept: any,
            },
        ];
        let err = build(&broken).err().unwrap();
        assert_eq!(err.rule, "broken");
    }

    #[test]
    fn redacts_and_counts() {
        let r = Redactor::standard().unwrap();
        let mut counts = RedactionCounts::new();
        let text = "key sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456 and ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
        let out = r.apply(text, &mut counts).unwrap();
        assert_eq!(
            out,
            "key [REDACTED:anthropic-key] and [REDACTED:github-token]"
        );
        assert_eq!(counts.get("anthropic-key"), Some(&1));
        assert_eq!(counts.get("github-token"), Some(&1));
        assert!(r.apply("nothing secret", &mut counts).is_none());
        assert!(r.matches("Bearer abcdefghijklmnopqrstuvwxyzABC"));
        assert!(r.matches("Authorization: Bearer abcdefghij"));
        assert!(r.matches("-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"));
    }

    #[test]
    fn bytes_that_are_not_utf8_are_redacted_and_kept_around_the_match() {
        let r = Redactor::standard().unwrap();
        let mut counts = RedactionCounts::new();
        let mut input = b"caf\xC3 ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab \xFF".to_vec();
        assert!(r.matches_bytes(&input));
        let out = r.apply_bytes(&input, &mut counts).unwrap();
        assert_eq!(out, b"caf\xC3 [REDACTED:github-token] \xFF");
        assert_eq!(counts.get("github-token"), Some(&1));
        input.truncate(5);
        assert!(r.apply_bytes(&input, &mut counts).is_none());
    }

    #[test]
    fn ordinary_words_and_paths_are_not_secrets() {
        let r = Redactor::standard().unwrap();
        for text in [
            "/Users/efe/oss/disk-usage-analyzer-service/src/main.rs",
            "task-list-item-checkbox-wrapper-component-with-a-long-name",
            "risk-assessment-framework-v2-with-many-more-words-after-it",
            "flask-authentication-demo-app-and-another-long-suffix-here",
            "desk-booking-service-ABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789",
            "the bearer authentication-mechanism-v2 is documented",
            "pass the bearer tokenvalueplaceholdertext to the server",
            "task_live_ABCDEFGHIJKLMNOPQRSTUV and mask_test_ABCDEFGHIJKLMNOP",
        ] {
            assert!(!r.matches(text), "{text} must not match");
            assert!(
                !r.matches_bytes(text.as_bytes()),
                "{text} must not match as bytes"
            );
            assert!(r.apply(text, &mut RedactionCounts::new()).is_none());
        }
    }

    #[test]
    fn real_key_shapes_still_match() {
        let r = Redactor::standard().unwrap();
        for text in [
            "OPENAI_API_KEY=sk-proj-Ab3dEfGhIjKlMnOpQrStUvWxYz0123456789abcd",
            "key sk-Ab3dEfGhIjKlMnOpQrStUvWxYz0123456789abcdEFGH",
            "STRIPE_KEY=sk_live_ABCDEFGHIJKLMNOP1234",
            "secret whsec_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
            "curl -H 'Authorization: Bearer 4f9c2a7e1b3d5f6a8c0e2b4d6f8a0c2e'",
        ] {
            assert!(r.matches(text), "{text} must match");
        }
    }

    #[test]
    fn credentials_in_urls_and_secret_named_assignments_keep_their_names() {
        let r = Redactor::standard().unwrap();
        let mut counts = RedactionCounts::new();
        let text = "DATABASE_URL=postgres://admin:hunter2@db.internal/prod DB_PASSWORD='s3cr3t-value' API_KEY=${API_KEY} PORT=5432";
        assert_eq!(
            r.apply(text, &mut counts).unwrap(),
            "DATABASE_URL=[REDACTED:secret-assignment] DB_PASSWORD='[REDACTED:secret-assignment]' API_KEY=${API_KEY} PORT=5432"
        );
        assert_eq!(counts.get("url-credentials"), Some(&1));
        assert_eq!(counts.get("secret-assignment"), Some(&2));
        assert_eq!(
            r.apply(
                "clone https://bot:ghs_x1@git.example.com/r.git",
                &mut counts
            )
            .unwrap(),
            "clone https://[REDACTED:url-credentials]git.example.com/r.git"
        );
        let once = r
            .apply(
                "GITHUB_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab",
                &mut counts,
            )
            .unwrap();
        assert_eq!(once, "GITHUB_TOKEN=[REDACTED:github-token]");
        assert!(
            r.apply(&once, &mut counts).is_none(),
            "a marker is never redacted again"
        );
    }
}
