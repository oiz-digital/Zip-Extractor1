//! Staking escrow: locked-funds ledger for validators and delegators.
//!
//! ## Overview
//!
//! The escrow module maintains a per-address record of staked and unbonding
//! amounts separately from the EVM `balance` field.  This prevents the
//! common attack where an operator calls `eth_sendTransaction` to the
//! staking contract and then immediately withdraws using a re-entrancy or
//! race condition before the unbonding period elapses.
//!
//! ## Escrow lifecycle
//!
//! ```text
//! stake(addr, amount)
//!   └─ EscrowEntry.locked += amount
//!
//! begin_unbond(addr, amount)
//!   └─ EscrowEntry.locked   -= amount
//!      EscrowEntry.unbonding += UnbondingEntry { amount, release_block }
//!
//! finalise_unbond(addr, current_block)
//!   └─ For each matured UnbondingEntry:
//!        EscrowEntry.claimable += entry.amount
//!        unbonding.remove(entry)
//!
//! withdraw(addr, amount) -> Result<()>
//!   └─ EscrowEntry.claimable -= amount  (returns to EVM balance)
//! ```
//!
//! ## Slashing interaction
//!
//! `slash(addr, amount)` burns from `locked` first, then spills into
//! `unbonding` (oldest entries first) if locked < slash_amount.
//! This mirrors EigenLayer's "slashing from unbonding" semantics and
//! prevents validators from front-running slash evidence with fast
//! unbond transactions.

use crate::{error::StakingError, UNBONDING_PERIOD};
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

// ── Unbonding record ──────────────────────────────────────────────────────────

/// A single unbonding chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbondingEntry {
    /// Amount of ZBX (wei) in this unbonding chunk.
    pub amount: u128,
    /// Block number after which this chunk may be withdrawn.
    pub release_block: u64,
}

// ── Per-address escrow entry ──────────────────────────────────────────────────

/// Escrow state for a single address (validator or delegator).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EscrowEntry {
    /// Actively locked stake (participating in consensus / subject to slash).
    pub locked: u128,
    /// Chunks currently in the unbonding queue.
    pub unbonding: Vec<UnbondingEntry>,
    /// Matured unbonding ready for withdrawal.
    pub claimable: u128,
    /// Total ever staked (informational, monotonically increasing).
    pub total_staked_ever: u128,
    /// Total ever slashed (informational, monotonically increasing).
    pub total_slashed_ever: u128,
}

impl EscrowEntry {
    /// Sum of locked + unbonding + claimable.
    pub fn total_escrowed(&self) -> u128 {
        let unbonding_sum: u128 = self.unbonding.iter().map(|e| e.amount).sum();
        self.locked
            .saturating_add(unbonding_sum)
            .saturating_add(self.claimable)
    }

    /// Total unbonding (not yet released).
    pub fn total_unbonding(&self) -> u128 {
        self.unbonding.iter().map(|e| e.amount).sum()
    }
}

// ── EscrowRegistry ───────────────────────────────────────────────────────────

/// Manages staking escrow for all validators and delegators.
pub struct EscrowRegistry {
    entries: HashMap<Address, EscrowEntry>,
}

impl EscrowRegistry {
    pub fn new() -> Self {
        EscrowRegistry { entries: HashMap::new() }
    }

    /// Lock `amount` wei for `addr`.  Called when a validator self-stakes or
    /// a delegator delegates.
    pub fn stake(&mut self, addr: &Address, amount: u128) {
        if amount == 0 { return; }
        let e = self.entries.entry(addr.clone()).or_default();
        e.locked = e.locked.saturating_add(amount);
        e.total_staked_ever = e.total_staked_ever.saturating_add(amount);
        debug!(?addr, amount, locked = e.locked, "escrow: stake");
    }

    /// Begin unbonding `amount` wei for `addr`.
    ///
    /// Returns `Err` if `addr` has less than `amount` locked.
    pub fn begin_unbond(
        &mut self,
        addr: &Address,
        amount: u128,
        current_block: u64,
    ) -> Result<u64, StakingError> {
        if amount == 0 {
            return Err(StakingError::InvalidAmount);
        }
        let e = self.entries.entry(addr.clone()).or_default();
        if e.locked < amount {
            return Err(StakingError::InsufficientStake);
        }
        e.locked -= amount;
        let release_block = current_block + UNBONDING_PERIOD;
        e.unbonding.push(UnbondingEntry { amount, release_block });
        info!(
            ?addr,
            amount,
            release_block,
            "escrow: unbonding started"
        );
        Ok(release_block)
    }

    /// Scan unbonding entries and move matured ones to `claimable`.
    ///
    /// Must be called once per block (or at least before `withdraw`).
    pub fn finalise_unbond(&mut self, addr: &Address, current_block: u64) -> u128 {
        let e = match self.entries.get_mut(addr) {
            Some(e) => e,
            None => return 0,
        };
        let mut released = 0u128;
        e.unbonding.retain(|entry| {
            if current_block >= entry.release_block {
                released = released.saturating_add(entry.amount);
                false // remove from unbonding
            } else {
                true  // keep
            }
        });
        if released > 0 {
            e.claimable = e.claimable.saturating_add(released);
            debug!(?addr, released, claimable = e.claimable, "escrow: unbonding matured");
        }
        released
    }

    /// Withdraw `amount` from `claimable` balance.
    ///
    /// The caller is responsible for crediting the EVM account balance.
    pub fn withdraw(&mut self, addr: &Address, amount: u128) -> Result<(), StakingError> {
        if amount == 0 {
            return Err(StakingError::InvalidAmount);
        }
        let e = self.entries.entry(addr.clone()).or_default();
        if e.claimable < amount {
            return Err(StakingError::NothingToWithdraw);
        }
        e.claimable -= amount;
        info!(?addr, amount, remaining_claimable = e.claimable, "escrow: withdraw");
        Ok(())
    }

    /// Slash `slash_amount` wei from `addr`.
    ///
    /// Burns locked first; if locked is insufficient, spills into the
    /// unbonding queue (oldest entries first).
    ///
    /// Returns the amount actually slashed (may be less than requested if
    /// the address has insufficient escrow — honest limitation).
    pub fn slash(
        &mut self,
        addr: &Address,
        mut slash_amount: u128,
    ) -> u128 {
        let e = match self.entries.get_mut(addr) {
            Some(e) => e,
            None => {
                warn!(?addr, slash_amount, "escrow: slash requested for unknown address");
                return 0;
            }
        };

        let mut actually_slashed = 0u128;

        // Burn from locked first.
        let from_locked = slash_amount.min(e.locked);
        e.locked -= from_locked;
        slash_amount -= from_locked;
        actually_slashed += from_locked;

        // Spill into unbonding (oldest entries first).
        if slash_amount > 0 {
            for entry in e.unbonding.iter_mut() {
                if slash_amount == 0 { break; }
                let from_entry = slash_amount.min(entry.amount);
                entry.amount -= from_entry;
                slash_amount -= from_entry;
                actually_slashed += from_entry;
            }
            // Remove fully-slashed entries.
            e.unbonding.retain(|entry| entry.amount > 0);
        }

        e.total_slashed_ever = e.total_slashed_ever.saturating_add(actually_slashed);
        warn!(
            ?addr,
            actually_slashed,
            remaining_locked = e.locked,
            "escrow: slash applied"
        );
        actually_slashed
    }

    /// Read the escrow entry for `addr` (returns a default if not found).
    pub fn get(&self, addr: &Address) -> EscrowEntry {
        self.entries.get(addr).cloned().unwrap_or_default()
    }

    /// Iterate over all entries (used by snapshots and state export).
    pub fn iter(&self) -> impl Iterator<Item = (&Address, &EscrowEntry)> {
        self.entries.iter()
    }
}

impl Default for EscrowRegistry {
    fn default() -> Self { Self::new() }
}
