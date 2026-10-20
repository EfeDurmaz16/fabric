//! Claude Code's project directory key.
//!
//! Invariant: the key of a workspace under `~/.claude/projects/` is the
//! physical workspace path with every character outside ASCII letters and
//! digits replaced by `-`. The rule is lossy and is never inverted; Fabric
//! recomputes it from the path it knows. Read on 2026-10-19 from
//! `code.claude.com/docs/en/sessions.md` and checked against the owner's
//! own store, where `/Users/efebarandurmaz/Desktop/compass` is filed as
//! `-Users-efebarandurmaz-Desktop-compass` and a `.data` component became
//! `--data`. Claude cuts a key longer than 200 characters and appends a
//! hash whose function the docs do not name, so such a path is refused
//! rather than guessed.

use std::path::Path;

/// The longest key this module will compute.
pub const MAX_SLUG_LEN: usize = 200;

/// Why a key could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlugError {
    /// The path is not UTF-8.
    #[error("workspace path is not UTF-8")]
    NotUtf8,
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
pub fn slug(path: &Path) -> Result<String, SlugError> {
    let text = path.to_str().ok_or(SlugError::NotUtf8)?;
    let key: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if key.len() > MAX_SLUG_LEN {
        return Err(SlugError::TooLong { len: key.len() });
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_observed_store() {
        assert_eq!(
            slug(Path::new("/Users/efebarandurmaz/Desktop/compass")).unwrap(),
            "-Users-efebarandurmaz-Desktop-compass"
        );
        assert_eq!(
            slug(Path::new("/Users/x/apps/workspace/.data/y")).unwrap(),
            "-Users-x-apps-workspace--data-y"
        );
        assert_eq!(
            slug(Path::new("/private/tmp/a_b")).unwrap(),
            "-private-tmp-a-b"
        );
        assert_eq!(
            slug(Path::new("/Users/x/caf\u{e9}")).unwrap(),
            "-Users-x-caf-"
        );
    }

    #[test]
    fn refuses_a_key_claude_would_hash() {
        let long = format!("/{}", "a".repeat(MAX_SLUG_LEN));
        assert_eq!(
            slug(Path::new(&long)),
            Err(SlugError::TooLong {
                len: MAX_SLUG_LEN + 1
            })
        );
    }
}
