//! Validator management operations (admin-only).

use crate::error::AdminError;
use zbx_types::address::Address;
use serde::{Serialize, Deserialize};
use tracing::{info, warn};

/// Reason codes for manual slash operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashReason {
    DoubleSign { block_a: u64, block_b: u64 },
    Liveness   { missed_votes: u32 },
    Manual     { note: String },
}

impl std::fmt::Display for SlashReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlashReason::DoubleSign { block_a, block_b } =>
                write!(f, "double-sign blocks #{} and #{}", block_a, block_b),
            SlashReason::Liveness { missed_votes } =>
                write!(f, "liveness failure: {} missed votes", missed_votes),
            SlashReason::Manual { note } =>
                write!(f, "manual: {}", note),
        }
    }
}

/// Result of a slash operation.
#[derive(Debug, Serialize, Deserialize)]
pub struct SlashResult {
    pub validator:      Address,
    pub slash_bps:      u64,    // fraction slashed (basis points)
    pub amount_slashed: u128,   // in wei
    pub jailed:         bool,
    pub reason:         String,
}

/// Slash a validator.
pub fn slash_validator(
    address:    Address,
    reason:     SlashReason,
    slash_bps:  u64,
    stake:      u128,
) -> Result<SlashResult, AdminError> {
    if slash_bps > 10_000 {
        return Err(AdminError::InvalidParam(format!(
            "slash_bps {} > 10000 (100%)", slash_bps
        )));
    }
    let amount = stake * slash_bps as u128 / 10_000;
    let jailed = slash_bps >= 1_000; // jail if >= 10%
    let reason_str = reason.to_string();

    warn!(
        "admin: slashing validator {:?} by {}bps ({} wei): {}",
        address, slash_bps, amount, reason_str
    );

    Ok(SlashResult {
        validator:      address,
        slash_bps,
        amount_slashed: amount,
        jailed,
        reason:         reason_str,
    })
}

/// Jail a validator (prevent block proposals).
pub fn jail_validator(address: Address) -> Result<(), AdminError> {
    warn!("admin: jailing validator {:?}", address);
    // In production: update ValidatorSet in the staking module.
    Ok(())
}

/// Unjail a validator.
pub fn unjail_validator(address: Address) -> Result<(), AdminError> {
    info!("admin: unjailing validator {:?}", address);
    Ok(())
}

/// Force-rotate the active validator set (emergency only).
pub fn force_epoch_transition(block: u64) -> Result<(), AdminError> {
    warn!("admin: forcing epoch transition at block #{}", block);
    Ok(())
}

/// Update a validator's commission rate.
pub fn set_commission(
    address:     Address,
    new_bps:     u64,
    max_bps:     u64,
) -> Result<(), AdminError> {
    if new_bps > max_bps {
        return Err(AdminError::InvalidParam(format!(
            "commission {}bps exceeds max {}bps", new_bps, max_bps
        )));
    }
    info!("admin: setting commission for {:?} to {}bps", address, new_bps);
    Ok(())
}