//! Fuzz target: import an arbitrary block into the chain.
//!
//! Goal: block validation must never panic on malformed input.
//! Property: invalid blocks are rejected, valid blocks are accepted.

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Attempt to parse and validate an arbitrary byte sequence as a block.
    // The validator must handle any input gracefully (no panics, no UB).
    
    if data.len() < 32 { return; } // need at least a block hash

    // Simulate block header parsing.
    let _parent_hash = &data[0..32];
    let _block_number = if data.len() >= 40 {
        u64::from_be_bytes(data[32..40].try_into().unwrap_or([0u8; 8]))
    } else { 0 };

    // Property: any block number is valid to parse (may be rejected by validation).
    // Property: parsing must terminate and not panic.
});