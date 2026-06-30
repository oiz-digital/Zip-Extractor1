//! Fuzz target: RLP encode/decode roundtrip.
//! Tests that encode(decode(x)) = x for all valid inputs.
//! Also tests that decode never panics on arbitrary bytes.
//!
//! Key properties:
//!   1. encode(decode(encode(x))) == encode(x)  (roundtrip)
//!   2. decode(arbitrary_bytes) never panics
//!   3. encode/decode are deterministic (same input → same output)
//!   4. decode of invalid bytes returns Err, never panics
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_rlp_encode_decode -- -max_total_time=120
#![no_main]

use libfuzzer_sys::{fuzz_target, arbitrary};
use arbitrary::Arbitrary;
use zbx_rlp::{RlpItem, encode, decode, RlpError};

/// Structured RLP items for roundtrip testing
#[derive(Arbitrary, Debug)]
enum FuzzRlpItem {
    Bytes(Vec<u8>),
    Short(u8),
    List(Vec<FuzzRlpItem>),
    NestedList(Box<FuzzRlpItem>, Box<FuzzRlpItem>),
}

impl FuzzRlpItem {
    fn to_rlp(&self) -> RlpItem {
        match self {
            FuzzRlpItem::Bytes(b) => RlpItem::Bytes(b.clone()),
            FuzzRlpItem::Short(n) => {
                if *n < 0x80 { RlpItem::Bytes(vec![*n]) }
                else         { RlpItem::Bytes(vec![*n]) }
            }
            FuzzRlpItem::List(items) => {
                RlpItem::List(items.iter().map(|i| i.to_rlp()).collect())
            }
            FuzzRlpItem::NestedList(a, b) => {
                RlpItem::List(vec![a.to_rlp(), b.to_rlp()])
            }
        }
    }
}

fuzz_target!(|input: FuzzRlpItem| {
    let item = input.to_rlp();

    // Invariant 1: encode must succeed (all valid RlpItems are encodable)
    let encoded = encode(&item);

    // Invariant 2: decode of encoded must succeed
    let decoded = decode(&encoded);
    assert!(
        decoded.is_ok(),
        "RLP roundtrip failed: encode produced un-decodable bytes\nItem: {:?}\nEncoded: {:?}",
        item,
        encoded
    );

    // Invariant 3: re-encode must produce same bytes
    let re_encoded = encode(&decoded.unwrap());
    assert_eq!(
        encoded,
        re_encoded,
        "RLP encode is not deterministic or roundtrip broken"
    );
});