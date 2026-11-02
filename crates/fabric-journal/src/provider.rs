//! Provider operations: the effects a managed attempt asks a provider
//! worker for over the bridge (ADR 0021, docs/bridge-protocol.md), and
//! their reconciliation and cleanup (ADR 0022, docs/managed-jobs.md).
//!
//! Invariant: a provider operation is written `dispatched` in its own
//! committed transaction before its request frame is written, and only
//! after the attempt it acts for is checked to be the job's current,
//! live grant (an effect), or the create it cleans is checked to name a
//! resource no destroy removed or is removing (a cleanup): the journal
//! owns identity and fencing, the worker owns none. Its key and id are
//! never used twice, so nothing the journal holds is ever sent again, and
//! no create is dispatched while a create of its job is `unknown`. It
//! leaves `dispatched` only for what the worker answered or for
//! `unknown`; it leaves `unknown` only for what a read found (`attached`,
//! `observed` or a terminal state) or for the owner's abandonment. The
//! target Fabric sent the action to is stored with the dispatch and never
//! changes, so an operation lost before any answer still names the
//! resource it acted on. Locators are stored as Fabric's canonical JSON
//! text with a key of their identity and are never parsed here: the
//! journal compares keys, and reads only what the bridge wrote.

use fabric_core::id::JobId;
use fabric_core::managed::{
    AttemptId, AttemptStatus, Command, DomainError, Envelope, Generation, JobState, ManagedState,
    NewIds, OperationId, OperationKey, RequestDigest, TaskId, Transition, accept_command,
};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::project::{get_u64, load_state, sql_u64};
use crate::{CommitError, Journal};

/// Where a provider operation is (docs/managed-jobs.md, the
/// reconciliation state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderStatus {
    /// Recorded before the request was written; an effect may have happened.
    Dispatched,
    /// Nobody can tell; only a read or the owner moves it on.
    Unknown,
    /// A lost create's resource was found by its identity; its locator is
    /// known and its action was not seen terminal.
    Attached,
    /// After `unknown`, a read saw the effect in a state that is not terminal.
    Observed,
    /// A terminal success was observed.
    Succeeded,
    /// The provider gave a definite failure.
    Failed,
    /// A terminal cancellation was observed.
    Cancelled,
    /// Refused before any effect, by Fabric or by the worker.
    Rejected,
    /// The owner gave up on certainty; says nothing about the effect.
    Abandoned,
}

impl ProviderStatus {
    /// Every status, in the order of the state machine's table.
    pub const ALL: [ProviderStatus; 9] = [
        ProviderStatus::Dispatched,
        ProviderStatus::Unknown,
        ProviderStatus::Attached,
        ProviderStatus::Observed,
        ProviderStatus::Succeeded,
        ProviderStatus::Failed,
        ProviderStatus::Cancelled,
        ProviderStatus::Rejected,
        ProviderStatus::Abandoned,
    ];

    /// The name the journal stores, the one the bridge fixtures use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ProviderStatus::Dispatched => "dispatched",
            ProviderStatus::Unknown => "unknown",
            ProviderStatus::Attached => "attached",
            ProviderStatus::Observed => "observed",
            ProviderStatus::Succeeded => "succeeded",
            ProviderStatus::Failed => "failed",
            ProviderStatus::Cancelled => "cancelled",
            ProviderStatus::Rejected => "rejected",
            ProviderStatus::Abandoned => "abandoned",
        }
    }

    fn parse(name: &str) -> Option<ProviderStatus> {
        ProviderStatus::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Whether nothing moves the operation on any more: settled
    /// (`succeeded`, `failed`, `cancelled`, `rejected`) or `abandoned`.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            ProviderStatus::Dispatched
                | ProviderStatus::Unknown
                | ProviderStatus::Attached
                | ProviderStatus::Observed
        )
    }

    /// Whether an answer or a read may move an operation from `self` to
    /// another status `to` (docs/managed-jobs.md, the transition table).
    /// `abandoned` is never reached this way, only by
    /// [`Journal::provider_abandon`]; a status kept as it is is not a move.
    #[must_use]
    pub fn may_become(self, to: ProviderStatus) -> bool {
        use ProviderStatus::{
            Attached, Cancelled, Dispatched, Failed, Observed, Rejected, Succeeded, Unknown,
        };
        match self {
            Dispatched => matches!(to, Unknown | Succeeded | Failed | Cancelled | Rejected),
            Unknown => matches!(to, Attached | Observed | Succeeded | Failed | Cancelled),
            Attached => matches!(to, Observed | Succeeded | Failed | Cancelled),
            Observed => matches!(to, Succeeded | Failed | Cancelled),
            _ => false,
        }
    }

    /// Whether the owner may abandon an operation in this status.
    #[must_use]
    pub fn may_be_abandoned(self) -> bool {
        matches!(
            self,
            ProviderStatus::Unknown | ProviderStatus::Attached | ProviderStatus::Observed
        )
    }
}

/// What an effect does to its resource: the `effect` of its action in the
/// capability profile it was dispatched under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectClass {
    /// Makes a resource.
    Create,
    /// Changes or runs inside one, such as an exec.
    Mutate,
    /// Removes one.
    Destroy,
}

impl EffectClass {
    /// The name the journal stores, the profile's.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            EffectClass::Create => "create",
            EffectClass::Mutate => "mutate",
            EffectClass::Destroy => "destroy",
        }
    }

    fn parse(name: &str) -> Option<EffectClass> {
        [
            EffectClass::Create,
            EffectClass::Mutate,
            EffectClass::Destroy,
        ]
        .into_iter()
        .find(|e| e.name() == name)
    }
}

/// Why a provider operation was dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// An effect the attempt asked for.
    Effect,
    /// A destroy of the resource that the create `cleans` made (same job).
    Cleanup {
        /// The create.
        cleans: OperationId,
    },
}

/// blake3 of `"fabric.bridge.locator.v1\0"` followed by the canonical JSON
/// (keys sorted) of a locator's provider, scope, kind and native id; the
/// native operation id is left out. Computed by the bridge, compared by
/// the journal, never parsed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceKey([u8; 32]);

impl ResourceKey {
    /// The key with these bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> ResourceKey {
        ResourceKey(bytes)
    }

    /// Its bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// A locator as the journal stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorRecord {
    /// Fabric's canonical JSON of the locator (keys sorted), as it sends it.
    pub text: String,
    /// The key of its identity.
    pub key: ResourceKey,
}

/// What a provider operation acts for, as Fabric sends it in `request`:
/// the intent, committed before the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDispatch {
    /// A fresh operation id; never an id the journal holds.
    pub operation: OperationId,
    /// The idempotency key; never a key the journal holds.
    pub key: OperationKey,
    /// Fabric's digest of the request.
    pub digest: RequestDigest,
    /// The job.
    pub job: JobId,
    /// The task.
    pub task: TaskId,
    /// The attempt the effect belongs to (a cleanup: its create's).
    pub attempt: AttemptId,
    /// The attempt's generation, which must be the job's for an effect.
    pub generation: Generation,
    /// The profile negotiated with the worker the frame goes to.
    pub profile: String,
    /// The bridge action, such as `sandbox.create`.
    pub action: String,
    /// The action's effect in that profile, so the journal never reads one.
    pub effect: EffectClass,
    /// Why it is dispatched.
    pub purpose: Purpose,
    /// The locator of the resource the action acts on; `None` for an
    /// action without one, such as `sandbox.create`.
    pub target: Option<LocatorRecord>,
}

/// Why the worker or Fabric answered as it did, kept with the status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderAnswer {
    /// The bridge error code, such as `denied`; `None` for a result.
    pub code: Option<String>,
    /// The bridge error reason, such as `policy_denied`.
    pub reason: Option<String>,
    /// The effect the answer declared: `none`, `applied`, `pending` or `unknown`.
    pub effect: Option<String>,
}

/// A provider operation as the journal holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperation {
    /// What it acts for.
    pub dispatch: ProviderDispatch,
    /// Where it is.
    pub status: ProviderStatus,
    /// The last answer recorded.
    pub answer: ProviderAnswer,
    /// The last locator the worker reported.
    pub locator: Option<LocatorRecord>,
    /// The scope the last `absent` attach searched, as canonical JSON.
    pub searched: Option<String>,
    /// Why the owner abandoned it; set exactly when it is `abandoned`.
    pub abandon_reason: Option<String>,
}

impl ProviderOperation {
    /// The resource it names: the locator its worker reported, else its target.
    #[must_use]
    pub fn resource(&self) -> Option<&LocatorRecord> {
        self.locator.as_ref().or(self.dispatch.target.as_ref())
    }
}

/// Why a cleanup was refused before anything was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CleanupRefusal {
    /// The named operation is not a create the journal holds, or the
    /// cleanup is not a destroy of its resource in its own scope.
    #[error(
        "it is not a create the journal holds, or the cleanup is not a destroy of its resource"
    )]
    NotACreate,
    /// The create has no locator: attach it first, or it stays a residual.
    #[error("the create names no resource")]
    Unlocated,
    /// The create is still `dispatched`.
    #[error("the create is still dispatched")]
    StillDispatched,
    /// A destroy of its resource settled `succeeded`.
    #[error("its resource was destroyed")]
    AlreadyDestroyed,
    /// A destroy of its resource is `dispatched`, `unknown` or `observed`.
    #[error("destroy {operation} of its resource is not settled; observe it first")]
    InFlight {
        /// That destroy.
        operation: OperationId,
    },
}

/// Why an abandonment was refused. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AbandonRefusal {
    /// The status is not `unknown`, `attached` or `observed`.
    #[error("it is {}, which is never abandoned", status.name())]
    NotAbandonable {
        /// The status.
        status: ProviderStatus,
    },
    /// The status is not the one the approval expects.
    #[error("it is {} now, not the status the owner saw", status.name())]
    Stale {
        /// The status.
        status: ProviderStatus,
    },
    /// The acknowledgement does not match the row.
    #[error("the acknowledgement is not what the operation names")]
    Acknowledgement,
    /// The reason is empty, too long or holds a control character.
    #[error("the reason is not 1 to 512 characters without a control character")]
    Reason,
}

/// What the owner accepts may remain when abandoning an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acknowledged {
    /// The resource the row names (`locator_key`, else `target_key`).
    Resource(ResourceKey),
    /// The scope the row's last absent attach searched, as `searched` holds it.
    Searched(String),
    /// The row names no resource and no search.
    Nothing,
}

/// The owner's approval to stop reconciling one provider operation. It is
/// bound to the row as the owner saw it, so it cannot apply to a later state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbandonApproval {
    /// The operation.
    pub operation: OperationId,
    /// `Unknown`, `Attached` or `Observed`: the status the owner saw.
    pub expected: ProviderStatus,
    /// What the owner accepts may remain.
    pub acknowledged: Acknowledged,
    /// 1 to 512 characters, no control character.
    pub reason: String,
}

/// The longest abandonment reason, in characters.
pub const MAX_ABANDON_REASON: usize = 512;

/// A provider operation that keeps a replacement attempt from being granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    /// A create that is `dispatched` or `unknown`: its resource cannot be named.
    CreateUnresolved {
        /// The create.
        operation: OperationId,
        /// Its status.
        status: ProviderStatus,
    },
    /// An exec not settled (`abandoned` included) whose sandbox has no
    /// destroy settled `succeeded`.
    ExecMayRun {
        /// The exec.
        operation: OperationId,
        /// Its status.
        status: ProviderStatus,
    },
    /// A destroy `dispatched`, `unknown`, `observed`, `failed`,
    /// `cancelled` or `abandoned` whose resource has no destroy settled
    /// `succeeded`.
    StopNotConfirmed {
        /// The destroy.
        operation: OperationId,
        /// Its status.
        status: ProviderStatus,
    },
}

impl Blocker {
    /// The operation that blocks.
    #[must_use]
    pub fn operation(&self) -> OperationId {
        match *self {
            Blocker::CreateUnresolved { operation, .. }
            | Blocker::ExecMayRun { operation, .. }
            | Blocker::StopNotConfirmed { operation, .. } => operation,
        }
    }
}

/// What a create of the job may have left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Residual {
    /// The create.
    pub creating: OperationId,
    /// Its status.
    pub status: ProviderStatus,
    /// Its resource, when known.
    pub locator: Option<LocatorRecord>,
    /// The scope its last absent attach searched, when it has no locator.
    pub searched: Option<String>,
}

/// Why a provider operation could not be recorded. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The key is already the key of an operation: it is never sent again.
    #[error("operation key {key} is already in the journal")]
    KeyUsed {
        /// The key.
        key: OperationKey,
    },
    /// The id is already the id of an operation.
    #[error("operation {operation} is already in the journal")]
    OperationUsed {
        /// The id.
        operation: OperationId,
    },
    /// The journal has no provider operation with this id.
    #[error("no provider operation {operation}")]
    UnknownOperation {
        /// The id.
        operation: OperationId,
    },
    /// The operation cannot move from where it is to that status.
    #[error("provider operation {operation} cannot go from {from} to {to}")]
    Transition {
        /// The id.
        operation: OperationId,
        /// Its status.
        from: &'static str,
        /// The status asked for.
        to: &'static str,
    },
    /// A create of the job is `unknown`: no create is dispatched until it
    /// is attached, observed, settled or abandoned.
    #[error("create {operation} of the job is unknown; attach it before another create")]
    CreateUnresolved {
        /// That create.
        operation: OperationId,
    },
    /// A cleanup was refused.
    #[error("no cleanup of create {creating}: {refusal}")]
    Cleanup {
        /// The create it named.
        creating: OperationId,
        /// Why.
        refusal: CleanupRefusal,
    },
    /// An abandonment was refused.
    #[error("provider operation {operation} is not abandoned: {refusal}")]
    Abandon {
        /// The operation.
        operation: OperationId,
        /// Why.
        refusal: AbandonRefusal,
    },
}

/// Keys are one space: a command whose key a provider operation holds is
/// refused as `OperationConflict`, as one whose key another command holds.
pub(crate) fn refuse_provider_key(
    conn: &rusqlite::Connection,
    key: &OperationKey,
) -> Result<(), CommitError> {
    let used = conn
        .query_row(
            "SELECT 1 FROM provider_operations WHERE op_key = ?1",
            [key.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if used {
        return Err(DomainError::OperationConflict { key: key.clone() }.into());
    }
    Ok(())
}

/// The columns [`read_row`] reads, in its order.
const COLUMNS: &str = "operation_id, op_key, request_digest, job_id, task_id, attempt_id,
    generation, profile, action, effect_class, purpose, cleans, target, target_key, status,
    code, reason, effect, locator, locator_key, searched, abandon_reason";

/// A row as `SQLite` hands it over, before its values are checked.
struct RawRow {
    operation: [u8; 16],
    key: String,
    digest: [u8; 32],
    job: [u8; 16],
    task: [u8; 16],
    attempt: [u8; 16],
    generation: u64,
    profile: String,
    action: String,
    effect_class: String,
    purpose: String,
    cleans: Option<[u8; 16]>,
    target: Option<String>,
    target_key: Option<[u8; 32]>,
    status: String,
    code: Option<String>,
    reason: Option<String>,
    effect: Option<String>,
    locator: Option<String>,
    locator_key: Option<[u8; 32]>,
    searched: Option<String>,
    abandon_reason: Option<String>,
}

fn read_row(r: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        operation: r.get(0)?,
        key: r.get(1)?,
        digest: r.get(2)?,
        job: r.get(3)?,
        task: r.get(4)?,
        attempt: r.get(5)?,
        generation: get_u64(r, 6)?,
        profile: r.get(7)?,
        action: r.get(8)?,
        effect_class: r.get(9)?,
        purpose: r.get(10)?,
        cleans: r.get(11)?,
        target: r.get(12)?,
        target_key: r.get(13)?,
        status: r.get(14)?,
        code: r.get(15)?,
        reason: r.get(16)?,
        effect: r.get(17)?,
        locator: r.get(18)?,
        locator_key: r.get(19)?,
        searched: r.get(20)?,
        abandon_reason: r.get(21)?,
    })
}

fn record(text: Option<String>, key: Option<[u8; 32]>) -> Option<LocatorRecord> {
    Some(LocatorRecord {
        text: text?,
        key: ResourceKey(key?),
    })
}

impl RawRow {
    fn check(self) -> Result<ProviderOperation, CommitError> {
        let operation = OperationId::from_bytes(self.operation);
        let job = JobId::from_bytes(self.job);
        let corrupt = |reason: String| CommitError::Corrupt {
            job: Some(job),
            reason: format!("provider operation {operation}: {reason}"),
        };
        let purpose = match (self.purpose.as_str(), self.cleans) {
            ("effect", None) => Purpose::Effect,
            ("cleanup", Some(cleans)) => Purpose::Cleanup {
                cleans: OperationId::from_bytes(cleans),
            },
            (other, _) => return Err(corrupt(format!("purpose {other}"))),
        };
        Ok(ProviderOperation {
            dispatch: ProviderDispatch {
                operation,
                key: OperationKey::parse(&self.key).map_err(|e| corrupt(e.to_string()))?,
                digest: RequestDigest::from_bytes(self.digest),
                job,
                task: TaskId::from_bytes(self.task),
                attempt: AttemptId::from_bytes(self.attempt),
                generation: Generation::new(self.generation)
                    .ok_or_else(|| corrupt("generation 0".into()))?,
                profile: self.profile,
                action: self.action,
                effect: EffectClass::parse(&self.effect_class)
                    .ok_or_else(|| corrupt(format!("effect class {}", self.effect_class)))?,
                purpose,
                target: record(self.target, self.target_key),
            },
            status: ProviderStatus::parse(&self.status)
                .ok_or_else(|| corrupt(format!("status {}", self.status)))?,
            answer: ProviderAnswer {
                code: self.code,
                reason: self.reason,
                effect: self.effect,
            },
            locator: record(self.locator, self.locator_key),
            searched: self.searched,
            abandon_reason: self.abandon_reason,
        })
    }
}

fn load(
    conn: &Connection,
    operation: OperationId,
) -> Result<Option<ProviderOperation>, CommitError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM provider_operations WHERE operation_id = ?1"
    ))?;
    stmt.query_row([operation.to_bytes()], read_row)
        .optional()?
        .map(RawRow::check)
        .transpose()
}

/// Whether a destroy of the resource `key` names settled `succeeded`.
fn destroyed(conn: &Connection, key: &[u8; 32]) -> Result<bool, CommitError> {
    let mut stmt = conn.prepare_cached(
        "SELECT 1 FROM provider_operations
         WHERE target_key = ?1 AND effect_class = 'destroy' AND status = 'succeeded' LIMIT 1",
    )?;
    Ok(stmt.query_row([key], |_| Ok(())).optional()?.is_some())
}

/// Every blocker of `job`, oldest dispatch first (docs/managed-jobs.md,
/// the replacement barrier). It reads the job's rows whose status may
/// block through the status index, and for an exec or a destroy whether
/// its resource has a destroy settled `succeeded` through the target index.
pub(crate) fn blockers(conn: &Connection, job: JobId) -> Result<Vec<Blocker>, CommitError> {
    use ProviderStatus::{Abandoned, Attached, Cancelled, Dispatched, Failed, Observed, Unknown};
    let mut stmt = conn.prepare_cached(
        "SELECT operation_id, effect_class, status, target_key FROM provider_operations
         WHERE status IN ('dispatched','unknown','attached','observed','failed','cancelled',
                          'abandoned')
           AND job_id = ?1
         ORDER BY dispatched_ms, operation_id",
    )?;
    let rows = stmt
        .query_map([job.to_bytes()], |r| {
            Ok((
                r.get::<_, [u8; 16]>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<[u8; 32]>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (operation, effect, status, target) in rows {
        let operation = OperationId::from_bytes(operation);
        let corrupt = |what: String| CommitError::Corrupt {
            job: Some(job),
            reason: format!("provider operation {operation}: {what}"),
        };
        let status =
            ProviderStatus::parse(&status).ok_or_else(|| corrupt(format!("status {status}")))?;
        let effect =
            EffectClass::parse(&effect).ok_or_else(|| corrupt(format!("effect class {effect}")))?;
        let unconfirmed = |target: Option<[u8; 32]>| -> Result<bool, CommitError> {
            Ok(match target {
                Some(key) => !destroyed(conn, &key)?,
                None => true,
            })
        };
        let blocker = match effect {
            EffectClass::Create if matches!(status, Dispatched | Unknown) => {
                Some(Blocker::CreateUnresolved { operation, status })
            }
            EffectClass::Mutate
                if matches!(
                    status,
                    Dispatched | Unknown | Attached | Observed | Abandoned
                ) && unconfirmed(target)? =>
            {
                Some(Blocker::ExecMayRun { operation, status })
            }
            EffectClass::Destroy
                if matches!(
                    status,
                    Dispatched | Unknown | Observed | Failed | Cancelled | Abandoned
                ) && unconfirmed(target)? =>
            {
                Some(Blocker::StopNotConfirmed { operation, status })
            }
            _ => None,
        };
        out.extend(blocker);
    }
    Ok(out)
}

/// The domain's answer to `envelope`, behind the replacement barrier
/// (docs/managed-jobs.md): a grant the domain accepts waits while a
/// provider effect of the job may still act on its resources, refused as
/// `ReplacementBlocked` naming the oldest blocker, like any domain
/// refusal. Read in the command's transaction.
pub(crate) fn accept(
    conn: &Connection,
    state: Option<&ManagedState>,
    envelope: &Envelope<Command>,
    ids: &NewIds,
) -> Result<Result<Transition, DomainError>, CommitError> {
    let accepted = accept_command(state, envelope, ids);
    match (accepted, state, &envelope.body) {
        (Ok(transition), Some(state), Command::StartAttempt { .. } | Command::Recover { .. }) => {
            Ok(match blockers(conn, state.job.id)?.first() {
                Some(blocker) => Err(DomainError::ReplacementBlocked {
                    operation: blocker.operation(),
                }),
                None => Ok(transition),
            })
        }
        (accepted, _, _) => Ok(accepted),
    }
}

/// Whether `dispatch`, an effect, acts for the job's current live grant:
/// the job active, the attempt granted or running at the job's generation.
fn fence(conn: &Connection, dispatch: &ProviderDispatch) -> Result<(), CommitError> {
    let state =
        load_state(conn, dispatch.job)?.ok_or(DomainError::UnknownJob { job: dispatch.job })?;
    if state.job.state != JobState::Active {
        return Err(DomainError::JobClosed { job: dispatch.job }.into());
    }
    let attempt = state
        .attempt(dispatch.attempt)
        .ok_or(DomainError::UnknownAttempt {
            attempt: dispatch.attempt,
        })?;
    if attempt.task != dispatch.task {
        return Err(DomainError::UnknownTask {
            task: dispatch.task,
        }
        .into());
    }
    if dispatch.generation != attempt.generation || attempt.generation != state.job.generation {
        return Err(DomainError::StaleGeneration {
            given: dispatch.generation,
            current: state.job.generation,
        }
        .into());
    }
    if !matches!(
        attempt.status,
        AttemptStatus::Granted | AttemptStatus::Running
    ) {
        return Err(DomainError::AttemptEnded {
            attempt: dispatch.attempt,
        }
        .into());
    }
    Ok(())
}

/// Whether `dispatch`, a cleanup of `cleans`, may be dispatched
/// (docs/managed-jobs.md, the fresh cleanup approval flow, step 3).
fn cleanable(
    conn: &Connection,
    dispatch: &ProviderDispatch,
    cleans: OperationId,
) -> Result<(), CommitError> {
    let refuse = |refusal: CleanupRefusal| -> CommitError {
        ProviderError::Cleanup {
            creating: cleans,
            refusal,
        }
        .into()
    };
    let Some(create) = load(conn, cleans)? else {
        return Err(refuse(CleanupRefusal::NotACreate));
    };
    let c = &create.dispatch;
    let own_scope = c.job == dispatch.job
        && c.task == dispatch.task
        && c.attempt == dispatch.attempt
        && c.generation == dispatch.generation;
    if c.effect != EffectClass::Create || dispatch.effect != EffectClass::Destroy || !own_scope {
        return Err(refuse(CleanupRefusal::NotACreate));
    }
    let Some(locator) = &create.locator else {
        return Err(refuse(CleanupRefusal::Unlocated));
    };
    if create.status == ProviderStatus::Dispatched {
        return Err(refuse(CleanupRefusal::StillDispatched));
    }
    if dispatch.target.as_ref() != Some(locator) {
        return Err(refuse(CleanupRefusal::NotACreate));
    }
    let key = locator.key.to_bytes();
    if destroyed(conn, &key)? {
        return Err(refuse(CleanupRefusal::AlreadyDestroyed));
    }
    let in_flight = conn
        .prepare_cached(
            "SELECT operation_id FROM provider_operations
             WHERE target_key = ?1 AND effect_class = 'destroy'
               AND status IN ('dispatched','unknown','observed')
             ORDER BY dispatched_ms, operation_id LIMIT 1",
        )?
        .query_row([key], |r| r.get::<_, [u8; 16]>(0))
        .optional()?;
    if let Some(operation) = in_flight {
        return Err(refuse(CleanupRefusal::InFlight {
            operation: OperationId::from_bytes(operation),
        }));
    }
    Ok(())
}

/// Whether `reason` is 1 to 512 characters with no control character.
fn reason_fits(reason: &str) -> bool {
    let n = reason.chars().count();
    (1..=MAX_ABANDON_REASON).contains(&n) && !reason.chars().any(char::is_control)
}

impl Journal {
    /// Records a provider operation as `dispatched`, or as `rejected` with
    /// `refusal` when Fabric refused it before sending: the intent, one
    /// committed write. Returns after `COMMIT`, so a request written after
    /// this returns can never be written again by a restart: recovery
    /// makes it `unknown`.
    ///
    /// Refused, writing nothing, in this order: when the key or the id is
    /// already the journal's (provider or command); for an effect, when
    /// the job is not active or the attempt is not the job's current
    /// grant, granted or running at the job's generation
    /// (`StaleGeneration`, `AttemptEnded`); for a cleanup, by the rules of
    /// [`CleanupRefusal`], whatever the attempt and the job; for a create,
    /// while a create of the same job is `unknown` (`CreateUnresolved`).
    pub fn provider_begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key_used = tx
            .query_row(
                "SELECT 1 FROM operations WHERE op_key = ?1
                 UNION ALL SELECT 1 FROM provider_operations WHERE op_key = ?1",
                [dispatch.key.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if key_used {
            return Err(ProviderError::KeyUsed {
                key: dispatch.key.clone(),
            }
            .into());
        }
        let id = dispatch.operation.to_bytes();
        let id_used = tx
            .query_row(
                "SELECT 1 FROM operations WHERE operation_id = ?1
                 UNION ALL SELECT 1 FROM provider_operations WHERE operation_id = ?1",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if id_used {
            return Err(ProviderError::OperationUsed {
                operation: dispatch.operation,
            }
            .into());
        }
        let (purpose, cleans) = match dispatch.purpose {
            Purpose::Effect => {
                fence(&tx, dispatch)?;
                ("effect", None)
            }
            Purpose::Cleanup { cleans } => {
                cleanable(&tx, dispatch, cleans)?;
                ("cleanup", Some(cleans.to_bytes()))
            }
        };
        if dispatch.effect == EffectClass::Create {
            let unresolved = tx
                .prepare_cached(
                    "SELECT operation_id FROM provider_operations
                     WHERE status = 'unknown' AND job_id = ?1 AND effect_class = 'create'
                     ORDER BY dispatched_ms, operation_id LIMIT 1",
                )?
                .query_row([dispatch.job.to_bytes()], |r| r.get::<_, [u8; 16]>(0))
                .optional()?;
            if let Some(operation) = unresolved {
                return Err(ProviderError::CreateUnresolved {
                    operation: OperationId::from_bytes(operation),
                }
                .into());
            }
        }
        let status = if refusal.is_some() {
            ProviderStatus::Rejected
        } else {
            ProviderStatus::Dispatched
        };
        let answer = refusal.cloned().unwrap_or_default();
        tx.prepare_cached(
            "INSERT INTO provider_operations
               (operation_id, op_key, request_digest, job_id, task_id, attempt_id, generation,
                profile, action, effect_class, purpose, cleans, target, target_key, status,
                code, reason, effect, dispatched_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18, ?19, ?19)",
        )?
        .execute(params![
            id,
            dispatch.key.as_str(),
            dispatch.digest.to_bytes(),
            dispatch.job.to_bytes(),
            dispatch.task.to_bytes(),
            dispatch.attempt.to_bytes(),
            sql_u64(dispatch.generation.get())?,
            dispatch.profile,
            dispatch.action,
            dispatch.effect.name(),
            purpose,
            cleans,
            dispatch.target.as_ref().map(|t| t.text.as_str()),
            dispatch.target.as_ref().map(|t| t.key.to_bytes()),
            status.name(),
            answer.code,
            answer.reason,
            answer.effect,
            sql_u64(at_ms)?,
        ])?;
        tx.commit()?;
        Ok(())
    }

    /// Records what the worker answered for a provider operation, or what
    /// a read of it found: its new status, the answer and, when given,
    /// the locator it reported. The status may stay as it is, as for
    /// `pending`, to store the answer and the locator alone. A move must
    /// be one [`ProviderStatus::may_become`] allows, to `attached` or
    /// `observed` only for an operation that names a resource; a terminal
    /// operation never changes.
    pub fn provider_record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&LocatorRecord>,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = operation.to_bytes();
        let row: Option<(String, bool)> = tx
            .prepare_cached(
                "SELECT status, locator IS NOT NULL OR target IS NOT NULL
                 FROM provider_operations WHERE operation_id = ?1",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (from, named) = row.ok_or(ProviderError::UnknownOperation { operation })?;
        let from = ProviderStatus::parse(&from).ok_or_else(|| CommitError::Corrupt {
            job: None,
            reason: format!("provider operation {operation} has status {from}"),
        })?;
        let unnamed = matches!(status, ProviderStatus::Attached | ProviderStatus::Observed)
            && !named
            && locator.is_none();
        if from.is_terminal() || (from != status && !from.may_become(status)) || unnamed {
            return Err(ProviderError::Transition {
                operation,
                from: from.name(),
                to: status.name(),
            }
            .into());
        }
        tx.prepare_cached(
            "UPDATE provider_operations
             SET status = ?2, code = ?3, reason = ?4, effect = ?5,
                 locator = coalesce(?6, locator), locator_key = coalesce(?7, locator_key),
                 updated_ms = ?8
             WHERE operation_id = ?1",
        )?
        .execute(params![
            id,
            status.name(),
            answer.code,
            answer.reason,
            answer.effect,
            locator.map(|l| l.text.as_str()),
            locator.map(|l| l.key.to_bytes()),
            sql_u64(at_ms)?,
        ])?;
        tx.commit()?;
        Ok(())
    }

    /// Records an `absent` answer to an attach of an operation that is
    /// `unknown`: the answer and the scope it searched (`searched`, its
    /// canonical JSON). The status stays `unknown`: absence settles nothing.
    pub fn provider_searched(
        &mut self,
        operation: OperationId,
        answer: &ProviderAnswer,
        searched: &str,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let changed = self.conn.execute(
            "UPDATE provider_operations
             SET code = ?2, reason = ?3, effect = ?4, searched = ?5, updated_ms = ?6
             WHERE operation_id = ?1 AND status = 'unknown'",
            params![
                operation.to_bytes(),
                answer.code,
                answer.reason,
                answer.effect,
                searched,
                sql_u64(at_ms)?,
            ],
        )?;
        if changed == 0 {
            let row = self
                .provider_operation(operation)?
                .ok_or(ProviderError::UnknownOperation { operation })?;
            return Err(ProviderError::Transition {
                operation,
                from: row.status.name(),
                to: ProviderStatus::Unknown.name(),
            }
            .into());
        }
        Ok(())
    }

    /// Stores the locator the worker reported for a provider operation
    /// that is not terminal, as `progress` reports it before the answer
    /// (ADR 0020, record the observed locator early). The status and the
    /// answer stay as they are; a terminal operation keeps the locator its
    /// answer gave.
    pub fn provider_locate(
        &mut self,
        operation: OperationId,
        locator: &LocatorRecord,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let changed = self.conn.execute(
            "UPDATE provider_operations SET locator = ?2, locator_key = ?3, updated_ms = ?4
             WHERE operation_id = ?1
               AND status IN ('dispatched', 'unknown', 'attached', 'observed')",
            params![
                operation.to_bytes(),
                locator.text,
                locator.key.to_bytes(),
                sql_u64(at_ms)?
            ],
        )?;
        if changed == 0 && self.provider_operation(operation)?.is_none() {
            return Err(ProviderError::UnknownOperation { operation }.into());
        }
        Ok(())
    }

    /// The owner gives up on certainty for one operation: `unknown`,
    /// `attached` or `observed` becomes `abandoned`, with the reason. The
    /// approval must name the status the row holds and acknowledge what
    /// the row names: its resource, else the scope its last attach
    /// searched, else nothing. It says nothing about the effect: an
    /// abandoned destroy or exec still blocks a replacement.
    pub fn provider_abandon(
        &mut self,
        approval: &AbandonApproval,
        at_ms: u64,
    ) -> Result<(), CommitError> {
        let operation = approval.operation;
        let refuse = |refusal: AbandonRefusal| -> CommitError {
            ProviderError::Abandon { operation, refusal }.into()
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = load(&tx, operation)?.ok_or(ProviderError::UnknownOperation { operation })?;
        if !row.status.may_be_abandoned() {
            return Err(refuse(AbandonRefusal::NotAbandonable {
                status: row.status,
            }));
        }
        if row.status != approval.expected {
            return Err(refuse(AbandonRefusal::Stale { status: row.status }));
        }
        let names = match (row.resource(), &row.searched) {
            (Some(resource), _) => Acknowledged::Resource(resource.key),
            (None, Some(searched)) => Acknowledged::Searched(searched.clone()),
            (None, None) => Acknowledged::Nothing,
        };
        if names != approval.acknowledged {
            return Err(refuse(AbandonRefusal::Acknowledgement));
        }
        if !reason_fits(&approval.reason) {
            return Err(refuse(AbandonRefusal::Reason));
        }
        tx.execute(
            "UPDATE provider_operations
             SET status = 'abandoned', abandon_reason = ?2, updated_ms = ?3
             WHERE operation_id = ?1",
            params![operation.to_bytes(), approval.reason, sql_u64(at_ms)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The provider operation with this id, or `None`.
    pub fn provider_operation(
        &self,
        operation: OperationId,
    ) -> Result<Option<ProviderOperation>, CommitError> {
        load(&self.conn, operation)
    }

    /// Every blocker of the job's replacement attempt, oldest dispatch
    /// first (docs/managed-jobs.md, the replacement barrier).
    pub fn replacement_blockers(&self, job: JobId) -> Result<Vec<Blocker>, CommitError> {
        blockers(&self.conn, job)
    }

    /// What the job's creates may have left, oldest dispatch first: every
    /// create `succeeded`, `attached`, `observed`, `unknown` or
    /// `abandoned`, and every create `failed` with a locator, whose
    /// resource has no destroy settled `succeeded`. A create with no
    /// locator is listed without one, with the scope its attach searched.
    pub fn residuals(&self, job: JobId) -> Result<Vec<Residual>, CommitError> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM provider_operations
             WHERE job_id = ?1 AND effect_class = 'create'
               AND (status IN ('succeeded','attached','observed','unknown','abandoned')
                    OR (status = 'failed' AND locator IS NOT NULL))
             ORDER BY dispatched_ms, operation_id"
        ))?;
        let rows = stmt
            .query_map([job.to_bytes()], read_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.check()?;
            if let Some(locator) = &row.locator
                && destroyed(&self.conn, &locator.key.to_bytes())?
            {
                continue;
            }
            out.push(Residual {
                creating: row.dispatch.operation,
                status: row.status,
                searched: if row.locator.is_none() {
                    row.searched
                } else {
                    None
                },
                locator: row.locator,
            });
        }
        Ok(out)
    }

    /// Every destroy of the resource `key` names, oldest dispatch first,
    /// with its status.
    pub fn provider_destroys(
        &self,
        resource: ResourceKey,
    ) -> Result<Vec<(OperationId, ProviderStatus)>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT operation_id, status FROM provider_operations
             WHERE target_key = ?1 AND effect_class = 'destroy'
             ORDER BY dispatched_ms, operation_id",
        )?;
        let rows = stmt
            .query_map([resource.to_bytes()], |r| {
                Ok((r.get::<_, [u8; 16]>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(operation, status)| {
                let operation = OperationId::from_bytes(operation);
                let status =
                    ProviderStatus::parse(&status).ok_or_else(|| CommitError::Corrupt {
                        job: None,
                        reason: format!("provider operation {operation} has status {status}"),
                    })?;
                Ok((operation, status))
            })
            .collect()
    }

    /// Every provider operation still `dispatched`, oldest first.
    pub(crate) fn provider_dispatched(&self) -> Result<Vec<(JobId, OperationId)>, CommitError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT job_id, operation_id FROM provider_operations
             WHERE status = 'dispatched' ORDER BY dispatched_ms, operation_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, [u8; 16]>(0)?, r.get::<_, [u8; 16]>(1)?))
        })?;
        Ok(rows
            .map(|r| r.map(|(job, op)| (JobId::from_bytes(job), OperationId::from_bytes(op))))
            .collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests;
