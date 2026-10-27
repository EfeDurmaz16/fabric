//! Where the bridge client records each provider operation: the managed
//! journal in a node, or an in-memory log in a test or a bench.
//!
//! Invariant: the client calls [`OperationLog::begin`] and gets `Ok`
//! before it writes a request frame, so an effect the worker may perform
//! is always in the log first; a key or id the log already holds is
//! [`LogError::Used`] and nothing is sent, so nothing is ever sent twice.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::managed::{OperationId, OperationKey};
use fabric_journal::{
    CommitError, Journal, ProviderAnswer, ProviderDispatch, ProviderError, ProviderStatus,
};

/// Why the log refused a change. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// The key or the id is already in the log: never send it again.
    #[error("already in the journal: {0}")]
    Used(String),
    /// The attempt is not the job's current live grant, or the job is closed.
    #[error("fenced: {0}")]
    Fenced(String),
    /// The log has no such operation, or it cannot move to that status.
    #[error("{0}")]
    Transition(String),
    /// The log failed.
    #[error("journal: {0}")]
    Storage(String),
}

/// A provider operation's status and last locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where it is.
    pub status: ProviderStatus,
    /// The last locator the worker reported, as JSON text.
    pub locator: Option<String>,
}

/// What the bridge client writes to.
pub trait OperationLog {
    /// Records the operation as `dispatched`, or as `rejected` with
    /// `refusal` when Fabric refused it before sending.
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError>;

    /// Records an answer: the new status, why, and the locator it named.
    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError>;

    /// Records a locator reported before the answer; the status stays.
    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError>;

    /// The operation's status and locator, or `None`.
    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError>;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl From<CommitError> for LogError {
    fn from(e: CommitError) -> Self {
        match e {
            CommitError::Provider(
                ProviderError::KeyUsed { .. } | ProviderError::OperationUsed { .. },
            ) => LogError::Used(e.to_string()),
            CommitError::Provider(_) => LogError::Transition(e.to_string()),
            CommitError::Domain(_) => LogError::Fenced(e.to_string()),
            other => LogError::Storage(other.to_string()),
        }
    }
}

impl OperationLog for Journal {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        Ok(self.provider_begin(dispatch, refusal, now_ms())?)
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        Ok(self.provider_record(operation, status, answer, locator, now_ms())?)
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        Ok(self.provider_locate(operation, locator, now_ms())?)
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.provider_operation(operation)?.map(|op| Entry {
            status: op.status,
            locator: op.locator,
        }))
    }
}

/// An in-memory log with the journal's rules but no fencing and no
/// durability: for tests and the bench, never for a node.
#[derive(Debug, Default)]
pub struct MemoryLog {
    entries: HashMap<OperationId, Entry>,
    keys: HashSet<OperationKey>,
}

impl MemoryLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> MemoryLog {
        MemoryLog::default()
    }
}

impl OperationLog for MemoryLog {
    fn begin(
        &mut self,
        dispatch: &ProviderDispatch,
        refusal: Option<&ProviderAnswer>,
    ) -> Result<(), LogError> {
        if self.keys.contains(&dispatch.key) || self.entries.contains_key(&dispatch.operation) {
            return Err(LogError::Used(format!(
                "{} or {}",
                dispatch.key, dispatch.operation
            )));
        }
        self.keys.insert(dispatch.key.clone());
        self.entries.insert(
            dispatch.operation,
            Entry {
                status: if refusal.is_some() {
                    ProviderStatus::Rejected
                } else {
                    ProviderStatus::Dispatched
                },
                locator: None,
            },
        );
        Ok(())
    }

    fn record(
        &mut self,
        operation: OperationId,
        status: ProviderStatus,
        _answer: &ProviderAnswer,
        locator: Option<&str>,
    ) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if entry.status.is_terminal()
            || (entry.status != status && !entry.status.may_become(status))
        {
            return Err(LogError::Transition(format!(
                "{operation} cannot go from {} to {}",
                entry.status.name(),
                status.name()
            )));
        }
        entry.status = status;
        if let Some(locator) = locator {
            entry.locator = Some(locator.to_owned());
        }
        Ok(())
    }

    fn locate(&mut self, operation: OperationId, locator: &str) -> Result<(), LogError> {
        let entry = self
            .entries
            .get_mut(&operation)
            .ok_or_else(|| LogError::Transition(format!("no provider operation {operation}")))?;
        if !entry.status.is_terminal() {
            entry.locator = Some(locator.to_owned());
        }
        Ok(())
    }

    fn entry(&self, operation: OperationId) -> Result<Option<Entry>, LogError> {
        Ok(self.entries.get(&operation).cloned())
    }
}
