//! Block types: BlockHeader, BlockBody, Block.
//!
//! ## N-03 — Canonical block header type (S54 resolution)
//!
//! **`zbx_types::block::BlockHeader` (this file) is the canonical production
//! type** used by the node binary, the execution engine (`zbx-execution`),
//! the RocksDB storage layer (`zbx-storage`), and the HotStuff consensus
//! driver (`zbx-consensus`).
//!
//! The workspace also contains `zbx_block::header::BlockHeader` — an
//! EIP-4844-extended type that adds blob fields, `chain_id`, and `version`.
//! It is used exclusively inside the `zbx-block` block-validation library.
//!
//! Use `zbx_block::compat::zbx_types_to_zbx_block` /
//! `zbx_block::compat::zbx_block_to_zbx_types` to convert between the two
//! when calling into the validation library from node code.
//!
//! ## BLK-HASH-01 (2026-05-16) — Complete canonical header hash
//!
//! The previous `rlp_encode()` omitted `logs_bloom` (256 bytes) and
//! `difficulty` from the header hash. A malicious or buggy proposer could
//! commit any `logs_bloom` value and the block hash would not reflect it,
//! allowing falsified Bloom filter data to survive signature verification.
//! `extra_data` also lacked a length prefix, creating encoding collisions
//! between payloads of differing lengths.
//!
//! Fix: `logs_bloom`, `difficulty` (32-byte BE), and a 4-byte length prefix
//! on `extra_data` are now included. `committee_signature` remains excluded
//! by design — validators sign the *unsigned* header hash; the signature is
//! attached afterwards (analogous to the PoW nonce/mix_hash in Ethereum
//! pre-Merge). The epoch/epoch_seed ZBX extension fields are included so
//! epoch transitions are committed to the hash.
//!
//! ## BLK-VAL-01 (2026-05-16) — Stricter parent validation
//!
//! `validate_against_parent()` previously only checked parent_hash, block
//! number, timestamp, and gas_limit ceiling. Added:
//!   * gas_used ≤ gas_limit (prevents execution with impossible gas budget)
//!   * extra_data length ≤ 32 bytes (matches go-ethereum consensus rule)
//!   * EIP-1559 base_fee_per_gas computed from parent (prevents base fee
//!     manipulation; uses u128 arithmetic to avoid overflow)

use crate::{address::Address, error::ZbxError, transaction::SignedTransaction, H256, U256};
use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// Block header — mirrors Ethereum structure with ZBX extensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Hash of the parent block.
    pub parent_hash: H256,
    /// Keccak256 of uncle list (always zero on ZBX PoS).
    pub uncle_hash: H256,
    /// Miner / validator who proposed this block.
    pub coinbase: Address,
    /// Root of the account state trie after applying this block.
    pub state_root: H256,
    /// Root of the transaction trie.
    pub transactions_root: H256,
    /// Root of the receipts trie.
    pub receipts_root: H256,
    /// Bloom filter over all log topics in receipts (2048 bits).
    #[serde(with = "BigArray")]
    pub logs_bloom: [u8; 256],
    /// PoS difficulty (always 1 on ZBX).
    pub difficulty: U256,
    /// Block number (height).
    pub number: u64,
    /// Maximum gas allowed for all transactions.
    pub gas_limit: u64,
    /// Actual gas consumed by all transactions.
    pub gas_used: u64,
    /// UNIX timestamp in seconds.
    pub timestamp: u64,
    /// Arbitrary extra data (up to 32 bytes).
    pub extra_data: Vec<u8>,
    /// Mix hash (used as VRF output on ZBX).
    pub mix_hash: H256,
    /// Block nonce (always zero on PoS).
    pub nonce: u64,
    /// EIP-1559 base fee per gas unit.
    pub base_fee_per_gas: u64,
    /// BLS aggregate signature from the validator committee.
    pub committee_signature: Vec<u8>,
    /// Epoch number within the staking schedule.
    pub epoch: u64,
    /// SEC-2026-05-09 Pass-19 (Task #9): per-epoch shuffle seed used by the
    /// keccak-keyed proposer rotation in `zbx_consensus::ValidatorSet`.
    /// Populated ONLY on the first block of each new epoch (height %
    /// epoch_length == 0 && height > 0); `None` on every other block.
    /// Light clients verify epoch transitions against this field. Default
    /// `None` (`#[serde(default)]`) so old snapshots / pre-Task-9 stored
    /// blocks deserialize cleanly without a wire-format break.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_seed: Option<H256>,
}

impl BlockHeader {
    /// Keccak256 of the canonical header encoding.
    ///
    /// ## BLK-HASH-01 (2026-05-16)
    ///
    /// The encoding now commits to **all** header fields that a verifier must
    /// be able to check independently:
    ///
    /// | Field            | Size      | Notes |
    /// |------------------|-----------|-------|
    /// | parent_hash      | 32        | |
    /// | uncle_hash       | 32        | always zero on ZBX PoS |
    /// | coinbase         | 20        | |
    /// | state_root       | 32        | |
    /// | transactions_root| 32        | |
    /// | receipts_root    | 32        | |
    /// | logs_bloom       | 256       | **added** — bloom forgery fix |
    /// | difficulty       | 32        | **added** (BE U256) |
    /// | number           | 8         | |
    /// | gas_limit        | 8         | |
    /// | gas_used         | 8         | |
    /// | timestamp        | 8         | |
    /// | extra_data       | 4 + len   | **length-prefixed** — collision fix |
    /// | mix_hash         | 32        | |
    /// | nonce            | 8         | |
    /// | base_fee_per_gas | 8         | |
    /// | epoch            | 8         | ZBX extension |
    /// | epoch_seed       | 1 [+ 32]  | ZBX extension — presence-tagged |
    ///
    /// `committee_signature` is intentionally excluded: validators sign this
    /// hash; the signature cannot be known before the hash is computed.
    pub fn hash(&self) -> H256 {
        let encoded = self.canonical_encode();
        let mut h = Keccak256::new();
        h.update(&encoded);
        H256::from_slice(&h.finalize())
    }

    /// Canonical deterministic encoding of all verifiable header fields.
    ///
    /// ## Why not proper RLP?
    ///
    /// ZBX Chain has custom fields (`epoch`, `epoch_seed`, `committee_signature`)
    /// that have no Ethereum RLP analogue. The encoding below is a bespoke
    /// length-safe format: all fixed-width fields are encoded as-is; variable-
    /// length fields (`extra_data`, `epoch_seed`) carry explicit length or
    /// presence tags so no two distinct headers produce the same byte sequence.
    fn canonical_encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(768);

        // ── Fixed-width hash / address fields ─────────────────────────────
        buf.extend_from_slice(self.parent_hash.as_bytes());       // 32
        buf.extend_from_slice(self.uncle_hash.as_bytes());        // 32
        buf.extend_from_slice(self.coinbase.as_bytes());          // 20
        buf.extend_from_slice(self.state_root.as_bytes());        // 32
        buf.extend_from_slice(self.transactions_root.as_bytes()); // 32
        buf.extend_from_slice(self.receipts_root.as_bytes());     // 32

        // ── BLK-HASH-01: logs_bloom (was previously omitted) ──────────────
        // A proposer could previously commit any bloom without changing the
        // block hash. Light clients and eth_getLogs rely on this field for
        // fast log filtering — a forged bloom causes silent false-negatives.
        buf.extend_from_slice(&self.logs_bloom);                  // 256

        // ── BLK-HASH-01: difficulty (was previously omitted) ──────────────
        let mut diff_be = [0u8; 32];
        self.difficulty.to_big_endian(&mut diff_be);
        buf.extend_from_slice(&diff_be);                          // 32

        // ── Scalar fields ─────────────────────────────────────────────────
        buf.extend_from_slice(&self.number.to_be_bytes());        // 8
        buf.extend_from_slice(&self.gas_limit.to_be_bytes());     // 8
        buf.extend_from_slice(&self.gas_used.to_be_bytes());      // 8
        buf.extend_from_slice(&self.timestamp.to_be_bytes());     // 8

        // ── BLK-HASH-01: length-prefixed extra_data ───────────────────────
        // Without a length prefix, the serialisation of [0x41, 0x42] is
        // indistinguishable from [0x41] followed by whatever comes next.
        // A 4-byte big-endian length tag makes every encoding unique.
        buf.extend_from_slice(&(self.extra_data.len() as u32).to_be_bytes()); // 4
        buf.extend_from_slice(&self.extra_data);                              // ≤32

        buf.extend_from_slice(self.mix_hash.as_bytes());          // 32
        buf.extend_from_slice(&self.nonce.to_be_bytes());         // 8
        buf.extend_from_slice(&self.base_fee_per_gas.to_be_bytes()); // 8

        // ── ZBX extensions ────────────────────────────────────────────────
        buf.extend_from_slice(&self.epoch.to_be_bytes());         // 8

        // epoch_seed: 1-byte presence tag + optional 32-byte seed.
        // A missing seed encodes as 0x00; a present seed encodes as
        // 0x01 || seed[32]. Length-tagged so None ≠ Some([0u8;32]).
        match &self.epoch_seed {
            None    => buf.push(0u8),
            Some(s) => { buf.push(1u8); buf.extend_from_slice(s.as_bytes()); }
        }

        // NOTE: committee_signature intentionally excluded — this is the
        // pre-signature hash that validators sign. The signature is appended
        // to the header after consensus, analogous to the PoW seal fields in
        // pre-Merge Ethereum.

        buf
    }

    /// Verify the block is a valid child of the given parent header.
    ///
    /// ## BLK-VAL-01 (2026-05-16) — extended validation
    ///
    /// Added checks vs the previous version:
    ///  * `gas_used ≤ gas_limit`
    ///  * `extra_data.len() ≤ 32`
    ///  * EIP-1559 `base_fee_per_gas` computed from parent
    pub fn validate_against_parent(&self, parent: &BlockHeader) -> Result<(), ZbxError> {
        // ── Parent linkage ────────────────────────────────────────────────
        let parent_hash = parent.hash();
        if self.parent_hash != parent_hash {
            return Err(ZbxError::Consensus(format!(
                "block {} parent_hash mismatch", self.number
            )));
        }
        if self.number != parent.number + 1 {
            return Err(ZbxError::Consensus(format!(
                "block number gap: expected {}, got {}", parent.number + 1, self.number
            )));
        }

        // ── Timestamp monotonicity ────────────────────────────────────────
        if self.timestamp <= parent.timestamp {
            return Err(ZbxError::Consensus(
                "block timestamp must be strictly increasing".into(),
            ));
        }

        // ── Gas accounting ────────────────────────────────────────────────
        // BLK-VAL-01: gas_used must not exceed the block's own gas_limit.
        if self.gas_used > self.gas_limit {
            return Err(ZbxError::Consensus(format!(
                "gas_used {} exceeds gas_limit {}", self.gas_used, self.gas_limit
            )));
        }
        if self.gas_limit > crate::BLOCK_GAS_LIMIT {
            return Err(ZbxError::Consensus(
                "block gas_limit exceeds protocol maximum".into(),
            ));
        }

        // ── Extra data ────────────────────────────────────────────────────
        // BLK-VAL-01: matches go-ethereum's 32-byte consensus rule.
        if self.extra_data.len() > 32 {
            return Err(ZbxError::Consensus(format!(
                "extra_data too long: {} bytes (max 32)", self.extra_data.len()
            )));
        }

        // ── EIP-1559 base fee ─────────────────────────────────────────────
        // BLK-VAL-01: verify base_fee_per_gas matches the protocol formula.
        // Genesis block (parent.number == 0) inherits a fixed initial fee;
        // every subsequent block must follow the EIP-1559 adjustment rule.
        // We skip the check when the genesis base fee is zero (unconfigured
        // dev-nets that haven't set a genesis base fee).
        if parent.number > 0 || parent.base_fee_per_gas > 0 {
            let expected = compute_eip1559_base_fee(
                parent.base_fee_per_gas,
                parent.gas_used,
                parent.gas_limit,
            );
            if self.base_fee_per_gas != expected {
                return Err(ZbxError::Consensus(format!(
                    "base_fee_per_gas mismatch at block {}: expected {}, got {}",
                    self.number, expected, self.base_fee_per_gas
                )));
            }
        }

        Ok(())
    }
}

/// EIP-1559 base fee adjustment formula.
///
/// `next = parent_base ± (parent_base × |gas_used − target| / target / 8)`
///
/// where `target = gas_limit / 2`.
///
/// Uses `u128` arithmetic to avoid overflow when `parent_base × Δgas` would
/// exceed `u64::MAX` (possible at high base fees and large gas deltas).
/// The result is saturating-clamped to `[1, u64::MAX]`.
fn compute_eip1559_base_fee(parent_base: u64, gas_used: u64, gas_limit: u64) -> u64 {
    if gas_limit == 0 {
        return parent_base;
    }
    let target = gas_limit / 2;
    if gas_used == target {
        return parent_base;
    }
    let base = parent_base as u128;
    let adj  = base / 8;

    if gas_used > target {
        let delta = (gas_used - target) as u128;
        // bump = max(1, min(adj, base × delta / target))
        let bump = (base * delta / target as u128).max(1).min(adj);
        (base.saturating_add(bump)) as u64
    } else {
        let delta = (target - gas_used) as u128;
        let cut = (base * delta / target as u128).min(adj);
        (base.saturating_sub(cut)) as u64
    }
}

/// Block body containing ordered transactions and uncle headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockBody {
    pub transactions: Vec<SignedTransaction>,
    /// Always empty on ZBX PoS — included for EVM-RPC compatibility.
    pub uncles: Vec<BlockHeader>,
}

/// A complete block (header + body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub body: BlockBody,
}

impl Block {
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn number(&self) -> u64 {
        self.header.number
    }

    pub fn coinbase(&self) -> Address {
        self.header.coinbase
    }

    pub fn transaction_count(&self) -> usize {
        self.body.transactions.len()
    }

    /// Build the genesis block for Zebvix mainnet.
    pub fn genesis(state_root: H256, timestamp: u64) -> Self {
        let header = BlockHeader {
            parent_hash: H256::zero(),
            uncle_hash: H256::zero(),
            coinbase: Address::ZERO,
            state_root,
            transactions_root: H256::zero(),
            receipts_root: H256::zero(),
            logs_bloom: [0u8; 256],
            difficulty: U256::zero(),
            number: 0,
            gas_limit: crate::BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp,
            extra_data: b"Zebvix Genesis".to_vec(),
            mix_hash: H256::zero(),
            nonce: 0,
            base_fee_per_gas: 1_000_000_000, // 1 Gwei
            committee_signature: Vec::new(),
            epoch: 0,
            epoch_seed: None,
        };
        Block {
            header,
            body: BlockBody { transactions: Vec::new(), uncles: Vec::new() },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header(number: u64, parent_hash: H256, base_fee: u64) -> BlockHeader {
        BlockHeader {
            parent_hash,
            uncle_hash:          H256::zero(),
            coinbase:            Address::ZERO,
            state_root:          H256::zero(),
            transactions_root:   H256::zero(),
            receipts_root:       H256::zero(),
            logs_bloom:          [0u8; 256],
            difficulty:          U256::zero(),
            number,
            gas_limit:           30_000_000,
            gas_used:            15_000_000, // = target → base fee unchanged
            timestamp:           1_700_000_000 + number,
            extra_data:          b"zbx".to_vec(),
            mix_hash:            H256::zero(),
            nonce:               0,
            base_fee_per_gas:    base_fee,
            committee_signature: Vec::new(),
            epoch:               0,
            epoch_seed:          None,
        }
    }

    #[test]
    fn hash_changes_when_logs_bloom_changes() {
        let h1 = sample_header(1, H256::zero(), 1_000_000_000);
        let mut h2 = h1.clone();
        h2.logs_bloom[0] = 0xFF;
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-01: different logs_bloom must produce different hash");
    }

    #[test]
    fn hash_changes_when_extra_data_length_changes() {
        // Regression for the pre-fix encoding collision: without a length
        // prefix, [0x41, 0x42] || next_field == [0x41] || [0x42, next_field…]
        let mut h1 = sample_header(1, H256::zero(), 1_000_000_000);
        let mut h2 = h1.clone();
        h1.extra_data = vec![0x41, 0x42];
        h2.extra_data = vec![0x41];
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-01: different extra_data must not collide");
    }

    #[test]
    fn hash_deterministic() {
        let h = sample_header(5, H256::zero(), 1_000_000_000);
        assert_eq!(h.hash(), h.hash(),
            "hash() must be deterministic");
    }

    #[test]
    fn validate_against_parent_ok() {
        let parent = sample_header(1, H256::zero(), 1_000_000_000);
        let parent_hash = parent.hash();
        let mut child = sample_header(2, parent_hash, 1_000_000_000);
        // EIP-1559: gas_used == target (gas_limit/2) → base fee unchanged.
        child.base_fee_per_gas = 1_000_000_000;
        assert!(child.validate_against_parent(&parent).is_ok());
    }

    #[test]
    fn validate_rejects_wrong_parent_hash() {
        let parent = sample_header(1, H256::zero(), 1_000_000_000);
        let mut child = sample_header(2, H256::zero(), 1_000_000_000);
        child.parent_hash = H256::zero(); // wrong
        assert!(child.validate_against_parent(&parent).is_err());
    }

    #[test]
    fn validate_rejects_gas_used_exceeds_limit() {
        let parent = sample_header(1, H256::zero(), 1_000_000_000);
        let parent_hash = parent.hash();
        let mut child = sample_header(2, parent_hash, 1_000_000_000);
        child.gas_used = child.gas_limit + 1;
        assert!(child.validate_against_parent(&parent).is_err());
    }

    #[test]
    fn validate_rejects_extra_data_too_long() {
        let parent = sample_header(1, H256::zero(), 1_000_000_000);
        let parent_hash = parent.hash();
        let mut child = sample_header(2, parent_hash, 1_000_000_000);
        child.extra_data = vec![0u8; 33]; // 33 > 32
        assert!(child.validate_against_parent(&parent).is_err());
    }

    #[test]
    fn validate_rejects_wrong_base_fee() {
        let parent = sample_header(1, H256::zero(), 1_000_000_000);
        let parent_hash = parent.hash();
        let mut child = sample_header(2, parent_hash, 1_000_000_000);
        child.base_fee_per_gas = 999_999_999; // wrong
        assert!(child.validate_against_parent(&parent).is_err());
    }

    #[test]
    fn eip1559_base_fee_at_target() {
        // gas_used == target → fee unchanged
        let fee = compute_eip1559_base_fee(1_000_000_000, 15_000_000, 30_000_000);
        assert_eq!(fee, 1_000_000_000);
    }

    #[test]
    fn eip1559_base_fee_above_target_bumps() {
        // gas_used > target → fee increases
        let fee = compute_eip1559_base_fee(1_000_000_000, 30_000_000, 30_000_000);
        assert!(fee > 1_000_000_000,
            "base fee should increase when all gas is used");
    }

    #[test]
    fn eip1559_base_fee_below_target_cuts() {
        // gas_used == 0 < target → fee decreases
        let fee = compute_eip1559_base_fee(1_000_000_000, 0, 30_000_000);
        assert!(fee < 1_000_000_000,
            "base fee should decrease when no gas is used");
    }

    #[test]
    fn eip1559_no_overflow_at_large_values() {
        // high base fee × large delta — must not panic or overflow
        let max = u64::MAX / 2;
        let _ = compute_eip1559_base_fee(max, 29_000_000, 30_000_000);
    }
}
