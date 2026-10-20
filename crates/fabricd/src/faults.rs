//! Named fault points for the end-to-end test (ADR 0016).
//!
//! Invariant: without the `faults` feature every function here does nothing
//! and returns `None`, so no fault control ships in a node. With it, a node
//! started with `FABRIC_FAULT=<point>[:<action>]` acts at that point:
//! `crash` (the default) writes the point's name to the file named by
//! `FABRIC_FAULT_SIGNAL` and waits to be killed with `SIGKILL`; `stall`
//! writes the same file and continues after two seconds, so the test can
//! kill the other node in that window; `drop` and `dup` are read by the call
//! site, which drops the reply it was about to send or applies the reply it
//! received twice.

use std::fmt;

/// The places the move protocol can be interrupted, in protocol order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// The source stopped the harness and has written nothing yet.
    AfterQuiesce,
    /// The snapshot is published in the store and the ledger does not name it yet.
    AfterSnapshotPublish,
    /// `MoveStarted` is durable and no offer was sent.
    AfterDepartingWrite,
    /// The target stored some of a snapshot's chunks and not all.
    MidTransfer,
    /// The target made `MoveImported` durable and did not answer yet; on the
    /// source, the answer just arrived.
    AfterImportReceipt,
}

impl Point {
    fn name(self) -> &'static str {
        match self {
            Point::AfterQuiesce => "after_quiesce",
            Point::AfterSnapshotPublish => "after_snapshot_publish",
            Point::AfterDepartingWrite => "after_departing_write",
            Point::MidTransfer => "mid_transfer",
            Point::AfterImportReceipt => "after_import_receipt",
        }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a call site does at a point the test armed with `drop` or `dup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Do not send the reply; close the connection instead.
    Drop,
    /// Apply the received reply twice.
    Dup,
}

/// Acts at `point` if the test armed it with `crash` or `stall`, and returns
/// the `drop` or `dup` action armed there, if any.
#[cfg(feature = "faults")]
pub async fn hit(point: Point) -> Option<Action> {
    let armed = std::env::var("FABRIC_FAULT").ok()?;
    let (name, action) = armed.split_once(':').unwrap_or((armed.as_str(), "crash"));
    if name != point.name() {
        return None;
    }
    let signal = || {
        if let Ok(path) = std::env::var("FABRIC_FAULT_SIGNAL") {
            let _ = std::fs::write(path, point.name());
        }
    };
    match action {
        "drop" => Some(Action::Drop),
        "dup" => Some(Action::Dup),
        "stall" => {
            signal();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            None
        }
        _ => {
            tracing::warn!(%point, "fault point reached; waiting to be killed");
            signal();
            std::future::pending::<()>().await;
            None
        }
    }
}

/// Acts at `point` if the test armed it; without the `faults` feature, nothing.
#[cfg(not(feature = "faults"))]
#[expect(
    clippy::unused_async,
    reason = "the faults build awaits here; both builds keep one signature"
)]
pub async fn hit(_point: Point) -> Option<Action> {
    None
}
