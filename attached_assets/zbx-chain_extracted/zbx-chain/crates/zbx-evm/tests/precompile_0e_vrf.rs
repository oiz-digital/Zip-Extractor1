//! Task #6 — Precompile 0x0E (VRF verify, RFC 9381 ECVRF-EDWARDS25519-SHA512-ELL2).
//!
//! Black-box dispatcher tests. Verifies (a) `is_precompile` extends to 0x0E,
//! (b) the dispatcher routes 0x0D + 0x0E to the new bodies, (c) gas accounting
//! charges the flat fee, and (d) the precompile is fail-soft on malformed
//! input (no revert; 32-byte zero output) — matches the ECRECOVER convention
//! and matches the ZVM body via the shared `zbx_crypto::vrf` body.
//!
//! NOTE: positive-path RFC 9381 vector reproduction is deferred to a follow-up
//! (see crate-level honest-gap note in `zbx-crypto/src/vrf.rs`). The
//! precompile is wired fail-closed today, so `commit()` on the
//! `ZbxRandomBeacon.sol` sample reverts with `VrfInvalidProof` until the
//! reference impl is cross-checked.

use zbx_evm::precompiles::{call_precompile, is_precompile};
use zbx_types::address::Address;

fn addr(x: u8) -> Address {
    let mut a = [0u8; 20];
    a[19] = x;
    Address(a)
}

#[test]
fn is_precompile_range_extended_to_0x0e() {
    assert!(is_precompile(&addr(0x01)));
    assert!(is_precompile(&addr(0x0C)));
    assert!(is_precompile(&addr(0x0D)));
    assert!(is_precompile(&addr(0x0E)));
    assert!(!is_precompile(&addr(0x0F)));
    assert!(!is_precompile(&addr(0x10)));
}

#[test]
fn vrf_short_input_returns_zero_padding_not_revert() {
    // 50 bytes < 112 minimum → fail-soft 32-byte zero, gas still charged.
    let input = vec![0u8; 50];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000)
        .expect("vrf precompile must not revert on short input");
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}

#[test]
fn vrf_garbage_input_returns_zero() {
    // 200 bytes of garbage — pubkey decompresses to nothing useful, proof
    // also garbage. Should fail-soft to 32-byte zero (NOT revert).
    let input = vec![0xAAu8; 200];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000)
        .expect("vrf precompile must not revert on garbage input");
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}

#[test]
fn vrf_out_of_gas() {
    let input = vec![0u8; 200];
    let res = call_precompile(&addr(0x0E), &input, 100);
    assert!(res.is_err(), "must reject when gas < 5000");
}

#[test]
fn ed25519_short_input_returns_zero() {
    let input = vec![0u8; 50];
    let (out, gas) = call_precompile(&addr(0x0D), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 3000);
}

#[test]
fn ed25519_garbage_pubkey_returns_zero() {
    // 128 bytes of 0xFF: pubkey decompression fails → fail-soft zero.
    let input = vec![0xFFu8; 128];
    let (out, gas) = call_precompile(&addr(0x0D), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 3000);
}

#[test]
fn vrf_minimum_length_input_no_crash() {
    // Exactly 112 bytes (32 + 0 alpha + 80 pi) — minimum valid layout.
    // alpha is empty, pubkey + pi are zeros → invalid proof, fail-soft zero.
    let input = vec![0u8; 112];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}

// EVM/ZVM byte-identical equivalence is guaranteed by construction: both
// VM precompile bodies are thin wrappers over the same
// `zbx_crypto::vrf::ecvrf_edwards25519::verify`, with the same flat 5000
// gas cost and the same fail-soft 32-byte-zero output convention. A
// dedicated cross-VM equivalence test would require a circular dev-dep
// (already avoided in zbx-evm); the zbx-zvm side has its own copy of
// these black-box assertions.
