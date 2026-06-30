//! Task #6 — Precompile 0x0E (VRF verify) — ZVM dispatcher tests.
//!
//! Mirrors `zbx-evm/tests/precompile_0e_vrf.rs`. See the EVM file for the
//! cross-VM byte-identity rationale; both precompile bodies are thin
//! wrappers around `zbx_crypto::vrf::ecvrf_edwards25519::verify`.

use zbx_zvm::precompiles::call_precompile;

fn addr(x: u8) -> [u8; 20] {
    let mut a = [0u8; 20];
    a[19] = x;
    a
}

#[test]
fn vrf_short_input_returns_zero() {
    let input = vec![0u8; 50];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}

#[test]
fn vrf_garbage_input_returns_zero() {
    let input = vec![0xAAu8; 200];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}

#[test]
fn vrf_out_of_gas() {
    let input = vec![0u8; 200];
    assert!(call_precompile(&addr(0x0E), &input, 100).is_err());
}

#[test]
fn vrf_minimum_length_input_no_crash() {
    let input = vec![0u8; 112];
    let (out, gas) = call_precompile(&addr(0x0E), &input, 100_000).unwrap();
    assert_eq!(out, vec![0u8; 32]);
    assert_eq!(gas, 5000);
}
