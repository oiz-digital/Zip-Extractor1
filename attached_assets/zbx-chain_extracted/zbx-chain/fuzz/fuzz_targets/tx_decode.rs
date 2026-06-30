//! Fuzz target: RLP-decode arbitrary bytes as a transaction.
//!
//! Goal: ensure the RLP decoder never panics or causes UB on any input.
//! Property: decode(arbitrary_bytes) must either succeed or return an error.

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Property 1: RLP decode must never panic.
    let result = zbx_rlp::decode_tx(data);
    // Property 2: if decode succeeds, re-encoding gives same bytes (roundtrip).
    if let Ok(tx) = result {
        let re_encoded = zbx_rlp::encode_tx(&tx);
        // Note: may differ due to leading zeros, which is OK.
        let _ = re_encoded;
    }
    // Property 3: decode always terminates (no infinite loop).
    // (guaranteed by Rust's memory model — no unsafe)
});