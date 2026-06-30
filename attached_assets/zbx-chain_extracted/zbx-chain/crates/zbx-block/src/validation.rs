//! Block validation for ZBX Chain.
//!
//! ## BLK-LB-01 fix (2026-05-05) — logs_bloom validation
//!
//! `validate_body()` previously only checked `transactions_root` and
//! `receipts_root`.  A malicious or buggy producer could commit an incorrect
//! `logs_bloom` in the header and the block would pass validation.  Light
//! clients and `eth_getLogs` callers rely on the block-level Bloom filter for
//! fast log filtering; a wrong bloom causes false negatives (missed events) or
//! excessive false positives.
//!
//! Fix: `validate_body()` now accepts the executor-computed `block_bloom`
//! (aggregated OR of all per-receipt blooms) and compares it to
//! `header.logs_bloom` byte-by-byte.  Any mismatch returns `LogsBloomMismatch`.
//!
//! ## BLK-VAL-02 fix (2026-05-16) — EIP-1559 gas_limit drift bound
//!
//! The previous `validate_header()` did not check that the block's
//! `gas_limit` stays within the EIP-1559 adjustment window relative to the
//! parent.  An unchecked gas_limit allowed a malicious proposer to spike the
//! block gas capacity by an arbitrary amount in a single block, enabling
//! oversized blocks that exceed the protocol's execution budget.
//!
//! Fix: `validate_header()` now enforces
//!   `|h.gas_limit − parent.gas_limit| ≤ parent.gas_limit / 1024`
//! for all non-genesis blocks.  This matches go-ethereum's `VerifyGaslimit`
//! check in `consensus/ethash/consensus.go`.

use zbx_primitives::{H256, U256};
use crate::header::BlockHeader;
use crate::body::BlockBody;

#[derive(Debug, thiserror::Error)]
pub enum BlockValidationError {
    #[error("Parent hash mismatch")]
    ParentHashMismatch,

    #[error("Block number: got {got}, want {want}")]
    InvalidBlockNumber { got: u64, want: u64 },

    #[error("Timestamp not increasing: {block} <= {parent}")]
    InvalidTimestamp { block: u64, parent: u64 },

    #[error("Gas used {used} > limit {limit}")]
    GasExceeded { used: u64, limit: u64 },

    #[error("Base fee mismatch: got {got}, want {want}")]
    BaseFeeMismatch { got: U256, want: U256 },

    #[error("Extra data too long: {0}")]
    ExtraDataTooLong(usize),

    #[error("Chain ID mismatch: got {got}, want {want}")]
    ChainIdMismatch { got: u64, want: u64 },

    #[error("Tx root mismatch")]
    TxRootMismatch,

    #[error("Receipts root mismatch")]
    ReceiptsRootMismatch,

    #[error("Logs bloom mismatch")]
    LogsBloomMismatch,

    /// BLK-VAL-02: gas_limit drifted beyond ±parent/1024 in one step.
    #[error("Gas limit drift too large: got {got}, parent {parent}, max allowed change {max_change}")]
    GasLimitDrift { got: u64, parent: u64, max_change: u64 },
}

/// Validate a block header against its parent.
///
/// Checks (in order):
///  1. `parent_hash` matches `parent.hash()`
///  2. `number` is exactly `parent.number + 1`
///  3. `timestamp > parent.timestamp`
///  4. `gas_used ≤ gas_limit`
///  5. `extra_data.len() ≤ 32`
///  6. `chain_id` matches `chain_id` parameter
///  7. `base_fee_per_gas` matches EIP-1559 formula applied to parent
///  8. **BLK-VAL-02** `|gas_limit − parent.gas_limit| ≤ parent.gas_limit / 1024`
///     (skipped for the genesis block where `parent.number == 0`)
pub fn validate_header(
    h:        &BlockHeader,
    parent:   &BlockHeader,
    chain_id: u64,
) -> Result<(), BlockValidationError> {
    // ── 1. Parent linkage ─────────────────────────────────────────────────
    if h.parent_hash != parent.hash() {
        return Err(BlockValidationError::ParentHashMismatch);
    }

    // ── 2. Block number ───────────────────────────────────────────────────
    if h.number != parent.number + 1 {
        return Err(BlockValidationError::InvalidBlockNumber {
            got: h.number,
            want: parent.number + 1,
        });
    }

    // ── 3. Timestamp ──────────────────────────────────────────────────────
    if h.timestamp <= parent.timestamp {
        return Err(BlockValidationError::InvalidTimestamp {
            block: h.timestamp,
            parent: parent.timestamp,
        });
    }

    // ── 4. Gas accounting ─────────────────────────────────────────────────
    if h.gas_used > h.gas_limit {
        return Err(BlockValidationError::GasExceeded {
            used: h.gas_used,
            limit: h.gas_limit,
        });
    }

    // ── 5. Extra data ─────────────────────────────────────────────────────
    if h.extra_data.len() > 32 {
        return Err(BlockValidationError::ExtraDataTooLong(h.extra_data.len()));
    }

    // ── 6. Chain ID ───────────────────────────────────────────────────────
    if h.chain_id != chain_id {
        return Err(BlockValidationError::ChainIdMismatch {
            got: h.chain_id,
            want: chain_id,
        });
    }

    // ── 7. EIP-1559 base fee ──────────────────────────────────────────────
    let exp = crate::builder::compute_next_base_fee(parent);
    if h.base_fee_per_gas != exp {
        return Err(BlockValidationError::BaseFeeMismatch {
            got:  h.base_fee_per_gas,
            want: exp,
        });
    }

    // ── 8. BLK-VAL-02: EIP-1559 gas_limit drift bound ────────────────────
    // Every non-genesis block must keep its gas_limit within ±parent/1024
    // of the parent's gas_limit. This prevents an adversarial proposer from
    // doubling the gas capacity in a single slot (e.g. to enable a block
    // large enough to exhaust the executor's budget).
    //
    // Ethereum reference: go-ethereum `consensus/misc.VerifyGaslimit`.
    if parent.number > 0 || parent.gas_limit > 0 {
        let max_change = parent.gas_limit / 1024;
        let drift = h.gas_limit.abs_diff(parent.gas_limit);
        if drift > max_change {
            return Err(BlockValidationError::GasLimitDrift {
                got:        h.gas_limit,
                parent:     parent.gas_limit,
                max_change,
            });
        }
    }

    Ok(())
}

/// Validate the block body against committed header roots.
///
/// * `h`             — the block header (contains committed roots).
/// * `body`          — the block body (transactions to re-derive tx root from).
/// * `receipts_root` — the receipts root computed by the executor.
/// * `block_bloom`   — the 256-byte block-level Bloom filter computed by the
///                     executor (aggregate OR of all per-receipt blooms).
///                     Pass `[0u8; 256]` for empty blocks (no receipts).
///
/// ## BLK-LB-01 (2026-05-05)
/// `block_bloom` is now compared against `h.logs_bloom`.  A mismatch returns
/// `LogsBloomMismatch` which callers should treat as a protocol violation.
pub fn validate_body(
    h:             &BlockHeader,
    body:          &BlockBody,
    receipts_root: H256,
    block_bloom:   &[u8; 256],
) -> Result<(), BlockValidationError> {
    if body.compute_tx_root() != h.transactions_root {
        return Err(BlockValidationError::TxRootMismatch);
    }
    if receipts_root != h.receipts_root {
        return Err(BlockValidationError::ReceiptsRootMismatch);
    }
    // BLK-LB-01: verify the committed logs_bloom matches execution output.
    if block_bloom != &h.logs_bloom {
        return Err(BlockValidationError::LogsBloomMismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{EMPTY_UNCLE_HASH, EMPTY_TRIE_HASH};
    use crate::body::BlockBody;
    use zbx_primitives::Address;

    const CHAIN_ID: u64 = 8989;

    fn base_header(number: u64) -> BlockHeader {
        BlockHeader {
            parent_hash:      zbx_primitives::H256::ZERO,
            ommers_hash:      EMPTY_UNCLE_HASH,
            beneficiary:      Address::ZERO,
            state_root:       zbx_primitives::H256::ZERO,
            transactions_root: EMPTY_TRIE_HASH,
            receipts_root:    EMPTY_TRIE_HASH,
            logs_bloom:       [0u8; 256],
            difficulty:       U256::ZERO,
            number,
            gas_limit:        30_000_000,
            gas_used:         15_000_000, // at target → base fee unchanged
            timestamp:        1_700_000_000 + number,
            extra_data:       b"zbx".to_vec(),
            prev_randao:      zbx_primitives::H256::ZERO,
            base_fee_per_gas: U256::from_u64(1_000_000_000),
            excess_blob_gas:  Some(0),
            blob_gas_used:    Some(0),
            nonce:            0,
            chain_id:         CHAIN_ID,
            version:          1,
        }
    }

    fn child_of(parent: &BlockHeader) -> BlockHeader {
        let mut child = base_header(parent.number + 1);
        child.parent_hash      = parent.hash();
        child.timestamp        = parent.timestamp + 1;
        child.base_fee_per_gas = crate::builder::compute_next_base_fee(parent);
        child
    }

    #[test]
    fn valid_header_passes() {
        let parent = base_header(1);
        let child  = child_of(&parent);
        assert!(validate_header(&child, &parent, CHAIN_ID).is_ok());
    }

    #[test]
    fn rejects_wrong_parent_hash() {
        let parent = base_header(1);
        let mut child = child_of(&parent);
        child.parent_hash = zbx_primitives::H256::ZERO; // wrong
        assert!(matches!(
            validate_header(&child, &parent, CHAIN_ID),
            Err(BlockValidationError::ParentHashMismatch)
        ));
    }

    #[test]
    fn rejects_gas_used_exceeds_limit() {
        let parent = base_header(1);
        let mut child = child_of(&parent);
        child.gas_used = child.gas_limit + 1;
        assert!(matches!(
            validate_header(&child, &parent, CHAIN_ID),
            Err(BlockValidationError::GasExceeded { .. })
        ));
    }

    #[test]
    fn rejects_chain_id_mismatch() {
        let parent = base_header(1);
        let child  = child_of(&parent);
        assert!(matches!(
            validate_header(&child, &parent, 9999), // wrong chain_id
            Err(BlockValidationError::ChainIdMismatch { .. })
        ));
    }

    #[test]
    fn rejects_extra_data_too_long() {
        let parent = base_header(1);
        let mut child = child_of(&parent);
        child.extra_data = vec![0u8; 33]; // 33 > 32
        assert!(matches!(
            validate_header(&child, &parent, CHAIN_ID),
            Err(BlockValidationError::ExtraDataTooLong(33))
        ));
    }

    #[test]
    fn rejects_gas_limit_drift_too_large() {
        // BLK-VAL-02: spike gas_limit by 2× in one block
        let parent = base_header(1);
        let mut child = child_of(&parent);
        child.gas_limit = parent.gas_limit * 2; // far exceeds ±parent/1024
        // base_fee is now computed for original gas_limit, so we skip that
        // check by forcing the fee to the expected value.
        child.base_fee_per_gas = crate::builder::compute_next_base_fee(&parent);
        assert!(matches!(
            validate_header(&child, &parent, CHAIN_ID),
            Err(BlockValidationError::GasLimitDrift { .. })
        ));
    }

    #[test]
    fn allows_small_gas_limit_change() {
        // A change of exactly parent/1024 is within bounds.
        let parent = base_header(1);
        let mut child = child_of(&parent);
        let max_change = parent.gas_limit / 1024;
        child.gas_limit = parent.gas_limit + max_change;
        child.base_fee_per_gas = crate::builder::compute_next_base_fee(&parent);
        assert!(validate_header(&child, &parent, CHAIN_ID).is_ok());
    }

    #[test]
    fn validate_body_ok() {
        let parent = base_header(1);
        let mut child = child_of(&parent);
        let body = BlockBody::new(vec![]);
        child.transactions_root = body.compute_tx_root();
        let receipts_root = EMPTY_TRIE_HASH;
        child.receipts_root = receipts_root;
        let bloom = [0u8; 256];
        child.logs_bloom = bloom;
        assert!(validate_body(&child, &body, receipts_root, &bloom).is_ok());
    }

    #[test]
    fn validate_body_rejects_logs_bloom_mismatch() {
        let parent = base_header(1);
        let mut child = child_of(&parent);
        let body = BlockBody::new(vec![]);
        child.transactions_root = body.compute_tx_root();
        let receipts_root = EMPTY_TRIE_HASH;
        child.receipts_root = receipts_root;
        child.logs_bloom = [0u8; 256];
        let mut wrong_bloom = [0u8; 256];
        wrong_bloom[0] = 0xFF; // differs from header
        assert!(matches!(
            validate_body(&child, &body, receipts_root, &wrong_bloom),
            Err(BlockValidationError::LogsBloomMismatch)
        ));
    }
}
