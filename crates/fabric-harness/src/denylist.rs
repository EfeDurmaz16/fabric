//! Files that never enter a state tree.
//!
//! Invariant: a credential, a login, a machine identity, a lock, a host
//! registry or a cache is refused by name, whichever adapter lists it, and
//! the refusal is checked on the tree path right before the copy, so an
//! adapter bug cannot carry one by accident (ADR 0006, Credentials and
//! terms). The list is hard-coded on purpose: it is policy, not config.

use crate::manifest::HOME_PREFIX;

/// Component names that are a credential or a private key wherever they appear.
const SECRET_NAMES: &[&str] = &[
    ".credentials.json",
    "credentials.json",
    "auth.json",
    ".claude.json",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
    ".coordination.lock",
];

/// Component suffixes that mark a key or a store of one.
const SECRET_SUFFIXES: &[&str] = &[
    ".key",
    ".pem",
    ".p12",
    ".pfx",
    ".keystore",
    ".sqlite",
    ".sqlite-wal",
    ".sqlite-shm",
];

/// Directory names under `~/.claude` that are host state, never session state.
const CLAUDE_HOST_DIRS: &[&str] = &[
    "sessions",
    "session-env",
    "shell-snapshots",
    "jobs",
    "daemon",
    "backups",
    "cache",
    "telemetry",
    "paste-cache",
    "plugins",
    "downloads",
    "chrome",
    "state",
    "statsig",
    "todos",
];

/// File names under `~/.claude` that are host state.
const CLAUDE_HOST_FILES: &[&str] = &[
    "stats-cache.json",
    "daemon.lock",
    "daemon.log",
    "daemon.status.json",
    "daemon-auth-status.json",
    "daemon-auth-cooldown",
    "mcp-needs-auth-cache.json",
    "history.jsonl",
    "launch.json",
    "remote-settings.json",
    "policy-limits.json",
];

/// Directory names under `~/.codex` that are host state.
const CODEX_HOST_DIRS: &[&str] = &[
    "thread-writer-locks",
    "shell_snapshots",
    "process_manager",
    "ipc",
    "mcp-oauth-locks",
    "sqlite",
    "state",
    "tmp",
    "cache",
    "browser",
    "computer-use",
    "worktrees",
];

/// File names under `~/.codex` that are host state or hold a token.
const CODEX_HOST_FILES: &[&str] = &[
    "environments.toml",
    "config.toml",
    "installation_id",
    "history.jsonl",
    "session_index.jsonl",
    "version.json",
    ".codex-global-state.json",
];

fn is_env_file(name: &str) -> bool {
    if name != ".env" && !name.starts_with(".env.") {
        return false;
    }
    !(name.ends_with(".example") || name.ends_with(".sample") || name.ends_with(".template"))
}

/// Returns the rule a tree path breaks, or `None` when it may be carried.
#[must_use]
pub fn check(rel_path: &str) -> Option<&'static str> {
    let components: Vec<&str> = rel_path.split('/').collect();
    for name in &components {
        if SECRET_NAMES.contains(name) {
            return Some("credential file name");
        }
        if SECRET_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
            return Some("key or database file suffix");
        }
        if is_env_file(name) {
            return Some("dotenv file");
        }
        if matches!(*name, ".ssh" | ".aws" | ".gnupg" | ".kube") {
            return Some("credential directory");
        }
    }
    if components.first() == Some(&HOME_PREFIX) {
        match (components.get(1), components.get(2)) {
            (Some(&".claude"), Some(third)) => {
                let is_dir = components.len() > 3;
                if is_dir && CLAUDE_HOST_DIRS.contains(third) {
                    return Some("claude host state directory");
                }
                if !is_dir && CLAUDE_HOST_FILES.contains(third) {
                    return Some("claude host state file");
                }
            }
            (Some(&".codex"), Some(third)) => {
                let is_dir = components.len() > 3;
                if is_dir && CODEX_HOST_DIRS.contains(third) {
                    return Some("codex host state directory");
                }
                if !is_dir && CODEX_HOST_FILES.contains(third) {
                    return Some("codex host state file");
                }
                if *third == "logs" || third.starts_with("logs_") {
                    return Some("codex host state directory");
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_host_state_are_denied() {
        for path in [
            "home/.claude/.credentials.json",
            "home/.claude.json",
            "home/.claude/sessions/123.json",
            "home/.claude/sessions/123.abc.key",
            "home/.claude/jobs/x/state.json",
            "home/.claude/daemon/roster.json",
            "home/.claude/shell-snapshots/snapshot.sh",
            "home/.claude/stats-cache.json",
            "home/.claude/history.jsonl",
            "home/.claude/plugins/x/bin",
            "home/.codex/auth.json",
            "home/.codex/state_5.sqlite",
            "home/.codex/thread_history_1.sqlite-wal",
            "home/.codex/thread-writer-locks/t.lock",
            "home/.codex/.coordination.lock",
            "home/.codex/shell_snapshots/a.sh",
            "home/.codex/environments.toml",
            "home/.codex/config.toml",
            "home/.codex/installation_id",
            "home/.codex/logs_2.sqlite",
            "home/.ssh/id_ed25519",
            "home/.aws/credentials",
            "workspace/.env",
            "workspace/.env.local",
            "workspace/config/server.pem",
            "workspace/.claude/.env.production",
        ] {
            assert!(check(path).is_some(), "{path} should be denied");
        }
    }

    #[test]
    fn session_state_is_allowed() {
        for path in [
            "home/.claude/projects/-Users-x-w/s.jsonl",
            "home/.claude/projects/-Users-x-w/s/subagents/agent-1.jsonl",
            "home/.claude/projects/-Users-x-w/s/tool-results/1.txt",
            "home/.claude/projects/-Users-x-w/memory/MEMORY.md",
            "home/.claude/file-history/s/abc@v1",
            "home/.claude/tasks/session-abcd1234/1.json",
            "home/.claude/settings.json",
            "home/.codex/sessions/2026/10/19/rollout-2026-10-19T10-00-00-abc.jsonl",
            "home/.codex/archived_sessions/rollout-2026-10-19T10-00-00-abc.jsonl.zst",
            "workspace/.claude/settings.local.json",
            "workspace/.env.example",
            "workspace/CLAUDE.local.md",
        ] {
            assert_eq!(check(path), None, "{path} should be allowed");
        }
    }
}
