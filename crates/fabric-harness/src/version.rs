//! The guard that keeps an older harness off a newer session. The version
//! type itself, its parse and its order, is `fabric_core::version`.
//!
//! An older harness must not run a session a newer one wrote (gate
//! finding F10b): Codex 0.142.4 appended records without the `ordinal`
//! field 0.155.1 writes, and wrote a `legacy` thread row into its own
//! database, after which 0.155.1 refused the session. `refuse_older`
//! therefore asks the binary a launch would run for its version before
//! anything starts, and refuses when it is older than the newest version
//! the session records, or when its version cannot be read.

pub(crate) use fabric_core::version::HarnessVersion;

/// True when `version` is at least `floor` by the same order as the
/// resume guard, pre-releases included; false when either does not
/// parse, so a caller that cannot tell takes the cautious path. Serves
/// the floors an adapter was checked on, such as the Codex version whose
/// writer lock it takes.
#[cfg(any(feature = "claude", feature = "codex"))]
pub(crate) fn at_least(version: &str, floor: &str) -> bool {
    match (HarnessVersion::parse(version), HarnessVersion::parse(floor)) {
        (Ok(version), Ok(floor)) => version >= floor,
        _ => false,
    }
}

/// Refuses to start `binary` on `session` when the version it reports is
/// older than `recorded`, the newest version the session records, or
/// cannot be read, and otherwise returns that version, the one about to
/// run the session. Runs `binary --version` in `cwd`, the directory the
/// launch starts it in, with the launch environment minus the
/// parent-session markers, as the launch itself would run.
#[cfg(any(feature = "claude", feature = "codex"))]
pub(crate) async fn refuse_older(
    program: &str,
    binary: &std::path::Path,
    cwd: &std::path::Path,
    env: &[(fabric_core::env::EnvVarName, String)],
    session: &fabric_core::session::SessionId,
    recorded: &HarnessVersion,
) -> Result<HarnessVersion, crate::error::HarnessError> {
    use crate::error::HarnessError;
    let unknown = |reason: String| HarnessError::HarnessVersionUnknown {
        program: program.to_owned(),
        recorded: recorded.to_string(),
        session: session.clone(),
        reason,
    };
    let output = crate::process::reported_version(binary, cwd, env)
        .await
        .map_err(|e| unknown(e.to_string()))?;
    let local = HarnessVersion::from_output(&output).map_err(|e| unknown(e.to_string()))?;
    if local < *recorded {
        return Err(HarnessError::HarnessOlderThanSession {
            program: program.to_owned(),
            local: local.to_string(),
            recorded: recorded.to_string(),
            session: session.clone(),
        });
    }
    Ok(local)
}

#[cfg(all(test, any(feature = "claude", feature = "codex")))]
mod tests {
    use super::*;

    /// A floor an adapter was checked on is compared by the guard's own
    /// order: a desktop pre-release of a later version is above it, a
    /// pre-release of the floor itself is below it, and text that is not
    /// a version is never at least anything.
    #[test]
    fn a_floor_is_compared_by_the_same_order_as_the_guard() {
        assert!(at_least("0.163.0-alpha.10.2", "0.155.1"));
        assert!(at_least("2.1.300-beta", "2.1.294"));
        assert!(at_least("2.1.294", "2.1.294"));
        assert!(at_least("2.10.0", "2.9.999"));
        assert!(!at_least("0.155.1-alpha.3", "0.155.1"));
        assert!(!at_least("2.1.199", "2.1.294"));
        assert!(!at_least("2.1", "2.1.294"));
        assert!(!at_least("2.1.x", "2.1.294"));
    }
}
