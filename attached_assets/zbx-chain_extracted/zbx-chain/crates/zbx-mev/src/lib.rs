//! zbx-mev — MEV protection for Zebvix Chain.
//!
//! ## What is MEV?
//! Maximal Extractable Value (MEV) is profit extracted by block producers who
//! can reorder, insert, or censor transactions. Left unchecked, it leads to
//! sandwich attacks, frontrunning, and unfair markets for users.
//!
//! ## ZBX Chain MEV Protection Strategy
//!
//! **Layer 1 — Private mempool (encrypted tx submission):**
//!   - Txs submitted via `zbx_sendPrivateTransaction` are encrypted.
//!   - Block builders cannot see tx content before block production.
//!   - Revealed only at block sealing time.
//!
//! **Layer 2 — Commit-reveal ordering:**
//!   - Tx hash committed in round N, content revealed in round N+1.
//!   - Prevents last-second frontrunning.
//!
//! **Layer 3 — PBS (Proposer-Builder Separation):**
//!   - Block validators propose slots, specialised builders fill them.
//!   - Builders bid for slots; the highest bid wins.
//!   - Prevents validator-level MEV extraction.
//!
//! **Layer 4 — MEV redistribution:**
//!   - Extracted MEV is captured and redistributed to stakers (30%)
//!     and a community fund (70%).

pub mod builder;
pub mod bundle;
pub mod commit_reveal;
pub mod error;
pub mod pbs;
pub mod private_pool;
pub mod redistribution;

pub use builder::{BlockBuilder, BuilderBid};
pub use bundle::{MevBundle, BundleSimulation};
pub use error::MevError;
pub use pbs::{PbsRelay, SlotAuction};
pub use private_pool::PrivateMempool;
pub use redistribution::MevRedistribution;
pub use commit_reveal::CommitRevealPool;

// ── High-level runner (MEV node wiring) ──────────────────────────────────────
pub mod service;
pub use service::MevCoordinator;