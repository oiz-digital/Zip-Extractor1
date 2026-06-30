//! Property-based tests for the zbx-rlp encoder/decoder.
//!
//! Pass-8 (S38-TRIE-REGRESSION follow-up): the previous version of this
//! file was an aspirational stub referencing a non-existent `RlpItem`
//! enum and free `encode`/`decode` functions; it never compiled. This
//! rewrite exercises the real `RlpStream` / `Rlp` API and asserts the
//! canonical RLP invariants the trie layer depends on.
//!
//! Run: `cargo test --test proptest_rlp -p zbx-rlp`.

use proptest::prelude::*;
use zbx_rlp::{Rlp, RlpStream};

/// Encode a flat list of byte-strings via `RlpStream`.
fn encode_string_list(items: &[Vec<u8>]) -> Vec<u8> {
    let mut s = RlpStream::new_list(items.len());
    for it in items {
        s.append(it.as_slice());
    }
    s.out()
}

/// Encode a single byte-string at top level.
fn encode_string(bytes: &[u8]) -> Vec<u8> {
    let mut s = RlpStream::new();
    s.append(bytes);
    s.out()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Empty bytes encodes to canonical `0x80`.
    #[test]
    fn prop_empty_bytes_is_80(_dummy in Just(())) {
        prop_assert_eq!(encode_string(&[]), vec![0x80u8]);
    }

    /// Empty list encodes to canonical `0xC0`.
    #[test]
    fn prop_empty_list_is_c0(_dummy in Just(())) {
        let s = RlpStream::new_list(0);
        prop_assert_eq!(s.out(), vec![0xC0u8]);
    }

    /// Single byte < 0x80 encodes as itself; >= 0x80 encodes as `0x81 b`.
    #[test]
    fn prop_single_byte_encoding(b in any::<u8>()) {
        let encoded = encode_string(&[b]);
        if b < 0x80 {
            prop_assert_eq!(encoded, vec![b]);
        } else {
            prop_assert_eq!(encoded, vec![0x81u8, b]);
        }
    }

    /// Encoding any byte string is deterministic.
    #[test]
    fn prop_encode_deterministic(data in prop::collection::vec(any::<u8>(), 0..=128usize)) {
        prop_assert_eq!(encode_string(&data), encode_string(&data));
    }

    /// `Rlp::at(i)` over a list of byte-strings round-trips: every
    /// item read back equals the original. Exercises `item_length`
    /// for short, medium, and long string variants.
    #[test]
    fn prop_string_list_roundtrip(
        items in prop::collection::vec(
            prop::collection::vec(any::<u8>(), 0..=80usize),
            0..=8usize)
    ) {
        let encoded = encode_string_list(&items);
        let rlp = Rlp::new(&encoded);
        prop_assert_eq!(rlp.item_count().unwrap(), items.len());
        for (i, expected) in items.iter().enumerate() {
            let got: Vec<u8> = rlp.val_at(i).unwrap();
            prop_assert_eq!(&got, expected);
        }
    }

    /// `is_list()` / `is_data()` correctly classify the top byte for
    /// the entire single-byte universe. Trie decoder depends on this
    /// to dispatch inline children vs string slots (S38 fix).
    #[test]
    fn prop_is_list_vs_is_data_dispatch(b in any::<u8>()) {
        let buf = [b];
        let rlp = Rlp::new(&buf);
        let is_list = rlp.is_list();
        let is_data = rlp.is_data();
        prop_assert_eq!(is_list, b >= 0xc0);
        prop_assert_eq!(is_data, b < 0xc0);
        prop_assert!(is_list ^ is_data, "is_list and is_data must be mutually exclusive");
    }

    /// Decoding arbitrary bytes never panics. `as_bytes` / `at` /
    /// `item_count` should return `Result`, never abort. Hardens the
    /// RLP layer against malformed P2P / RPC input.
    #[test]
    fn prop_decode_no_panic(data in prop::collection::vec(any::<u8>(), 0..=256usize)) {
        let result = std::panic::catch_unwind(|| {
            let rlp = Rlp::new(&data);
            let _ = rlp.item_count();
            let _ = rlp.as_bytes();
            for i in 0..4 { let _ = rlp.at(i); }
        });
        prop_assert!(result.is_ok(), "RLP decode panicked on: {:?}", data);
    }

    /// List length encoding is monotone: list of items is at least as
    /// long as the sum of the individually-encoded items.
    #[test]
    fn prop_list_len_monotone(
        items in prop::collection::vec(
            prop::collection::vec(any::<u8>(), 0..=32usize),
            0..=6usize)
    ) {
        let element_total: usize = items.iter().map(|i| encode_string(i).len()).sum();
        let list_encoded = encode_string_list(&items);
        prop_assert!(list_encoded.len() >= element_total);
    }
}
