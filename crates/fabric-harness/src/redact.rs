//! Secret redaction.
//!
//! Invariant: a redaction rule never leaves part of a match behind, and a
//! text that is not UTF-8 is matched as bytes with the same rules, so a
//! line cut inside a multi-byte character is redacted like any other.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use regex::bytes::Regex as BytesRegex;

/// Per-rule match counts.
pub type RedactionCounts = BTreeMap<&'static str, u32>;

/// Secret-shaped text that must never leave the machine it was typed on.
pub struct Redactor {
    rules: Vec<Rule>,
}

struct Rule {
    name: &'static str,
    text: Regex,
    bytes: BytesRegex,
}

const PATTERNS: &[(&str, &str)] = &[
    ("anthropic-key", r"sk-ant-[A-Za-z0-9_\-]{20,}"),
    ("openai-key", r"sk-(?:proj-|svcacct-)?[A-Za-z0-9_\-]{20,}"),
    ("github-token", r"gh[pousr]_[A-Za-z0-9]{36,}"),
    ("github-pat", r"github_pat_[A-Za-z0-9_]{22,}"),
    ("aws-access-key", r"AKIA[0-9A-Z]{16}"),
    ("slack-token", r"xox[abprs]-[A-Za-z0-9\-]{10,}"),
    ("google-api-key", r"AIza[0-9A-Za-z_\-]{35}"),
    ("stripe-key", r"[sr]k_(?:live|test)_[A-Za-z0-9]{16,}"),
    (
        "jwt",
        r"eyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
    ),
    (
        "private-key-block",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ),
    ("bearer-token", r"(?i)\bbearer\s+[A-Za-z0-9._\-]{20,}"),
];

static STANDARD: LazyLock<Redactor> = LazyLock::new(|| Redactor {
    rules: PATTERNS
        .iter()
        .filter_map(|(name, pattern)| {
            let text = Regex::new(pattern).ok()?;
            let bytes = BytesRegex::new(pattern).ok()?;
            Some(Rule { name, text, bytes })
        })
        .collect(),
});

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Redactor {
    /// The standard rule set.
    #[must_use]
    pub fn standard() -> &'static Redactor {
        &STANDARD
    }

    /// Returns true when any rule matches somewhere in `text`.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        self.rules.iter().any(|rule| rule.text.is_match(text))
    }

    /// Returns true when any rule matches somewhere in `bytes`.
    #[must_use]
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.rules.iter().any(|rule| rule.bytes.is_match(bytes))
    }

    /// Replaces every match with `[REDACTED:<rule>]`, counting per rule;
    /// `None` when nothing matched.
    #[must_use]
    pub fn apply(&self, text: &str, counts: &mut RedactionCounts) -> Option<String> {
        let mut current: Option<String> = None;
        for rule in &self.rules {
            let input = current.as_deref().unwrap_or(text);
            let n = rule.text.find_iter(input).count();
            if n == 0 {
                continue;
            }
            let replaced = rule
                .text
                .replace_all(input, format!("[REDACTED:{}]", rule.name).as_str())
                .into_owned();
            *counts.entry(rule.name).or_insert(0) += count(n);
            current = Some(replaced);
        }
        current
    }

    /// `apply` for bytes that are not UTF-8; bytes outside a match are kept.
    #[must_use]
    pub fn apply_bytes(&self, bytes: &[u8], counts: &mut RedactionCounts) -> Option<Vec<u8>> {
        let mut current: Option<Vec<u8>> = None;
        for rule in &self.rules {
            let input = current.as_deref().unwrap_or(bytes);
            let n = rule.bytes.find_iter(input).count();
            if n == 0 {
                continue;
            }
            let replaced = rule
                .bytes
                .replace_all(input, format!("[REDACTED:{}]", rule.name).as_bytes())
                .into_owned();
            *counts.entry(rule.name).or_insert(0) += count(n);
            current = Some(replaced);
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_pattern_compiles() {
        assert_eq!(Redactor::standard().rules.len(), PATTERNS.len());
    }

    #[test]
    fn redacts_and_counts() {
        let r = Redactor::standard();
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
        assert!(r.matches("Bearer abcdefghijklmnopqrstuvwxyz"));
        assert!(r.matches("-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"));
    }

    #[test]
    fn bytes_that_are_not_utf8_are_redacted_and_kept_around_the_match() {
        let r = Redactor::standard();
        let mut counts = RedactionCounts::new();
        let mut input = b"caf\xC3 ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab \xFF".to_vec();
        assert!(r.matches_bytes(&input));
        let out = r.apply_bytes(&input, &mut counts).unwrap();
        assert_eq!(out, b"caf\xC3 [REDACTED:github-token] \xFF");
        assert_eq!(counts.get("github-token"), Some(&1));
        input.truncate(5);
        assert!(r.apply_bytes(&input, &mut counts).is_none());
    }
}
