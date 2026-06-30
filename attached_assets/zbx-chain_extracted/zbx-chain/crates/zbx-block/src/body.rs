//! Block body — transactions and uncle headers.
//!
//! `compute_tx_root()` uses the Ethereum-compatible Modified Patricia Merkle
//! Trie (MPT) from `zbx_crypto::mpt`. This closes S7-PROD1: the root matches
//! go-ethereum's `types.DeriveSha()` algorithm so Ethereum SPV inclusion
//! proofs are verifiable against ZBX Chain block headers.
//!
//! Key(i)   = `rlp_uint64(i)` (Ethereum integer RLP).
//! Value(i) = `signing_hash(tx_i)` — the canonical 32-byte hash of the
//!            **unsigned** transaction fields in canonical field order:
//!            `tx_type || chain_id || nonce || max_priority_fee || max_fee ||
//!             gas_limit || to || value || data`.
//!
//! ## BLK-TX-01 fix (2026-05-05) — TX root consistency
//!
//! The previous implementation hashed transaction fields in the wrong order
//! (`gas_limit` before `max_fee/max_priority_fee`) and `verifier.rs` used
//! `SignedTransaction.hash` (which includes the ECDSA signature bytes) as the
//! leaf hash.  A block produced by the builder and then verified by the
//! executor would always disagree on `transactions_root`.
//!
//! Fix:
//! * `body.rs` now hashes fields in canonical `signing_hash()` order.
//! * `verifier.rs` now calls `tx.tx.signing_hash()` instead of `tx.hash`,
//!   so both sides commit to the **unsigned** tx content and agree by
//!   construction.
//!
//! Empty body → `EMPTY_TRIE_HASH` = `keccak256(0x80)`.

use serde_big_array::BigArray;
use serde::{Deserialize, Serialize};
use zbx_primitives::H256;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockBody {
    pub transactions: Vec<zbx_tx::Transaction>,
    pub ommers:       Vec<crate::header::BlockHeader>,
    pub blob_sidecars: Vec<BlobSidecar>,
}

impl BlockBody {
    pub fn new(transactions: Vec<zbx_tx::Transaction>) -> Self {
        Self { transactions, ommers: vec![], blob_sidecars: vec![] }
    }

    pub fn tx_count(&self) -> usize { self.transactions.len() }

    /// Compute the Ethereum-compatible MPT transactions root.
    ///
    /// ## BLK-TX-01 (2026-05-05): canonical signing_hash field order
    ///
    /// Each transaction leaf is `keccak256` of the **unsigned** fields in the
    /// same order as `zbx_types::transaction::Transaction::signing_hash()`:
    ///   `tx_type || chain_id || nonce || max_priority_fee || max_fee ||
    ///    gas_limit || to (20 bytes, zero-padded if None) || value (32 BE) || data`
    ///
    /// This matches what `verifier.rs::verify_transactions_root()` computes
    /// via `tx.tx.signing_hash()` so producer and verifier always agree.
    ///
    /// An empty transaction list returns `EMPTY_TRIE_HASH`
    /// (`0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421`),
    /// matching go-ethereum's empty-trie constant.
    pub fn compute_tx_root(&self) -> H256 {
        if self.transactions.is_empty() {
            return crate::header::EMPTY_TRIE_HASH;
        }
        use sha3::{Digest, Keccak256};
        let hashes: Vec<[u8; 32]> = self
            .transactions
            .iter()
            .map(|tx| {
                let mut h = Keccak256::new();
                // Canonical signing_hash field order (BLK-TX-01):
                // tx_type, chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value, data
                h.update(&[tx.tx_type as u8]);
                h.update(&tx.chain_id.to_be_bytes());
                h.update(&tx.nonce.to_be_bytes());
                h.update(&tx.max_priority_fee.to_be_bytes());
                h.update(&tx.max_fee_per_gas.to_be_bytes());
                h.update(&tx.gas_limit.to_be_bytes());
                h.update(tx.to.as_ref().map(|a| a.as_slice()).unwrap_or(&[0u8; 20]));
                // tx.value is u128 — encode as 32-byte big-endian (EVM word).
                let mut value_be = [0u8; 32];
                value_be[16..].copy_from_slice(&tx.value.to_be_bytes());
                h.update(&value_be);
                h.update(&tx.data);
                h.finalize().into()
            })
            .collect();
        let root_bytes = zbx_crypto::mpt::transactions_root_mpt(&hashes);
        H256(root_bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobSidecar {
    pub blob:       Vec<u8>,
    #[serde(with = "BigArray")]
    pub commitment: [u8; 48],
    #[serde(with = "BigArray")]
    pub proof:      [u8; 48],
    pub tx_hash:    H256,
    pub blob_index: u32,
}
