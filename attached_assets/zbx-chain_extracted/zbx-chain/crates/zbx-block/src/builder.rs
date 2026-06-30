//! Block builder — assembles block from pending txs.
//!
//! ## BLK-TS-01 fix (2026-05-05) — monotonic timestamp
//!
//! `build()` previously used `SystemTime::now()` as-is.  NTP step adjustments
//! can push the system clock backwards, producing a timestamp equal to or less
//! than `parent.timestamp`.  `validate_header()` enforces strict monotonicity
//! (`block.timestamp > parent.timestamp`), so such a block would be immediately
//! rejected by any validator.
//!
//! Fix: the produced timestamp is `max(SystemTime::now(), parent.timestamp + 1)`
//! so the block is always strictly monotone regardless of clock skew.
//!
//! ## BLK-BUILD-01 fix (2026-05-16) — integer overflow in `compute_next_base_fee`
//!
//! The previous implementation computed `base * (gas_used - target) / target`
//! entirely in `u64`. For a high base fee (≥ ~1.8 × 10^10 wei, ~18 Gwei) and a
//! fully-used block (`gas_used = gas_limit`), the intermediate product
//! `base * delta` wraps around silently, producing an arbitrarily wrong fee.
//!
//! Fix: intermediate arithmetic is promoted to `u128` before any
//! multiplication.  The final clamped result is cast back to `u64`; for any
//! in-range EIP-1559 base fee the value fits comfortably in `u64`.
//!
//! Additionally guarded the `target == 0` edge case (would previously divide
//! by zero if `gas_limit == 0`) by returning the parent fee unchanged.

use std::time::{SystemTime, UNIX_EPOCH};
use zbx_primitives::{H256, Address, U256};
use crate::header::{BlockHeader, EMPTY_UNCLE_HASH};
use crate::body::BlockBody;

pub struct BlockBuilder {
    pub parent:       BlockHeader,
    pub beneficiary:  Address,
    pub extra_data:   Vec<u8>,
    pub transactions: Vec<zbx_tx::Transaction>,
    pub gas_limit:    u64,
    pub base_fee:     U256,
    pub chain_id:     u64,
}

impl BlockBuilder {
    pub fn new(parent: &BlockHeader, beneficiary: Address, chain_id: u64) -> Self {
        Self {
            parent: parent.clone(), beneficiary,
            extra_data: b"zbx".to_vec(),
            transactions: Vec::new(),
            gas_limit: parent.gas_limit,
            base_fee: compute_next_base_fee(parent),
            chain_id,
        }
    }

    pub fn add_tx(&mut self, tx: zbx_tx::Transaction, gas_so_far: u64) -> bool {
        if gas_so_far + tx.gas_limit > self.gas_limit { return false; }
        self.transactions.push(tx);
        true
    }

    pub fn build(
        self,
        state_root:    H256,
        receipts_root: H256,
        gas_used:      u64,
        logs_bloom:    [u8; 256],
    ) -> (BlockHeader, BlockBody) {
        let body    = BlockBody::new(self.transactions.clone());
        let tx_root = body.compute_tx_root();
        // BLK-TS-01: guarantee strict monotonicity regardless of NTP clock skew.
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let now = wall.max(self.parent.timestamp + 1);
        let header = BlockHeader {
            parent_hash:      self.parent.hash(),
            ommers_hash:      EMPTY_UNCLE_HASH,
            beneficiary:      self.beneficiary,
            state_root,
            transactions_root: tx_root,
            receipts_root,
            logs_bloom,
            difficulty:       U256::ZERO,
            number:           self.parent.number + 1,
            gas_limit:        self.gas_limit,
            gas_used,
            timestamp:        now,
            extra_data:       self.extra_data,
            prev_randao:      H256::ZERO,
            base_fee_per_gas: self.base_fee,
            excess_blob_gas:  Some(0),
            blob_gas_used:    Some(0),
            nonce:            0,
            chain_id:         self.chain_id,
            version:          1,
        };
        (header, body)
    }
}

/// EIP-1559 base fee adjustment: `next = parent ± (parent × |Δgas| / target / 8)`.
///
/// ## BLK-BUILD-01 (2026-05-16) — overflow-safe u128 arithmetic
///
/// The previous implementation performed `base × delta` in `u64`. At high
/// base fees (≥ ~18 Gwei) with large gas deltas the product wraps silently.
/// The intermediate multiplication is now done in `u128` (max value ~3.4 × 10^38)
/// which is far beyond any realistic base fee × gas delta combination.
///
/// The `target == 0` guard prevents a divide-by-zero when `gas_limit == 0`.
pub fn compute_next_base_fee(parent: &BlockHeader) -> U256 {
    let base   = parent.base_fee_per_gas.as_u64();
    let target = parent.gas_limit / 2;

    // gas_used == target: fee unchanged.
    // target == 0: degenerate block (gas_limit == 0) — leave fee alone.
    if parent.gas_used == target || target == 0 {
        return parent.base_fee_per_gas;
    }

    // BLK-BUILD-01: promote to u128 before multiplication.
    let base128 = base as u128;
    let adj     = base128 / 8; // max increase/decrease per block

    let next = if parent.gas_used > target {
        let delta = (parent.gas_used - target) as u128;
        // bump = clamp(base × delta / target, 1, adj)
        let bump = (base128 * delta / target as u128).max(1).min(adj);
        (base as u128).saturating_add(bump) as u64
    } else {
        let delta = (target - parent.gas_used) as u128;
        let cut = (base128 * delta / target as u128).min(adj);
        (base as u128).saturating_sub(cut) as u64
    };

    U256::from_u64(next)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::EMPTY_TRIE_HASH;

    fn parent_at(gas_limit: u64, gas_used: u64, base: u64) -> BlockHeader {
        BlockHeader {
            parent_hash:      H256::ZERO,
            ommers_hash:      EMPTY_UNCLE_HASH,
            beneficiary:      Address::ZERO,
            state_root:       H256::ZERO,
            transactions_root: EMPTY_TRIE_HASH,
            receipts_root:    EMPTY_TRIE_HASH,
            logs_bloom:       [0u8; 256],
            difficulty:       U256::ZERO,
            number:           1,
            gas_limit,
            gas_used,
            timestamp:        1_700_000_000,
            extra_data:       Vec::new(),
            prev_randao:      H256::ZERO,
            base_fee_per_gas: U256::from_u64(base),
            excess_blob_gas:  Some(0),
            blob_gas_used:    Some(0),
            nonce:            0,
            chain_id:         8989,
            version:          1,
        }
    }

    #[test]
    fn base_fee_unchanged_at_target() {
        let p = parent_at(30_000_000, 15_000_000, 1_000_000_000);
        assert_eq!(
            compute_next_base_fee(&p),
            U256::from_u64(1_000_000_000),
            "fee must be unchanged when gas_used == target"
        );
    }

    #[test]
    fn base_fee_increases_above_target() {
        let p = parent_at(30_000_000, 30_000_000, 1_000_000_000);
        let next = compute_next_base_fee(&p).as_u64();
        assert!(next > 1_000_000_000, "fee must increase when fully utilised");
        // EIP-1559 max increase is +12.5 % (1/8)
        assert!(next <= 1_000_000_000 + 1_000_000_000 / 8);
    }

    #[test]
    fn base_fee_decreases_below_target() {
        let p = parent_at(30_000_000, 0, 1_000_000_000);
        let next = compute_next_base_fee(&p).as_u64();
        assert!(next < 1_000_000_000, "fee must decrease when no gas used");
        // EIP-1559 max decrease is -12.5 %
        assert!(next >= 1_000_000_000 - 1_000_000_000 / 8);
    }

    #[test]
    fn base_fee_no_overflow_high_values() {
        // BLK-BUILD-01: previously wrapped silently at high base fees.
        // u64::MAX / 2 ≈ 9.2 × 10^18 wei — far above any real base fee but
        // must not panic or produce a nonsensical result.
        let big = u64::MAX / 2;
        let p = parent_at(30_000_000, 29_000_000, big);
        let next = compute_next_base_fee(&p).as_u64();
        assert!(next >= big, "fee must increase when gas_used > target");
    }

    #[test]
    fn base_fee_degenerate_zero_gas_limit() {
        let p = parent_at(0, 0, 1_000_000_000);
        assert_eq!(
            compute_next_base_fee(&p),
            U256::from_u64(1_000_000_000),
            "degenerate gas_limit == 0 must not divide-by-zero"
        );
    }
}
