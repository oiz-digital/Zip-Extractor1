//! Fuzz target: MPT TrieNode::decode panic-safety + round-trip.
//!
//! Added 2026-05-09 (Pass-9) as defense-in-depth on the surface that
//! Pass-8 just fixed: the Branch / Extension RLP decoder was producing
//! `RlpDecode("expected string, got list")` on inline children, and the
//! underlying `Rlp::item_length` was unchecked for short-form lengths
//! (would panic on truncated input). This fuzz target keeps both bugs
//! from regressing.
//!
//! Invariants:
//!   1. `TrieNode::decode(arbitrary_bytes)` MUST NOT panic — only Ok/Err.
//!   2. If decode succeeds, `node.encode()` then `decode()` again MUST round-trip.
//!   3. The re-encoded bytes MUST be canonical (decode → encode is idempotent).
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_trie_node_decode -- -max_total_time=120
#![no_main]

use libfuzzer_sys::fuzz_target;
use zbx_trie::TrieNode;

fuzz_target!(|data: &[u8]| {
    // Invariant 1: decode is panic-safe on arbitrary bytes.
    let decoded = std::panic::catch_unwind(|| TrieNode::decode(data));
    assert!(
        decoded.is_ok(),
        "TrieNode::decode panicked on input len={} first16={:?}",
        data.len(),
        &data[..data.len().min(16)],
    );

    if let Ok(Ok(node)) = decoded {
        // Invariant 2: encode never panics on a successfully-decoded node.
        let re = std::panic::catch_unwind(|| node.encode());
        assert!(re.is_ok(), "TrieNode::encode panicked after successful decode");
        let re_bytes = re.unwrap();

        // Invariant 2 cont.: re-decode of the canonical encoding must succeed.
        let round = std::panic::catch_unwind(|| TrieNode::decode(&re_bytes));
        assert!(round.is_ok(), "round-trip decode panicked");
        let node2 = round.unwrap().expect("round-trip decode failed");

        // Invariant 3: encode is idempotent — encode(decode(encode(x))) == encode(x).
        // This catches non-canonical encoding bugs like the Pass-8 `81 80` vs `80`
        // issue where a "successful" round-trip masked a wire-format divergence.
        let re2 = node2.encode();
        assert_eq!(
            re_bytes, re2,
            "encode is not idempotent — non-canonical encoding regression",
        );
    }
});
