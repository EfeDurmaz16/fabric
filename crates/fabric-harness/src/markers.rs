//! Parent-session markers that a harness child must not inherit.
//!
//! Invariant: a harness started by Fabric is a session of its own, never a
//! child of the Claude Code or Codex session Fabric itself may run under.
//! Its environment is the one the caller passed minus the names below; a
//! user's own configuration (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//! `HTTPS_PROXY`, `PATH`, `CODEX_HOME`) always passes, and a name a user
//! may set is left out only where the evidence shows a host set it.
//!
//! Why: on the first gate run (2026-10-20, gate finding F2) a `claude`
//! started from inside a Claude Code session printed that transcript
//! saving was off because of an inherited `CLAUDE_CODE_CHILD_SESSION`
//! marker, so a job launched that way left no transcript to resume.
//!
//! Evidence. The Claude names were read on 2026-10-20 from the Claude
//! Code 2.1.294 bundle and from the environment of a Claude desktop
//! session on the owner's machine, names only. The bundle builds the
//! environment of every process a session starts as `CLAUDECODE`,
//! `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`,
//! `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_PID`, then `AI_AGENT` when an
//! agent started it, `CLAUDE_EFFORT` and `TRACEPARENT`; its list of the
//! variables it sets on its own shells adds `GIT_EDITOR`,
//! `GIT_CONFIG_PARAMETERS`, `BUN_OPTIONS`, `CLAUDE_CODE_EXECPATH` and
//! `CLAUDE_CODE_INVOKED_SKILLS`. The desktop session also carried the
//! host's messaging and identity names, `ANTHROPIC_BASE_URL`, `BAGGAGE`,
//! `MCP_CONNECTION_NONBLOCKING`, `MCP_SERVER_CONNECTION_BATCH_SIZE` and
//! feature switches such as `CLAUDE_CODE_SIMPLE` and
//! `CLAUDE_CODE_DISABLE_CRON`. The Codex names were read from
//! openai/codex at `e4568a3`: `CODEX_THREAD_ID` and `CODEX_TOOL_CALL_ID`
//! (`codex-rs/protocol/src/shell_environment.rs`), `CODEX_SESSION_ID`,
//! `CODEX_VERSION`, `CODEX_PERMISSION_PROFILE` and
//! `CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS` (`codex-rs/core/src/exec_env.rs`).
//!
//! The rules:
//!
//! - Always left out: the names that only a host sets, which say whose
//!   child a process is. `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, the
//!   session, attendance, host session, messaging, `EXECPATH` and invoked
//!   skills names under `CLAUDE_CODE_`, `CLAUDE_PID`, `CLAUDE_EFFORT`,
//!   `AI_AGENT`, every `CLAUDE_AGENT_SDK_*` and `CLAUDE_PREVIEW_*`, and the
//!   Codex names above, which name the parent thread and tool call; Codex
//!   reads none of them at startup and sets them again for its own
//!   children. These do not depend on telling a host session apart, so a
//!   caller that filtered `CLAUDECODE` away still loses them.
//! - Left out in any Claude host session, that is when `CLAUDECODE` or
//!   `CLAUDE_CODE_ENTRYPOINT` is present: `TRACEPARENT` and `BAGGAGE`,
//!   which would make the child's telemetry a span of the parent's trace,
//!   and `GIT_EDITOR`, `GIT_CONFIG_PARAMETERS` and `BUN_OPTIONS`, which the
//!   host sets for its own tool shells (`GIT_EDITOR=true` turns every
//!   editor git opens into a no-op). A user may set any of these outside a
//!   host session, and then they pass.
//! - Left out only under the desktop host, when `CLAUDE_CODE_ENTRYPOINT`
//!   is `claude-desktop`: every `CLAUDE_CODE_*`, `ANTHROPIC_BASE_URL`,
//!   `MCP_CONNECTION_NONBLOCKING` and `MCP_SERVER_CONNECTION_BATCH_SIZE`.
//!   The desktop host points `ANTHROPIC_BASE_URL` at an endpoint it
//!   chooses, switches features of the child with `CLAUDE_CODE_*`, and
//!   makes a headless turn start before its MCP servers connect with the
//!   two `MCP_` names. A terminal `claude` (`CLAUDE_CODE_ENTRYPOINT=cli`)
//!   sets none of these, so there the user's own gateway URL,
//!   `CLAUDE_CODE_USE_BEDROCK` and MCP settings pass. Under the desktop
//!   host a user's own `CLAUDE_CODE_*` setting is lost too; it belongs in
//!   a settings overlay. `MCP_TIMEOUT` and `MCP_TOOL_TIMEOUT` are user
//!   settings and always pass. Other embedding hosts Claude knows
//!   (`claude-vscode`, `claude-desktop-3p`) were not observed, so they get
//!   the first two rules only.
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
    Name::Is("CLAUDE_CODE_ENTRYPOINT"),
    Name::Is("CLAUDE_CODE_SESSION_ID"),
    Name::Is("CLAUDE_CODE_CHILD_SESSION"),
    Name::Is("CLAUDE_CODE_SESSION_ATTENDED"),
    Name::Is("CLAUDE_CODE_HOST_SESSION_ID"),
    Name::Is("CLAUDE_CODE_MESSAGING_SOCKET"),
    Name::Is("CLAUDE_CODE_MESSAGING_TOKEN"),
    Name::Is("CLAUDE_CODE_EXECPATH"),
    Name::Is("CLAUDE_CODE_INVOKED_SKILLS"),
    Name::Is("CLAUDE_PID"),
    Name::Is("CLAUDE_EFFORT"),
    Name::Is("AI_AGENT"),
    Name::Starts("CLAUDE_AGENT_SDK_"),
    Name::Starts("CLAUDE_PREVIEW_"),
    Name::Is("CODEX_THREAD_ID"),
    Name::Is("CODEX_TOOL_CALL_ID"),
    Name::Is("CODEX_SESSION_ID"),
    Name::Is("CODEX_VERSION"),
    Name::Is("CODEX_PERMISSION_PROFILE"),
    Name::Is("CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS"),
];

/// Names a user may set, which are markers in an environment inherited
/// from any Claude host session.
const HOSTED: &[Name] = &[
    Name::Is("TRACEPARENT"),
    Name::Is("BAGGAGE"),
    Name::Is("GIT_EDITOR"),
    Name::Is("GIT_CONFIG_PARAMETERS"),
    Name::Is("BUN_OPTIONS"),
];

/// Names a user may set, which are markers in an environment inherited
/// from the Claude desktop host.
const DESKTOP: &[Name] = &[
    Name::Starts("CLAUDE_CODE_"),
    Name::Is("ANTHROPIC_BASE_URL"),
    Name::Is("MCP_CONNECTION_NONBLOCKING"),
    Name::Is("MCP_SERVER_CONNECTION_BATCH_SIZE"),
];

/// The variable that names the host a session runs under.
const ENTRYPOINT: &str = "CLAUDE_CODE_ENTRYPOINT";

/// The entrypoint value of the Claude desktop host.
const DESKTOP_ENTRYPOINT: &str = "claude-desktop";

/// Where an environment comes from, as far as its markers tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    /// No Claude host session.
    None,
    /// A Claude host session other than the desktop host.
    Claude,
    /// The Claude desktop host.
    Desktop,
}

impl Host {
    fn of(env: &[(EnvVarName, String)]) -> Self {
        let value = |wanted: &str| {
            env.iter()
                .find(|(name, _)| name.as_str() == wanted)
                .map(|(_, value)| value.as_str())
        };
        match (value("CLAUDECODE"), value(ENTRYPOINT)) {
            (_, Some(DESKTOP_ENTRYPOINT)) => Host::Desktop,
            (None, None) => Host::None,
            _ => Host::Claude,
        }
    }

    /// True when `name` is a parent-session marker in an environment
    /// from this host.
    fn marks(self, name: &str) -> bool {
        let any = |rules: &[Name]| rules.iter().any(|rule| rule.matches(name));
        any(ALWAYS)
            || (self != Host::None && any(HOSTED))
            || (self == Host::Desktop && any(DESKTOP))
    }
}

/// The variables of `env` a harness child receives: all of them except
/// the parent-session markers. The names left out are logged at debug
/// level, never their values.
pub(crate) fn child_env(env: &[(EnvVarName, String)]) -> Vec<(&str, &str)> {
    let host = Host::of(env);
    let (kept, left_out): (Vec<_>, Vec<_>) = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .partition(|(name, _)| !host.marks(name));
    if !left_out.is_empty() {
        let names: Vec<&str> = left_out.iter().map(|(name, _)| *name).collect();
        debug!(
            ?names,
            ?host,
            "harness: parent-session markers left out of the child environment"
        );
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names of `names` a child keeps, under the host `entrypoint`
    /// names (none when `None`).
    fn kept(entrypoint: Option<&str>, names: &[&str]) -> Vec<String> {
        let mut env: Vec<(EnvVarName, String)> = entrypoint
            .map(|value| (EnvVarName::parse(ENTRYPOINT).unwrap(), value.to_owned()))
            .into_iter()
            .collect();
        env.extend(
            names
                .iter()
                .map(|name| (EnvVarName::parse(name).unwrap(), format!("v-{name}"))),
        );
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
        "MCP_TIMEOUT",
        "MCP_TOOL_TIMEOUT",
    ];

    /// Names only a host sets.
    const IDENTITY: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_HOST_SESSION_ID",
        "CLAUDE_CODE_MESSAGING_SOCKET",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_CODE_INVOKED_SKILLS",
        "CLAUDE_PID",
        "CLAUDE_EFFORT",
        "AI_AGENT",
        "CLAUDE_AGENT_SDK_VERSION",
        "CLAUDE_PREVIEW_CLASSIFIER_FLOOR",
        "CODEX_THREAD_ID",
        "CODEX_TOOL_CALL_ID",
        "CODEX_SESSION_ID",
        "CODEX_VERSION",
        "CODEX_PERMISSION_PROFILE",
        "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS",
    ];

    /// Names a host sets for its tool shells and a user may also set.
    const SHELL: &[&str] = &[
        "TRACEPARENT",
        "BAGGAGE",
        "GIT_EDITOR",
        "GIT_CONFIG_PARAMETERS",
        "BUN_OPTIONS",
    ];

    /// Names the desktop host sets and a user may also set.
    const PROVIDER: &[&str] = &[
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_SIMPLE",
        "MCP_CONNECTION_NONBLOCKING",
        "MCP_SERVER_CONNECTION_BATCH_SIZE",
    ];

    fn all() -> Vec<&'static str> {
        [IDENTITY, SHELL, PROVIDER, USER].concat()
    }

    #[test]
    fn the_desktop_host_loses_every_marker_and_keeps_user_configuration() {
        assert_eq!(kept(Some(DESKTOP_ENTRYPOINT), &all()), USER);
    }

    #[test]
    fn a_terminal_host_keeps_the_users_provider_settings() {
        assert_eq!(
            kept(Some("cli"), &all()),
            [PROVIDER, USER].concat(),
            "a terminal claude sets no gateway, feature switch or MCP name"
        );
    }

    #[test]
    fn outside_a_host_session_only_identity_markers_are_left_out() {
        let names: Vec<&str> = all()
            .into_iter()
            .filter(|name| *name != "CLAUDECODE")
            .collect();
        assert_eq!(kept(None, &names), [SHELL, PROVIDER, USER].concat());
    }

    #[test]
    fn claudecode_alone_marks_a_host_session() {
        assert_eq!(
            kept(None, &["CLAUDECODE", "GIT_EDITOR", "ANTHROPIC_BASE_URL"]),
            ["ANTHROPIC_BASE_URL"]
        );
    }
}
