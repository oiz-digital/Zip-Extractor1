//! State rent system for ZBX chain (ZEP-008).
//!
//! # Why State Rent?
//!
//! Without state rent, state grows unboundedly as smart contracts store data forever.
//! By block 1,000,000 a node might need 100+ GB of storage.
//! State rent forces users to pay for long-term storage or let state expire.
//!
//! # ZBX State Rent Design
//!
//! Each 32-byte state slot incurs:
//!   0.0001 ZBX / year (~0.274 micro-ZBX per day)
//!
//! Rent is deducted from the contract/account balance:
//!   - Annually on each block the account is touched
//!   - Or in bulk when the account is accessed after dormancy
//!
//! If the account balance falls below MIN_BALANCE + rent_due:
//!   - Account enters "hibernation" (state merklelized, not in active trie)
//!   - To revive: owner must pay revival fee + back-rent
//!
//! If dormant > EXPIRY_PERIOD (2 years): state is permanently pruned.

pub mod rent;
pub mod scheduler;
pub mod revival;
pub mod error;

pub use rent::{RentConfig, RentLedger, RentState};
pub use error::RentError;

/// Cost per 32-byte slot per year in wei (0.0001 ZBX).
pub const SLOT_RENT_WEI_PER_YEAR: u128 = 100_000_000_000_000; // 0.0001 ZBX

/// Number of 32-byte slots below which rent is waived (small accounts).
pub const FREE_SLOTS: u64 = 5;

/// If balance < this, account is hibernated before expiry.
pub const MIN_BALANCE_WEI: u128 = 10_000_000_000_000_000; // 0.01 ZBX

/// Blocks before dormant account state is permanently pruned.
/// At 5s/block → 2 years = 12,614,400 blocks.
pub const EXPIRY_BLOCKS: u64 = 12_614_400;

/// ZBX chain block time (seconds) for rent calculations.
const BLOCK_TIME_SECS: u64 = 5;
/// Approximate blocks per year at 5s/block.
pub const BLOCKS_PER_YEAR: u64 = 365 * 24 * 3600 / BLOCK_TIME_SECS; // ~6,307,200