//! Tombstone registry — permanently banned validators.
//!
//! Once tombstoned, a validator address can NEVER produce blocks again.
//! This is the harshest penalty — reserved for consensus safety attacks
//! (double signing, equivocation).
//!
//! # Tombstone vs Jail
//!
//! | Event                    | Result          | Recoverable? |
//! |--------------------------|-----------------|--------------|
//! | Missed 500 blocks        | Jailed (temp)   | Yes (unjail) |
//! | Signed wrong shard once  | Slashed 10%     | Yes          |
//! | Double-signed same block | Tombstoned 50%  | ❌ NEVER      |
//! | Consensus safety attack  | Tombstoned 100% | ❌ NEVER      |
//!
//! # Evidence
//!
//! Tombstone requires on-chain cryptographic proof:
//!   - Two signed block headers at the same height with same validator key
//!   - The signatures must verify against the validator's registered BLS key
//!   - Anyone can submit evidence — rewarded with 10% of slashed stake

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Tombstone registry — one global instance per chain.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TombstoneRegistry {
    /// Set of permanently banned validator addresses.
    pub tombstoned: HashMap<[u8; 20], TombstoneRecord>,
}

/// Record of a tombstone event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TombstoneRecord {
    /// Validator address
    pub validator:     [u8; 20],
    /// Block at which tombstone was applied
    pub at_block:      u64,
    /// Reason for tombstone
    pub reason:        TombstoneReason,
    /// Amount slashed (in wei)
    pub slashed_wei:   u128,
    /// Reporter who submitted evidence (receives 10% reward)
    pub reporter:      [u8; 20],
}

/// Why was this validator tombstoned?
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TombstoneReason {
    /// Signed two different blocks at the same height.
    DoubleSign {
        height:    u64,
        block_a:   [u8; 32],
        block_b:   [u8; 32],
    },
    /// Voted for two conflicting chains in the same BFT view.
    Equivocation {
        view:       u64,
        round:      u64,
    },
    /// Submitted fraudulent state transition proof.
    FraudulentStateProof {
        proof_hash: [u8; 32],
    },
}

impl TombstoneRegistry {
    pub fn new() -> Self { Self::default() }

    /// Apply tombstone to a validator.
    /// Returns false if already tombstoned.
    pub fn tombstone(
        &mut self,
        validator:   [u8; 20],
        at_block:    u64,
        reason:      TombstoneReason,
        slashed_wei: u128,
        reporter:    [u8; 20],
    ) -> bool {
        if self.tombstoned.contains_key(&validator) {
            tracing::warn!(
                validator = hex::encode(validator),
                "Tombstone requested but validator already tombstoned"
            );
            return false;
        }
        self.tombstoned.insert(validator, TombstoneRecord {
            validator,
            at_block,
            reason,
            slashed_wei,
            reporter,
        });
        tracing::error!(
            validator  = hex::encode(validator),
            block      = at_block,
            slashed    = slashed_wei,
            reporter   = hex::encode(reporter),
            "Validator TOMBSTONED — permanently banned"
        );
        true
    }

    /// Returns true if the validator is permanently banned.
    pub fn is_tombstoned(&self, validator: &[u8; 20]) -> bool {
        self.tombstoned.contains_key(validator)
    }

    /// Get the tombstone record for a validator.
    pub fn record(&self, validator: &[u8; 20]) -> Option<&TombstoneRecord> {
        self.tombstoned.get(validator)
    }

    /// Total number of tombstoned validators.
    pub fn count(&self) -> usize { self.tombstoned.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val() -> [u8; 20]      { [0xAA; 20] }
    fn reporter() -> [u8; 20] { [0x99; 20] }

    #[test]
    fn tombstone_validator() {
        let mut reg = TombstoneRegistry::new();
        let ok = reg.tombstone(
            val(), 1000,
            TombstoneReason::DoubleSign {
                height:  1000,
                block_a: [0x01; 32],
                block_b: [0x02; 32],
            },
            5_000 * 1_000_000_000_000_000_000u128, // 5000 ZBX slashed
            reporter(),
        );
        assert!(ok);
        assert!(reg.is_tombstoned(&val()));
    }

    #[test]
    fn double_tombstone_returns_false() {
        let mut reg = TombstoneRegistry::new();
        let reason = || TombstoneReason::Equivocation { view: 1, round: 2 };
        reg.tombstone(val(), 100, reason(), 0, reporter());
        let second = reg.tombstone(val(), 200, reason(), 0, reporter());
        assert!(!second); // already tombstoned
        assert_eq!(reg.count(), 1); // still only 1 record
    }

    #[test]
    fn non_tombstoned_is_clean() {
        let reg = TombstoneRegistry::new();
        assert!(!reg.is_tombstoned(&val()));
    }

    #[test]
    fn tombstone_record_preserved() {
        let mut reg = TombstoneRegistry::new();
        reg.tombstone(
            val(), 5000,
            TombstoneReason::FraudulentStateProof { proof_hash: [0xDE; 32] },
            1_000 * 1_000_000_000_000_000_000u128,
            reporter(),
        );
        let rec = reg.record(&val()).unwrap();
        assert_eq!(rec.at_block, 5000);
        assert_eq!(rec.reporter, reporter());
    }
}