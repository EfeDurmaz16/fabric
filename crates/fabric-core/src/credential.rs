//! Recognizing text shaped like a credential.
//!
//! Invariant: text that travels to other nodes in a spec, such as a setup
//! argument, a tool name or a version pin, can be refused when it holds a
//! token, a key or a URL with a password. Only shapes that are certain are
//! recognized, so an ordinary argument is never refused; this is a guard on
//! short text, not a scanner for files.

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
    TokenShape {
        prefix: "sk_live_",
        min_len: 24,
        allowed: alnum,
    },
    TokenShape {
        prefix: "sk_test_",
        min_len: 24,
        allowed: alnum,
    },
    TokenShape {
        prefix: "rk_live_",
        min_len: 24,
        allowed: alnum,
    },
    TokenShape {
        prefix: "rk_test_",
        min_len: 24,
        allowed: alnum,
    },
    TokenShape {
        prefix: "AIza",
        min_len: 35,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "hf_",
        min_len: 34,
        allowed: alnum,
    },
    TokenShape {
        prefix: "pypi-",
        min_len: 50,
        allowed: alnum_dash_underscore,
    },
    TokenShape {
        prefix: "glptt-",
        min_len: 20,
        allowed: alnum_dash_underscore,
    },
];

/// Returns true when `text` holds something shaped like a credential value.
///
/// Checks the shapes that are certain: GitHub, GitLab (personal and
/// pipeline trigger), npm, Slack, Hugging Face and `PyPI` tokens, `OpenAI`
/// and Anthropic style API keys, Stripe secret and restricted keys, Google
/// API keys, AWS access key ids, a PEM block header and a URL with a
/// password before the host. It is a guard on
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

    /// One positive and one near miss per fixed-prefix shape: Stripe,
    /// Google, Hugging Face, `PyPI` and GitLab trigger tokens.
    #[test]
    fn fixed_prefix_token_shapes_are_recognized_and_their_near_misses_are_not() {
        let body = "a1B2".repeat(13);
        let cases = [
            (
                format!("--api-key=sk_live_{}", &body[..24]),
                "sk_live_short",
            ),
            (format!("sk_test_{}", &body[..24]), "sk_test_"),
            (format!("rk_live_{}", &body[..24]), "rk_live_x1"),
            (
                format!("rk_test_{}", &body[..24]),
                "work_test_abcdefghijklmnopqrstuvwxyz",
            ),
            (format!("AIza{}", &body[..35]), "AIzaShort"),
            (format!("hf_{}", &body[..34]), "hf_hub_download"),
            (format!("pypi-{}", &body[..50]), "pypi-simple-index"),
            (format!("glptt-{}", &body[..20]), "glptt-x"),
        ];
        for (token, near_miss) in cases {
            assert!(looks_like_credential(&token), "{token}");
            assert!(!looks_like_credential(near_miss), "{near_miss}");
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
