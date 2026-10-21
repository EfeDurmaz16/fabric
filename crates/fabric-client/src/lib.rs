//! The client API of a Fabric node (ADR 0018): typed requests, replies,
//! progress events and refusals over the node's control socket, and a
//! client that speaks them.
//!
//! Invariant: what crosses the socket is parsed into [`api`] types at the
//! boundary, on both sides, so a client never acts on an id, an epoch or a
//! run plan that did not parse, and the node never serves a request that
//! did not. The CLI and every SDK speak this one contract; the human text
//! of the CLI is the CLI's own.
//!
//! ```no_run
//! use fabric_client::Client;
//!
//! # async fn example() -> Result<(), fabric_client::ClientError> {
//! let client = Client::new("/Users/me/.fabric/fabric.sock");
//! let jobs = client.job_list(|_| {}).await?;
//! for job in jobs.jobs {
//!     let status = client.job_status(job.job, |_| {}).await?;
//!     assert_eq!(status.epoch, job.epoch);
//! }
//! # Ok(())
//! # }
//! ```

pub mod api;
mod client;
pub mod frame;
mod text;

pub use client::{Client, ClientError, Launched, Resume};
