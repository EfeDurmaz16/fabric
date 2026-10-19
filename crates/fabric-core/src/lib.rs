//! Core types of Fabric: ids, the job ledger and its lease, the move
//! protocol, snapshot manifests and the environment spec.
//!
//! This crate is pure. It does no IO, holds no clock and opens no socket.
//! It depends on nothing else in the workspace and knows nothing about QUIC,
//! a database or any agent vendor. Everything above it is an adapter that
//! feeds it bytes and persists what it returns.
//!
//! The one invariant the whole crate serves: a job has exactly one live copy.
//! The ledger refuses any record that would give it two.
//!
//! # A move between two nodes
//!
//! ```
//! use fabric_core::harness::HarnessKind;
//! use fabric_core::id::{EventId, JobId, MoveId, NodeId, SnapshotId};
//! use fabric_core::ledger::{Author, EventKind, Ledger, LedgerStore, MemoryStore};
//! use fabric_core::moves::{CompleteSnapshot, LeaseView, OfferAnswer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let job = JobId::generate();
//! let pro = NodeId::from_bytes([1; 32]);
//! let air = NodeId::from_bytes([2; 32]);
//! let now = 1_760_000_000_000;
//!
//! // The Pro creates the job and owns it at epoch 1.
//! let created = Ledger::created(job, "fix the flaky test".into(), HarnessKind::ClaudeCode, pro, EventId::generate(), now)?;
//! let mut pro_store = MemoryStore::default();
//! pro_store.append(&created)?;
//! let mut on_pro = Ledger::open(job, &pro_store)?;
//! assert!(on_pro.runnable_by(pro).is_some());
//!
//! // Suspend: publish a snapshot, then record the suspend.
//! let snapshot = SnapshotId::of(b"manifest bytes");
//! let published = on_pro.propose(pro, EventKind::SnapshotPublished { snapshot, parent: None }, EventId::generate(), now)?;
//! on_pro.commit(&mut pro_store, published, Author::Node(pro))?;
//! let suspended = on_pro.propose(pro, EventKind::Suspended { snapshot }, EventId::generate(), now)?;
//! on_pro.commit(&mut pro_store, suspended, Author::Node(pro))?;
//!
//! // The Air got the Created record when the job was made and verifies every
//! // later record against the node the lease names as its writer.
//! let mut air_store = MemoryStore::default();
//! air_store.append(&created)?;
//! let mut on_air = Ledger::open(job, &air_store)?;
//!
//! // Step 3: the Pro commits to departing. From here it is not runnable.
//! let started = on_pro.start_move(pro, air, MoveId::generate(), EventId::generate(), now)?;
//! on_pro.commit(&mut pro_store, started, Author::Node(pro))?;
//! assert!(on_pro.runnable_by(pro).is_none());
//! let offer = on_pro.offer(pro).expect("departing source has an offer");
//!
//! // The Air first needs the MoveStarted record itself, then decides.
//! let suffix = on_pro.suffix_after(on_air.head()).expect("same chain");
//! for record in suffix {
//!     let checked = on_air.verify(record.bytes(), pro)?;
//!     on_air.commit(&mut air_store, checked, Author::Node(pro))?;
//! }
//! let complete = Some(CompleteSnapshot::new(snapshot));
//! let decision = match on_air.answer_offer(air, &offer, on_pro.lease().epoch(), complete, EventId::generate(), now)? {
//!     OfferAnswer::Import(record) => on_air.commit(&mut air_store, record, Author::Node(air))?.clone(),
//!     other => panic!("expected an import, got {other:?}"),
//! };
//! assert!(on_air.runnable_by(air).is_some());
//!
//! // The reply reaches the Pro, possibly twice. The first completes the move;
//! // the second does not link and changes nothing.
//! let completed = on_pro.verify(decision.bytes(), air)?;
//! on_pro.commit(&mut pro_store, completed, Author::Node(air))?;
//! assert!(on_pro.verify(decision.bytes(), air).is_err());
//! assert!(matches!(on_pro.view(pro), LeaseView::Departed { owner, .. } if owner == air));
//! assert_eq!(on_pro.lease(), on_air.lease());
//! # Ok(()) }
//! ```

pub mod codec;
pub mod env;
pub mod git;
pub mod harness;
pub mod id;
pub mod lease;
pub mod ledger;
pub mod moves;
pub mod path;
pub mod snapshot;
