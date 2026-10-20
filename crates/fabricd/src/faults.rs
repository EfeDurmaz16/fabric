//! Named fault points for the end-to-end test (ADR 0016).
//!
//! Invariant: without the `faults` feature every function here does nothing
//! and returns `None`, so no fault control ships in a node. With it, a node
//! started with `FABRIC_FAULT=<point>[:<action>]` acts the first time it
//! reaches that point and never again in that process, and a
//! value that names no point or action this build knows stops the node at
//! start, so a test never runs with a fault it believes armed and is not:
//! `crash` (the default) writes the point's name to the file named by
//! `FABRIC_FAULT_SIGNAL` and waits to be killed with `SIGKILL`; `stall`
//! writes the same file and continues after two seconds, so the test can
//! kill the other node in that window; `hold` writes the file and waits
//! until the test removes it, so the test can act on this node's peer for
//! as long as it needs; `drop` and `dup` are read by the call site, which
//! drops the reply it was about to send or applies the reply it received
//! twice.

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
    /// Every point, for parsing.
    #[cfg(feature = "faults")]
    const ALL: [Point; 5] = [
        Point::AfterQuiesce,
        Point::AfterSnapshotPublish,
        Point::AfterDepartingWrite,
        Point::MidTransfer,
        Point::AfterImportReceipt,
    ];

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

/// `FABRIC_FAULT` names a point or an action this build does not know.
#[derive(Debug, thiserror::Error)]
#[error(
    "FABRIC_FAULT={0:?} is not <point>[:crash|stall|hold|drop|dup] for a point this build knows"
)]
pub struct UnknownFault(String);

#[cfg(feature = "faults")]
mod armed {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{Action, Point, UnknownFault};

    /// What the node does when it reaches the armed point.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Mode {
        Crash,
        Stall,
        Hold,
        Reply(Action),
    }

    static ARMED: OnceLock<Option<(Point, Mode)>> = OnceLock::new();
    static FIRED: AtomicBool = AtomicBool::new(false);

    /// Reads `spec` as `<point>[:<action>]`.
    pub(super) fn parse(spec: &str) -> Result<(Point, Mode), UnknownFault> {
        let unknown = || UnknownFault(spec.to_owned());
        let (name, action) = spec.split_once(':').unwrap_or((spec, "crash"));
        let point = Point::ALL
            .into_iter()
            .find(|point| point.name() == name)
            .ok_or_else(unknown)?;
        let mode = match action {
            "crash" => Mode::Crash,
            "stall" => Mode::Stall,
            "hold" => Mode::Hold,
            "drop" => Mode::Reply(Action::Drop),
            "dup" => Mode::Reply(Action::Dup),
            _ => return Err(unknown()),
        };
        Ok((point, mode))
    }

    pub(super) fn arm() -> Result<(), UnknownFault> {
        let armed = match std::env::var("FABRIC_FAULT") {
            Ok(spec) => Some(parse(&spec)?),
            Err(_) => None,
        };
        let _ = ARMED.set(armed);
        Ok(())
    }

    pub(super) async fn hit(point: Point) -> Option<Action> {
        let (armed, mode) = (*ARMED.get()?)?;
        if armed != point || FIRED.swap(true, Ordering::SeqCst) {
            return None;
        }
        let signal = std::env::var_os("FABRIC_FAULT_SIGNAL");
        if let Some(path) = &signal {
            let _ = std::fs::write(path, point.name());
        }
        match mode {
            Mode::Reply(action) => {
                tracing::warn!(%point, ?action, "fault point reached");
                Some(action)
            }
            Mode::Stall => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                None
            }
            Mode::Hold => {
                tracing::warn!(%point, "fault point reached; holding until the signal file is removed");
                while signal
                    .as_ref()
                    .is_some_and(|path| std::path::Path::new(path).exists())
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                None
            }
            Mode::Crash => {
                tracing::warn!(%point, "fault point reached; waiting to be killed");
                std::future::pending::<()>().await;
                None
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_unknown_point_or_action_is_refused() {
            assert_eq!(
                parse("mid_transfer:hold").unwrap(),
                (Point::MidTransfer, Mode::Hold)
            );
            assert_eq!(
                parse("after_quiesce").unwrap(),
                (Point::AfterQuiesce, Mode::Crash)
            );
            assert!(parse("mid_transfr").is_err());
            assert!(parse("mid_transfer:stal").is_err());
        }
    }
}

/// Reads `FABRIC_FAULT` once, at start, and refuses a value this build does
/// not know.
#[cfg(feature = "faults")]
pub fn arm() -> Result<(), UnknownFault> {
    armed::arm()
}

/// Reads `FABRIC_FAULT` once, at start; without the `faults` feature, nothing.
#[cfg(not(feature = "faults"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the faults build can refuse here; both builds keep one signature"
)]
pub fn arm() -> Result<(), UnknownFault> {
    Ok(())
}

/// Acts at `point` if the test armed it with `crash`, `stall` or `hold`,
/// and returns the `drop` or `dup` action armed there, if any.
#[cfg(feature = "faults")]
pub async fn hit(point: Point) -> Option<Action> {
    armed::hit(point).await
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
