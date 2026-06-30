//! Finality gadget types for ZBX Chain.
//!
//! # Migration (2026-06-27)
//!
//! These types were originally implemented in this crate and depended on
//! `zbx-primitives`. They have been moved into `zbx-consensus::finality`
//! (which uses `zbx-types`, consistent with the rest of the consensus stack).
//!
//! This crate is now a thin re-export shim so any future consumer can still
//! `use zbx_finality::FinalityTracker` without breakage. New code should
//! import directly from `zbx_consensus`:
//!
//! ```rust,ignore
//! use zbx_consensus::{Checkpoint, FinalityTracker, Justification};
//! ```

pub use zbx_consensus::finality::Checkpoint;
pub use zbx_consensus::finality::FinalityTracker;
pub use zbx_consensus::finality::Justification;
