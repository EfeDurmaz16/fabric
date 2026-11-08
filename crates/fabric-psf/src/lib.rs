//! The Portable Session Format v1 (ADR 0029, `docs/portable-session-format.md`).
//!
//! A PSF container is a directory with a canonical JSON manifest and the
//! blobs it names by sha256. It is the public form of one coding-agent job:
//! the workspace revision by its bundle and ledger heads, the caches that
//! travel with it, the agent's native session as opaque bytes, and for a
//! managed job the work package of ADR 0024. This crate reads and checks a
//! manifest under every rule of the document, in its order, so it refuses
//! the same documents with the same code and field as the other two
//! readers (`sdk/typescript/src/psf.ts` and `scripts/psf_check.py`); writes
//! manifests canonically through the same checks; reads and writes
//! containers; and computes the fidelity a target can achieve.
//!
//! The format does not depend on Fabric's store: it names a revision, it
//! does not carry one. [`harness_state`] packs a captured harness state
//! tree into the one attachment format Fabric's adapters read.

mod achieve;
mod canon;
mod check;
mod container;
pub mod harness_state;
mod manifest;
mod refusal;
mod sha256;
mod verdict;

pub use achieve::{Achieved, HARNESS_STATE_FORMAT, Target, achievable};
pub use check::{
    CACHES_MAX, LIST_MAX, MANIFEST_MAX_BYTES, NAME_MAX_BYTES, PACKAGE_MAX_BYTES, TEXT_MAX_BYTES,
};
pub use container::{
    BLOBS_DIR, Container, MANIFEST_FILE, WriteError, read_container, write_container,
};
pub use manifest::{
    Attachment, Bound, Cache, Evidence, Fidelity, Harness, Job, Manifest, Parts, Produced, Source,
    Workspace, read_manifest, write_manifest,
};
pub use refusal::{Code, ContainerCode, ContainerRefusal, Refusal};
pub use sha256::{hex, sha256};
pub use verdict::{
    FixtureError, case_verdict, container_verdict, fixture_target, fixture_verdicts,
};
