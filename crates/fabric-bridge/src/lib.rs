//! The bridge from a Fabric node to a provider worker (ADR 0021,
//! docs/bridge-protocol.md): protocol v1's messages, their framing, and a
//! client that runs one worker over stdio.
//!
//! Ownership does not cross the bridge. Fabric owns job, task, attempt
//! and operation identity, the journal, placement and fencing; the worker
//! owns the provider mapping, provider calls, observations and receipts.
//! So the client records every effect in the journal before it writes the
//! request, never sends an operation twice, and records `unknown`, never
//! cancelled or failed, when a deadline passes or the worker is lost.
//!
//! The journal blocks on `SQLite` and fsync, so the client reaches it
//! through a [`LogThread`], which runs it on a thread of its own:
//!
//! ```no_run
//! use fabric_bridge::{Bridge, Config, Effect, LogThread};
//! # async fn example(effect: Effect, journal: fabric_journal::Journal)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! let mut log = LogThread::spawn(journal)?;
//! let worker = tokio::process::Command::new("capsule-worker");
//! let bridge = Bridge::spawn(worker, Config::new("fixture")).await?;
//! let outcome = bridge.request(&mut log, effect).await?.finish(&mut log).await?;
//! println!("{:?}", outcome.status);
//! # Ok(())
//! # }
//! ```

mod client;
pub mod frames;
mod log;
pub mod wire;

pub use client::{
    Answer, Bridge, BridgeError, Config, Control, Effect, Event, InFlight, Outcome, Pending,
};
pub use log::{Entry, LogError, LogThread, MemoryLog, OperationLog, Recorder, Slot};
