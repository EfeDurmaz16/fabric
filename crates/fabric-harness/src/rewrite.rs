//! Text rules: path mapping at component boundaries and secret redaction.
//!
//! Invariant: a path rule replaces `from` only where it stands as a whole
//! path component run, never inside a longer name, so `/Users/x` is mapped
//! in `/Users/x/oss` and left alone in `/Users/x-old`. Rules are applied
//! longest first, so a root under a home is mapped before the home is. A
//! redaction rule never leaves part of a match behind, and a text that is
//! not UTF-8 is matched as bytes with the same rules.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use regex::bytes::Regex as BytesRegex;

/// An ordered set of `from -> to` path rules.
#[derive(Debug, Clone, Default)]
pub struct PathMap {
    rules: Vec<(String, String)>,
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

impl PathMap {
    /// Builds a map from pairs, dropping identity pairs and ordering the
    /// rest longest `from` first.
    #[must_use]
    pub fn new(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut rules: Vec<(String, String)> = pairs
            .into_iter()
            .filter(|(from, to)| !from.is_empty() && from != to)
            .collect();
        rules.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        rules.dedup_by(|a, b| a.0 == b.0);
        Self { rules }
    }

    /// Returns true when the map has no rule.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns the rules, longest first.
    #[must_use]
    pub fn rules(&self) -> &[(String, String)] {
        &self.rules
    }

    /// Applies the rules to `text`; `None` when nothing matched.
    #[must_use]
    pub fn apply(&self, text: &str) -> Option<String> {
        if self.rules.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut changed = false;
        let mut i = 0;
        let bytes = text.as_bytes();
        while i < text.len() {
            let mut matched = false;
            if text.is_char_boundary(i) {
                let before_ok = i == 0
                    || text[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| !is_path_char(c));
                if before_ok {
                    for (from, to) in &self.rules {
                        if !text[i..].starts_with(from.as_str()) {
                            continue;
                        }
                        let end = i + from.len();
                        let after_ok = end == text.len()
                            || text[end..].chars().next().is_some_and(|c| !is_path_char(c));
                        if after_ok {
                            out.push_str(to);
                            i = end;
                            matched = true;
                            changed = true;
                            break;
                        }
                    }
                }
            }
            if !matched {
                // Copy one byte; a multi-byte char is copied over several
                // iterations, none of which can start a match because the
                // boundary check above requires a char boundary.
                let next = (i + 1..=text.len())
                    .find(|&j| text.is_char_boundary(j))
                    .unwrap_or(text.len());
                out.push_str(&text[i..next]);
                i = next;
            }
            debug_assert!(i <= bytes.len());
        }
        changed.then_some(out)
    }
}

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

    /// The number of rules in force.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// True when the set is empty, which the standard set never is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> PathMap {
        PathMap::new(
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned())),
        )
    }

    #[test]
    fn maps_whole_components_only() {
        let m = map(&[("/Users/x", "/Users/y")]);
        assert_eq!(m.apply("/Users/x/oss"), Some("/Users/y/oss".to_owned()));
        assert_eq!(m.apply("/Users/x"), Some("/Users/y".to_owned()));
        assert_eq!(
            m.apply("cd /Users/x && ls"),
            Some("cd /Users/y && ls".to_owned())
        );
        assert_eq!(m.apply("/Users/x-old/oss"), None);
        assert_eq!(m.apply("/Users/xy"), None);
        assert_eq!(m.apply("a/Users/x"), None);
        assert_eq!(m.apply("no path here"), None);
    }

    #[test]
    fn longest_rule_wins_and_identity_is_dropped() {
        let m = map(&[
            ("/Users/x", "/Users/y"),
            ("/Users/x/w", "/Users/y/jobs/w"),
            ("/same", "/same"),
        ]);
        assert_eq!(m.rules().len(), 2);
        assert_eq!(m.rules()[0].0, "/Users/x/w");
        assert_eq!(
            m.apply("/Users/x/w/src and /Users/x/other"),
            Some("/Users/y/jobs/w/src and /Users/y/other".to_owned())
        );
    }

    #[test]
    fn slug_rules_work_through_the_same_boundary() {
        let m = map(&[("-Users-x-w", "-Users-y-w")]);
        assert_eq!(
            m.apply("/h/.claude/projects/-Users-x-w/s.jsonl"),
            Some("/h/.claude/projects/-Users-y-w/s.jsonl".to_owned())
        );
        assert_eq!(m.apply("/h/.claude/projects/-Users-x-w2/s.jsonl"), None);
    }

    #[test]
    fn multibyte_text_is_copied_intact() {
        let m = map(&[("/a", "/b")]);
        assert_eq!(
            m.apply("caf\u{e9} /a \u{1F600}"),
            Some("caf\u{e9} /b \u{1F600}".to_owned())
        );
        assert_eq!(m.apply("\u{e9}/a"), Some("\u{e9}/b".to_owned()));
    }

    #[test]
    fn every_standard_pattern_compiles() {
        assert_eq!(Redactor::standard().len(), PATTERNS.len());
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
}
