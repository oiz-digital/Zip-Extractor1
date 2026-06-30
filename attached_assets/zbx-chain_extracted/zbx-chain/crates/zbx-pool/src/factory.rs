//! Pool factory — create liquidity pools with a paid fee.
//!
//! ## Flow
//!
//! ```text
//! User ──create_pool(token_a, token_b, fee_tier)──►
//!   PoolFactory
//!     ├── Checks: tokens distinct, pair doesn't already exist
//!     ├── Collects POOL_CREATION_FEE_WEI (paid in ZBX) from creator
//!     ├── Assigns deterministic pool address (keccak256-based)
//!     ├── Registers pair in the pairs registry
//!     └── Emits PoolCreated event record
//! ```
//!
//! ## Fee tiers
//!
//! | Tier | bps | Use Case |
//! |------|-----|----------|
//! | Lowest  | 5  | Stablecoin pairs (ZUSD/USDC) |
//! | Standard | 30 | General ERC-20 pairs (default) |
//! | High | 100 | Exotic / highly volatile tokens |
//!
//! ## Pool creation fee
//!
//! Creating a pool costs `POOL_CREATION_FEE_WEI` (500 ZBX) to prevent spam
//! and fund protocol liquidity bootstrapping. Fee goes to the protocol treasury.

use std::collections::HashMap;
use sha3::{Digest, Sha3_256};
use zbx_types::address::Address;
use serde::{Deserialize, Serialize};
use crate::{
    error::AmmError,
    fee::FeeTier,
    pair::{Pair, PairId},
    registry::FeeRegistry,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Pool creation fee in ZBX wei (500 ZBX).
/// Prevents spam and funds protocol treasury / liquidity bootstrapping.
pub const POOL_CREATION_FEE_WEI: u128 = 500 * 10u128.pow(18);

/// Protocol treasury — receives pool creation fees.
pub const PROTOCOL_TREASURY: [u8; 20] = [
    0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01,
];

// ── Event types ───────────────────────────────────────────────────────────────

/// Emitted when a new pool is created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolCreatedEvent {
    pub pair_id:       PairId,
    pub fee_tier:      FeeTier,
    pub pool_address:  Address,
    pub creator:       Address,
    pub creation_fee:  u128,
    pub block_number:  u64,
}

// ── Pool registration record ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolRecord {
    pub pair_id:      PairId,
    pub fee_tier:     FeeTier,
    pub pool_address: Address,
    pub creator:      Address,
    pub created_at:   u64,
    pub total_volume: u128,
}

// ── PoolFactory ────────────────────────────────────────────────────────────────

/// Registry and factory for all liquidity pools on ZBX DEX.
pub struct PoolFactory {
    /// All live pools keyed by PairId.
    pub pairs: HashMap<PairId, Pair>,
    /// Pool metadata.
    pool_records: HashMap<PairId, PoolRecord>,
    /// Creation events log.
    pub events: Vec<PoolCreatedEvent>,
    /// Collected protocol fees (ZBX wei).
    pub treasury_balance: u128,
    /// Fee registry for platform fees.
    fee_registry: FeeRegistry,
}

impl PoolFactory {
    pub fn new() -> Self {
        PoolFactory {
            pairs:            HashMap::new(),
            pool_records:     HashMap::new(),
            events:           Vec::new(),
            treasury_balance: 0,
            fee_registry:     FeeRegistry::default(),
        }
    }

    // ── Pool creation ──────────────────────────────────────────────────────────

    /// Create a new liquidity pool.
    ///
    /// # Arguments
    /// - `token_a`, `token_b` — the two tokens of the pair (order doesn't matter; canonicalised)
    /// - `fee_tier` — swap fee level for this pool
    /// - `creator` — wallet creating the pool (pays the creation fee)
    /// - `creator_zbx_balance` — creator's ZBX balance available (checked against fee)
    /// - `block_number` — current block
    ///
    /// # Returns
    /// `(pool_address, creation_fee_collected)`
    ///
    /// # Errors
    ///
    /// | Error | Condition |
    /// |-------|-----------|
    /// | `IdenticalTokens` | `token_a == token_b` |
    /// | `PoolAlreadyExists` | A pool for this pair already exists |
    /// | `InsufficientCreationFee` | Creator balance < pool creation fee |
    pub fn create_pool(
        &mut self,
        token_a:              Address,
        token_b:              Address,
        fee_tier:             FeeTier,
        creator:              Address,
        creator_zbx_balance:  u128,
        block_number:         u64,
    ) -> Result<(Address, u128), AmmError> {
        // Tokens must be different
        if token_a == token_b {
            return Err(AmmError::IdenticalTokens);
        }

        let pair_id = PairId::new(token_a, token_b);

        // Pool must not already exist
        if self.pairs.contains_key(&pair_id) {
            return Err(AmmError::PoolAlreadyExists);
        }

        // Creator must have enough ZBX for the creation fee
        let creation_fee = self.fee_registry.pool_creation_fee();
        if creator_zbx_balance < creation_fee {
            return Err(AmmError::InsufficientCreationFee {
                have: creator_zbx_balance,
                need: creation_fee,
            });
        }

        // Deterministic pool address from SHA3(token_a || token_b || fee_tier_byte)
        let pool_address = derive_pool_address(&pair_id, fee_tier);

        // Collect creation fee → treasury
        self.treasury_balance = self.treasury_balance.saturating_add(creation_fee);

        // Create the pair
        let pair = Pair::new(pair_id.clone(), fee_tier);
        self.pairs.insert(pair_id.clone(), pair);

        // Record metadata
        let record = PoolRecord {
            pair_id:      pair_id.clone(),
            fee_tier,
            pool_address,
            creator,
            created_at:   block_number,
            total_volume: 0,
        };
        self.pool_records.insert(pair_id.clone(), record);

        // Emit event
        self.events.push(PoolCreatedEvent {
            pair_id:       pair_id.clone(),
            fee_tier,
            pool_address,
            creator,
            creation_fee,
            block_number,
        });

        Ok((pool_address, creation_fee))
    }

    // ── Queries ────────────────────────────────────────────────────────────────

    pub fn get_pair(&self, pair_id: &PairId) -> Option<&Pair> {
        self.pairs.get(pair_id)
    }

    pub fn get_pair_mut(&mut self, pair_id: &PairId) -> Option<&mut Pair> {
        self.pairs.get_mut(pair_id)
    }

    pub fn get_record(&self, pair_id: &PairId) -> Option<&PoolRecord> {
        self.pool_records.get(pair_id)
    }

    pub fn pool_count(&self) -> usize {
        self.pairs.len()
    }

    pub fn all_pair_ids(&self) -> Vec<PairId> {
        self.pairs.keys().cloned().collect()
    }

    pub fn fee_registry(&self) -> &FeeRegistry {
        &self.fee_registry
    }

    /// Record swap volume for a pool (called by DexEngine after each swap).
    pub fn record_volume(&mut self, pair_id: &PairId, volume_wei: u128) {
        if let Some(r) = self.pool_records.get_mut(pair_id) {
            r.total_volume = r.total_volume.saturating_add(volume_wei);
        }
    }
}

impl Default for PoolFactory {
    fn default() -> Self { Self::new() }
}

// ── Address derivation ────────────────────────────────────────────────────────

/// Derive a deterministic pool address from pair tokens and fee tier.
///
/// Formula: first 20 bytes of SHA3-256(token_a.0 || token_b.0 || [fee_bps_byte])
/// where token_a < token_b (canonical ordering, already enforced by PairId::new).
fn derive_pool_address(pair_id: &PairId, fee_tier: FeeTier) -> Address {
    let mut h = Sha3_256::new();
    h.update(&pair_id.token_a.0);
    h.update(&pair_id.token_b.0);
    h.update([(fee_tier.bps() & 0xFF) as u8]);
    let digest = h.finalize();
    let mut bytes = [0u8; 20];
    bytes.copy_from_slice(&digest[..20]);
    Address(bytes)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address { Address([n; 20]) }

    #[test]
    fn create_pool_success() {
        let mut f = PoolFactory::new();
        let fee = f.fee_registry.pool_creation_fee();
        let (pool_addr, collected) = f.create_pool(
            addr(1), addr(2), FeeTier::Standard,
            addr(99), fee + 100, 1,
        ).unwrap();
        assert_ne!(pool_addr, addr(0));
        assert_eq!(collected, fee);
        assert_eq!(f.pool_count(), 1);
        assert_eq!(f.treasury_balance, fee);
    }

    #[test]
    fn duplicate_pool_rejected_with_correct_error() {
        let mut f = PoolFactory::new();
        let fee = f.fee_registry.pool_creation_fee();
        f.create_pool(addr(1), addr(2), FeeTier::Standard, addr(99), fee * 10, 1).unwrap();
        let r = f.create_pool(addr(1), addr(2), FeeTier::Standard, addr(99), fee * 10, 2);
        // FIX: was AmmError::EmptyReserve — now proper PoolAlreadyExists
        assert!(
            matches!(r, Err(AmmError::PoolAlreadyExists)),
            "duplicate pool must return PoolAlreadyExists, got {:?}", r
        );
    }

    #[test]
    fn identical_tokens_rejected_with_correct_error() {
        let mut f = PoolFactory::new();
        let r = f.create_pool(addr(1), addr(1), FeeTier::Standard, addr(99), 10_000, 1);
        // FIX: was AmmError::ZeroAmount — now proper IdenticalTokens
        assert!(
            matches!(r, Err(AmmError::IdenticalTokens)),
            "identical tokens must return IdenticalTokens, got {:?}", r
        );
    }

    #[test]
    fn insufficient_fee_rejected_with_correct_error() {
        let mut f = PoolFactory::new();
        let fee = f.fee_registry.pool_creation_fee();
        let r = f.create_pool(addr(1), addr(2), FeeTier::Standard, addr(99), fee - 1, 1);
        // FIX: was AmmError::Overflow — now proper InsufficientCreationFee
        assert!(
            matches!(r, Err(AmmError::InsufficientCreationFee { have: _, need: _ })),
            "insufficient fee must return InsufficientCreationFee, got {:?}", r
        );
        // Verify the error carries correct diagnostic values
        if let Err(AmmError::InsufficientCreationFee { have, need }) = r {
            assert_eq!(need, fee);
            assert_eq!(have, fee - 1);
        }
    }

    #[test]
    fn pool_address_is_deterministic() {
        let pair_id = PairId::new(addr(10), addr(20));
        let a1 = derive_pool_address(&pair_id, FeeTier::Standard);
        let a2 = derive_pool_address(&pair_id, FeeTier::Standard);
        assert_eq!(a1, a2);
    }

    #[test]
    fn different_fee_tiers_give_different_addresses() {
        let pair_id = PairId::new(addr(10), addr(20));
        let a1 = derive_pool_address(&pair_id, FeeTier::Lowest);
        let a2 = derive_pool_address(&pair_id, FeeTier::Standard);
        assert_ne!(a1, a2);
    }
}
