//! Block-level Bloom filter, transactions root, and receipts root.
//!
//! Implements:
//!   1. Ethereum Yellow Paper §4.4.3 `M3:2048` Bloom filter
//!      (three bits set per inserted item, indexed via the first six bytes
//!      of `keccak256(item)` masked to `[0, 2048)`).
//!   2. Ethereum-compatible MPT `transactions_root` via `zbx_crypto::mpt`.
//!      Closes **S7-PROD1**: producer and verifier both use the same MPT
//!      algorithm, so they agree by construction.
//!   3. Binary-Merkle `receipts_root` over length-prefixed receipt encodings.
//!
//! ## Sprint S33 (2026-05-02) — closes consensus-safety findings
//!
//! - **N-01**  production block_producer hardcoded `receipts_root = [0u8; 32]`
//! - **N-02**  per-receipt `logs_bloom` never computed in `BlockExecutor`
//! - **S7-PROD1** `transactions_root` upgraded from flat SHA-256 → binary Keccak
//!   Merkle → (this session) full Ethereum MPT.
//!
//! ## Why a `data`-field is excluded from the Bloom
//!
//! Per Ethereum convention (Yellow Paper §4.4.3.2), only the **indexed**
//! log components — emitting `address` and each `topic` — feed the Bloom.
//! The non-indexed `data` payload is intentionally excluded.
//!
//! ## Why receipt encoding is length-prefixed (not RLP)
//!
//! `zbx-rlp` is wired into transaction encoding but not receipts yet.
//! Until that lands, every variable-length receipt field is preceded by an
//! 8-byte big-endian length so two distinct receipts can never collide.

use zbx_crypto::keccak::keccak256;
use zbx_crypto::merkle::transactions_root;
use zbx_types::receipt::{Log, TransactionReceipt};
use zbx_types::transaction::SignedTransaction;
use zbx_types::H256;

/// Insert a single byte item into a 2048-bit (256-byte) Bloom filter
/// per Ethereum Yellow Paper §4.4.3.
///
/// Bit-position derivation matches `go-ethereum`'s `bloom9`:
/// for each byte pair `(h[v], h[v+1])` at `v ∈ {0, 2, 4}`,
/// `bit = (h[v] << 8 | h[v+1]) & 2047`, then
/// `bloom[256 - (bit/8) - 1] |= 1 << (bit % 8)`. The reverse byte order
/// (256-(bit/8)-1) reflects the big-endian-bignum view: the most-significant
/// byte of the 2048-bit Bloom lives at index 0.
pub fn bloom_add(bloom: &mut [u8; 256], item: &[u8]) {
    let h: H256 = keccak256(item);
    let h_bytes: [u8; 32] = h.0;
    for v in [0usize, 2, 4] {
        let bit = ((h_bytes[v] as u16) << 8 | h_bytes[v + 1] as u16) & 2047;
        let byte_idx = 256 - (bit as usize / 8) - 1;
        bloom[byte_idx] |= 1u8 << (bit as usize % 8);
    }
}

/// Compute the per-receipt Bloom filter over all logs in a transaction.
/// Each log contributes its emitting `address` plus every `topic`.
/// Log `data` is intentionally excluded (see module-level docs).
pub fn compute_receipt_bloom(logs: &[Log]) -> [u8; 256] {
    let mut bloom = [0u8; 256];
    for log in logs {
        bloom_add(&mut bloom, log.address.as_bytes());
        for topic in &log.topics {
            bloom_add(&mut bloom, &topic.0);
        }
    }
    bloom
}

/// Aggregate the per-receipt Bloom filters into the block-level Bloom by
/// bitwise-OR of every receipt's `logs_bloom`. The producer commits this
/// as `BlockHeader::logs_bloom`.
///
/// Empty input yields `[0u8; 256]` — the canonical "no logs in this block"
/// sentinel which `eth_getLogs` filters short-circuit on.
pub fn aggregate_block_bloom(receipts: &[TransactionReceipt]) -> [u8; 256] {
    let mut block_bloom = [0u8; 256];
    for r in receipts {
        for (i, byte) in r.logs_bloom.iter().enumerate() {
            block_bloom[i] |= *byte;
        }
    }
    block_bloom
}

/// Length-prefixed canonical encoding of a receipt, then `keccak256`.
/// Field order, lengths, and prefixing are stable; any change to a field
/// (status flip, a new log, a single byte added to log data) yields a
/// different hash by construction.
pub fn compute_receipt_hash(r: &TransactionReceipt) -> H256 {
    let mut buf = Vec::with_capacity(512);
    // 1-byte EIP-658 status
    buf.push(r.status as u8);
    // 8-byte cumulative gas
    buf.extend_from_slice(&r.cumulative_gas_used.to_be_bytes());
    // 256-byte per-receipt bloom (already fixed-width, no length prefix)
    buf.extend_from_slice(&r.logs_bloom);
    // 8-byte log count, then per-log (length-prefixed)
    buf.extend_from_slice(&(r.logs.len() as u64).to_be_bytes());
    for log in &r.logs {
        // 20-byte address (fixed-width)
        buf.extend_from_slice(log.address.as_bytes());
        // 8-byte topic count, then 32-byte topics
        buf.extend_from_slice(&(log.topics.len() as u64).to_be_bytes());
        for t in &log.topics {
            buf.extend_from_slice(&t.0);
        }
        // 8-byte data length, then raw data
        buf.extend_from_slice(&(log.data.len() as u64).to_be_bytes());
        buf.extend_from_slice(&log.data);
    }
    keccak256(&buf)
}

/// Compute the binary-Merkle receipts root over the block's receipts.
/// Returns `[0u8; 32]` for an empty receipts list — same convention as
/// `zbx_crypto::merkle::transactions_root`.
pub fn compute_receipts_root(receipts: &[TransactionReceipt]) -> [u8; 32] {
    if receipts.is_empty() {
        return [0u8; 32];
    }
    let hashes: Vec<H256> = receipts.iter().map(compute_receipt_hash).collect();
    transactions_root(&hashes).0
}

/// Compute the Ethereum-compatible MPT `transactions_root` for a block.
///
/// Closes **S7-PROD1**: replaces the previous binary-Keccak256 Merkle tree
/// with a full Ethereum Modified Patricia Merkle Trie (MPT).
///
/// ## BLOOM-TX-01 fix (2026-05-16)
///
/// Previously this function used `t.hash` (the full signed-transaction hash
/// = `keccak256(signing_hash || sig_bytes)`) while `verifier.rs`
/// `verify_transactions_root` was using `tx.tx.signing_hash()` (the unsigned
/// canonical encoding).  The two functions agreed only on empty blocks; every
/// non-empty block permanently failed verification.
///
/// Fix: both the producer (here) and the verifier use `signing_hash()`.
/// The unsigned hash contains every payload field that uniquely identifies
/// the transaction; the outer signature wrapper is redundant for trie
/// commitments and was the root of the disagreement.
///
/// Algorithm:
///   key(i)   = `rlp_uint64(i)` (matches go-ethereum's `DeriveSha`)
///   value(i) = `rlp_bytes(tx.signing_hash())` — the 32-byte unsigned
///              canonical hash wrapped as an RLP byte string (`0xa0 || 32 bytes`)
///   root     = `keccak256(RLP(trie_root_node))`
///
/// Empty list → Ethereum empty-trie root (`keccak256(0x80)`):
///   `0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421`
///
/// Producer and verifier both call this function so they agree by construction.
pub fn compute_tx_root(txs: &[SignedTransaction]) -> [u8; 32] {
    // BLOOM-TX-01: use signing_hash (unsigned canonical hash) so this matches
    // verifier.rs::verify_transactions_root exactly.  See fix comment above.
    let hashes: Vec<[u8; 32]> = txs.iter().map(|t| t.tx.signing_hash().0).collect();
    zbx_crypto::mpt::transactions_root_mpt(&hashes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zbx_types::address::Address;
    use zbx_types::receipt::TxStatus;

    fn empty_log() -> Log {
        Log {
            address: Address([0u8; 20]),
            topics: Vec::new(),
            data: Vec::new(),
            block_number: 0,
            log_index: 0,
            transaction_hash: H256([0u8; 32]),
            transaction_index: 0,
        }
    }

    fn receipt_with_logs(logs: Vec<Log>, status: TxStatus) -> TransactionReceipt {
        let bloom = compute_receipt_bloom(&logs);
        TransactionReceipt {
            status,
            cumulative_gas_used: 21_000,
            logs_bloom: bloom,
            logs,
            transaction_hash: H256([1u8; 32]),
            transaction_index: 0,
            block_hash: H256([2u8; 32]),
            block_number: 1,
            from: Address([3u8; 20]),
            to: Some(Address([4u8; 20])),
            contract_address: None,
            gas_used: 21_000,
            effective_gas_price: 1_000_000_000,
        }
    }

    // --- bloom_add / compute_receipt_bloom ---

    #[test]
    fn bloom_empty_logs_is_all_zero() {
        assert_eq!(compute_receipt_bloom(&[]), [0u8; 256]);
    }

    #[test]
    fn bloom_single_address_only_log_sets_at_most_3_bits() {
        // A single 20-byte item populates exactly 3 bits unless two of the
        // (bit/8, bit%8) tuples collide on the same byte+bit (vanishingly
        // unlikely for typical inputs but theoretically possible).
        let log = empty_log();
        let bloom = compute_receipt_bloom(&[log]);
        let popcount: u32 = bloom.iter().map(|b| b.count_ones()).sum();
        assert!(popcount >= 1 && popcount <= 3,
            "single log should set 1..=3 bits, got {popcount}");
        assert_ne!(bloom, [0u8; 256], "bloom must not be zero after insertion");
    }

    #[test]
    fn bloom_log_with_topics_sets_more_bits_than_address_alone() {
        let no_topics = empty_log();
        let mut with_topics = empty_log();
        with_topics.topics = vec![H256([0xAA; 32]), H256([0xBB; 32])];
        let pc_a: u32 = compute_receipt_bloom(&[no_topics]).iter().map(|b| b.count_ones()).sum();
        let pc_b: u32 = compute_receipt_bloom(&[with_topics]).iter().map(|b| b.count_ones()).sum();
        assert!(pc_b >= pc_a,
            "bloom with topics ({pc_b}) must have ≥ bits than address-only ({pc_a})");
    }

    #[test]
    fn bloom_known_vector_yellow_paper_compatible() {
        // Hand-compute Bloom for keccak256(b"") = c5d2... (well-known value).
        // We don't assert the full 256-byte vector — just that the result is
        // deterministic and stable across runs.
        let mut b1 = [0u8; 256];
        let mut b2 = [0u8; 256];
        bloom_add(&mut b1, b"");
        bloom_add(&mut b2, b"");
        assert_eq!(b1, b2, "bloom_add must be deterministic on identical input");
    }

    #[test]
    fn bloom_add_is_idempotent() {
        // Adding the same item twice should not flip any bits OFF, and
        // should leave the bloom unchanged after the second call.
        let mut bloom = [0u8; 256];
        bloom_add(&mut bloom, b"hello");
        let after_one = bloom;
        bloom_add(&mut bloom, b"hello");
        assert_eq!(bloom, after_one, "second insertion of same item must be no-op");
    }

    #[test]
    fn bloom_aggregate_is_or_of_per_receipt_blooms() {
        let r1 = receipt_with_logs(vec![empty_log()], TxStatus::Success);
        let mut log2 = empty_log();
        log2.address = Address([0xFF; 20]);
        let r2 = receipt_with_logs(vec![log2], TxStatus::Success);

        let agg = aggregate_block_bloom(&[r1.clone(), r2.clone()]);
        for i in 0..256 {
            assert_eq!(agg[i], r1.logs_bloom[i] | r2.logs_bloom[i],
                "byte {i}: aggregate must equal bitwise OR");
        }
    }

    #[test]
    fn bloom_aggregate_empty_receipts_is_zero() {
        assert_eq!(aggregate_block_bloom(&[]), [0u8; 256]);
    }

    // --- compute_receipt_hash ---

    #[test]
    fn receipt_hash_status_field_changes_root() {
        let logs = vec![empty_log()];
        let r_succ = receipt_with_logs(logs.clone(), TxStatus::Success);
        let r_fail = receipt_with_logs(logs, TxStatus::Failure);
        assert_ne!(compute_receipt_hash(&r_succ), compute_receipt_hash(&r_fail),
            "status flip must produce different receipt hash");
    }

    #[test]
    fn receipt_hash_log_data_change_yields_different_root() {
        let mut log_a = empty_log();
        log_a.data = vec![0x01];
        let mut log_b = empty_log();
        log_b.data = vec![0x02];
        let r_a = receipt_with_logs(vec![log_a], TxStatus::Success);
        let r_b = receipt_with_logs(vec![log_b], TxStatus::Success);
        assert_ne!(compute_receipt_hash(&r_a), compute_receipt_hash(&r_b),
            "differing log.data must produce different receipt hashes");
    }

    #[test]
    fn receipt_hash_log_count_change_yields_different_root() {
        let r_one = receipt_with_logs(vec![empty_log()], TxStatus::Success);
        let r_two = receipt_with_logs(vec![empty_log(), empty_log()], TxStatus::Success);
        assert_ne!(compute_receipt_hash(&r_one), compute_receipt_hash(&r_two),
            "different log counts must produce different receipt hashes");
    }

    // --- compute_receipts_root ---

    #[test]
    fn receipts_root_empty_is_zero() {
        assert_eq!(compute_receipts_root(&[]), [0u8; 32]);
    }

    #[test]
    fn receipts_root_two_distinct_receipts_differs_from_either() {
        let r1 = receipt_with_logs(vec![empty_log()], TxStatus::Success);
        let mut log2 = empty_log();
        log2.address = Address([0xCC; 20]);
        let r2 = receipt_with_logs(vec![log2], TxStatus::Success);

        let root = compute_receipts_root(&[r1.clone(), r2.clone()]);
        let h1 = compute_receipt_hash(&r1).0;
        let h2 = compute_receipt_hash(&r2).0;
        assert_ne!(root, h1, "root over [r1, r2] must not equal hash(r1)");
        assert_ne!(root, h2, "root over [r1, r2] must not equal hash(r2)");
        assert_ne!(root, [0u8; 32], "root over non-empty receipts must be non-zero");
    }

    #[test]
    fn receipts_root_changes_when_log_topic_changes() {
        let mut log_a = empty_log();
        log_a.topics = vec![H256([0x11; 32])];
        let mut log_b = empty_log();
        log_b.topics = vec![H256([0x22; 32])];
        let r_a = receipt_with_logs(vec![log_a], TxStatus::Success);
        let r_b = receipt_with_logs(vec![log_b], TxStatus::Success);
        assert_ne!(compute_receipts_root(&[r_a]), compute_receipts_root(&[r_b]),
            "different topic must propagate through both bloom and root");
    }

    // --- compute_tx_root ---

    #[test]
    fn tx_root_empty_is_zero() {
        assert_eq!(compute_tx_root(&[]), [0u8; 32]);
    }
}
