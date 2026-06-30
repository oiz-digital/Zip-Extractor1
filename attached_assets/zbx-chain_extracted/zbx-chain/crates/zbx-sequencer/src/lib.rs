//! zbx-sequencer — Block proposer and sequencer for ZBX Chain.
//!
//! The sequencer is responsible for:
//!   1. Selecting transactions from the mempool (highest fee first)
//!   2. Ordering txs to comply with MEV protection rules
//!   3. Executing the selected txs to produce the block body
//!   4. Sealing the block (computing state root, receipts root)
//!   5. Proposing the block to the consensus layer (HotStuff)
//!
//! ## Sequencer Modes
//!
//! **Single-sequencer (v0.1 PoA):**
//! ```
//! Validator → Sequencer → Block → HotStuff → Network
//! ```
//!
//! **PBS (v0.2 BFT, with MEV protection):**
//! ```
//! Validator (proposer) ← Winning bid ← PBS Relay ← Block Builders
//!      │
//!      └─ Seals header → HotStuff vote
//! ```
//!
//! **Decentralised (v0.3, fully permissionless):**
//! All validators rotate as sequencer each epoch.

pub mod block_builder;
pub mod error;
pub mod ordering;
pub mod proposer;
pub mod sealer;
pub mod slot_timer;

pub use block_builder::BlockAssembler;
pub use error::SequencerError;
pub use proposer::Proposer;
pub use sealer::BlockSealer;
pub use slot_timer::SlotTimer;