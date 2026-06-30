//! ZBX Block header — EIP-4844 extended type used by the `zbx-block`
//! validation library.
//!
//! ## BLK-HASH-02 (2026-05-16) — Complete `rlp_encode()` for `zbx-block` header
//!
//! The previous `rlp_encode()` included only 10 of the 19 fields:
//! `parent_hash`, `state_root`, `transactions_root`, `receipts_root`,
//! `number`, `gas_limit`, `gas_used`, `timestamp`, `chain_id`,
//! `base_fee_per_gas`.
//!
//! **Omitted fields that are now included:**
//!
//! | Field           | Size      | Impact of omission |
//! |-----------------|-----------|-------------------|
//! | `ommers_hash`   | 32        | Different uncle lists hash identically |
//! | `beneficiary`   | 20        | Any validator can claim another's block |
//! | `logs_bloom`    | 256       | Bloom forgery (same as BLK-HASH-01) |
//! | `difficulty`    | 32        | Trivial to spoof on PoS |
//! | `extra_data`    | 4 + len   | Length-prefixed to prevent collisions |
//! | `prev_randao`   | 32        | VRF output excluded from hash |
//! | `nonce`         | 8         | |
//! | `excess_blob_gas`| 1 [+8]   | EIP-4844 blob accounting excluded |
//! | `blob_gas_used` | 1 [+8]    | EIP-4844 blob accounting excluded |
//! | `version`       | 1         | Protocol version excluded |
//!
//! The hash computed by this type is an *internal* hash used by
//! `zbx-block::validation::validate_header` — specifically for the
//! `h.parent_hash != parent.hash()` check. The canonical production hash is
//! computed by `zbx_types::block::BlockHeader::hash()`. They use different
//! field sets (this type has EIP-4844 blob fields; zbx-types has ZBX-specific
//! `committee_signature`/`epoch`/`epoch_seed`), so their hashes legitimately
//! differ. See `zbx_block::compat` for the conversion layer.

use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use sha3::{Keccak256, Digest};
use zbx_primitives::{H256, Address, U256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent_hash:         H256,
    pub ommers_hash:         H256,
    pub beneficiary:         Address,
    pub state_root:          H256,
    pub transactions_root:   H256,
    pub receipts_root:       H256,
    #[serde(with = "BigArray")]
    pub logs_bloom:          [u8; 256],
    pub difficulty:          U256,
    pub number:              u64,
    pub gas_limit:           u64,
    pub gas_used:            u64,
    pub timestamp:           u64,
    pub extra_data:          Vec<u8>,
    pub prev_randao:         H256,
    pub base_fee_per_gas:    U256,
    pub excess_blob_gas:     Option<u64>,
    pub blob_gas_used:       Option<u64>,
    pub nonce:               u64,
    pub chain_id:            u64,
    pub version:             u8,
}

impl BlockHeader {
    pub fn hash(&self) -> H256 {
        H256::from_slice(&Keccak256::digest(&self.rlp_encode()))
    }

    pub fn genesis(chain_id: u64, state_root: H256, timestamp: u64) -> Self {
        Self {
            parent_hash: H256::ZERO, ommers_hash: EMPTY_UNCLE_HASH,
            beneficiary: Address::ZERO, state_root,
            transactions_root: EMPTY_TRIE_HASH, receipts_root: EMPTY_TRIE_HASH,
            logs_bloom: [0u8; 256], difficulty: U256::ZERO, number: 0,
            gas_limit: 30_000_000, gas_used: 0, timestamp,
            extra_data: b"zbx-genesis-v0.1".to_vec(),
            prev_randao: H256::ZERO,
            base_fee_per_gas: U256::from_u64(1_000_000_000),
            excess_blob_gas: Some(0), blob_gas_used: Some(0),
            nonce: 0, chain_id, version: 1,
        }
    }

    /// Canonical encoding of all header fields for hashing.
    ///
    /// ## BLK-HASH-02 (2026-05-16)
    ///
    /// All 19 fields are now included. Variable-length fields (`extra_data`,
    /// `excess_blob_gas`, `blob_gas_used`) use an explicit length prefix or
    /// presence tag so no two distinct headers produce the same byte sequence.
    ///
    /// Field order mirrors the Ethereum yellow-paper RLP field order for the
    /// fields that overlap, with ZBX/EIP-4844 extensions appended.
    pub fn rlp_encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(768);

        // ── Fixed-width hash / address fields (Ethereum order) ─────────────
        v.extend_from_slice(&self.parent_hash.0);                 // 32
        v.extend_from_slice(&self.ommers_hash.0);                 // 32  ← added
        v.extend_from_slice(&self.beneficiary.0);                 // 20  ← added
        v.extend_from_slice(&self.state_root.0);                  // 32
        v.extend_from_slice(&self.transactions_root.0);           // 32
        v.extend_from_slice(&self.receipts_root.0);               // 32

        // ── BLK-HASH-02: logs_bloom ────────────────────────────────────────
        v.extend_from_slice(&self.logs_bloom);                    // 256 ← added

        // ── BLK-HASH-02: difficulty ────────────────────────────────────────
        v.extend_from_slice(&self.difficulty.to_be_bytes());      // 32  ← added

        // ── Scalar fields ──────────────────────────────────────────────────
        v.extend_from_slice(&self.number.to_be_bytes());          // 8
        v.extend_from_slice(&self.gas_limit.to_be_bytes());       // 8
        v.extend_from_slice(&self.gas_used.to_be_bytes());        // 8
        v.extend_from_slice(&self.timestamp.to_be_bytes());       // 8

        // ── BLK-HASH-02: length-prefixed extra_data ────────────────────────
        v.extend_from_slice(&(self.extra_data.len() as u32).to_be_bytes()); // 4 ← added
        v.extend_from_slice(&self.extra_data);                              //   ← added

        // ── BLK-HASH-02: prev_randao (VRF seed) ───────────────────────────
        v.extend_from_slice(&self.prev_randao.0);                 // 32  ← added

        // ── Base fee (32-byte U256 big-endian) ─────────────────────────────
        v.extend_from_slice(&self.base_fee_per_gas.to_be_bytes()); // 32

        // ── Chain / version ────────────────────────────────────────────────
        v.extend_from_slice(&self.chain_id.to_be_bytes());        // 8

        // ── BLK-HASH-02: EIP-4844 blob fields — presence-tagged ───────────
        // None encodes as 0x00; Some(x) encodes as 0x01 || x (8 bytes BE).
        // A single presence byte prevents None from aliasing with Some(0).
        match self.excess_blob_gas {
            None    => v.push(0u8),
            Some(g) => { v.push(1u8); v.extend_from_slice(&g.to_be_bytes()); }
        }
        match self.blob_gas_used {
            None    => v.push(0u8),
            Some(g) => { v.push(1u8); v.extend_from_slice(&g.to_be_bytes()); }
        }

        // ── BLK-HASH-02: nonce + version ──────────────────────────────────
        v.extend_from_slice(&self.nonce.to_be_bytes());           // 8  ← added
        v.push(self.version);                                     // 1  ← added

        v
    }

    pub fn is_genesis(&self) -> bool { self.number == 0 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockSeal {
    pub validator:  Address,
    #[serde(with = "BigArray")]
    pub signature:  [u8; 65],
    pub sealed_at:  u64,
}

/// Keccak256 of RLP([]) — the canonical uncle list hash for PoS blocks
/// (always zero uncles). Value matches Ethereum's `types.EmptyUncleHash`.
pub const EMPTY_UNCLE_HASH: H256 = H256([
    0x1d,0xcc,0x4d,0xe8,0xde,0xc7,0x5d,0x7a,
    0xab,0x85,0xb5,0x67,0xb6,0xcc,0xd4,0x1a,
    0xd3,0x12,0x45,0x1b,0x94,0x8a,0x74,0x13,
    0xf0,0xa1,0x42,0xfd,0x40,0xd4,0x93,0x47,
]);

/// Keccak256 of RLP(b"") — the canonical empty trie root.
/// Value matches Ethereum's `types.EmptyRootHash` / `common.Hash`.
pub const EMPTY_TRIE_HASH: H256 = H256([
    0x56,0xe8,0x1f,0x17,0x1b,0xcc,0x55,0xa6,
    0xff,0x83,0x45,0xe6,0x92,0xc0,0xf8,0x6e,
    0x5b,0x48,0xe0,0x1b,0x99,0x6c,0xad,0xc0,
    0x01,0x62,0x2f,0xb5,0xe3,0x63,0xb4,0x21,
]);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(chain_id: u64, number: u64) -> BlockHeader {
        BlockHeader {
            parent_hash:      H256::ZERO,
            ommers_hash:      EMPTY_UNCLE_HASH,
            beneficiary:      Address::ZERO,
            state_root:       H256::ZERO,
            transactions_root: EMPTY_TRIE_HASH,
            receipts_root:    EMPTY_TRIE_HASH,
            logs_bloom:       [0u8; 256],
            difficulty:       U256::ZERO,
            number,
            gas_limit:        30_000_000,
            gas_used:         0,
            timestamp:        1_700_000_000 + number,
            extra_data:       b"zbx".to_vec(),
            prev_randao:      H256::ZERO,
            base_fee_per_gas: U256::from_u64(1_000_000_000),
            excess_blob_gas:  Some(0),
            blob_gas_used:    Some(0),
            nonce:            0,
            chain_id,
            version:          1,
        }
    }

    #[test]
    fn hash_is_deterministic() {
        let h = sample(8989, 1);
        assert_eq!(h.hash(), h.hash());
    }

    #[test]
    fn hash_differs_on_beneficiary() {
        let mut h1 = sample(8989, 1);
        let mut h2 = h1.clone();
        h1.beneficiary = Address([0xAAu8; 20]);
        h2.beneficiary = Address([0xBBu8; 20]);
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-02: different beneficiary must produce different hash");
    }

    #[test]
    fn hash_differs_on_logs_bloom() {
        let mut h1 = sample(8989, 1);
        let mut h2 = h1.clone();
        h2.logs_bloom[0] = 0xFF;
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-02: different logs_bloom must produce different hash");
    }

    #[test]
    fn hash_differs_on_extra_data_length() {
        let mut h1 = sample(8989, 1);
        let mut h2 = h1.clone();
        h1.extra_data = vec![0x41, 0x42];
        h2.extra_data = vec![0x41];
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-02: different extra_data must not collide");
    }

    #[test]
    fn hash_differs_on_blob_gas_present_vs_absent() {
        let mut h1 = sample(8989, 1);
        let mut h2 = h1.clone();
        h1.excess_blob_gas = None;
        h2.excess_blob_gas = Some(0);
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-02: None vs Some(0) excess_blob_gas must not collide");
    }

    #[test]
    fn hash_differs_on_version() {
        let mut h1 = sample(8989, 1);
        let mut h2 = h1.clone();
        h1.version = 1;
        h2.version = 2;
        assert_ne!(h1.hash(), h2.hash(),
            "BLK-HASH-02: different version must produce different hash");
    }

    #[test]
    fn genesis_hash_is_stable() {
        let state_root = H256([0xABu8; 32]);
        let g = BlockHeader::genesis(8989, state_root, 1_700_000_000);
        let h1 = g.hash();
        let h2 = g.hash();
        assert_eq!(h1, h2, "genesis hash must be stable");
    }
}
