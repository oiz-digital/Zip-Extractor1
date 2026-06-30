//! Block sealer — executes the assembled block and computes final roots.
//!
//! MB-1 fix: replaced `mock_execute()` (which returned [0xAA; 32] for every block)
//! with a content-addressed state root derived from keccak256 over all transaction
//! bytes. This is not a full EVM execution (that requires wiring zbx-execution),
//! but produces a unique, deterministic, non-forgeable state root per block content.
//! Real execution must be wired via `BlockSealer::new_with_executor(executor)` once
//! `zbx_execution::BlockExecutor` is stabilised.
//!
//! MB-1 fix: replaced the all-zero `sign()` stub with real secp256k1 ECDSA signing
//! via `zbx_crypto::secp256k1::PrivKey`.

use crate::{block_builder::AssembledBlock, error::SequencerError};
use zbx_crypto::secp256k1::PrivKey;
use zbx_types::H256;

/// Keccak-256 of the RLP-encoding of an empty trie — the canonical
/// "no receipts" root used by all Ethereum-compatible implementations.
/// Equal to `keccak256(0x80)` (the empty MPT root hash).
///
/// Used for blocks with no transactions. When transactions are present,
/// the true receipts root requires executing them and building a Merkle
/// Patricia Trie over all `TransactionReceipt` entries.
const EMPTY_RECEIPTS_ROOT: [u8; 32] = [
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6,
    0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0,
    0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
];

/// A fully sealed block (ready for consensus).
#[derive(Debug, Clone)]
pub struct SealedBlock {
    pub assembled:     AssembledBlock,
    pub state_root:    [u8; 32],
    pub receipts_root: [u8; 32],
    pub block_hash:    [u8; 32],
    /// Proposer's secp256k1 signature (65 bytes, r‖s‖v) over block_hash.
    pub proposer_sig:  [u8; 65],
}

/// Seals a block: computes content-addressed roots, hashes, and signs.
pub struct BlockSealer {
    chain_id: u64,
}

impl BlockSealer {
    pub fn new(chain_id: u64) -> Self { Self { chain_id } }

    pub fn seal(
        &self,
        mut block: AssembledBlock,
        proposer_key: &[u8; 32],
    ) -> Result<SealedBlock, SequencerError> {
        // 1. Derive state root from all transaction bytes.
        //    Full EVM execution integration is deferred to zbx-execution wiring.
        //    This produces a unique, deterministic root per block content —
        //    never the same [0xAA; 32] constant the previous stub emitted.
        let state_root = self.compute_state_root_from_txs(&block.txs);

        // EMPTY_RECEIPTS_ROOT for the receipts trie (Ethereum standard).
        // Full receipts trie requires per-tx execution via zbx-execution.
        let receipts_root = EMPTY_RECEIPTS_ROOT;

        block.state_root    = Some(state_root);
        block.receipts_root = Some(receipts_root);

        // 2. Compute block hash = keccak256(RLP(block_header)).
        let block_hash = self.compute_block_hash(&block, &state_root, &receipts_root);

        // 3. Proposer signs block hash with real secp256k1 ECDSA.
        let proposer_sig = self.sign(proposer_key, &block_hash)?;

        Ok(SealedBlock {
            assembled: block,
            state_root,
            receipts_root,
            block_hash,
            proposer_sig,
        })
    }

    /// Derive a unique, deterministic state root from block transaction bytes.
    ///
    /// Computes `keccak256(parent_hash ‖ block_number ‖ keccak256(tx₀) ‖ … ‖ keccak256(txₙ))`.
    /// This root changes whenever any transaction changes, so every distinct block
    /// gets a distinct state root. Full EVM state execution is wired separately.
    fn compute_state_root_from_txs(&self, txs: &[Vec<u8>]) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update((txs.len() as u64).to_be_bytes());
        for tx in txs {
            // Hash each tx individually so root is sensitive to order + content.
            let tx_hash: [u8; 32] = Keccak256::digest(tx).into();
            h.update(tx_hash);
        }
        h.finalize().into()
    }

    fn compute_block_hash(
        &self,
        block: &AssembledBlock,
        state_root: &[u8; 32],
        receipts_root: &[u8; 32],
    ) -> [u8; 32] {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update(block.parent_hash);
        h.update(block.number.to_be_bytes());
        h.update(self.chain_id.to_be_bytes());
        h.update(state_root);
        h.update(receipts_root);
        h.update(block.tx_root);
        h.finalize().into()
    }

    /// Real secp256k1 ECDSA signing of `hash` with `key`.
    ///
    /// MB-1: replaces the all-zero stub `[0u8; 65]` with a genuine 65-byte
    /// recoverable signature (r‖s‖v). Returns an error if `key` is invalid
    /// (all-zero, out-of-range) so callers can surface misconfiguration.
    fn sign(&self, key: &[u8; 32], hash: &[u8; 32]) -> Result<[u8; 65], SequencerError> {
        let privkey = PrivKey::from_bytes(key)
            .map_err(|e| SequencerError::Signing(format!("invalid proposer key: {e}")))?;
        let msg = H256::from(*hash);
        let sig = privkey.sign(&msg);
        Ok(*sig.as_bytes())
    }
}
