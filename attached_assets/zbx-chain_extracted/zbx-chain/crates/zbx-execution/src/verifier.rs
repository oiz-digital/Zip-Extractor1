//! Block header and body verification.
//!
//! ## N-04 fix (2026-05-05) — 4 missing verifier checks
//!
//! The previous `BlockVerifier` only verified `transactions_root`.  The
//! four checks below were absent, meaning a proposer could commit arbitrary
//! `state_root`, `receipts_root`, `logs_bloom`, or `gas_used > gas_limit`
//! values without any verifier catching the inconsistency.
//!
//! Added:
//!   1. `verify_state_root`     — post-execution state_root matches header.
//!   2. `verify_receipts_root`  — post-execution receipts MPT matches header.
//!   3. `verify_logs_bloom`     — post-execution bloom filter matches header.
//!   4. `verify_gas_limit_bound` — gas_used ≤ gas_limit (protocol invariant).

use crate::error::ExecutionError;
use zbx_types::block::{Block, BlockHeader};
use zbx_types::H256;
use zbx_crypto::mpt::transactions_root_mpt;

pub struct BlockVerifier;

impl BlockVerifier {
    /// Verify all header fields against the parent header.
    pub fn verify_header(
        header: &BlockHeader,
        parent: &BlockHeader,
    ) -> Result<(), ExecutionError> {
        header
            .validate_against_parent(parent)
            .map_err(|e| ExecutionError::Validation(e.to_string()))
    }

    /// Verify the transactions root matches the block body using Ethereum MPT.
    ///
    /// S7-PROD1 CLOSED: uses `zbx_crypto::mpt::transactions_root_mpt` (full
    /// Ethereum MPT) instead of the previous binary Keccak256 Merkle tree,
    /// so that SPV inclusion proofs are verifiable against ZBX Chain headers.
    ///
    /// ## BLK-TX-01 fix (2026-05-05) — TX root consistency
    ///
    /// The previous implementation used `tx.hash` (the full signed transaction
    /// hash = `keccak256(signing_hash || sig_bytes)`) as the Merkle leaf.
    /// `BlockBody::compute_tx_root()` on the producer side has no access to
    /// the ECDSA signature, so it computed `signing_hash` (unsigned fields only).
    /// The two sides always disagreed → every non-empty block failed verification.
    ///
    /// Fix: use `tx.tx.signing_hash().0` here so verifier and producer both
    /// commit to the **unsigned** canonical field encoding and agree by
    /// construction.  See `zbx_block::body` BLK-TX-01 for the matching change.
    ///
    /// Security note (S7-CR7): empty block must produce the Ethereum empty-trie
    /// root (`keccak256(0x80)`) — NOT an arbitrary value. `transactions_root_mpt`
    /// always returns the canonical empty root for an empty hash slice, so a
    /// proposer cannot insert a malicious value in `transactions_root` for an
    /// empty block.
    pub fn verify_transactions_root(block: &Block) -> Result<(), ExecutionError> {
        let hashes: Vec<[u8; 32]> = block
            .body
            .transactions
            .iter()
            .map(|tx| tx.tx.signing_hash().0)
            .collect();
        let computed = transactions_root_mpt(&hashes);
        if computed != block.header.transactions_root.0 {
            return Err(ExecutionError::Validation(
                "transactions_root mismatch".into()
            ));
        }
        Ok(())
    }

    /// Verify gas_used matches the sum of receipts.
    pub fn verify_gas_used(
        block: &Block,
        actual_gas: u64,
    ) -> Result<(), ExecutionError> {
        if block.header.gas_used != actual_gas {
            return Err(ExecutionError::Validation(format!(
                "gas_used mismatch: header says {}, execution used {}",
                block.header.gas_used, actual_gas
            )));
        }
        Ok(())
    }

    /// N-04 fix check 1: Verify the post-execution state root matches the
    /// value committed in the block header.
    ///
    /// The caller must supply `computed_state_root` from the execution engine
    /// (i.e. the MPT root after applying all transactions).  Without this check
    /// a proposer could commit an arbitrary `state_root` and all validators
    /// would accept it silently.
    pub fn verify_state_root(
        block: &Block,
        computed_state_root: H256,
    ) -> Result<(), ExecutionError> {
        if block.header.state_root != computed_state_root {
            return Err(ExecutionError::Validation(format!(
                "state_root mismatch at height {}: header={:?} computed={:?}",
                block.header.number,
                block.header.state_root.as_bytes(),
                computed_state_root.as_bytes(),
            )));
        }
        Ok(())
    }

    /// N-04 fix check 2: Verify the post-execution receipts root matches the
    /// value committed in the block header.
    ///
    /// The caller must supply `computed_receipts_root` from the execution engine
    /// (keccak256-MPT over all transaction receipts in canonical RLP order).
    /// Without this check a proposer could commit an arbitrary `receipts_root`
    /// — breaking SPV receipt inclusion proofs for light clients.
    pub fn verify_receipts_root(
        block: &Block,
        computed_receipts_root: H256,
    ) -> Result<(), ExecutionError> {
        if block.header.receipts_root != computed_receipts_root {
            return Err(ExecutionError::Validation(format!(
                "receipts_root mismatch at height {}: header={:?} computed={:?}",
                block.header.number,
                block.header.receipts_root.as_bytes(),
                computed_receipts_root.as_bytes(),
            )));
        }
        Ok(())
    }

    /// N-04 fix check 3: Verify the post-execution logs bloom filter matches
    /// the value committed in the block header.
    ///
    /// The caller must supply `computed_logs_bloom` built by OR-ing every log
    /// entry's bloom bits across all executed transactions.  Without this check
    /// bloom-filter queries (used by wallets and indexers to efficiently scan
    /// for events) can silently return false negatives.
    pub fn verify_logs_bloom(
        block: &Block,
        computed_logs_bloom: [u8; 256],
    ) -> Result<(), ExecutionError> {
        if block.header.logs_bloom != computed_logs_bloom {
            return Err(ExecutionError::Validation(format!(
                "logs_bloom mismatch at height {}",
                block.header.number,
            )));
        }
        Ok(())
    }

    /// N-04 fix check 4: Verify `gas_used ≤ gas_limit` (EIP-1559 protocol
    /// invariant).
    ///
    /// The block producer enforces this before sealing, but the verifier must
    /// independently check it so that a byzantine proposer cannot publish a
    /// block with `gas_used > gas_limit` and have it accepted by honest nodes.
    pub fn verify_gas_limit_bound(block: &Block) -> Result<(), ExecutionError> {
        if block.header.gas_used > block.header.gas_limit {
            return Err(ExecutionError::Validation(format!(
                "gas_used {} exceeds gas_limit {} at height {}",
                block.header.gas_used,
                block.header.gas_limit,
                block.header.number,
            )));
        }
        Ok(())
    }
}
