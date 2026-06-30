//! Fuzz target: feed completely arbitrary bytes to RLP decoder.
//! Goal: decoder must NEVER panic on any input, only return Ok or Err.
//!
//! Critical for consensus safety: if a malicious peer sends bad RLP,
//! the node must not crash — it must reject with an error.
//!
//! Run:
//!   cargo +nightly fuzz run fuzz_rlp_decode_arbitrary -- -max_total_time=120
#![no_main]

use libfuzzer_sys::fuzz_target;
use zbx_rlp::{decode, decode_list};

fuzz_target!(|data: &[u8]| {
    // Invariant 1: decode never panics on arbitrary bytes
    let result = std::panic::catch_unwind(|| {
        let _ = decode(data);
    });
    assert!(
        result.is_ok(),
        "RLP decode panicked on input: {:?}", data
    );

    // Invariant 2: decode_list never panics on arbitrary bytes
    let result2 = std::panic::catch_unwind(|| {
        let _ = decode_list(data);
    });
    assert!(
        result2.is_ok(),
        "RLP decode_list panicked on input: {:?}", data
    );

    // Invariant 3: if decode succeeds, re-encode must not panic
    if let Ok(item) = decode(data) {
        let result3 = std::panic::catch_unwind(|| {
            let _ = zbx_rlp::encode(&item);
        });
        assert!(
            result3.is_ok(),
            "RLP encode panicked on decoded item from input: {:?}", data
        );
    }
});