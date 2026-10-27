//! The Fabric node: one long-running process per machine and user that
//! holds the store, the job ledgers, the network endpoint and the control
//! socket the `fabric` CLI talks to (ADR 0016).
//!
//! Invariant: on any node, a job runs only while this node's ledger says
//! this node owns it at the epoch the run was planned under, and the only
//! way a job changes owner is a ledger record the core accepted: the
//! target's `MoveImported` for a cooperative move, or a `Stolen` written
//! behind `--force`. A departed copy is moved out of the job root and never
//! launched again. Everything that persists is written durably before
//! anything acts on it, so a `kill -9` at any point leaves state the next
//! command finishes or refuses with the command that resolves it.

mod authority;
mod carry;
mod catchup;
mod clock;
mod control;
mod doctor;
pub mod durable;
pub mod error;
mod facts;
mod faults;
pub mod harnesses;
mod job;
mod ledger_file;
mod local;
pub mod node;
mod ops;
pub mod paths;
mod service;
mod stamp;
pub mod tool_env;
mod transfer;
