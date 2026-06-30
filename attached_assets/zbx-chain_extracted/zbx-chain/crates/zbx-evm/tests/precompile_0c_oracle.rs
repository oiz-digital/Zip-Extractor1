//! Task #5 (Precompile 0x0C — Price oracle read): EVM-side tests.
//!
//! Verifies the EVM wrapper `zbx_evm::precompiles::do_price_oracle`
//! and asserts byte-identical results against the ZVM shared body in
//! `zbx_crypto::oracle_state::do_price_oracle`.

use std::collections::HashMap;
use zbx_crypto::keccak::keccak256;
use zbx_crypto::oracle_state::{
    do_price_oracle as shared_oracle, encode_price_e8, encode_timestamp, write_feed,
    OracleStateReader, ORACLE_REGISTRY_ADDRESS, TOTAL_GAS,
};
use zbx_evm::error::EvmError;
use zbx_evm::precompiles::{do_price_oracle, is_precompile};
use zbx_types::address::Address;

#[derive(Default)]
struct InMemReader(HashMap<([u8; 20], [u8; 32]), [u8; 32]>);
impl OracleStateReader for InMemReader {
    fn read_slot(&self, addr: &[u8; 20], slot: &[u8; 32]) -> [u8; 32] {
        self.0.get(&(*addr, *slot)).copied().unwrap_or([0u8; 32])
    }
}

fn sym(name: &str) -> [u8; 32] {
    let mut h = [0u8; 32];
    h.copy_from_slice(keccak256(name.as_bytes()).as_bytes());
    h
}

fn seed(r: &mut InMemReader, name: &str, p: u128, t: u64) {
    write_feed(&sym(name), encode_price_e8(p), encode_timestamp(t), |a, slot, v| {
        r.0.insert((*a, slot), v);
    });
}

#[test]
fn evm_is_precompile_includes_0c() {
    let mut a = [0u8; 20];
    a[19] = 0x0C;
    assert!(is_precompile(&Address(a)));
    a[19] = 0x0D;
    assert!(!is_precompile(&Address(a)));
}

#[test]
fn evm_known_feed_returns_real_layout() {
    let mut r = InMemReader::default();
    seed(&mut r, "ETH/USD", 3_500_00_000_000, 1_700_000_500);
    let (out, gas) = do_price_oracle(&sym("ETH/USD"), 100_000, &r).unwrap();
    assert_eq!(gas, TOTAL_GAS);
    assert_eq!(&out[0..32], &encode_price_e8(3_500_00_000_000)[..]);
    assert_eq!(&out[32..64], &encode_timestamp(1_700_000_500)[..]);
}

#[test]
fn evm_unknown_feed_returns_zeros() {
    let r = InMemReader::default();
    let (out, _) = do_price_oracle(&sym("UNK/UNK"), 100_000, &r).unwrap();
    assert_eq!(out, vec![0u8; 64]);
}

#[test]
fn evm_oog_maps_to_evm_oog() {
    let r = InMemReader::default();
    let err = do_price_oracle(&sym("BTC/USD"), TOTAL_GAS - 1, &r).unwrap_err();
    assert!(matches!(err, EvmError::OutOfGas));
}

#[test]
fn evm_bad_input_length() {
    let r = InMemReader::default();
    let err = do_price_oracle(&[0u8; 32 + 1], 100_000, &r).unwrap_err();
    assert!(matches!(err, EvmError::Precompile(_)));
}

#[test]
fn evm_byte_identical_to_shared_body() {
    let mut r = InMemReader::default();
    for (n, p, t) in [
        ("BTC/USD",  67_500_00_000_000u128, 1_700_000_000u64),
        ("ETH/USD",  3_500_00_000_000,      1_700_000_001),
        ("USDT/USD", 1_00_000_000,          1_700_000_002),
    ] {
        seed(&mut r, n, p, t);
    }
    for n in ["BTC/USD", "ETH/USD", "USDT/USD", "MISSING"] {
        let s = sym(n);
        let evm = do_price_oracle(&s, 50_000, &r).unwrap();
        let shared = shared_oracle(&s, 50_000, &r).unwrap();
        assert_eq!(evm, shared, "{n}: EVM wrapper drift from shared body");
    }
}

#[test]
fn evm_registry_address_is_consensus_critical() {
    // If the precompile ever started reading from a different address,
    // every node that upgraded would fork. Lock the address explicitly.
    assert_eq!(ORACLE_REGISTRY_ADDRESS[19], 0xCC);
    assert!(ORACLE_REGISTRY_ADDRESS[..19].iter().all(|&b| b == 0));
}
