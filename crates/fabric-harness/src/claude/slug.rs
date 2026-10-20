//! Claude Code's project directory key.
//!
//! Invariant: the key of a workspace under `~/.claude/projects/` is the
//! physical workspace path with every UTF-16 code unit outside ASCII
//! letters and digits replaced by `-`, so a character outside the Basic
//! Multilingual Plane becomes two dashes. That is the JavaScript
//! `replace(/[^a-zA-Z0-9]/g, "-")` of the Claude Code 2.1.294 bundle,
//! read on 2026-10-20, and its 200 limit counts the same code units,
//! which the all-ASCII key has as many of as it has bytes. The rule is lossy and is never inverted; Fabric
//! recomputes it from the path it knows. Read on 2026-10-19 from
//! `code.claude.com/docs/en/sessions.md` and checked against the owner's
//! own store, where `/Users/efebarandurmaz/Desktop/compass` is filed as
//! `-Users-efebarandurmaz-Desktop-compass` and a `.data` component became
//! `--data`. Claude cuts a key longer than 200 characters and appends a
//! hash whose function the docs do not name, so such a path is refused
//! rather than guessed.

use fabric_core::path::AbsPath;

/// The longest key this module will compute.
pub const MAX_SLUG_LEN: usize = 200;

/// Why a key could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlugError {
    /// The key would be longer than `MAX_SLUG_LEN`, where Claude appends an undocumented hash.
    #[error(
        "project key would be {len} characters, above {MAX_SLUG_LEN}; the hash suffix rule is not known"
    )]
    TooLong {
        /// The key length that was computed.
        len: usize,
    },
}

/// Computes the project directory key of a physical workspace path.
pub fn slug(path: &AbsPath) -> Result<String, SlugError> {
    let mut key = String::with_capacity(path.as_str().len());
    for c in path.as_str().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else {
            key.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if key.len() > MAX_SLUG_LEN {
        return Err(SlugError::TooLong { len: key.len() });
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(path: &str) -> AbsPath {
        AbsPath::parse(path).unwrap()
    }

    #[test]
    fn matches_the_observed_store() {
        assert_eq!(
            slug(&abs("/Users/efebarandurmaz/Desktop/compass")).unwrap(),
            "-Users-efebarandurmaz-Desktop-compass"
        );
        assert_eq!(
            slug(&abs("/Users/x/apps/workspace/.data/y")).unwrap(),
            "-Users-x-apps-workspace--data-y"
        );
        assert_eq!(slug(&abs("/private/tmp/a_b")).unwrap(), "-private-tmp-a-b");
        assert_eq!(slug(&abs("/Users/x/caf\u{e9}")).unwrap(), "-Users-x-caf-");
    }

    #[test]
    fn a_character_outside_the_bmp_becomes_two_dashes() {
        assert_eq!(
            slug(&abs("/Users/x/\u{1f4c1}proj")).unwrap(),
            "-Users-x---proj"
        );
    }

    #[test]
    fn refuses_a_key_claude_would_hash() {
        let long = format!("/{}", "a".repeat(MAX_SLUG_LEN));
        assert_eq!(
            slug(&abs(&long)),
            Err(SlugError::TooLong {
                len: MAX_SLUG_LEN + 1
            })
        );
    }
}
