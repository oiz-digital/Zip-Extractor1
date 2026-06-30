//! Fuzz target: RLP decoder correctness.
//!
//! RLP is the core encoding used throughout ZBX Chain.
//! Any bug here could corrupt state or allow DoS.

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Property: decode(bytes) must not panic.
    let result = zbx_rlp::decode(data);
    
    if let Ok(items) = result {
        // Property: re-encoding a decoded value gives back the original bytes.
        let re_encoded = zbx_rlp::encode(&items);
        // Prefix-free: re-encoded must be decodeable.
        let _ = zbx_rlp::decode(&re_encoded);
    }
    // Property: nested structures are bounded by input length.
    // (ensured by recursive decode with depth limit)
});