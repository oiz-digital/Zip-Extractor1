//! Canonical trading pair addresses for ZBX chain native pools.
//!
//! # Native Token Addresses
//!
//! | Token | Address | Type |
//! |-------|---------|------|
//! | WZBX  | 0x0000...0001 | Wrapped native ZBX (precompile) |
//! | ZUSD  | 0x000...231D0001 | USD stablecoin (genesis contract) |
//!
//! # Canonical Pools
//!
//! | Pool | Fee | Rationale |
//! |------|-----|-----------|
//! | ZBX/ZUSD | 0.30% (Standard) | ZBX is volatile — standard fee |
//!
//! # Pool Contract Addresses
//!
//! Canonical pool contracts are pre-deployed at genesis.
//! Address scheme: `0xPool<N>` — deterministic, well-known.

use zbx_types::address::Address;
use crate::fee::FeeTier;
use crate::pair::PairId;

// ── Token addresses ───────────────────────────────────────────────────────────

/// Wrapped ZBX (WZBX) — precompile at 0x0000...0001.
/// ZBX is the native gas token; WZBX is its ERC-20 wrapper for AMM pools.
/// Unwrapping 1 WZBX → 1 ZBX (1:1 peg, always).
pub const WZBX_ADDR: [u8; 20] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01,
];

/// ZUSD — USD stablecoin genesis contract.
/// Address: 0x000000000000000000000000000000000000231D0001
/// (last 8 bytes encode the genesis contract index 0x231D0001)
pub const ZUSD_ADDR: [u8; 20] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x23, 0x1D, 0x00, 0x01,
];

// ── Pool contract addresses (pre-deployed at genesis) ─────────────────────────

/// ZBX/ZUSD pool contract address.
pub const POOL_ZBX_ZUSD_ADDR: [u8; 20] = [
    0xAA, 0xBB, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01,
];

// ── Canonical pair descriptors ────────────────────────────────────────────────

/// All information about a canonical pool at genesis.
#[derive(Debug, Clone)]
pub struct CanonicalPool {
    /// Human-readable name (e.g. "ZBX/ZUSD").
    pub name:             &'static str,
    /// Trading pair identifier (canonical ordering: smaller address first).
    pub pair_id:          PairId,
    /// Pool fee tier.
    pub fee_tier:         FeeTier,
    /// On-chain pool contract address.
    pub contract_address: [u8; 20],
}

/// Returns the single canonical pool defined at genesis.
///
/// # Pool
/// 1. **ZBX/ZUSD** — Standard 0.30% fee (ZBX is volatile)
pub fn canonical_pools() -> [CanonicalPool; 1] {
    [
        CanonicalPool {
            name:             "ZBX/ZUSD",
            pair_id:          PairId::new(wzbx(), zusd()),
            fee_tier:         FeeTier::Standard,
            contract_address: POOL_ZBX_ZUSD_ADDR,
        },
    ]
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Canonical `Address` wrappers for use in code.
pub fn wzbx() -> Address { Address(WZBX_ADDR) }
pub fn zusd() -> Address { Address(ZUSD_ADDR) }

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_canonical_pool_defined() {
        let pools = canonical_pools();
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "ZBX/ZUSD");
    }

    #[test]
    fn zbx_zusd_has_standard_fee() {
        let pools = canonical_pools();
        assert_eq!(pools[0].fee_tier, FeeTier::Standard);
    }

    #[test]
    fn pair_id_has_canonical_ordering() {
        let pools = canonical_pools();
        let pool = &pools[0];
        let a = pool.pair_id.token_a.as_bytes();
        let b = pool.pair_id.token_b.as_bytes();
        assert!(a < b, "Pool {} pair_id not in canonical order", pool.name);
    }

    #[test]
    fn token_addresses_are_distinct() {
        assert_ne!(WZBX_ADDR, ZUSD_ADDR);
    }
}
