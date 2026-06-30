//! `zbx-node` library facade.
//!
//! The node is primarily a binary (`src/main.rs`), but a small set of
//! modules are exposed as a library so they can be exercised by
//! integration tests under `node/tests/` without depending on the
//! binary's private item tree. Keep this surface minimal — most node
//! internals must remain private to the binary.
//!
//! Currently exposed:
//! - [`snapshot_import`] — Task #22 fast-sync manifest verification
//!   gate (used by `node/tests/snapshot_import_boundary.rs`).

pub mod snapshot_import;

// Task #15 — exposed so `node/tests/pruner_producer_e2e.rs` can drive
// the actual producer commit path (`execute_and_commit` +
// `set_retained_tracker`) end-to-end against a real `ZbxDb` with the
// production pruner subsystem wired in. Surface stays minimal: only
// the two functions the integration test needs.
#[path = "block_producer.rs"]
mod block_producer_inner;
pub mod block_producer {
    pub use crate::block_producer_inner::{execute_and_commit, set_retained_tracker};
}
