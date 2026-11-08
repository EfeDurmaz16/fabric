//! The manifest of section 5 as typed parts, and the document that holds them.

use serde_json::{Map, Value, json};

use crate::canon;
use crate::check;
use crate::refusal::{Code, Refusal};
use crate::sha256::sha256;

/// A fidelity level of the ladder (section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fidelity {
    /// The writer could not capture; nothing continues.
    Unsupported,
    /// The revision restores; no session starts.
    WorkspaceOnly,
    /// A new session starts from an attachment and the package.
    NewNativeSessionWithPackage,
    /// The captured session continues.
    NativeSession,
}

impl Fidelity {
    /// The level as the document spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Fidelity::Unsupported => "unsupported",
            Fidelity::WorkspaceOnly => "workspace_only",
            Fidelity::NewNativeSessionWithPackage => "new_native_session_with_package",
            Fidelity::NativeSession => "native_session",
        }
    }

    fn parse(text: &str) -> Fidelity {
        match text {
            "native_session" => Fidelity::NativeSession,
            "new_native_session_with_package" => Fidelity::NewNativeSessionWithPackage,
            "workspace_only" => Fidelity::WorkspaceOnly,
            _ => Fidelity::Unsupported,
        }
    }
}

impl std::fmt::Display for Fidelity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `job` (section 5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The job's ULID.
    pub id: String,
    /// The goal in the user's words.
    pub goal: String,
    /// The node that created the job.
    pub created_by: String,
}

/// `produced` (section 5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Produced {
    /// Unix milliseconds when the document was written.
    pub at_ms: u64,
    /// The node that wrote it.
    pub by: String,
    /// The writing program and version.
    pub writer: String,
}

/// `workspace` (section 5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The directory the revision was captured in.
    pub root: String,
    /// The snapshot at the head of the bundle chain, blake3 hex.
    pub bundle_head: String,
    /// The hash of the job's latest ledger record, blake3 hex.
    pub ledger_head: String,
    /// The lease epoch, at least 1.
    pub epoch: u64,
    /// The capturing node's operating system name.
    pub os: String,
    /// Its architecture.
    pub arch: String,
}

/// `source` (section 5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A repository and an exact commit.
    Git {
        /// The repository.
        uri: String,
        /// The commit, 40 or 64 hex.
        commit: String,
    },
    /// A published snapshot, blake3 hex.
    Snapshot(String),
    /// An empty directory.
    Empty,
}

/// `harness` (section 5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harness {
    /// The kind, such as `claude_code`.
    pub kind: String,
    /// For `acp`, the program.
    pub program: Option<String>,
    /// For `acp`, its arguments.
    pub args: Vec<String>,
    /// The version that last wrote the session.
    pub version: Option<String>,
}

/// `attachments[].bound` (section 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// The workspace root the bytes were captured under.
    pub root: String,
    /// The home directory they were captured under.
    pub home: String,
}

/// One entry of `attachments` (section 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Its name in the document.
    pub name: String,
    /// The harness that can read it.
    pub harness: String,
    /// What the bytes are, such as `fabric.harness_state.v1`.
    pub format: String,
    /// What it gives when usable.
    pub fidelity: Fidelity,
    /// The harness's own session id.
    pub session: Option<String>,
    /// The root and home the bytes are bound to.
    pub bound: Option<Bound>,
    /// The blob's sha256, hex.
    pub digest: String,
    /// The blob's length.
    pub bytes: u64,
}

/// One entry of `caches` (section 5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    /// The cache root under the workspace root.
    pub path: String,
    /// `deps`, `build` or `tool_cache`.
    pub class: String,
    /// `carried`, `partial` or `not_carried`.
    pub carry: String,
    /// The carry key's exact strings, `kind` among them; `None` for `null`.
    pub key: Option<Vec<(String, String)>>,
    /// The blake3 of the cache's entries; `None` when not carried.
    pub digest: Option<String>,
}

/// One entry of `evidence` (section 5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// The attempt's ULID.
    pub attempt: String,
    /// The evidence's blake3, hex.
    pub digest: String,
    /// `checkpoint`, `exit` or `receipt`.
    pub kind: String,
}

/// Everything a v1.0 manifest says. A writer fills it in and
/// [`Manifest::new`] checks it by the same rules as a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    /// The job.
    pub job: Job,
    /// Who wrote the document.
    pub produced: Produced,
    /// The revision.
    pub workspace: Workspace,
    /// Where the workspace came from.
    pub source: Source,
    /// The harness.
    pub harness: Harness,
    /// The offered level; [`Parts::derived_fidelity`] gives the one the rules allow.
    pub fidelity: Fidelity,
    /// Why nothing is offered, for [`Fidelity::Unsupported`] only.
    pub reason: Option<String>,
    /// The native session attachments, sorted by name.
    pub attachments: Vec<Attachment>,
    /// The cache rows, sorted by path.
    pub caches: Vec<Cache>,
    /// Accepted evidence by digest.
    pub evidence: Vec<Evidence>,
    /// The `WorkPackage` v1 section, as its canonical object.
    pub package: Option<Value>,
    /// Workspace paths the writer kept back.
    pub left_behind: Vec<String>,
    /// Workspace paths allowed to travel although a pattern matched.
    pub allowlist: Vec<String>,
}

impl Parts {
    /// The level section 5.6 derives from the attachments.
    pub fn derived_fidelity(&self) -> Fidelity {
        self.attachments
            .iter()
            .map(|a| a.fidelity)
            .max()
            .unwrap_or(Fidelity::WorkspaceOnly)
    }

    fn to_value(&self) -> Value {
        let opt = |value: &Option<String>| value.as_ref().map_or(Value::Null, |s| json!(s));
        let source = match &self.source {
            Source::Git { uri, commit } => json!({"kind": "git", "uri": uri, "commit": commit}),
            Source::Snapshot(id) => json!({"kind": "snapshot", "snapshot": id}),
            Source::Empty => json!({"kind": "empty"}),
        };
        let attachments: Vec<Value> = self
            .attachments
            .iter()
            .map(|a| {
                json!({
                    "algorithm": "sha256",
                    "bound": a.bound.as_ref().map_or(Value::Null, |b| json!({"home": b.home, "root": b.root})),
                    "bytes": a.bytes.to_string(),
                    "digest": a.digest,
                    "fidelity": a.fidelity.as_str(),
                    "format": a.format,
                    "harness": a.harness,
                    "name": a.name,
                    "session": opt(&a.session),
                })
            })
            .collect();
        let caches: Vec<Value> = self
            .caches
            .iter()
            .map(|c| {
                let key = c.key.as_ref().map_or(Value::Null, |pairs| {
                    Value::Object(
                        pairs
                            .iter()
                            .map(|(k, v)| (k.clone(), json!(v)))
                            .collect::<Map<String, Value>>(),
                    )
                });
                json!({"carry": c.carry, "class": c.class, "digest": opt(&c.digest), "key": key, "path": c.path})
            })
            .collect();
        let evidence: Vec<Value> = self
            .evidence
            .iter()
            .map(|e| json!({"attempt": e.attempt, "digest": e.digest, "kind": e.kind}))
            .collect();
        json!({
            "attachments": attachments,
            "caches": caches,
            "evidence": evidence,
            "fidelity": {"level": self.fidelity.as_str(), "reason": opt(&self.reason)},
            "harness": {
                "args": self.harness.args,
                "kind": self.harness.kind,
                "program": opt(&self.harness.program),
                "version": opt(&self.harness.version),
            },
            "job": {"created_by": self.job.created_by, "goal": self.job.goal, "id": self.job.id},
            "package": self.package.clone().unwrap_or(Value::Null),
            "produced": {
                "at_ms": self.produced.at_ms.to_string(),
                "by": self.produced.by,
                "writer": self.produced.writer,
            },
            "psf": "1",
            "requires": [],
            "secrets": {"allowlist": self.allowlist, "left_behind": self.left_behind, "policy": "never_inside"},
            "source": source,
            "workspace": {
                "bundle_head": self.workspace.bundle_head,
                "epoch": self.workspace.epoch.to_string(),
                "ledger_head": self.workspace.ledger_head,
                "platform": {"arch": self.workspace.arch, "os": self.workspace.os},
                "root": self.workspace.root,
            },
        })
    }

    /// Reads the parts of a value [`check::check`] accepted.
    fn from_value(value: &Value) -> Parts {
        let uint = |v: &Value, keys: &[&str]| {
            check::forms::uint(at(v, keys).as_str().unwrap_or_default()).unwrap_or_default()
        };
        let list = |keys: &[&str]| at(value, keys).as_array().cloned().unwrap_or_default();
        let strings = |keys: &[&str]| -> Vec<String> {
            list(keys)
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let source = match text(value, &["source", "kind"]).as_str() {
            "git" => Source::Git {
                uri: text(value, &["source", "uri"]),
                commit: text(value, &["source", "commit"]),
            },
            "snapshot" => Source::Snapshot(text(value, &["source", "snapshot"])),
            _ => Source::Empty,
        };
        Parts {
            job: Job {
                id: text(value, &["job", "id"]),
                goal: text(value, &["job", "goal"]),
                created_by: text(value, &["job", "created_by"]),
            },
            produced: Produced {
                at_ms: uint(value, &["produced", "at_ms"]),
                by: text(value, &["produced", "by"]),
                writer: text(value, &["produced", "writer"]),
            },
            workspace: Workspace {
                root: text(value, &["workspace", "root"]),
                bundle_head: text(value, &["workspace", "bundle_head"]),
                ledger_head: text(value, &["workspace", "ledger_head"]),
                epoch: uint(value, &["workspace", "epoch"]),
                os: text(value, &["workspace", "platform", "os"]),
                arch: text(value, &["workspace", "platform", "arch"]),
            },
            source,
            harness: Harness {
                kind: text(value, &["harness", "kind"]),
                program: opt(value, &["harness", "program"]),
                args: strings(&["harness", "args"]),
                version: opt(value, &["harness", "version"]),
            },
            fidelity: Fidelity::parse(
                at(value, &["fidelity", "level"])
                    .as_str()
                    .unwrap_or_default(),
            ),
            reason: opt(value, &["fidelity", "reason"]),
            attachments: list(&["attachments"]).iter().map(attachment).collect(),
            caches: list(&["caches"]).iter().map(cache).collect(),
            evidence: list(&["evidence"])
                .iter()
                .map(|e| Evidence {
                    attempt: text(e, &["attempt"]),
                    digest: text(e, &["digest"]),
                    kind: text(e, &["kind"]),
                })
                .collect(),
            package: Some(at(value, &["package"]).clone()).filter(|p| !p.is_null()),
            left_behind: strings(&["secrets", "left_behind"]),
            allowlist: strings(&["secrets", "allowlist"]),
        }
    }
}

fn text(v: &Value, keys: &[&str]) -> String {
    at(v, keys).as_str().unwrap_or_default().to_owned()
}

fn opt(v: &Value, keys: &[&str]) -> Option<String> {
    at(v, keys).as_str().map(str::to_owned)
}

fn attachment(a: &Value) -> Attachment {
    Attachment {
        name: text(a, &["name"]),
        harness: text(a, &["harness"]),
        format: text(a, &["format"]),
        fidelity: Fidelity::parse(at(a, &["fidelity"]).as_str().unwrap_or_default()),
        session: opt(a, &["session"]),
        bound: at(a, &["bound"]).as_object().map(|_| Bound {
            root: text(a, &["bound", "root"]),
            home: text(a, &["bound", "home"]),
        }),
        digest: text(a, &["digest"]),
        bytes: check::forms::uint(at(a, &["bytes"]).as_str().unwrap_or_default())
            .unwrap_or_default(),
    }
}

fn cache(c: &Value) -> Cache {
    Cache {
        path: text(c, &["path"]),
        class: text(c, &["class"]),
        carry: text(c, &["carry"]),
        key: at(c, &["key"]).as_object().map(|map| {
            canon::sorted_keys(map)
                .into_iter()
                .map(|k| {
                    (
                        k.clone(),
                        map.get(k)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                })
                .collect()
        }),
        digest: opt(c, &["digest"]),
    }
}

fn at<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .try_fold(value, |v, key| v.get(*key))
        .unwrap_or(&Value::Null)
}

/// A manifest that passed every rule, with its canonical bytes.
///
/// It is made by [`read_manifest`] or [`Manifest::new`], and both run the
/// same checks, so a value of this type is always a valid v1 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    bytes: Vec<u8>,
    parts: Parts,
}

impl Manifest {
    /// Writes `parts` canonically and reads them back under every rule.
    pub fn new(parts: &Parts) -> Result<Manifest, Refusal> {
        let text =
            canon::write(&parts.to_value()).map_err(|_| Refusal::new(Code::NotCanonical, ""))?;
        read_manifest(text.as_bytes())
    }

    /// What the document says.
    pub fn parts(&self) -> &Parts {
        &self.parts
    }

    /// The document digest: sha256 of the canonical bytes.
    pub fn digest(&self) -> [u8; 32] {
        sha256(&self.bytes)
    }

    /// The canonical bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Reads one manifest: the bytes, checked by section 10 in order.
pub fn read_manifest(bytes: &[u8]) -> Result<Manifest, Refusal> {
    let value = check::check(bytes)?;
    Ok(Manifest {
        bytes: bytes.to_vec(),
        parts: Parts::from_value(&value),
    })
}

/// The canonical bytes of `manifest`.
pub fn write_manifest(manifest: &Manifest) -> Vec<u8> {
    manifest.bytes.clone()
}
