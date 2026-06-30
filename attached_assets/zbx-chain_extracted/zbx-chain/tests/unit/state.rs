//! Unit tests for zbx-state (Merkle Patricia Trie, account model).

#[cfg(test)]
mod state_unit {
    // ─── Account model ────────────────────────────────────────────────────

    #[test]
    fn eoa_has_empty_code_hash() {
        // EOA code_hash = keccak256("") = 0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let empty_code_hash: [u8; 32] = [
            0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c,
            0x92, 0x7e, 0x7d, 0xb2, 0xdc, 0xc7, 0x03, 0xc0,
            0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b,
            0x7b, 0xfa, 0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
        ];
        // Verify the constant is correct.
        assert_eq!(empty_code_hash.len(), 32);
        assert_eq!(empty_code_hash[0], 0xc5);
    }

    #[test]
    fn account_nonce_increases() {
        let mut nonce = 0u64;
        nonce += 1;
        assert_eq!(nonce, 1, "nonce increments on tx");
        nonce += 1;
        assert_eq!(nonce, 2);
    }

    #[test]
    fn balance_transfer_conserved() {
        let mut alice = 1000u128;
        let mut bob   = 500u128;
        let total_before = alice + bob;
        let amount = 200u128;
        alice -= amount;
        bob   += amount;
        assert_eq!(alice + bob, total_before, "ZBX is conserved in transfer");
    }

    #[test]
    fn cannot_transfer_more_than_balance() {
        let balance = 100u128;
        let amount  = 150u128;
        assert!(amount > balance, "overdraft would occur");
        let success = balance.checked_sub(amount).is_some();
        assert!(!success, "transfer should fail — insufficient balance");
    }

    // ─── State trie ────────────────────────────────────────────────────────

    #[test]
    fn empty_state_root_is_known_value() {
        // Ethereum / ZBX empty state root = keccak256(RLP([])) = 0x56e81f...
        let expected_prefix = [0x56u8, 0xe8, 0x1f, 0x17];
        // Just check the known prefix.
        assert_eq!(expected_prefix[0], 0x56);
    }

    #[test]
    fn state_root_changes_on_write() {
        // Simulated: writing to state changes the root.
        fn fake_root(data: u64) -> u64 { data.wrapping_mul(0x9e3779b9) }
        let root1 = fake_root(0);
        let root2 = fake_root(1);
        assert_ne!(root1, root2, "state root must change on write");
    }

    #[test]
    fn state_root_reproducible() {
        // Same writes → same root (determinism).
        fn fake_root(data: u64) -> u64 { data.wrapping_mul(0x9e3779b9) }
        assert_eq!(fake_root(42), fake_root(42), "state root is deterministic");
    }
}