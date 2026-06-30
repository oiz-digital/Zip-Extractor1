//! Core rent accounting and deduction logic.

use serde::{Serialize, Deserialize};
use crate::{SLOT_RENT_WEI_PER_YEAR, FREE_SLOTS, MIN_BALANCE_WEI, BLOCKS_PER_YEAR, EXPIRY_BLOCKS};
use crate::error::RentError;

/// Rent configuration (can be updated via governance).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RentConfig {
    /// Cost per slot per year in wei.
    pub slot_rent_per_year: u128,
    /// Minimum balance to keep account active.
    pub min_balance:        u128,
    /// Slots below which rent is free.
    pub free_slots:         u64,
    /// Blocks before expired state is pruned.
    pub expiry_blocks:      u64,
}

impl Default for RentConfig {
    fn default() -> Self {
        Self {
            slot_rent_per_year: SLOT_RENT_WEI_PER_YEAR,
            min_balance:        MIN_BALANCE_WEI,
            free_slots:         FREE_SLOTS,
            expiry_blocks:      EXPIRY_BLOCKS,
        }
    }
}

/// The rent state of a single account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RentState {
    /// Last block at which rent was collected.
    pub last_rent_block:    u64,
    /// Number of 32-byte storage slots used.
    pub slot_count:         u64,
    /// Is this account hibernated (balance too low)?
    pub hibernated:         bool,
    /// Block at which hibernation started (for expiry).
    pub hibernated_at:      Option<u64>,
}

impl RentState {
    pub fn new(slot_count: u64, current_block: u64) -> Self {
        Self {
            last_rent_block: current_block,
            slot_count,
            hibernated: false,
            hibernated_at: None,
        }
    }
}

/// Per-block rent calculation results.
#[derive(Debug, Clone)]
pub struct RentLedger {
    pub due_wei:     u128,   // rent owed this collection
    pub blocks_owed: u64,    // blocks elapsed since last collection
    pub chargeable:  u64,    // chargeable slots (slot_count - free_slots)
}

impl RentLedger {
    /// Compute rent due for an account from last collection to current block.
    ///
    /// Formula:
    ///   chargeable_slots = max(0, slot_count - free_slots)
    ///   elapsed_years    = blocks_since / BLOCKS_PER_YEAR
    ///   rent_due         = chargeable_slots × slot_rent_per_year × elapsed_years
    pub fn compute(
        state:         &RentState,
        config:        &RentConfig,
        current_block: u64,
    ) -> Self {
        let blocks_since = current_block.saturating_sub(state.last_rent_block);
        let chargeable   = state.slot_count.saturating_sub(config.free_slots);

        // Rent = slots × rate × (blocks / blocks_per_year)
        // Use integer math: (slots × rate × blocks) / blocks_per_year
        let due_wei = if chargeable == 0 || blocks_since == 0 {
            0u128
        } else {
            (chargeable as u128)
                .saturating_mul(config.slot_rent_per_year)
                .saturating_mul(blocks_since as u128)
                / BLOCKS_PER_YEAR as u128
        };

        Self {
            due_wei,
            blocks_owed: blocks_since,
            chargeable,
        }
    }
}

/// Try to collect rent from an account balance.
///
/// Returns: (new_balance, is_hibernated, should_expire)
pub fn collect_rent(
    balance:       u128,
    state:         &mut RentState,
    config:        &RentConfig,
    current_block: u64,
) -> Result<(u128, bool, bool), RentError> {
    let ledger = RentLedger::compute(state, config, current_block);

    if ledger.due_wei > balance {
        // Cannot pay rent — hibernation
        let new_balance = 0;
        state.hibernated    = true;
        state.hibernated_at = Some(current_block);
        state.last_rent_block = current_block;

        tracing::warn!(
            slots = state.slot_count,
            due   = ledger.due_wei,
            balance,
            "Account hibernated: insufficient balance for state rent"
        );

        return Ok((new_balance, true, false));
    }

    let new_balance = balance - ledger.due_wei;

    // Check if balance is below minimum — trigger hibernation even if rent paid
    if new_balance < config.min_balance && state.slot_count > config.free_slots {
        state.hibernated    = true;
        state.hibernated_at = Some(current_block);
        tracing::info!(balance = new_balance, "Account hibernated: below minimum balance");
    }

    state.last_rent_block = current_block;

    // Check expiry for already-hibernated accounts
    let should_expire = if let Some(hiber_at) = state.hibernated_at {
        current_block.saturating_sub(hiber_at) > config.expiry_blocks
    } else {
        false
    };

    if should_expire {
        tracing::warn!("Account state expired — pruning from trie");
    }

    tracing::debug!(
        due_wei = ledger.due_wei,
        new_balance,
        chargeable = ledger.chargeable,
        blocks = ledger.blocks_owed,
        "Rent collected"
    );

    Ok((new_balance, false, should_expire))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state(slots: u64, block: u64) -> RentState { RentState::new(slots, block) }
    fn cfg() -> RentConfig { RentConfig::default() }

    #[test]
    fn zero_rent_for_free_slots() {
        let s = default_state(FREE_SLOTS, 0);
        let l = RentLedger::compute(&s, &cfg(), BLOCKS_PER_YEAR);
        assert_eq!(l.due_wei, 0, "Free slots should incur no rent");
    }

    #[test]
    fn one_year_one_slot_above_free() {
        let s = default_state(FREE_SLOTS + 1, 0);
        let l = RentLedger::compute(&s, &cfg(), BLOCKS_PER_YEAR);
        assert_eq!(l.due_wei, SLOT_RENT_WEI_PER_YEAR, "One chargeable slot for one year");
    }

    #[test]
    fn partial_year() {
        let s = default_state(FREE_SLOTS + 2, 0);
        let l = RentLedger::compute(&s, &cfg(), BLOCKS_PER_YEAR / 2);
        let expected = 2 * SLOT_RENT_WEI_PER_YEAR / 2;
        assert_eq!(l.due_wei, expected, "Half-year for 2 slots");
    }

    #[test]
    fn hibernation_when_balance_insufficient() {
        let mut s  = default_state(10, 0);
        let balance = 1; // almost nothing
        let (new_bal, hibernated, _) = collect_rent(balance, &mut s, &cfg(), BLOCKS_PER_YEAR).unwrap();
        assert!(hibernated, "Must hibernate when rent exceeds balance");
        assert_eq!(new_bal, 0);
    }

    #[test]
    fn rent_collection_reduces_balance() {
        let mut s = default_state(FREE_SLOTS + 5, 0);
        let bal: u128 = 1_000 * 10u128.pow(18); // 1000 ZBX
        let (new_bal, hibernated, expired) = collect_rent(bal, &mut s, &cfg(), BLOCKS_PER_YEAR).unwrap();
        assert!(!hibernated);
        assert!(!expired);
        assert!(new_bal < bal, "Balance must decrease after rent");
        assert_eq!(bal - new_bal, 5 * SLOT_RENT_WEI_PER_YEAR);
    }
}