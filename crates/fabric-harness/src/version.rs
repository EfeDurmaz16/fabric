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
/// parent-session markers, as the launch itself would run, or takes the
/// answer of `ahead` when it asked exactly that.
#[cfg(any(feature = "claude", feature = "codex"))]
pub(crate) async fn refuse_older(
    program: &str,
    (binary, cwd, env): (
        &std::path::Path,
        &std::path::Path,
        &[(fabric_core::env::EnvVarName, String)],
    ),
    ahead: Option<&VersionAhead>,
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
    let output = reported_version(ahead, binary, cwd, env)
        .await
        .map_err(unknown)?;
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

/// A `--version` started before the step that reads it, so the probe, a
/// process start of tens of milliseconds, runs beside other work: the
/// daemon starts the one a harness restore asks as the restore begins,
/// and the CLI the one its resume guard asks as soon as the node names
/// the session the launch continues (items 14 and 15 of
/// docs/evidence/perf-ab/README.md).
///
/// The step takes the answer only when it would have asked the same
/// question: the same binary, in the same directory, with the same
/// environment, or the same `PATH` for a probe of the installed version.
/// Otherwise it asks again, so nothing is decided on what another binary
/// or another shim answered. A probe nobody takes is stopped when the
/// last copy of this handle goes.
#[cfg(any(feature = "claude", feature = "codex"))]
#[derive(Debug, Clone)]
pub struct VersionAhead(std::sync::Arc<Ahead>);

#[cfg(any(feature = "claude", feature = "codex"))]
#[derive(Debug)]
pub(crate) struct Ahead {
    question: Question,
    answer: tokio::sync::Mutex<Answer>,
}

/// What a probe asked ahead was asked.
#[cfg(any(feature = "claude", feature = "codex"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Question {
    /// The installed version, as `process::installed_version` asks it:
    /// `binary` found on `path_var`, run in this process's directory.
    Installed {
        binary: Option<std::path::PathBuf>,
        path_var: std::ffi::OsString,
    },
    /// What `binary --version` prints in `cwd` with `env`, as
    /// `process::reported_version` asks it for the resume guard.
    Reported {
        binary: std::path::PathBuf,
        cwd: std::path::PathBuf,
        env: Vec<(fabric_core::env::EnvVarName, String)>,
    },
}

/// The probe's answer: its output, or why it failed, in words.
#[cfg(any(feature = "claude", feature = "codex"))]
type Reply = Result<Option<String>, String>;

#[cfg(any(feature = "claude", feature = "codex"))]
#[derive(Debug)]
enum Answer {
    Running(tokio::task::JoinHandle<Reply>),
    Done(Reply),
}

#[cfg(any(feature = "claude", feature = "codex"))]
impl Drop for Ahead {
    fn drop(&mut self) {
        if let Answer::Running(task) = self.answer.get_mut() {
            task.abort();
        }
    }
}

#[cfg(any(feature = "claude", feature = "codex"))]
impl VersionAhead {
    fn start(question: Question, probe: impl Future<Output = Reply> + Send + 'static) -> Self {
        Self(std::sync::Arc::new(Ahead {
            question,
            answer: tokio::sync::Mutex::new(Answer::Running(tokio::spawn(probe))),
        }))
    }

    /// Starts `process::installed_version` of `binary`, found on `path_var`.
    pub(crate) fn installed(
        binary: Option<std::path::PathBuf>,
        path_var: std::ffi::OsString,
        program: &'static str,
    ) -> Self {
        let question = Question::Installed {
            binary: binary.clone(),
            path_var: path_var.clone(),
        };
        Self::start(question, async move {
            Ok(crate::process::installed_version(binary, &path_var, program).await)
        })
    }

    /// Starts `process::reported_version` of `binary` in `cwd` with `env`.
    pub(crate) fn reported(
        binary: std::path::PathBuf,
        cwd: std::path::PathBuf,
        env: Vec<(fabric_core::env::EnvVarName, String)>,
    ) -> Self {
        let question = Question::Reported {
            binary: binary.clone(),
            cwd: cwd.clone(),
            env: env.clone(),
        };
        Self::start(question, async move {
            crate::process::reported_version(&binary, &cwd, &env)
                .await
                .map(Some)
                .map_err(|e| e.to_string())
        })
    }

    /// The answer to `question`, waited for, when it is the one asked
    /// ahead; `None` otherwise.
    async fn answer_to(&self, question: &Question) -> Option<Reply> {
        if self.0.question != *question {
            return None;
        }
        let mut answer = self.0.answer.lock().await;
        if let Answer::Running(task) = &mut *answer {
            let reply = match task.await {
                Ok(reply) => reply,
                Err(e) => Err(format!("the version probe did not finish: {e}")),
            };
            *answer = Answer::Done(reply);
        }
        match &*answer {
            Answer::Done(reply) => Some(reply.clone()),
            Answer::Running(_) => None,
        }
    }
}

/// `process::installed_version(binary, path_var, program)`, answered by
/// `ahead` when it asked exactly that.
#[cfg(any(feature = "claude", feature = "codex"))]
pub(crate) async fn installed_version(
    ahead: Option<&VersionAhead>,
    binary: Option<std::path::PathBuf>,
    path_var: &std::ffi::OsStr,
    program: &str,
) -> Option<String> {
    let question = Question::Installed {
        binary: binary.clone(),
        path_var: path_var.to_owned(),
    };
    if let Some(ahead) = ahead
        && let Some(Ok(version)) = ahead.answer_to(&question).await
    {
        return version;
    }
    crate::process::installed_version(binary, path_var, program).await
}

/// `process::reported_version(binary, cwd, env)`, answered by `ahead`
/// when it asked exactly that.
#[cfg(any(feature = "claude", feature = "codex"))]
async fn reported_version(
    ahead: Option<&VersionAhead>,
    binary: &std::path::Path,
    cwd: &std::path::Path,
    env: &[(fabric_core::env::EnvVarName, String)],
) -> Result<String, String> {
    let question = Question::Reported {
        binary: binary.to_owned(),
        cwd: cwd.to_owned(),
        env: env.to_vec(),
    };
    if let Some(ahead) = ahead
        && let Some(reply) = ahead.answer_to(&question).await
    {
        return reply.map(Option::unwrap_or_default);
    }
    crate::process::reported_version(binary, cwd, env)
        .await
        .map_err(|e| e.to_string())
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

    /// A fake binary at `dir/tool` that logs each run, with its directory,
    /// to `dir/runs` and prints `version`.
    fn counted_tool(dir: &std::path::Path, version: &str) -> std::path::PathBuf {
        let tool = dir.join("tool");
        let runs = dir.join("runs");
        crate::fake_tool::write(
            &tool,
            format!(
                "#!/bin/sh\n{}pwd >> '{}'\necho '{version}'\n",
                crate::fake_tool::WARM_GUARD,
                runs.display()
            )
            .as_bytes(),
        );
        tool
    }

    fn runs(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("runs"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The resume guard takes the answer of a `--version` asked ahead
    /// when it asks the same binary in the same directory with the same
    /// environment, and runs the binary again for any other question, so
    /// it never decides on another directory's or environment's answer.
    #[tokio::test]
    async fn the_guard_takes_an_answer_asked_ahead_only_for_the_same_question() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let (work, other) = (dir.join("work"), dir.join("other"));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let tool = counted_tool(&dir, "tool 2.1.300");
        let session = fabric_core::session::SessionId::parse("s").unwrap();
        let recorded = HarnessVersion::parse("2.1.294").unwrap();
        let env: Vec<(fabric_core::env::EnvVarName, String)> = vec![(
            fabric_core::env::EnvVarName::parse("PATH").unwrap(),
            "/usr/bin:/bin".to_owned(),
        )];
        let ahead = VersionAhead::reported(tool.clone(), work.clone(), env.clone());
        let guard = |cwd: std::path::PathBuf, env: Vec<(fabric_core::env::EnvVarName, String)>| {
            let (tool, ahead, session, recorded) = (
                tool.clone(),
                ahead.clone(),
                session.clone(),
                recorded.clone(),
            );
            async move {
                refuse_older(
                    "tool",
                    (&tool, &cwd, &env),
                    Some(&ahead),
                    &session,
                    &recorded,
                )
                .await
            }
        };
        let version = guard(work.clone(), env.clone()).await.unwrap();
        assert_eq!(version.to_string(), "2.1.300");
        assert_eq!(runs(&dir).len(), 1, "the guard ran the binary again");
        guard(work.clone(), env.clone()).await.unwrap();
        assert_eq!(runs(&dir).len(), 1);

        guard(other.clone(), env.clone()).await.unwrap();
        let mut more = env.clone();
        more.push((
            fabric_core::env::EnvVarName::parse("MISE_ENV").unwrap(),
            "ci".to_owned(),
        ));
        guard(work.clone(), more).await.unwrap();
        let ran = runs(&dir);
        assert_eq!(ran.len(), 3, "{ran:?}");
        assert!(ran[1].ends_with("other"), "{ran:?}");
    }

    /// A restore takes the installed version asked ahead only when it
    /// asks the same binary on the same `PATH`.
    #[tokio::test]
    async fn a_restore_takes_an_installed_version_asked_ahead_only_for_the_same_binary() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let tool = counted_tool(&dir, "tool 0.160.0");
        let path_var = std::ffi::OsString::from("/usr/bin:/bin");
        let ahead = VersionAhead::installed(Some(tool.clone()), path_var.clone(), "tool");
        let asked = installed_version(Some(&ahead), Some(tool.clone()), &path_var, "tool").await;
        assert_eq!(asked.as_deref(), Some("0.160.0"));
        assert_eq!(runs(&dir).len(), 1);
        let other_path = std::ffi::OsString::from("/bin");
        installed_version(Some(&ahead), Some(tool.clone()), &other_path, "tool").await;
        installed_version(Some(&ahead), None, &path_var, "tool").await;
        assert_eq!(runs(&dir).len(), 2, "{:?}", runs(&dir));
    }
}
