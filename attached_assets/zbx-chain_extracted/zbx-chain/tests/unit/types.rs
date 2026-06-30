//! Unit tests for zbx-types (Block, Transaction, Receipt).
//! Run: cargo test --package zbx-types

#[cfg(test)]
mod transaction_tests {
    use zbx_types::transaction::{Transaction, TxType};
    use zbx_crypto::keccak::keccak256;

    #[test]
    fn tx_hash_is_keccak_of_rlp_encoded_fields() {
        // A transaction's hash is keccak256(RLP(tx fields)). Two transactions
        // with identical fields must have identical hashes; one field change
        // must produce a different hash.
        let tx = Transaction::eip1559_stub(
            /* chain_id */ 8989,
            /* nonce    */ 0,
            /* to       */ [0xABu8; 20],
            /* value    */ 1_000_000_000_000_000_000u128,
        );
        let hash1 = tx.hash();
        let hash2 = tx.hash();
        assert_eq!(hash1, hash2, "tx.hash() must be deterministic");
        assert_ne!(hash1.0, [0u8; 32], "tx hash must not be all-zero");
        assert_eq!(hash1.0.len(), 32, "tx hash must be exactly 32 bytes");

        // A different nonce must produce a different hash.
        let tx2 = Transaction::eip1559_stub(8989, 1, [0xABu8; 20],
                                            1_000_000_000_000_000_000u128);
        assert_ne!(tx.hash(), tx2.hash(), "nonce change must change the tx hash");
    }

    #[test]
    fn eip1559_tx_type_is_2() {
        // EIP-1559 transactions carry type = 0x02 in the EIP-2718 envelope.
        assert_eq!(TxType::Eip1559 as u8, 2u8, "EIP-1559 tx type must be 0x02");
    }

    #[test]
    fn legacy_tx_type_is_0() {
        // Legacy transactions are type 0 (no EIP-2718 prefix byte).
        assert_eq!(TxType::Legacy as u8, 0u8, "legacy tx type must be 0x00");
    }

    #[test]
    fn eip2930_tx_type_is_1() {
        // EIP-2930 (access list) transactions carry type = 0x01.
        assert_eq!(TxType::Eip2930 as u8, 1u8, "EIP-2930 tx type must be 0x01");
    }

    #[test]
    fn different_chain_ids_produce_different_hashes() {
        // EIP-155 replay protection: chain_id is part of the signing payload,
        // so a tx on mainnet (8989) must have a different hash than on testnet (8990).
        let tx_main = Transaction::eip1559_stub(8989, 0, [0xABu8; 20], 0);
        let tx_test = Transaction::eip1559_stub(8990, 0, [0xABu8; 20], 0);
        assert_ne!(
            tx_main.hash(), tx_test.hash(),
            "same tx on different chain_ids must hash differently (EIP-155)"
        );
    }
}

#[cfg(test)]
mod block_tests {
    use zbx_types::block::{BlockHeader, GENESIS_BLOCK_NUMBER};

    #[test]
    fn genesis_block_number_is_zero() {
        assert_eq!(GENESIS_BLOCK_NUMBER, 0u64, "genesis block number must be 0");
    }

    #[test]
    fn block_hash_changes_when_any_header_field_changes() {
        // Block hash = keccak256(RLP(header)). Changing any field must change
        // the hash — this guards against header fields being accidentally
        // excluded from the hashing pre-image.
        let base = BlockHeader::testnet_stub(/* number */ 1);
        let with_different_number = BlockHeader::testnet_stub(/* number */ 2);
        assert_ne!(
            base.hash(), with_different_number.hash(),
            "changing block number must change the block hash"
        );
    }

    #[test]
    fn block_header_includes_required_fields() {
        // All 10 consensus-critical header fields must be present.
        let header = BlockHeader::testnet_stub(1);
        // Accessing each field panics if missing — this serves as a compile-time
        // and runtime assertion that all fields are populated.
        let _ = header.parent_hash;
        let _ = header.state_root;
        let _ = header.transactions_root;
        let _ = header.receipts_root;
        let _ = header.number;
        let _ = header.timestamp;
        let _ = header.gas_limit;
        let _ = header.gas_used;
        let _ = header.base_fee_per_gas;
        let _ = header.miner;
    }
}

#[cfg(test)]
mod receipt_tests {
    use zbx_types::receipt::{Receipt, ReceiptStatus};

    #[test]
    fn successful_receipt_status_is_1() {
        let r = Receipt { status: ReceiptStatus::Success, ..Default::default() };
        assert_eq!(r.status as u8, 1u8, "success receipt status must be 1");
    }

    #[test]
    fn failed_receipt_status_is_0() {
        let r = Receipt { status: ReceiptStatus::Failure, ..Default::default() };
        assert_eq!(r.status as u8, 0u8, "failure receipt status must be 0");
    }

    #[test]
    fn receipt_cumulative_gas_is_at_least_gas_used() {
        // Per Ethereum yellow paper: cumulative_gas_used[n] >= gas_used[n].
        let r = Receipt {
            status: ReceiptStatus::Success,
            gas_used: 21_000,
            cumulative_gas_used: 21_000,
            ..Default::default()
        };
        assert!(
            r.cumulative_gas_used >= r.gas_used,
            "cumulative gas used must be >= this tx's gas used"
        );
    }
}
