//! zbx-execution: Block and transaction execution for Zebvix.
//!
//! Provides both sequential and parallel (Block-STM) execution modes.
//! The executor reads world state from ZbxDb, applies transactions via the EVM,
//! and writes the resulting state diff back atomically.

pub mod bloom;
pub mod error;
pub mod executor;
pub mod host_zvm;
pub mod parallel;
pub mod scheduler;
pub mod state_diff;
pub mod verifier;

pub use bloom::{
    aggregate_block_bloom, bloom_add, compute_receipt_bloom, compute_receipt_hash,
    compute_receipts_root, compute_tx_root,
};
pub use error::ExecutionError;
pub use executor::{BlockExecutor, ExecutionResult, StateView, ZVM_DEPLOY_DISCRIMINATOR};
pub use host_zvm::{ProductionZvmHost, TransientScratchpad, ZvmBlockEnv};
pub use scheduler::{schedule, AccessKey, AccessSet, Lanes};
pub use state_diff::StateDiff;