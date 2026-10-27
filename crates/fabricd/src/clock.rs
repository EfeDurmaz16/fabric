//! The clock and the id source the core leaves to the edge.
//!
//! Invariant: the core never reads a clock or draws a random number; every
//! time and every ULID a record or a move carries is made here and handed
//! to it as a value. Times are informational (ADR 0002): nothing orders by
//! them.

use std::time::{SystemTime, UNIX_EPOCH};

use fabric_core::id::{EventId, JobId, MoveId};
use fabric_core::managed::{OperationKey, OperationKeyError};
use ulid::Ulid;

/// Milliseconds since the Unix epoch; zero before 1970.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A new job id, time-sortable and collision-free.
pub(crate) fn job_id() -> JobId {
    JobId::from_bytes(Ulid::generate().to_bytes())
}

/// A new event id for one ledger record.
pub(crate) fn event_id() -> EventId {
    EventId::from_bytes(Ulid::generate().to_bytes())
}

/// A new move id, durable in the `MoveStarted` record that carries it.
pub(crate) fn move_id() -> MoveId {
    MoveId::from_bytes(Ulid::generate().to_bytes())
}

/// A fresh operation key for a managed command whose caller named none:
/// a ULID, whose 26 Crockford base32 characters the key alphabet takes as
/// they are (docs/managed-jobs.md, Idempotency keys from command ids).
pub fn operation_key() -> Result<OperationKey, OperationKeyError> {
    OperationKey::parse(&Ulid::generate().to_string())
}
