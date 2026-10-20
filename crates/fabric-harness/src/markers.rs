//! Parent-session markers that a harness child must not inherit.
//!
//! Invariant: a harness started by Fabric is a session of its own, never a
//! child of the Claude Code or Codex session Fabric itself may run under.
//! Its environment is the one the caller passed minus the names below; a
//! user's own configuration (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//! `HTTPS_PROXY`, `PATH`, `CODEX_HOME`) always passes.
//!
//! Why: on the first gate run (2026-10-20, gate finding F2) a `claude`
//! started from inside a Claude Code session printed that transcript
//! saving was off because of an inherited `CLAUDE_CODE_CHILD_SESSION`
//! marker, so a job launched that way left no transcript to resume. The
//! host session's environment carried about 30 such names; the Claude
//! names below were read from it on the owner's machine the same day,
//! names only.
//!
//! The rules:
//!
//! - Always left out: `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`, every
//!   `CLAUDE_AGENT_SDK_*` and `CLAUDE_PREVIEW_*`, which only a host
//!   session sets, and the markers Codex sets on the processes its tools
//!   start. Those were read from openai/codex at `e4568a3`:
//!   `CODEX_THREAD_ID` and `CODEX_TOOL_CALL_ID`
//!   (`codex-rs/protocol/src/shell_environment.rs`), `CODEX_SESSION_ID`,
//!   `CODEX_VERSION`, `CODEX_PERMISSION_PROFILE` and
//!   `CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS`
//!   (`codex-rs/core/src/exec_env.rs`). They name the parent thread and
//!   tool call; Codex reads none of them at startup and sets them again
//!   for its own children.
//! - Left out only when the environment comes from a host session, that
//!   is when `CLAUDECODE` or `CLAUDE_CODE_ENTRYPOINT` is present: every
//!   `CLAUDE_CODE_*` and `ANTHROPIC_BASE_URL`. A user may set these
//!   (`CLAUDE_CODE_USE_BEDROCK`, a proxy base URL), but inside a host
//!   session the Claude desktop host sets `ANTHROPIC_BASE_URL` to an
//!   endpoint of its own on the same machine, and the `CLAUDE_CODE_*` names
//!   it sets switch features of the child (`CLAUDE_CODE_CHILD_SESSION`,
//!   `CLAUDE_CODE_SIMPLE`, `CLAUDE_CODE_DISABLE_CRON`). Inside a host
//!   session a user's own `CLAUDE_CODE_*` setting is lost too; it belongs
//!   in a settings overlay.
//! - Kept: what Codex sets to describe a confinement the child really
//!   shares. `CODEX_SANDBOX` (`codex-rs/core/src/sandboxing/mod.rs`),
//!   `CODEX_SANDBOX_NETWORK_DISABLED` (`codex-rs/core/src/spawn.rs`) and
//!   the network proxy variables (`CODEX_NETWORK_PROXY_*`,
//!   `CODEX_NETWORK_ALLOW_LOCAL_BINDING` and the proxy URLs,
//!   `codex-rs/network-proxy/src/proxy.rs`) do change a Codex that
//!   inherits them: with `CODEX_SANDBOX=seatbelt` its HTTP client skips
//!   proxies (`codex-rs/login/src/auth/default_client.rs`), and with
//!   `CODEX_SANDBOX_NETWORK_DISABLED` its Ollama and LM Studio providers
//!   stay off the network. But the seatbelt sandbox and the network proxy
//!   pass to every descendant process, so the markers stay true, and
//!   removing them would send a confined Codex down a path its sandbox
//!   blocks.

use fabric_core::env::EnvVarName;
use tracing::debug;

/// A rule that matches variable names.
#[derive(Debug, Clone, Copy)]
enum Name {
    /// Exactly this name.
    Is(&'static str),
    /// Any name that starts with this prefix.
    Starts(&'static str),
}

impl Name {
    fn matches(self, name: &str) -> bool {
        match self {
            Name::Is(exact) => name == exact,
            Name::Starts(prefix) => name.starts_with(prefix),
        }
    }
}

/// Names that are parent-session markers wherever they appear.
const ALWAYS: &[Name] = &[
    Name::Is("CLAUDECODE"),
    Name::Is("CLAUDE_PID"),
    Name::Is("CLAUDE_EFFORT"),
    Name::Starts("CLAUDE_AGENT_SDK_"),
    Name::Starts("CLAUDE_PREVIEW_"),
    Name::Is("CODEX_THREAD_ID"),
    Name::Is("CODEX_TOOL_CALL_ID"),
    Name::Is("CODEX_SESSION_ID"),
    Name::Is("CODEX_VERSION"),
    Name::Is("CODEX_PERMISSION_PROFILE"),
    Name::Is("CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS"),
];

/// Names a user may set, which are markers only in an environment
/// inherited from a host session.
const HOSTED: &[Name] = &[Name::Starts("CLAUDE_CODE_"), Name::Is("ANTHROPIC_BASE_URL")];

/// Names whose presence says the environment comes from a Claude Code
/// session.
const HOST_SESSION: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];

/// True when `name` is a parent-session marker in an environment that
/// does (`hosted`) or does not come from a host session.
fn is_marker(name: &str, hosted: bool) -> bool {
    ALWAYS.iter().any(|rule| rule.matches(name))
        || (hosted && HOSTED.iter().any(|rule| rule.matches(name)))
}

/// The variables of `env` a harness child receives: all of them except
/// the parent-session markers. The names left out are logged at debug
/// level, never their values.
pub(crate) fn child_env(env: &[(EnvVarName, String)]) -> Vec<(&str, &str)> {
    let hosted = env
        .iter()
        .any(|(name, _)| HOST_SESSION.contains(&name.as_str()));
    let (kept, left_out): (Vec<_>, Vec<_>) = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .partition(|(name, _)| !is_marker(name, hosted));
    if !left_out.is_empty() {
        let names: Vec<&str> = left_out.iter().map(|(name, _)| *name).collect();
        debug!(
            ?names,
            hosted, "harness: parent-session markers left out of the child environment"
        );
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(names: &[&str]) -> Vec<(EnvVarName, String)> {
        names
            .iter()
            .map(|name| (EnvVarName::parse(name).unwrap(), format!("v-{name}")))
            .collect()
    }

    fn kept(names: &[&str]) -> Vec<String> {
        let env = env(names);
        child_env(&env)
            .into_iter()
            .map(|(name, value)| {
                assert_eq!(value, format!("v-{name}"), "values pass unchanged");
                name.to_owned()
            })
            .collect()
    }

    const USER: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "HTTPS_PROXY",
        "PATH",
        "HOME",
        "CODEX_HOME",
        "CODEX_SANDBOX",
        "CODEX_NETWORK_PROXY_ACTIVE",
        "DISABLE_AUTOUPDATER",
    ];

    #[test]
    fn a_host_session_loses_every_marker_and_keeps_user_configuration() {
        let mut names = vec![
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_PID",
            "CLAUDE_EFFORT",
            "CLAUDE_AGENT_SDK_VERSION",
            "CLAUDE_PREVIEW_CLASSIFIER_FLOOR",
            "ANTHROPIC_BASE_URL",
            "CODEX_THREAD_ID",
            "CODEX_TOOL_CALL_ID",
            "CODEX_SESSION_ID",
            "CODEX_VERSION",
            "CODEX_PERMISSION_PROFILE",
            "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS",
        ];
        names.extend(USER);
        assert_eq!(kept(&names), USER);
    }

    #[test]
    fn outside_a_host_session_a_users_claude_settings_pass() {
        let mut names = vec![
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CODEX_THREAD_ID",
        ];
        names.extend(USER);
        let mut expected = vec!["ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK"];
        expected.extend(USER);
        assert_eq!(kept(&names), expected);
    }

    #[test]
    fn the_entrypoint_alone_marks_a_host_session() {
        assert_eq!(
            kept(&["CLAUDE_CODE_ENTRYPOINT", "ANTHROPIC_BASE_URL", "PATH"]),
            ["PATH"]
        );
    }
}
