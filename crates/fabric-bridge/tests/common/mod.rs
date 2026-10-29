//! What the bridge tests share: a journal holding the fixtures' job,
//! effects on it, and the command that starts the Capsule worker.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_bridge::wire::{Action, CreateParams, Digest, OperationRef, Scope};
use fabric_bridge::{Effect, LogError, LogThread, Recorder, Slot};
use fabric_core::harness::HarnessKind;
use fabric_core::id::NodeId;
use fabric_core::managed::{
    Command, Envelope, Generation, Intent, JobPolicy, NewIds, OperationId, OperationKey,
    ResourceRef, RestartPolicy, SourceBinding,
};
use fabric_core::path::AbsPath;
use fabric_journal::{FILE_NAME, Journal};

/// The fixtures' job, task and attempt.
pub const JOB: &str = "01K8W3A00000000000000000J1";
/// The fixtures' task.
pub const TASK: &str = "01K8W3A00000000000000000T1";
/// The fixtures' attempt.
pub const ATTEMPT: &str = "01K8W3A00000000000000000A1";

/// A journal holding the fixtures' job with its attempt granted at
/// generation 1, on its log thread.
pub fn journal(dir: &Path) -> Logged {
    Logged::new(journal_for(dir, scope()))
}

/// A journal on its log thread, as a node hands it to the bridge, with a
/// second connection the test reads through, which sees what the log
/// thread committed. It derefs to that reader.
pub struct Logged {
    log: LogThread<Journal>,
    read: Journal,
}

impl Logged {
    /// Moves `journal` to a log thread and opens a reader beside it.
    pub fn new(journal: Journal) -> Logged {
        let read = Journal::open(journal.path()).unwrap();
        Logged {
            log: LogThread::spawn(journal).unwrap(),
            read,
        }
    }

    /// The log thread.
    pub fn log(&self) -> &LogThread<Journal> {
        &self.log
    }

    /// Stops the log thread and hands its journal back.
    pub async fn into_inner(self) -> Journal {
        self.log.into_inner().await.unwrap()
    }
}

impl std::ops::Deref for Logged {
    type Target = Journal;

    fn deref(&self) -> &Journal {
        &self.read
    }
}

impl Recorder for Logged {
    type Log = Journal;

    fn reserve(&mut self) -> impl Future<Output = Result<Slot<'_, Journal>, LogError>> + Send {
        self.log.reserve()
    }
}

/// A journal holding `scope`'s job, task and attempt, the attempt granted
/// at generation 1.
pub fn journal_for(dir: &Path, scope: Scope) -> Journal {
    let node = NodeId::from_bytes([4; 32]);
    let mut journal = Journal::create(&dir.join(FILE_NAME), node, 1).unwrap();
    let Scope {
        job, task, attempt, ..
    } = scope;
    let ids = |n: u8| NewIds {
        job,
        task,
        attempt,
        operation: OperationId::from_bytes([0xf0 + n; 16]),
        at_ms: 1_760_000_000_000 + u64::from(n),
    };
    let create = Envelope {
        key: OperationKey::parse("scenario-create").unwrap(),
        job: None,
        expected: None,
        body: Command::CreateJob {
            intent: Intent {
                goal: "bridge scenario".into(),
                harness: HarnessKind::Codex,
            },
            source: SourceBinding::Repository {
                path: AbsPath::parse("/src/repo").unwrap(),
            },
            policy: JobPolicy {
                restart: RestartPolicy::ExplicitOnly,
            },
        },
    };
    journal.submit(&create, ids(1)).unwrap();
    let start = Envelope {
        key: OperationKey::parse("scenario-start").unwrap(),
        job: Some(job),
        expected: None,
        body: Command::StartAttempt {
            task,
            resource: ResourceRef::Node { node },
            initial_checkpoint: None,
        },
    };
    journal.submit(&start, ids(2)).unwrap();
    journal
}

/// The fixtures' scope.
pub fn scope() -> Scope {
    Scope {
        job: JOB.parse().unwrap(),
        task: TASK.parse().unwrap(),
        attempt: ATTEMPT.parse().unwrap(),
        generation: Generation::FIRST,
    }
}

/// Operation `n` of a test, with key `key-<n>`.
pub fn operation(n: u8) -> OperationRef {
    OperationRef {
        id: OperationId::from_bytes([n; 16]),
        key: OperationKey::parse(&format!("key-{n}")).unwrap(),
        digest: Digest([n; 32]),
    }
}

/// A `sandbox.create` of `image` as operation `n`.
pub fn create(n: u8, image: &str) -> Effect {
    Effect {
        operation: operation(n),
        scope: scope(),
        action: Action::Create(CreateParams {
            image: image.into(),
            ..CreateParams::default()
        }),
        approval: None,
        policy: None,
        deadline_ms: 30_000,
    }
}

/// The variable naming the Capsule worker the interop tests run.
pub const WORKER_VAR: &str = "CAPSULE_BRIDGE_WORKER";

/// The command that starts the Capsule worker (Capsule's
/// `packages/bridge-worker`) serving `profile` with the config at `config`.
///
/// `CAPSULE_BRIDGE_WORKER` names the worker: a path ending in `.js` runs
/// under `node` (`CAPSULE_BRIDGE_NODE`, or `node` on `PATH`), any other
/// path is run as it is, such as an installed `capsule-bridge-worker`. The
/// worker gets `PATH`, `HOME` and the `DOCKER_*` variables and nothing
/// else, so no credential of the test's environment reaches it.
///
/// # Panics
///
/// When the variable is not set: the tests that call this are ignored by
/// default, and running them without a worker is a mistake, not a pass.
pub fn capsule_worker(profile: &str, config: &Path) -> tokio::process::Command {
    let Some(path) = std::env::var_os(WORKER_VAR).map(PathBuf::from) else {
        panic!(
            "{WORKER_VAR} is not set; build the Capsule worker and point it at \
             packages/bridge-worker/dist/bin.js (docs/bridge-protocol.md)"
        );
    };
    let mut command = if path.extension().is_some_and(|e| e == "js" || e == "mjs") {
        let node =
            std::env::var_os("CAPSULE_BRIDGE_NODE").unwrap_or_else(|| OsString::from("node"));
        let mut command = tokio::process::Command::new(node);
        command.arg(&path);
        command
    } else {
        tokio::process::Command::new(&path)
    };
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let keep = name
            .to_str()
            .is_some_and(|n| n == "PATH" || n == "HOME" || n.starts_with("DOCKER_"));
        if keep {
            command.env(name, value);
        }
    }
    command
        .arg("--profile")
        .arg(profile)
        .arg("--config")
        .arg(config);
    command
}

/// Runs the docker CLI; its stdout when it exits 0.
pub fn docker(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Removes every container matching the filter it holds, such as
/// `label=fabric.job=<job>`, when dropped: pass or fail.
pub struct Cleanup(pub String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(ids) = docker(&["ps", "-a", "-q", "--filter", &self.0]) {
            for id in ids.split_whitespace() {
                let _ = docker(&["rm", "-f", id]);
            }
        }
    }
}

/// 16 bytes no earlier run used, for this run's job, task and attempt, so
/// labels from an earlier run can never match.
pub fn fresh(salt: &str) -> [u8; 16] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = blake3::hash(format!("{}:{nanos}:{salt}", std::process::id()).as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    bytes
}

/// The variable naming the image the Docker tests use.
pub const DOCKER_IMAGE_VAR: &str = "CAPSULE_BRIDGE_DOCKER_IMAGE";

/// The image `CAPSULE_BRIDGE_DOCKER_IMAGE` names when the local engine is
/// reachable and already holds it, or why the Docker run cannot happen.
/// Nothing here pulls an image.
pub fn docker_image() -> Result<String, String> {
    let Some(image) = std::env::var_os(DOCKER_IMAGE_VAR) else {
        return Err(format!(
            "{DOCKER_IMAGE_VAR} is not set; name an image the engine already holds"
        ));
    };
    let image = image
        .into_string()
        .map_err(|_| format!("{DOCKER_IMAGE_VAR} is not UTF-8"))?;
    if docker(&["version", "--format", "{{.Server.Version}}"]).is_none() {
        return Err("the Docker engine is not reachable (docker version failed)".into());
    }
    if docker(&["image", "inspect", &image]).is_none() {
        return Err(format!(
            "{image} is not on the engine, and nothing here pulls"
        ));
    }
    Ok(image)
}
